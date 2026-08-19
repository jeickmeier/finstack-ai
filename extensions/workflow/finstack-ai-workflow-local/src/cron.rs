use std::sync::Arc;

use finstack_ai_kernel::Timestamp;
use thiserror::Error;

/// Adapter-owned cron failures. These are not kernel record errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CronError {
    /// Expression text or period is unusable.
    #[error("invalid cron expression")]
    InvalidExpression {
        /// Stable reason code.
        code: &'static str,
    },
    /// Schedule identity is empty, oversized, or contains NUL.
    #[error("invalid schedule id")]
    InvalidScheduleId {
        /// Stable reason code.
        code: &'static str,
    },
    /// Adapter table is temporarily unavailable.
    #[error("cron store unavailable: {code}")]
    StoreUnavailable {
        /// Stable reason code.
        code: &'static str,
    },
    /// Adapter table could not be decoded safely.
    #[error("cron store integrity: {code}")]
    StoreIntegrity {
        /// Stable reason code.
        code: &'static str,
    },
    /// Clock or next-fire arithmetic left the representable range.
    #[error("cron time overflow")]
    TimeOverflow,
}

impl CronError {
    /// Stable lowercase error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidExpression { code }
            | Self::InvalidScheduleId { code }
            | Self::StoreUnavailable { code }
            | Self::StoreIntegrity { code } => code,
            Self::TimeOverflow => "time_overflow",
        }
    }
}

/// Interval expression evaluated against [`finstack_ai_runtime::ExternalClock`].
///
/// # Examples
///
/// ```
/// use finstack_ai_kernel::Timestamp;
/// use finstack_ai_workflow_local::IntervalSchedule;
///
/// let expr = IntervalSchedule::parse("every 10ms").expect("expr");
/// assert_eq!(expr.period_ms(), 10);
/// let origin = Timestamp::from_unix_ms(2_000).expect("origin");
/// let now = Timestamp::from_unix_ms(2_000).expect("now");
/// assert_eq!(
///     expr.next_after(origin, now).expect("next").as_unix_ms(),
///     2_010
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntervalSchedule {
    period_ms: u64,
}

impl IntervalSchedule {
    /// Construct a positive millisecond interval.
    ///
    /// # Errors
    ///
    /// Returns [`CronError::InvalidExpression`] when `period_ms` is zero.
    pub fn every_millis(period_ms: u64) -> Result<Self, CronError> {
        if period_ms == 0 {
            return Err(CronError::InvalidExpression {
                code: "zero_period",
            });
        }
        Ok(Self { period_ms })
    }

    /// Parse `every <n>ms`, `every <n>s`, or `every <n>m`.
    ///
    /// # Errors
    ///
    /// Returns [`CronError::InvalidExpression`] for empty, unknown, or
    /// zero-period text.
    pub fn parse(text: &str) -> Result<Self, CronError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(CronError::InvalidExpression {
                code: "empty_expression",
            });
        }
        let rest = trimmed
            .strip_prefix("every ")
            .or_else(|| trimmed.strip_prefix("every"))
            .ok_or(CronError::InvalidExpression {
                code: "unknown_expression",
            })?
            .trim();
        let (digits, unit) = split_period(rest)?;
        let count: u64 = digits.parse().map_err(|_| CronError::InvalidExpression {
            code: "unknown_expression",
        })?;
        if count == 0 {
            return Err(CronError::InvalidExpression {
                code: "zero_period",
            });
        }
        let period_ms = match unit {
            "ms" => count,
            "s" => count
                .checked_mul(1_000)
                .ok_or(CronError::InvalidExpression {
                    code: "unknown_expression",
                })?,
            "m" => count
                .checked_mul(60_000)
                .ok_or(CronError::InvalidExpression {
                    code: "unknown_expression",
                })?,
            _ => {
                return Err(CronError::InvalidExpression {
                    code: "unknown_expression",
                });
            }
        };
        Self::every_millis(period_ms)
    }

    /// Interval in milliseconds.
    #[must_use]
    pub const fn period_ms(&self) -> u64 {
        self.period_ms
    }

    /// Canonical text stored in the adapter table.
    #[must_use]
    pub fn as_str(&self) -> String {
        format!("every {}ms", self.period_ms)
    }

    /// Next strictly future tick on the origin-aligned grid.
    ///
    /// # Errors
    ///
    /// Returns [`CronError::TimeOverflow`] when the tick leaves the
    /// timestamp range.
    pub fn next_after(&self, origin: Timestamp, now: Timestamp) -> Result<Timestamp, CronError> {
        let origin_ms = origin.as_unix_ms();
        let now_ms = now.as_unix_ms();
        let period = i64::try_from(self.period_ms).map_err(|_| CronError::TimeOverflow)?;
        if now_ms < origin_ms {
            return Timestamp::from_unix_ms(origin_ms).map_err(|_| CronError::TimeOverflow);
        }
        let elapsed = now_ms
            .checked_sub(origin_ms)
            .ok_or(CronError::TimeOverflow)?;
        let steps = elapsed / period;
        let next_ms = origin_ms
            .checked_add(
                steps
                    .checked_add(1)
                    .ok_or(CronError::TimeOverflow)?
                    .checked_mul(period)
                    .ok_or(CronError::TimeOverflow)?,
            )
            .ok_or(CronError::TimeOverflow)?;
        Timestamp::from_unix_ms(next_ms).map_err(|_| CronError::TimeOverflow)
    }
}

