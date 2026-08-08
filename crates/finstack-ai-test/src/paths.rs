//! Repository path helpers for fixture and schema discovery.

use std::path::{Path, PathBuf};

/// Return the repository root containing `schemas/` and `fixtures/`.
///
/// # Panics
///
/// Panics only when the package is built outside the expected workspace layout.
#[must_use]
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("finstack-ai-test must live under the workspace crates/ tree")
}

/// Resolve `schemas/<family>/v<major>/<kind>.schema.json`.
#[must_use]
pub fn schema_path(family: &str, major: u32, kind: &str) -> PathBuf {
    repo_root().join(format!("schemas/{family}/v{major}/{kind}.schema.json"))
}

/// Resolve a compatibility fixture path under `fixtures/compatibility/`.
#[must_use]
pub fn compatibility_fixture(relative: impl AsRef<Path>) -> PathBuf {
    repo_root()
        .join("fixtures/compatibility")
        .join(relative.as_ref())
}
