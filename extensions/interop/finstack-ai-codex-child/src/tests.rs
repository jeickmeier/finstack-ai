use super::*;

#[test]
fn codex_agent_ref_is_stable() {
    let first = codex_agent_ref().expect("agent ref");
    let second = codex_agent_ref().expect("agent ref");
    assert_eq!(first.id.to_string(), "finstack.peer.codex");
    assert!(first.bundle.is_none());
    assert_eq!(first.spec_digest, second.spec_digest);
}

#[test]
fn codex_route_ref_targets_the_peer_component() {
    let route = codex_route_ref().expect("route ref");
    assert_eq!(route.route.handle(), "codex-exec");
    assert_eq!(route.route.provider().to_string(), "finstack.peer.codex");
}
