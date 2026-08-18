//! Private bounded native Toolset job/result scheduler.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    EffectId, KernelInput, PostCommitAction, ReducerStageOutcome, ToolCallPlan, ToolId,
    ValidatedToolCall,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;

use crate::coordinator::{DispatchError, PostCommitDispatcher, RuntimeDispatch, ToolDispatchSeed};
use crate::native::model::ModelDispatcher;
use crate::settlement::ToolDriverResult;
use crate::tool::AssembledToolTerminal;
use crate::{
    CancellationSignal, Clock, MonotonicDeadline, PortFuture, ResolvedTool, ResolvedToolCatalog,
    RunCallContext, TOOL_CANCELLED, TOOL_DEADLINE_EXCEEDED, TOOL_PANICKED, ToolCallContext,
    ToolError, ToolProgress, ToolStreamAssembler,
};

pub use crate::run_types::ToolTaskConfig;

pub(crate) struct ToolJob {
    pub(crate) seed: ToolDispatchSeed,
    pub(crate) context: ToolCallContext,
    pub(crate) resolved: Arc<ResolvedTool>,
}

pub(crate) enum ToolDriverMessage {
    Progress {
        effect_id: EffectId,
        progress: ToolProgress,
    },
    Terminal(Box<ToolDriverResult>),
}

type ActiveEffects = Arc<Mutex<BTreeMap<EffectId, CancellationSignal>>>;

pub(crate) struct ToolExecutionContext<C> {
    assembler: ToolStreamAssembler,
    active: ActiveEffects,
    results: mpsc::Sender<ToolDriverMessage>,
    clock: Arc<C>,
    cancellation_grace: Duration,
}

impl<C> ToolExecutionContext<C> {
    pub(crate) fn new(
        assembler: ToolStreamAssembler,
        active: ActiveEffects,
        results: mpsc::Sender<ToolDriverMessage>,
        clock: Arc<C>,
        cancellation_grace: Duration,
    ) -> Self {
        Self {
            assembler,
            active,
            results,
            clock,
            cancellation_grace,
        }
    }
}

impl<C> Clone for ToolExecutionContext<C> {
    fn clone(&self) -> Self {
        Self {
            assembler: self.assembler,
            active: Arc::clone(&self.active),
            results: self.results.clone(),
            clock: Arc::clone(&self.clock),
            cancellation_grace: self.cancellation_grace,
        }
    }
}

pub(crate) struct ToolDispatcher {
    catalog: Arc<ResolvedToolCatalog>,
    jobs: mpsc::Sender<ToolJob>,
    active: ActiveEffects,
    per_tool: BTreeMap<ToolId, Arc<Semaphore>>,
    parent: CancellationSignal,
}

impl ToolDispatcher {
    pub(crate) fn new(
        catalog: Arc<ResolvedToolCatalog>,
        jobs: mpsc::Sender<ToolJob>,
        parent: CancellationSignal,
    ) -> Self {
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
            parent,
        }
    }

    pub(crate) fn active(&self) -> ActiveEffects {
        Arc::clone(&self.active)
    }

    pub(crate) fn semaphores(&self) -> BTreeMap<ToolId, Arc<Semaphore>> {
        self.per_tool.clone()
    }

    pub(crate) async fn resume_call(&self, seed: ToolDispatchSeed) -> Result<(), DispatchError> {
        let effect_id = seed.requested.effect_id();
        self.dispatch(RuntimeDispatch {
            action: finstack_ai_kernel::PostCommitAction::ExecuteEffect { effect_id },
            model: None,
            tool: Some(seed),
            timer: None,
            context: None,
        })
        .await
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
                let cancellation = match crate::coordinator::cancel_registered_effect(
                    &self.active,
                    effect_id,
                    "tool_effect_registry_unavailable",
                ) {
                    Ok(cancellation) => cancellation,
                    Err(error) => return Box::pin(async move { Err(error) }),
                };
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
                let cancellation = self.parent.child();
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
    tools: Option<Arc<ToolDispatcher>>,
    timers: Arc<dyn PostCommitDispatcher>,
}

impl RuntimeDispatcher {
    pub(crate) fn model_only(
        model: Arc<ModelDispatcher>,
        timers: Arc<dyn PostCommitDispatcher>,
    ) -> Self {
        Self {
            model,
            tools: None,
            timers,
        }
    }

