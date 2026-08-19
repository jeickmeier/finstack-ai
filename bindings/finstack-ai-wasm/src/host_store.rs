//! Host journal store. Optional JS `append` / `load` / `writeSnapshot` methods
//! are proxied as transitional JSON; missing methods stay pre-beta Unavailable.

use std::sync::Arc;

#[cfg(target_arch = "wasm32")]
use finstack_ai::runtime::OpaqueSnapshot;
use finstack_ai::runtime::{
    JournalStore, LoadRequest, LoadedSession, PortFuture, SnapshotReceipt, SnapshotRequest,
    StoreError, StoreHealth,
};
use finstack_ai_kernel::{AppendRequest, CommittedBatch};
#[cfg(target_arch = "wasm32")]
use finstack_ai_kernel::{Digest, Metadata, SessionId};
use serde::Deserialize;
#[cfg(target_arch = "wasm32")]
use serde::Serialize;

#[cfg(not(target_arch = "wasm32"))]
use crate::host::HostFailure;

#[cfg(not(target_arch = "wasm32"))]
use crate::host::NativeHostResult;

/// Constructor options for a JS journal-store wrapper.
#[derive(Debug, Clone, Deserialize)]
pub struct HostJournalStoreOptions {
    /// Stable non-secret operating-mode description.
    #[serde(default = "default_detail")]
    pub detail: String,
}

fn default_detail() -> String {
    "js_memory_prebeta".into()
}

#[cfg(target_arch = "wasm32")]
type JsMethod = std::rc::Rc<std::cell::RefCell<js_sys::Function>>;

/// Scripted host journal store. Health is never durable.
pub struct HostJournalStore {
    detail: Arc<str>,
    #[cfg(not(target_arch = "wasm32"))]
    health: Arc<dyn Fn() -> Result<NativeHostResult, HostFailure> + Send + Sync>,
    #[cfg(target_arch = "wasm32")]
    adapter: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    health: JsMethod,
    #[cfg(target_arch = "wasm32")]
    append: Option<JsMethod>,
    #[cfg(target_arch = "wasm32")]
    load: Option<JsMethod>,
    #[cfg(target_arch = "wasm32")]
    write_snapshot: Option<JsMethod>,
}

impl HostJournalStore {
    fn from_parts(
        options: HostJournalStoreOptions,
        #[cfg(not(target_arch = "wasm32"))] health: Arc<
            dyn Fn() -> Result<NativeHostResult, HostFailure> + Send + Sync,
        >,
        #[cfg(target_arch = "wasm32")] adapter: wasm_bindgen::JsValue,
        #[cfg(target_arch = "wasm32")] health: js_sys::Function,
        #[cfg(target_arch = "wasm32")] append: Option<js_sys::Function>,
        #[cfg(target_arch = "wasm32")] load: Option<js_sys::Function>,
        #[cfg(target_arch = "wasm32")] write_snapshot: Option<js_sys::Function>,
    ) -> Self {
        Self {
            detail: Arc::from(options.detail),
            #[cfg(not(target_arch = "wasm32"))]
            health,
            #[cfg(target_arch = "wasm32")]
            adapter,
            #[cfg(target_arch = "wasm32")]
            health: std::rc::Rc::new(std::cell::RefCell::new(health)),
            #[cfg(target_arch = "wasm32")]
            append: append.map(|method| std::rc::Rc::new(std::cell::RefCell::new(method))),
            #[cfg(target_arch = "wasm32")]
            load: load.map(|method| std::rc::Rc::new(std::cell::RefCell::new(method))),
            #[cfg(target_arch = "wasm32")]
            write_snapshot: write_snapshot
                .map(|method| std::rc::Rc::new(std::cell::RefCell::new(method))),
        }
    }

    /// Construct a native scripted store.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_callback(
        options: HostJournalStoreOptions,
        health: impl Fn() -> Result<NativeHostResult, HostFailure> + Send + Sync + 'static,
    ) -> Self {
        Self::from_parts(options, Arc::new(health))
    }

    /// Construct a wasm32 store around a JS `HostJournalStore`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the host object is missing `health`.
    #[cfg(target_arch = "wasm32")]
    pub fn from_js(
        adapter: wasm_bindgen::JsValue,
        options: HostJournalStoreOptions,
    ) -> Result<Self, StoreError> {
        let health = crate::host::extract_method(&adapter, "health").map_err(|_| {
            StoreError::Unavailable {
                reason_code: crate::host::JS_HOST_FAILED,
            }
        })?;
        let append = crate::host::extract_optional_method(&adapter, "append");
        let load = crate::host::extract_optional_method(&adapter, "load");
        let write_snapshot = crate::host::extract_optional_method(&adapter, "writeSnapshot");
        Ok(Self::from_parts(
            options,
            adapter,
            health,
            append,
            load,
            write_snapshot,
        ))
    }
}

