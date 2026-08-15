//! Isolated Wasmtime engine, host configuration, and compiled-component load.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use finstack_ai_wit::{PluginManifest, parse_manifest, validate_manifest};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine};

use crate::bindings::context::ContextPlugin;
use crate::bindings::toolset::ToolsetPlugin;
use crate::cache::{
    CacheKeyParts, ComponentCache, abi_identity, cache_key, component_digest, engine_fingerprint,
    host_target,
};
use crate::error::PluginHostError;
use crate::grants::{
    FilesystemPreopen, GrantResources, default_application_grants, require_offered,
    validate_application_grants,
};
use crate::instantiate::HostState;
use crate::limits::EffectiveLimits;
use crate::lockfile::{LockedPlugin, resolve_lockfile, world_from_manifest};
use crate::signature::{SignaturePolicy, verify_manifest};

/// Per-call instance/store policy. Worlds do not declare safe reuse, so this
/// crate never pools live instances across components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstancePolicy {
    /// Each call gets its own `Store` and `Instance` from the cached `Component`.
    Exclusive,
    /// One store/instance behind a mutex. Concurrent guest entry fails closed.
    Serialized,
}

/// Which experimental world a compiled component implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginWorld {
    /// `toolset-plugin`.
    Toolset,
    /// `context-plugin`.
    Context,
}

impl PluginWorld {
    /// WIT world name recorded in the ABI cache field.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Toolset => "toolset-plugin",
            Self::Context => "context-plugin",
        }
    }
}

/// Construction options for [`PluginHost`].
#[derive(Debug, Clone)]
pub struct PluginHostConfig {
    cache_dir: Option<PathBuf>,
    instance_policy: InstancePolicy,
    max_concurrent_instances: u32,
    application_grants: BTreeSet<String>,
    signature_policy: SignaturePolicy,
    trust_roots: BTreeMap<String, [u8; 32]>,
    resources: GrantResources,
    default_limits: EffectiveLimits,
}

impl PluginHostConfig {
    /// Build a validated host config.
    ///
    /// `cache_dir` of `None` keeps compiled artifacts in memory. Directory
    /// mode is host-owned and does not use Wasmtime's implicit global cache.
    /// Default grants are `{logging, blobs}`. Signature policy is Permissive.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError::ConfigInvalid`] when `max_concurrent_instances`
    /// is zero.
    pub fn try_new(
        cache_dir: Option<PathBuf>,
        instance_policy: InstancePolicy,
        max_concurrent_instances: u32,
    ) -> Result<Self, PluginHostError> {
        if max_concurrent_instances == 0 {
            return Err(PluginHostError::ConfigInvalid(
                "max_concurrent_instances must be non-zero",
            ));
        }
        Ok(Self {
            cache_dir,
            instance_policy,
            max_concurrent_instances,
            application_grants: default_application_grants(),
            signature_policy: SignaturePolicy::Permissive,
            trust_roots: BTreeMap::new(),
            resources: GrantResources::default(),
            default_limits: EffectiveLimits::default(),
        })
    }

    /// Replace the host-offered grant set.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError::ConfigInvalid`] for `secrets` or unknown names.
    pub fn with_application_grants(
        mut self,
        grants: BTreeSet<String>,
    ) -> Result<Self, PluginHostError> {
        validate_application_grants(&grants)?;
        self.application_grants = grants;
        Ok(self)
    }

    /// Set signature policy. Strict with empty trust roots rejects every package.
    #[must_use]
    pub const fn with_signature_policy(mut self, policy: SignaturePolicy) -> Self {
        self.signature_policy = policy;
        self
    }

    /// Replace ed25519 trust roots keyed by non-secret `key_id`.
    #[must_use]
    pub fn with_trust_roots(mut self, roots: BTreeMap<String, [u8; 32]>) -> Self {
        self.trust_roots = roots;
        self
    }

    /// Replace filesystem preopens. An empty list does not link `wasi:filesystem`.
    #[must_use]
    pub fn with_filesystem_preopens(mut self, preopens: Vec<FilesystemPreopen>) -> Self {
        self.resources.filesystem = preopens;
        self
    }

