//! Deterministic scripted `ContextProvider`, `Middleware`, `Observer`, and store faults.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{AppendRequest, CommittedBatch, ErrorCategory, Metadata, RunEvent};
use finstack_ai_runtime::{
    ContextCallContext, ContextContribution, ContextError, ContextProvider,
    ContextProviderDescriptor, ContextRequest, JournalStore, LoadRequest, LoadedSession,
    Middleware, MiddlewareContext, MiddlewareDescriptor, MiddlewareError, Observer,
    ObserverDescriptor, ObserverError, PortFuture, SnapshotReceipt, SnapshotRequest, StageInput,
    StageOutcome, StoreError, StoreHealth,
};

use crate::ManualGate;

/// One scripted `ContextProvider` action.
#[derive(Debug, Clone)]
pub enum ScriptedContextAction {
    /// Return immediately.
    Return(Result<ContextContribution, ContextError>),
    /// Wait at an explicit gate, then return.
    Wait {
        /// Gate controlled by the test.
        gate: ManualGate,
        /// Result returned after release.
        outcome: Result<ContextContribution, ContextError>,
    },
}

/// Queue-backed deterministic `ContextProvider`.
#[derive(Debug)]
pub struct ScriptedContextProvider {
    descriptor: ContextProviderDescriptor,
    actions: Mutex<VecDeque<ScriptedContextAction>>,
    calls: AtomicUsize,
}

impl ScriptedContextProvider {
    /// Construct with source-ordered collection actions.
    #[must_use]
    pub fn new(descriptor: ContextProviderDescriptor, actions: Vec<ScriptedContextAction>) -> Self {
        Self {
            descriptor,
            actions: Mutex::new(actions.into()),
            calls: AtomicUsize::new(0),
        }
    }

    /// Number of collection calls observed.
    #[must_use]
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }
}

impl ContextProvider for ScriptedContextProvider {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        _ctx: ContextCallContext,
        _request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let action = self
            .actions
            .lock()
            .ok()
            .and_then(|mut queue| queue.pop_front());
        Box::pin(async move {
            match action {
                Some(ScriptedContextAction::Return(outcome)) => outcome,
                Some(ScriptedContextAction::Wait { gate, outcome }) => {
                    gate.wait().await;
                    outcome
                }
                None => Err(ContextError::try_new(
                    "scripted_context_exhausted",
                    ErrorCategory::Context,
                    "no scripted context action remains",
                    Metadata::empty(),
                )
                .unwrap_or_else(ContextError::from)),
            }
        })
    }
}

/// One scripted `Middleware` action.
#[derive(Debug, Clone)]
pub enum ScriptedMiddlewareAction {
    /// Return immediately.
    Return(Result<StageOutcome, MiddlewareError>),
    /// Wait at an explicit gate, then return.
    Wait {
        /// Gate controlled by the test.
        gate: ManualGate,
        /// Result returned after release.
        outcome: Result<StageOutcome, MiddlewareError>,
    },
}

/// Queue-backed deterministic `Middleware`.
#[derive(Debug)]
pub struct ScriptedMiddleware {
    descriptor: MiddlewareDescriptor,
    actions: Mutex<VecDeque<ScriptedMiddlewareAction>>,
    calls: AtomicUsize,
}

impl ScriptedMiddleware {
    /// Construct with source-ordered stage actions.
    #[must_use]
    pub fn new(descriptor: MiddlewareDescriptor, actions: Vec<ScriptedMiddlewareAction>) -> Self {
        Self {
            descriptor,
            actions: Mutex::new(actions.into()),
            calls: AtomicUsize::new(0),
        }
    }

    /// Number of stage calls observed.
    #[must_use]
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }
}

