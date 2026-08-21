//! Idempotent remote [`AgentInvoker`](finstack_ai_runtime::AgentInvoker).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    AcceptRun, BudgetPropagation, CancelRequested, CancellationInitiator, CancellationPropagation,
    ChildPlacement, ChildRunLocator, DeadlinePropagation, Digest, PrincipalPropagation,
    RunAccepted, RunId, RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext,
};
use finstack_ai_protocol::{RemoteAgentRef, RemoteCommandPayload, RemoteStartRequest};
use finstack_ai_runtime::{
    AgentInvokeError, AgentInvoker, ChildRunContext, ChildRunHandle, ChildRunRequest, PortFuture,
    child_relation_digest,
};

use crate::route::{RemoteChildRoute, invalid, parse_endpoint, route_ref, unavailable};
use crate::transport::exchange;

const MAX_ACCEPTED: usize = 1_024;

/// Remote child invoker bound to one explicit route.
pub struct RemoteChildInvoker {
    route: RemoteChildRoute,
    accepted: Arc<Mutex<BTreeMap<RunId, AcceptedChild>>>,
    command_ids: Arc<Mutex<BTreeMap<RunId, LogicalCommandIds>>>,
}

#[derive(Clone)]
struct AcceptedChild {
    request_digest: Digest,
    handle: ChildRunHandle,
}

#[derive(Clone)]
struct LogicalCommandIds {
    start: String,
    cancel: Option<String>,
}

