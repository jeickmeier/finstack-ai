//! Host-owned compiled-component cache keyed by digest, engine, target, and ABI.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use crate::error::PluginHostError;

/// Canonical fields hashed into a cache key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKeyParts {
    /// SHA-256 of the component bytes.
    pub digest: String,
    /// `wasmtime` crate version plus the host config fingerprint.
    pub engine: String,
    /// Host `os` + `arch` (and cranelift ISA when exposed).
    pub target: String,
    /// World name plus the `@0.0.4` or `@1.0.0` package set.
    pub abi: String,
}

/// In-memory or directory cache of `Engine::precompile_component` artifacts.
pub struct ComponentCache {
    inner: CacheInner,
}

enum CacheInner {
    Memory(Mutex<BTreeMap<String, Vec<u8>>>),
    Directory(PathBuf),
}

impl ComponentCache {
    /// Store artifacts in process memory only.
    #[must_use]
    pub fn memory() -> Self {
        Self {
            inner: CacheInner::Memory(Mutex::new(BTreeMap::new())),
        }
    }

    /// Store artifacts under `dir`. The directory is created if needed.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError::CompileFailed`] when the directory cannot be created.
    pub fn directory(dir: impl Into<PathBuf>) -> Result<Self, PluginHostError> {
        let dir = dir.into();
        fs::create_dir_all(&dir)
            .map_err(|error| PluginHostError::CompileFailed(format!("cache directory: {error}")))?;
        Ok(Self {
            inner: CacheInner::Directory(dir),
        })
    }

    /// Return a previously stored artifact for `key`.
    ///
    /// Directory hits are existence/identity only. They are not safe to pass
    /// to `Component::deserialize`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        match &self.inner {
            CacheInner::Memory(map) => map.lock().ok()?.get(key).cloned(),
            CacheInner::Directory(dir) => fs::read(artifact_path(dir, key)).ok(),
        }
    }

    /// Return precompiled bytes that this process wrote into the in-memory
    /// backend. Directory artifacts are never returned: a writable cache
    /// directory is not an authentication boundary (TM-08).
    #[must_use]
    pub fn trusted_precompiled(&self, key: &str) -> Option<Vec<u8>> {
        match &self.inner {
            CacheInner::Memory(map) => map.lock().ok()?.get(key).cloned(),
            CacheInner::Directory(_) => None,
        }
    }

    /// Store `bytes` under `key`, replacing any previous artifact.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError::CompileFailed`] when the artifact cannot be written.
    pub fn put(&self, key: &str, bytes: &[u8]) -> Result<(), PluginHostError> {
        match &self.inner {
            CacheInner::Memory(map) => {
                let mut map = map.lock().map_err(|_| {
                    PluginHostError::CompileFailed("cache mutex poisoned".to_owned())
                })?;
                map.insert(key.to_owned(), bytes.to_vec());
                Ok(())
            }
            CacheInner::Directory(dir) => {
                let path = artifact_path(dir, key);
                let tmp = path.with_extension("tmp");
                fs::write(&tmp, bytes).map_err(|error| {
                    PluginHostError::CompileFailed(format!("cache write: {error}"))
                })?;
                fs::rename(&tmp, &path).map_err(|error| {
                    PluginHostError::CompileFailed(format!("cache rename: {error}"))
                })?;
                Ok(())
            }
        }
    }
}

fn artifact_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.cwasm"))
}

/// SHA-256 hex of the component bytes. This is the cache identity, not the
/// plugin-manifest contract manifest digest.
#[must_use]
pub fn component_digest(bytes: &[u8]) -> String {
    hex_sha256(bytes)
}

/// Workspace-pinned `wasmtime` crate version recorded in the engine field.
pub const WASMTIME_CRATE_VERSION: &str = "47.0.3";

/// `wasmtime` version plus the compile-config fingerprint used by this host.
#[must_use]
pub fn engine_fingerprint() -> String {
    engine_fingerprint_parts(WASMTIME_CRATE_VERSION, CONFIG_FINGERPRINT)
}

/// Build an engine fingerprint from explicit parts so tests can invalidate independently.
#[must_use]
pub fn engine_fingerprint_parts(version: &str, config: &str) -> String {
    format!("{version}:{config}")
}

/// Host target triple used in the cache key.
#[must_use]
pub fn host_target() -> String {
    format!(
        "{}-{}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        cranelift_isa()
    )
}

