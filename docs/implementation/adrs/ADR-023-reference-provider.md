# ADR-023: reference provider

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

The network reference provider is OpenAI-compatible Chat Completions; the scripted model remains semantic reference

## Consequences

Provider quirks stay in adapters; semantic traces use the scripted model.

## Rejected alternatives

Making a proprietary provider SDK the semantic reference instead of the scripted model.

## Compatibility and schema-change classification

Provider adapter contract; scripted model remains semantic reference.

## Security classification

- References: SEC-INV-005; TM-04
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-MDL
- Affected Technical Design: Technical Design §14 (reference provider)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-024
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

### PRD §18 delivery detail

- Product decision: Use an OpenAI-compatible Chat Completions baseline as the reference network provider; keep the scripted model as the semantic reference and Responses mapping optional.
- Delivery point: PR-024.
- Source: [PRD §18](../../planning/01-finstack-ai-product-requirements.md#18-resolved-foundational-product-decisions)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Verified by PR-024-E-provider-6c2a8f4d1b75, PR-024-E-compat-3e7b9d1a5c82, PR-024-E-security-9d3a7c1e5b84, PR-024-E-benchmark-4f8a2d6c9b15, PR-024-E-ci-7b3d1a8c5e92, PR-024-E-integration-1f6a9c4e2b83, `PH3-E-exit-provider-tools-f0a2080a7d6d`, exact hosted evidence `PR-026-E-hosted-56c3039835fb`, and passing decision `G3-D-native-preview-14a386c7db24`
