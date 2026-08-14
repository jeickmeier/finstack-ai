# ADR-032: disposable snapshots

## Status

Accepted

## Date

2026-08-08

## Accountable role

Durability/ecosystem lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

This decision was accepted in the planning baseline and is recorded here so later
implementation pull requests cannot silently reinterpret it. Canonical summary
text lives in Architecture Specification §25; this standalone record captures
stable decision context, consequences, compatibility classification, security
linkage, and change-control metadata required by PR-004.

## Decision

Initial snapshots are direct versioned kernel-state CBOR projections and remain disposable derived caches

## Consequences

Journals remain authoritative; snapshots may be discarded and rebuilt.

## Rejected alternatives

Treating snapshots as authoritative history rather than disposable caches.

## Compatibility and schema-change classification

Snapshot schema family; disposable derived caches only.

## Security classification

- References: TM-13
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-DUR
- Affected Technical Design: Technical Design §18, §28 (disposable snapshots)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-041
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate
- Reconsideration source: [Technical Design §37](../../planning/03-finstack-ai-technical-design.md#37-technical-decision-status)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

Exact Technical Design §37 gate: PR-041 benchmarks may propose a new ADR for a compact projection.

Additionally requires a superseding ADR and primary-document reconciliation.

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial at PR-041 candidate `8643349141ec43a342307a8708c7b204a91960c9` (A01–A04; TM-13 review). Verified waits for local merge.
