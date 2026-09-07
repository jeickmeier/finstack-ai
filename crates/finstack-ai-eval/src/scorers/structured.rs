//! RFC 6901 field grading without converting decimal JSON numbers to binary floats.
use super::{ParsedNumber, ToleranceBands, identity};
use crate::scoring::{implement_scorer, target_invalid, value};
use crate::{EvalError, Score, ScoreContext, ScoreMicros, ScorerId};
use serde::{
    Deserialize, Serialize,
    de::{MapAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

/// Field-specific comparison rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "ppm",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FieldTolerance {
    /// Semantic JSON equality, with exact decimal comparison for numbers.
    Exact,
    /// Relative numeric tolerance in parts per million, including numeric strings.
    NumericPpm(u64),
}
/// One weighted RFC 6901 field. Missing answer fields receive zero credit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldSpec {
    /// JSON pointer; empty addresses the whole document. At most 32 segments.
    pub pointer: Arc<str>,
    /// Exact or unit-aware numeric comparison.
    pub tolerance: FieldTolerance,
    /// Positive relative importance for the aggregate score.
    pub weight: u32,
}

/// Emit one score per selected field and an integer weighted `aggregate` score.
pub struct StructuredFieldScorer {
    id: ScorerId,
    version: u32,
    fields: Vec<FieldSpec>,
}
impl StructuredFieldScorer {
    /// Configure between one and 128 unique fields with positive weights.
    /// # Errors
    /// Rejects invalid IDs, versions, JSON pointers, duplicate fields and zero weights.
    pub fn new(
        id: impl Into<ScorerId>,
        version: u32,
        fields: Vec<FieldSpec>,
    ) -> Result<Self, EvalError> {
        if fields.is_empty() || fields.len() > 128 {
            return Err(crate::error::invalid());
        }
        let mut seen = BTreeSet::new();
        for field in &fields {
            segments(&field.pointer)?;
            if field.weight == 0 || !seen.insert(&field.pointer) {
                return Err(crate::error::invalid());
            }
        }
        Ok(Self {
            id: identity(id, version)?,
            version,
            fields,
        })
    }
    fn evaluate(&self, ctx: &ScoreContext<'_>) -> Result<Vec<Score>, EvalError> {
        let target = raw(&ctx.sample.target)?;
        let answer = ctx
            .output
            .and_then(finstack_ai::AgentRunOutput::structured_json)
            .map(|json| raw(json.as_str()))
            .transpose()?;
        let mut scores = Vec::with_capacity(self.fields.len() + 1);
        let mut total = 0_u128;
        let mut weight = 0_u128;
        for field in &self.fields {
            let expected = select(&target, &field.pointer)?.ok_or_else(target_invalid)?;
            let actual = answer
                .as_ref()
                .map(|answer| select(answer, &field.pointer))
                .transpose()?
                .flatten();
            let grade = match actual {
                None => ScoreMicros::ZERO,
                Some(actual) => field_grade(&actual, &expected, &field.tolerance)?,
            };
            total += u128::from(grade.get()) * u128::from(field.weight);
            weight += u128::from(field.weight);
            scores.push(value(
                self,
                if field.pointer.is_empty() {
                    "$"
                } else {
                    &field.pointer
                },
                grade,
            ));
        }
        let aggregate =
            u32::try_from((total + weight / 2) / weight).map_err(|_| crate::error::invalid())?;
        scores.push(value(self, "aggregate", ScoreMicros::try_new(aggregate)?));
        Ok(scores)
    }
}
implement_scorer!(StructuredFieldScorer);

