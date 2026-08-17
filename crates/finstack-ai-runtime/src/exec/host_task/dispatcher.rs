use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    EffectId, EffectInput, KernelInput, PostCommitAction, ReducerStageOutcome, ToolCallPlan,
};

use crate::context::{
    CONTEXT_STAGE, CommittedContextCall, ContextCallContext, ContextProvider, ContextRequest,
};
use crate::coordinator::{
    ContextDispatchSeed, DispatchError, ModelDispatchSeed, PostCommitDispatcher, RuntimeDispatch,
    ToolDispatchSeed,
};
use crate::{
    CancellationSignal, LockedModelContextProfile, Model, ModelCallContext, ModelError,
    ModelRequest, ModelRequestDraft, PortFuture, ResolvedTool, ResolvedToolCatalog, RunCallContext,
    ToolCallContext, validate_model_request,
};

use super::fault::stable_dispatch_code;
use super::shared::HostWork;

pub(super) struct HostDispatcher {
    pub(super) model: Arc<dyn Model>,
    pub(super) profile: LockedModelContextProfile,
    pub(super) catalog: Option<Arc<ResolvedToolCatalog>>,
    pub(super) context_providers: Option<Arc<[Arc<dyn ContextProvider>]>>,
    pub(super) pending: Arc<Mutex<VecDeque<HostWork>>>,
    pub(super) active: Arc<Mutex<BTreeMap<EffectId, CancellationSignal>>>,
    pub(super) parent: CancellationSignal,
}

