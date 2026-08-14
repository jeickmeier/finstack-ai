//! Serde deserializer from a [`CanonicalValue`] tree.

use serde::Deserialize;
use serde::de::{
    DeserializeSeed, Deserializer, EnumAccess, IntoDeserializer, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};

use crate::error::ProtocolError;
use crate::value::CanonicalValue;

/// Deserialize `T` from a validated canonical tree.
///
/// # Errors
///
/// Returns typed decode failures.
pub fn from_canonical<'de, T: Deserialize<'de>>(value: CanonicalValue) -> Result<T, ProtocolError> {
    T::deserialize(ValueDeserializer { value })
}

struct ValueDeserializer {
    value: CanonicalValue,
}

impl<'de> Deserializer<'de> for ValueDeserializer {
    type Error = ProtocolError;

    fn is_human_readable(&self) -> bool {
        false
    }

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.value {
            CanonicalValue::Null => visitor.visit_unit(),
            CanonicalValue::Bool(flag) => visitor.visit_bool(flag),
            CanonicalValue::Unsigned(n) => visitor.visit_u64(n),
            CanonicalValue::Negative(n) => {
                let value = -1_i128 - i128::from(n);
                i64::try_from(value).map_or_else(
                    |_| Err(ProtocolError::invalid("integer_overflow")),
                    |signed| visitor.visit_i64(signed),
                )
            }
            CanonicalValue::Bytes(bytes) => visitor.visit_byte_buf(bytes),
            CanonicalValue::Text(text) => visitor.visit_string(text),
            CanonicalValue::Array(items) => visitor.visit_seq(SeqDeserializer { items, index: 0 }),
            CanonicalValue::Map(entries) => visitor.visit_map(MapDeserializer {
                entries: entries.into_iter(),
                value: None,
            }),
            CanonicalValue::Float(bits) => visitor.visit_f64(f64::from_bits(bits)),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.value {
            CanonicalValue::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self.value {
            CanonicalValue::Text(variant) => visitor.visit_enum(variant.into_deserializer()),
            CanonicalValue::Map(mut entries) if entries.len() == 1 => {
                let (key, value) = entries.pop().expect("one entry");
                let CanonicalValue::Text(variant) = key else {
                    return Err(ProtocolError::codec("enum variant key must be text"));
                };
                visitor.visit_enum(EnumDeserializer { variant, value })
            }
            _ => Err(ProtocolError::codec("invalid enum encoding")),
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }
}

struct SeqDeserializer {
    items: Vec<CanonicalValue>,
    index: usize,
}

impl<'de> SeqAccess<'de> for SeqDeserializer {
    type Error = ProtocolError;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        if self.index >= self.items.len() {
            return Ok(None);
        }
        let value = self.items[self.index].clone();
        self.index += 1;
        seed.deserialize(ValueDeserializer { value }).map(Some)
    }
}

struct MapDeserializer {
    entries: std::vec::IntoIter<(CanonicalValue, CanonicalValue)>,
    value: Option<CanonicalValue>,
}

impl<'de> MapAccess<'de> for MapDeserializer {
    type Error = ProtocolError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        match self.entries.next() {
            Some((key, value)) => {
                self.value = Some(value);
                seed.deserialize(ValueDeserializer { value: key }).map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        let value = self
            .value
            .take()
            .ok_or_else(|| ProtocolError::codec("map value without key"))?;
        seed.deserialize(ValueDeserializer { value })
    }
}

struct EnumDeserializer {
    variant: String,
    value: CanonicalValue,
}

impl<'de> EnumAccess<'de> for EnumDeserializer {
    type Error = ProtocolError;
    type Variant = ValueDeserializer;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Self::Error> {
        let variant = seed.deserialize(self.variant.into_deserializer())?;
        Ok((variant, ValueDeserializer { value: self.value }))
    }
}

impl<'de> VariantAccess<'de> for ValueDeserializer {
    type Error = ProtocolError;

    fn unit_variant(self) -> Result<(), Self::Error> {
        match self.value {
            CanonicalValue::Null => Ok(()),
            _ => Err(ProtocolError::codec("expected unit variant")),
        }
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<T::Value, Self::Error> {
        seed.deserialize(self)
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_map(visitor)
    }
}
