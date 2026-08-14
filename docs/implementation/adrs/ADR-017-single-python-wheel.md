# ADR-017: single python wheel

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

Curated Rust-backed providers ship in one initial Python wheel

## Consequences

Python users get curated providers without multi-wheel Rust ABI risk.

## Rejected alternatives

Multiple Python wheels that rely on an unstable Rust ABI across packages.

## Compatibility and schema-change classification

Python distribution compatibility; wheel composition is lockstep pre-1.0.

## Security classification

- References: TM-18
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-PY
- Affected Technical Design: Technical Design §25 (single Python wheel)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: Freeze before PR-027; verify through PR-032
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

### PRD §18 delivery detail

- Product decision: Bundle curated Rust-backed providers in the single initial Python wheel while retaining separate Rust crates.
- Delivery point: Finalized before PR-027.
- Source: [PRD §18](../../planning/01-finstack-ai-product-requirements.md#18-resolved-foundational-product-decisions)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Verified through PR-027–PR-032 single-extension packaging, staged 0.0.2 wheel/sdist artifacts, `PH4-E-exit-wheels-45a88cc26d0b`, and local merge `5f15210780254e27dbe0ecf665cf38f99e412b1f`
