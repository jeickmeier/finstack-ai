//! Repository path helpers for fixture and schema discovery.

use std::path::{Component, Path, PathBuf};

use thiserror::Error;

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
///
/// Rejects absolute paths and `..` traversal so fixture-controlled relative
/// paths cannot escape the corpus root.
///
/// # Panics
///
/// Panics when `relative` escapes `fixtures/compatibility/` or is absolute.
#[must_use]
pub fn compatibility_fixture(relative: impl AsRef<Path>) -> PathBuf {
    try_compatibility_fixture(relative).expect("trusted compatibility fixture path")
}

pub(crate) fn try_compatibility_fixture(
    relative: impl AsRef<Path>,
) -> Result<PathBuf, FixturePathError> {
    let relative = relative.as_ref();
    if relative.is_absolute() {
        return Err(FixturePathError::Absolute);
    }
    if relative
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(FixturePathError::ParentTraversal);
    }
    let root = repo_root()
        .join("fixtures/compatibility")
        .canonicalize()
        .map_err(|_| FixturePathError::RootUnavailable)?;
    let joined = root.join(relative);
    let canonical = joined.canonicalize().unwrap_or(joined);
    if !canonical.starts_with(&root) {
        return Err(FixturePathError::EscapesRoot);
    }
    Ok(canonical)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum FixturePathError {
    #[error("compatibility fixture path must be relative")]
    Absolute,
    #[error("compatibility fixture path must not contain '..'")]
    ParentTraversal,
    #[error("compatibility fixture root is unavailable")]
    RootUnavailable,
    #[error("compatibility fixture path escapes corpus root")]
    EscapesRoot,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_fixture_path_rejects_absolute_and_parent_traversal() {
        assert!(try_compatibility_fixture(Path::new("../Cargo.toml")).is_err());
        assert!(try_compatibility_fixture(Path::new("/etc/passwd")).is_err());
    }
}