impl JournalStore for HostJournalStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = request;
            Box::pin(async {
                Err(StoreError::Unavailable {
                    reason_code: "js_host_store_prebeta",
                })
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let Some(method) = self.append.as_ref().map(|method| method.borrow().clone()) else {
                return Box::pin(async {
                    Err(StoreError::Unavailable {
                        reason_code: "js_host_store_prebeta",
                    })
                });
            };
            let adapter = self.adapter.clone();
            Box::pin(async move {
                let encoded =
                    serde_json::to_string(&request).map_err(|_| StoreError::Integrity {
                        reason_code: "transitional_payload_encode_failed",
                    })?;
                let body = invoke_store_json(&adapter, &method, &encoded).await?;
                serde_json::from_str(&body).map_err(|_| StoreError::Unavailable {
                    reason_code: crate::host::JS_HOST_RESULT_INVALID,
                })
            })
        }
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Box::pin(async move { Ok(LoadedSession::empty(request.session_id)) })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let Some(method) = self.load.as_ref().map(|method| method.borrow().clone()) else {
                return Box::pin(async move { Ok(LoadedSession::empty(request.session_id)) });
            };
            let adapter = self.adapter.clone();
            Box::pin(async move {
                let encoded = serde_json::to_string(&LoadRequestWire {
                    session_id: request.session_id,
                })
                .map_err(|_| StoreError::Integrity {
                    reason_code: "transitional_payload_encode_failed",
                })?;
                let body = invoke_store_json(&adapter, &method, &encoded).await?;
                let parsed: LoadedSessionWire =
                    serde_json::from_str(&body).map_err(|_| StoreError::Integrity {
                        reason_code: "journal_record_corrupt",
                    })?;
                parsed.into_loaded()
            })
        }
    }

    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = request;
            Box::pin(async {
                Err(StoreError::Unavailable {
                    reason_code: "js_host_store_prebeta",
                })
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let Some(method) = self
                .write_snapshot
                .as_ref()
                .map(|method| method.borrow().clone())
            else {
                return Box::pin(async {
                    Err(StoreError::Unavailable {
                        reason_code: "js_host_store_prebeta",
                    })
                });
            };
            let adapter = self.adapter.clone();
            Box::pin(async move {
                let encoded =
                    serde_json::to_string(&SnapshotRequestWire::from(&request)).map_err(|_| {
                        StoreError::Integrity {
                            reason_code: "transitional_payload_encode_failed",
                        }
                    })?;
                let body = invoke_store_json(&adapter, &method, &encoded).await?;
                let parsed: SnapshotReceiptWire =
                    serde_json::from_str(&body).map_err(|_| StoreError::Unavailable {
                        reason_code: crate::host::JS_HOST_RESULT_INVALID,
                    })?;
                Ok(SnapshotReceipt {
                    session_id: parsed.session_id,
                    sequence: parsed.sequence,
                    digest: parsed.digest,
                    bytes: parsed.bytes,
                })
            })
        }
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        let detail = Arc::clone(&self.detail);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let health = Arc::clone(&self.health);
            Box::pin(async move {
                match health() {
                    Ok(NativeHostResult::Object(body)) => {
                        let parsed: HostStoreHealth =
                            serde_json::from_str(&body).map_err(|_| StoreError::Unavailable {
                                reason_code: crate::host::JS_HOST_RESULT_INVALID,
                            })?;
                        Ok(StoreHealth {
                            ready: parsed.ready,
                            durable: false,
                            detail: parsed.detail.unwrap_or(detail),
                        })
                    }
                    _ => Ok(StoreHealth {
                        ready: true,
                        durable: false,
                        detail,
                    }),
                }
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = self.adapter.clone();
            let method = self.health.borrow().clone();
            Box::pin(async move {
                let result = crate::host::invoke_host(&adapter, &method, &[], None)
                    .await
                    .map_err(|_| StoreError::Unavailable {
                        reason_code: crate::host::JS_HOST_FAILED,
                    })?;
                let crate::host::HostJsResult::Value(value) = result else {
                    return Err(StoreError::Unavailable {
                        reason_code: crate::host::JS_HOST_RESULT_INVALID,
                    });
                };
                let body =
                    crate::host::stringify_js(&value).map_err(|_| StoreError::Unavailable {
                        reason_code: crate::host::JS_HOST_RESULT_INVALID,
                    })?;
                let parsed: HostStoreHealth =
                    serde_json::from_str(&body).map_err(|_| StoreError::Unavailable {
                        reason_code: crate::host::JS_HOST_RESULT_INVALID,
                    })?;
                Ok(StoreHealth {
                    ready: parsed.ready,
                    durable: false,
                    detail: parsed.detail.unwrap_or(detail),
                })
            })
        }
    }
}

