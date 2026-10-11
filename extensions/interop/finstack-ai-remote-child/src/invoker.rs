//! Idempotent remote [`AgentInvoker`](finstack_ai_runtime::child::AgentInvoker).

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    AcceptRun, BudgetPropagation, CancelRequested, CancellationInitiator, CancellationPropagation,
    ChildPlacement, ChildRunLocator, DeadlinePropagation, Digest, PrincipalPropagation,
    RunAccepted, RunId, RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext,
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
    /// Parent run that dispatched the child; labels tool-initiated cancels.
    parent_run_id: RunId,
}

#[derive(Clone)]
struct TerminalChild {
    accepted: AcceptedChild,
    /// `None` when the local view cannot vouch for a terminal status; the
    /// child may still be running remotely (see `evicted`).
    status: Option<ChildRunStatus>,
    /// Moved here by `evict_oldest_accepted` rather than by a confirmed
    /// cancel. A later cancel re-admits the entry so the request still
    /// reaches the remote peer.
    evicted: bool,
}

#[derive(Clone)]
enum ChildEntry {
    Pending {
        request_digest: Digest,
        shared: Arc<Pending<ChildRunHandle>>,
    },
    Accepted(AcceptedChild),
    Cancelling {
        accepted: AcceptedChild,
        shared: Arc<Pending<()>>,
    },
}

#[derive(Default)]
struct ChildState {
    active: BTreeMap<Digest, ChildEntry>,
    /// Exactly the keys whose `active` entry is `Accepted`, oldest first.
    /// Maintained by `insert_accepted` / `reserve_cancel`, so it is bounded
    /// by `MAX_ACTIVE_CHILDREN` and `evict_oldest_accepted` never has to
    /// scan the map.
    accepted_order: VecDeque<Digest>,
    terminal: BTreeMap<Digest, TerminalChild>,
    terminal_order: VecDeque<Digest>,
}

/// One in-flight exchange shared by every caller that attached to it.
struct Pending<T> {
    result: Mutex<Option<Result<T, AgentInvokeError>>>,
    notify: tokio::sync::Notify,
}

impl<T: Clone> Pending<T> {
    fn new(result: Option<Result<T, AgentInvokeError>>) -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(result),
            notify: tokio::sync::Notify::new(),
        })
    }

    async fn wait(self: Arc<Self>) -> Result<T, AgentInvokeError> {
        loop {
            let notified = self.notify.notified();
            if let Some(result) = self
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

    /// Publish the exchange's result and wake every waiter.
    fn publish(&self, result: Result<T, AgentInvokeError>) {
        if let Ok(mut slot) = self.result.lock() {
            *slot = Some(result);
        }
        self.notify.notify_waiters();
    }

    /// Fail every waiter with `message` unless a result was already
    /// published; used when the worker dies before publishing.
    fn abandon(&self, message: &'static str) {
        if let Ok(mut slot) = self.result.lock()
            && slot.is_none()
        {
            *slot = Some(Err(unavailable(message)));
        }
        self.notify.notify_waiters();
    }
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
                let parent_run_id = ctx.parent.run_id;
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
                        parent_run_id,
                    });
                    completion.publish(result, accepted);
                });
            }
            shared.wait().await
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
                let payload = cancel_payload(&accepted);
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
            shared.wait().await
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

/// Cancel command for an accepted child, labelled as propagated from the
/// parent run that dispatched it: a tool-initiated cancel is cascade
/// cancellation, not a runtime shutdown.
fn cancel_payload(accepted: &AcceptedChild) -> RemoteCommandPayload {
    RemoteCommandPayload::Cancel(Box::new(CancelRequested {
        initiator: CancellationInitiator::ParentRun {
            parent_run_id: accepted.parent_run_id,
        },
        reason: None,
    }))
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
    shared: Arc<Pending<ChildRunHandle>>,
    armed: bool,
}

impl PendingCompletion {
    fn new(
        state: Arc<Mutex<ChildState>>,
        key: Digest,
        shared: Arc<Pending<ChildRunHandle>>,
    ) -> Self {
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
                    insert_accepted(&mut state, self.key, accepted);
                } else {
                    state.active.remove(&self.key);
                }
            }
            Err(_) => {
                result = Err(unavailable("remote child entry lock is poisoned"));
            }
        }
        self.shared.publish(result);
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
        self.shared
            .abandon("remote child start worker terminated before publishing a result");
    }
}

