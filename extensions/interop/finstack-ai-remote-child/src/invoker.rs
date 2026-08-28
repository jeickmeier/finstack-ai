//! Idempotent remote [`AgentInvoker`](finstack_ai_runtime::child::AgentInvoker).

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    AcceptRun, BudgetPropagation, CancelRequested, CancellationInitiator, CancellationPropagation,
    ChildPlacement, ChildRunLocator, DeadlinePropagation, Digest, PrincipalPropagation,
    RunAccepted, RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext,
};
use finstack_ai_protocol::{RemoteAgentRef, RemoteCommandPayload, RemoteStartRequest};
use finstack_ai_runtime::child::{
    AgentInvokeError, AgentInvoker, ChildRunContext, ChildRunHandle, ChildRunRequest,
    ChildRunStatus, child_relation_digest,
};
use finstack_ai_runtime::ports::PortFuture;

use crate::route::{RemoteChildRoute, ResolvedRemoteRoute, invalid, resolve_route, unavailable};
use crate::transport::exchange;

const MAX_ACTIVE_CHILDREN: usize = 1_024;
const MAX_TERMINAL_CHILDREN: usize = 1_024;

/// Remote child invoker bound to one explicit route.
pub struct RemoteChildInvoker {
    route: ResolvedRemoteRoute,
    state: Arc<Mutex<ChildState>>,
}

#[derive(Clone)]
struct AcceptedChild {
    request_digest: Digest,
    handle: ChildRunHandle,
}

#[derive(Clone)]
struct TerminalChild {
    accepted: AcceptedChild,
    status: Option<ChildRunStatus>,
}

#[derive(Clone)]
enum ChildEntry {
    Pending {
        request_digest: Digest,
        shared: Arc<PendingResult>,
    },
    Accepted(AcceptedChild),
    Cancelling {
        accepted: AcceptedChild,
        shared: Arc<PendingCancelResult>,
    },
}

#[derive(Default)]
struct ChildState {
    active: BTreeMap<Digest, ChildEntry>,
    terminal: BTreeMap<Digest, TerminalChild>,
    terminal_order: VecDeque<Digest>,
}

struct PendingResult {
    result: Mutex<Option<Result<ChildRunHandle, AgentInvokeError>>>,
    notify: tokio::sync::Notify,
}

