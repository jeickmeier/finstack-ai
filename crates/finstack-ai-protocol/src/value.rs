//! Canonical value tree and RFC 8949 definite-length encoder/decoder.

use crate::error::ProtocolError;
use crate::{
    CANONICAL_ARRAY_MAX_ITEMS, CANONICAL_MAP_MAX_ENTRIES, CANONICAL_NESTING_DEPTH,
    CANONICAL_STRING_MAX_BYTES,
};

/// Validated canonical-CBOR value tree.
#[derive(Debug, Clone, PartialEq)]
pub enum CanonicalValue {
    /// CBOR null.
    Null,
    /// CBOR boolean.
    Bool(bool),
    /// Major-type 0 integer.
    Unsigned(u64),
    /// Major-type 1 integer stored as CBOR `n` where the value is `-1 - n`.
    Negative(u64),
    /// Definite byte string.
    Bytes(Vec<u8>),
    /// Definite text string.
    Text(String),
    /// Definite array.
    Array(Vec<CanonicalValue>),
    /// Definite map with RFC 8949-sorted keys.
    Map(Vec<(CanonicalValue, CanonicalValue)>),
    /// Finite float stored as IEEE-754 binary64 bits, including negative zero.
    Float(u64),
}

impl Eq for CanonicalValue {}

/// Encode one canonical value with definite lengths and shortest forms.
///
/// # Errors
///
/// Returns [`ProtocolError::InvalidCbor`] for non-finite floats.
pub fn encode_value(value: &CanonicalValue) -> Result<Vec<u8>, ProtocolError> {
    let mut out = Vec::new();
    write_value(value, &mut out)?;
    Ok(out)
}

/// Decode one canonical value with v1 ceilings enforced before allocation.
///
/// # Errors
///
/// Returns limit, profile, or trailing-data failures.
pub fn decode_value(bytes: &[u8]) -> Result<CanonicalValue, ProtocolError> {
    let mut decoder = Decoder {
        input: bytes,
        offset: 0,
        depth: 0,
    };
    let value = decoder.item()?;
    if decoder.offset != bytes.len() {
        return Err(ProtocolError::invalid("trailing_cbor"));
    }
    Ok(value)
}

/// Convert a validated tree into `ciborium::value::Value` for the library writer.
#[must_use]
pub fn to_ciborium(value: &CanonicalValue) -> ciborium::value::Value {
    match value {
        CanonicalValue::Null => ciborium::value::Value::Null,
        CanonicalValue::Bool(flag) => ciborium::value::Value::Bool(*flag),
        CanonicalValue::Unsigned(n) => ciborium::value::Value::Integer((*n).into()),
        CanonicalValue::Negative(n) => ciborium::value::Value::from(-1_i128 - i128::from(*n)),
        CanonicalValue::Bytes(bytes) => ciborium::value::Value::Bytes(bytes.clone()),
        CanonicalValue::Text(text) => ciborium::value::Value::Text(text.clone()),
        CanonicalValue::Array(items) => {
            ciborium::value::Value::Array(items.iter().map(to_ciborium).collect())
        }
        CanonicalValue::Map(entries) => ciborium::value::Value::Map(
            entries
                .iter()
                .map(|(key, value)| (to_ciborium(key), to_ciborium(value)))
                .collect(),
        ),
        CanonicalValue::Float(bits) => ciborium::value::Value::Float(f64::from_bits(*bits)),
    }
}

pub(crate) fn sort_map(
    entries: Vec<(CanonicalValue, CanonicalValue)>,
) -> Result<Vec<(CanonicalValue, CanonicalValue)>, ProtocolError> {
    if entries.len() > CANONICAL_MAP_MAX_ENTRIES {
        return Err(ProtocolError::limit(
            "map_entries",
            CANONICAL_MAP_MAX_ENTRIES,
        ));
    }
    let mut keyed = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        let encoded = encode_value(&key)?;
        keyed.push((encoded, key, value));
    }
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    for window in keyed.windows(2) {
        if window[0].0 == window[1].0 {
            return Err(ProtocolError::invalid("duplicate_map_key"));
        }
    }
    Ok(keyed
        .into_iter()
        .map(|(_, key, value)| (key, value))
        .collect())
}

