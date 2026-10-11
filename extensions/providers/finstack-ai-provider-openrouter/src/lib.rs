//! `OpenRouter` Responses (`/api/v1/responses`) implementation of the public [`Model`](finstack_ai_runtime::ports::model::Model) port.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

mod catalog;
mod config;
mod error;
mod provider;
mod request;

pub use catalog::model_configs_from_catalog_json;
pub use config::{OpenRouterConfig, OpenRouterModelConfig, SecretHeader};
pub use finstack_ai_runtime::ports::model::{
    Authentication, CredentialReference, CredentialRejected, CredentialStore, SecretRejected,
    SecretString,
};
pub use provider::OpenRouterProvider;