struct PendingCancelResult {
    result: Mutex<Option<Result<(), AgentInvokeError>>>,
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
            state: Arc::new(Mutex::new(ChildState::default())),
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
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            request.validate()?;
            if request.placement() != ChildPlacement::RemoteChildSession {
                return Err(invalid(
                    "remote child invoker only accepts remote_child_session",
                ));
            }
            if request.locator().remote.as_ref() != Some(&route.reference) {
                return Err(invalid("remote child locator route does not match"));
            }
            let key = locator_key(request.locator())?;
            let payload = start_payload(&ctx, &request)?;
            let handle = ChildRunHandle {
                locator: request.locator().clone(),
                relation_digest: child_relation_digest(&ctx, &request).map_err(|error| {
                    AgentInvokeError::InvalidRequest {
                        message: Arc::from(error.to_string()),
                    }
                })?,
            };
            let (shared, initiator) = reserve_entry(&state, key, &request)?;
            if initiator {
                let command_id =
                    deterministic_command_id("remote-child-start", request.request_digest())?;
                let locator = request.locator().clone();
                let request_digest = request.request_digest();
                let worker_handle = handle.clone();
                let worker_shared = Arc::clone(&shared);
                let worker_state = Arc::clone(&state);
                tokio::spawn(async move {
                    let mut completion =
                        PendingCompletion::new(worker_state, key, Arc::clone(&worker_shared));
                    let result = async {
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
                    let accepted = result.as_ref().ok().map(|_| AcceptedChild {
                        request_digest,
                        handle: worker_handle,
                    });
                    completion.publish(result, accepted);
                });
            }
            wait_pending(shared).await
        })
    }

    fn cancel(&self, locator: &ChildRunLocator) -> PortFuture<Result<(), AgentInvokeError>> {
        let route = self.route.clone();
        let locator = locator.clone();
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            if locator.remote.as_ref() != Some(&route.reference) {
                return Err(invalid("remote child locator route does not match"));
            }
            let key = locator_key(&locator)?;
            let (shared, initiator, accepted) = reserve_cancel(&state, key, &locator)?;
            if initiator {
                let command_id =
                    deterministic_command_id("remote-child-cancel", accepted.request_digest)?;
                let payload = RemoteCommandPayload::Cancel(Box::new(CancelRequested {
                    initiator: CancellationInitiator::RuntimeShutdown,
                    reason: None,
                }));
                let worker_shared = Arc::clone(&shared);
                let worker_state = Arc::clone(&state);
                tokio::spawn(async move {
                    let mut completion = PendingCancelCompletion::new(
                        worker_state,
                        key,
                        accepted,
                        Arc::clone(&worker_shared),
                    );
                    let result = exchange(&route, &locator, payload, &command_id).await;
                    let (result, status) = match result {
                        Ok(result) if result.accepted() => {
                            (Ok(()), Some(ChildRunStatus::Cancelled))
                        }
                        Ok(result) if result.reason_code() == Some("run_terminal") => {
                            (Ok(()), None)
                        }
                        Ok(result) => (
                            Err(unavailable(
                                result
                                    .reason_code()
                                    .unwrap_or("remote child cancel was rejected"),
                            )),
                            None,
                        ),
                        Err(error) => (Err(error), None),
                    };
                    completion.publish(result, status);
                });
            }
            wait_pending_cancel(shared).await
        })
    }

    fn status(
        &self,
        locator: &ChildRunLocator,
    ) -> PortFuture<Result<ChildRunStatus, AgentInvokeError>> {
        let route = self.route.clone();
        let locator = locator.clone();
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            if locator.remote.as_ref() != Some(&route.reference) {
                return Err(invalid("remote child locator route does not match"));
            }
            let key = locator_key(&locator)?;
            lookup_status(&state, key, &locator)
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

struct PendingCompletion {
    state: Arc<Mutex<ChildState>>,
    key: Digest,
    shared: Arc<PendingResult>,
    armed: bool,
}

impl PendingCompletion {
    fn new(state: Arc<Mutex<ChildState>>, key: Digest, shared: Arc<PendingResult>) -> Self {
        Self {
            state,
            key,
            shared,
            armed: true,
        }
    }

    fn publish(
        &mut self,
        mut result: Result<ChildRunHandle, AgentInvokeError>,
        accepted: Option<AcceptedChild>,
    ) {
        match self.state.lock() {
            Ok(mut state) => {
                let owns_pending = matches!(
                    state.active.get(&self.key),
                    Some(ChildEntry::Pending { shared, .. })
                        if Arc::ptr_eq(shared, &self.shared)
                );
                if !owns_pending {
                    result = Err(unavailable("remote child pending entry was replaced"));
                } else if let Some(accepted) = accepted {
                    state
                        .active
                        .insert(self.key, ChildEntry::Accepted(accepted));
                } else {
                    state.active.remove(&self.key);
                }
            }
            Err(_) => {
                result = Err(unavailable("remote child entry lock is poisoned"));
            }
        }
        if let Ok(mut slot) = self.shared.result.lock() {
            *slot = Some(result);
        }
        self.shared.notify.notify_waiters();
        self.armed = false;
    }
}

impl Drop for PendingCompletion {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(mut state) = self.state.lock() {
            let owns_pending = matches!(
                state.active.get(&self.key),
                Some(ChildEntry::Pending { shared, .. })
                    if Arc::ptr_eq(shared, &self.shared)
            );
            if owns_pending {
                state.active.remove(&self.key);
            }
        }
        if let Ok(mut slot) = self.shared.result.lock()
            && slot.is_none()
        {
            *slot = Some(Err(unavailable(
                "remote child start worker terminated before publishing a result",
            )));
        }
        self.shared.notify.notify_waiters();
    }
}

fn reserve_entry(
    state: &Mutex<ChildState>,
    key: Digest,
    request: &ChildRunRequest,
) -> Result<(Arc<PendingResult>, bool), AgentInvokeError> {
    let mut state = state
        .lock()
        .map_err(|_| unavailable("remote child entry lock is poisoned"))?;
    if let Some(entry) = state.active.get(&key) {
        return match entry {
            ChildEntry::Accepted(existing)
            | ChildEntry::Cancelling {
                accepted: existing, ..
            } => completed_entry(existing, request),
            ChildEntry::Pending {
                request_digest,
                shared,
            } => {
                if *request_digest == request.request_digest() {
                    Ok((Arc::clone(shared), false))
                } else {
                    Err(AgentInvokeError::Conflict {
                        existing: *request_digest,
                        submitted: request.request_digest(),
                    })
                }
            }
        };
    }
    if let Some(existing) = state.terminal.get(&key) {
        return completed_entry(&existing.accepted, request);
    }
    if state.active.len() >= MAX_ACTIVE_CHILDREN {
        return Err(unavailable("remote child active entry map is full"));
    }
    let shared = Arc::new(PendingResult {
        result: Mutex::new(None),
        notify: tokio::sync::Notify::new(),
    });
    state.active.insert(
        key,
        ChildEntry::Pending {
            request_digest: request.request_digest(),
            shared: Arc::clone(&shared),
        },
    );
    Ok((shared, true))
}