    /// Replace the HTTP hostname allowlist. Empty does not link `wasi:http`.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError::ConfigInvalid`] when an entry contains
    /// userinfo or a scheme.
    pub fn with_http_allowlist(mut self, hosts: BTreeSet<String>) -> Result<Self, PluginHostError> {
        for host in &hosts {
            if host.contains("://") || host.contains('@') {
                return Err(PluginHostError::ConfigInvalid(
                    "http allowlist entries must be hostnames",
                ));
            }
        }
        self.resources.http_hosts = hosts;
        Ok(self)
    }

    /// Enable or disable the explicit socket grant. Off by default.
    #[must_use]
    pub const fn with_socket_grant(mut self, enabled: bool) -> Self {
        self.resources.sockets = enabled;
        self
    }

    /// Replace experimental host default limits.
    #[must_use]
    pub const fn with_default_limits(mut self, limits: EffectiveLimits) -> Self {
        self.default_limits = limits;
        self
    }

    /// Configured instance policy.
    #[must_use]
    pub const fn instance_policy(&self) -> InstancePolicy {
        self.instance_policy
    }

    /// Configured concurrent-instance ceiling.
    #[must_use]
    pub const fn max_concurrent_instances(&self) -> u32 {
        self.max_concurrent_instances
    }
}

/// Isolated Wasmtime (T3) host. One engine is created per host; there is no
/// process-global engine at crate load.
pub struct PluginHost {
    engine: Engine,
    toolset_linker: Linker<HostState>,
    context_linker: Linker<HostState>,
    cache: ComponentCache,
    instance_policy: InstancePolicy,
    max_concurrent_instances: u32,
    application_grants: BTreeSet<String>,
    signature_policy: SignaturePolicy,
    trust_roots: BTreeMap<String, [u8; 32]>,
    resources: GrantResources,
    default_limits: EffectiveLimits,
}

/// Compiled component retained without a live instance.
#[derive(Clone)]
pub struct ReadyWasm {
    pub(crate) component: Component,
    pub(crate) digest: String,
    pub(crate) world: PluginWorld,
    pub(crate) manifest: PluginManifest,
    pub(crate) granted: BTreeSet<String>,
}

impl PluginHost {
    /// Construct an engine, host-import linker, and compile cache.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the engine or cache cannot be created.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_plugin_host::{InstancePolicy, PluginHost, PluginHostConfig};
    ///
    /// let config = PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 4)
    ///     .expect("config");
    /// let host = PluginHost::try_new(config).expect("host");
    /// assert_eq!(host.instance_policy(), InstancePolicy::Exclusive);
    /// ```
    pub fn try_new(config: PluginHostConfig) -> Result<Self, PluginHostError> {
        let mut wasm_config = Config::new();
        #[allow(deprecated)]
        {
            // Plan lock: request async support. Wasmtime 47 treats this as a no-op.
            wasm_config.async_support(true);
        }
        wasm_config.epoch_interruption(true);
        wasm_config.consume_fuel(true);
        let engine = Engine::new(&wasm_config)
            .map_err(|error| PluginHostError::CompileFailed(format!("engine: {error}")))?;
        let mut toolset_linker = Linker::new(&engine);
        ToolsetPlugin::add_to_linker::<_, HasSelf<_>>(&mut toolset_linker, |state| state).map_err(
            |error| PluginHostError::InstantiateFailed(format!("link toolset: {error}")),
        )?;
        let mut context_linker = Linker::new(&engine);
        ContextPlugin::add_to_linker::<_, HasSelf<_>>(&mut context_linker, |state| state).map_err(
            |error| PluginHostError::InstantiateFailed(format!("link context: {error}")),
        )?;
        let cache = match config.cache_dir {
            Some(dir) => ComponentCache::directory(dir)?,
            None => ComponentCache::memory(),
        };
        Ok(Self {
            engine,
            toolset_linker,
            context_linker,
            cache,
            instance_policy: config.instance_policy,
            max_concurrent_instances: config.max_concurrent_instances,
            application_grants: config.application_grants,
            signature_policy: config.signature_policy,
            trust_roots: config.trust_roots,
            resources: config.resources,
            default_limits: config.default_limits,
        })
    }