#[derive(Debug, Deserialize)]
struct HostStoreHealth {
    #[serde(default = "default_ready")]
    ready: bool,
    #[serde(default)]
    detail: Option<Arc<str>>,
}

const fn default_ready() -> bool {
    true
}

#[cfg(target_arch = "wasm32")]
#[derive(Serialize)]
struct LoadRequestWire {
    session_id: SessionId,
}

#[cfg(target_arch = "wasm32")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadedSessionWire {
    session_id: SessionId,
    head_sequence: u64,
    #[serde(default)]
    head_checksum: Option<Digest>,
    #[serde(default)]
    metadata: Metadata,
    committed_batches: Vec<CommittedBatch>,
    #[serde(default)]
    snapshot: Option<OpaqueSnapshotWire>,
}

#[cfg(target_arch = "wasm32")]
impl LoadedSessionWire {
    fn into_loaded(self) -> Result<LoadedSession, StoreError> {
        let snapshot = self
            .snapshot
            .map(OpaqueSnapshotWire::into_snapshot)
            .transpose()?;
        Ok(LoadedSession {
            session_id: self.session_id,
            head_sequence: self.head_sequence,
            head_checksum: self.head_checksum,
            metadata: self.metadata,
            committed_batches: self.committed_batches.into(),
            snapshot,
            accelerated: None,
        })
    }
}

#[cfg(target_arch = "wasm32")]
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpaqueSnapshotWire {
    sequence: u64,
    digest: Digest,
    bytes: Vec<u8>,
}

#[cfg(target_arch = "wasm32")]
impl OpaqueSnapshotWire {
    fn into_snapshot(self) -> Result<OpaqueSnapshot, StoreError> {
        OpaqueSnapshot::try_new(self.sequence, self.digest, self.bytes, usize::MAX)
    }
}

#[cfg(target_arch = "wasm32")]
#[derive(Serialize)]
struct SnapshotRequestWire {
    session_id: SessionId,
    snapshot: OpaqueSnapshotWire,
}

