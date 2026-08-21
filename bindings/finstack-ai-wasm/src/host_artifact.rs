//! Host artifact store over `Uint8Array` payloads.

#[cfg(not(target_arch = "wasm32"))]
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;

use finstack_ai::runtime::{
    ArtifactError, ArtifactGcReport, ArtifactMetadata, ArtifactOwnerId, ArtifactPersistence,
    ArtifactRead, ArtifactScope, ArtifactStore, ArtifactStoreDescriptor, ArtifactStoreLimits,
    Bytes, PortFuture, artifact_storage_key, build_artifact_ref, validate_artifact_scope,
    validate_retrieved_artifact,
};
#[cfg(not(target_arch = "wasm32"))]
use finstack_ai_kernel::Digest;
use finstack_ai_kernel::{ArtifactRef, BlobRef, Timestamp};

use crate::host::HostFailure;

#[cfg(not(target_arch = "wasm32"))]
type NativeMap = Arc<Mutex<NativeState>>;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct NativeState {
    entries: BTreeMap<Digest, StoredArtifact>,
    total_bytes: u64,
}

#[cfg(not(target_arch = "wasm32"))]
struct StoredArtifact {
    scope: ArtifactScope,
    artifact: ArtifactRef,
    content: Bytes,
    owners: BTreeSet<ArtifactOwnerId>,
    unreferenced_since: Option<Timestamp>,
}

/// In-memory / scripted artifact store. Rust owns `ArtifactRef` construction.
pub struct HostArtifactStore {
    #[cfg(not(target_arch = "wasm32"))]
    entries: NativeMap,
    #[cfg(target_arch = "wasm32")]
    adapter: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    stage_put: std::rc::Rc<std::cell::RefCell<js_sys::Function>>,
    #[cfg(target_arch = "wasm32")]
    get: std::rc::Rc<std::cell::RefCell<js_sys::Function>>,
    #[cfg(target_arch = "wasm32")]
    get_by_blob: std::rc::Rc<std::cell::RefCell<js_sys::Function>>,
    #[cfg(target_arch = "wasm32")]
    pin: std::rc::Rc<std::cell::RefCell<js_sys::Function>>,
    #[cfg(target_arch = "wasm32")]
    unpin: std::rc::Rc<std::cell::RefCell<js_sys::Function>>,
    #[cfg(target_arch = "wasm32")]
    collect_orphans: std::rc::Rc<std::cell::RefCell<js_sys::Function>>,
}

fn artifact_unavailable(failure: HostFailure) -> ArtifactError {
    ArtifactError::Unavailable {
        message: Arc::from(failure.message()),
    }
}

impl HostArtifactStore {
    /// Construct a native in-memory artifact store.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn memory() -> Self {
        Self {
            entries: Arc::new(Mutex::new(NativeState::default())),
        }
    }

    /// Construct a wasm32 store around a JS `HostArtifactStore`.
    ///
    /// # Errors
    ///
    /// Returns [`HostFailure`] when a required artifact-store method is missing.
    #[cfg(target_arch = "wasm32")]
    pub fn from_js(adapter: wasm_bindgen::JsValue) -> Result<Self, HostFailure> {
        let stage_put = crate::host::extract_method(&adapter, "stagePut")?;
        let get = crate::host::extract_method(&adapter, "get")?;
        let get_by_blob = crate::host::extract_method(&adapter, "getByBlob")?;
        let pin = crate::host::extract_method(&adapter, "pin")?;
        let unpin = crate::host::extract_method(&adapter, "unpin")?;
        let collect_orphans = crate::host::extract_method(&adapter, "collectOrphans")?;
        Ok(Self {
            adapter,
            stage_put: std::rc::Rc::new(std::cell::RefCell::new(stage_put)),
            get: std::rc::Rc::new(std::cell::RefCell::new(get)),
            get_by_blob: std::rc::Rc::new(std::cell::RefCell::new(get_by_blob)),
            pin: std::rc::Rc::new(std::cell::RefCell::new(pin)),
            unpin: std::rc::Rc::new(std::cell::RefCell::new(unpin)),
            collect_orphans: std::rc::Rc::new(std::cell::RefCell::new(collect_orphans)),
        })
    }
}

