fn block_on<T>(future: impl Future<Output = T>) -> T {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn timestamp(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("timestamp")
}

#[allow(clippy::too_many_arguments)]
fn env(
    now: i64,
    records: &[u64],
    events: &[u64],
    effects: &[u64],
    turns: &[u64],
    model_requests: &[u64],
    messages: &[u64],
    append_batch: u64,
) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            records.iter().copied().map(id).collect(),
            events.iter().copied().map(id).collect(),
            effects.iter().copied().map(id).collect(),
            Vec::new(),
            messages.iter().copied().map(id).collect(),
            turns.iter().copied().map(id).collect(),
            model_requests.iter().copied().map(id).collect(),
            Vec::new(),
            Vec::new(),
            vec![id(append_batch)],
            Vec::new(),
        )
        .expect("allocated ids"),
    }
}

fn acceptance() -> RunAccepted {
    let run_id = id(3);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a")).expect("principal"),
            "oidc",
            "high",
            "policy-v1",
            "decision-v1",
            None,
        )
        .expect("security"),
        None,
        RunLimits::empty(),
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(br#"{"agent":"fixture"}"#),
        None,
    )
    .expect("acceptance")
}

fn accept_input() -> KernelInput {
    KernelInput::AcceptRun(AcceptRun {
        session_id: id::<SessionTag>(1),
        lane_id: id::<LaneTag>(2),
        accepted: acceptance(),
    })
}

/// A `ChildAgent` acceptance one level below `acceptance`'s root, for
/// asserting that dispatch-seed `relation_depth` is the accepted run's real
/// relation depth rather than a stubbed `0`.
#[cfg_attr(
    not(any(feature = "native-tokio", feature = "wasm-host")),
    allow(dead_code)
)]
fn child_acceptance() -> RunAccepted {
    let root = acceptance();
    let child_run_id = id(9);
    let relation = RunRelation::try_new(
        root.relation().root_run_id(),
        Some(root.run_id()),
        Some(id::<finstack_ai_kernel::EffectTag>(10)),
        finstack_ai_kernel::RunRelationKind::ChildAgent,
        1,
        None,
        None::<&str>,
    )
    .expect("child relation");
    RunAccepted::try_new(
        child_run_id,
        relation,
        root.security().clone(),
        None,
        RunLimits::empty(),
        root.propagation(),
        root.resolved_agent_lock_digest(),
        Some(&root),
    )
    .expect("child accepted")
}

#[cfg_attr(
    not(any(feature = "native-tokio", feature = "wasm-host")),
    allow(dead_code)
)]
fn child_accept_input() -> KernelInput {
    KernelInput::AcceptRun(AcceptRun {
        session_id: id::<SessionTag>(1),
        lane_id: id::<LaneTag>(2),
        accepted: child_acceptance(),
    })
}

#[cfg_attr(
    not(any(feature = "native-tokio", feature = "wasm-host")),
    allow(dead_code)
)]
async fn drive_child_to_model_request(coordinator: &mut CommitCoordinator) -> CommitOutcome {
    coordinator
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            child_accept_input(),
        )
        .await
        .expect("accept child");
    drive_accepted_to_model_request(coordinator).await
}

fn create_compatible_lane(commit: &mut CommitCoordinator, lane: u64, batch: u64, record: u64) {
    block_on(commit.commit_session_records(
        id(batch),
        vec![session_draft(
                id(record),
                id::<SessionTag>(1),
                id(lane),
                timestamp(1_050),
                RecordBody::LaneCreated(LaneCreated::try_new("research").expect("lane")),
            )
            .expect("lane draft")],
    ))
    .expect("create child lane");
}

fn stage(stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
        cursor: StageCursor { cycle: 0, stage },
        outcome,
    })
}

fn context_message() -> Message {
    Message::try_new(
        id(4),
        MessageRole::User,
        vec![ContentBlock::Text(
            TextBlock::try_new("Say hello.").expect("text"),
        )],
        timestamp(900),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn output_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
    }
}

