//! Host artifact store over `Uint8Array` payloads.

#[cfg(not(target_arch = "wasm32"))]
use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;

use finstack_ai::runtime::{
    ArtifactError, ArtifactId, ArtifactMetadata, ArtifactRef, ArtifactScope, ArtifactStore,
    BlobRef, Bytes, Digest, PortFuture,
};

use crate::host::HostFailure;

#[cfg(not(target_arch = "wasm32"))]
type NativeMap = Arc<Mutex<BTreeMap<String, (ArtifactRef, Bytes)>>>;

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
}

fn artifact_unavailable(failure: HostFailure) -> ArtifactError {
    ArtifactError::Unavailable {
        message: Arc::from(failure.message()),
    }
}

fn build_artifact(
    scope: &ArtifactScope,
    content: &[u8],
    metadata: &ArtifactMetadata,
) -> Result<ArtifactRef, ArtifactError> {
    let digest = Digest::blob_content(content);
    let blob = BlobRef::try_new(
        digest.to_hex(),
        metadata.media_type.as_ref(),
        u64::try_from(content.len()).map_err(|_| ArtifactError::InvalidMetadata {
            message: Arc::from("invalid_length"),
        })?,
        Some(digest),
        metadata.name.as_deref(),
    )
    .map_err(|_| ArtifactError::InvalidMetadata {
        message: Arc::from("invalid_blob"),
    })?;
    let mut artifact_id = [0_u8; 16];
    artifact_id.copy_from_slice(&digest.as_bytes()[..16]);
    ArtifactRef::try_new(
        ArtifactId::from_bytes(artifact_id),
        metadata.kind.as_ref(),
        blob,
        digest,
        scope.digest()?,
        metadata.attributes.clone(),
    )
    .map_err(|_| ArtifactError::InvalidMetadata {
        message: Arc::from("invalid_artifact"),
    })
}

impl HostArtifactStore {
    /// Construct a native in-memory artifact store.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn memory() -> Self {
        Self {
            entries: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Construct a wasm32 store around a JS `HostArtifactStore`.
    ///
    /// # Errors
    ///
    /// Returns [`HostFailure`] when `stagePut` or `get` is missing.
    #[cfg(target_arch = "wasm32")]
    pub fn from_js(adapter: wasm_bindgen::JsValue) -> Result<Self, HostFailure> {
        let stage_put = crate::host::extract_method(&adapter, "stagePut")?;
        let get = crate::host::extract_method(&adapter, "get")?;
        Ok(Self {
            adapter,
            stage_put: std::rc::Rc::new(std::cell::RefCell::new(stage_put)),
            get: std::rc::Rc::new(std::cell::RefCell::new(get)),
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
        let artifact = match build_artifact(&scope, &content, &metadata) {
            Ok(artifact) => artifact,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let entries = Arc::clone(&self.entries);
            let key = artifact.id().to_canonical_string();
            Box::pin(async move {
                entries
                    .lock()
                    .map_err(|_| artifact_unavailable(HostFailure::Failed))?
                    .insert(key, (artifact.clone(), content));
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
                crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[
                        crate::host::json_string_value(&scope_json),
                        crate::host::uint8_array_from_bytes(&content).into(),
                        crate::host::json_string_value(&metadata_json),
                        crate::host::json_string_value(&artifact.id().to_canonical_string()),
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
        _scope: ArtifactScope,
        artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let entries = Arc::clone(&self.entries);
            let key = artifact.id().to_canonical_string();
            Box::pin(async move {
                let entries = entries
                    .lock()
                    .map_err(|_| artifact_unavailable(HostFailure::Failed))?;
                entries
                    .get(&key)
                    .map(|(_, bytes)| bytes.clone())
                    .ok_or(ArtifactError::NotFound)
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = self.adapter.clone();
            let method = self.get.borrow().clone();
            Box::pin(async move {
                let key = artifact.id().to_canonical_string();
                let result = crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[crate::host::json_string_value(&key)],
                    None,
                )
                .await
                .map_err(artifact_unavailable)?;
                let crate::host::HostJsResult::Value(value) = result else {
                    return Err(artifact_unavailable(HostFailure::InvalidResult));
                };
                Ok(Bytes::from(
                    crate::host::bytes_from_uint8_array(&value).map_err(artifact_unavailable)?,
                ))
            })
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::HostArtifactStore;
    use crate::executor::block_on_ready;
    use finstack_ai::runtime::{
        ArtifactMetadata, ArtifactScope, ArtifactStore, Bytes, Metadata, Sensitivity, SessionId,
    };

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
        let got = block_on_ready(store.get(scope, artifact)).expect("get");
        assert_eq!(&got[..], b"abc");
    }
}
