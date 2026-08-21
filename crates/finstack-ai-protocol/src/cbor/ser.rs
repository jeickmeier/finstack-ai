//! Serde serializer that builds a [`CanonicalValue`] tree.

use serde::ser::{
    SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};
use serde::{Serialize, Serializer};

use super::value::{CanonicalValue, sort_map};
use crate::error::ProtocolError;
use crate::{
    CANONICAL_ARRAY_MAX_ITEMS, CANONICAL_MAP_MAX_ENTRIES, CANONICAL_NESTING_DEPTH,
    CANONICAL_STRING_MAX_BYTES,
};

/// Serialize `value` into a validated canonical tree.
///
/// # Errors
///
/// Returns limit or profile failures.
pub fn to_canonical<T: Serialize + ?Sized>(value: &T) -> Result<CanonicalValue, ProtocolError> {
    value.serialize(CanonicalSerializer { depth: 0 })
}

#[derive(Clone, Copy)]
struct CanonicalSerializer {
    depth: usize,
}

impl CanonicalSerializer {
    fn nested(self) -> Result<Self, ProtocolError> {
        if self.depth >= CANONICAL_NESTING_DEPTH {
            return Err(ProtocolError::limit(
                "nesting_depth",
                CANONICAL_NESTING_DEPTH,
            ));
        }
        Ok(Self {
            depth: self.depth + 1,
        })
    }
}

impl Serializer for CanonicalSerializer {
    type Ok = CanonicalValue;
    type Error = ProtocolError;
    type SerializeSeq = SerializeVec;
    type SerializeTuple = SerializeVec;
    type SerializeTupleStruct = SerializeVec;
    type SerializeTupleVariant = SerializeNamedVec;
    type SerializeMap = SerializeEntries;
    type SerializeStruct = SerializeEntries;
    type SerializeStructVariant = SerializeNamedEntries;

    fn is_human_readable(&self) -> bool {
        false
    }

    fn serialize_bool(self, value: bool) -> Result<Self::Ok, Self::Error> {
        Ok(CanonicalValue::Bool(value))
    }

    fn serialize_i8(self, value: i8) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_i16(self, value: i16) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_i32(self, value: i32) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_i64(self, value: i64) -> Result<Self::Ok, Self::Error> {
        if value >= 0 {
            Ok(CanonicalValue::Unsigned(value.cast_unsigned()))
        } else {
            Ok(CanonicalValue::Negative((!value).cast_unsigned()))
        }
    }

    fn serialize_i128(self, _value: i128) -> Result<Self::Ok, Self::Error> {
        Err(ProtocolError::invalid("integer_overflow"))
    }

    fn serialize_u8(self, value: u8) -> Result<Self::Ok, Self::Error> {
        Ok(CanonicalValue::Unsigned(u64::from(value)))
    }

    fn serialize_u16(self, value: u16) -> Result<Self::Ok, Self::Error> {
        Ok(CanonicalValue::Unsigned(u64::from(value)))
    }

    fn serialize_u32(self, value: u32) -> Result<Self::Ok, Self::Error> {
        Ok(CanonicalValue::Unsigned(u64::from(value)))
    }

    fn serialize_u64(self, value: u64) -> Result<Self::Ok, Self::Error> {
        Ok(CanonicalValue::Unsigned(value))
    }

    fn serialize_u128(self, _value: u128) -> Result<Self::Ok, Self::Error> {
        Err(ProtocolError::invalid("integer_overflow"))
    }

    fn serialize_f32(self, value: f32) -> Result<Self::Ok, Self::Error> {
        self.serialize_f64(f64::from(value))
    }

    fn serialize_f64(self, value: f64) -> Result<Self::Ok, Self::Error> {
        if !value.is_finite() {
            return Err(ProtocolError::invalid("non_finite_float"));
        }
        Ok(CanonicalValue::Float(value.to_bits()))
    }

    fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
        self.serialize_str(&value.to_string())
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        if value.len() > CANONICAL_STRING_MAX_BYTES {
            return Err(ProtocolError::limit(
                "text_string",
                CANONICAL_STRING_MAX_BYTES,
            ));
        }
        Ok(CanonicalValue::Text(value.to_owned()))
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Self::Ok, Self::Error> {
        if value.len() > CANONICAL_STRING_MAX_BYTES {
            return Err(ProtocolError::limit(
                "byte_string",
                CANONICAL_STRING_MAX_BYTES,
            ));
        }
        Ok(CanonicalValue::Bytes(value.to_vec()))
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(CanonicalValue::Null)
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(CanonicalValue::Null)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(CanonicalValue::Null)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.serialize_str(variant)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        let nested = self.nested()?;
        Ok(CanonicalValue::Map(vec![(
            CanonicalValue::Text(variant.to_owned()),
            value.serialize(nested)?,
        )]))
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        check_array_len(len)?;
        Ok(SerializeVec {
            serializer: self.nested()?,
            expected_len: len,
            items: Vec::new(),
        })
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        check_array_len(Some(len))?;
        Ok(SerializeNamedVec {
            name: variant,
            serializer: self.nested()?.nested()?,
            expected_len: len,
            items: Vec::new(),
        })
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        check_map_len(len)?;
        Ok(SerializeEntries {
            serializer: self.nested()?,
            expected_len: len,
            entries: Vec::new(),
            pending_key: None,
        })
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.serialize_map(Some(len))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        check_map_len(Some(len))?;
        Ok(SerializeNamedEntries {
            name: variant,
            serializer: self.nested()?.nested()?,
            expected_len: len,
            entries: Vec::new(),
        })
    }
}