fn write_value(value: &CanonicalValue, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
    match value {
        CanonicalValue::Null => out.push(0xf6),
        CanonicalValue::Bool(false) => out.push(0xf4),
        CanonicalValue::Bool(true) => out.push(0xf5),
        CanonicalValue::Unsigned(n) => write_uint(0, *n, out),
        CanonicalValue::Negative(n) => write_uint(1, *n, out),
        CanonicalValue::Bytes(bytes) => {
            if bytes.len() > CANONICAL_STRING_MAX_BYTES {
                return Err(ProtocolError::limit(
                    "byte_string",
                    CANONICAL_STRING_MAX_BYTES,
                ));
            }
            write_uint(2, u64::try_from(bytes.len()).expect("usize fits u64"), out);
            out.extend_from_slice(bytes);
        }
        CanonicalValue::Text(text) => {
            if text.len() > CANONICAL_STRING_MAX_BYTES {
                return Err(ProtocolError::limit(
                    "text_string",
                    CANONICAL_STRING_MAX_BYTES,
                ));
            }
            write_uint(3, u64::try_from(text.len()).expect("usize fits u64"), out);
            out.extend_from_slice(text.as_bytes());
        }
        CanonicalValue::Array(items) => {
            if items.len() > CANONICAL_ARRAY_MAX_ITEMS {
                return Err(ProtocolError::limit(
                    "array_items",
                    CANONICAL_ARRAY_MAX_ITEMS,
                ));
            }
            write_uint(4, u64::try_from(items.len()).expect("usize fits u64"), out);
            for item in items {
                write_value(item, out)?;
            }
        }
        CanonicalValue::Map(entries) => {
            let entries = sort_map(entries.clone())?;
            write_uint(
                5,
                u64::try_from(entries.len()).expect("usize fits u64"),
                out,
            );
            for (key, value) in entries {
                write_value(&key, out)?;
                write_value(&value, out)?;
            }
        }
        CanonicalValue::Float(bits) => write_float(*bits, out)?,
    }
    Ok(())
}

fn write_uint(major: u8, n: u64, out: &mut Vec<u8>) {
    let major = major << 5;
    if n < 24 {
        out.push(major | u8::try_from(n).expect("n < 24"));
    } else if let Ok(byte) = u8::try_from(n) {
        out.push(major | 0x18);
        out.push(byte);
    } else if let Ok(short) = u16::try_from(n) {
        out.push(major | 0x19);
        out.extend_from_slice(&short.to_be_bytes());
    } else if let Ok(word) = u32::try_from(n) {
        out.push(major | 0x1a);
        out.extend_from_slice(&word.to_be_bytes());
    } else {
        out.push(major | 0x1b);
        out.extend_from_slice(&n.to_be_bytes());
    }
}

fn write_float(bits: u64, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
    let value = f64::from_bits(bits);
    if !value.is_finite() {
        return Err(ProtocolError::invalid("non_finite_float"));
    }
    if value == 0.0 {
        if bits >> 63 == 1 {
            out.extend_from_slice(&[0xf9, 0x80, 0x00]);
        } else {
            out.extend_from_slice(&[0xf9, 0x00, 0x00]);
        }
        return Ok(());
    }
    if let Some(half) = f64_to_f16_exact(value) {
        out.push(0xf9);
        out.extend_from_slice(&half.to_be_bytes());
        return Ok(());
    }
    let single = f64_to_f32_lossy(value);
    if f64::from(single).to_bits() == bits {
        out.push(0xfa);
        out.extend_from_slice(&single.to_bits().to_be_bytes());
        return Ok(());
    }
    out.push(0xfb);
    out.extend_from_slice(&bits.to_be_bytes());
    Ok(())
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "shortest finite float width is selected only when the f32 round-trip is bit-exact"
)]
fn f64_to_f32_lossy(value: f64) -> f32 {
    value as f32
}

fn f64_to_f16_exact(value: f64) -> Option<u16> {
    let bits = value.to_bits();
    let sign = u16::try_from(bits >> 63).ok()? << 15;
    let exp = i32::try_from((bits >> 52) & 0x7ff).ok()?;
    let frac = bits & ((1_u64 << 52) - 1);
    if exp == 0 || exp == 0x7ff {
        return None;
    }
    let unbiased = exp - 1023;
    if !(0..=15).contains(&(unbiased + 14)) {
        return None;
    }
    if frac & ((1_u64 << 42) - 1) != 0 {
        return None;
    }
    let mantissa = u16::try_from(frac >> 42).ok()?;
    let exp16 = u16::try_from(unbiased + 15).ok()?;
    Some(sign | (exp16 << 10) | mantissa)
}

