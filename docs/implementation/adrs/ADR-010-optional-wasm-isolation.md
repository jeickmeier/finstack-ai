# ADR-010: optional wasm isolation

## Status

Accepted

## Date

2026-08-08

## Accountable role

Runtime/security owner

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

This decision was accepted in the planning baseline and is recorded here so later
implementation pull requests cannot silently reinterpret it. Canonical summary
text lives in Architecture Specification §25; this standalone record captures
stable decision context, consequences, compatibility classification, security
linkage, and change-control metadata required by PR-004.

## Decision

WASM isolation is optional and kept outside the kernel

## Consequences

Isolation is opt-in through plugins/; trusted native batteries stay under extensions/.

## Rejected alternatives

Making Wasmtime a kernel dependency or mandatory isolation path.

## Compatibility and schema-change classification

Plugin isolation boundary; WIT/package contracts stay outside the kernel.

## Security classification

- References: SEC-INV-006, SEC-INV-007, SEC-INV-011, SEC-INV-012; TM-06, TM-07, TM-08
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-PLG
- Affected Technical Design: Technical Design §27, §31 (optional Wasm isolation)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-002, PR-049–PR-054; G6
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial via PR-002 wasm-host checks, PR-049/PR-050 in-process WIT leaf, PR-051-E-candidate-36f1ce2e263b / PR-051-E-security-36f1ce2e263b / PR-051-E-integration-cf7eaebab383 at local merge `cf7eaebab383724fa4b7b19cb204febec41a768a` (first isolated Wasmtime T3 host and host-owned compile cache), and PR-052-E-candidate-4234efbcc3d9 / PR-052-E-security-4234efbcc3d9 (deny-by-default WASI grants, fuel/StoreLimits, ed25519 trust-root verify). Lockfile discovery remains PR-054. G6 owns Implemented.