    /// Configured instance policy.
    #[must_use]
    pub const fn instance_policy(&self) -> InstancePolicy {
        self.instance_policy
    }

    /// Configured concurrent-instance ceiling.
    #[must_use]
    pub const fn max_concurrent_instances(&self) -> u32 {
        self.max_concurrent_instances
    }

    /// Borrow the process-local engine.
    #[must_use]
    pub const fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Borrow the toolset-world host-import linker.
    #[must_use]
    pub const fn linker(&self) -> &Linker<HostState> {
        &self.toolset_linker
    }

    /// Borrow the context-world host-import linker.
    #[must_use]
    pub const fn context_linker(&self) -> &Linker<HostState> {
        &self.context_linker
    }

    /// Host-offered grant set.
    #[must_use]
    pub const fn application_grants(&self) -> &BTreeSet<String> {
        &self.application_grants
    }

    /// Concrete grant resources (preopens, HTTP allowlist, socket flag).
    #[must_use]
    pub const fn grant_resources(&self) -> &GrantResources {
        &self.resources
    }

    /// Experimental host default limits.
    #[must_use]
    pub const fn default_limits(&self) -> EffectiveLimits {
        self.default_limits
    }

    /// Clone the world linker and add only granted WASI interfaces.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError::InstantiateFailed`] when a selected WASI
    /// interface cannot be linked.
    pub fn linker_for_world(
        &self,
        world: PluginWorld,
        granted: &BTreeSet<String>,
    ) -> Result<Linker<HostState>, PluginHostError> {
        let mut linker = match world {
            PluginWorld::Toolset => self.toolset_linker.clone(),
            PluginWorld::Context => self.context_linker.clone(),
        };
        crate::instantiate::link_granted_wasi(&mut linker, granted, &self.resources)?;
        Ok(linker)
    }

    /// Validate `manifest`, compile or deserialize `bytes`, and retain the
    /// `Component` without instantiating it.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the manifest is invalid or compilation
    /// / deserialize fails.
    pub fn load(
        &self,
        bytes: &[u8],
        manifest: PluginManifest,
        world: PluginWorld,
    ) -> Result<ReadyWasm, PluginHostError> {
        validate_manifest(&manifest).map_err(|error| PluginHostError::from_map(&error))?;
        require_world(&manifest, world.as_str())?;
        let granted = require_offered(&manifest.permissions, &self.application_grants)?;
        verify_manifest(&manifest, self.signature_policy, &self.trust_roots)?;
        let digest = component_digest(bytes);
        let key = cache_key(&CacheKeyParts {
            digest: digest.clone(),
            engine: engine_fingerprint(),
            target: host_target(),
            abi: abi_identity(world.as_str()),
        });
        let component = if let Some(precompiled) = self.cache.get(&key) {
            deserialize_component(&self.engine, &precompiled)?
        } else {
            let precompiled = self
                .engine
                .precompile_component(bytes)
                .map_err(|error| PluginHostError::CompileFailed(format!("precompile: {error}")))?;
            self.cache.put(&key, &precompiled)?;
            deserialize_component(&self.engine, &precompiled)?
        };
        Ok(ReadyWasm {
            component,
            digest,
            world,
            manifest,
            granted,
        })
    }

    /// Load one enabled lock entry after verifying bytes and manifest pins.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError::LockDisabled`] when `entry.enabled` is
    /// false, [`PluginHostError::LockDigestMismatch`] when component bytes
    /// or the parsed manifest digest disagree with the lock, and the usual
    /// [`Self::load`] failures after those checks.
    pub fn load_locked(&self, entry: &LockedPlugin) -> Result<ReadyWasm, PluginHostError> {
        if !entry.enabled {
            return Err(PluginHostError::LockDisabled);
        }
        let bytes = read_locked_file(&entry.component_path)?;
        if component_digest(&bytes) != entry.component_digest {
            return Err(PluginHostError::LockDigestMismatch("component".into()));
        }
        let manifest_bytes = read_locked_file(&entry.manifest_path)?;
        let manifest =
            parse_manifest(&manifest_bytes).map_err(|error| PluginHostError::from_map(&error))?;
        validate_manifest(&manifest).map_err(|error| PluginHostError::from_map(&error))?;
        if manifest.digest != entry.manifest_digest {
            return Err(PluginHostError::LockDigestMismatch("manifest".into()));
        }
        if manifest.identity.as_str() != entry.identity {
            return Err(PluginHostError::LockInvalid(
                "identity does not match manifest".into(),
            ));
        }
        if manifest.version != entry.version {
            return Err(PluginHostError::LockInvalid(
                "version does not match manifest".into(),
            ));
        }
        let world = world_from_manifest(&manifest)?;
        self.load(&bytes, manifest, world)
    }

