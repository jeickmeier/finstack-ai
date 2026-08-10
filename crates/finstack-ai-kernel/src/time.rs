//! Canonical UTC millisecond timestamps and non-negative durations.
//!
//! [`Timestamp`] is Unix-epoch milliseconds restricted to UTC years 0001–9999.
//! Leap seconds are not representable. JSON/diagnostic text uses exact
//! RFC 3339 UTC with millisecond precision (`YYYY-MM-DDTHH:MM:SS.sssZ`).

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Earliest representable UTC millisecond instant (`0001-01-01T00:00:00.000Z`).
pub const TIMESTAMP_MIN_MS: i64 = -62_135_596_800_000;
/// Latest representable UTC millisecond instant (`9999-12-31T23:59:59.999Z`).
pub const TIMESTAMP_MAX_MS: i64 = 253_402_300_799_999;
/// Largest duration that converts losslessly to a JavaScript `number`.
pub const DURATION_JS_SAFE_MAX_MS: u64 = 9_007_199_254_740_991;

/// Canonical UTC Unix-epoch millisecond timestamp.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    /// Construct a timestamp from Unix-epoch milliseconds when in range.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError::OutOfRange`] outside years 0001–9999.
    pub const fn from_unix_ms(ms: i64) -> Result<Self, TimeError> {
        if ms < TIMESTAMP_MIN_MS || ms > TIMESTAMP_MAX_MS {
            return Err(TimeError::OutOfRange { value: ms });
        }
        Ok(Self(ms))
    }

    /// Unix-epoch milliseconds.
    #[must_use]
    pub const fn as_unix_ms(self) -> i64 {
        self.0
    }

    /// Format as exact RFC 3339 UTC with millisecond precision.
    #[must_use]
    pub fn to_rfc3339(&self) -> String {
        let mut buffer = Rfc3339Buffer::new();
        buffer.encode(self.0).to_owned()
    }

    /// Parse exact RFC 3339 UTC with millisecond precision.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError`] for malformed text, non-UTC offsets, leap seconds,
    /// or out-of-range civil values.
    pub fn parse_rfc3339(input: &str) -> Result<Self, TimeError> {
        parse_rfc3339_ms(input)
    }

    /// Checked addition of a duration.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError::Overflow`] when the result leaves the representable range.
    pub fn checked_add(self, duration: Duration) -> Result<Self, TimeError> {
        let ms = self
            .0
            .checked_add(i64::try_from(duration.as_millis()).map_err(|_| TimeError::Overflow)?)
            .ok_or(TimeError::Overflow)?;
        Self::from_unix_ms(ms).map_err(|_| TimeError::Overflow)
    }

    /// Checked subtraction of a duration.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError::Overflow`] when the result leaves the representable range.
    pub fn checked_sub(self, duration: Duration) -> Result<Self, TimeError> {
        let ms = self
            .0
            .checked_sub(i64::try_from(duration.as_millis()).map_err(|_| TimeError::Overflow)?)
            .ok_or(TimeError::Overflow)?;
        Self::from_unix_ms(ms).map_err(|_| TimeError::Overflow)
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buffer = Rfc3339Buffer::new();
        f.debug_tuple("Timestamp")
            .field(&buffer.encode(self.0))
            .finish()
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buffer = Rfc3339Buffer::new();
        f.write_str(buffer.encode(self.0))
    }
}

/// Stack buffer for `YYYY-MM-DDTHH:MM:SS.sssZ` text.
///
/// Writes the seven fixed-width fields directly instead of running seven
/// padded-integer `core::fmt` dispatches plus a heap allocation. Every message
/// and every record carries a timestamp.
pub(crate) struct Rfc3339Buffer([u8; 24]);

impl Rfc3339Buffer {
    pub(crate) const fn new() -> Self {
        Self(*b"0000-00-00T00:00:00.000Z")
    }

    pub(crate) fn encode(&mut self, unix_ms: i64) -> &str {
        let (year, month, day, hour, minute, second, millis) = civil_from_unix_ms(unix_ms);
        write_digits(&mut self.0[0..4], year);
        write_digits(&mut self.0[5..7], month);
        write_digits(&mut self.0[8..10], day);
        write_digits(&mut self.0[11..13], hour);
        write_digits(&mut self.0[14..16], minute);
        write_digits(&mut self.0[17..19], second);
        write_digits(&mut self.0[20..23], millis);
        // Only ASCII digits and fixed separators are ever written.
        core::str::from_utf8(&self.0).expect("RFC 3339 text is ASCII")
    }
}

/// Write `value` right-aligned and zero-padded across the whole slice.
///
/// Callers pass slices sized to the field width, and `civil_from_unix_ms`
/// bounds every field, so truncation is unreachable.
fn write_digits(slot: &mut [u8], value: u32) {
    let mut remaining = value;
    for position in slot.iter_mut().rev() {
        *position = b'0' + u8::try_from(remaining % 10).expect("digit fits u8");
        remaining /= 10;
    }
}