fn reserve_entry(
    state: &Mutex<ChildState>,
    key: Digest,
    request: &ChildRunRequest,
) -> Result<(Arc<Pending<ChildRunHandle>>, bool), AgentInvokeError> {
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
    if state.active.len() >= MAX_ACTIVE_CHILDREN && !evict_oldest_accepted(&mut state) {
        return Err(unavailable("remote child active entry map is full"));
    }
    let shared = Pending::new(None);
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
) -> Result<(Arc<Pending<ChildRunHandle>>, bool), AgentInvokeError> {
    if &existing.handle.locator != request.locator() {
        return Err(invalid("remote child locator digest collision"));
    }
    if existing.request_digest != request.request_digest() {
        return Err(AgentInvokeError::Conflict {
            existing: existing.request_digest,
            submitted: request.request_digest(),
        });
    }
    Ok((Pending::new(Some(Ok(existing.handle.clone()))), false))
}

struct PendingCancelCompletion {
    state: Arc<Mutex<ChildState>>,
    key: Digest,
    accepted: AcceptedChild,
    shared: Arc<Pending<()>>,
    armed: bool,
}

impl PendingCancelCompletion {
    fn new(
        state: Arc<Mutex<ChildState>>,
        key: Digest,
        accepted: AcceptedChild,
        shared: Arc<Pending<()>>,
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
                            evicted: false,
                        },
                    );
                } else {
                    insert_accepted(&mut state, self.key, self.accepted.clone());
                }
            }
            Err(_) => {
                result = Err(unavailable("remote child entry lock is poisoned"));
            }
        }
        self.shared.publish(result);
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
                insert_accepted(&mut state, self.key, self.accepted.clone());
            }
        }
        self.shared
            .abandon("remote child cancel worker terminated before publishing a result");
    }
}

fn reserve_cancel(
    state: &Mutex<ChildState>,
    key: Digest,
    locator: &ChildRunLocator,
) -> Result<(Arc<Pending<()>>, bool, AcceptedChild), AgentInvokeError> {
    let mut state = state
        .lock()
        .map_err(|_| unavailable("remote child entry lock is poisoned"))?;
    match state.active.get(&key).cloned() {
        Some(ChildEntry::Accepted(accepted)) if accepted.handle.locator == *locator => {
            Ok(begin_cancel(&mut state, key, accepted))
        }
        Some(ChildEntry::Cancelling { accepted, shared })
            if accepted.handle.locator == *locator =>
        {
            Ok((shared, false, accepted))
        }
        Some(ChildEntry::Pending { .. }) => Err(unavailable("remote child start is still pending")),
        Some(_) => Err(invalid("remote child locator digest collision")),
        None => match state.terminal.get(&key).cloned() {
            Some(terminal) if terminal.accepted.handle.locator == *locator => {
                if !terminal.evicted {
                    return Ok((Pending::new(Some(Ok(()))), false, terminal.accepted));
                }
                // An evicted child may still be running remotely, so the
                // cancel must reach the peer: re-admit the entry as
                // `Cancelling`, making room the same way a start would.
                if state.active.len() >= MAX_ACTIVE_CHILDREN && !evict_oldest_accepted(&mut state) {
                    return Err(unavailable("remote child active entry map is full"));
                }
                state.terminal.remove(&key);
                state.terminal_order.retain(|entry| *entry != key);
                Ok(begin_cancel(&mut state, key, terminal.accepted))
            }
            Some(_) => Err(invalid("remote child locator digest collision")),
            None => Err(invalid("remote child locator was never accepted")),
        },
    }
}

/// Move `key` from `Accepted` to `Cancelling`, keeping `accepted_order` exact.
fn begin_cancel(
    state: &mut ChildState,
    key: Digest,
    accepted: AcceptedChild,
) -> (Arc<Pending<()>>, bool, AcceptedChild) {
    let shared = Pending::new(None);
    state.accepted_order.retain(|entry| *entry != key);
    state.active.insert(
        key,
        ChildEntry::Cancelling {
            accepted: accepted.clone(),
            shared: Arc::clone(&shared),
        },
    );
    (shared, true, accepted)
}

/// Record `key` as `Accepted`, appending it to `accepted_order`.
///
/// Every transition into `Accepted` (start acknowledged, cancel failed or
/// abandoned) goes through here so the order queue stays exact.
fn insert_accepted(state: &mut ChildState, key: Digest, accepted: AcceptedChild) {
    state.accepted_order.retain(|entry| *entry != key);
    state.accepted_order.push_back(key);
    state.active.insert(key, ChildEntry::Accepted(accepted));
}