struct Decoder<'a> {
    input: &'a [u8],
    offset: usize,
    depth: usize,
}

impl Decoder<'_> {
    fn item(&mut self) -> Result<CanonicalValue, ProtocolError> {
        let initial = self.read_u8()?;
        let major = initial >> 5;
        let additional = initial & 0x1f;
        match major {
            0 => Ok(CanonicalValue::Unsigned(self.read_uint(additional)?)),
            1 => Ok(CanonicalValue::Negative(self.read_uint(additional)?)),
            2 => {
                let len =
                    self.bounded_len(additional, CANONICAL_STRING_MAX_BYTES, "byte_string")?;
                Ok(CanonicalValue::Bytes(self.read_exact(len)?.to_vec()))
            }
            3 => {
                let len =
                    self.bounded_len(additional, CANONICAL_STRING_MAX_BYTES, "text_string")?;
                let bytes = self.read_exact(len)?;
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| ProtocolError::invalid("invalid_utf8"))?;
                Ok(CanonicalValue::Text(text.to_owned()))
            }
            4 => {
                self.enter_container()?;
                let len = self.bounded_len(additional, CANONICAL_ARRAY_MAX_ITEMS, "array_items")?;
                let mut items = Vec::with_capacity(len);
                for _ in 0..len {
                    items.push(self.item()?);
                }
                self.depth -= 1;
                Ok(CanonicalValue::Array(items))
            }
            5 => {
                self.enter_container()?;
                let len = self.bounded_len(additional, CANONICAL_MAP_MAX_ENTRIES, "map_entries")?;
                let mut entries = Vec::with_capacity(len);
                let mut previous_key = None;
                for _ in 0..len {
                    let key = self.item()?;
                    let encoded_key = encode_value(&key)?;
                    if previous_key
                        .as_ref()
                        .is_some_and(|previous: &Vec<u8>| encoded_key <= *previous)
                    {
                        return Err(ProtocolError::invalid("unsorted_or_duplicate_map_key"));
                    }
                    previous_key = Some(encoded_key);
                    let value = self.item()?;
                    entries.push((key, value));
                }
                self.depth -= 1;
                Ok(CanonicalValue::Map(entries))
            }
            6 => {
                let tag = self.read_uint(additional)?;
                if tag == 2 || tag == 3 {
                    return Err(ProtocolError::invalid("bignum_tag"));
                }
                Err(ProtocolError::invalid("cbor_tag"))
            }
            7 => self.simple(additional),
            _ => Err(ProtocolError::invalid("unknown_major_type")),
        }
    }

    fn enter_container(&mut self) -> Result<(), ProtocolError> {
        if self.depth >= CANONICAL_NESTING_DEPTH {
            return Err(ProtocolError::limit(
                "nesting_depth",
                CANONICAL_NESTING_DEPTH,
            ));
        }
        self.depth += 1;
        Ok(())
    }

    fn simple(&mut self, additional: u8) -> Result<CanonicalValue, ProtocolError> {
        match additional {
            20 => Ok(CanonicalValue::Bool(false)),
            21 => Ok(CanonicalValue::Bool(true)),
            22 => Ok(CanonicalValue::Null),
            25 => {
                let bits = u16::from_be_bytes(self.read_array()?);
                Ok(CanonicalValue::Float(f16_to_f64_bits(bits)?))
            }
            26 => {
                let bits = u32::from_be_bytes(self.read_array()?);
                let value = f32::from_bits(bits);
                if !value.is_finite() {
                    return Err(ProtocolError::invalid("non_finite_float"));
                }
                Ok(CanonicalValue::Float(f64::from(value).to_bits()))
            }
            27 => {
                let bits = u64::from_be_bytes(self.read_array()?);
                if !f64::from_bits(bits).is_finite() {
                    return Err(ProtocolError::invalid("non_finite_float"));
                }
                Ok(CanonicalValue::Float(bits))
            }
            31 => Err(ProtocolError::invalid("indefinite_length")),
            _ => Err(ProtocolError::invalid("unsupported_simple")),
        }
    }

    fn bounded_len(
        &mut self,
        additional: u8,
        max: usize,
        resource: &'static str,
    ) -> Result<usize, ProtocolError> {
        if additional == 31 {
            return Err(ProtocolError::invalid("indefinite_length"));
        }
        let declared = self.read_uint(additional)?;
        let len = usize::try_from(declared).map_err(|_| ProtocolError::limit(resource, max))?;
        if len > max {
            return Err(ProtocolError::limit(resource, max));
        }
        if self.offset.saturating_add(len) > self.input.len() && resource.ends_with("string") {
            return Err(ProtocolError::invalid("declared_length_overrun"));
        }
        Ok(len)
    }

    fn read_uint(&mut self, additional: u8) -> Result<u64, ProtocolError> {
        match additional {
            0..=23 => Ok(u64::from(additional)),
            24 => Ok(u64::from(self.read_u8()?)),
            25 => Ok(u64::from(u16::from_be_bytes(self.read_array()?))),
            26 => Ok(u64::from(u32::from_be_bytes(self.read_array()?))),
            27 => Ok(u64::from_be_bytes(self.read_array()?)),
            31 => Err(ProtocolError::invalid("indefinite_length")),
            _ => Err(ProtocolError::invalid("reserved_additional_info")),
        }
    }

    fn read_u8(&mut self) -> Result<u8, ProtocolError> {
        let byte = *self
            .input
            .get(self.offset)
            .ok_or_else(|| ProtocolError::invalid("truncated_cbor"))?;
        self.offset += 1;
        Ok(byte)
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        let slice = self.read_exact(N)?;
        let mut out = [0_u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    fn read_exact(&mut self, len: usize) -> Result<&[u8], ProtocolError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| ProtocolError::invalid("declared_length_overrun"))?;
        let slice = self
            .input
            .get(self.offset..end)
            .ok_or_else(|| ProtocolError::invalid("declared_length_overrun"))?;
        self.offset = end;
        Ok(slice)
    }
}

