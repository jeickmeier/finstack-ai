//! Idempotent remote [`AgentInvoker`](finstack_ai_runtime::AgentInvoker).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{ChildPlacement, ChildRunLocator, Digest, RunId};
use finstack_ai_protocol::RemoteCommandOp;
use finstack_ai_runtime::{
    AgentInvokeError, AgentInvoker, ChildRunContext, ChildRunHandle, ChildRunRequest, PortFuture,
    child_relation_digest,
};

use crate::route::{RemoteChildRoute, invalid, parse_endpoint, route_ref, unavailable};
use crate::transport::exchange;

/// Remote child invoker bound to one explicit route.
pub struct RemoteChildInvoker {
    route: RemoteChildRoute,
    accepted: Arc<Mutex<BTreeMap<RunId, AcceptedChild>>>,
}

#[derive(Clone)]
struct AcceptedChild {
    request_digest: Digest,
    handle: ChildRunHandle,
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
        let route = clone_route(&self.route);
        let accepted = Arc::clone(&self.accepted);
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
            let result = exchange(&route, &request.locator, RemoteCommandOp::Start).await?;
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
        let route = clone_route(&self.route);
        let locator = locator.clone();
        Box::pin(async move {
            let result = exchange(&route, &locator, RemoteCommandOp::Cancel).await?;
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

fn clone_route(route: &RemoteChildRoute) -> RemoteChildRoute {
    RemoteChildRoute {
        endpoint: route.endpoint.clone(),
        service: route.service.clone(),
        route: route.route.clone(),
        token: route.token.clone(),
    }
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
    accepted.insert(run_id, child);
    Ok(())
}
