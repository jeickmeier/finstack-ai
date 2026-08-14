//! Private bounded native model job/result path.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    EffectId, EffectInput, KernelInput, PostCommitAction, ReducerStageOutcome,
};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::coordinator::{DispatchError, ModelDispatchSeed, PostCommitDispatcher, RuntimeDispatch};
use crate::settlement::ModelDriverResult;
use crate::{
    CancellationSignal, Clock, LockedModelContextProfile, Metadata, Model, ModelCallContext,
    ModelError, ModelProgress, ModelRequest, ModelRequestDraft, ModelStreamAssembler,
    ModelTerminal, MonotonicDeadline, PortFuture, RunCallContext, validate_model_request,
};

pub(crate) struct ModelJob {
    pub(crate) seed: ModelDispatchSeed,
    pub(crate) request: ModelRequest,
}

pub(crate) enum ModelDriverMessage {
    Progress {
        effect_id: EffectId,
        provider: Arc<str>,
        progress: ModelProgress,
    },
    Terminal(Box<ModelDriverResult>),
}

type ActiveEffects = Arc<Mutex<BTreeMap<EffectId, CancellationSignal>>>;

pub(crate) struct ModelDispatcher {
    model: Arc<dyn Model>,
    profile: LockedModelContextProfile,
    jobs: mpsc::Sender<ModelJob>,
    active: ActiveEffects,
    parent: CancellationSignal,
}

impl ModelDispatcher {
    pub(crate) fn new(
        model: Arc<dyn Model>,
        profile: LockedModelContextProfile,
        jobs: mpsc::Sender<ModelJob>,
        parent: CancellationSignal,
    ) -> Self {
        Self {
            model,
            profile,
            jobs,
            active: Arc::new(Mutex::new(BTreeMap::new())),
            parent,
        }
    }

    pub(crate) fn active(&self) -> ActiveEffects {
        Arc::clone(&self.active)
    }

    pub(crate) async fn resume_request(
        &self,
        seed: ModelDispatchSeed,
    ) -> Result<(), DispatchError> {
        let effect_id = seed.pending.requested.effect_id();
        self.dispatch(RuntimeDispatch {
            action: finstack_ai_kernel::PostCommitAction::ExecuteEffect { effect_id },
            model: Some(seed),
            tool: None,
            timer: None,
        })
        .await
    }

    fn parse_and_validate(
        &self,
        raw: &finstack_ai_kernel::RawJson,
    ) -> Result<ModelRequestDraft, ModelError> {
        let draft: ModelRequestDraft = serde_json::from_slice(raw.as_bytes()).map_err(|_| {
            ModelError::try_new(
                crate::MODEL_REQUEST_INVALID,
                finstack_ai_kernel::ErrorCategory::Validation,
                false,
                "committed model request draft is invalid",
                finstack_ai_kernel::Metadata::empty(),
            )
            .expect("frozen model request error")
        })?;
        if draft.canonical_bytes()?.as_slice() != raw.as_bytes() {
            return Err(ModelError::try_new(
                crate::MODEL_REQUEST_INVALID,
                finstack_ai_kernel::ErrorCategory::Validation,
                false,
                "model request draft is not the canonical committed DTO",
                finstack_ai_kernel::Metadata::empty(),
            )
            .expect("frozen model request error"));
        }
        validate_model_request(self.model.as_ref(), &draft, &self.profile)?;
        Ok(draft)
    }
}

impl PostCommitDispatcher for ModelDispatcher {
    fn validate_before_commit(&self, input: &KernelInput) -> Result<(), DispatchError> {
        let KernelInput::StageSettled(settled) = input else {
            return Ok(());
        };
        let ReducerStageOutcome::ModelRequestPrepared { request, .. } = &settled.outcome else {
            return Ok(());
        };
        self.parse_and_validate(request)
            .map(|_| ())
            .map_err(|error| DispatchError {
                code: stable_dispatch_code(error.code()),
            })
    }