/// Evict the oldest `Accepted` child into the terminal map to free one
/// active slot. Returns `false` when nothing is evictable: `Pending` and
/// `Cancelling` entries own an in-flight exchange and are never evicted.
///
/// The evicted child is not known to be terminal; the remote peer still
/// owns it. The kernel has no "evicted" status, so the entry is recorded
/// with `status: None` (reported as an unknown terminal status, matching
/// the codex child's `unknown` for evicted runs) and flagged `evicted` so
/// a later cancel is re-admitted and still reaches the peer.
fn evict_oldest_accepted(state: &mut ChildState) -> bool {
    while let Some(oldest) = state.accepted_order.pop_front() {
        let Some(ChildEntry::Accepted(accepted)) = state.active.get(&oldest).cloned() else {
            continue;
        };
        state.active.remove(&oldest);
        insert_terminal(
            state,
            oldest,
            TerminalChild {
                accepted,
                status: None,
                evicted: true,
            },
        );
        return true;
    }
    false
}

fn insert_terminal(state: &mut ChildState, key: Digest, terminal: TerminalChild) {
    state.terminal.insert(key, terminal);
    state.terminal_order.push_back(key);
    // `terminal_order` holds exactly the keys of `terminal` (re-admission
    // removes from both), so each pop frees one slot; the `break` is a
    // defensive guard against spinning should that ever drift.
    while state.terminal.len() > MAX_TERMINAL_CHILDREN {
        let Some(oldest) = state.terminal_order.pop_front() else {
            break;
        };
        state.terminal.remove(&oldest);
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
    use finstack_ai_kernel::{
        BudgetRequest, ContentBlock, LaneId, Metadata, OperationLocator, SessionId, TextBlock,
    };

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

    fn parent_run_id() -> RunId {
        RunId::from_bytes([6; 16])
    }

    fn accepted(locator: &ChildRunLocator, request_digest: Digest) -> AcceptedChild {
        AcceptedChild {
            request_digest,
            handle: ChildRunHandle {
                locator: locator.clone(),
                relation_digest: Digest::raw_json(b"relation"),
            },
            parent_run_id: parent_run_id(),
        }
    }

    fn request(locator: &ChildRunLocator) -> ChildRunRequest {
        let agent = finstack_ai_runtime::child::AgentRef {
            id: finstack_ai_kernel::AgentId::parse("python.agent.remote").expect("agent"),
            bundle: None,
            spec_digest: Digest::raw_json(b"{}"),
        };
        let input = Arc::from([ContentBlock::Text(
            TextBlock::try_new("hello").expect("text"),
        )]);
        // The fixture locator carries no remote route, so use the isolated
        // placement: `reserve_entry` only reads the digest and locator.
        ChildRunRequest::try_new(
            agent,
            input,
            ChildPlacement::IsolatedChildSession,
            locator.clone(),
            None,
            BudgetRequest::default(),
            None,
            Metadata::empty(),
        )
        .expect("valid child request")
    }

    /// Fill `state` with `MAX_ACTIVE_CHILDREN` accepted children, returning
    /// the key inserted first.
    fn fill_accepted(state: &mut ChildState, locator: &ChildRunLocator) -> Digest {
        let mut oldest = None;
        for index in 0..MAX_ACTIVE_CHILDREN {
            let key = Digest::raw_json(&(index as u64).to_be_bytes());
            insert_accepted(state, key, accepted(locator, key));
            oldest.get_or_insert(key);
        }
        oldest.expect("filled")
    }

    #[test]
    fn successful_cancel_releases_active_capacity() {
        let locator = locator();
        let mut state = ChildState::default();
        let key = fill_accepted(&mut state, &locator);
        let (shared, initiator, accepted) = begin_cancel(&mut state, key, accepted(&locator, key));
        assert!(initiator);
        let state = Arc::new(Mutex::new(state));
        let mut completion =
            PendingCancelCompletion::new(Arc::clone(&state), key, accepted, shared);
        completion.publish(Ok(()), Some(ChildRunStatus::Cancelled));
        let state = state.lock().expect("state");
        assert_eq!(state.active.len(), MAX_ACTIVE_CHILDREN - 1);
        assert_eq!(state.terminal.len(), 1);
        assert_eq!(state.accepted_order.len(), MAX_ACTIVE_CHILDREN - 1);
    }

    /// A table of 1 024 accepted children that are never cancelled must not
    /// refuse every later start forever: the oldest accepted child is moved
    /// to the terminal map (unknown status) to make room.
    #[test]
    fn full_active_table_evicts_oldest_accepted() {
        let locator = locator();
        let mut state = ChildState::default();
        let oldest = fill_accepted(&mut state, &locator);
        let state = Mutex::new(state);

        let new_key = Digest::raw_json(b"new");
        let (_, initiator) = reserve_entry(&state, new_key, &request(&locator)).expect("evicts");
        assert!(initiator);

        let guard = state.lock().expect("state");
        assert_eq!(guard.active.len(), MAX_ACTIVE_CHILDREN);
        assert!(matches!(
            guard.active.get(&new_key),
            Some(ChildEntry::Pending { .. })
        ));
        assert!(!guard.active.contains_key(&oldest));
        let evicted = guard.terminal.get(&oldest).expect("evicted into terminal");
        assert!(evicted.evicted);
        assert_eq!(evicted.status, None);
        assert_eq!(guard.accepted_order.len(), MAX_ACTIVE_CHILDREN - 1);
        drop(guard);

        let error = lookup_status(&state, oldest, &locator).expect_err("evicted status");
        assert!(error.to_string().contains("status is unknown"));
    }

    /// Only `Accepted` entries are evictable; a table full of in-flight
    /// starts or cancels still fails closed.
    #[test]
    fn full_table_of_pending_or_cancelling_is_not_evicted() {
        let locator = locator();
        let mut state = ChildState::default();
        for index in 0..MAX_ACTIVE_CHILDREN {
            let key = Digest::raw_json(&(index as u64).to_be_bytes());
            if index % 2 == 0 {
                state.active.insert(
                    key,
                    ChildEntry::Pending {
                        request_digest: key,
                        shared: Pending::new(None),
                    },
                );
            } else {
                begin_cancel(&mut state, key, accepted(&locator, key));
            }
        }
        let state = Mutex::new(state);
        let Err(error) = reserve_entry(&state, Digest::raw_json(b"new"), &request(&locator)) else {
            panic!("nothing is evictable");
        };
        assert!(error.to_string().contains("is full"));
        assert_eq!(
            state.lock().expect("state").active.len(),
            MAX_ACTIVE_CHILDREN
        );
    }

    /// An evicted child may still be running remotely, so a cancel for it
    /// is re-admitted as an in-flight cancel rather than answered locally.
    #[test]
    fn cancel_of_evicted_child_is_readmitted() {
        let locator = locator();
        let mut state = ChildState::default();
        let oldest = fill_accepted(&mut state, &locator);
        assert!(evict_oldest_accepted(&mut state));
        assert!(
            state
                .terminal
                .get(&oldest)
                .is_some_and(|entry| entry.evicted)
        );
        let state = Mutex::new(state);

        let (shared, initiator, accepted) =
            reserve_cancel(&state, oldest, &locator).expect("cancel");
        assert!(initiator);
        assert_eq!(accepted.request_digest, oldest);
        {
            let guard = state.lock().expect("state");
            assert!(matches!(
                guard.active.get(&oldest),
                Some(ChildEntry::Cancelling { .. })
            ));
            assert!(!guard.terminal.contains_key(&oldest));
            assert!(
                guard.terminal_order.is_empty(),
                "re-admission drops the order entry"
            );
            assert_eq!(guard.active.len(), MAX_ACTIVE_CHILDREN);
        }

        // Once the re-admitted cancel is confirmed the child is terminal
        // again, recorded exactly once with a known status.
        let state = Arc::new(Mutex::new(state.into_inner().expect("state")));
        let mut completion =
            PendingCancelCompletion::new(Arc::clone(&state), oldest, accepted, shared);
        completion.publish(Ok(()), Some(ChildRunStatus::Cancelled));
        let guard = state.lock().expect("state");
        assert_eq!(guard.active.len(), MAX_ACTIVE_CHILDREN - 1);
        assert_eq!(guard.terminal.len(), 1);
        assert_eq!(guard.terminal_order.len(), guard.terminal.len());
        let terminal = guard.terminal.get(&oldest).expect("cancelled");
        assert!(!terminal.evicted);
        assert_eq!(terminal.status, Some(ChildRunStatus::Cancelled));
    }

    #[test]
    fn tool_cancel_is_labelled_parent_run() {
        let locator = locator();
        let payload = cancel_payload(&accepted(&locator, Digest::raw_json(b"request")));
        let RemoteCommandPayload::Cancel(cancel) = payload else {
            panic!("expected cancel payload");
        };
        assert_eq!(
            cancel.initiator,
            CancellationInitiator::ParentRun {
                parent_run_id: parent_run_id()
            }
        );
        assert_eq!(cancel.reason, None);
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
        let shared = Pending::new(None);
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
                evicted: false,
            },
        );

        let error = lookup_status(&Mutex::new(state), key, &locator).expect_err("unknown terminal");
        assert!(error.to_string().contains("status is unknown"));
    }

    #[tokio::test]
    async fn abandoned_pending_worker_wakes_waiters_and_releases_capacity() {
        let key = Digest::raw_json(b"pending");
        let shared = Pending::new(None);
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
        let error = shared.wait().await.expect_err("abandoned worker");
        assert!(error.to_string().contains("terminated"));
        assert!(state.lock().expect("state").active.is_empty());
    }
}