async fn drive_to_model_request(coordinator: &mut CommitCoordinator) -> CommitOutcome {
    coordinator
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        )
        .await
        .expect("accept");
    drive_accepted_to_model_request(coordinator).await
}

async fn drive_accepted_to_model_request(coordinator: &mut CommitCoordinator) -> CommitOutcome {
    coordinator
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    coordinator
        .submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            stage(
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([context_message()]),
                },
            ),
        )
        .await
        .expect("context");
    coordinator
        .submit(
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            stage(
                Stage::BeforeModel,
                ReducerStageOutcome::ModelRequestPrepared {
                    request: RawJson::parse(
                        r#"{"messages":[{"role":"user","text":"Say hello."}]}"#,
                    )
                    .expect("request"),
                    component: None,
                    output_contract: output_contract(),
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: Some(timestamp(8_000)),
                },
            ),
        )
        .await
        .expect("model request")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FakeMode {
    Normal,
    ConflictOnce,
    ConflictAlways,
    AmbiguousOnce,
    AmbiguousAlways,
    IntegrityAlways,
}

struct FakeStore {
    inner: Mutex<FakeInner>,
    composition_log: Option<Arc<Mutex<Vec<&'static str>>>>,
}

struct FakeInner {
    mode: FakeMode,
    append_calls: usize,
    batches: Vec<CommittedBatch>,
    requests: BTreeMap<finstack_ai_kernel::AppendBatchId, AppendRequest>,
}

#[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
struct StallingSnapshotStore {
    inner: FakeStore,
}

impl JournalStore for StallingSnapshotStore {
    fn append(
        &self,
        request: finstack_ai_kernel::AppendRequest,
    ) -> PortFuture<Result<finstack_ai_kernel::CommittedBatch, StoreError>> {
        self.inner.append(request)
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        self.inner.load(request)
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

    fn write_state_snapshot(
        &self,
        _request: StateSnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(async {
            #[cfg(feature = "native-tokio")]
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Err(StoreError::Unavailable {
                reason_code: "snapshot_write_stalled",
            })
        })
    }
}

impl FakeStore {
    fn new(mode: FakeMode) -> Self {
        Self {
            inner: Mutex::new(FakeInner {
                mode,
                append_calls: 0,
                batches: Vec::new(),
                requests: BTreeMap::new(),
            }),
            composition_log: None,
        }
    }

    fn with_composition_log(log: Arc<Mutex<Vec<&'static str>>>) -> Self {
        let mut store = Self::new(FakeMode::Normal);
        store.composition_log = Some(log);
        store
    }