impl Middleware for ScriptedMiddleware {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        _ctx: MiddlewareContext,
        _input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let action = self
            .actions
            .lock()
            .ok()
            .and_then(|mut queue| queue.pop_front());
        Box::pin(async move {
            match action {
                Some(ScriptedMiddlewareAction::Return(outcome)) => outcome,
                Some(ScriptedMiddlewareAction::Wait { gate, outcome }) => {
                    gate.wait().await;
                    outcome
                }
                None => Err(MiddlewareError::try_new(
                    "scripted_middleware_exhausted",
                    ErrorCategory::Middleware,
                    "no scripted middleware action remains",
                    Metadata::empty(),
                )
                .unwrap_or_else(MiddlewareError::from)),
            }
        })
    }
}

/// One scripted `Observer` action.
#[derive(Debug, Clone)]
pub enum ScriptedObserverAction {
    /// Return immediately.
    Return(Result<(), ObserverError>),
    /// Wait at an explicit gate, then return.
    Wait {
        /// Gate controlled by the test.
        gate: ManualGate,
        /// Result returned after release.
        outcome: Result<(), ObserverError>,
    },
}

/// Queue-backed deterministic `Observer` with bounded captured batches.
#[derive(Debug)]
pub struct ScriptedObserver {
    descriptor: ObserverDescriptor,
    actions: Mutex<VecDeque<ScriptedObserverAction>>,
    max_events: usize,
    batches: Mutex<Vec<Arc<[RunEvent]>>>,
}

impl ScriptedObserver {
    /// Construct with an explicit capture ceiling and source-ordered actions.
    ///
    /// # Errors
    ///
    /// Returns [`ObserverError::ConfigurationInvalid`] for a zero event ceiling.
    pub fn try_new(
        descriptor: ObserverDescriptor,
        max_events: usize,
        actions: Vec<ScriptedObserverAction>,
    ) -> Result<Self, ObserverError> {
        if max_events == 0 {
            return Err(ObserverError::ConfigurationInvalid);
        }
        Ok(Self {
            descriptor,
            actions: Mutex::new(actions.into()),
            max_events,
            batches: Mutex::new(Vec::new()),
        })
    }

    /// Snapshot captured source batches in delivery order.
    ///
    /// # Errors
    ///
    /// Returns [`ObserverError::Unavailable`] when capture state is unavailable.
    pub fn batches(&self) -> Result<Arc<[Arc<[RunEvent]>]>, ObserverError> {
        self.batches
            .lock()
            .map(|batches| Arc::from(batches.clone()))
            .map_err(|_| ObserverError::Unavailable)
    }
}

impl Observer for ScriptedObserver {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }

    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        let captured = self.batches.lock().ok().map_or(0, |batches| {
            batches.iter().map(|current| current.len()).sum::<usize>()
        });
        if captured.saturating_add(batch.len()) > self.max_events {
            return Box::pin(async { Err(ObserverError::CapacityExceeded) });
        }
        if let Ok(mut batches) = self.batches.lock() {
            batches.push(batch);
        } else {
            return Box::pin(async { Err(ObserverError::Unavailable) });
        }
        let action = self
            .actions
            .lock()
            .ok()
            .and_then(|mut queue| queue.pop_front());
        Box::pin(async move {
            match action {
                Some(ScriptedObserverAction::Return(outcome)) => outcome,
                Some(ScriptedObserverAction::Wait { gate, outcome }) => {
                    gate.wait().await;
                    outcome
                }
                None => Ok(()),
            }
        })
    }
}

/// `JournalStore` operation eligible for one injected fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOperation {
    /// Atomic append.
    Append,
    /// Session load.
    Load,
    /// Snapshot replacement.
    WriteSnapshot,
    /// Readiness health query.
    Health,
    /// Session-local scan.
    Scan,
    /// Metadata compare-and-swap.
    WriteMetadata,
}

/// Public `JournalStore` wrapper with deterministic one-shot operation faults.
pub struct FaultJournalStore {
    inner: Arc<dyn JournalStore>,
    faults: Mutex<VecDeque<(StoreOperation, StoreError)>>,
}

impl FaultJournalStore {
    /// Wrap a direct store handle.
    #[must_use]
    pub fn new(inner: Arc<dyn JournalStore>) -> Self {
        Self {
            inner,
            faults: Mutex::new(VecDeque::new()),
        }
    }