/// Durable adapter schedule row. Not a kernel record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    /// Tenant captured from [`finstack_ai_runtime::WorkflowSession::tenant_scope`].
    pub tenant_scope: Arc<str>,
    /// Caller-chosen schedule identity, unique per tenant.
    pub schedule_id: Arc<str>,
    /// Interval expression.
    pub expression: IntervalSchedule,
    /// Alignment origin used to recompute future ticks.
    pub origin: Timestamp,
    /// Next fire instant against the injected clock.
    pub next_fire_at: Timestamp,
    /// Last catch-up or on-time fire, when any.
    pub last_fired_at: Option<Timestamp>,
    /// Successful fires, including a single restart catch-up.
    pub fire_count: u64,
}

/// One adapter-owned fire. Starting a run remains a caller action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronFire {
    /// Tenant that owned the schedule.
    pub tenant_scope: Arc<str>,
    /// Fired schedule.
    pub schedule_id: Arc<str>,
    /// Clock instant used for the fire.
    pub fired_at: Timestamp,
}

pub(crate) fn validate_schedule_id(schedule_id: &str) -> Result<Arc<str>, CronError> {
    if schedule_id.is_empty() {
        return Err(CronError::InvalidScheduleId {
            code: "empty_schedule_id",
        });
    }
    if schedule_id.len() > 128 {
        return Err(CronError::InvalidScheduleId {
            code: "oversized_schedule_id",
        });
    }
    if schedule_id.as_bytes().contains(&0) {
        return Err(CronError::InvalidScheduleId {
            code: "nul_schedule_id",
        });
    }
    Ok(Arc::from(schedule_id))
}

fn split_period(rest: &str) -> Result<(&str, &str), CronError> {
    let unit_at =
        rest.find(|ch: char| !ch.is_ascii_digit())
            .ok_or(CronError::InvalidExpression {
                code: "unknown_expression",
            })?;
    if unit_at == 0 {
        return Err(CronError::InvalidExpression {
            code: "unknown_expression",
        });
    }
    Ok((&rest[..unit_at], rest[unit_at..].trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_zero_and_unknown() {
        assert_eq!(
            IntervalSchedule::parse("").expect_err("empty").code(),
            "empty_expression"
        );
        assert_eq!(
            IntervalSchedule::parse("0 * * * *")
                .expect_err("unknown")
                .code(),
            "unknown_expression"
        );
        assert_eq!(
            IntervalSchedule::parse("every 0ms")
                .expect_err("zero")
                .code(),
            "zero_period"
        );
    }

    #[test]
    fn next_after_is_strictly_future_and_aligned() {
        let expr = IntervalSchedule::parse("every 10ms").expect("expr");
        let origin = Timestamp::from_unix_ms(2_000).expect("origin");
        assert_eq!(
            expr.next_after(origin, origin)
                .expect("on origin")
                .as_unix_ms(),
            2_010
        );
        let mid = Timestamp::from_unix_ms(2_053).expect("mid");
        assert_eq!(
            expr.next_after(origin, mid).expect("mid").as_unix_ms(),
            2_060
        );
        let on_tick = Timestamp::from_unix_ms(2_050).expect("tick");
        assert_eq!(
            expr.next_after(origin, on_tick).expect("tick").as_unix_ms(),
            2_060
        );
    }
}
