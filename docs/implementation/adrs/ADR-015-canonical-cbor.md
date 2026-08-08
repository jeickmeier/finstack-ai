# ADR-015: canonical cbor

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

Deterministic versioned CBOR is canonical; JSON/JSONL is diagnostic.

PR-004 freezes the v1 canonical profile from Technical Design §28.1. Implementation remains PR-039. The frozen profile requires:

- definite-length items;
- shortest-form integer and length encodings;
- integers representable directly by CBOR major types 0 and 1 only; bignum tags 2 and 3 are rejected in v1;
- deterministic map-key ordering (RFC 8949);
- finite floats only, encoded in the shortest IEEE-754 width that preserves the value exactly; negative zero is preserved as distinct from positive zero; non-finite values are rejected;
- no duplicate map keys; and
- schema-level size/depth limits before allocation.

A project-owned codec wrapper validates the value tree and applies canonical map ordering before encoding. Diagnostic JSON/JSONL remains a lossless projection and must not become the authoritative journal encoding.

## Consequences

Durable digests and journals use the frozen CBOR profile; JSON remains diagnostic/lossless projection. PR-039 must publish binary compatibility fixtures covering nested-map insertion-order permutations, integer/float width boundaries through `u64::MAX`, negative zero, and rejection of bignum tags, duplicate keys, and non-finite values.

## Rejected alternatives

JSON-only journals as the authoritative durable encoding.

## Compatibility and schema-change classification

Journal/protocol encoding profile; canonical CBOR with diagnostic JSON/JSONL projection.

## Security classification

- References: SEC-INV-007, SEC-INV-010; TM-09, TM-12, TM-16
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-DUR, NFR-COMP
- Affected Technical Design: Technical Design §28 (canonical CBOR)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-004 profile; PR-008 freezes digest domain constants and semantic `RecordDraft`/`RecordEnvelope` field shapes without a CBOR codec; PR-039 implements the codec, payload digests, checksums, and binary fixtures
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

### PRD §18 delivery detail

- Product decision: Use deterministic, versioned CBOR as the canonical journal and protocol envelope; retain lossless JSON/JSONL diagnostic projection.
- Delivery point: Profile frozen in PR-004; semantic digest domains and record field shapes referenced from PR-008; codec/digest/checksum implementation in PR-039. Pack v0.15 clarifies that PR-008 must not implement canonical-CBOR encoding or claim cross-language payload-digest evidence.
- Source: [PRD §18](../../planning/01-finstack-ai-product-requirements.md#18-resolved-foundational-product-decisions)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Missing until mapped delivery work completes and evidence is verified