    /// Resolve one local lockfile and load every enabled entry in order.
    /// Extra `component.wasm` files beside the lock are ignored.
    ///
    /// # Errors
    ///
    /// Returns lockfile parse errors from [`resolve_lockfile`] or a
    /// [`Self::load_locked`] failure for an enabled entry.
    pub fn load_enabled(&self, lockfile: &Path) -> Result<Vec<ReadyWasm>, PluginHostError> {
        let resolved = resolve_lockfile(lockfile)?;
        let mut loaded = Vec::new();
        for entry in resolved.enabled() {
            loaded.push(self.load_locked(entry)?);
        }
        Ok(loaded)
    }

    /// Compile `bytes` through the cache using an explicit key, for A02 tests.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError::CompileFailed`] when compile or deserialize fails.
    pub fn compile_with_key(
        &self,
        bytes: &[u8],
        parts: &CacheKeyParts,
    ) -> Result<bool, PluginHostError> {
        let key = cache_key(parts);
        if self.cache.get(&key).is_some() {
            return Ok(true);
        }
        let precompiled = self
            .engine
            .precompile_component(bytes)
            .map_err(|error| PluginHostError::CompileFailed(format!("precompile: {error}")))?;
        self.cache.put(&key, &precompiled)?;
        Ok(false)
    }

    /// Whether `parts` already names a stored artifact.
    #[must_use]
    pub fn cache_hit(&self, parts: &CacheKeyParts) -> bool {
        self.cache.get(&cache_key(parts)).is_some()
    }
}

impl ReadyWasm {
    /// Component-bytes digest used as the cache identity.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Declared world.
    #[must_use]
    pub const fn world(&self) -> PluginWorld {
        self.world
    }

    /// Validated host-side manifest.
    #[must_use]
    pub const fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Borrow the compiled component.
    #[must_use]
    pub const fn component(&self) -> &Component {
        &self.component
    }

    /// Granted permission names (manifest ∩ host offers).
    #[must_use]
    pub const fn granted(&self) -> &BTreeSet<String> {
        &self.granted
    }
}

#[allow(unsafe_code)]
fn deserialize_component(engine: &Engine, bytes: &[u8]) -> Result<Component, PluginHostError> {
    // SAFETY: `bytes` are `Engine::precompile_component` output from this
    // engine, stored under a host-owned digest/engine/target/ABI key.
    unsafe { Component::deserialize(engine, bytes) }
        .map_err(|error| PluginHostError::CompileFailed(format!("deserialize: {error}")))
}

fn read_locked_file(path: &Path) -> Result<Vec<u8>, PluginHostError> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => Err(PluginHostError::LockNotFound),
        Err(error) => Err(PluginHostError::LockInvalid(format!(
            "locked path could not be read: {error}"
        ))),
    }
}