fn completed_entry(
    existing: &AcceptedChild,
    request: &ChildRunRequest,
) -> Result<(Arc<PendingResult>, bool), AgentInvokeError> {
    if &existing.handle.locator != request.locator() {
        return Err(invalid("remote child locator digest collision"));
    }
    if existing.request_digest != request.request_digest() {
        return Err(AgentInvokeError::Conflict {
            existing: existing.request_digest,
            submitted: request.request_digest(),
        });
    }
    Ok((
        Arc::new(PendingResult {
            result: Mutex::new(Some(Ok(existing.handle.clone()))),
            notify: tokio::sync::Notify::new(),
        }),
        false,
    ))
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

struct PendingCancelCompletion {
    state: Arc<Mutex<ChildState>>,
    key: Digest,
    accepted: AcceptedChild,
    shared: Arc<PendingCancelResult>,
    armed: bool,
}

impl PendingCancelCompletion {
    fn new(
        state: Arc<Mutex<ChildState>>,
        key: Digest,
        accepted: AcceptedChild,
        shared: Arc<PendingCancelResult>,
    ) -> Self {
        Self {
            state,
            key,
            accepted,
            shared,
            armed: true,
        }
    }

    fn publish(
        &mut self,
        mut result: Result<(), AgentInvokeError>,
        status: Option<ChildRunStatus>,
    ) {
        match self.state.lock() {
            Ok(mut state) => {
                let owns_pending = matches!(
                    state.active.get(&self.key),
                    Some(ChildEntry::Cancelling { accepted, shared })
                        if accepted.request_digest == self.accepted.request_digest
                            && accepted.handle.locator == self.accepted.handle.locator
                            && Arc::ptr_eq(shared, &self.shared)
                );
                if !owns_pending {
                    result = Err(unavailable("remote child cancellation entry was replaced"));
                } else if result.is_ok() {
                    state.active.remove(&self.key);
                    insert_terminal(
                        &mut state,
                        self.key,
                        TerminalChild {
                            accepted: self.accepted.clone(),
                            status,
                        },
                    );
                } else {
                    state
                        .active
                        .insert(self.key, ChildEntry::Accepted(self.accepted.clone()));
                }
            }
            Err(_) => {
                result = Err(unavailable("remote child entry lock is poisoned"));
            }
        }
        if let Ok(mut slot) = self.shared.result.lock() {
            *slot = Some(result);
        }
        self.shared.notify.notify_waiters();
        self.armed = false;
    }
}

impl Drop for PendingCancelCompletion {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(mut state) = self.state.lock() {
            let owns_pending = matches!(
                state.active.get(&self.key),
                Some(ChildEntry::Cancelling { shared, .. })
                    if Arc::ptr_eq(shared, &self.shared)
            );
            if owns_pending {
                state
                    .active
                    .insert(self.key, ChildEntry::Accepted(self.accepted.clone()));
            }
        }
        if let Ok(mut slot) = self.shared.result.lock()
            && slot.is_none()
        {
            *slot = Some(Err(unavailable(
                "remote child cancel worker terminated before publishing a result",
            )));
        }
        self.shared.notify.notify_waiters();
    }
}

