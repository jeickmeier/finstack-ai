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

#[expect(clippy::too_many_arguments, reason = "mirrors AllocatedIds' own bags")]
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

fn acceptance(limits: RunLimits) -> RunAccepted {
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
        limits,
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

fn accept_input(limits: RunLimits) -> KernelInput {
    KernelInput::AcceptRun(AcceptRun {
        session_id: id::<SessionTag>(1),
        lane_id: id::<LaneTag>(2),
        accepted: acceptance(limits),
    })
}

fn user_message(ordinal: u64, text: &str) -> Message {
    Message::try_new(
        id(ordinal),
        MessageRole::User,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        timestamp(900),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn message_text(message: &Message) -> String {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text().to_owned()),
            _ => None,
        })
        .collect()
}

fn item(text: &str) -> ContextItem {
    ContextItem::try_new(
        ContextItemKind::Instruction,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        ContextProvenance {
            source_id: Arc::from("fixture.source"),
            source_ref: None,
            external: false,
        },
        ContextAuthority::TrustedApplication,
        0,
        4,
        Sensitivity::Internal,
        false,
    )
    .expect("item")
}

fn descriptor(component: &str, stage: Stage) -> MiddlewareDescriptor {
    MiddlewareDescriptor {
        invocation: finstack_ai_kernel::ComponentInvocation {
            component: finstack_ai_kernel::ComponentId::parse(component).expect("component"),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            configuration_digest: Digest::raw_json(b"{}"),
            recovery: finstack_ai_kernel::InvocationRecovery::RecomputeSafe,
        },
        stages: StageMask::from_stages([stage]),
        order: MiddlewareOrder {
            tier: OrderTier::Standard,
            priority: 0,
            before: Arc::from([]),
            after: Arc::from([]),
        },
        role: MiddlewareRole::Standard,
        metadata: Metadata::empty(),
    }
}

/// A component that always returns one fixed outcome at one stage.
struct Fixed {
    descriptor: MiddlewareDescriptor,
    outcome: StageOutcome,
}

impl crate::middleware::Middleware for Fixed {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        _ctx: crate::middleware::MiddlewareContext,
        _input: crate::middleware::StageInput,
    ) -> PortFuture<Result<StageOutcome, crate::middleware::MiddlewareError>> {
        let outcome = self.outcome.clone();
        Box::pin(async move { Ok(outcome) })
    }
}

fn driver_for(component: &str, stage: Stage, outcome: StageOutcome) -> StageDriver {
    let middleware: Arc<dyn crate::middleware::Middleware> = Arc::new(Fixed {
        descriptor: descriptor(component, stage),
        outcome,
    });
    StageDriver::new(
        Arc::new(
            ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration { middleware }])
                .expect("chain"),
        ),
        CancellationSignal::new(),
    )
}

/// Deterministic, collision-free random source.
#[derive(Default)]
struct CountingRandom(AtomicU64);

impl RandomSource for CountingRandom {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
        let counter = self.0.fetch_add(1, Ordering::Relaxed);
        let bytes = counter.to_be_bytes();
        for (index, slot) in buf.iter_mut().enumerate() {
            *slot = bytes[index % bytes.len()];
        }
        Ok(())
    }
}

fn test_sources() -> SettlementSources<ExternalClock, CountingRandom> {
    SettlementSources::try_new(
        ExternalClock::new(timestamp(1_000)),
        CountingRandom::default(),
    )
    .expect("sources")
}

struct MemoryStore {
    inner: Mutex<MemoryInner>,
}

struct MemoryInner {
    batches: Vec<CommittedBatch>,
    requests: BTreeMap<AppendBatchId, AppendRequest>,
}

impl MemoryStore {
    fn new() -> Self {
        Self {
            inner: Mutex::new(MemoryInner {
                batches: Vec::new(),
                requests: BTreeMap::new(),
            }),
        }
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
    .expect("committed batch")
}

impl JournalStore for MemoryStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        let mut inner = self.inner.lock().expect("lock");
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
            return Box::pin(async move { Ok(committed) });
        }
        let committed = commit_request(&request);
        inner.requests.insert(request.batch_id(), request);
        inner.batches.push(committed.clone());
        Box::pin(async move { Ok(committed) })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        let inner = self.inner.lock().expect("lock");
        let batches = inner.batches.clone();
        let head_sequence = batches.last().map_or(0, |batch| batch.last_sequence);
        Box::pin(async move {
            Ok(LoadedSession {
                session_id: request.session_id,
                head_sequence,
                head_checksum: batches
                    .last()
                    .and_then(|batch| batch.records.last().map(RecordEnvelope::checksum)),
                metadata: Metadata::empty(),
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

/// An accepted, `BeforeRun`-phase coordinator over an in-memory journal.
fn accepted_coordinator(limits: RunLimits) -> CommitCoordinator {
    let mut coordinator = CommitCoordinator::new(Arc::new(MemoryStore::new()));
    block_on(coordinator.submit(
        env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
        accept_input(limits),
    ))
    .expect("accept");
    coordinator
}

/// Drive `coordinator` through `BeforeRun` so the next cursor is
/// `PrepareContext`.
fn drive_to_prepare_context(coordinator: &mut CommitCoordinator) {
    block_on(coordinator.submit(
        env(1_100, &[2], &[], &[], &[], &[], &[], 102),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeRun,
            },
            outcome: ReducerStageOutcome::Continue,
        }),
    ))
    .expect("before run");
}

fn committed_record_ids(outcome: &crate::CommitOutcome) -> Vec<finstack_ai_kernel::RecordId> {
    outcome
        .committed
        .as_ref()
        .expect("committed batch")
        .records
        .iter()
        .map(RecordEnvelope::record_id)
        .collect()
}

fn before_run_settled() -> StageSettled {
    StageSettled {
        cursor: StageCursor {
            cycle: 0,
            stage: Stage::BeforeRun,
        },
        outcome: ReducerStageOutcome::Continue,
    }
}
