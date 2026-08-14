//! Compile-time proof of native and browser-WASM `JournalStore` object bounds.

use std::sync::Arc;

use finstack_ai_kernel::{AppendRequest, CommittedBatch};
use finstack_ai_runtime::{
    JournalStore, LoadRequest, LoadedSession, PortFuture, SnapshotReceipt, SnapshotRequest,
    StoreError, StoreHealth,
};

#[cfg(not(target_arch = "wasm32"))]
struct NativeStore(std::sync::Mutex<()>);

#[cfg(not(target_arch = "wasm32"))]
impl JournalStore for NativeStore {
    fn append(&self, _request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        drop(self.0.lock());
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "compile_proof",
            })
        })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        Box::pin(async move { Ok(LoadedSession::empty(request.session_id)) })
    }

    fn write_snapshot(
        &self,
        _request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "compile_proof",
            })
        })
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        Box::pin(async {
            Ok(StoreHealth {
                ready: true,
                durable: false,
                detail: Arc::from("compile_proof"),
            })
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_store_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<NativeStore>();
    let _: Arc<dyn JournalStore> = Arc::new(NativeStore(std::sync::Mutex::new(())));
}

#[cfg(target_arch = "wasm32")]
struct LocalStore(std::rc::Rc<std::cell::Cell<u64>>);

#[cfg(target_arch = "wasm32")]
impl JournalStore for LocalStore {
    fn append(&self, _request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        self.0.set(self.0.get() + 1);
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "compile_proof",
            })
        })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        Box::pin(async move { Ok(LoadedSession::empty(request.session_id)) })
    }

    fn write_snapshot(
        &self,
        _request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "compile_proof",
            })
        })
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        Box::pin(async {
            Ok(StoreHealth {
                ready: true,
                durable: false,
                detail: Arc::from("compile_proof"),
            })
        })
    }
}

#[cfg(target_arch = "wasm32")]
#[test]
fn wasm_store_may_be_local_and_non_send() {
    let local = LocalStore(std::rc::Rc::new(std::cell::Cell::new(0)));
    let _: Arc<dyn JournalStore> = Arc::new(local);
}
