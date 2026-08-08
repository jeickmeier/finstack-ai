# ADR-018: python version matrix

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

Python launches with CPython 3.11-3.14 per-version wheels plus 3.14t

## Consequences

CI and packaging must cover the approved CPython matrix before PR-027 lands.

## Rejected alternatives

Launching on classic abi3 wheels before production evidence justifies it.

## Compatibility and schema-change classification

Python support matrix; binding CI owns verification.

## Security classification

- References: TM-18
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-PY
- Affected Technical Design: Technical Design §25 (Python version matrix)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: Approve before PR-027; verify through PR-032
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

### PRD §18 delivery detail

- Product decision: Support CPython 3.11-3.14 with per-version wheels and 3.14t where supported; skip classic abi3 initially and gate Python 3.15+ abi3t adoption on production evidence.
- Delivery point: Matrix approved before PR-027.
- Source: [PRD §18](../../planning/01-finstack-ai-product-requirements.md#18-resolved-foundational-product-decisions)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Missing until mapped delivery work completes and evidence is verified
