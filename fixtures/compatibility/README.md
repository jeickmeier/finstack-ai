# Compatibility fixtures

Fixture roots for versioned public contracts. PR-004 defines naming and
ownership only; payload corpora arrive with owning implementation PRs.

## Naming

```text
fixtures/compatibility/<family>/v<version>/<kind>/<valid|invalid|roundtrip>--<slug>.<ext>
fixtures/compatibility/<family>/migrations/v<from>-to-v<to>--<slug>.{before,after}.json
```

- `<family>` matches `schemas/schema-families.toml`
- `<version>` is a major integer for JSON Schema families, or a semver triple
  (`X.Y.Z`) for WIT packages that live under `plugins/finstack-ai-wit/wit/vX.Y.Z/`
- `<kind>` names the schema/artifact under test
- case kinds: `valid`, `invalid`, `roundtrip`, or migration pairs
- README files under fixture trees are scaffolding only and do not satisfy GOV006
- golden-trace / conformance fixtures reject unknown fields (TDD §28.4)

## Families

| Family | Directory |
| --- | --- |
| Public Rust APIs | [`public-rust-api/`](public-rust-api/) |
| AgentSpec | [`agent-spec/`](agent-spec/) |
| Journal | [`journal/`](journal/) |
| Runtime events | [`runtime-events/`](runtime-events/) |
| Remote DTOs | [`remote/`](remote/) |
| Process DTOs | [`process/`](process/) |
| WIT | [`wit/`](wit/) |

Reserved README-only directories are not passing conformance evidence.
