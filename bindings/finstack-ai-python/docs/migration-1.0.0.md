# Python 0.1.0 → 1.0.0 migration

Public import names stay. Pin `finstack-ai==1.0.0` when consuming a staged
wheel. The package is not on PyPI.

## Capability selection

`Agent.start` / `Agent.run` no longer select a model capability by word
overlap in user input. Pass optional `capability=` with a catalog id.
`None` (the default) runs the agent that was called. An unknown id raises
`ConfigurationError`.

Rust callers set `AgentRunRequest.capability` the same way (`try_new`
defaults it to `None`).

## Sessions

`Agent.open_session(session_id, tenant_scope)` still inspects; it does not
respawn parked runs. IndexedDB remains experimental.

## WIT guests

Experimental `@0.0.4` components stay loadable. Retarget published guests
to `@1.0.0` with
[plugins/finstack-ai-guest-sdk/MIGRATION.md](../../../plugins/finstack-ai-guest-sdk/MIGRATION.md).

## Historical notes

Pre-0.1.0 Python shape changes remain in [migration-0.0.2.md](migration-0.0.2.md).
Repository-wide paths are in [docs/site/migration.md](../../../docs/site/migration.md).
