//! Isolated Wasmtime engine, host configuration, and compiled-component load.

use std::path::PathBuf;

use finstack_ai_wit::{PluginManifest, validate_manifest};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine};

use crate::bindings::context::ContextPlugin;
use crate::bindings::toolset::ToolsetPlugin;
use crate::cache::{
    CacheKeyParts, ComponentCache, abi_identity, cache_key, component_digest, engine_fingerprint,
    host_target,
};
use crate::error::PluginHostError;
use crate::instantiate::HostState;

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
}

impl PluginHostConfig {
    /// Build a validated host config.
    ///
    /// `cache_dir` of `None` keeps compiled artifacts in memory. Directory
    /// mode is host-owned and does not use Wasmtime's implicit global cache.
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
}

/// Compiled component retained without a live instance.
#[derive(Clone)]
pub struct ReadyWasm {
    pub(crate) component: Component,
    pub(crate) digest: String,
    pub(crate) world: PluginWorld,
    pub(crate) manifest: PluginManifest,
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
        })
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
}

#[allow(unsafe_code)]
fn deserialize_component(engine: &Engine, bytes: &[u8]) -> Result<Component, PluginHostError> {
    // SAFETY: `bytes` are `Engine::precompile_component` output from this
    // engine, stored under a host-owned digest/engine/target/ABI key.
    unsafe { Component::deserialize(engine, bytes) }
        .map_err(|error| PluginHostError::CompileFailed(format!("deserialize: {error}")))
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
