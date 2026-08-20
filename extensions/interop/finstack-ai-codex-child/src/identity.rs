//! Frozen peer identity for the Codex child agent.

use finstack_ai_kernel::{
    ComponentId, ComponentRef, Digest, ExternalHandleRef, RawJson, RemoteRouteRef, Version,
};
use finstack_ai_runtime::AgentRef;

use crate::CodexChildError;

/// Frozen allow-list identity for the Codex peer. Codex is not a
/// bundle-resolved finstack agent; the spec digest is a version marker,
/// not a kernel spec.
pub const CODEX_PEER_AGENT_ID: &str = "finstack.peer.codex";

const CODEX_ROUTE_LABEL: &str = "codex-exec";
const CODEX_SPEC_DOMAIN: &str = "codex-peer-spec";
const CODEX_SPEC_INPUT: &[u8] = b"codex-exec/v1";

/// Build the frozen [`AgentRef`] for the Codex peer.
///
/// # Errors
///
/// Returns [`CodexChildError::Configuration`] when the checked-in identity
/// cannot be constructed (never expected at runtime).
pub fn codex_agent_ref() -> Result<AgentRef, CodexChildError> {
    let id = finstack_ai_kernel::AgentId::parse(CODEX_PEER_AGENT_ID)
        .map_err(|_| configuration("peer_agent_id_invalid"))?;
    let spec_digest = Digest::domain_separated(CODEX_SPEC_DOMAIN, 1, CODEX_SPEC_INPUT)
        .map_err(|_| configuration("peer_spec_digest_failed"))?;
    Ok(AgentRef {
        id,
        bundle: None,
        spec_digest,
    })
}

/// Build the stable [`RemoteRouteRef`] carried on every Codex child locator.
///
/// # Errors
///
/// Returns [`CodexChildError::Configuration`] when the checked-in route
/// cannot be constructed (never expected at runtime).
pub fn codex_route_ref() -> Result<RemoteRouteRef, CodexChildError> {
    let service = ComponentId::parse(CODEX_PEER_AGENT_ID)
        .map_err(|_| configuration("peer_component_id_invalid"))?;
    let metadata = RawJson::parse(b"{}").map_err(|_| configuration("route_metadata_invalid"))?;
    let route = ExternalHandleRef::try_new(service.clone(), CODEX_ROUTE_LABEL, metadata)
        .map_err(|_| configuration("route_handle_invalid"))?;
    Ok(RemoteRouteRef {
        service: ComponentRef::new(
            service,
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
        ),
        route,
    })
}

pub(crate) fn configuration(reason: &'static str) -> CodexChildError {
    CodexChildError::Configuration { reason }
}
