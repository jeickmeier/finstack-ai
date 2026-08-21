//! Idempotent remote [`AgentInvoker`](finstack_ai_runtime::AgentInvoker).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    AcceptRun, BudgetPropagation, CancelRequested, CancellationInitiator, CancellationPropagation,
    ChildPlacement, ChildRunLocator, DeadlinePropagation, Digest, PrincipalPropagation,
    RunAccepted, RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext,
};
use finstack_ai_protocol::{RemoteAgentRef, RemoteCommandPayload, RemoteStartRequest};
use finstack_ai_runtime::{
    AgentInvokeError, AgentInvoker, ChildRunContext, ChildRunHandle, ChildRunRequest, PortFuture,
    child_relation_digest,
};

use crate::route::{RemoteChildRoute, ResolvedRemoteRoute, invalid, resolve_route, unavailable};
use crate::transport::exchange;

const MAX_ACCEPTED: usize = 1_024;

/// Remote child invoker bound to one explicit route.
pub struct RemoteChildInvoker {
    route: ResolvedRemoteRoute,
    entries: Arc<Mutex<BTreeMap<Digest, ChildEntry>>>,
}

#[derive(Clone)]
struct AcceptedChild {
    request_digest: Digest,
    handle: ChildRunHandle,
}

#[derive(Clone)]
enum ChildEntry {
    Pending {
        request_digest: Digest,
        shared: Arc<PendingResult>,
    },
    Accepted(AcceptedChild),
}

