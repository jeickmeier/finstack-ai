//! Provider-neutral model support contracts.

mod credentials;
mod media;
mod secret;

pub use credentials::{Authentication, CredentialReference, CredentialRejected, CredentialStore};
pub use media::{
    MediaResolveError, MediaResolveKind, MediaResolver, ResolveDraftMediaError, ResolvedMedia,
    resolve_draft_media,
};
pub use secret::{SECRET_MAX_BYTES, SecretRejected, SecretString, secret_is_valid};

/// Budget tiers for the portable `thinking_level` setting.
///
/// Every leaf that honors `thinking_level` maps `low`/`medium`/`high` through
/// this one table so the setting means the same token budget on every
/// provider; returns `None` for values outside the allowlist.
#[must_use]
pub fn thinking_level_budget(level: &str) -> Option<u64> {
    match level {
        "low" => Some(1_024),
        "medium" => Some(4_096),
        "high" => Some(8_192),
        _ => None,
    }
}