    pub(crate) fn with_tools(
        model: Arc<ModelDispatcher>,
        tools: Arc<ToolDispatcher>,
        timers: Arc<dyn PostCommitDispatcher>,
    ) -> Self {
        Self {
            model,
            tools: Some(tools),
            timers,
        }
    }
}

impl PostCommitDispatcher for RuntimeDispatcher {
    fn validate_before_commit(&self, input: &KernelInput) -> Result<(), DispatchError> {
        self.model.validate_before_commit(input)?;
        if let Some(tools) = &self.tools {
            tools.validate_before_commit(input)?;
        }
        Ok(())
    }

    fn dispatch(&self, dispatch: RuntimeDispatch) -> PortFuture<Result<(), DispatchError>> {
        if matches!(dispatch.action, PostCommitAction::CancelEffect { .. }) {
            let model = Arc::clone(&self.model);
            let tools = self.tools.clone();
            let timers = Arc::clone(&self.timers);
            let model_dispatch = dispatch.clone();
            let tool_dispatch = dispatch.clone();
            return Box::pin(async move {
                model.dispatch(model_dispatch).await?;
                if let Some(tools) = tools {
                    tools.dispatch(tool_dispatch).await?;
                }
                timers.dispatch(dispatch).await
            });
        }
        if dispatch.timer.is_some() {
            return self.timers.dispatch(dispatch);
        }
        if dispatch.model.is_some() {
            self.model.dispatch(dispatch)
        } else if dispatch.tool.is_some() {
            if let Some(tools) = &self.tools {
                tools.dispatch(dispatch)
            } else {
                Box::pin(async {
                    Err(DispatchError {
                        code: "unsupported_effect_driver",
                    })
                })
            }
        } else if dispatch.context.is_some() {
            Box::pin(async { Ok(()) })
        } else {
            Box::pin(async {
                Err(DispatchError {
                    code: "unsupported_effect_driver",
                })
            })
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

pub(crate) async fn run_tool_jobs<C>(
    config: ToolTaskConfig,
    per_tool: BTreeMap<ToolId, Arc<Semaphore>>,
    mut jobs: mpsc::Receiver<ToolJob>,
    context: ToolExecutionContext<C>,
) where
    C: Clock + Send + Sync + 'static,
{
    let global = Arc::new(Semaphore::new(config.global_max_concurrency));
    let mut pending = VecDeque::new();
    let mut tasks = JoinSet::new();
    let mut intake_open = true;
    loop {
        spawn_ready(&mut pending, &mut tasks, &global, &per_tool, &context);
        if !intake_open && pending.is_empty() && tasks.is_empty() {
            break;
        }
        tokio::select! {
            job = jobs.recv(), if intake_open && pending.len() < config.job_capacity => {
                if let Some(job) = job {
                    pending.push_back(job);
                } else {
                    intake_open = false;
                    if let Ok(values) = context.active.lock() {
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

fn spawn_ready<C>(
    pending: &mut VecDeque<ToolJob>,
    tasks: &mut JoinSet<()>,
    global: &Arc<Semaphore>,
    per_tool: &BTreeMap<ToolId, Arc<Semaphore>>,
    context: &ToolExecutionContext<C>,
) where
    C: Clock + Send + Sync + 'static,
{
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
        tasks.spawn(run_scheduled(
            ScheduledJob {
                job,
                global: global_permit,
                per_tool: per_tool_permit,
            },
            context.clone(),
        ));
    }
}

async fn run_scheduled<C>(scheduled: ScheduledJob, context: ToolExecutionContext<C>)
where
    C: Clock + Send + Sync + 'static,
{
    let ScheduledJob {
        job,
        global,
        per_tool,
    } = scheduled;
    let effect_id = job.seed.requested.effect_id();
    let cancellation = job.context.run.cancellation.clone();
    let resolved = Arc::clone(&job.resolved);
    let call = job.seed.call.clone();
    let assembler = context.assembler;
    let cancellation_grace = context.cancellation_grace;
    let progress_results = context.results.clone();
    let deadline = job
        .context
        .run
        .deadline
        .map(|deadline| {
            MonotonicDeadline::from_persisted(
                context.clock.as_ref(),
                job.seed.requested_at,
                deadline,
            )
        })
        .transpose();
    let deadline = match tool_job_preflight(deadline, &cancellation) {
        Ok(deadline) => deadline,
        Err(error) => {
            remove_active(&context.active, effect_id);
            drop(per_tool);
            drop(global);
            let _ = context
                .results
                .send(ToolDriverMessage::Terminal(Box::new(ToolDriverResult {
                    seed: job.seed,
                    result: Err(error),
                })))
                .await;
            return;
        }
    };
    let mut child = AbortOnDrop(tokio::spawn(async move {
        execute_tool(
            resolved,
            job.context,
            call,
            assembler,
            progress_results,
            effect_id,
        )
        .await
    }));
    let result = if let Some(deadline) = deadline {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                settle_tool_cancellation(&mut child.0, cancellation_grace).await
            }
            () = deadline.wait() => {
                cancellation.cancel();
                child.0.abort();
                let _ = (&mut child.0).await;
                Err(ToolError::stable(
                    TOOL_DEADLINE_EXCEEDED,
                    "tool call exceeded its committed deadline",
                ))
            }
            joined = &mut child.0 => joined_tool_result(joined),
        }
    } else {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
            settle_tool_cancellation(&mut child.0, cancellation_grace).await
            }
            joined = &mut child.0 => joined_tool_result(joined),
        }
    };
    remove_active(&context.active, effect_id);
    drop(per_tool);
    drop(global);
    let _ = context
        .results
        .send(ToolDriverMessage::Terminal(Box::new(ToolDriverResult {
            seed: job.seed,
            result,
        })))
        .await;
}

fn tool_job_preflight(
    deadline: Result<Option<MonotonicDeadline>, crate::RuntimeTimeError>,
    cancellation: &CancellationSignal,
) -> Result<Option<MonotonicDeadline>, ToolError> {
    if cancellation.is_cancelled() {
        return Err(ToolError::stable(
            TOOL_CANCELLED,
            "tool call was cancelled before execution",
        ));
    }
    let deadline = deadline.map_err(|_| {
        ToolError::stable(
            TOOL_DEADLINE_EXCEEDED,
            "tool call deadline configuration is invalid",
        )
    })?;
    if deadline
        .as_ref()
        .is_some_and(|deadline| deadline.remaining() == finstack_ai_kernel::Duration::ZERO)
    {
        cancellation.cancel();
        return Err(ToolError::stable(
            TOOL_DEADLINE_EXCEEDED,
            "tool call exceeded its committed deadline",
        ));
    }
    Ok(deadline)
}

fn remove_active(active: &ActiveEffects, effect_id: EffectId) {
    if let Ok(mut values) = active.lock() {
        values.remove(&effect_id);
    }
}

async fn execute_tool(
    resolved: Arc<ResolvedTool>,
    context: ToolCallContext,
    call: ValidatedToolCall,
    assembler: ToolStreamAssembler,
    progress_results: mpsc::Sender<ToolDriverMessage>,
    effect_id: EffectId,
) -> Result<AssembledToolTerminal, ToolError> {
    let stream = resolved.toolset.call(context, call).await?;
    assembler
        .assemble_incremental(
            stream,
            resolved.output_validator.as_deref(),
            resolved.spec.max_result_bytes,
            resolved.spec.deferral,
            move |progress| {
                let sender = progress_results.clone();
                async move {
                    sender
                        .send(ToolDriverMessage::Progress {
                            effect_id,
                            progress,
                        })
                        .await
                        .map_err(|_| {
                            ToolError::stable(
                                crate::TOOL_STREAM_INVALID,
                                "runtime tool progress path closed",
                            )
                        })
                }
            },
        )
        .await
}

async fn settle_tool_cancellation(
    child: &mut JoinHandle<Result<AssembledToolTerminal, ToolError>>,
    grace: Duration,
) -> Result<AssembledToolTerminal, ToolError> {
    if let Ok(joined) = timeout(grace, &mut *child).await {
        return joined_tool_result(joined);
    }
    child.abort();
    let _ = child.await;
    Err(ToolError::stable(
        TOOL_CANCELLED,
        "tool call exceeded its cancellation grace period",
    ))
}

fn joined_tool_result(
    joined: Result<Result<AssembledToolTerminal, ToolError>, tokio::task::JoinError>,
) -> Result<AssembledToolTerminal, ToolError> {
    match joined {
        Ok(result) => result,
        Err(_) => Err(ToolError::stable(
            TOOL_PANICKED,
            "tool execution panicked at the native extension boundary",
        )),
    }
}
