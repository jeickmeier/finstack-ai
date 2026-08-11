//! OpenAI-compatible Chat Completions implementation of the public `Model` port.

#![warn(missing_docs)]

mod config;
mod error;
mod provider;
mod quirks;
mod request;
mod sse;

pub use config::{
    Authentication, OpenAiCompatibleConfig, OpenAiModelConfig, SecretHeader, SecretString,
};
pub use provider::OpenAiCompatibleProvider;
pub use quirks::{AuthenticationConvention, ENDPOINT_QUIRKS_VERSION, EndpointKind, EndpointQuirks};
