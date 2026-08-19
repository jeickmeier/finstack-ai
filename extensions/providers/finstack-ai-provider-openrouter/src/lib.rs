//! `OpenRouter` Responses (`/api/v1/responses`) implementation of the public [`Model`](finstack_ai_runtime::Model) port.

#![warn(missing_docs)]

mod config;
mod error;

pub use config::{OpenRouterConfig, OpenRouterModelConfig, SecretHeader};
pub use finstack_ai_runtime::{
    Authentication, CredentialReference, CredentialRejected, CredentialStore, SecretRejected,
    SecretString,
};
