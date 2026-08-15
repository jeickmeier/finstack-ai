//! Local plugin lockfile parse and path sanitizer. No registry fetch.

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use finstack_ai_wit::MAX_RAW_JSON_BYTES;
use serde::Deserialize;

use crate::error::PluginHostError;
use crate::host::PluginWorld;

const LOCKFILE_VERSION: u32 = 1;
const IDENTITY_PREFIX: &str = "finstack.plugin.";
const EXPERIMENTAL_VERSION: &str = "0.0.4";

/// One resolved lock entry. Paths are relative to the lockfile directory
/// and also stored as joined filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPlugin {
    /// Published `finstack.plugin.*` identity.
    pub identity: String,
    /// Experimental package version. Must be `0.0.4`.
    pub version: String,
    /// Whether [`crate::PluginHost::load_enabled`] should load this entry.
    pub enabled: bool,
    /// Component path as written in the lockfile.
    pub component: String,
    /// Component path resolved against the lockfile directory.
    pub component_path: PathBuf,
    /// Hex SHA-256 of the component bytes ([`crate::component_digest`]).
    pub component_digest: String,
    /// Manifest path as written in the lockfile.
    pub manifest: String,
    /// Manifest path resolved against the lockfile directory.
    pub manifest_path: PathBuf,
    /// Host-computed [`finstack_ai_wit::PluginManifest::digest`].
    pub manifest_digest: String,
}

/// Parsed lockfile. [`Self::enabled`] filters; it does not search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPluginLock {
    directory: PathBuf,
    plugins: Vec<LockedPlugin>,
}

impl ResolvedPluginLock {
    /// Directory that relative component and manifest paths resolve against.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Every lock entry, including disabled ones, in lockfile order.
    #[must_use]
    pub fn plugins(&self) -> &[LockedPlugin] {
        &self.plugins
    }

    /// Enabled entries only, in lockfile order.
    pub fn enabled(&self) -> impl Iterator<Item = &LockedPlugin> {
        self.plugins.iter().filter(|entry| entry.enabled)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LockfileWire {
    lockfile_version: u32,
    plugins: Vec<LockedPluginWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedPluginWire {
    identity: String,
    version: String,
    enabled: bool,
    component: String,
    component_digest: String,
    manifest: String,
    manifest_digest: String,
}

/// Parse one local lockfile. Component bytes are not read.
///
/// # Errors
///
/// Returns [`PluginHostError::LockNotFound`] when the file is missing,
/// [`PluginHostError::LockDuplicate`] when two entries share an identity,
/// and [`PluginHostError::LockInvalid`] for oversize JSON, unknown fields,
/// a non-`1` version, or a path that is empty, absolute, contains `..`,
/// or looks like a URL.
pub fn resolve_lockfile(path: impl AsRef<Path>) -> Result<ResolvedPluginLock, PluginHostError> {
    let path = path.as_ref();
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(PluginHostError::LockNotFound);
        }
        Err(error) => {
            return Err(PluginHostError::LockInvalid(format!(
                "lockfile could not be read: {error}"
            )));
        }
    };
    if bytes.len() > MAX_RAW_JSON_BYTES {
        return Err(PluginHostError::LockInvalid(format!(
            "lockfile is {} bytes; max {MAX_RAW_JSON_BYTES}",
            bytes.len()
        )));
    }
    let wire: LockfileWire = serde_json::from_slice(&bytes).map_err(|error| {
        PluginHostError::LockInvalid(format!("lockfile JSON is invalid: {error}"))
    })?;
    if wire.lockfile_version != LOCKFILE_VERSION {
        return Err(PluginHostError::LockInvalid(format!(
            "lockfile_version must be {LOCKFILE_VERSION}"
        )));
    }
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut seen = BTreeSet::new();
    let mut plugins = Vec::with_capacity(wire.plugins.len());
    for entry in wire.plugins {
        if !seen.insert(entry.identity.clone()) {
            return Err(PluginHostError::LockDuplicate);
        }
        plugins.push(resolve_entry(&directory, entry)?);
    }
    Ok(ResolvedPluginLock { directory, plugins })
}

fn resolve_entry(
    directory: &Path,
    entry: LockedPluginWire,
) -> Result<LockedPlugin, PluginHostError> {
    if !entry.identity.starts_with(IDENTITY_PREFIX) {
        return Err(PluginHostError::LockInvalid(
            "identity must start with finstack.plugin.".into(),
        ));
    }
    if entry.version != EXPERIMENTAL_VERSION {
        return Err(PluginHostError::LockInvalid(
            "version must be experimental 0.0.4".into(),
        ));
    }
    let component = sanitize_relative_path(&entry.component)?;
    let manifest = sanitize_relative_path(&entry.manifest)?;
    Ok(LockedPlugin {
        identity: entry.identity,
        version: entry.version,
        enabled: entry.enabled,
        component: entry.component,
        component_path: directory.join(component),
        component_digest: entry.component_digest,
        manifest: entry.manifest,
        manifest_path: directory.join(manifest),
        manifest_digest: entry.manifest_digest,
    })
}

/// Infer the hosted world from declared manifest worlds.
///
/// # Errors
///
/// Returns [`PluginHostError::LockInvalid`] unless the manifest declares
/// exactly one of `toolset-plugin` or `context-plugin`.
pub(crate) fn world_from_manifest(
    manifest: &finstack_ai_wit::PluginManifest,
) -> Result<PluginWorld, PluginHostError> {
    let toolset = manifest
        .worlds
        .iter()
        .any(|world| world == PluginWorld::Toolset.as_str());
    let context = manifest
        .worlds
        .iter()
        .any(|world| world == PluginWorld::Context.as_str());
    match (toolset, context) {
        (true, false) => Ok(PluginWorld::Toolset),
        (false, true) => Ok(PluginWorld::Context),
        _ => Err(PluginHostError::LockInvalid(
            "lock entry must declare exactly one of toolset-plugin or context-plugin".into(),
        )),
    }
}

fn sanitize_relative_path(path: &str) -> Result<PathBuf, PluginHostError> {
    if path.is_empty() || path.contains('\0') || has_url_scheme(path) {
        return Err(PluginHostError::LockInvalid(
            "lock path must be a relative local file".into(),
        ));
    }
    let parsed = Path::new(path);
    if parsed.is_absolute() {
        return Err(PluginHostError::LockInvalid(
            "lock path must not be absolute".into(),
        ));
    }
    if parsed
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(PluginHostError::LockInvalid(
            "lock path must not contain ..".into(),
        ));
    }
    Ok(parsed.to_path_buf())
}