    fn append_calls(&self) -> usize {
        self.inner.lock().expect("lock").append_calls
    }
}

impl JournalStore for FakeStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        if let Some(log) = &self.composition_log {
            for record in request.records() {
                let label = match record.body() {
                    RecordBody::ChildRunPrepared(_) => Some("prepare_committed"),
                    RecordBody::BudgetReservationRequested(_) => Some("reservation_requested"),
                    RecordBody::BudgetReservationSettled(_) => Some("reservation_settled"),
                    _ => None,
                };
                if let Some(label) = label {
                    log.lock().expect("log").push(label);
                }
            }
        }
        let mut inner = self.inner.lock().expect("lock");
        inner.append_calls += 1;
        if let Some(existing) = inner.requests.get(&request.batch_id()) {
            if existing != &request {
                return Box::pin(async {
                    Err(StoreError::Corruption {
                        reason_code: "batch_reuse",
                    })
                });
            }
            let committed = inner
                .batches
                .iter()
                .find(|batch| batch.batch_id == request.batch_id())
                .cloned()
                .expect("indexed batch");
            if inner.mode == FakeMode::AmbiguousAlways {
                return Box::pin(async { Err(StoreError::AmbiguousAcknowledgement) });
            }
            return Box::pin(async move { Ok(committed) });
        }
        if inner.mode == FakeMode::IntegrityAlways {
            return Box::pin(async {
                Err(StoreError::Integrity {
                    reason_code: "checksum_mismatch",
                })
            });
        }
        if inner.mode == FakeMode::ConflictAlways
            || (inner.mode == FakeMode::ConflictOnce && inner.append_calls == 1)
        {
            return Box::pin(async {
                Err(StoreError::Conflict {
                    expected_sequence: 1,
                    actual_next_sequence: 1,
                })
            });
        }
        let committed = commit_request(&request);
        inner.requests.insert(request.batch_id(), request);
        inner.batches.push(committed.clone());
        if matches!(
            inner.mode,
            FakeMode::AmbiguousOnce | FakeMode::AmbiguousAlways
        ) {
            return Box::pin(async { Err(StoreError::AmbiguousAcknowledgement) });
        }
        Box::pin(async move { Ok(committed) })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        let inner = self.inner.lock().expect("lock");
        let batches = inner
            .batches
            .iter()
            .filter(|batch| {
                batch
                    .records
                    .first()
                    .is_some_and(|record| record.session_id() == request.session_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        let head_sequence = batches.last().map_or(0, |batch| batch.last_sequence);
        Box::pin(async move {
            Ok(LoadedSession {
                session_id: request.session_id,
                head_sequence,
                head_checksum: batches.last().and_then(|batch| {
                    batch
                        .records
                        .last()
                        .map(finstack_ai_kernel::RecordEnvelope::checksum)
                }),
                metadata: finstack_ai_kernel::Metadata::empty(),
                committed_batches: batches.into(),
                snapshot: None,
                accelerated: None,
            })
        })
    }

    fn write_snapshot(
        &self,
        _request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "not_used",
            })
        })
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        Box::pin(async {
            Ok(StoreHealth {
                ready: true,
                durable: false,
                detail: Arc::from("test"),
            })
        })
    }
}

struct AmbiguousReserveLedger {
    log: Arc<Mutex<Vec<&'static str>>>,
    receipt: BudgetReservationReceipt,
    charge_receipt: Option<finstack_ai_kernel::BudgetChargeReceipt>,
    release_receipt: Option<finstack_ai_kernel::BudgetReleaseReceipt>,
    reserved: Mutex<bool>,
    fail_after_reserve_once: Mutex<bool>,
    reserve_calls: Mutex<usize>,
    charge_calls: Mutex<usize>,
    release_calls: Mutex<usize>,
}

impl BudgetLedger for AmbiguousReserveLedger {
    fn reserve(
        &self,
        request: BudgetReserveRequest,
    ) -> PortFuture<Result<BudgetReservationReceipt, BudgetError>> {
        self.log.lock().expect("log").push("reserve");
        *self.reserve_calls.lock().expect("calls") += 1;
        *self.reserved.lock().expect("reserved") = true;
        let fail = std::mem::take(&mut *self.fail_after_reserve_once.lock().expect("fail"));
        let receipt = self.receipt.clone();
        assert_eq!(request.request_digest, receipt.request_digest);
        Box::pin(async move {
            if fail {
                Err(BudgetError::Unavailable {
                    message: Arc::from("ambiguous reserve acknowledgement"),
                })
            } else {
                Ok(receipt)
            }
        })
    }

    fn reconcile(
        &self,
        scope_id: finstack_ai_kernel::BudgetScopeId,
        reservation_id: finstack_ai_kernel::BudgetReservationId,
    ) -> PortFuture<Result<BudgetReservationState, BudgetError>> {
        self.log.lock().expect("log").push("reconcile");
        let reserved = *self.reserved.lock().expect("reserved");
        let receipt = self.receipt.clone();
        assert_eq!(scope_id, receipt.scope_id);
        assert_eq!(reservation_id, receipt.reservation_id);
        Box::pin(async move {
            Ok(if reserved {
                BudgetReservationState::Reserved(receipt)
            } else {
                BudgetReservationState::NotFound
            })
        })
    }

