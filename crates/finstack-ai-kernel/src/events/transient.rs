//! Transient runtime-event bodies.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::content::{BoundedString, LABEL_MAX_BYTES, TEXT_MAX_BYTES};
use crate::refs::{validated_label, validated_text};

use super::EventError;

/// Model text delta body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelTextDelta {
    text: Arc<str>,
}

impl ModelTextDelta {
    /// Construct a text delta.
    ///
    /// # Errors
    ///
    /// Returns [`EventError`] when text is empty, oversized, or NUL-bearing.
    pub fn try_new(text: impl AsRef<str>) -> Result<Self, EventError> {
        let text = text.as_ref();
        if text.is_empty()
            || text.len() > crate::content::TEXT_MAX_BYTES
            || text.as_bytes().contains(&0)
        {
            return Err(EventError::InvalidLabel { field: "text" });
        }
        Ok(Self {
            text: Arc::<str>::from(text),
        })
    }

    /// Borrow text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl<'de> Deserialize<'de> for ModelTextDelta {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            text: BoundedString<TEXT_MAX_BYTES>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.text.into_inner()).map_err(de::Error::custom)
    }
}

/// Reasoning delta body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReasoningDelta {
    text: Arc<str>,
}

impl ReasoningDelta {
    /// Construct a reasoning delta.
    ///
    /// # Errors
    ///
    /// Returns [`EventError`] when text is invalid.
    pub fn try_new(text: impl AsRef<str>) -> Result<Self, EventError> {
        Ok(Self {
            text: ModelTextDelta::try_new(text)?.text,
        })
    }

    /// Borrow text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl<'de> Deserialize<'de> for ReasoningDelta {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            text: BoundedString<TEXT_MAX_BYTES>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.text.into_inner()).map_err(de::Error::custom)
    }
}

/// Tool progress body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolProgress {
    message: Arc<str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    percent: Option<u8>,
}

impl ToolProgress {
    /// Construct tool progress.
    ///
    /// # Errors
    ///
    /// Returns [`EventError`] when message is invalid or percent > 100.
    pub fn try_new(message: impl AsRef<str>, percent: Option<u8>) -> Result<Self, EventError> {
        if percent.is_some_and(|value| value > 100) {
            return Err(EventError::InvalidPercent);
        }
        Ok(Self {
            message: validated_text(message.as_ref(), "message")?,
            percent,
        })
    }
}

impl<'de> Deserialize<'de> for ToolProgress {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            message: BoundedString<TEXT_MAX_BYTES>,
            #[serde(default)]
            percent: Option<u8>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.message.into_inner(), wire.percent).map_err(de::Error::custom)
    }
}

/// Queue depth warning body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueDepthWarning {
    /// Current depth.
    pub depth: u32,
    /// Configured limit.
    pub limit: u32,
}

/// Provider heartbeat body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderHeartbeat {
    provider: Arc<str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<Arc<str>>,
}

impl ProviderHeartbeat {
    /// Construct a heartbeat.
    ///
    /// # Errors
    ///
    /// Returns [`EventError`] when labels fail validation.
    pub fn try_new(
        provider: impl AsRef<str>,
        detail: Option<impl AsRef<str>>,
    ) -> Result<Self, EventError> {
        Ok(Self {
            provider: validated_label(provider.as_ref(), "provider")?,
            detail: match detail {
                Some(value) => Some(validated_text(value.as_ref(), "detail")?),
                None => None,
            },
        })
    }
}

impl<'de> Deserialize<'de> for ProviderHeartbeat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            provider: BoundedString<LABEL_MAX_BYTES>,
            #[serde(default)]
            detail: Option<BoundedString<TEXT_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.provider.into_inner(),
            wire.detail.map(BoundedString::into_inner),
        )
        .map_err(de::Error::custom)
    }
}