fn check_array_len(len: Option<usize>) -> Result<(), ProtocolError> {
    if len.is_some_and(|count| count > CANONICAL_ARRAY_MAX_ITEMS) {
        return Err(ProtocolError::limit(
            "array_items",
            CANONICAL_ARRAY_MAX_ITEMS,
        ));
    }
    Ok(())
}

fn check_map_len(len: Option<usize>) -> Result<(), ProtocolError> {
    if len.is_some_and(|count| count > CANONICAL_MAP_MAX_ENTRIES) {
        return Err(ProtocolError::limit(
            "map_entries",
            CANONICAL_MAP_MAX_ENTRIES,
        ));
    }
    Ok(())
}

pub struct SerializeVec {
    serializer: CanonicalSerializer,
    expected_len: Option<usize>,
    items: Vec<CanonicalValue>,
}

impl SerializeSeq for SerializeVec {
    type Ok = CanonicalValue;
    type Error = ProtocolError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        if self.items.len() >= CANONICAL_ARRAY_MAX_ITEMS {
            return Err(ProtocolError::limit(
                "array_items",
                CANONICAL_ARRAY_MAX_ITEMS,
            ));
        }
        self.items.push(value.serialize(self.serializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        check_actual_len(self.expected_len, self.items.len())?;
        Ok(CanonicalValue::Array(self.items))
    }
}

impl SerializeTuple for SerializeVec {
    type Ok = CanonicalValue;
    type Error = ProtocolError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        SerializeSeq::end(self)
    }
}

impl SerializeTupleStruct for SerializeVec {
    type Ok = CanonicalValue;
    type Error = ProtocolError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        SerializeSeq::end(self)
    }
}

pub struct SerializeNamedVec {
    name: &'static str,
    serializer: CanonicalSerializer,
    expected_len: usize,
    items: Vec<CanonicalValue>,
}

impl SerializeTupleVariant for SerializeNamedVec {
    type Ok = CanonicalValue;
    type Error = ProtocolError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        if self.items.len() >= CANONICAL_ARRAY_MAX_ITEMS {
            return Err(ProtocolError::limit(
                "array_items",
                CANONICAL_ARRAY_MAX_ITEMS,
            ));
        }
        self.items.push(value.serialize(self.serializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        check_actual_len(Some(self.expected_len), self.items.len())?;
        Ok(CanonicalValue::Map(vec![(
            CanonicalValue::Text(self.name.to_owned()),
            CanonicalValue::Array(self.items),
        )]))
    }
}

pub struct SerializeEntries {
    serializer: CanonicalSerializer,
    expected_len: Option<usize>,
    entries: Vec<(CanonicalValue, CanonicalValue)>,
    pending_key: Option<CanonicalValue>,
}

impl SerializeMap for SerializeEntries {
    type Ok = CanonicalValue;
    type Error = ProtocolError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Self::Error> {
        if self.pending_key.is_some() {
            return Err(ProtocolError::codec("map key without value"));
        }
        self.pending_key = Some(key.serialize(self.serializer)?);
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        let key = self
            .pending_key
            .take()
            .ok_or_else(|| ProtocolError::codec("map value without key"))?;
        if self.entries.len() >= CANONICAL_MAP_MAX_ENTRIES {
            return Err(ProtocolError::limit(
                "map_entries",
                CANONICAL_MAP_MAX_ENTRIES,
            ));
        }
        self.entries.push((key, value.serialize(self.serializer)?));
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        if self.pending_key.is_some() {
            return Err(ProtocolError::codec("map key without value"));
        }
        check_actual_len(self.expected_len, self.entries.len())?;
        Ok(CanonicalValue::Map(sort_map(self.entries)?))
    }
}

impl SerializeStruct for SerializeEntries {
    type Ok = CanonicalValue;
    type Error = ProtocolError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        SerializeMap::serialize_entry(self, key, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        SerializeMap::end(self)
    }
}

pub struct SerializeNamedEntries {
    name: &'static str,
    serializer: CanonicalSerializer,
    expected_len: usize,
    entries: Vec<(CanonicalValue, CanonicalValue)>,
}

impl SerializeStructVariant for SerializeNamedEntries {
    type Ok = CanonicalValue;
    type Error = ProtocolError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        if self.entries.len() >= CANONICAL_MAP_MAX_ENTRIES {
            return Err(ProtocolError::limit(
                "map_entries",
                CANONICAL_MAP_MAX_ENTRIES,
            ));
        }
        self.entries.push((
            CanonicalValue::Text(key.to_owned()),
            value.serialize(self.serializer)?,
        ));
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        check_actual_len(Some(self.expected_len), self.entries.len())?;
        Ok(CanonicalValue::Map(vec![(
            CanonicalValue::Text(self.name.to_owned()),
            CanonicalValue::Map(sort_map(self.entries)?),
        )]))
    }
}

fn check_actual_len(expected: Option<usize>, actual: usize) -> Result<(), ProtocolError> {
    if expected.is_some_and(|expected| expected != actual) {
        return Err(ProtocolError::codec("collection length hint mismatch"));
    }
    Ok(())
}