impl RemoteChildInvoker {
    /// Construct an invoker for one explicit loopback or Unix route.
    ///
    /// Does not connect and does not read environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`AgentInvokeError::InvalidRequest`] when the endpoint, service,
    /// or route handle is invalid, or when plaintext targets a non-loopback
    /// address.
    pub fn try_new(route: RemoteChildRoute) -> Result<Self, AgentInvokeError> {
        parse_endpoint(&route.endpoint)?;
        let _ = route_ref(&route)?;
        if route
            .token
            .as_deref()
            .is_some_and(|token| token.is_empty() || token.as_bytes().contains(&0))
        {
            return Err(invalid("remote child token is invalid"));
        }
        Ok(Self {
            route,
            accepted: Arc::new(Mutex::new(BTreeMap::new())),
            command_ids: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    /// Frozen route reference recorded on a remote child locator.
    ///
    /// # Errors
    ///
    /// Returns [`AgentInvokeError::InvalidRequest`] when the service or route
    /// handle is invalid.
    pub fn route_ref(&self) -> Result<finstack_ai_kernel::RemoteRouteRef, AgentInvokeError> {
        route_ref(&self.route)
    }
}

impl AgentInvoker for RemoteChildInvoker {
    fn start_or_attach(
        &self,
        ctx: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        let route = self.route.clone();
        let accepted = Arc::clone(&self.accepted);
        let command_ids = Arc::clone(&self.command_ids);
        Box::pin(async move {
            request.validate()?;
            if request.placement != ChildPlacement::RemoteChildSession {
                return Err(invalid(
                    "remote child invoker only accepts remote_child_session",
                ));
            }
            if request.locator.remote.is_none() {
                return Err(invalid("remote child locator is missing a route"));
            }
            if let Some(existing) = lookup(&accepted, request.locator.operation.run_id)? {
                if existing.request_digest == request.request_digest {
                    return Ok(existing.handle);
                }
                return Err(AgentInvokeError::Conflict {
                    existing: existing.request_digest,
                    submitted: request.request_digest,
                });
            }
            let command_id = start_command_id(&command_ids, request.locator.operation.run_id)?;
            let payload = start_payload(&ctx, &request)?;
            let result = exchange(&route, &request.locator, payload, &command_id).await?;
            if !result.accepted() {
                return Err(unavailable(
                    result
                        .reason_code()
                        .unwrap_or("remote child start was rejected"),
                ));
            }
            let handle = ChildRunHandle {
                locator: request.locator.clone(),
                relation_digest: child_relation_digest(&ctx, &request).map_err(|error| {
                    AgentInvokeError::InvalidRequest {
                        message: Arc::from(error.to_string()),
                    }
                })?,
            };
            insert_accepted(
                &accepted,
                request.locator.operation.run_id,
                AcceptedChild {
                    request_digest: request.request_digest,
                    handle: handle.clone(),
                },
            )?;
            Ok(handle)
        })
    }

    fn cancel(&self, locator: &ChildRunLocator) -> PortFuture<Result<(), AgentInvokeError>> {
        let route = self.route.clone();
        let locator = locator.clone();
        let accepted = Arc::clone(&self.accepted);
        let command_ids = Arc::clone(&self.command_ids);
        Box::pin(async move {
            if lookup(&accepted, locator.operation.run_id)?.is_none() {
                return Err(invalid("remote child locator was never accepted"));
            }
            let command_id = cancel_command_id(&command_ids, locator.operation.run_id)?;
            let payload = RemoteCommandPayload::Cancel(Box::new(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: None,
            }));
            let result = exchange(&route, &locator, payload, &command_id).await?;
            if result.accepted() {
                Ok(())
            } else {
                Err(unavailable(
                    result
                        .reason_code()
                        .unwrap_or("remote child cancel was rejected"),
                ))
            }
        })
    }
}

fn start_payload(
    ctx: &ChildRunContext,
    request: &ChildRunRequest,
) -> Result<RemoteCommandPayload, AgentInvokeError> {
    let operation = &request.locator.operation;
    let security = RunSecurityContext::try_new(
        operation.tenant_scope.as_ref(),
        ctx.authorization.principal.clone(),
        ctx.authorization.authentication_method.as_ref(),
        ctx.authorization.assurance_level.as_ref(),
        ctx.authorization.policy_version.as_ref(),
        ctx.authorization.decision_id.as_ref(),
        None,
    )
    .map_err(|_| invalid("remote child security context is invalid"))?;
    let limits = RunLimits {
        max_input_tokens: request.requested_budget.input_tokens,
        max_output_tokens: request.requested_budget.output_tokens,
        max_cost: request.requested_budget.cost.clone(),
        extension_counters: request
            .requested_budget
            .extension_counters
            .clone()
            .into_inner(),
        ..RunLimits::empty()
    };
    let accepted = RunAccepted::try_new(
        operation.run_id,
        RunRelation::root(operation.run_id)
            .map_err(|_| invalid("remote child run relation is invalid"))?,
        security,
        request.requested_deadline,
        limits,
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        request.agent.spec_digest,
        None,
    )
    .map_err(|_| invalid("remote child acceptance is invalid"))?;
    let start = RemoteStartRequest::try_new(
        AcceptRun {
            session_id: operation.session_id,
            lane_id: operation.lane_id,
            accepted,
        },
        RemoteAgentRef {
            agent_id: request.agent.id.clone(),
            bundle_id: request.agent.bundle.clone(),
            spec_digest: request.agent.spec_digest,
        },
        request.input.as_ref().to_vec(),
        request.metadata.clone(),
        request.delegation_id.as_deref().map(str::to_owned),
    )
    .map_err(|_| invalid("remote child start payload is invalid"))?;
    Ok(RemoteCommandPayload::Start(Box::new(start)))
}

fn start_command_id(
    ids: &Mutex<BTreeMap<RunId, LogicalCommandIds>>,
    run_id: RunId,
) -> Result<String, AgentInvokeError> {
    let mut ids = ids
        .lock()
        .map_err(|_| unavailable("remote child command-id lock is poisoned"))?;
    Ok(ids
        .entry(run_id)
        .or_insert_with(|| LogicalCommandIds {
            start: uuid::Uuid::now_v7().to_string(),
            cancel: None,
        })
        .start
        .clone())
}

fn cancel_command_id(
    ids: &Mutex<BTreeMap<RunId, LogicalCommandIds>>,
    run_id: RunId,
) -> Result<String, AgentInvokeError> {
    let mut ids = ids
        .lock()
        .map_err(|_| unavailable("remote child command-id lock is poisoned"))?;
    let logical = ids.entry(run_id).or_insert_with(|| LogicalCommandIds {
        start: uuid::Uuid::now_v7().to_string(),
        cancel: None,
    });
    Ok(logical
        .cancel
        .get_or_insert_with(|| uuid::Uuid::now_v7().to_string())
        .clone())
}

fn lookup(
    accepted: &Mutex<BTreeMap<RunId, AcceptedChild>>,
    run_id: RunId,
) -> Result<Option<AcceptedChild>, AgentInvokeError> {
    let accepted = accepted
        .lock()
        .map_err(|_| unavailable("remote child acceptance lock is poisoned"))?;
    Ok(accepted.get(&run_id).cloned())
}

fn insert_accepted(
    accepted: &Mutex<BTreeMap<RunId, AcceptedChild>>,
    run_id: RunId,
    child: AcceptedChild,
) -> Result<(), AgentInvokeError> {
    let mut accepted = accepted
        .lock()
        .map_err(|_| unavailable("remote child acceptance lock is poisoned"))?;
    if accepted.len() >= MAX_ACCEPTED && !accepted.contains_key(&run_id) {
        return Err(unavailable("remote child accepted map is full"));
    }
    accepted.insert(run_id, child);
    Ok(())
}