fn reserve_cancel(
    state: &Mutex<ChildState>,
    key: Digest,
    locator: &ChildRunLocator,
) -> Result<(Arc<PendingCancelResult>, bool, AcceptedChild), AgentInvokeError> {
    let mut state = state
        .lock()
        .map_err(|_| unavailable("remote child entry lock is poisoned"))?;
    match state.active.get(&key).cloned() {
        Some(ChildEntry::Accepted(accepted)) if accepted.handle.locator == *locator => {
            let shared = Arc::new(PendingCancelResult {
                result: Mutex::new(None),
                notify: tokio::sync::Notify::new(),
            });
            state.active.insert(
                key,
                ChildEntry::Cancelling {
                    accepted: accepted.clone(),
                    shared: Arc::clone(&shared),
                },
            );
            Ok((shared, true, accepted))
        }
        Some(ChildEntry::Cancelling { accepted, shared })
            if accepted.handle.locator == *locator =>
        {
            Ok((shared, false, accepted))
        }
        Some(ChildEntry::Pending { .. }) => Err(unavailable("remote child start is still pending")),
        Some(_) => Err(invalid("remote child locator digest collision")),
        None => match state.terminal.get(&key) {
            Some(terminal) if terminal.accepted.handle.locator == *locator => Ok((
                Arc::new(PendingCancelResult {
                    result: Mutex::new(Some(Ok(()))),
                    notify: tokio::sync::Notify::new(),
                }),
                false,
                terminal.accepted.clone(),
            )),
            Some(_) => Err(invalid("remote child locator digest collision")),
            None => Err(invalid("remote child locator was never accepted")),
        },
    }
}

async fn wait_pending_cancel(shared: Arc<PendingCancelResult>) -> Result<(), AgentInvokeError> {
    loop {
        let notified = shared.notify.notified();
        if let Some(result) = shared
            .result
            .lock()
            .map_err(|_| unavailable("remote child pending cancel result is poisoned"))?
            .clone()
        {
            return result;
        }
        notified.await;
    }
}

fn insert_terminal(state: &mut ChildState, key: Digest, terminal: TerminalChild) {
    state.terminal.insert(key, terminal);
    state.terminal_order.push_back(key);
    while state.terminal.len() > MAX_TERMINAL_CHILDREN {
        if let Some(oldest) = state.terminal_order.pop_front() {
            state.terminal.remove(&oldest);
        }
    }
}

fn lookup_status(
    state: &Mutex<ChildState>,
    key: Digest,
    locator: &ChildRunLocator,
) -> Result<ChildRunStatus, AgentInvokeError> {
    let state = state
        .lock()
        .map_err(|_| unavailable("remote child entry lock is poisoned"))?;
    match state.active.get(&key) {
        Some(ChildEntry::Pending { .. }) => Err(unavailable("remote child start is still pending")),
        Some(ChildEntry::Accepted(accepted)) if accepted.handle.locator == *locator => {
            Ok(ChildRunStatus::Accepted)
        }
        Some(ChildEntry::Cancelling { accepted, .. }) if accepted.handle.locator == *locator => {
            Ok(ChildRunStatus::Accepted)
        }
        Some(_) => Err(invalid("remote child locator digest collision")),
        None => match state.terminal.get(&key) {
            Some(terminal) if terminal.accepted.handle.locator == *locator => terminal
                .status
                .ok_or_else(|| unavailable("remote child terminal status is unknown")),
            Some(_) => Err(invalid("remote child locator digest collision")),
            None => Err(invalid("remote child locator was never accepted")),
        },
    }
}