fn f16_to_f64_bits(half: u16) -> Result<u64, ProtocolError> {
    let sign = u64::from(half >> 15);
    let exp = (half >> 10) & 0x1f;
    let frac = half & 0x3ff;
    if exp == 0x1f {
        return Err(ProtocolError::invalid("non_finite_float"));
    }
    if exp == 0 {
        if frac == 0 {
            return Ok(sign << 63);
        }
        let value = f64::from(frac) * 2_f64.powi(-24);
        let bits = if sign == 1 {
            (-value).to_bits()
        } else {
            value.to_bits()
        };
        return Ok(bits);
    }
    Ok((sign << 63) | ((u64::from(exp) + 1023 - 15) << 52) | (u64::from(frac) << 42))
}

#[cfg(test)]
mod tests {
    use super::{CanonicalValue, decode_value, encode_value};

    #[test]
    fn unsigned_shortest_form() {
        assert_eq!(
            encode_value(&CanonicalValue::Unsigned(0)).expect("0"),
            [0x00]
        );
        assert_eq!(
            encode_value(&CanonicalValue::Unsigned(23)).expect("23"),
            [0x17]
        );
        assert_eq!(
            encode_value(&CanonicalValue::Unsigned(24)).expect("24"),
            [0x18, 24]
        );
    }

    #[test]
    fn map_order_is_bytewise() {
        let value = CanonicalValue::Map(vec![
            (
                CanonicalValue::Text("b".into()),
                CanonicalValue::Unsigned(1),
            ),
            (
                CanonicalValue::Text("a".into()),
                CanonicalValue::Unsigned(2),
            ),
        ]);
        let encoded = encode_value(&value).expect("encode");
        let decoded = decode_value(&encoded).expect("decode");
        let CanonicalValue::Map(entries) = decoded else {
            panic!("map");
        };
        assert_eq!(entries[0].0, CanonicalValue::Text("a".into()));
        assert_eq!(entries[1].0, CanonicalValue::Text("b".into()));
    }

    #[test]
    fn negative_zero_is_preserved() {
        let bits = (-0.0_f64).to_bits();
        let encoded = encode_value(&CanonicalValue::Float(bits)).expect("encode");
        assert_eq!(encoded, [0xf9, 0x80, 0x00]);
        let CanonicalValue::Float(decoded) = decode_value(&encoded).expect("decode") else {
            panic!("float");
        };
        assert_eq!(decoded, bits);
    }

    #[test]
    fn bignum_tag_is_rejected() {
        let err = decode_value(&[0xc2, 0x41, 0x01]).expect_err("tag 2");
        assert_eq!(err.code(), "bignum_tag");
    }
}
