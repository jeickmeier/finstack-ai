use std::sync::Arc;

use finstack_ai_kernel::{AppendRequest, CommittedBatch};
use finstack_ai_runtime::{
    JournalStore, LoadRequest, LoadedSession, MetadataReceipt, PortFuture, PruneReceipt,
    PruneRequest, ScanPage, ScanRequest, SnapshotReceipt, SnapshotRequest, StateSnapshotRequest,
    StoreError, StoreHealth, WriteMetadataRequest,
};

use crate::store::SqliteJournalStore;

impl JournalStore for SqliteJournalStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        let result = self.append_sync(&request);
        Box::pin(async move { result })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        let result = self.load_sync(request);
        Box::pin(async move { result })
    }

    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        let result = self.write_snapshot_sync(&request);
        Box::pin(async move { result })
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        let ready = true;
        let durable = self.durable;
        let detail = Arc::from(self.detail);
        Box::pin(async move {
            Ok(StoreHealth {
                ready,
                durable,
                detail,
            })
        })
    }

    fn scan(&self, request: ScanRequest) -> PortFuture<Result<ScanPage, StoreError>> {
        let result = self.scan_sync(request);
        Box::pin(async move { result })
    }

    fn write_metadata(
        &self,
        request: WriteMetadataRequest,
    ) -> PortFuture<Result<MetadataReceipt, StoreError>> {
        let result = self.write_metadata_sync(request);
        Box::pin(async move { result })
    }

    fn write_state_snapshot(
        &self,
        request: StateSnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        let result = self.write_state_snapshot_sync(&request);
        Box::pin(async move { result })
    }

    fn prune(&self, request: PruneRequest) -> PortFuture<Result<PruneReceipt, StoreError>> {
        let result = self.prune_sync(request);
        Box::pin(async move { result })
    }
}
