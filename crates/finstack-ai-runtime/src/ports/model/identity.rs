use core::fmt;
use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{Metadata, label_is_valid};
use serde::de;
use serde::{Deserialize, Deserializer, Serialize};

use super::error::ModelError;
use super::{MODEL_PROFILE_INVALID, MODEL_REQUEST_INVALID};

/// Validated provider model name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ModelName(Arc<str>);

impl ModelName {
    /// Construct a non-empty bounded model name.
    ///
    /// # Errors
    ///
    /// Returns a stable request error when the name is empty, oversized, or NUL-bearing.
    pub fn try_new(value: impl AsRef<str>) -> Result<Self, ModelError> {
        Ok(Self(validated_label(value.as_ref())?))
    }

    /// Borrow the model name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ModelName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<'de> Deserialize<'de> for ModelName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_new(value).map_err(de::Error::custom)
    }
}

/// Immutable provider/model descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDescriptor {
    /// Provider identity.
    pub provider: Arc<str>,
    /// Supported provider model names.
    pub models: Arc<[ModelName]>,
    /// Bounded provider-specific descriptor metadata.
    #[serde(default)]
    pub metadata: Metadata,
}

impl ModelDescriptor {
    /// Maximum supported model names on one provider descriptor.
    pub const MAX_MODELS: usize = 256;

    /// Validate descriptor labels, cardinality, and uniqueness.
    ///
    /// # Errors
    ///
    /// Returns `model_profile_invalid` for an invalid provider or model set.
    pub fn validate(&self) -> Result<(), ModelError> {
        if !label_is_valid(&self.provider) {
            return Err(ModelError::validation(
                MODEL_PROFILE_INVALID,
                "model provider identity is invalid",
            ));
        }
        if self.models.is_empty() || self.models.len() > Self::MAX_MODELS {
            return Err(ModelError::validation(
                MODEL_PROFILE_INVALID,
                "model descriptor has an invalid model count",
            ));
        }
        let unique = self.models.iter().collect::<BTreeSet<_>>();
        if unique.len() != self.models.len() {
            return Err(ModelError::validation(
                MODEL_PROFILE_INVALID,
                "model descriptor contains duplicate model names",
            ));
        }
        Ok(())
    }
}

pub(super) fn validated_label(value: &str) -> Result<Arc<str>, ModelError> {
    if !label_is_valid(value) {
        return Err(ModelError::validation(
            MODEL_REQUEST_INVALID,
            "model label is empty, oversized, or contains NUL",
        ));
    }
    Ok(Arc::from(value))
}

impl fmt::Display for ModelName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
