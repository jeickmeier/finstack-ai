//! Runtime error wrapper that may carry a local Rust source chain.
//!
//! Durable and kernel boundaries must use [`ErrorDescriptor`] only.

use core::fmt;
use std::error::Error as StdError;
use std::sync::Arc;

use finstack_ai_kernel::ErrorDescriptor;

/// Runtime error with an optional local diagnostic source chain.
#[derive(Debug, Clone)]
pub struct FrameworkError {
    /// Source-free serializable descriptor.
    pub descriptor: ErrorDescriptor,
    /// Local diagnostic source; never persisted or hashed.
    pub source: Option<Arc<dyn StdError + Send + Sync>>,
}

impl FrameworkError {
    /// Construct a framework error without a local source chain.
    #[must_use]
    pub fn from_descriptor(descriptor: ErrorDescriptor) -> Self {
        Self {
            descriptor,
            source: None,
        }
    }

    /// Attach a local diagnostic source.
    #[must_use]
    pub fn with_source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Arc::new(source));
        self
    }

    /// Normalize to the source-free descriptor required at kernel / durable boundaries.
    #[must_use]
    pub fn into_descriptor(self) -> ErrorDescriptor {
        self.descriptor
    }

    /// Borrow the source-free descriptor.
    #[must_use]
    pub fn descriptor(&self) -> &ErrorDescriptor {
        &self.descriptor
    }
}

impl fmt::Display for FrameworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.descriptor)
    }
}

impl StdError for FrameworkError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_ref()
            .map(|error| error.as_ref() as &(dyn StdError + 'static))
    }
}
