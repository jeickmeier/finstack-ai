//! `ArtifactStore` backed by S3-compatible or local blob storage.
//!
//! Artifact storage is the one public storage concept. This crate holds the
//! artifact algorithm -- the `artifacts/v2/{reference-digest}` key scheme, the
//! versioned envelope that preserves the complete [`ArtifactRef`] beside the
//! bytes, and the pin / orphan-collection protocol -- written once over a
//! private blob driver, plus the two drivers themselves.
//!
//! [`ArtifactRef`]: finstack_ai_kernel::ArtifactRef
//!
//! Enable the `s3` feature for [`S3ArtifactStore`] and the `local` feature for
//! [`LocalArtifactStore`]; neither is on by default.
//!
//! The blob driver is deliberately private. It exists so one algorithm serves
//! both drivers, not as a storage abstraction consumers implement -- an
//! artifact store is what the runtime and every toolset actually bind to.

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

#[cfg(any(feature = "s3", feature = "local", test))]
mod artifact;
#[cfg(any(feature = "s3", feature = "local", test))]
mod driver;
#[cfg(feature = "local")]
mod local;
#[cfg(feature = "s3")]
mod s3;
#[cfg(any(feature = "s3", feature = "local"))]
mod stores;

/// Default artifact byte ceiling: 64 MiB, clamped to the backing object
/// store's own single-object limit.
pub const DEFAULT_MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

#[cfg(feature = "s3")]
pub use s3::{Addressing, S3ObjectStoreConfig};
#[cfg(feature = "local")]
pub use stores::LocalArtifactStore;
#[cfg(feature = "s3")]
pub use stores::S3ArtifactStore;

#[cfg(test)]
mod artifact_tests;
#[cfg(test)]
mod driver_fake;
