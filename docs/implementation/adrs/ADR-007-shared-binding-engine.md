# ADR-007: shared binding engine

## Status

Accepted

## Date

2026-08-08

## Accountable role

Bindings lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

This decision was accepted in the planning baseline and is recorded here so later
implementation pull requests cannot silently reinterpret it. Canonical summary
text lives in Architecture Specification §25; this standalone record captures
stable decision context, consequences, compatibility classification, security
linkage, and change-control metadata required by PR-004.

## Decision

Python and WASM are bindings to the same engine, not separate runtimes

## Consequences

One Rust-owned engine serves native, Python, and browser WASM surfaces.

## Rejected alternatives

Separate Python/JS runtimes that reimplement kernel semantics.

## Compatibility and schema-change classification

Cross-binding compatibility; shared conformance traces own semantic parity.

## Security classification

- References: TM-04, TM-05, TM-06
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-PY, FR-WASM, FR-RS
- Affected Technical Design: Technical Design §25–§26 (shared binding engine)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-005, PR-027–PR-038; G4
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Verified — Python Phase 4 traces at local merge `5f15210780254e27dbe0ecf665cf38f99e412b1f` and WASM Playwright traces at local merge `04407192289c24cbfb087357b1e2ca928a8f3b55` pass G4-D-binding-parity-101224c5eb60. `DeferredBindingAdapter::wasm()` stays Unavailable; Playwright is the WASM parity evidence path.
