//! Private bounded native Toolset job/result scheduler.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    EffectId, KernelInput, PostCommitAction, ReducerStageOutcome, ToolCallPlan, ToolId,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::{JoinHandle, JoinSet};

use crate::coordinator::{DispatchError, PostCommitDispatcher, RuntimeDispatch, ToolDispatchSeed};
use crate::model_runtime::ModelDispatcher;
use crate::{
    AssembledToolStream, CancellationSignal, PortFuture, ResolvedTool, ResolvedToolCatalog,
    RunCallContext, TOOL_CANCELLED, TOOL_PANICKED, ToolCallContext, ToolError, ToolStreamAssembler,
    ToolStreamLimits,
};

/// Configuration for bounded native tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolTaskConfig {
    /// Bounded committed tool-job queue capacity.
    pub job_capacity: usize,
    /// Bounded terminal result queue capacity.
    pub result_capacity: usize,
    /// Executor-wide active-call ceiling.
    pub global_max_concurrency: usize,
    /// Target-neutral stream normalization limits.
    pub stream_limits: ToolStreamLimits,
}

impl ToolTaskConfig {
    pub(crate) fn validate(self) -> Result<Self, &'static str> {
        if self.job_capacity == 0
            || self.result_capacity == 0
            || self.global_max_concurrency == 0
            || self.stream_limits.max_items == 0
            || self.stream_limits.max_stream_bytes == 0
        {
            return Err("tool_task_configuration_invalid");
        }
        Ok(self)
    }
}

pub(crate) struct ToolJob {
    pub(crate) seed: ToolDispatchSeed,
    pub(crate) context: ToolCallContext,
    pub(crate) resolved: Arc<ResolvedTool>,
}

pub(crate) struct ToolDriverResult {
    pub(crate) seed: ToolDispatchSeed,
    pub(crate) result: Result<AssembledToolStream, ToolError>,
}

type ActiveEffects = Arc<Mutex<BTreeMap<EffectId, CancellationSignal>>>;

pub(crate) struct ToolDispatcher {
    catalog: Arc<ResolvedToolCatalog>,
    jobs: mpsc::Sender<ToolJob>,
    active: ActiveEffects,
    per_tool: BTreeMap<ToolId, Arc<Semaphore>>,
}

impl ToolDispatcher {
    pub(crate) fn new(catalog: Arc<ResolvedToolCatalog>, jobs: mpsc::Sender<ToolJob>) -> Self {
        let per_tool = catalog
            .tools()
            .map(|tool| {
                (
                    tool.spec.id.clone(),
                    Arc::new(Semaphore::new(tool.policy.max_concurrency)),
                )
            })
            .collect();
        Self {
            catalog,
            jobs,
            active: Arc::new(Mutex::new(BTreeMap::new())),
            per_tool,
        }
    }

    pub(crate) fn active(&self) -> ActiveEffects {
        Arc::clone(&self.active)
    }

    pub(crate) fn semaphores(&self) -> BTreeMap<ToolId, Arc<Semaphore>> {
        self.per_tool.clone()
    }

    fn resolved_for_seed(
        &self,
        seed: &ToolDispatchSeed,
    ) -> Result<Arc<ResolvedTool>, DispatchError> {
        let resolved = self
            .catalog
            .by_id(&seed.call.tool_id)
            .cloned()
            .ok_or(DispatchError {
                code: "tool_resolution_missing",
            })?;
        let finstack_ai_kernel::EffectInput::Tool { call: committed } = seed.requested.input()
        else {
            return Err(DispatchError {
                code: "tool_dispatch_contract_mismatch",
            });
        };
        if seed.call.call.tool_name() != resolved.spec.model_name.as_ref()
            || committed != &seed.call.call
            || seed.call.output_contract != resolved.output_contract
            || seed.call.execution != resolved.spec.execution
            || seed.call.retry_safety != resolved.spec.retry_safety
            || seed.call.failure_policy != resolved.policy.failure_policy
        {
            return Err(DispatchError {
                code: "tool_dispatch_contract_mismatch",
            });
        }
        Ok(resolved)
    }
}

