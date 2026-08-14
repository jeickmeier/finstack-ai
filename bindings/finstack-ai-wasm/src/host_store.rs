//! In-memory / scripted JS journal store. Pre-beta: not crash-durable.

use std::sync::Arc;

use finstack_ai::runtime::{
    JournalStore, LoadRequest, LoadedSession, PortFuture, SnapshotReceipt, SnapshotRequest,
    StoreError, StoreHealth,
};
use finstack_ai_kernel::{AppendRequest, CommittedBatch};
use serde::Deserialize;

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

/// Scripted host journal store. Appends are rejected; load is empty; health is not durable.
pub struct HostJournalStore {
    detail: Arc<str>,
    #[cfg(not(target_arch = "wasm32"))]
    health: Arc<dyn Fn() -> Result<NativeHostResult, HostFailure> + Send + Sync>,
    #[cfg(target_arch = "wasm32")]
    adapter: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    health: std::rc::Rc<std::cell::RefCell<js_sys::Function>>,
}

impl HostJournalStore {
    fn from_parts(
        options: HostJournalStoreOptions,
        #[cfg(not(target_arch = "wasm32"))] health: Arc<
            dyn Fn() -> Result<NativeHostResult, HostFailure> + Send + Sync,
        >,
        #[cfg(target_arch = "wasm32")] adapter: wasm_bindgen::JsValue,
        #[cfg(target_arch = "wasm32")] health: js_sys::Function,
    ) -> Self {
        Self {
            detail: Arc::from(options.detail),
            #[cfg(not(target_arch = "wasm32"))]
            health,
            #[cfg(target_arch = "wasm32")]
            adapter,
            #[cfg(target_arch = "wasm32")]
            health: std::rc::Rc::new(std::cell::RefCell::new(health)),
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

    /// Construct a wasm32 scripted store around a JS `HostJournalStore`.
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
        Ok(Self::from_parts(options, adapter, health))
    }
}

impl JournalStore for HostJournalStore {
    fn append(&self, _request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "js_host_store_prebeta",
            })
        })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        Box::pin(async move {
            Ok(LoadedSession {
                session_id: request.session_id,
                head_sequence: 0,
                committed_batches: Arc::from([]),
                snapshot: None,
            })
        })
    }

    fn write_snapshot(
        &self,
        _request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "js_host_store_prebeta",
            })
        })
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{HostJournalStore, HostJournalStoreOptions};
    use crate::executor::block_on_ready;
    use crate::host::NativeHostResult;
    use finstack_ai::runtime::{JournalStore, LoadRequest, SessionId};

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
