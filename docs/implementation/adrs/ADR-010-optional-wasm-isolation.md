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
- Implementation evidence: Verified — optional isolation stays outside the kernel through PR-049–PR-054 local merges and G6-D-plugin-alpha-018aaea9aa00. Wasmtime remains the `plugins/` host leaf; default bundles do not depend on it. Lockfile-driven local discovery is `PluginHost::load_enabled`.
