//! Internal bounded collection deserializers for semantic DTOs.

use core::fmt;
use core::marker::PhantomData;
use std::collections::BTreeMap;

use serde::de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

/// V1 maximum items in a semantic array (TDD §6.5).
pub const SEMANTIC_ARRAY_MAX_ITEMS: usize = 4_096;
/// V1 maximum entries in a semantic map (TDD §6.5).
pub const SEMANTIC_MAP_MAX_ENTRIES: usize = 256;

#[derive(Debug)]
pub(crate) struct BoundedVec<T, const MAX: usize>(Vec<T>);

impl<T, const MAX: usize> BoundedVec<T, MAX> {
    pub(crate) fn into_inner(self) -> Vec<T> {
        self.0
    }
}

impl<T, const MAX: usize> Default for BoundedVec<T, MAX> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<'de, T, const MAX: usize> Deserialize<'de> for BoundedVec<T, MAX>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BoundedVecVisitor<T, const MAX: usize>(PhantomData<T>);

        impl<'de, T, const MAX: usize> Visitor<'de> for BoundedVecVisitor<T, MAX>
        where
            T: Deserialize<'de>,
        {
            type Value = BoundedVec<T, MAX>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "an array containing at most {MAX} items")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                if sequence.size_hint().is_some_and(|length| length > MAX) {
                    return Err(de::Error::custom(format_args!(
                        "array item count exceeds max {MAX}"
                    )));
                }
                let capacity = sequence.size_hint().unwrap_or(0).min(MAX);
                let mut values = Vec::with_capacity(capacity);
                while values.len() < MAX {
                    let Some(value) = sequence.next_element()? else {
                        return Ok(BoundedVec(values));
                    };
                    values.push(value);
                }
                if sequence.next_element::<IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom(format_args!(
                        "array item count exceeds max {MAX}"
                    )));
                }
                Ok(BoundedVec(values))
            }
        }

        deserializer.deserialize_seq(BoundedVecVisitor(PhantomData))
    }
}

#[derive(Debug)]
pub(crate) struct BoundedMap<K, V, const MAX: usize>(BTreeMap<K, V>);

impl<K, V, const MAX: usize> BoundedMap<K, V, MAX> {
    pub(crate) fn into_inner(self) -> BTreeMap<K, V> {
        self.0
    }
}

impl<K, V, const MAX: usize> Default for BoundedMap<K, V, MAX> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}

impl<'de, K, V, const MAX: usize> Deserialize<'de> for BoundedMap<K, V, MAX>
where
    K: Deserialize<'de> + Ord,
    V: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BoundedMapVisitor<K, V, const MAX: usize>(PhantomData<(K, V)>);

        impl<'de, K, V, const MAX: usize> Visitor<'de> for BoundedMapVisitor<K, V, MAX>
        where
            K: Deserialize<'de> + Ord,
            V: Deserialize<'de>,
        {
            type Value = BoundedMap<K, V, MAX>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "a map containing at most {MAX} entries")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                if map.size_hint().is_some_and(|length| length > MAX) {
                    return Err(de::Error::custom(format_args!(
                        "map entry count exceeds max {MAX}"
                    )));
                }
                let mut values = BTreeMap::new();
                while values.len() < MAX {
                    let Some((key, value)) = map.next_entry()? else {
                        return Ok(BoundedMap(values));
                    };
                    if values.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate map key"));
                    }
                }
                if map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom(format_args!(
                        "map entry count exceeds max {MAX}"
                    )));
                }
                Ok(BoundedMap(values))
            }
        }

        deserializer.deserialize_map(BoundedMapVisitor(PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use serde::de::value::{MapDeserializer, SeqDeserializer};

    use super::*;

    #[test]
    fn oversized_sequence_hint_is_rejected_before_elements_are_read() {
        let items = core::iter::repeat_with(|| -> serde_json::Value {
            panic!("oversized sequence should be rejected before reading an element")
        })
        .take(5);
        let deserializer = SeqDeserializer::<_, serde_json::Error>::new(items);
        let error = BoundedVec::<serde_json::Value, 4>::deserialize(deserializer)
            .expect_err("oversized sequence");
        assert!(error.to_string().contains("array item count"));
    }

    #[test]
    fn oversized_map_hint_is_rejected_before_entries_are_read() {
        let entries = core::iter::repeat_with(|| -> (serde_json::Value, serde_json::Value) {
            panic!("oversized map should be rejected before reading an entry")
        })
        .take(5);
        let deserializer = MapDeserializer::<_, serde_json::Error>::new(entries);
        let error = BoundedMap::<String, serde_json::Value, 4>::deserialize(deserializer)
            .expect_err("oversized map");
        assert!(error.to_string().contains("map entry count"));
    }
}
