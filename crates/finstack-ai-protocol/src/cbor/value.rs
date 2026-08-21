//! Canonical value tree and RFC 8949 definite-length encoder/decoder.

use crate::error::ProtocolError;
use crate::{
    CANONICAL_ARRAY_MAX_ITEMS, CANONICAL_ENVELOPE_MAX_BYTES, CANONICAL_MAP_MAX_ENTRIES,
    CANONICAL_NESTING_DEPTH, CANONICAL_STRING_MAX_BYTES,
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
    let mut writer = Writer::new(CANONICAL_ENVELOPE_MAX_BYTES);
    writer.value(value, 0)?;
    Ok(writer.out)
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

struct Writer {
    out: Vec<u8>,
    limit: usize,
}

impl Writer {
    fn new(limit: usize) -> Self {
        Self {
            out: Vec::new(),
            limit,
        }
    }

    fn push(&mut self, byte: u8) -> Result<(), ProtocolError> {
        self.extend(&[byte])
    }

    fn extend(&mut self, bytes: &[u8]) -> Result<(), ProtocolError> {
        let next = self
            .out
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| ProtocolError::limit("canonical_envelope", self.limit))?;
        if next > self.limit {
            return Err(ProtocolError::limit("canonical_envelope", self.limit));
        }
        self.out.extend_from_slice(bytes);
        Ok(())
    }

    fn value(&mut self, value: &CanonicalValue, depth: usize) -> Result<(), ProtocolError> {
        match value {
            CanonicalValue::Null => self.push(0xf6)?,
            CanonicalValue::Bool(false) => self.push(0xf4)?,
            CanonicalValue::Bool(true) => self.push(0xf5)?,
            CanonicalValue::Unsigned(n) => self.uint(0, *n)?,
            CanonicalValue::Negative(n) => self.uint(1, *n)?,
            CanonicalValue::Bytes(bytes) => {
                if bytes.len() > CANONICAL_STRING_MAX_BYTES {
                    return Err(ProtocolError::limit(
                        "byte_string",
                        CANONICAL_STRING_MAX_BYTES,
                    ));
                }
                self.uint(2, usize_to_u64(bytes.len(), "byte_string")?)?;
                self.extend(bytes)?;
            }
            CanonicalValue::Text(text) => {
                if text.len() > CANONICAL_STRING_MAX_BYTES {
                    return Err(ProtocolError::limit(
                        "text_string",
                        CANONICAL_STRING_MAX_BYTES,
                    ));
                }
                self.uint(3, usize_to_u64(text.len(), "text_string")?)?;
                self.extend(text.as_bytes())?;
            }
            CanonicalValue::Array(items) => {
                let nested = enter_depth(depth)?;
                if items.len() > CANONICAL_ARRAY_MAX_ITEMS {
                    return Err(ProtocolError::limit(
                        "array_items",
                        CANONICAL_ARRAY_MAX_ITEMS,
                    ));
                }
                self.uint(4, usize_to_u64(items.len(), "array_items")?)?;
                for item in items {
                    self.value(item, nested)?;
                }
            }
            CanonicalValue::Map(entries) => {
                let nested = enter_depth(depth)?;
                if entries.len() > CANONICAL_MAP_MAX_ENTRIES {
                    return Err(ProtocolError::limit(
                        "map_entries",
                        CANONICAL_MAP_MAX_ENTRIES,
                    ));
                }
                let mut keys = Vec::with_capacity(entries.len());
                for (index, (key, _)) in entries.iter().enumerate() {
                    let mut key_writer = Writer::new(CANONICAL_ENVELOPE_MAX_BYTES);
                    key_writer.value(key, nested)?;
                    keys.push((key_writer.out, index));
                }
                keys.sort_by(|left, right| left.0.cmp(&right.0));
                for window in keys.windows(2) {
                    if window[0].0 == window[1].0 {
                        return Err(ProtocolError::invalid("duplicate_map_key"));
                    }
                }
                self.uint(5, usize_to_u64(entries.len(), "map_entries")?)?;
                for (key_bytes, index) in keys {
                    self.extend(&key_bytes)?;
                    let (_, value) = &entries[index];
                    self.value(value, nested)?;
                }
            }
            CanonicalValue::Float(bits) => self.float(*bits)?,
        }
        Ok(())
    }

    fn uint(&mut self, major: u8, n: u64) -> Result<(), ProtocolError> {
        let major = major << 5;
        if let Ok(byte) = u8::try_from(n) {
            if byte < 24 {
                self.push(major | byte)?;
            } else {
                self.extend(&[major | 0x18, byte])?;
            }
        } else if let Ok(short) = u16::try_from(n) {
            self.push(major | 0x19)?;
            self.extend(&short.to_be_bytes())?;
        } else if let Ok(word) = u32::try_from(n) {
            self.push(major | 0x1a)?;
            self.extend(&word.to_be_bytes())?;
        } else {
            self.push(major | 0x1b)?;
            self.extend(&n.to_be_bytes())?;
        }
        Ok(())
    }

    fn float(&mut self, bits: u64) -> Result<(), ProtocolError> {
        let value = f64::from_bits(bits);
        if !value.is_finite() {
            return Err(ProtocolError::invalid("non_finite_float"));
        }
        if let Some(half) = f64_to_f16_exact(value) {
            self.push(0xf9)?;
            self.extend(&half.to_be_bytes())?;
            return Ok(());
        }
        let single = f64_to_f32_lossy(value);
        if f64::from(single).to_bits() == bits {
            self.push(0xfa)?;
            self.extend(&single.to_bits().to_be_bytes())?;
            return Ok(());
        }
        self.push(0xfb)?;
        self.extend(&bits.to_be_bytes())
    }
}