    fn dispatch(&self, dispatch: RuntimeDispatch) -> PortFuture<Result<(), DispatchError>> {
        match dispatch.action {
            PostCommitAction::CancelEffect { effect_id } => {
                let cancellation = self
                    .active
                    .lock()
                    .ok()
                    .and_then(|active| active.get(&effect_id).cloned());
                Box::pin(async move {
                    // Completion may win the active-map race after the durable
                    // cancellation decision. The run worker still rejects the
                    // late terminal from committed state, so absence is an
                    // idempotent cancellation acknowledgement rather than a
                    // lane fault.
                    if let Some(cancellation) = cancellation {
                        cancellation.cancel();
                    }
                    Ok(())
                })
            }
            PostCommitAction::ExecuteEffect { effect_id } => {
                let Some(seed) = dispatch.model else {
                    return Box::pin(async {
                        Err(DispatchError {
                            code: "unsupported_effect_driver",
                        })
                    });
                };
                let EffectInput::Model { request: raw } = seed.pending.requested.input() else {
                    return Box::pin(async {
                        Err(DispatchError {
                            code: "model_request_invalid",
                        })
                    });
                };
                let draft = match self.parse_and_validate(raw) {
                    Ok(draft) => draft,
                    Err(error) => {
                        return Box::pin(async move {
                            Err(DispatchError {
                                code: stable_dispatch_code(error.code()),
                            })
                        });
                    }
                };
                let cancellation = self.parent.child();
                {
                    let Ok(mut active) = self.active.lock() else {
                        return Box::pin(async {
                            Err(DispatchError {
                                code: "model_effect_registry_unavailable",
                            })
                        });
                    };
                    if active.insert(effect_id, cancellation.clone()).is_some() {
                        return Box::pin(async {
                            Err(DispatchError {
                                code: "model_effect_already_active",
                            })
                        });
                    }
                }
                let request = ModelRequest {
                    call: ModelCallContext {
                        run: RunCallContext {
                            locator: seed.locator.clone(),
                            authorization: seed.authorization.clone(),
                            effect_id,
                            attempt: seed.attempt,
                            deadline: seed.pending.requested.deadline(),
                            budget_scope_id: seed.budget_scope_id,
                            cancellation,
                        },
                        request_id: seed.pending.model_request_id,
                    },
                    draft,
                    continuation_state: None,
                };
                let jobs = self.jobs.clone();
                let active = Arc::clone(&self.active);
                Box::pin(async move {
                    if jobs.send(ModelJob { seed, request }).await.is_err() {
                        if let Ok(mut values) = active.lock() {
                            values.remove(&effect_id);
                        }
                        return Err(DispatchError {
                            code: "model_job_queue_closed",
                        });
                    }
                    Ok(())
                })
            }
        }
    }
}

