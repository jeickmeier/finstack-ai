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
//! Signed callback-token ingress for authenticated external effect completions.

mod audit;
mod config;
mod ingress;
mod token;

pub use config::{
    CompletionIngressConfig, CompletionIngressConfigError, MAX_VERIFICATION_KEYS, MIN_KEY_BYTES,
};
pub use ingress::{CompletionGrant, CompletionIngress, IngressError, MAX_BODY_BYTES, MintError};
pub use token::{CallbackToken, MAX_TOKEN_BYTES};
