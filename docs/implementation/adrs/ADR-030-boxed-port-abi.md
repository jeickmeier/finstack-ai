# ADR-030: boxed port abi

## Status

Accepted

## Date

2026-08-08

## Accountable role

Core/runtime lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

This decision was accepted in the planning baseline and is recorded here so later
implementation pull requests cannot silently reinterpret it. Canonical summary
text lives in Architecture Specification §25; this standalone record captures
stable decision context, consequences, compatibility classification, security
linkage, and change-control metadata required by PR-004.

## Decision

Object-safe boxed futures/streams are the initial public extension ABI; concrete implementations may optimize internally

## Consequences

Public traits stay object-safe; internals may specialize.

## Rejected alternatives

Exposing only concrete generic ABIs that cannot object-safe across bindings.

## Compatibility and schema-change classification

Public Rust extension ABI; object-safe boxed futures/streams initially.

## Security classification

- References: TM-06
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-EXT, FR-RS
- Affected Technical Design: Technical Design §3, §14–§18 (boxed port ABI)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-015–PR-018, PR-026; G3
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate
- Reconsideration source: [Technical Design §37](../../planning/03-finstack-ai-technical-design.md#37-technical-decision-status)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

Exact Technical Design §37 gate: Phase 3 benchmarks must show material dispatch cost before changing the public ABI.

Additionally requires a superseding ADR and primary-document reconciliation.

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial through PR-015 Model, locally integrated PR-016 Toolset, and immutable PR-018 target-correct boxed `ContextProvider`/`Middleware`/`Observer` native/WASM leaf evidence (`PR-018-E-extensions-8c2a6f4d1b73`, `PR-018-E-compat-5e9b3d7a2c16`) at `aac573b593dd3950a4143672aeadc3765fa88a29`; PR-026/G3 review remains
