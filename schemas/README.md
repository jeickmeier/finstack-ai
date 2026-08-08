# Schema directories

Versioned public-contract schema roots. PR-004 reserved ownership and
fixture locations; PR-005 activates `golden-trace` and `benchmark-report`
payload schemas. Remaining families stay reserved until their owning PRs.

## Layout

```text
schemas/<family>/v<major>/<kind>.schema.json
```

JSON Schema draft 2020-12 is the portable schema source of truth
(ADR-022 / Engineering Standards §6).

| Family | Root | Status |
| --- | --- | --- |
| Public Rust API notes (source is `crates/`) | [`public-rust-api/`](public-rust-api/) | active notes (PR-006; fixtures under `fixtures/compatibility/public-rust-api/`) |
| AgentSpec / bundle / locks | [`agent-spec/`](agent-spec/) | reserved |
| Journal records / snapshots | [`journal/`](journal/) | reserved |
| Runtime events | [`runtime-events/`](runtime-events/) | reserved |
| Remote protocol DTOs | [`remote/`](remote/) | reserved |
| Process protocol DTOs | [`process/`](process/) | reserved |
| WIT packages | [`../plugins/finstack-ai-wit/wit/`](../plugins/finstack-ai-wit/wit/) | reserved (canonical future source under `wit/v<x.y.z>/`) |
| Golden traces / scripted inputs | [`golden-trace/`](golden-trace/) | active (PR-005) |
| Benchmark report metadata | [`benchmark-report/`](benchmark-report/) | active (PR-005) |

Registry: [`schema-families.toml`](schema-families.toml).  
Governance: [`../docs/implementation/compatibility-governance.md`](../docs/implementation/compatibility-governance.md).  
Fixtures: [`../fixtures/compatibility/`](../fixtures/compatibility/).

## Dirty-change policy

Once `*.schema.json` or versioned `.wit` files exist, a change that modifies a
family/version without updating fixtures for that same family/version under
`fixtures/compatibility/<family>/` fails `mise run schema-governance` (GOV006)
when a git base revision is provided.
Reserved README-only directories are not passing schema or conformance evidence.
