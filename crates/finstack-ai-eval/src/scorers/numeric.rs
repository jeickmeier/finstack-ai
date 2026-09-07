//! Exact bounded decimal parsing and unit-aware tolerance arithmetic.
use crate::{EVAL_ARITHMETIC_OVERFLOW, EVAL_TARGET_INVALID, EvalError, ScoreMicros};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Normalized number dimension. Percent, bps and plain fractions share `Scalar`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "code",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum NumberUnit {
    /// Dimensionless value, including normalized fractions.
    Scalar,
    /// Explicit ISO-style currency code. `$` means USD, never an inferred locale.
    Currency(Arc<str>),
}

/// Exact value `mantissa × 10^exponent` with a normalized unit.
/// Mantissa is serialized as a decimal string, preserving cross-language precision.
/// Parsing accepts at most 28 significant digits and exponents in `-24..=24`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "NumberWire", into = "NumberWire")]
pub struct ParsedNumber {
    /// Signed integer significand.
    pub mantissa: i128,
    /// Base-ten exponent.
    pub exponent: i32,
    /// Normalized dimension, checked before comparing values.
    pub unit: NumberUnit,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NumberWire {
    mantissa: String,
    exponent: i32,
    unit: NumberUnit,
}
impl TryFrom<NumberWire> for ParsedNumber {
    type Error = EvalError;
    fn try_from(wire: NumberWire) -> Result<Self, Self::Error> {
        Self::try_new(
            wire.mantissa.parse().map_err(|_| invalid_number())?,
            wire.exponent,
            wire.unit,
        )
    }
}
impl From<ParsedNumber> for NumberWire {
    fn from(value: ParsedNumber) -> Self {
        Self {
            mantissa: value.mantissa.to_string(),
            exponent: value.exponent,
            unit: value.unit,
        }
    }
}
fn invalid_number() -> EvalError {
    EvalError::new(
        EVAL_TARGET_INVALID,
        "number is invalid or outside supported decimal bounds",
    )
}
fn overflow() -> EvalError {
    EvalError::new(
        EVAL_ARITHMETIC_OVERFLOW,
        "exact decimal arithmetic exceeded its bound",
    )
}

impl ParsedNumber {
    /// Construct and normalize a decimal, removing trailing significand zeroes.
    /// # Errors
    /// Returns invalid-number errors outside supported mantissa/exponent/unit bounds.
    pub fn try_new(
        mut mantissa: i128,
        mut exponent: i32,
        unit: NumberUnit,
    ) -> Result<Self, EvalError> {
        if mantissa.unsigned_abs() >= 10_u128.pow(28)
            || !(-24..=24).contains(&exponent)
            || matches!(&unit, NumberUnit::Currency(code) if !currency_code(code))
        {
            return Err(invalid_number());
        }
        if mantissa == 0 {
            exponent = 0;
        }
        while mantissa != 0 && mantissa % 10 == 0 && exponent < 24 {
            mantissa /= 10;
            exponent += 1;
        }
        Ok(Self {
            mantissa,
            exponent,
            unit,
        })
    }
    /// Parse decimal/scientific notation, strict comma grouping, accounting
    /// parentheses, currency symbols/codes, k/m/mm/bn and percent/bps suffixes.
    /// Examples: `$1.2m`, `USD 1,200,000`, `119.7 bps`, `1.197%`.
    /// # Errors
    /// Rejects ambiguous prose, malformed separators, non-finite numbers, mixed
    /// dimensions, and inputs beyond 256 UTF-8 bytes or supported decimal bounds.
    pub fn parse(text: &str) -> Result<Self, EvalError> {
        if text.len() > 256 {
            return Err(invalid_number());
        }
        let text = text.trim();
        let (text, accounting) = if text.starts_with('(') && text.ends_with(')') {
            (
                text.get(1..text.len().saturating_sub(1))
                    .ok_or_else(invalid_number)?
                    .trim(),
                true,
            )
        } else {
            (text, false)
        };
        let (text, unit) = strip_currency(text)?;
        let (text, unit_power) = strip_unit_scale(text, &unit)?;
        let (text, scale_power) = strip_scale(text);
        let (mantissa, exponent) = parse_decimal(text.trim())?;
        if accounting && text.trim_start().starts_with(['+', '-']) {
            return Err(invalid_number());
        }
        let exponent = exponent
            .checked_add(unit_power)
            .and_then(|value| value.checked_add(scale_power))
            .ok_or_else(invalid_number)?;
        Self::try_new(
            if accounting { -mantissa } else { mantissa },
            exponent,
            unit,
        )
    }
    /// Compare numerically, after checking dimension and representational bounds.
    /// # Errors
    /// Returns arithmetic overflow when exact alignment exceeds signed 128 bits.
    pub fn equivalent(&self, other: &Self) -> Result<bool, EvalError> {
        if self.unit != other.unit {
            return Ok(false);
        }
        let (left, right) = aligned(self, other)?;
        Ok(left == right)
    }
}
fn currency_code(code: &str) -> bool {
    matches!(
        code,
        "USD" | "EUR" | "GBP" | "JPY" | "CHF" | "CAD" | "AUD" | "CNY"
    )
}
fn strip_currency(text: &str) -> Result<(&str, NumberUnit), EvalError> {
    for (symbol, code) in [("$", "USD"), ("€", "EUR"), ("£", "GBP"), ("¥", "JPY")] {
        if let Some(tail) = text.strip_prefix(symbol) {
            return Ok((tail.trim(), NumberUnit::Currency(Arc::from(code))));
        }
    }
    for code in ["USD", "EUR", "GBP", "JPY", "CHF", "CAD", "AUD", "CNY"] {
        if let Some(tail) = text.strip_prefix(code) {
            return Ok((tail.trim(), NumberUnit::Currency(Arc::from(code))));
        }
        if let Some(head) = text.strip_suffix(code) {
            return Ok((head.trim(), NumberUnit::Currency(Arc::from(code))));
        }
    }
    if text.is_empty() {
        return Err(invalid_number());
    }
    Ok((text, NumberUnit::Scalar))
}
fn strip_unit_scale<'a>(text: &'a str, unit: &NumberUnit) -> Result<(&'a str, i32), EvalError> {
    for (suffix, power) in [("percent", -2), ("bps", -4), ("bp", -4), ("%", -2)] {
        if let Some(head) = text.strip_suffix(suffix) {
            if *unit != NumberUnit::Scalar {
                return Err(invalid_number());
            }
            return Ok((head.trim(), power));
        }
    }
    Ok((text, 0))
}
fn strip_scale(text: &str) -> (&str, i32) {
    for (suffix, power) in [
        ("billion", 9),
        ("million", 6),
        ("thousand", 3),
        ("bn", 9),
        ("mm", 6),
        ("k", 3),
        ("m", 6),
        ("b", 9),
        ("K", 3),
        ("M", 6),
        ("B", 9),
    ] {
        if let Some(head) = text.strip_suffix(suffix) {
            return (head.trim(), power);
        }
    }
    (text, 0)
}
fn parse_decimal(text: &str) -> Result<(i128, i32), EvalError> {
    let (text, negative) = match text.strip_prefix('-') {
        Some(text) => (text, true),
        None => (text.strip_prefix('+').unwrap_or(text), false),
    };
    let (significand, exponent) = match text.split_once(['e', 'E']) {
        Some((number, power)) => (number, power.parse::<i32>().map_err(|_| invalid_number())?),
        None => (text, 0),
    };
    let (integer, fraction) = significand.split_once('.').unwrap_or((significand, ""));
    if integer.is_empty() && fraction.is_empty() {
        return Err(invalid_number());
    }
    validate_grouping(integer)?;
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_number());
    }
    let digits: String = integer
        .bytes()
        .filter(|byte| *byte != b',')
        .map(char::from)
        .chain(fraction.chars())
        .collect();
    if digits.is_empty() || digits.len() > 28 {
        return Err(invalid_number());
    }
    let mantissa = digits.parse::<i128>().map_err(|_| invalid_number())?;
    let places = i32::try_from(fraction.len()).map_err(|_| invalid_number())?;
    Ok((
        if negative { -mantissa } else { mantissa },
        exponent.checked_sub(places).ok_or_else(invalid_number)?,
    ))
}
fn validate_grouping(integer: &str) -> Result<(), EvalError> {
    if !integer
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b',')
    {
        return Err(invalid_number());
    }
    if integer.contains(',') {
        for (index, group) in integer.split(',').enumerate() {
            if group.is_empty()
                || (index == 0 && group.len() > 3)
                || (index > 0 && group.len() != 3)
            {
                return Err(invalid_number());
            }
        }
    }
    Ok(())
}
fn scaled(value: &ParsedNumber, exponent: i32) -> Result<i128, EvalError> {
    // Public fields are revalidated before arithmetic or shifting.
    ParsedNumber::try_new(value.mantissa, value.exponent, value.unit.clone())?;
    let power = u32::try_from(value.exponent.checked_sub(exponent).ok_or_else(overflow)?)
        .map_err(|_| overflow())?;
    let scale = 10_i128.checked_pow(power).ok_or_else(overflow)?;
    value.mantissa.checked_mul(scale).ok_or_else(overflow)
}
fn aligned(left: &ParsedNumber, right: &ParsedNumber) -> Result<(i128, i128), EvalError> {
    let exponent = left.exponent.min(right.exponent);
    Ok((scaled(left, exponent)?, scaled(right, exponent)?))
}