struct PendingResult {
    result: Mutex<Option<Result<ChildRunHandle, AgentInvokeError>>>,
    notify: tokio::sync::Notify,
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
        Ok(Self {
            route: resolve_route(route)?,
            entries: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    /// Frozen route reference recorded on a remote child locator.
    ///
    /// # Errors
    ///
    /// Returns [`AgentInvokeError::InvalidRequest`] when the service or route
    /// handle is invalid.
    pub fn route_ref(&self) -> Result<finstack_ai_kernel::RemoteRouteRef, AgentInvokeError> {
        Ok(self.route.reference.clone())
    }
}

impl AgentInvoker for RemoteChildInvoker {
    fn start_or_attach(
        &self,
        ctx: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        let route = self.route.clone();
        let entries = Arc::clone(&self.entries);
        Box::pin(async move {
            request.validate()?;
            if request.placement != ChildPlacement::RemoteChildSession {
                return Err(invalid(
                    "remote child invoker only accepts remote_child_session",
                ));
            }
            if request.locator.remote.as_ref() != Some(&route.reference) {
                return Err(invalid("remote child locator route does not match"));
            }
            let key = locator_key(&request.locator)?;
            let payload = start_payload(&ctx, &request)?;
            let handle = ChildRunHandle {
                locator: request.locator.clone(),
                relation_digest: child_relation_digest(&ctx, &request).map_err(|error| {
                    AgentInvokeError::InvalidRequest {
                        message: Arc::from(error.to_string()),
                    }
                })?,
            };
            let (shared, initiator) = reserve_entry(&entries, key, &request)?;
            if initiator {
                let command_id =
                    deterministic_command_id("remote-child-start", request.request_digest)?;
                let worker_entries = Arc::clone(&entries);
                let worker_shared = Arc::clone(&shared);
                let locator = request.locator.clone();
                let request_digest = request.request_digest;
                let worker_handle = handle.clone();
                tokio::spawn(async move {
                    let mut result = async {
                        let result = exchange(&route, &locator, payload, &command_id).await?;
                        if !result.accepted() {
                            return Err(unavailable(
                                result
                                    .reason_code()
                                    .unwrap_or("remote child start was rejected"),
                            ));
                        }
                        Ok(worker_handle.clone())
                    }
                    .await;
                    match worker_entries.lock() {
                        Ok(mut entries) if result.is_ok() => {
                            entries.insert(
                                key,
                                ChildEntry::Accepted(AcceptedChild {
                                    request_digest,
                                    handle: worker_handle,
                                }),
                            );
                        }
                        Ok(mut entries) => {
                            entries.remove(&key);
                        }
                        Err(_) => {
                            result = Err(unavailable("remote child entry lock is poisoned"));
                        }
                    }
                    if let Ok(mut slot) = worker_shared.result.lock() {
                        *slot = Some(result);
                    }
                    worker_shared.notify.notify_waiters();
                });
            }
            wait_pending(shared).await
        })
    }

    fn cancel(&self, locator: &ChildRunLocator) -> PortFuture<Result<(), AgentInvokeError>> {
        let route = self.route.clone();
        let locator = locator.clone();
        let entries = Arc::clone(&self.entries);
        Box::pin(async move {
            if locator.remote.as_ref() != Some(&route.reference) {
                return Err(invalid("remote child locator route does not match"));
            }
            let key = locator_key(&locator)?;
            let accepted = lookup_accepted(&entries, key, &locator)?;
            let command_id =
                deterministic_command_id("remote-child-cancel", accepted.request_digest)?;
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

fn locator_key(locator: &ChildRunLocator) -> Result<Digest, AgentInvokeError> {
    let canonical = serde_json_canonicalizer::to_vec(locator)
        .map_err(|_| invalid("remote child locator is not serializable"))?;
    Digest::domain_separated("remote-child-locator", 1, &canonical)
        .map_err(|_| invalid("remote child locator digest failed"))
}

fn deterministic_command_id(domain: &str, digest: Digest) -> Result<String, AgentInvokeError> {
    let derived = Digest::domain_separated(domain, 1, digest.as_bytes())
        .map_err(|_| invalid("remote child command identity failed"))?;
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&derived.as_bytes()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(uuid::Uuid::from_bytes(bytes).to_string())
}

fn reserve_entry(
    entries: &Mutex<BTreeMap<Digest, ChildEntry>>,
    key: Digest,
    request: &ChildRunRequest,
) -> Result<(Arc<PendingResult>, bool), AgentInvokeError> {
    let mut entries = entries
        .lock()
        .map_err(|_| unavailable("remote child entry lock is poisoned"))?;
    match entries.get(&key) {
        Some(ChildEntry::Accepted(existing)) => {
            if existing.handle.locator != request.locator {
                return Err(invalid("remote child locator digest collision"));
            }
            if existing.request_digest != request.request_digest {
                return Err(AgentInvokeError::Conflict {
                    existing: existing.request_digest,
                    submitted: request.request_digest,
                });
            }
            let shared = Arc::new(PendingResult {
                result: Mutex::new(Some(Ok(existing.handle.clone()))),
                notify: tokio::sync::Notify::new(),
            });
            Ok((shared, false))
        }
        Some(ChildEntry::Pending {
            request_digest,
            shared,
        }) => {
            if *request_digest != request.request_digest {
                return Err(AgentInvokeError::Conflict {
                    existing: *request_digest,
                    submitted: request.request_digest,
                });
            }
            Ok((Arc::clone(shared), false))
        }
        None => {
            if entries.len() >= MAX_ACCEPTED {
                return Err(unavailable("remote child entry map is full"));
            }
            let shared = Arc::new(PendingResult {
                result: Mutex::new(None),
                notify: tokio::sync::Notify::new(),
            });
            entries.insert(
                key,
                ChildEntry::Pending {
                    request_digest: request.request_digest,
                    shared: Arc::clone(&shared),
                },
            );
            Ok((shared, true))
        }
    }
}

async fn wait_pending(shared: Arc<PendingResult>) -> Result<ChildRunHandle, AgentInvokeError> {
    loop {
        let notified = shared.notify.notified();
        if let Some(result) = shared
            .result
            .lock()
            .map_err(|_| unavailable("remote child pending result is poisoned"))?
            .clone()
        {
            return result;
        }
        notified.await;
    }
}

fn lookup_accepted(
    entries: &Mutex<BTreeMap<Digest, ChildEntry>>,
    key: Digest,
    locator: &ChildRunLocator,
) -> Result<AcceptedChild, AgentInvokeError> {
    let entries = entries
        .lock()
        .map_err(|_| unavailable("remote child entry lock is poisoned"))?;
    match entries.get(&key) {
        Some(ChildEntry::Accepted(accepted)) if accepted.handle.locator == *locator => {
            Ok(accepted.clone())
        }
        _ => Err(invalid("remote child locator was never accepted")),
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
