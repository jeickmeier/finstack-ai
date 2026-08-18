//! Native Ollama `/api/chat` implementation of the public `Model` port.

#![warn(missing_docs)]

mod config;
mod error;
mod ndjson;
mod provider;
mod request;

pub use config::{OllamaConfig, OllamaModelConfig};
pub use finstack_ai_runtime::{
    Authentication, CredentialReference, CredentialRejected, CredentialStore, SecretRejected,
    SecretString,
};
pub use provider::OllamaProvider;
