//! Repository path helpers for fixture and schema discovery.

use std::path::{Component, Path, PathBuf};

use thiserror::Error;

/// Return the repository root containing `schemas/` and `fixtures/`.
///
/// Falls back to the unresolved workspace-relative path when canonicalize fails
/// (for example, a missing parent during an unusual checkout).
#[must_use]
pub fn repo_root() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    path.canonicalize().unwrap_or(path)
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
/// Invalid relative paths fall back to the unresolved join so callers still
/// receive a path; later I/O then fails instead of this helper panicking.
#[must_use]
pub fn compatibility_fixture(relative: impl AsRef<Path>) -> PathBuf {
    let relative = relative.as_ref();
    try_compatibility_fixture(relative)
        .unwrap_or_else(|_| repo_root().join("fixtures/compatibility").join(relative))
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