fn field_grade(
    answer: &RawValue,
    target: &RawValue,
    tolerance: &FieldTolerance,
) -> Result<ScoreMicros, EvalError> {
    match tolerance {
        FieldTolerance::Exact => Ok(if json_equal(answer, target, 0)? {
            ScoreMicros::ONE
        } else {
            ScoreMicros::ZERO
        }),
        FieldTolerance::NumericPpm(ppm) => {
            let target = number(target)?;
            let Ok(answer) = number(answer) else {
                return Ok(ScoreMicros::ZERO);
            };
            ToleranceBands::relative(*ppm).grade(&answer, &target)
        }
    }
}
fn raw(text: &str) -> Result<Box<RawValue>, EvalError> {
    if text.len() > 65_536 {
        return Err(target_invalid());
    }
    RawValue::from_string(text.to_owned()).map_err(|_| target_invalid())
}
fn segments(pointer: &str) -> Result<Vec<String>, EvalError> {
    if pointer.is_empty() {
        return Ok(Vec::new());
    }
    if pointer.len() > 256 || !pointer.starts_with('/') {
        return Err(crate::error::invalid());
    }
    let mut result = Vec::new();
    for segment in pointer.split('/').skip(1) {
        if result.len() >= 32 {
            return Err(crate::error::invalid());
        }
        let mut decoded = String::new();
        let mut chars = segment.chars();
        while let Some(character) = chars.next() {
            if character == '~' {
                decoded.push(match chars.next() {
                    Some('0') => '~',
                    Some('1') => '/',
                    _ => return Err(crate::error::invalid()),
                });
            } else {
                decoded.push(character);
            }
        }
        result.push(decoded);
    }
    Ok(result)
}
fn select(value: &RawValue, pointer: &str) -> Result<Option<Box<RawValue>>, EvalError> {
    let mut current = raw(value.get())?;
    for segment in segments(pointer)? {
        let next = match first(&current) {
            Some(b'{') => object(current.get())?.remove(&segment),
            Some(b'[') => {
                if (segment.starts_with('0') && segment.len() != 1)
                    || !segment.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return Ok(None);
                }
                let Ok(index) = segment.parse::<usize>() else {
                    return Ok(None);
                };
                array(current.get())?.into_iter().nth(index)
            }
            _ => return Ok(None),
        };
        let Some(next) = next else {
            return Ok(None);
        };
        current = next;
    }
    Ok(Some(current))
}
fn first(value: &RawValue) -> Option<u8> {
    value.get().trim_start().bytes().next()
}
fn number(value: &RawValue) -> Result<ParsedNumber, EvalError> {
    if first(value) == Some(b'"') {
        let text: String = serde_json::from_str(value.get()).map_err(|_| target_invalid())?;
        ParsedNumber::parse(&text)
    } else {
        ParsedNumber::parse(value.get())
    }
}
fn array(text: &str) -> Result<Vec<Box<RawValue>>, EvalError> {
    let values: Vec<Box<RawValue>> = serde_json::from_str(text).map_err(|_| target_invalid())?;
    if values.len() > 4096 {
        return Err(target_invalid());
    }
    Ok(values)
}
fn object(text: &str) -> Result<BTreeMap<String, Box<RawValue>>, EvalError> {
    struct UniqueObject;
    impl<'de> Visitor<'de> for UniqueObject {
        type Value = BTreeMap<String, Box<RawValue>>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object with unique bounded keys")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, Box<RawValue>>()? {
                if result.len() >= 4096 || result.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("object bound or duplicate key"));
                }
            }
            Ok(result)
        }
    }
    use serde::Deserializer;
    serde_json::Deserializer::from_str(text)
        .deserialize_map(UniqueObject)
        .map_err(|_| target_invalid())
}
fn json_equal(left: &RawValue, right: &RawValue, depth: usize) -> Result<bool, EvalError> {
    if depth > 32 {
        return Err(target_invalid());
    }
    match (first(left), first(right)) {
        (Some(b'{'), Some(b'{')) => {
            let left = object(left.get())?;
            let right = object(right.get())?;
            if left.len() != right.len() {
                return Ok(false);
            }
            for (key, value) in left {
                let Some(other) = right.get(&key) else {
                    return Ok(false);
                };
                if !json_equal(&value, other, depth + 1)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Some(b'['), Some(b'[')) => {
            let left = array(left.get())?;
            let right = array(right.get())?;
            if left.len() != right.len() {
                return Ok(false);
            }
            for (value, other) in left.iter().zip(&right) {
                if !json_equal(value, other, depth + 1)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Some(b'"'), Some(b'"')) => {
            let left: String = serde_json::from_str(left.get()).map_err(|_| target_invalid())?;
            let right: String = serde_json::from_str(right.get()).map_err(|_| target_invalid())?;
            Ok(left == right)
        }
        (Some(b'-' | b'0'..=b'9'), Some(b'-' | b'0'..=b'9')) => {
            number(left)?.equivalent(&number(right)?)
        }
        _ => Ok(left.get().trim() == right.get().trim()),
    }
}
