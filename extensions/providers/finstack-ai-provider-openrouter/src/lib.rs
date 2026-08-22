//! `OpenRouter` Responses (`/api/v1/responses`) implementation of the public [`Model`](finstack_ai_runtime::ports::model::Model) port.

#![warn(missing_docs)]

mod catalog;
mod config;
mod error;
mod provider;
mod request;
mod sse;
mod stream;

pub use catalog::model_configs_from_catalog_json;
pub use config::{OpenRouterConfig, OpenRouterModelConfig, SecretHeader};
pub use finstack_ai_runtime::{
    Authentication, CredentialReference, CredentialRejected, CredentialStore, SecretRejected,
    SecretString,
};
pub use provider::OpenRouterProvider;