    fn charge(
        &self,
        request: finstack_ai_kernel::BudgetChargeRequest,
    ) -> PortFuture<Result<finstack_ai_kernel::BudgetChargeReceipt, BudgetError>> {
        *self.charge_calls.lock().expect("calls") += 1;
        let receipt = self.charge_receipt.clone();
        Box::pin(async move {
            receipt.map_or_else(
                || {
                    Err(BudgetError::InvalidRequest {
                        message: Arc::from("charge not configured"),
                    })
                },
                |receipt| {
                    assert_eq!(receipt.effect_id, request.effect_id);
                    Ok(receipt)
                },
            )
        })
    }

    fn release(
        &self,
        request: finstack_ai_kernel::BudgetReleaseRequest,
    ) -> PortFuture<Result<finstack_ai_kernel::BudgetReleaseReceipt, BudgetError>> {
        *self.release_calls.lock().expect("calls") += 1;
        let receipt = self.release_receipt.clone();
        Box::pin(async move {
            receipt.map_or_else(
                || {
                    Err(BudgetError::InvalidRequest {
                        message: Arc::from("release not configured"),
                    })
                },
                |receipt| {
                    assert_eq!(receipt.terminal_run_id, request.terminal_run_id);
                    Ok(receipt)
                },
            )
        })
    }
}

struct IdempotentChildInvoker {
    log: Arc<Mutex<Vec<&'static str>>>,
    accepted: Mutex<BTreeMap<EffectId, Digest>>,
    physical_starts: Mutex<usize>,
}

impl AgentInvoker for IdempotentChildInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        self.log.lock().expect("log").push("invoke");
        let relation_digest = child_relation_digest(&context, &request).expect("relation");
        let mut accepted = self.accepted.lock().expect("accepted");
        match accepted.get(&context.parent_effect_id) {
            Some(existing) if *existing != request.request_digest => {
                let existing = *existing;
                let submitted = request.request_digest;
                return Box::pin(async move {
                    Err(AgentInvokeError::Conflict {
                        existing,
                        submitted,
                    })
                });
            }
            Some(_) => {}
            None => {
                accepted.insert(context.parent_effect_id, request.request_digest);
                *self.physical_starts.lock().expect("starts") += 1;
            }
        }
        let locator = request.locator;
        Box::pin(async move {
            Ok(ChildRunHandle {
                locator,
                relation_digest,
            })
        })
    }
}

fn commit_request(request: &AppendRequest) -> CommittedBatch {
    let records = request
        .records()
        .iter()
        .enumerate()
        .map(|(offset, draft)| {
            let sequence = request.expected_sequence() + u64::try_from(offset).expect("offset");
            RecordEnvelope::try_new(
                draft.format_version(),
                draft.kind_version(),
                draft.record_id(),
                draft.session_id(),
                draft.lane_id(),
                draft.run_id(),
                sequence,
                draft.timestamp(),
                None,
                Digest::raw_json(format!("payload-{sequence}").as_bytes()),
                None,
                Digest::raw_json(format!("checksum-{sequence}").as_bytes()),
                draft.derived_event_ids().to_vec(),
                draft.body().clone(),
            )
            .expect("envelope")
        })
        .collect::<Vec<_>>();
    CommittedBatch::try_new(
        request.batch_id(),
        request.expected_sequence(),
        request.expected_sequence() + u64::try_from(records.len()).expect("count") - 1,
        records,
    )
    .expect("batch")
}

#[derive(Default)]
struct RecordingDispatcher {
    actions: Mutex<Vec<PostCommitAction>>,
}

impl PostCommitDispatcher for RecordingDispatcher {
    fn dispatch(&self, dispatch: RuntimeDispatch) -> PortFuture<Result<(), DispatchError>> {
        self.actions.lock().expect("lock").push(dispatch.action);
        Box::pin(async { Ok(()) })
    }
}
