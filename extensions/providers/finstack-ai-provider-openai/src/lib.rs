//! Official `OpenAI` Responses (`/v1/responses`) implementation of the public [`Model`](finstack_ai_runtime::Model) port.

#![warn(missing_docs)]

mod config;
mod error;
mod provider;
mod request;
mod sse;
mod stream;

pub use config::{OpenAiConfig, OpenAiModelConfig, SecretHeader};
pub use finstack_ai_runtime::{
    Authentication, CredentialReference, CredentialRejected, CredentialStore, SecretRejected,
    SecretString,
};
pub use provider::OpenAiProvider;
