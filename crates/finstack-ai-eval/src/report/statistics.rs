//! Derived floating-point statistics; exact grades and costs remain in storage.
use crate::{EvalError, RepetitionReducer, ScoreMicros};
use serde::{Deserialize, Serialize};

/// Sample mean and standard error. A singleton has no estimable standard error.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Statistics {
    /// Number of observations, after the documented reduction/pairing.
    pub n: u32,
    /// Mean, serialized as a decimal string; absent for an empty sample.
    #[serde(with = "decimal")]
    pub mean: Option<f64>,
    /// Sample standard deviation divided by sqrt(n), absent below two observations.
    #[serde(with = "decimal")]
    pub stderr: Option<f64>,
}
impl Statistics {
    /// Compute bounded finite sample statistics using Welford accumulation.
    /// # Errors
    /// Rejects non-finite values or more than 100,000 observations.
    pub fn from_values(values: impl IntoIterator<Item = f64>) -> Result<Self, EvalError> {
        let (mut n, mut mean, mut m2) = (0_u32, 0.0, 0.0);
        for value in values {
            if !value.is_finite() || n >= super::count(crate::MAX_CELLS)? {
                return Err(crate::error::invalid());
            }
            n += 1;
            let delta = value - mean;
            mean += delta / f64::from(n);
            m2 += delta * (value - mean);
        }
        let stderr = (n > 1).then(|| (m2.max(0.0) / f64::from(n - 1) / f64::from(n)).sqrt());
        if !mean.is_finite() || stderr.is_some_and(|value| !value.is_finite()) {
            return Err(crate::error::invalid());
        }
        Ok(Self {
            n,
            mean: (n != 0).then_some(mean),
            stderr,
        })
    }
}

/// Reduce observed repetitions; callers must also report missing repetition coverage.
/// # Errors
/// Rejects empty/oversized input and invalid `k`; returns `None` if fewer than `k` are observed.
pub fn reduce_repetitions(
    values: &[ScoreMicros],
    reducer: &RepetitionReducer,
) -> Result<Option<f64>, EvalError> {
    let n = u32::try_from(values.len()).map_err(|_| crate::error::invalid())?;
    if n == 0 || n > 10_000 {
        return Err(crate::error::invalid());
    }
    match reducer {
        RepetitionReducer::Mean => Ok(Some(
            values.iter().map(|v| f64::from(v.get())).sum::<f64>() / f64::from(n) / 1_000_000.0,
        )),
        RepetitionReducer::PassAtK {
            k,
            pass_threshold_micros,
        }
        | RepetitionReducer::AtLeastK {
            k,
            pass_threshold_micros,
        } => {
            if *k == 0 || *k > 10_000 {
                return Err(crate::error::invalid());
            }
            if n < *k {
                return Ok(None);
            }
            let c = super::count(
                values
                    .iter()
                    .filter(|value| **value >= *pass_threshold_micros)
                    .count(),
            )?;
            if matches!(reducer, RepetitionReducer::AtLeastK { .. }) {
                return Ok(Some(f64::from(u8::from(c >= *k))));
            }
            if n - c < *k {
                return Ok(Some(1.0));
            }
            // Probability of drawing no pass. log1p/expm1 avoid cancellation near zero.
            let log_failure = (0..*k)
                .map(|i| (-f64::from(c) / f64::from(n - i)).ln_1p())
                .sum::<f64>();
            Ok(Some((-log_failure.exp_m1()).clamp(0.0, 1.0)))
        }
    }
}

pub(super) mod decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    #[allow(clippy::ref_option, reason = "serde with serializer signature")]
    pub fn serialize<S: Serializer>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => serializer.serialize_some(&value.to_string()),
            None => serializer.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<f64>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| {
                text.parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| serde::de::Error::custom("expected finite decimal string"))
            })
            .transpose()
    }
}
