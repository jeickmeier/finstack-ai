//! Host-owned permission grants. A name is not a linked capability.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::error::PluginHostError;

/// Catalog names accepted on manifests and host grant sets.
pub const CATALOG: [&str; 8] = [
    "logging",
    "blobs",
    "http",
    "filesystem",
    "network",
    "secrets",
    "clock",
    "random",
];

/// Default host application grants so plugin-grant contract echo fixtures keep working.
#[must_use]
pub fn default_application_grants() -> BTreeSet<String> {
    ["logging", "blobs"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// One filesystem preopen. A grant name without a preopen is not linked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemPreopen {
    /// Guest-visible path.
    pub guest_path: String,
    /// Host directory. Tests use a tempdir; never inherit `$HOME`.
    /// Validated fail-closed by [`validate_preopen_host_path`].
    pub host_path: PathBuf,
    /// Read permission on the preopen.
    pub read: bool,
    /// Write permission on the preopen.
    pub write: bool,
}

/// Sensitive host roots that can never back a preopen.
const SENSITIVE_ROOTS: [&str; 11] = [
    "/", "/etc", "/usr", "/bin", "/sbin", "/var", "/proc", "/sys", "/dev", "/boot", "/root",
];

/// Fail-closed validation of one preopen host path.
///
/// # Errors
///
/// Returns [`PluginHostError::ConfigInvalid`] when the path is empty,
/// contains a `..` component, or names a sensitive root (`/`, `/etc`,
/// `/usr`, `/bin`, `/sbin`, `/var`, `/proc`, `/sys`, `/dev`, `/boot`,
/// `/root`, or the user's home directory).
pub fn validate_preopen_host_path(host_path: &Path) -> Result<(), PluginHostError> {
    if host_path.as_os_str().is_empty() {
        return Err(PluginHostError::ConfigInvalid(
            "preopen host path must not be empty",
        ));
    }
    if host_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(PluginHostError::ConfigInvalid(
            "preopen host path must not contain '..'",
        ));
    }
    if SENSITIVE_ROOTS
        .iter()
        .any(|root| host_path == Path::new(root))
    {
        return Err(PluginHostError::ConfigInvalid(
            "preopen host path names a sensitive root",
        ));
    }
    if let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
        && host_path == Path::new(&home)
    {
        return Err(PluginHostError::ConfigInvalid(
            "preopen host path must not be the home directory",
        ));
    }
    Ok(())
}

/// Concrete resources required before WASI interfaces are linked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrantResources {
    /// Preopens backing a `filesystem` grant.
    pub filesystem: Vec<FilesystemPreopen>,
    /// Hostname allowlist backing an `http` grant.
    pub http_hosts: BTreeSet<String>,
    /// Explicit socket-grant flag backing a `network` grant.
    pub sockets: bool,
}

impl GrantResources {
    /// Whether `filesystem` may be linked.
    #[must_use]
    pub fn filesystem_linkable(&self) -> bool {
        !self.filesystem.is_empty()
    }

    /// Whether `http` may be linked.
    #[must_use]
    pub fn http_linkable(&self) -> bool {
        !self.http_hosts.is_empty()
    }

    /// Whether `network` / sockets may be linked.
    #[must_use]
    pub const fn network_linkable(&self) -> bool {
        self.sockets
    }
}

/// Validate a host-offered grant set. `secrets` cannot be offered in this PR.
///
/// # Errors
///
/// Returns [`PluginHostError::ConfigInvalid`] for unknown names or `secrets`.
pub fn validate_application_grants(grants: &BTreeSet<String>) -> Result<(), PluginHostError> {
    for grant in grants {
        if grant == "secrets" {
            return Err(PluginHostError::ConfigInvalid(
                "secrets provider is not available",
            ));
        }
        if !CATALOG.contains(&grant.as_str()) {
            return Err(PluginHostError::ConfigInvalid("undeclared host permission"));
        }
    }
    Ok(())
}

/// Fail closed when the manifest requests a permission the host does not offer.
///
/// # Errors
///
/// Returns [`PluginHostError::PermissionDenied`] for the first missing grant.
pub fn require_offered(
    requested: &[String],
    offered: &BTreeSet<String>,
) -> Result<BTreeSet<String>, PluginHostError> {
    let mut granted = BTreeSet::new();
    for permission in requested {
        if !offered.contains(permission) {
            return Err(PluginHostError::PermissionDenied(permission.clone()));
        }
        granted.insert(permission.clone());
    }
    Ok(granted)
}

/// Whether `name` is in `granted` and has the concrete resource required to link.
#[must_use]
pub fn can_link(name: &str, granted: &BTreeSet<String>, resources: &GrantResources) -> bool {
    if !granted.contains(name) {
        return false;
    }
    match name {
        "filesystem" => resources.filesystem_linkable(),
        "http" => resources.http_linkable(),
        "network" => resources.network_linkable(),
        "clock" | "random" | "logging" | "blobs" => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        default_application_grants, require_offered, validate_application_grants,
        validate_preopen_host_path,
    };
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn default_grants_are_logging_and_blobs() {
        let grants = default_application_grants();
        assert!(grants.contains("logging"));
        assert!(grants.contains("blobs"));
        assert!(!grants.contains("filesystem"));
    }

    #[test]
    fn secrets_cannot_be_offered() {
        let mut grants = BTreeSet::new();
        grants.insert("secrets".into());
        assert_eq!(
            validate_application_grants(&grants)
                .expect_err("secrets")
                .code(),
            "plugin_registration_invalid"
        );
    }

    #[test]
    fn unoffered_filesystem_is_denied() {
        let offered = default_application_grants();
        let error = require_offered(&["filesystem".into()], &offered).expect_err("fs");
        assert_eq!(error.code(), "plugin_permission_denied");
    }

    #[test]
    fn sensitive_roots_cannot_back_a_preopen() {
        for root in [
            "/", "/etc", "/usr", "/bin", "/sbin", "/var", "/proc", "/sys", "/dev", "/boot", "/root",
        ] {
            assert_eq!(
                validate_preopen_host_path(Path::new(root))
                    .expect_err(root)
                    .code(),
                "plugin_registration_invalid"
            );
        }
    }

    #[test]
    fn home_directory_cannot_back_a_preopen() {
        if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
            assert_eq!(
                validate_preopen_host_path(Path::new(&home))
                    .expect_err("home")
                    .code(),
                "plugin_registration_invalid"
            );
        }
    }

    #[test]
    fn parent_traversal_and_empty_paths_are_rejected() {
        assert_eq!(
            validate_preopen_host_path(Path::new("/tmp/sandbox/../../etc"))
                .expect_err("traversal")
                .code(),
            "plugin_registration_invalid"
        );
        assert_eq!(
            validate_preopen_host_path(Path::new(""))
                .expect_err("empty")
                .code(),
            "plugin_registration_invalid"
        );
    }

    #[test]
    fn tempdir_path_is_accepted() {
        let dir = tempfile::tempdir().expect("tempdir");
        validate_preopen_host_path(dir.path()).expect("tempdir preopen");
    }
}
