# Schema directories

Versioned public-contract schema roots. Active families own executable schemas
and compatibility fixtures; remaining families are reserved until implemented.

## Layout

```text
schemas/<family>/v<major>/<kind>.schema.json
```

JSON Schema draft 2020-12 is the portable schema source of truth
(ADR-022 / Engineering Standards §6).

| Family | Root | Status |
| --- | --- | --- |
| Public Rust API notes (source is `crates/`) | [`public-rust-api/`](public-rust-api/) | active notes and fixtures |
| AgentSpec / bundle / locks | [`agent-spec/`](agent-spec/) | reserved |
| Journal records / snapshots | [`journal/`](journal/) | reserved |
| Runtime events | [`runtime-events/`](runtime-events/) | reserved |
| Remote protocol DTOs | [`remote/`](remote/) | reserved |
| Process protocol DTOs | [`process/`](process/) | reserved |
| WIT packages | [`../plugins/finstack-ai-wit/wit/`](../plugins/finstack-ai-wit/wit/) | reserved (canonical future source under `wit/v<x.y.z>/`) |
| Golden traces / scripted inputs | [`golden-trace/`](golden-trace/) | active |
| Benchmark report metadata | [`benchmark-report/`](benchmark-report/) | active |

Registry: [`schema-families.toml`](schema-families.toml).  
Fixtures: [`../fixtures/compatibility/`](../fixtures/compatibility/).

## Dirty-change policy

A change that modifies a family/version must update the fixtures for that same
family/version under `fixtures/compatibility/<family>/` in the same change.
Reserved README-only directories are not passing schema or conformance evidence.