impl PostCommitDispatcher for ToolDispatcher {
    fn validate_before_commit(&self, input: &KernelInput) -> Result<(), DispatchError> {
        let KernelInput::StageSettled(settled) = input else {
            return Ok(());
        };
        let ReducerStageOutcome::ToolBatchPrepared { calls, .. } = &settled.outcome else {
            return Ok(());
        };
        for plan in calls.iter() {
            let ToolCallPlan::Execute(call) = plan else {
                continue;
            };
            let resolved = self.catalog.by_id(&call.tool_id).ok_or(DispatchError {
                code: "tool_resolution_missing",
            })?;
            if call.call.tool_name() != resolved.spec.model_name.as_ref()
                || call.output_contract != resolved.output_contract
                || call.execution != resolved.spec.execution
                || call.retry_safety != resolved.spec.retry_safety
                || call.failure_policy != resolved.policy.failure_policy
            {
                return Err(DispatchError {
                    code: "tool_dispatch_contract_mismatch",
                });
            }
        }
        Ok(())
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
                    if let Some(cancellation) = cancellation {
                        cancellation.cancel();
                    }
                    Ok(())
                })
            }
            PostCommitAction::ExecuteEffect { effect_id } => {
                let Some(seed) = dispatch.tool else {
                    return Box::pin(async {
                        Err(DispatchError {
                            code: "unsupported_effect_driver",
                        })
                    });
                };
                let resolved = match self.resolved_for_seed(&seed) {
                    Ok(value) => value,
                    Err(error) => return Box::pin(async move { Err(error) }),
                };
                let cancellation = CancellationSignal::new();
                {
                    let Ok(mut active) = self.active.lock() else {
                        return Box::pin(async {
                            Err(DispatchError {
                                code: "tool_effect_registry_unavailable",
                            })
                        });
                    };
                    if active.insert(effect_id, cancellation.clone()).is_some() {
                        return Box::pin(async {
                            Err(DispatchError {
                                code: "tool_effect_already_active",
                            })
                        });
                    }
                }
                let context = ToolCallContext {
                    run: RunCallContext {
                        locator: seed.locator.clone(),
                        authorization: seed.authorization.clone(),
                        effect_id,
                        attempt: seed.attempt,
                        deadline: seed.requested.deadline(),
                        budget_scope_id: seed.budget_scope_id,
                        cancellation,
                    },
                    tool_batch_id: seed.tool_batch_id,
                    tool_call_id: seed.tool_call_id,
                };
                let jobs = self.jobs.clone();
                let active = Arc::clone(&self.active);
                Box::pin(async move {
                    if jobs
                        .send(ToolJob {
                            seed,
                            context,
                            resolved,
                        })
                        .await
                        .is_err()
                    {
                        if let Ok(mut values) = active.lock() {
                            values.remove(&effect_id);
                        }
                        return Err(DispatchError {
                            code: "tool_job_queue_closed",
                        });
                    }
                    Ok(())
                })
            }
        }
    }
}

pub(crate) struct RuntimeDispatcher {
    model: Arc<ModelDispatcher>,
    tools: Arc<ToolDispatcher>,
}

impl RuntimeDispatcher {
    pub(crate) fn new(model: Arc<ModelDispatcher>, tools: Arc<ToolDispatcher>) -> Self {
        Self { model, tools }
    }
}

impl PostCommitDispatcher for RuntimeDispatcher {
    fn validate_before_commit(&self, input: &KernelInput) -> Result<(), DispatchError> {
        self.model.validate_before_commit(input)?;
        self.tools.validate_before_commit(input)
    }

    fn dispatch(&self, dispatch: RuntimeDispatch) -> PortFuture<Result<(), DispatchError>> {
        if matches!(dispatch.action, PostCommitAction::CancelEffect { .. }) {
            let model = Arc::clone(&self.model);
            let tools = Arc::clone(&self.tools);
            let model_dispatch = dispatch.clone();
            return Box::pin(async move {
                model.dispatch(model_dispatch).await?;
                tools.dispatch(dispatch).await
            });
        }
        if dispatch.model.is_some() {
            self.model.dispatch(dispatch)
        } else {
            self.tools.dispatch(dispatch)
        }
    }
}

struct ScheduledJob {
    job: ToolJob,
    global: OwnedSemaphorePermit,
    per_tool: OwnedSemaphorePermit,
}

struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) async fn run_tool_jobs(
    assembler: ToolStreamAssembler,
    config: ToolTaskConfig,
    active: ActiveEffects,
    per_tool: BTreeMap<ToolId, Arc<Semaphore>>,
    mut jobs: mpsc::Receiver<ToolJob>,
    results: mpsc::Sender<ToolDriverResult>,
) {
    let global = Arc::new(Semaphore::new(config.global_max_concurrency));
    let mut pending = VecDeque::new();
    let mut tasks = JoinSet::new();
    let mut intake_open = true;
    loop {
        spawn_ready(
            &mut pending,
            &mut tasks,
            &global,
            &per_tool,
            assembler,
            &active,
            &results,
        );
        if !intake_open && pending.is_empty() && tasks.is_empty() {
            break;
        }
        tokio::select! {
            job = jobs.recv(), if intake_open && pending.len() < config.job_capacity => {
                if let Some(job) = job {
                    pending.push_back(job);
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

fn spawn_ready(
    pending: &mut VecDeque<ToolJob>,
    tasks: &mut JoinSet<()>,
    global: &Arc<Semaphore>,
    per_tool: &BTreeMap<ToolId, Arc<Semaphore>>,
    assembler: ToolStreamAssembler,
    active: &ActiveEffects,
    results: &mpsc::Sender<ToolDriverResult>,
) {
    loop {
        let Ok(global_permit) = Arc::clone(global).try_acquire_owned() else {
            return;
        };
        let available = pending.iter().position(|job| {
            per_tool
                .get(&job.seed.call.tool_id)
                .is_some_and(|semaphore| semaphore.available_permits() > 0)
        });
        let Some(index) = available else {
            drop(global_permit);
            return;
        };
        let Some(job) = pending.remove(index) else {
            drop(global_permit);
            return;
        };
        let Some(semaphore) = per_tool.get(&job.seed.call.tool_id).cloned() else {
            drop(global_permit);
            continue;
        };
        let Ok(per_tool_permit) = semaphore.try_acquire_owned() else {
            pending.push_front(job);
            drop(global_permit);
            return;
        };
        let active = Arc::clone(active);
        let results = results.clone();
        tasks.spawn(run_scheduled(
            ScheduledJob {
                job,
                global: global_permit,
                per_tool: per_tool_permit,
            },
            assembler,
            active,
            results,
        ));
    }
}

async fn run_scheduled(
    scheduled: ScheduledJob,
    assembler: ToolStreamAssembler,
    active: ActiveEffects,
    results: mpsc::Sender<ToolDriverResult>,
) {
    let ScheduledJob {
        job,
        global,
        per_tool,
    } = scheduled;
    let effect_id = job.seed.requested.effect_id();
    let cancellation = job.context.run.cancellation.clone();
    if cancellation.is_cancelled() {
        if let Ok(mut values) = active.lock() {
            values.remove(&effect_id);
        }
        drop(per_tool);
        drop(global);
        let _ = results
            .send(ToolDriverResult {
                seed: job.seed,
                result: Err(ToolError::stable(
                    TOOL_CANCELLED,
                    "tool call was cancelled before execution",
                )),
            })
            .await;
        return;
    }
    let resolved = Arc::clone(&job.resolved);
    let call = job.seed.call.clone();
    let mut child = AbortOnDrop(tokio::spawn(async move {
        let stream = resolved.toolset.call(job.context, call).await?;
        assembler
            .assemble(
                stream,
                resolved.output_validator.as_deref(),
                resolved.spec.max_result_bytes,
            )
            .await
    }));
    let result = tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            child.0.abort();
            let _ = (&mut child.0).await;
            Err(ToolError::stable(TOOL_CANCELLED, "tool call was cancelled"))
        }
        joined = &mut child.0 => match joined {
            Ok(result) => result,
            Err(_) => Err(ToolError::stable(
                TOOL_PANICKED,
                "tool execution panicked at the native extension boundary",
            )),
        }
    };
    if let Ok(mut values) = active.lock() {
        values.remove(&effect_id);
    }
    drop(per_tool);
    drop(global);
    let _ = results
        .send(ToolDriverResult {
            seed: job.seed,
            result,
        })
        .await;
}