impl FromStr for Timestamp {
    type Err = TimeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_rfc3339(s)
    }
}

impl Serialize for Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            serializer.serialize_str(&self.to_rfc3339())
        } else {
            serializer.serialize_i64(self.0)
        }
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            struct Rfc3339Visitor;

            impl serde::de::Visitor<'_> for Rfc3339Visitor {
                type Value = Timestamp;

                fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter.write_str("an RFC 3339 UTC timestamp with millisecond precision")
                }

                fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                    Timestamp::parse_rfc3339(value).map_err(E::custom)
                }
            }

            deserializer.deserialize_str(Rfc3339Visitor)
        } else {
            let ms = i64::deserialize(deserializer)?;
            Self::from_unix_ms(ms).map_err(serde::de::Error::custom)
        }
    }
}

/// Non-negative elapsed milliseconds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Duration(u64);

impl Duration {
    /// Zero duration.
    pub const ZERO: Self = Self(0);

    /// Construct from non-negative milliseconds.
    #[must_use]
    pub const fn from_millis(ms: u64) -> Self {
        Self(ms)
    }

    /// Milliseconds.
    #[must_use]
    pub const fn as_millis(self) -> u64 {
        self.0
    }

    /// Convert to an IEEE-754 integer that JavaScript can represent exactly.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError::NotJsSafe`] when the value exceeds
    /// [`DURATION_JS_SAFE_MAX_MS`].
    pub const fn to_js_number(self) -> Result<f64, TimeError> {
        if self.0 > DURATION_JS_SAFE_MAX_MS {
            return Err(TimeError::NotJsSafe { value: self.0 });
        }
        // Values at or below `DURATION_JS_SAFE_MAX_MS` are exact IEEE-754 integers.
        #[allow(clippy::cast_precision_loss)]
        Ok(self.0 as f64)
    }

    /// Construct from a JavaScript number that must be an exact non-negative integer.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError`] for NaN, infinity, negatives, fractions, or values
    /// outside the safe integer domain.
    pub fn from_js_number(value: f64) -> Result<Self, TimeError> {
        if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
            return Err(TimeError::InvalidJsNumber { value });
        }
        #[allow(clippy::cast_precision_loss)]
        let max = DURATION_JS_SAFE_MAX_MS as f64;
        if value > max {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                return Err(TimeError::NotJsSafe {
                    value: value as u64,
                });
            }
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(Self(value as u64))
    }

    /// Checked addition.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError::Overflow`] on `u64` overflow.
    pub const fn checked_add(self, other: Self) -> Result<Self, TimeError> {
        match self.0.checked_add(other.0) {
            Some(ms) => Ok(Self(ms)),
            None => Err(TimeError::Overflow),
        }
    }

    /// Checked subtraction.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError::Overflow`] when `other` is larger than `self`.
    pub const fn checked_sub(self, other: Self) -> Result<Self, TimeError> {
        match self.0.checked_sub(other.0) {
            Some(ms) => Ok(Self(ms)),
            None => Err(TimeError::Overflow),
        }
    }
}

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Duration").field(&self.0).finish()
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

/// Time parsing / arithmetic failure.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum TimeError {
    /// Milliseconds outside years 0001–9999.
    #[error("timestamp out of range: {value}")]
    OutOfRange {
        /// Rejected millisecond value.
        value: i64,
    },
    /// Arithmetic left the representable domain.
    #[error("time arithmetic overflow")]
    Overflow,
    /// RFC 3339 text was rejected.
    #[error("invalid RFC 3339 timestamp: {input}")]
    InvalidRfc3339 {
        /// Rejected text.
        input: String,
    },
    /// Duration is not an exact JavaScript integer.
    #[error("duration is not JavaScript-safe: {value}")]
    NotJsSafe {
        /// Rejected millisecond value.
        value: u64,
    },
    /// JavaScript number was not an exact non-negative integer.
    #[error("invalid JavaScript duration number: {value}")]
    InvalidJsNumber {
        /// Rejected number.
        value: f64,
    },
}

