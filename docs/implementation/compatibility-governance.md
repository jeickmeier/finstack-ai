# Compatibility governance

Operational routing for public contracts governed by Engineering Standards §6,
Technical Design §28.4, and Implementation Plan §6.1–§6.3. This document does
not redefine requirements; it records owners, promises, unknown-field profiles,
and test locations for PR-004 acceptance.

Planning docs under `docs/planning/` remain authoritative.

Open public-surface deltas before `0.1.0` are triaged in
[`public-api-change-backlog.md`](public-api-change-backlog.md). That
file is Phase 8 entrance evidence. The adopter-facing unpublished
`0.1.0` promise is [`preview-compatibility-policy.md`](preview-compatibility-policy.md).

## Contract families

| Family | Owner | Review partner | Source location | Compatibility promise | Unknown-field / evolution profile | Test location |
| --- | --- | --- | --- | --- | --- | --- |
| Public Rust APIs | Core/runtime lead (`me@jeickmeier.com`) | Bindings lead | `crates/` public items and feature names (`schemas/public-rust-api/` holds reserved notes only) | Pre-1.0 candidate surface; breaking changes require classification, changelog, and cross-binding review | N/A (Rust API); semantic renames/removals follow pre-1.0 policy | `fixtures/compatibility/public-rust-api/` |
| Journal records / snapshots | Durability/ecosystem lead (`me@jeickmeier.com`) | Core/runtime lead | `schemas/journal/`; kernel record types and `finstack-ai-protocol` | Candidate-v1 until public preview enumerates support; journal meaning breaks need ADR + migration | Schema-declared ignorable durable optionals preserved; unknown state-bearing fields/kinds fatal (TDD §28.4 durable row) | `fixtures/compatibility/journal/v1/`; `cargo test -p finstack-ai-test --test journal_v1` |
| Runtime events | Core/runtime lead (`me@jeickmeier.com`) | Bindings lead | `schemas/runtime-events/` (reserved) | Candidate-v1; durable vs transient classification is part of the contract | Durable-derived event payloads follow the durable-record unknown-field rule (ignorable optionals only; unknown state-bearing fields/kinds fatal). Transient progress/diagnostic events follow diagnostic-metadata rules (retain-or-ignore; must not influence replay). Durable-derived IDs are replay-stable; transient IDs are explicitly non-replay-stable (TDD §20.2, §28.4) | `fixtures/compatibility/runtime-events/` |
| AgentSpec / config | Ecosystem lead (`me@jeickmeier.com`) | Core/runtime lead | `schemas/agent-spec/` (reserved) | Strict reject-unknown for AgentSpec, BundleSpec, locks, and component config unless an extension-owned versioned schema declares fields | Reject by default; additive fields need schema-version/default semantics | `fixtures/compatibility/agent-spec/` |
| Remote protocol DTOs | Runtime/security owner (`me@jeickmeier.com`) | Core/runtime lead | `schemas/remote/` (candidate-v1) | Inbound commands reject unknowns before state lookup; outbound may accept schema-declared ignorable optionals within negotiated version | Strict inbound; negotiated outbound optionals (TDD §28.4 remote row) | `fixtures/compatibility/remote/v1/` |
| Process protocol DTOs | Runtime/security owner (`me@jeickmeier.com`) | Core/runtime lead | `schemas/process/` (candidate-v1) | Same unknown-field profile as remote inbound/outbound, with a distinct handshake vocabulary | Strict inbound; negotiated outbound optionals. Shared framing with remote is allowed (ADR-021); fixtures and kinds stay family-separate | `fixtures/compatibility/process/v1/` |
| WIT packages | Runtime/security owner (`me@jeickmeier.com`) | Security reviewer | `plugins/finstack-ai-wit/wit/v0.0.4/` | Experimental @0.x until framework 1.0; worlds are exact | Exact compiled worlds; no unknown fields | `fixtures/compatibility/wit/v0.0.4/` |
| Golden traces / scripted inputs | Core/runtime lead (`me@jeickmeier.com`) | Bindings lead | `schemas/golden-trace/` | Candidate-v1 fixture language for conformance; opaque expected state/result/hash until Phase 1 | Reject unknown fields; reject oversized payload declarations against TDD §6.5 ceilings | `fixtures/compatibility/golden-trace/`; `cargo test -p finstack-ai-test`; `mise run conformance` |
| Benchmark report metadata | Core/runtime lead (`me@jeickmeier.com`) | Release/CI owner | `schemas/benchmark-report/` | Candidate-v1 machine-readable Criterion metadata | Reject unknown fields; array/object bounds enforced | `fixtures/compatibility/benchmark-report/`; `mise run benchmark` / `benchmark-smoke` |
| Plugin lockfile | Runtime/security owner (`me@jeickmeier.com`) | Security reviewer | `schemas/plugin-lock/` | Experimental 0.x local lock; no registry fetch | Reject unknown fields; relative local paths only | `fixtures/compatibility/plugin-lock/v1/`; `mise run check-plugin-lock` |

Machine-readable registry: [`schemas/schema-families.toml`](../../schemas/schema-families.toml).

## Pre-1.0 breakage policy

Through pre-1.0, semantic core crates, binding distributions, and first-party
leaf crates share one lockstep workspace version. Candidate or experimental
labeling does **not** permit unclassified breakage.

For an independently decoded contract change:

1. Classify the change with [`schema-change-template.md`](schema-change-template.md).
2. Bump the family/version when decoding independence requires it.
3. Update or add compatibility fixtures for the same family/version in the same change.
4. Record migration/compatibility analysis and cross-surface review notes.
5. Add a changelog entry.
6. Open a superseding ADR when the change alters compatibility policy itself,
   journal/protocol/WIT stability rules, or any Implementation Plan §6.3 trigger.

Silent semantic discard of unknown values that could affect state,
authorization, idempotency, ordering, or recovery is prohibited.

## Seventh port and eighth middleware stage gates

Before merge, a change that adds a seventh primary extension port or an eighth
normalized middleware stage requires:

1. a new ADR accepted under the register change-control rules;
2. Architecture Specification and Threat Model reconciliation;
3. compatibility fixtures and migration/compatibility analysis;
4. performance evidence appropriate to the hot path; and
5. Implementation Plan §6.3 / ENG-ARCH-002 compliance.

ADR-005 freezes six ports for the first major version. ADR-036 keeps blob
storage outside kernel ports through 1.0. ADR-028 freezes `before_finalize` as
the final behavior-changing stage; ADR-037 places compaction on `before_model`
rather than inventing a new stage.

## Security-sensitive ADR closure

An ADR that changes a listed trust boundary cannot close without updating the
Threat Model or documenting why no threat/control changes. Standalone PR-004
records of the accepted baseline normally use:

> No control change: standalone recording of the accepted planning baseline…

Automation (`mise run schema-governance`, GOV003) proves completeness of the
security section; correctness of the security judgment remains a named review.
