use std::sync::Arc;

use finstack_ai_kernel::{AppendRequest, CommittedBatch};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::journal::{
    JournalStore, LoadFromRequest, LoadRequest, LoadedSession, MetadataReceipt, PruneReceipt,
    PruneRequest, ScanPage, ScanRequest, SnapshotReceipt, SnapshotRequest, StateSnapshotRequest,
    StoreError, StoreHealth, WriteMetadataRequest,
};

use crate::store::SqliteJournalStore;

impl JournalStore for SqliteJournalStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        self.worker.submit(move |ctx| ctx.append(&request))
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        self.worker.submit(move |ctx| ctx.load(request))
    }

    fn load_from(&self, request: LoadFromRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        self.worker.submit(move |ctx| ctx.load_from(request))
    }

    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        self.worker.submit(move |ctx| ctx.write_snapshot(&request))
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        let durable = self.durable;
        let detail = Arc::from(self.detail);
        self.worker.submit(move |ctx| {
            let ready = ctx
                .connection
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .is_ok();
            Ok(StoreHealth {
                ready,
                durable,
                detail,
            })
        })
    }

    fn scan(&self, request: ScanRequest) -> PortFuture<Result<ScanPage, StoreError>> {
        self.worker.submit(move |ctx| ctx.scan(request))
    }

    fn write_metadata(
        &self,
        request: WriteMetadataRequest,
    ) -> PortFuture<Result<MetadataReceipt, StoreError>> {
        self.worker.submit(move |ctx| ctx.write_metadata(request))
    }

    fn write_state_snapshot(
        &self,
        request: StateSnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        self.worker
            .submit(move |ctx| ctx.write_state_snapshot(&request))
    }

    fn prune(&self, request: PruneRequest) -> PortFuture<Result<PruneReceipt, StoreError>> {
        self.worker.submit(move |ctx| ctx.prune(request))
    }
}