fn parse_rfc3339_ms(input: &str) -> Result<Timestamp, TimeError> {
    // Exact shape: YYYY-MM-DDTHH:MM:SS.sssZ
    let bytes = input.as_bytes();
    if bytes.len() != 24
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'.'
        || bytes[23] != b'Z'
    {
        return Err(TimeError::InvalidRfc3339 {
            input: input.to_owned(),
        });
    }
    let year = parse_u32(&bytes[0..4], input)?;
    let month = parse_u32(&bytes[5..7], input)?;
    let day = parse_u32(&bytes[8..10], input)?;
    let hour = parse_u32(&bytes[11..13], input)?;
    let minute = parse_u32(&bytes[14..16], input)?;
    let second = parse_u32(&bytes[17..19], input)?;
    let millis = parse_u32(&bytes[20..23], input)?;
    if !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
        || millis > 999
    {
        // Leap seconds (second == 60) are intentionally rejected.
        return Err(TimeError::InvalidRfc3339 {
            input: input.to_owned(),
        });
    }
    let year = i32::try_from(year).map_err(|_| TimeError::InvalidRfc3339 {
        input: input.to_owned(),
    })?;
    let month = u8::try_from(month).map_err(|_| TimeError::InvalidRfc3339 {
        input: input.to_owned(),
    })?;
    let day = u8::try_from(day).map_err(|_| TimeError::InvalidRfc3339 {
        input: input.to_owned(),
    })?;
    let days = days_from_civil(year, month, day);
    let day_ms = i64::from(hour) * 3_600_000
        + i64::from(minute) * 60_000
        + i64::from(second) * 1_000
        + i64::from(millis);
    let unix_ms = i64::from(days) * 86_400_000 + day_ms;
    Timestamp::from_unix_ms(unix_ms).map_err(|_| TimeError::InvalidRfc3339 {
        input: input.to_owned(),
    })
}

fn parse_u32(slice: &[u8], input: &str) -> Result<u32, TimeError> {
    let mut value = 0_u32;
    for &b in slice {
        if !b.is_ascii_digit() {
            return Err(TimeError::InvalidRfc3339 {
                input: input.to_owned(),
            });
        }
        value = value * 10 + u32::from(b - b'0');
    }
    Ok(value)
}

fn is_leap(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Howard Hinnant `civil_from_days` / `days_from_civil` adapted to Unix epoch days.
///
/// Casts are bounded by the year-0001–9999 constructor range.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn days_from_civil(year: i32, month: u8, day: u8) -> i32 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let mp = if month > 2 {
        u32::from(month) - 3
    } else {
        u32::from(month) + 9
    };
    let doy = (153 * mp + 2) / 5 + u32::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i32 - 719_468
}

/// Inverse of [`days_from_civil`] for in-range millisecond timestamps.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn civil_from_unix_ms(ms: i64) -> (u32, u32, u32, u32, u32, u32, u32) {
    let day_ms = 86_400_000_i64;
    let mut days = ms.div_euclid(day_ms) as i32;
    let mut tod = ms.rem_euclid(day_ms) as u32;
    let hour = tod / 3_600_000;
    tod %= 3_600_000;
    let minute = tod / 60_000;
    tod %= 60_000;
    let second = tod / 1_000;
    let millis = tod % 1_000;

    days += 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = (days - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i32) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as u32, m, d, hour, minute, second, millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_round_trips_rfc3339_and_unix_ms() {
        let ts = Timestamp::from_unix_ms(0).expect("epoch");
        assert_eq!(ts.to_rfc3339(), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            Timestamp::parse_rfc3339("1970-01-01T00:00:00.000Z").expect("parse"),
            ts
        );

        let min = Timestamp::from_unix_ms(TIMESTAMP_MIN_MS).expect("min");
        assert_eq!(min.to_rfc3339(), "0001-01-01T00:00:00.000Z");
        let max = Timestamp::from_unix_ms(TIMESTAMP_MAX_MS).expect("max");
        assert_eq!(max.to_rfc3339(), "9999-12-31T23:59:59.999Z");
    }

    #[test]
    fn rejects_leap_seconds_and_out_of_range() {
        assert!(Timestamp::parse_rfc3339("1970-01-01T00:00:60.000Z").is_err());
        assert!(Timestamp::from_unix_ms(TIMESTAMP_MIN_MS - 1).is_err());
        assert!(Timestamp::from_unix_ms(TIMESTAMP_MAX_MS + 1).is_err());
    }

    #[test]
    fn duration_js_safe_boundary() {
        let ok = Duration::from_millis(DURATION_JS_SAFE_MAX_MS);
        #[allow(clippy::cast_precision_loss, clippy::float_cmp)]
        {
            assert_eq!(
                ok.to_js_number().expect("js"),
                DURATION_JS_SAFE_MAX_MS as f64
            );
        }
        assert!(
            Duration::from_millis(DURATION_JS_SAFE_MAX_MS + 1)
                .to_js_number()
                .is_err()
        );
        assert!(Duration::from_js_number(1.5).is_err());
        assert!(Duration::from_js_number(-1.0).is_err());
    }

    #[test]
    fn json_uses_rfc3339_text() {
        let ts = Timestamp::from_unix_ms(1_704_067_200_000).expect("ts");
        let json = serde_json::to_string(&ts).expect("ser");
        assert_eq!(json, "\"2024-01-01T00:00:00.000Z\"");
        let round: Timestamp = serde_json::from_str(&json).expect("de");
        assert_eq!(round, ts);
    }
}
