# ADR-031: worker based wasm

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

Browser WASM is single-threaded/worker-based by default; threaded WASM is a post-preview opt-in requiring separate evidence

## Consequences

Default browser path avoids SharedArrayBuffer requirements.

## Rejected alternatives

Requiring SharedArrayBuffer/threaded WASM for the default browser path.

## Compatibility and schema-change classification

Browser WASM topology policy; threaded WASM is post-preview opt-in.

## Security classification

- References: TM-05
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-WASM
- Affected Technical Design: Technical Design §26 (worker-based Wasm)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-033, PR-036, PR-038
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate
- Reconsideration source: [Technical Design §37](../../planning/03-finstack-ai-technical-design.md#37-technical-decision-status)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

Exact Technical Design §37 gate: Post-preview opt-in only, after cross-origin isolation and conformance evidence.

Additionally requires a superseding ADR and primary-document reconciliation.

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial — PR-033 package start; PR-036 worker helper and transferable batch protocol at local merge `ecf6d22962676ba0159ab6f31ba5040d0d4cd7f4`; PR-038 candidate `f1ff388563dfce92169c9fe6ae8c845fec86794b` keeps the single-thread worker default and documents SharedArrayBuffer as post-preview reconsideration. Implemented closeout waits for local merge.