fn require_world(manifest: &PluginManifest, world: &str) -> Result<(), PluginHostError> {
    if manifest.worlds.iter().any(|declared| declared == world) {
        Ok(())
    } else {
        Err(PluginHostError::ConfigInvalid(
            "kind does not match a declared capability world",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{InstancePolicy, PluginHost, PluginHostConfig};
    use crate::error::PluginHostError;

    #[test]
    fn zero_concurrency_is_rejected() {
        let error =
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 0).expect_err("zero");
        assert_eq!(
            error,
            PluginHostError::ConfigInvalid("max_concurrent_instances must be non-zero",)
        );
    }

    fn tiny_component() -> Vec<u8> {
        wat::parse_str(
            r#"
            (component
              (core module $m
                (func (export "add") (param i32 i32) (result i32)
                  local.get 0
                  local.get 1
                  i32.add)
              )
              (core instance $i (instantiate $m))
              (func (export "add") (param "a" u32) (param "b" u32) (result u32)
                (canon lift (core func $i "add")))
            )
            "#,
        )
        .expect("wat")
    }

    #[test]
    fn compiled_cache_invalidates_on_each_key_field() {
        use crate::cache::{
            CONFIG_FINGERPRINT, CacheKeyParts, abi_identity, component_digest, engine_fingerprint,
            engine_fingerprint_parts, host_target,
        };

        let dir = tempfile::tempdir().expect("tempdir");
        let host = PluginHost::try_new(
            PluginHostConfig::try_new(Some(dir.path().to_path_buf()), InstancePolicy::Exclusive, 2)
                .expect("cfg"),
        )
        .expect("host");
        let bytes = tiny_component();
        let base = CacheKeyParts {
            digest: component_digest(&bytes),
            engine: engine_fingerprint(),
            target: host_target(),
            abi: abi_identity("toolset-plugin"),
        };
        assert!(!host.compile_with_key(&bytes, &base).expect("miss"));
        assert!(host.compile_with_key(&bytes, &base).expect("hit"));
        let mut digest = base.clone();
        digest.digest = component_digest(b"other-bytes");
        assert!(!host.compile_with_key(&bytes, &digest).expect("digest miss"));
        assert!(!host.cache_hit(&CacheKeyParts {
            digest: component_digest(b"missing"),
            ..base.clone()
        }));
        assert!(!host.cache_hit(&CacheKeyParts {
            engine: engine_fingerprint_parts("0.0.0-test", CONFIG_FINGERPRINT),
            ..base.clone()
        }));
        assert!(!host.cache_hit(&CacheKeyParts {
            target: "linux-x86_64-cranelift-x86_64".into(),
            ..base.clone()
        }));
        assert!(!host.cache_hit(&CacheKeyParts {
            abi: abi_identity("context-plugin"),
            ..base
        }));
    }

    #[test]
    fn strict_unsigned_load_is_rejected() {
        use crate::signature::SignaturePolicy;
        use finstack_ai_wit::{manifest_digest_hex, parse_manifest};

        let host = PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2)
                .expect("cfg")
                .with_signature_policy(SignaturePolicy::Strict),
        )
        .expect("host");
        let identity = "finstack.plugin.echo.toolset";
        let worlds = vec!["toolset-plugin".to_owned()];
        let digest = manifest_digest_hex(identity, "0.0.4", &worlds).expect("digest");
        let bytes = serde_json::to_vec(&serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging"],
            "configuration_schema": {},
            "digest": digest
        }))
        .expect("json");
        let manifest = parse_manifest(&bytes).expect("parses");
        let Err(error) = host.load(b"(component)", manifest, super::PluginWorld::Toolset) else {
            panic!("unsigned package must fail Strict policy");
        };
        assert_eq!(error.code(), "plugin_signature_untrusted");
    }

    #[test]
    fn filesystem_request_without_host_grant_is_denied() {
        use finstack_ai_wit::{manifest_digest_hex, parse_manifest};

        let host = PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2).expect("cfg"),
        )
        .expect("host");
        let identity = "finstack.plugin.echo.toolset";
        let worlds = vec!["toolset-plugin".to_owned()];
        let digest = manifest_digest_hex(identity, "0.0.4", &worlds).expect("digest");
        let bytes = serde_json::to_vec(&serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging", "filesystem"],
            "configuration_schema": {},
            "digest": digest
        }))
        .expect("json");
        let manifest = parse_manifest(&bytes).expect("parses");
        let Err(error) = host.load(b"(component)", manifest, super::PluginWorld::Toolset) else {
            panic!("ungranted filesystem must fail");
        };
        assert_eq!(error.code(), "plugin_permission_denied");
    }

    #[test]
    fn try_new_does_not_use_a_process_global_engine() {
        let first = PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2).expect("cfg"),
        )
        .expect("first");
        let second = PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Serialized, 1).expect("cfg"),
        )
        .expect("second");
        assert_eq!(first.instance_policy(), InstancePolicy::Exclusive);
        assert_eq!(second.instance_policy(), InstancePolicy::Serialized);
    }
}