/// ABI identity: world plus the package set for that world and major.
#[must_use]
pub fn abi_identity(world: &str, version: &str) -> String {
    match (world, version) {
        ("context-plugin", "1.0.0") => {
            "context-plugin+finstack:ai-types@1.0.0+finstack:ai-host@1.0.0+finstack:ai-context@1.0.0"
                .to_owned()
        }
        ("context-plugin", _) => {
            "context-plugin+finstack:ai-types@0.0.4+finstack:ai-host@0.0.4+finstack:ai-context@0.0.4"
                .to_owned()
        }
        (_, "1.0.0") => {
            "toolset-plugin+finstack:ai-types@1.0.0+finstack:ai-host@1.0.0+finstack:ai-toolset@1.0.0"
                .to_owned()
        }
        _ => {
            "toolset-plugin+finstack:ai-types@0.0.4+finstack:ai-host@0.0.4+finstack:ai-toolset@0.0.4"
                .to_owned()
        }
    }
}

/// Canonical JSON then SHA-256 of the four cache-key fields.
#[must_use]
pub fn cache_key(parts: &CacheKeyParts) -> String {
    let value = serde_json::json!({
        "abi": parts.abi,
        "digest": parts.digest,
        "engine": parts.engine,
        "target": parts.target,
    });
    let canonical = serde_json_canonicalizer::to_vec(&value).unwrap_or_else(|_| {
        serde_json::to_vec(&value).unwrap_or_else(|_| parts.digest.as_bytes().to_vec())
    });
    hex_sha256(&canonical)
}

/// Config fingerprint recorded in the engine field. Fuel and the store limiter
/// are resource-limit claims; epoch interruption remains the cancel channel.
pub const CONFIG_FINGERPRINT: &str =
    "async=true,epoch=true,fuel=true,limiter=true,compiler=cranelift";

fn cranelift_isa() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "cranelift-aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "cranelift-x86_64"
    } else {
        "cranelift-host"
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::{
        CONFIG_FINGERPRINT, CacheKeyParts, ComponentCache, abi_identity, cache_key,
        component_digest, engine_fingerprint, engine_fingerprint_parts, host_target,
    };

    fn parts() -> CacheKeyParts {
        CacheKeyParts {
            digest: component_digest(b"component-a"),
            engine: engine_fingerprint(),
            target: host_target(),
            abi: abi_identity("toolset-plugin", "0.0.4"),
        }
    }

    #[test]
    fn each_field_changes_the_key_independently() {
        let base = cache_key(&parts());
        assert_eq!(base, cache_key(&parts()));
        let mut digest = parts();
        digest.digest = component_digest(b"component-b");
        let mut engine = parts();
        engine.engine = engine_fingerprint_parts("0.0.0-test", CONFIG_FINGERPRINT);
        let mut target = parts();
        target.target = "linux-x86_64-cranelift-x86_64".to_owned();
        let mut abi = parts();
        abi.abi = abi_identity("context-plugin", "0.0.4");
        let keys = [
            cache_key(&digest),
            cache_key(&engine),
            cache_key(&target),
            cache_key(&abi),
        ];
        assert!(keys.iter().all(|key| key != &base));
        assert_eq!(
            keys.len(),
            keys.iter().collect::<std::collections::BTreeSet<_>>().len()
        );
        assert_ne!(
            abi_identity("toolset-plugin", "0.0.4"),
            abi_identity("toolset-plugin", "1.0.0")
        );
    }

    #[test]
    fn memory_cache_round_trips() {
        let cache = ComponentCache::memory();
        let key = cache_key(&parts());
        assert!(cache.get(&key).is_none());
        cache.put(&key, b"precompiled").expect("put");
        assert_eq!(cache.get(&key).as_deref(), Some(b"precompiled".as_ref()));
    }

    #[test]
    fn directory_cache_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = ComponentCache::directory(dir.path()).expect("dir");
        let key = cache_key(&parts());
        cache.put(&key, b"artifact").expect("put");
        assert_eq!(cache.get(&key).as_deref(), Some(b"artifact".as_ref()));
        let miss = cache_key(&CacheKeyParts {
            digest: component_digest(b"other"),
            ..parts()
        });
        assert!(cache.get(&miss).is_none());
        assert!(
            cache.trusted_precompiled(&key).is_none(),
            "directory artifacts are not trusted for deserialize"
        );
    }

    #[test]
    fn only_memory_artifacts_are_trusted_for_deserialize() {
        let cache = ComponentCache::memory();
        let key = cache_key(&parts());
        cache.put(&key, b"precompiled").expect("put");
        assert_eq!(
            cache.trusted_precompiled(&key).as_deref(),
            Some(b"precompiled".as_ref())
        );
    }
}