fn start_payload(
    ctx: &ChildRunContext,
    request: &ChildRunRequest,
) -> Result<RemoteCommandPayload, AgentInvokeError> {
    let operation = &request.locator().operation;
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
        max_input_tokens: request.requested_budget().input_tokens,
        max_output_tokens: request.requested_budget().output_tokens,
        max_cost: request.requested_budget().cost.clone(),
        extension_counters: request
            .requested_budget()
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
        request.requested_deadline(),
        limits,
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        request.agent().spec_digest,
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
            agent_id: request.agent().id.clone(),
            bundle_id: request.agent().bundle.clone(),
            spec_digest: request.agent().spec_digest,
        },
        request.input().as_ref().to_vec(),
        request.metadata().clone(),
        request.delegation_id().map(str::to_owned),
    )
    .map_err(|_| invalid("remote child start payload is invalid"))?;
    Ok(RemoteCommandPayload::Start(Box::new(start)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::{LaneId, OperationLocator, RunId, SessionId};

    fn locator() -> ChildRunLocator {
        ChildRunLocator {
            operation: OperationLocator::try_new(
                "tenant-a",
                SessionId::from_bytes([1; 16]),
                LaneId::from_bytes([2; 16]),
                RunId::from_bytes([3; 16]),
            )
            .expect("locator"),
            remote: None,
        }
    }

    fn accepted(locator: &ChildRunLocator, request_digest: Digest) -> AcceptedChild {
        AcceptedChild {
            request_digest,
            handle: ChildRunHandle {
                locator: locator.clone(),
                relation_digest: Digest::raw_json(b"relation"),
            },
        }
    }

    #[test]
    fn successful_cancel_releases_active_capacity() {
        let locator = locator();
        let mut state = ChildState::default();
        let mut target = None;
        for index in 0..MAX_ACTIVE_CHILDREN {
            let key = Digest::raw_json(&(index as u64).to_be_bytes());
            let accepted = accepted(&locator, key);
            if index == 0 {
                target = Some((key, accepted.clone()));
            }
            state.active.insert(key, ChildEntry::Accepted(accepted));
        }
        let (key, accepted) = target.expect("target");
        let shared = Arc::new(PendingCancelResult {
            result: Mutex::new(None),
            notify: tokio::sync::Notify::new(),
        });
        state.active.insert(
            key,
            ChildEntry::Cancelling {
                accepted: accepted.clone(),
                shared: Arc::clone(&shared),
            },
        );
        let state = Arc::new(Mutex::new(state));
        let mut completion =
            PendingCancelCompletion::new(Arc::clone(&state), key, accepted, shared);
        completion.publish(Ok(()), Some(ChildRunStatus::Cancelled));
        let state = state.lock().expect("state");
        assert_eq!(state.active.len(), MAX_ACTIVE_CHILDREN - 1);
        assert_eq!(state.terminal.len(), 1);
    }

    #[test]
    fn concurrent_cancel_reservations_share_one_result() {
        let locator = locator();
        let key = Digest::raw_json(b"cancel");
        let accepted = accepted(&locator, Digest::raw_json(b"request"));
        let mut child_state = ChildState::default();
        child_state
            .active
            .insert(key, ChildEntry::Accepted(accepted.clone()));
        let state = Arc::new(Mutex::new(child_state));

        let (first, first_initiator, first_accepted) =
            reserve_cancel(&state, key, &locator).expect("first cancel");
        let (second, second_initiator, second_accepted) =
            reserve_cancel(&state, key, &locator).expect("second cancel");

        assert!(first_initiator);
        assert!(!second_initiator);
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(
            first_accepted.request_digest,
            second_accepted.request_digest
        );
        let mut completion =
            PendingCancelCompletion::new(Arc::clone(&state), key, first_accepted, first);
        completion.publish(Ok(()), Some(ChildRunStatus::Cancelled));
    }

    #[test]
    fn pending_start_status_fails_closed() {
        let locator = locator();
        let key = Digest::raw_json(b"pending");
        let shared = Arc::new(PendingResult {
            result: Mutex::new(None),
            notify: tokio::sync::Notify::new(),
        });
        let mut state = ChildState::default();
        state.active.insert(
            key,
            ChildEntry::Pending {
                request_digest: Digest::raw_json(b"request"),
                shared,
            },
        );

        let error = lookup_status(&Mutex::new(state), key, &locator).expect_err("pending");
        assert!(error.to_string().contains("still pending"));
    }

    #[test]
    fn unknown_terminal_status_fails_closed() {
        let locator = locator();
        let key = Digest::raw_json(b"terminal");
        let mut state = ChildState::default();
        insert_terminal(
            &mut state,
            key,
            TerminalChild {
                accepted: accepted(&locator, Digest::raw_json(b"request")),
                status: None,
            },
        );

        let error = lookup_status(&Mutex::new(state), key, &locator).expect_err("unknown terminal");
        assert!(error.to_string().contains("status is unknown"));
    }

    #[tokio::test]
    async fn abandoned_pending_worker_wakes_waiters_and_releases_capacity() {
        let key = Digest::raw_json(b"pending");
        let shared = Arc::new(PendingResult {
            result: Mutex::new(None),
            notify: tokio::sync::Notify::new(),
        });
        let state = Arc::new(Mutex::new(ChildState::default()));
        state.lock().expect("state").active.insert(
            key,
            ChildEntry::Pending {
                request_digest: Digest::raw_json(b"request"),
                shared: Arc::clone(&shared),
            },
        );
        drop(PendingCompletion::new(
            Arc::clone(&state),
            key,
            Arc::clone(&shared),
        ));
        let error = wait_pending(shared).await.expect_err("abandoned worker");
        assert!(error.to_string().contains("terminated"));
        assert!(state.lock().expect("state").active.is_empty());
    }
}