fn has_url_scheme(path: &str) -> bool {
    if path.contains("://") {
        return true;
    }
    path.split_once(':').is_some_and(|(scheme, rest)| {
        !rest.starts_with('/')
            && !scheme.is_empty()
            && scheme.bytes().all(|byte| byte.is_ascii_alphabetic())
    })
}

#[cfg(test)]
mod tests {
    use super::{LockedPlugin, resolve_lockfile, sanitize_relative_path, world_from_manifest};
    use crate::cache::component_digest;
    use crate::host::{InstancePolicy, PluginHost, PluginHostConfig, PluginWorld};
    use finstack_ai_wit::{manifest_digest_hex, parse_manifest};
    use std::fs;
    use std::path::PathBuf;

    fn write_lock(dir: &std::path::Path, body: &str) -> PathBuf {
        let path = dir.join("plugin.lock.json");
        fs::write(&path, body).expect("write lock");
        path
    }

    fn placeholder_hex() -> String {
        "ab".repeat(32)
    }

    #[test]
    fn missing_lockfile_is_not_found() {
        let error =
            resolve_lockfile("/tmp/finstack-missing-plugin.lock.json").expect_err("missing");
        assert_eq!(error.code(), "plugin_lock_not_found");
    }

    #[test]
    fn duplicate_identity_fails_including_disabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hex = placeholder_hex();
        let path = write_lock(
            dir.path(),
            &format!(
                r#"{{
  "lockfile_version": 1,
  "plugins": [
    {{
      "identity": "finstack.plugin.calculator",
      "version": "0.0.4",
      "enabled": true,
      "component": "calculator/component.wasm",
      "component_digest": "{hex}",
      "manifest": "calculator/plugin.manifest.json",
      "manifest_digest": "{hex}"
    }},
    {{
      "identity": "finstack.plugin.calculator",
      "version": "0.0.4",
      "enabled": false,
      "component": "other/component.wasm",
      "component_digest": "{hex}",
      "manifest": "other/plugin.manifest.json",
      "manifest_digest": "{hex}"
    }}
  ]
}}
"#
            ),
        );
        let error = resolve_lockfile(&path).expect_err("duplicate");
        assert_eq!(error.code(), "plugin_lock_duplicate");
    }

    #[test]
    fn registry_url_path_is_invalid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hex = placeholder_hex();
        let path = write_lock(
            dir.path(),
            &format!(
                r#"{{
  "lockfile_version": 1,
  "plugins": [
    {{
      "identity": "finstack.plugin.calculator",
      "version": "0.0.4",
      "enabled": true,
      "component": "https://example.invalid/p.wasm",
      "component_digest": "{hex}",
      "manifest": "calculator/plugin.manifest.json",
      "manifest_digest": "{hex}"
    }}
  ]
}}
"#
            ),
        );
        let error = resolve_lockfile(&path).expect_err("url");
        assert_eq!(error.code(), "plugin_lock_invalid");
    }

    #[test]
    fn parent_dir_and_absolute_paths_are_invalid() {
        assert_eq!(
            sanitize_relative_path("../evil.wasm")
                .expect_err("parent")
                .code(),
            "plugin_lock_invalid"
        );
        assert_eq!(
            sanitize_relative_path("/tmp/p.wasm")
                .expect_err("absolute")
                .code(),
            "plugin_lock_invalid"
        );
        assert_eq!(
            sanitize_relative_path("git:example.invalid/p.wasm")
                .expect_err("scheme")
                .code(),
            "plugin_lock_invalid"
        );
    }

    #[test]
    fn unknown_field_and_bad_version_are_invalid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hex = placeholder_hex();
        let unknown = write_lock(
            dir.path(),
            r#"{
  "lockfile_version": 1,
  "registry": "https://example.invalid",
  "plugins": []
}
"#,
        );
        assert_eq!(
            resolve_lockfile(&unknown).expect_err("unknown").code(),
            "plugin_lock_invalid"
        );
        let version = write_lock(
            dir.path(),
            &format!(
                r#"{{
  "lockfile_version": 2,
  "plugins": [
    {{
      "identity": "finstack.plugin.calculator",
      "version": "0.0.4",
      "enabled": true,
      "component": "calculator/component.wasm",
      "component_digest": "{hex}",
      "manifest": "calculator/plugin.manifest.json",
      "manifest_digest": "{hex}"
    }}
  ]
}}
"#
            ),
        );
        assert_eq!(
            resolve_lockfile(&version).expect_err("version").code(),
            "plugin_lock_invalid"
        );
    }

    #[test]
    fn missing_enabled_is_invalid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hex = placeholder_hex();
        let path = write_lock(
            dir.path(),
            &format!(
                r#"{{
  "lockfile_version": 1,
  "plugins": [
    {{
      "identity": "finstack.plugin.calculator",
      "version": "0.0.4",
      "component": "calculator/component.wasm",
      "component_digest": "{hex}",
      "manifest": "calculator/plugin.manifest.json",
      "manifest_digest": "{hex}"
    }}
  ]
}}
"#
            ),
        );
        assert_eq!(
            resolve_lockfile(&path).expect_err("enabled").code(),
            "plugin_lock_invalid"
        );
    }

    #[test]
    fn enabled_filters_without_searching() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hex = placeholder_hex();
        let path = write_lock(
            dir.path(),
            &format!(
                r#"{{
  "lockfile_version": 1,
  "plugins": [
    {{
      "identity": "finstack.plugin.calculator",
      "version": "0.0.4",
      "enabled": true,
      "component": "calculator/component.wasm",
      "component_digest": "{hex}",
      "manifest": "calculator/plugin.manifest.json",
      "manifest_digest": "{hex}"
    }},
    {{
      "identity": "finstack.plugin.filesystem.sandbox",
      "version": "0.0.4",
      "enabled": false,
      "component": "filesystem-sandbox/component.wasm",
      "component_digest": "{hex}",
      "manifest": "filesystem-sandbox/plugin.manifest.json",
      "manifest_digest": "{hex}"
    }}
  ]
}}
"#
            ),
        );
        let resolved = resolve_lockfile(&path).expect("resolve");
        let enabled: Vec<&str> = resolved
            .enabled()
            .map(|entry| entry.identity.as_str())
            .collect();
        assert_eq!(enabled, ["finstack.plugin.calculator"]);
        assert_eq!(resolved.plugins().len(), 2);
        fs::write(dir.path().join("extra-component.wasm"), b"ignored").expect("extra");
        assert_eq!(resolved.plugins().len(), 2);
    }

    #[test]
    fn load_locked_rejects_disabled_and_digest_mismatch() {
        let host = PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2).expect("cfg"),
        )
        .expect("host");
        let dir = tempfile::tempdir().expect("tempdir");
        let wasm = dir.path().join("component.wasm");
        fs::write(&wasm, b"(component)").expect("wasm");
        let digest = component_digest(b"(component)");
        let disabled = LockedPlugin {
            identity: "finstack.plugin.calculator".into(),
            version: "0.0.4".into(),
            enabled: false,
            component: "component.wasm".into(),
            component_path: wasm.clone(),
            component_digest: digest.clone(),
            manifest: "plugin.manifest.json".into(),
            manifest_path: dir.path().join("plugin.manifest.json"),
            manifest_digest: digest.clone(),
        };
        let Err(disabled_error) = host.load_locked(&disabled) else {
            panic!("disabled");
        };
        assert_eq!(disabled_error.code(), "plugin_lock_disabled");
        let mut swapped = disabled.clone();
        swapped.enabled = true;
        swapped.component_digest = "00".repeat(32);
        let Err(digest_error) = host.load_locked(&swapped) else {
            panic!("digest");
        };
        assert_eq!(digest_error.code(), "plugin_lock_digest_mismatch");
    }

    #[test]
    fn world_from_manifest_requires_exactly_one_world() {
        let identity = "finstack.plugin.calculator";
        let worlds = vec!["toolset-plugin".to_owned()];
        let digest = manifest_digest_hex(identity, "0.0.4", &worlds).expect("digest");
        let manifest = parse_manifest(
            &serde_json::to_vec(&serde_json::json!({
                "identity": identity,
                "version": "0.0.4",
                "worlds": worlds,
                "permissions": ["logging"],
                "configuration_schema": {},
                "digest": digest
            }))
            .expect("json"),
        )
        .expect("manifest");
        assert_eq!(
            world_from_manifest(&manifest).expect("world"),
            PluginWorld::Toolset
        );
    }
}