impl HostDispatcher {
    fn enqueue(&self, work: HostWork) -> Result<(), DispatchError> {
        self.pending
            .lock()
            .map_err(|_| DispatchError {
                code: "host_work_queue_unavailable",
            })?
            .push_back(work);
        Ok(())
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

    fn resolved_for_seed(
        &self,
        seed: &ToolDispatchSeed,
    ) -> Result<Arc<ResolvedTool>, DispatchError> {
        let catalog = self.catalog.as_ref().ok_or(DispatchError {
            code: "unsupported_effect_driver",
        })?;
        let resolved = catalog
            .by_id(&seed.call.tool_id)
            .cloned()
            .ok_or(DispatchError {
                code: "tool_resolution_missing",
            })?;
        let EffectInput::Tool { call: committed } = seed.requested.input() else {
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

    pub(super) fn resume_request(&self, seed: ModelDispatchSeed) -> Result<(), DispatchError> {
        self.enqueue_model(seed.pending.requested.effect_id(), seed)
    }

    fn enqueue_model(
        &self,
        effect_id: EffectId,
        seed: ModelDispatchSeed,
    ) -> Result<(), DispatchError> {
        let EffectInput::Model { request: raw } = seed.pending.requested.input() else {
            return Err(DispatchError {
                code: "model_request_invalid",
            });
        };
        let draft = self
            .parse_and_validate(raw)
            .map_err(|error| DispatchError {
                code: stable_dispatch_code(error.code()),
            })?;
        let cancellation = self.parent.child();
        {
            let Ok(mut active) = self.active.lock() else {
                return Err(DispatchError {
                    code: "model_effect_registry_unavailable",
                });
            };
            if active.insert(effect_id, cancellation.clone()).is_some() {
                return Err(DispatchError {
                    code: "model_effect_already_active",
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
            continuation_state: seed.continuation_state.clone(),
        };
        self.enqueue(HostWork::Model { seed, request })
    }

    pub(super) fn resume_call(&self, seed: ToolDispatchSeed) -> Result<(), DispatchError> {
        self.enqueue_tool(seed.requested.effect_id(), seed)
    }

    fn dispatch_model(
        &self,
        effect_id: EffectId,
        seed: ModelDispatchSeed,
    ) -> PortFuture<Result<(), DispatchError>> {
        let result = self.enqueue_model(effect_id, seed);
        Box::pin(async move { result })
    }

    fn enqueue_tool(
        &self,
        effect_id: EffectId,
        seed: ToolDispatchSeed,
    ) -> Result<(), DispatchError> {
        let resolved = self.resolved_for_seed(&seed)?;
        let cancellation = self.parent.child();
        {
            let Ok(mut active) = self.active.lock() else {
                return Err(DispatchError {
                    code: "tool_effect_registry_unavailable",
                });
            };
            if active.insert(effect_id, cancellation.clone()).is_some() {
                return Err(DispatchError {
                    code: "tool_effect_already_active",
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
        self.enqueue(HostWork::Tool {
            seed,
            context,
            resolved,
        })
    }

    fn dispatch_tool(
        &self,
        effect_id: EffectId,
        seed: ToolDispatchSeed,
    ) -> PortFuture<Result<(), DispatchError>> {
        let result = self.enqueue_tool(effect_id, seed);
        Box::pin(async move { result })
    }

    fn dispatch_context(
        &self,
        effect_id: EffectId,
        seed: ContextDispatchSeed,
    ) -> PortFuture<Result<(), DispatchError>> {
        let providers = self.context_providers.clone();
        let parent = self.parent.clone();
        Box::pin(async move {
            let providers = providers.ok_or(DispatchError {
                code: "unsupported_effect_driver",
            })?;
            let pipeline = seed.requested.pipeline().ok_or(DispatchError {
                code: "unsupported_effect_driver",
            })?;
            if pipeline.stage() != CONTEXT_STAGE {
                return Err(DispatchError {
                    code: "unsupported_effect_driver",
                });
            }
            let index = usize::try_from(pipeline.index()).map_err(|_| DispatchError {
                code: "unsupported_effect_driver",
            })?;
            let provider = providers.get(index).ok_or(DispatchError {
                code: "unsupported_effect_driver",
            })?;
            let EffectInput::Context { request: raw } = seed.requested.input() else {
                return Err(DispatchError {
                    code: "unsupported_effect_driver",
                });
            };
            let request: ContextRequest =
                serde_json::from_slice(raw.as_bytes()).map_err(|_| DispatchError {
                    code: "unsupported_effect_driver",
                })?;
            let context = ContextCallContext {
                run: RunCallContext {
                    locator: seed.locator,
                    authorization: seed.authorization,
                    effect_id,
                    attempt: seed.attempt,
                    deadline: seed.requested.deadline(),
                    budget_scope_id: seed.budget_scope_id,
                    cancellation: parent.child(),
                },
                provider_index: pipeline.index(),
                chain_digest: pipeline.chain_digest(),
            };
            CommittedContextCall::try_new(&seed.envelope, context, request, &provider.descriptor())
                .map_err(|_| DispatchError {
                    code: "unsupported_effect_driver",
                })?
                .invoke(provider.as_ref())
                .await
                .map(|_| ())
                .map_err(|_| DispatchError {
                    code: "unsupported_effect_driver",
                })
        })
    }
}

impl PostCommitDispatcher for HostDispatcher {
    fn validate_before_commit(&self, input: &KernelInput) -> Result<(), DispatchError> {
        if let KernelInput::StageSettled(settled) = input {
            if let ReducerStageOutcome::ModelRequestPrepared { request, .. } = &settled.outcome {
                self.parse_and_validate(request)
                    .map(|_| ())
                    .map_err(|error| DispatchError {
                        code: stable_dispatch_code(error.code()),
                    })?;
            }
            if let ReducerStageOutcome::ToolBatchPrepared { calls, .. } = &settled.outcome {
                let catalog = self.catalog.as_ref().ok_or(DispatchError {
                    code: "unsupported_effect_driver",
                })?;
                for plan in calls.iter() {
                    let ToolCallPlan::Execute(call) = plan else {
                        continue;
                    };
                    let resolved = catalog.by_id(&call.tool_id).ok_or(DispatchError {
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
            }
        }
        Ok(())
    }

    fn dispatch(&self, dispatch: RuntimeDispatch) -> PortFuture<Result<(), DispatchError>> {
        match dispatch.action {
            PostCommitAction::CancelEffect { effect_id } => {
                match crate::coordinator::cancel_registered_effect(
                    self.active.as_ref(),
                    effect_id,
                    "host_effect_registry_unavailable",
                ) {
                    Ok(Some(signal)) => signal.cancel(),
                    Ok(None) => {}
                    Err(error) => return Box::pin(async move { Err(error) }),
                }
                Box::pin(async { Ok(()) })
            }
            PostCommitAction::ExecuteEffect { effect_id } => {
                if let Some(seed) = dispatch.model {
                    return self.dispatch_model(effect_id, seed);
                }
                if let Some(seed) = dispatch.tool {
                    return self.dispatch_tool(effect_id, seed);
                }
                if let Some(seed) = dispatch.context {
                    return self.dispatch_context(effect_id, seed);
                }
                Box::pin(async {
                    Err(DispatchError {
                        code: "unsupported_effect_driver",
                    })
                })
            }
        }
    }
}