fn enter_depth(depth: usize) -> Result<usize, ProtocolError> {
    if depth >= CANONICAL_NESTING_DEPTH {
        return Err(ProtocolError::limit(
            "nesting_depth",
            CANONICAL_NESTING_DEPTH,
        ));
    }
    Ok(depth + 1)
}

fn usize_to_u64(len: usize, resource: &'static str) -> Result<u64, ProtocolError> {
    u64::try_from(len).map_err(|_| ProtocolError::limit(resource, usize::MAX))
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
    let half = half::f16::from_f64(value);
    (half.to_f64().to_bits() == value.to_bits()).then_some(half.to_bits())
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
                let mut previous_key: Option<(usize, usize)> = None;
                for _ in 0..len {
                    let key_start = self.offset;
                    let key = self.item()?;
                    let key_end = self.offset;
                    if let Some((previous_start, previous_end)) = previous_key
                        && self.input[key_start..key_end]
                            <= self.input[previous_start..previous_end]
                    {
                        return Err(ProtocolError::invalid("unsorted_or_duplicate_map_key"));
                    }
                    previous_key = Some((key_start, key_end));
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
                let widened = f64::from(value);
                if f64_to_f16_exact(widened).is_some() {
                    return Err(ProtocolError::invalid("non_minimal_float"));
                }
                Ok(CanonicalValue::Float(widened.to_bits()))
            }
            27 => {
                let bits = u64::from_be_bytes(self.read_array()?);
                let value = f64::from_bits(bits);
                if !value.is_finite() {
                    return Err(ProtocolError::invalid("non_finite_float"));
                }
                if f64_to_f16_exact(value).is_some()
                    || f64::from(f64_to_f32_lossy(value)).to_bits() == bits
                {
                    return Err(ProtocolError::invalid("non_minimal_float"));
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
        let value = match additional {
            0..=23 => Ok(u64::from(additional)),
            24 => Ok(u64::from(self.read_u8()?)),
            25 => Ok(u64::from(u16::from_be_bytes(self.read_array()?))),
            26 => Ok(u64::from(u32::from_be_bytes(self.read_array()?))),
            27 => Ok(u64::from_be_bytes(self.read_array()?)),
            31 => Err(ProtocolError::invalid("indefinite_length")),
            _ => Err(ProtocolError::invalid("reserved_additional_info")),
        }?;
        let minimal = match additional {
            24 => value >= 24,
            25 => value > u64::from(u8::MAX),
            26 => value > u64::from(u16::MAX),
            27 => value > u64::from(u32::MAX),
            _ => true,
        };
        if !minimal {
            return Err(ProtocolError::invalid("non_minimal_integer_or_length"));
        }
        Ok(value)
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
    let value = half::f16::from_bits(half);
    if !value.is_finite() {
        return Err(ProtocolError::invalid("non_finite_float"));
    }
    Ok(value.to_f64().to_bits())
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
