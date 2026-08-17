//! Native Ollama `/api/chat` implementation of the public `Model` port.

#![warn(missing_docs)]

mod config;
mod error;
mod ndjson;
mod provider;
mod request;

pub use config::{Authentication, OllamaConfig, OllamaModelConfig, SecretString};
pub use provider::OllamaProvider;