    /// Queue one operation-specific fault.
    pub fn fail_next(&self, operation: StoreOperation, error: StoreError) {
        if let Ok(mut faults) = self.faults.lock() {
            faults.push_back((operation, error));
        }
    }

    fn take_fault(&self, operation: StoreOperation) -> Option<StoreError> {
        let mut faults = self.faults.lock().ok()?;
        let index = faults
            .iter()
            .position(|(candidate, _)| *candidate == operation)?;
        faults.remove(index).map(|(_, error)| error)
    }
}

impl JournalStore for FaultJournalStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        if let Some(error) = self.take_fault(StoreOperation::Append) {
            return Box::pin(async move { Err(error) });
        }
        self.inner.append(request)
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        if let Some(error) = self.take_fault(StoreOperation::Load) {
            return Box::pin(async move { Err(error) });
        }
        self.inner.load(request)
    }

    fn load_from(
        &self,
        request: finstack_ai_runtime::LoadFromRequest,
    ) -> PortFuture<Result<LoadedSession, StoreError>> {
        // Delegate instead of using the trait default so wrapped stores keep
        // their own unified gap/split window semantics.
        if let Some(error) = self.take_fault(StoreOperation::Load) {
            return Box::pin(async move { Err(error) });
        }
        self.inner.load_from(request)
    }

    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        if let Some(error) = self.take_fault(StoreOperation::WriteSnapshot) {
            return Box::pin(async move { Err(error) });
        }
        self.inner.write_snapshot(request)
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        if let Some(error) = self.take_fault(StoreOperation::Health) {
            return Box::pin(async move { Err(error) });
        }
        self.inner.health()
    }

    fn scan(
        &self,
        request: finstack_ai_runtime::ScanRequest,
    ) -> PortFuture<Result<finstack_ai_runtime::ScanPage, StoreError>> {
        if let Some(error) = self.take_fault(StoreOperation::Scan) {
            return Box::pin(async move { Err(error) });
        }
        self.inner.scan(request)
    }

    fn write_metadata(
        &self,
        request: finstack_ai_runtime::WriteMetadataRequest,
    ) -> PortFuture<Result<finstack_ai_runtime::MetadataReceipt, StoreError>> {
        if let Some(error) = self.take_fault(StoreOperation::WriteMetadata) {
            return Box::pin(async move { Err(error) });
        }
        self.inner.write_metadata(request)
    }
}

/// Store wrapper that commits, then returns [`StoreError::AmbiguousAcknowledgement`] once.
pub struct AmbiguousAckAfterCommitStore {
    inner: Arc<dyn JournalStore>,
    remaining: Arc<AtomicU32>,
}

impl AmbiguousAckAfterCommitStore {
    /// Fail the first successful append acknowledgement.
    #[must_use]
    pub fn once(inner: Arc<dyn JournalStore>) -> Self {
        Self {
            inner,
            remaining: Arc::new(AtomicU32::new(1)),
        }
    }
}

impl JournalStore for AmbiguousAckAfterCommitStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        let inner = Arc::clone(&self.inner);
        let remaining = Arc::clone(&self.remaining);
        Box::pin(async move {
            let result = inner.append(request).await;
            if result.is_ok()
                && remaining.load(Ordering::SeqCst) > 0
                && remaining.fetch_sub(1, Ordering::SeqCst) == 1
            {
                return Err(StoreError::AmbiguousAcknowledgement);
            }
            result
        })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        self.inner.load(request)
    }

    fn load_from(
        &self,
        request: finstack_ai_runtime::LoadFromRequest,
    ) -> PortFuture<Result<LoadedSession, StoreError>> {
        // Delegate instead of using the trait default so wrapped stores keep
        // their own unified gap/split window semantics.
        self.inner.load_from(request)
    }

    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        self.inner.write_snapshot(request)
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        self.inner.health()
    }
}