#[cfg(target_arch = "wasm32")]
impl From<&SnapshotRequest> for SnapshotRequestWire {
    fn from(request: &SnapshotRequest) -> Self {
        Self {
            session_id: request.session_id,
            snapshot: OpaqueSnapshotWire {
                sequence: request.snapshot.sequence(),
                digest: request.snapshot.digest(),
                bytes: request.snapshot.bytes().to_vec(),
            },
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotReceiptWire {
    session_id: SessionId,
    sequence: u64,
    digest: Digest,
    bytes: usize,
}

#[cfg(target_arch = "wasm32")]
async fn invoke_store_json(
    adapter: &wasm_bindgen::JsValue,
    method: &js_sys::Function,
    encoded: &str,
) -> Result<String, StoreError> {
    let value = crate::host::invoke_host_raw(
        adapter,
        method,
        &[crate::host::json_string_value(encoded)],
        None,
    )
    .await
    .map_err(store_error_from_js)?;
    crate::host::stringify_js(&value).map_err(|_| StoreError::Unavailable {
        reason_code: crate::host::JS_HOST_RESULT_INVALID,
    })
}

#[cfg(target_arch = "wasm32")]
fn store_error_from_js(error: wasm_bindgen::JsValue) -> StoreError {
    let code = js_string_field(&error, "code").unwrap_or_default();
    let reason = intern_reason(
        js_string_field(&error, "reasonCode")
            .or_else(|| js_string_field(&error, "reason_code"))
            .as_deref()
            .unwrap_or(""),
    );
    match code.as_str() {
        "store_conflict" => StoreError::Conflict {
            expected_sequence: js_u64_field(&error, "expectedSequence")
                .or_else(|| js_u64_field(&error, "expected_sequence"))
                .unwrap_or(0),
            actual_next_sequence: js_u64_field(&error, "actualNextSequence")
                .or_else(|| js_u64_field(&error, "actual_next_sequence"))
                .unwrap_or(0),
        },
        "store_corruption" => StoreError::Corruption {
            reason_code: reason,
        },
        "store_limit_exceeded" => StoreError::LimitExceeded {
            resource: intern_resource(
                js_string_field(&error, "resource")
                    .as_deref()
                    .unwrap_or("unknown"),
            ),
            limit: usize::try_from(js_u64_field(&error, "limit").unwrap_or(0)).unwrap_or(0),
        },
        "invalid_store_request" => StoreError::InvalidRequest {
            reason_code: reason,
        },
        "store_integrity_failure" => StoreError::Integrity {
            reason_code: reason,
        },
        "store_unavailable" => StoreError::Unavailable {
            reason_code: reason,
        },
        "ambiguous_acknowledgement" => StoreError::AmbiguousAcknowledgement,
        _ => StoreError::Unavailable {
            reason_code: crate::host::JS_HOST_FAILED,
        },
    }
}

#[cfg(target_arch = "wasm32")]
fn js_string_field(value: &wasm_bindgen::JsValue, name: &str) -> Option<String> {
    js_sys::Reflect::get(value, &wasm_bindgen::JsValue::from_str(name))
        .ok()
        .and_then(|field| field.as_string())
}

#[cfg(target_arch = "wasm32")]
fn js_u64_field(value: &wasm_bindgen::JsValue, name: &str) -> Option<u64> {
    js_sys::Reflect::get(value, &wasm_bindgen::JsValue::from_str(name))
        .ok()
        .and_then(|field| field.as_f64())
        .filter(|number| number.is_finite() && *number >= 0.0)
        .map(|number| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "finite non-negative JS numbers are store sequence counters"
            )]
            {
                number as u64
            }
        })
}

#[cfg(target_arch = "wasm32")]
fn intern_reason(reason: &str) -> &'static str {
    match reason {
        "journal_schema_unsupported" => "journal_schema_unsupported",
        "journal_record_corrupt" => "journal_record_corrupt",
        "empty_append_batch" => "empty_append_batch",
        "append_batch_id_reuse" => "append_batch_id_reuse",
        "mixed_record_id_reuse" => "mixed_record_id_reuse",
        "mixed_record_batch_reuse" => "mixed_record_batch_reuse",
        "record_id_reuse" => "record_id_reuse",
        "snapshot_session_not_found" => "snapshot_session_not_found",
        "snapshot_ahead_of_journal" => "snapshot_ahead_of_journal",
        "snapshot_sequence_regression" => "snapshot_sequence_regression",
        "sequence_exhausted" => "sequence_exhausted",
        "transitional_payload_encode_failed" => "transitional_payload_encode_failed",
        "js_host_store_prebeta" => "js_host_store_prebeta",
        "js_host_failed" => crate::host::JS_HOST_FAILED,
        "js_host_result_invalid" => crate::host::JS_HOST_RESULT_INVALID,
        _ => "js_host_store_error",
    }
}

#[cfg(target_arch = "wasm32")]
fn intern_resource(resource: &str) -> &'static str {
    match resource {
        "sessions" => "sessions",
        "batches_per_session" => "batches_per_session",
        "records_per_session" => "records_per_session",
        "snapshot_bytes" => "snapshot_bytes",
        "blob_bytes" => "blob_bytes",
        "blobs" => "blobs",
        _ => "unknown",
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{HostJournalStore, HostJournalStoreOptions};
    use crate::executor::block_on_ready;
    use crate::host::NativeHostResult;
    use finstack_ai::runtime::{JournalStore, LoadRequest};
    use finstack_ai_kernel::SessionId;

    #[test]
    fn scripted_store_is_ready_and_not_durable() {
        let store = HostJournalStore::from_callback(
            HostJournalStoreOptions {
                detail: "js_memory_prebeta".into(),
            },
            || {
                Ok(NativeHostResult::Object(
                    r#"{"ready":true,"detail":"js_memory_prebeta"}"#.into(),
                ))
            },
        );
        let health = block_on_ready(store.health()).expect("health");
        assert!(health.ready);
        assert!(!health.durable);
        let session = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("session");
        let loaded = block_on_ready(store.load(LoadRequest {
            session_id: session,
        }))
        .expect("load");
        assert_eq!(loaded.head_sequence, 0);
    }
}