/// Full and optional partial credit bands, with exact relative parts per million.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToleranceBands {
    /// Full credit when absolute relative error is within this many ppm.
    pub full_within_ppm: u64,
    /// Optional wider relative band for partial credit.
    pub partial_within_ppm: Option<u64>,
    /// Exact partial credit; must be less than full credit.
    pub partial_micros: ScoreMicros,
    /// Optional nonnegative full-credit absolute tolerance in the target's unit.
    pub absolute: Option<ParsedNumber>,
}
impl ToleranceBands {
    /// Relative tolerance with no partial or absolute band.
    #[must_use]
    pub fn relative(ppm: u64) -> Self {
        Self {
            full_within_ppm: ppm,
            partial_within_ppm: None,
            partial_micros: ScoreMicros::ZERO,
            absolute: None,
        }
    }
    /// Validate nonnegative absolute tolerance and ordered partial bands.
    /// # Errors
    /// Returns invalid scorer configuration or number bounds.
    pub fn validate(&self) -> Result<(), EvalError> {
        if self
            .partial_within_ppm
            .is_some_and(|ppm| ppm < self.full_within_ppm)
            || self.partial_micros == ScoreMicros::ONE
            || self
                .absolute
                .as_ref()
                .is_some_and(|number| number.mantissa < 0)
        {
            return Err(crate::error::invalid());
        }
        if let Some(number) = &self.absolute {
            ParsedNumber::try_new(number.mantissa, number.exponent, number.unit.clone())?;
        }
        Ok(())
    }
    /// Return full, partial, or zero credit. Different answer units score zero;
    /// an absolute tolerance with the wrong target unit is invalid configuration.
    /// # Errors
    /// Returns invalid configuration or checked arithmetic overflow, never a rounded grade.
    pub fn grade(
        &self,
        answer: &ParsedNumber,
        target: &ParsedNumber,
    ) -> Result<ScoreMicros, EvalError> {
        self.validate()?;
        if self
            .absolute
            .as_ref()
            .is_some_and(|absolute| absolute.unit != target.unit)
        {
            return Err(crate::error::invalid());
        }
        if answer.unit != target.unit {
            return Ok(ScoreMicros::ZERO);
        }
        if let Some(absolute) = &self.absolute {
            let exponent = answer.exponent.min(target.exponent).min(absolute.exponent);
            let difference = scaled(answer, exponent)?
                .checked_sub(scaled(target, exponent)?)
                .ok_or_else(overflow)?
                .unsigned_abs();
            if difference <= scaled(absolute, exponent)?.unsigned_abs() {
                return Ok(ScoreMicros::ONE);
            }
        }
        let (actual, expected) = aligned(answer, target)?;
        let difference = actual
            .checked_sub(expected)
            .ok_or_else(overflow)?
            .unsigned_abs();
        let error = difference.checked_mul(1_000_000).ok_or_else(overflow)?;
        let full = expected
            .unsigned_abs()
            .checked_mul(u128::from(self.full_within_ppm))
            .ok_or_else(overflow)?;
        if error <= full {
            return Ok(ScoreMicros::ONE);
        }
        if let Some(ppm) = self.partial_within_ppm {
            let partial = expected
                .unsigned_abs()
                .checked_mul(u128::from(ppm))
                .ok_or_else(overflow)?;
            if error <= partial {
                return Ok(self.partial_micros);
            }
        }
        Ok(ScoreMicros::ZERO)
    }
}
