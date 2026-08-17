//! Finite bundle/catalog resolution and credential-free exact locks.

mod catalog;
mod compose;
mod error;
mod lock;
mod resolver;
mod secret;
mod types;

#[cfg(test)]
mod tests;

/// Current strict bundle and lock schema version.
pub const BUNDLE_SCHEMA_VERSION: u16 = 1;
/// Stable code for malformed bundle/lock input.
pub const BUNDLE_RESOLUTION_INVALID: &str = "bundle_resolution_invalid";
/// Stable code for a missing bundle, agent, capability, component, or service.
pub const BUNDLE_RESOLUTION_MISSING: &str = "bundle_resolution_missing";
/// Stable code for a finite requirement or declared conflict.
pub const BUNDLE_RESOLUTION_CONFLICT: &str = "bundle_resolution_conflict";
/// Stable code for exact lock reconstruction mismatch.
pub const BUNDLE_RESOLUTION_LOCK_MISMATCH: &str = "bundle_resolution_lock_mismatch";

pub(crate) use catalog::CompositionRecipe;
pub use catalog::{BundleCatalog, RuntimeServices};
pub use error::BundleResolutionError;
pub use lock::{
    LockedBundle, LockedCapability, LockedComponent, LockedComponentKind, RequiredServices,
    ResolvedAgentLock,
};
pub use resolver::BundleResolver;
pub use types::{
    BundleConflict, BundleDefaults, BundleRequirement, BundleSpec, CompatibilityRequirements,
    HostFeature, VersionRequirement,
};
