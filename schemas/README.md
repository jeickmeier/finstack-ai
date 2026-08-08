# Schema directories

Reserved roots for versioned public contracts. PR-004 creates ownership,
compatibility promises, and fixture locations only. No schema payloads,
validators, codecs, or WIT packages are implemented here.

## Layout

```text
schemas/<family>/v<major>/<kind>.schema.json
```

JSON Schema draft 2020-12 is the portable schema source of truth
(ADR-022 / Engineering Standards §6).

| Family | Root | Status |
| --- | --- | --- |
| Public Rust API notes (source is `crates/`) | [`public-rust-api/`](public-rust-api/) | reserved notes |
| AgentSpec / bundle / locks | [`agent-spec/`](agent-spec/) | reserved |
| Journal records / snapshots | [`journal/`](journal/) | reserved |
| Runtime events | [`runtime-events/`](runtime-events/) | reserved |
| Remote protocol DTOs | [`remote/`](remote/) | reserved |
| Process protocol DTOs | [`process/`](process/) | reserved |
| WIT packages | [`../plugins/finstack-ai-wit/wit/`](../plugins/finstack-ai-wit/wit/) | reserved (canonical future source under `wit/v<x.y.z>/`) |

Registry: [`schema-families.toml`](schema-families.toml).  
Governance: [`../docs/implementation/compatibility-governance.md`](../docs/implementation/compatibility-governance.md).  
Fixtures: [`../fixtures/compatibility/`](../fixtures/compatibility/).

## Dirty-change policy

Once `*.schema.json` or versioned `.wit` files exist, a change that modifies a
family/version without updating fixtures for that same family/version under
`fixtures/compatibility/<family>/` fails `mise run schema-governance` (GOV006)
when a git base revision is provided.
Reserved README-only directories are not passing schema or conformance evidence.
