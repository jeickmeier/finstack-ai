//! Host artifact store over `Uint8Array` payloads.

use std::sync::Arc;

use finstack_ai::runtime::Bytes;
use finstack_ai::runtime::artifact::{
    ArtifactError, ArtifactGcReport, ArtifactMetadata, ArtifactOwnerId, ArtifactPersistence,
    ArtifactRead, ArtifactScope, ArtifactStore, ArtifactStoreDescriptor, ArtifactStoreLimits,
};
#[cfg(target_arch = "wasm32")]
use finstack_ai::runtime::artifact::{
    artifact_storage_key, build_artifact_ref, validate_retrieved_artifact,
};
use finstack_ai::runtime::ports::PortFuture;
use finstack_ai_kernel::{ArtifactRef, BlobRef, Timestamp};

#[cfg(not(target_arch = "wasm32"))]
use crate::document_store::MemoryArtifactStore;
#[cfg(target_arch = "wasm32")]
use crate::host::HostFailure;

const STORE_ID: &str = "wasm.host-artifacts";

/// In-memory / scripted artifact store. Rust owns `ArtifactRef` construction.
pub struct HostArtifactStore {
    #[cfg(not(target_arch = "wasm32"))]
    inner: MemoryArtifactStore,
    #[cfg(target_arch = "wasm32")]
    adapter: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    stage_put: js_sys::Function,
    #[cfg(target_arch = "wasm32")]
    get: js_sys::Function,
    #[cfg(target_arch = "wasm32")]
    get_by_blob: js_sys::Function,
    #[cfg(target_arch = "wasm32")]
    pin: js_sys::Function,
    #[cfg(target_arch = "wasm32")]
    unpin: js_sys::Function,
    #[cfg(target_arch = "wasm32")]
    collect_orphans: js_sys::Function,
}

#[cfg(target_arch = "wasm32")]
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
            inner: MemoryArtifactStore::new(STORE_ID, ArtifactStoreLimits::default()),
        }
    }

    /// Construct a wasm32 store around a JS `HostArtifactStore`.
    ///
    /// # Errors
    ///
    /// Returns [`HostFailure`] when a required artifact-store method is missing.
    #[cfg(target_arch = "wasm32")]
    pub fn from_js(adapter: wasm_bindgen::JsValue) -> Result<Self, HostFailure> {
        let method = |name| crate::host::extract_method(&adapter, name);
        Ok(Self {
            stage_put: method("stagePut")?,
            get: method("get")?,
            get_by_blob: method("getByBlob")?,
            pin: method("pin")?,
            unpin: method("unpin")?,
            collect_orphans: method("collectOrphans")?,
            adapter,
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.inner.stage_put(scope, content, metadata)
        }
        #[cfg(target_arch = "wasm32")]
        {
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
            let adapter = self.adapter.clone();
            let method = self.stage_put.clone();
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
                        wasm_bindgen::JsValue::from_str(&scope_json),
                        crate::host::uint8_array_from_bytes(&content).into(),
                        wasm_bindgen::JsValue::from_str(&metadata_json),
                        wasm_bindgen::JsValue::from_str(&artifact_json),
                        wasm_bindgen::JsValue::from_str(&storage_key.to_hex()),
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.inner.get(scope, artifact)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let storage_key = match artifact_storage_key(&scope, &artifact) {
                Ok(key) => key,
                Err(error) => return Box::pin(async move { Err(error) }),
            };
            let adapter = self.adapter.clone();
            let method = self.get.clone();
            Box::pin(async move {
                let scope_json = serde_json::to_string(&scope)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let artifact_json = serde_json::to_string(&artifact)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let result = crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[
                        wasm_bindgen::JsValue::from_str(&scope_json),
                        wasm_bindgen::JsValue::from_str(&artifact_json),
                        wasm_bindgen::JsValue::from_str(&storage_key.to_hex()),
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.inner.get_by_blob(scope, blob)
        }
        #[cfg(target_arch = "wasm32")]
        {
            if blob.digest().is_none() {
                return Box::pin(async {
                    Err(ArtifactError::InvalidMetadata {
                        message: Arc::from("blob_digest_required"),
                    })
                });
            }
            let adapter = self.adapter.clone();
            let method = self.get_by_blob.clone();
            Box::pin(async move {
                let scope_json = serde_json::to_string(&scope)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let blob_json = serde_json::to_string(&blob)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let result = crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[
                        wasm_bindgen::JsValue::from_str(&scope_json),
                        wasm_bindgen::JsValue::from_str(&blob_json),
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
            store_id: Arc::from(STORE_ID),
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.inner.pin(scope, artifact, owner)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let storage_key = match artifact_storage_key(&scope, &artifact) {
                Ok(key) => key,
                Err(error) => return Box::pin(async move { Err(error) }),
            };
            let adapter = self.adapter.clone();
            let method = self.pin.clone();
            Box::pin(async move {
                let scope_json = serde_json::to_string(&scope)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let artifact_json = serde_json::to_string(&artifact)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[
                        wasm_bindgen::JsValue::from_str(&scope_json),
                        wasm_bindgen::JsValue::from_str(&artifact_json),
                        wasm_bindgen::JsValue::from_str(&storage_key.to_hex()),
                        wasm_bindgen::JsValue::from_str(owner.as_str()),
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.inner.unpin(scope, artifact, owner, now)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let storage_key = match artifact_storage_key(&scope, &artifact) {
                Ok(key) => key,
                Err(error) => return Box::pin(async move { Err(error) }),
            };
            let adapter = self.adapter.clone();
            let method = self.unpin.clone();
            Box::pin(async move {
                let scope_json = serde_json::to_string(&scope)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let artifact_json = serde_json::to_string(&artifact)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[
                        wasm_bindgen::JsValue::from_str(&scope_json),
                        wasm_bindgen::JsValue::from_str(&artifact_json),
                        wasm_bindgen::JsValue::from_str(&storage_key.to_hex()),
                        wasm_bindgen::JsValue::from_str(owner.as_str()),
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
            self.inner.collect_orphans(scope, now, limit)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = self.adapter.clone();
            let method = self.collect_orphans.clone();
            Box::pin(async move {
                let scope_json = serde_json::to_string(&scope)
                    .map_err(|_| artifact_unavailable(HostFailure::InvalidResult))?;
                let result = crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[
                        wasm_bindgen::JsValue::from_str(&scope_json),
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
    use finstack_ai::runtime::Bytes;
    use finstack_ai::runtime::artifact::{ArtifactMetadata, ArtifactScope, ArtifactStore};
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
