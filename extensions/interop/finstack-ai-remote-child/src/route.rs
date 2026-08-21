//! Explicit remote-child route. Never discovered and never read from env.

use std::net::SocketAddr;
use std::path::PathBuf;

use finstack_ai_kernel::{
    ComponentId, ComponentRef, ExternalHandleRef, RawJson, RemoteRouteRef, Version,
};
use finstack_ai_runtime::{AgentInvokeError, SecretString};

/// Explicit route and credential used to construct a remote invoker.
///
/// The invoker never reads environment variables and never discovers peers.
#[derive(Clone)]
pub struct RemoteChildRoute {
    /// Loopback `host:port` or `unix:/absolute/path`.
    pub endpoint: String,
    /// Remote service component id.
    pub service: String,
    /// Opaque non-secret route handle label.
    pub route: String,
    /// Optional Bearer token. `None` selects loopback auth.
    pub token: Option<String>,
}

/// Parsed connect target after loopback validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteEndpoint {
    Tcp(SocketAddr),
    Unix(PathBuf),
}

/// Fully validated internal route used by all exchanges.
#[derive(Clone)]
pub(crate) struct ResolvedRemoteRoute {
    pub(crate) endpoint: RemoteEndpoint,
    pub(crate) reference: RemoteRouteRef,
    pub(crate) token: Option<SecretString>,
}

pub(crate) fn resolve_route(
    route: RemoteChildRoute,
) -> Result<ResolvedRemoteRoute, AgentInvokeError> {
    let endpoint = parse_endpoint(&route.endpoint)?;
    let reference = route_ref(&route)?;
    let token = route
        .token
        .map(SecretString::try_new)
        .transpose()
        .map_err(|_| invalid("remote child token is invalid"))?;
    Ok(ResolvedRemoteRoute {
        endpoint,
        reference,
        token,
    })
}

pub(crate) fn parse_endpoint(endpoint: &str) -> Result<RemoteEndpoint, AgentInvokeError> {
    if let Some(path) = endpoint.strip_prefix("unix:") {
        let path = PathBuf::from(path);
        if path.as_os_str().is_empty()
            || path.as_os_str().as_encoded_bytes().contains(&0)
            || !path.is_absolute()
        {
            return Err(invalid("remote child unix endpoint is invalid"));
        }
        return Ok(RemoteEndpoint::Unix(path));
    }
    let addr = endpoint
        .parse::<SocketAddr>()
        .map_err(|_| invalid("remote child TCP endpoint is invalid"))?;
    if !addr.ip().is_loopback() {
        return Err(invalid(
            "plaintext remote child endpoints must be loopback TCP or unix",
        ));
    }
    Ok(RemoteEndpoint::Tcp(addr))
}

pub(crate) fn route_ref(route: &RemoteChildRoute) -> Result<RemoteRouteRef, AgentInvokeError> {
    let service =
        ComponentId::parse(&route.service).map_err(|_| invalid("remote service is invalid"))?;
    let metadata =
        RawJson::parse(b"{}").map_err(|_| invalid("remote route metadata is invalid"))?;
    Ok(RemoteRouteRef {
        service: ComponentRef::new(
            service.clone(),
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
        ),
        route: ExternalHandleRef::try_new(service, &route.route, metadata)
            .map_err(|_| invalid("remote route handle is invalid"))?,
    })
}

pub(crate) fn invalid(message: &'static str) -> AgentInvokeError {
    AgentInvokeError::InvalidRequest {
        message: std::sync::Arc::from(message),
    }
}

pub(crate) fn unavailable(message: impl Into<String>) -> AgentInvokeError {
    AgentInvokeError::Unavailable {
        message: std::sync::Arc::from(message.into()),
    }
}
