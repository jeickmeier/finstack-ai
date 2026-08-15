# ADR-036: blob storage boundary

## Status

Accepted

## Date

2026-08-08

## Accountable role

Ecosystem lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

This decision was accepted in the planning baseline and is recorded here so later
implementation pull requests cannot silently reinterpret it. Canonical summary
text lives in Architecture Specification §25; this standalone record captures
stable decision context, consequences, compatibility classification, security
linkage, and change-control metadata required by PR-004.

## Decision

Blob storage remains an application/runtime service through 1.0 and does not become a seventh kernel port

## Consequences

Blob storage policy stays outside kernel ports through 1.0.

## Rejected alternatives

Adding blob storage as a seventh primary kernel port before 1.0.

## Compatibility and schema-change classification

Blob reference/content contract; not a seventh kernel port through 1.0.

## Security classification

- References: TM-16, TM-20
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-KRN, FR-EXT
- Affected Technical Design: Technical Design §7; Architecture Specification blob-boundary notes

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-007, PR-022, PR-037, PR-056
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate
- Reconsideration source: [Technical Design §37](../../planning/03-finstack-ai-technical-design.md#37-technical-decision-status)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

Exact Technical Design §37 gate: Post-1.0 usage evidence and a new architecture ADR.

Additionally requires a superseding ADR and primary-document reconciliation.

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Verified — PR-007 BlobRef, PR-022 scoped ArtifactStore, PR-037 experimental IndexedDB artifact adapter, and PR-056 memory/shell ArtifactStore spill. G7-D-public-preview-f7c7e70b9e04 closes the preview-mapped delivery. Blob storage is not a seventh kernel port.