pub(crate) async fn run_model_jobs<C>(
    model: Arc<dyn Model>,
    assembler: ModelStreamAssembler,
    active: ActiveEffects,
    mut jobs: mpsc::Receiver<ModelJob>,
    results: mpsc::Sender<ModelDriverMessage>,
    clock: Arc<C>,
    cancellation_grace: Duration,
) where
    C: Clock + Send + Sync + 'static,
{
    let provider = model.descriptor().provider;
    let mut tasks = JoinSet::new();
    let mut intake_open = true;
    while intake_open || !tasks.is_empty() {
        tokio::select! {
            job = jobs.recv(), if intake_open => {
                if let Some(job) = job {
                    let model = Arc::clone(&model);
                    let provider = Arc::clone(&provider);
                    let results = results.clone();
                    let active = Arc::clone(&active);
                    let clock = Arc::clone(&clock);
                    tasks.spawn(async move {
                        let effect_id = job.seed.pending.requested.effect_id();
                        let draft = job.request.draft.clone();
                        let progress_sender = results.clone();
                        let progress_provider = Arc::clone(&provider);
                        let cancellation = job.request.call.run.cancellation.clone();
                        let deadline = job
                            .request
                            .call
                            .run
                            .deadline
                            .map(|deadline| {
                                MonotonicDeadline::from_persisted(
                                    clock.as_ref(),
                                    job.seed.requested_at,
                                    deadline,
                                )
                            })
                            .transpose();
                        let result = match model_job_preflight(deadline, &cancellation) {
                            Err(error) => Err(error),
                            Ok(deadline) => {
                            let mut child = tokio::spawn(async move {
                                match model.request(job.request).await {
                                    Ok(stream) => assembler.assemble_incremental(stream, move |progress| {
                                        let sender = progress_sender.clone();
                                        let provider = Arc::clone(&progress_provider);
                                        async move {
                                            sender.send(ModelDriverMessage::Progress {
                                                effect_id,
                                                provider,
                                                progress,
                                            }).await.map_err(|_| progress_delivery_error())
                                        }
                                    }).await,
                                    Err(error) => Err(error),
                                }
                            });
                            if let Some(deadline) = deadline {
                                tokio::select! {
                                    biased;
                                    () = cancellation.cancelled() => {
                                        settle_model_cancellation(&mut child, cancellation_grace).await
                                    }
                                    () = deadline.wait() => {
                                        cancellation.cancel();
                                        child.abort();
                                        let _ = (&mut child).await;
                                        Err(model_deadline_error())
                                    }
                                    joined = &mut child => joined_model_result(joined),
                                }
                            } else {
                                tokio::select! {
                                    biased;
                                    () = cancellation.cancelled() => {
                                        settle_model_cancellation(&mut child, cancellation_grace).await
                                    }
                                    joined = &mut child => joined_model_result(joined),
                                }
                            }
                            }
                        };
                        if let Ok(mut values) = active.lock() {
                            values.remove(&effect_id);
                        }
                        let _ = results.send(ModelDriverMessage::Terminal(Box::new(ModelDriverResult {
                            seed: job.seed,
                            draft,
                            provider,
                            result,
                        }))).await;
                    });
                } else {
                    intake_open = false;
                    if let Ok(values) = active.lock() {
                        for cancellation in values.values() {
                            cancellation.cancel();
                        }
                    }
                }
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}

async fn settle_model_cancellation(
    child: &mut tokio::task::JoinHandle<Result<ModelTerminal, ModelError>>,
    grace: Duration,
) -> Result<ModelTerminal, ModelError> {
    if let Ok(joined) = timeout(grace, &mut *child).await {
        return joined_model_result(joined);
    }
    child.abort();
    let _ = child.await;
    Err(model_cancellation_error(
        "model request exceeded its cancellation grace period",
    ))
}

fn model_job_preflight(
    deadline: Result<Option<MonotonicDeadline>, crate::RuntimeTimeError>,
    cancellation: &CancellationSignal,
) -> Result<Option<MonotonicDeadline>, ModelError> {
    if cancellation.is_cancelled() {
        return Err(model_cancellation_error(
            "model request was cancelled before execution",
        ));
    }
    let deadline = deadline.map_err(|_| model_deadline_error())?;
    if deadline
        .as_ref()
        .is_some_and(|deadline| deadline.remaining() == finstack_ai_kernel::Duration::ZERO)
    {
        cancellation.cancel();
        return Err(model_deadline_error());
    }
    Ok(deadline)
}

fn model_cancellation_error(message: &'static str) -> ModelError {
    ModelError::try_new(
        "model_cancelled",
        finstack_ai_kernel::ErrorCategory::Cancellation,
        false,
        message,
        Metadata::empty(),
    )
    .expect("frozen model cancellation error")
}

fn joined_model_result(
    joined: Result<Result<ModelTerminal, ModelError>, tokio::task::JoinError>,
) -> Result<ModelTerminal, ModelError> {
    match joined {
        Ok(result) => result,
        Err(_) => Err(ModelError::try_new(
            "model_panicked",
            finstack_ai_kernel::ErrorCategory::Internal,
            false,
            "model execution failed at the native task boundary",
            Metadata::empty(),
        )
        .expect("frozen model task error")),
    }
}

fn model_deadline_error() -> ModelError {
    ModelError::try_new(
        "model_deadline_exceeded",
        finstack_ai_kernel::ErrorCategory::Deadline,
        false,
        "model request exceeded its committed deadline",
        Metadata::empty(),
    )
    .expect("frozen model deadline error")
}

fn progress_delivery_error() -> ModelError {
    ModelError::try_new(
        "model_progress_delivery_closed",
        finstack_ai_kernel::ErrorCategory::Internal,
        false,
        "runtime model progress delivery closed",
        finstack_ai_kernel::Metadata::empty(),
    )
    .expect("frozen model progress delivery error")
}

fn stable_dispatch_code(code: &str) -> &'static str {
    match code {
        crate::MODEL_REQUEST_INVALID => crate::MODEL_REQUEST_INVALID,
        crate::MODEL_PROFILE_INVALID => crate::MODEL_PROFILE_INVALID,
        crate::MODEL_PROFILE_RELAXATION => crate::MODEL_PROFILE_RELAXATION,
        crate::MODEL_PROFILE_OVERRIDE_NOT_ALLOWED => crate::MODEL_PROFILE_OVERRIDE_NOT_ALLOWED,
        crate::MODEL_ESTIMATOR_MISMATCH => crate::MODEL_ESTIMATOR_MISMATCH,
        crate::MODEL_CONTEXT_LIMIT_EXCEEDED => crate::MODEL_CONTEXT_LIMIT_EXCEEDED,
        _ => "model_request_invalid",
    }
}
