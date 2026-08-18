//! Config-driven multi-protocol implementation of the public [`Model`](finstack_ai_runtime::Model) port.

#![warn(missing_docs)]

mod config;
mod provider;

pub use config::{
    Authentication, CredentialReference, CredentialStore, GatewayCapabilityFlags,
    GatewayModelConfig, GatewayModelSpec, GatewayRouteConfig, SecretString, WireProtocol,
};
pub use provider::GatewayProvider;
