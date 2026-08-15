//! Anthropic Messages implementation of the public `Model` port.

#![warn(missing_docs)]

mod config;
mod error;
mod provider;
mod request;
mod sse;

/// Default Anthropic Messages API version header value.
pub const ANTHROPIC_MESSAGES_VERSION: &str = "2023-06-01";

pub use config::{
    AnthropicConfig, AnthropicModelConfig, Authentication, SecretHeader, SecretString,
};
pub use provider::AnthropicProvider;
