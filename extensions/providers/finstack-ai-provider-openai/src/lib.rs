//! Official `OpenAI` Responses (`/v1/responses`) implementation of the public [`Model`](finstack_ai_runtime::Model) port.

#![warn(missing_docs)]

mod config;
mod error;
mod provider;
mod request;
mod sse;
mod stream;

pub use config::{Authentication, OpenAiConfig, OpenAiModelConfig, SecretHeader, SecretString};
pub use provider::OpenAiProvider;