impl ArtifactStore for HostArtifactStore {
    fn stage_put(
        &self,
        scope: ArtifactScope,
        content: Bytes,
        metadata: ArtifactMetadata,
    ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
        let artifact = match build_artifact_ref(
            &scope,
            &content,
            &metadata,
            &ArtifactStoreLimits::default(),
        ) {
            Ok(artifact) => artifact,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let storage_key = match artifact_storage_key(&scope, &artifact) {
            Ok(key) => key,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let entries = Arc::clone(&self.entries);
            Box::pin(async move {
                let mut entries = entries
                    .lock()
                    .map_err(|_| artifact_unavailable(HostFailure::Failed))?;
                if let Some(stored) = entries.entries.get(&storage_key) {
                    if stored.scope == scope
                        && stored.artifact == artifact
                        && stored.content == content
                    {
                        return Ok(artifact);
                    }
                    return Err(ArtifactError::Integrity {
                        message: Arc::from("artifact_identity_collision"),
                    });
                }
                let limits = ArtifactStoreLimits::default();
                if entries.entries.len() >= limits.max_artifacts {
                    return Err(ArtifactError::CapacityExceeded {
                        resource: "artifacts",
                        limit: u64::try_from(limits.max_artifacts).unwrap_or(u64::MAX),
                    });
                }
                let content_len = u64::try_from(content.len()).unwrap_or(u64::MAX);
                let total_bytes = entries.total_bytes.checked_add(content_len).ok_or(
                    ArtifactError::CapacityExceeded {
                        resource: "total_bytes",
                        limit: limits.max_total_bytes,
                    },
                )?;
                if total_bytes > limits.max_total_bytes {
                    return Err(ArtifactError::CapacityExceeded {
                        resource: "total_bytes",
                        limit: limits.max_total_bytes,
                    });
                }
                entries.entries.insert(
                    storage_key,
                    StoredArtifact {
                        scope,
                        artifact: artifact.clone(),
                        content,
                        owners: BTreeSet::new(),
                        unreferenced_since: None,
                    },
                );
                entries.total_bytes = total_bytes;
                Ok(artifact)
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = self.adapter.clone();
            let method = self.stage_put.borrow().clone();
            Box::pin(async move {
                let scope_json = serde_json::to_string(&scope)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let metadata_json = serde_json::to_string(&metadata)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let artifact_json = serde_json::to_string(&artifact)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[
                        crate::host::json_string_value(&scope_json),
                        crate::host::uint8_array_from_bytes(&content).into(),
                        crate::host::json_string_value(&metadata_json),
                        crate::host::json_string_value(&artifact_json),
                        crate::host::json_string_value(&storage_key.to_hex()),
                    ],
                    None,
                )
                .await
                .map_err(artifact_unavailable)?;
                Ok(artifact)
            })
        }
    }

    fn get(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        if let Err(error) = validate_artifact_scope(&scope, &artifact) {
            return Box::pin(async move { Err(error) });
        }
        let storage_key = match artifact_storage_key(&scope, &artifact) {
            Ok(key) => key,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let entries = Arc::clone(&self.entries);
            Box::pin(async move {
                let entries = entries
                    .lock()
                    .map_err(|_| artifact_unavailable(HostFailure::Failed))?;
                let stored = entries
                    .entries
                    .get(&storage_key)
                    .ok_or(ArtifactError::NotFound)?;
                if stored.scope != scope || stored.artifact != artifact {
                    return Err(ArtifactError::Integrity {
                        message: Arc::from("stored_reference_mismatch"),
                    });
                }
                validate_retrieved_artifact(&scope, &artifact, &stored.content)?;
                Ok(stored.content.clone())
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = self.adapter.clone();
            let method = self.get.borrow().clone();
            Box::pin(async move {
                let scope_json = serde_json::to_string(&scope)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let artifact_json = serde_json::to_string(&artifact)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let result = crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[
                        crate::host::json_string_value(&scope_json),
                        crate::host::json_string_value(&artifact_json),
                        crate::host::json_string_value(&storage_key.to_hex()),
                    ],
                    None,
                )
                .await
                .map_err(artifact_unavailable)?;
                let crate::host::HostJsResult::Value(value) = result else {
                    return Err(artifact_unavailable(HostFailure::InvalidResult));
                };
                let content = Bytes::from(
                    crate::host::bytes_from_uint8_array(&value).map_err(artifact_unavailable)?,
                );
                validate_retrieved_artifact(&scope, &artifact, &content)?;
                Ok(content)
            })
        }
    }

    fn get_by_blob(
        &self,
        scope: ArtifactScope,
        blob: BlobRef,
    ) -> PortFuture<Result<ArtifactRead, ArtifactError>> {
        if blob.digest().is_none() {
            return Box::pin(async {
                Err(ArtifactError::InvalidMetadata {
                    message: Arc::from("blob_digest_required"),
                })
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let entries = Arc::clone(&self.entries);
            Box::pin(async move {
                scope.digest()?;
                let entries = entries
                    .lock()
                    .map_err(|_| artifact_unavailable(HostFailure::Failed))?;
                let stored = entries
                    .entries
                    .values()
                    .find(|stored| stored.scope == scope && stored.artifact.blob() == &blob)
                    .ok_or(ArtifactError::NotFound)?;
                validate_retrieved_artifact(&scope, &stored.artifact, &stored.content)?;
                Ok(ArtifactRead {
                    reference: stored.artifact.clone(),
                    content: stored.content.clone(),
                })
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = self.adapter.clone();
            let method = self.get_by_blob.borrow().clone();
            Box::pin(async move {
                let scope_json = serde_json::to_string(&scope)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let blob_json = serde_json::to_string(&blob)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let result = crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[
                        crate::host::json_string_value(&scope_json),
                        crate::host::json_string_value(&blob_json),
                    ],
                    None,
                )
                .await
                .map_err(artifact_unavailable)?;
                let crate::host::HostJsResult::Value(value) = result else {
                    return Err(artifact_unavailable(HostFailure::InvalidResult));
                };
                if !js_sys::Array::is_array(&value) {
                    return Err(artifact_unavailable(HostFailure::InvalidResult));
                }
                let pair = js_sys::Array::from(&value);
                if pair.length() != 2 {
                    return Err(artifact_unavailable(HostFailure::InvalidResult));
                }
                let artifact_json = pair
                    .get(0)
                    .as_string()
                    .ok_or_else(|| artifact_unavailable(HostFailure::InvalidResult))?;
                let reference: ArtifactRef = serde_json::from_str(&artifact_json)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                if reference.blob() != &blob {
                    return Err(ArtifactError::Integrity {
                        message: Arc::from("stored_reference_mismatch"),
                    });
                }
                let content = Bytes::from(
                    crate::host::bytes_from_uint8_array(&pair.get(1))
                        .map_err(artifact_unavailable)?,
                );
                validate_retrieved_artifact(&scope, &reference, &content)?;
                Ok(ArtifactRead { reference, content })
            })
        }
    }

    fn descriptor(&self) -> ArtifactStoreDescriptor {
        ArtifactStoreDescriptor {
            store_id: Arc::from("wasm.host-artifacts"),
            persistence: ArtifactPersistence::Ephemeral,
            limits: ArtifactStoreLimits::default(),
        }
    }

    fn pin(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
        owner: ArtifactOwnerId,
    ) -> PortFuture<Result<(), ArtifactError>> {
        let storage_key = match artifact_storage_key(&scope, &artifact) {
            Ok(key) => key,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let entries = Arc::clone(&self.entries);
            Box::pin(async move {
                let mut state = entries
                    .lock()
                    .map_err(|_| artifact_unavailable(HostFailure::Failed))?;
                let stored = state
                    .entries
                    .get_mut(&storage_key)
                    .ok_or(ArtifactError::NotFound)?;
                if stored.scope != scope || stored.artifact != artifact {
                    return Err(ArtifactError::Integrity {
                        message: Arc::from("stored_reference_mismatch"),
                    });
                }
                let limits = ArtifactStoreLimits::default();
                if !stored.owners.contains(&owner)
                    && stored.owners.len() >= limits.max_owners_per_artifact
                {
                    return Err(ArtifactError::CapacityExceeded {
                        resource: "owners",
                        limit: u64::try_from(limits.max_owners_per_artifact).unwrap_or(u64::MAX),
                    });
                }
                stored.owners.insert(owner);
                stored.unreferenced_since = None;
                Ok(())
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = self.adapter.clone();
            let method = self.pin.borrow().clone();
            Box::pin(async move {
                let scope_json = serde_json::to_string(&scope)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let artifact_json = serde_json::to_string(&artifact)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[
                        crate::host::json_string_value(&scope_json),
                        crate::host::json_string_value(&artifact_json),
                        crate::host::json_string_value(&storage_key.to_hex()),
                        crate::host::json_string_value(owner.as_str()),
                    ],
                    None,
                )
                .await
                .map_err(artifact_unavailable)?;
                Ok(())
            })
        }
    }

    fn unpin(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
        owner: ArtifactOwnerId,
        now: Timestamp,
    ) -> PortFuture<Result<(), ArtifactError>> {
        let storage_key = match artifact_storage_key(&scope, &artifact) {
            Ok(key) => key,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let entries = Arc::clone(&self.entries);
            Box::pin(async move {
                let mut state = entries
                    .lock()
                    .map_err(|_| artifact_unavailable(HostFailure::Failed))?;
                let stored = state
                    .entries
                    .get_mut(&storage_key)
                    .ok_or(ArtifactError::NotFound)?;
                if stored.scope != scope || stored.artifact != artifact {
                    return Err(ArtifactError::Integrity {
                        message: Arc::from("stored_reference_mismatch"),
                    });
                }
                if stored.owners.remove(&owner) && stored.owners.is_empty() {
                    stored.unreferenced_since = Some(now);
                }
                Ok(())
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = self.adapter.clone();
            let method = self.unpin.borrow().clone();
            Box::pin(async move {
                let scope_json = serde_json::to_string(&scope)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let artifact_json = serde_json::to_string(&artifact)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[
                        crate::host::json_string_value(&scope_json),
                        crate::host::json_string_value(&artifact_json),
                        crate::host::json_string_value(&storage_key.to_hex()),
                        crate::host::json_string_value(owner.as_str()),
                        wasm_bindgen::JsValue::from_f64(now.as_unix_ms() as f64),
                    ],
                    None,
                )
                .await
                .map_err(artifact_unavailable)?;
                Ok(())
            })
        }
    }

    fn collect_orphans(
        &self,
        scope: ArtifactScope,
        now: Timestamp,
        limit: usize,
    ) -> PortFuture<Result<ArtifactGcReport, ArtifactError>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let entries = Arc::clone(&self.entries);
            Box::pin(async move {
                scope.digest()?;
                let limits = ArtifactStoreLimits::default();
                let mut state = entries
                    .lock()
                    .map_err(|_| artifact_unavailable(HostFailure::Failed))?;
                let mut examined = 0_usize;
                let mut delete = Vec::new();
                for (key, stored) in &mut state.entries {
                    if examined >= limit.min(limits.max_gc_batch) || stored.scope != scope {
                        continue;
                    }
                    examined += 1;
                    if !stored.owners.is_empty() {
                        continue;
                    }
                    let Some(since) = stored.unreferenced_since else {
                        stored.unreferenced_since = Some(now);
                        continue;
                    };
                    let elapsed = now.as_unix_ms().checked_sub(since.as_unix_ms());
                    if elapsed.and_then(|value| u64::try_from(value).ok())
                        >= Some(limits.orphan_grace_ms)
                    {
                        delete.push(*key);
                    }
                }
                let mut report = ArtifactGcReport {
                    examined,
                    ..ArtifactGcReport::default()
                };
                for key in delete {
                    if let Some(stored) = state.entries.remove(&key) {
                        let bytes = u64::try_from(stored.content.len()).unwrap_or(u64::MAX);
                        state.total_bytes = state.total_bytes.saturating_sub(bytes);
                        report.deleted += 1;
                        report.bytes_deleted = report.bytes_deleted.saturating_add(bytes);
                    }
                }
                Ok(report)
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = self.adapter.clone();
            let method = self.collect_orphans.borrow().clone();
            Box::pin(async move {
                let scope_json = serde_json::to_string(&scope)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let result = crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[
                        crate::host::json_string_value(&scope_json),
                        wasm_bindgen::JsValue::from_f64(now.as_unix_ms() as f64),
                        wasm_bindgen::JsValue::from_f64(limit as f64),
                    ],
                    None,
                )
                .await
                .map_err(artifact_unavailable)?;
                let crate::host::HostJsResult::Value(value) = result else {
                    return Err(artifact_unavailable(HostFailure::InvalidResult));
                };
                let encoded = crate::host::stringify_js(&value)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                serde_json::from_str(&encoded)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))
            })
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::HostArtifactStore;
    use crate::executor::block_on_ready;
    use finstack_ai::runtime::{ArtifactMetadata, ArtifactScope, ArtifactStore, Bytes};
    use finstack_ai_kernel::{Metadata, Sensitivity, SessionId};

    #[test]
    fn native_artifact_store_round_trips_bytes() {
        let store = HostArtifactStore::memory();
        let scope = ArtifactScope {
            tenant_scope: std::sync::Arc::from("tenant-a"),
            session_id: SessionId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("session"),
            run_id: None,
            sensitivity: Sensitivity::Public,
        };
        let metadata = ArtifactMetadata {
            kind: std::sync::Arc::from("scripted"),
            media_type: std::sync::Arc::from("application/octet-stream"),
            name: None,
            attributes: Metadata::empty(),
        };
        let artifact =
            block_on_ready(store.stage_put(scope.clone(), Bytes::from_static(b"abc"), metadata))
                .expect("stage");
        let got = block_on_ready(store.get(scope.clone(), artifact.clone())).expect("get");
        assert_eq!(&got[..], b"abc");
        let by_blob =
            block_on_ready(store.get_by_blob(scope, artifact.blob().clone())).expect("get by blob");
        assert_eq!(by_blob.reference, artifact);
        assert_eq!(&by_blob.content[..], b"abc");
    }
}
