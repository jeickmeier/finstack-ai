# ADR-025: generic deferred effects

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

Effects may defer generically and later complete under the original EffectId

## Consequences

Deferred work keeps original identity through external completion.

## Rejected alternatives

Feature-specific background-job state machines that replace original EffectId identity.

## Compatibility and schema-change classification

Journal/effect schema family; original EffectId preserved across deferral.

## Security classification

- References: SEC-INV-003, SEC-INV-004, SEC-INV-008; TM-10, TM-14
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-DUR
- Affected Technical Design: Technical Design §12–§13 (generic deferred effects)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-008, PR-014, PR-042–PR-044, PR-048
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial: PR-014 merge
  `399f3a7d9d987268f4d79ab90f31b93f854084f8` authenticates, replays, validates,
  and routes deferred external completion under the original `EffectId`
  (PR-014-E-runtime-f7d0f0ecede0; PR-014-E-security-a8a835efcf19;
  PR-014-E-integration-e7ec699722ab).
  PR-042 local merge `4d627711632c771d733b689e1325b2d9679ee317` adds model-effect
  runtime lifecycle, same-identity retry, and fail-closed conflict under that
  `EffectId` (PR-042-E-resume-d69e4a140a85; PR-042-E-security-d69e4a140a85;
  PR-042-E-integration-4d627711632c).
  PR-043 candidate `86f71c8fd47c0c6d721f9ae36df5c0ba90b22025` adds
  tool-effect runtime lifecycle under that `EffectId`
  (PR-043-E-resume-86f71c8fd47c; PR-043-E-security-86f71c8fd47c).
  PR-044 candidate `e56d6f1d0638986d1201f2b901f372d64d01d062` continues the
  interaction path under the original `EffectId`
  (PR-044-E-lifecycle-e56d6f1d0638; PR-044-E-security-e56d6f1d0638).
  PR-045 candidate `e58ff33138dd1c4d5c1be9e611575b12cd3a4ac5` continues
  cancel-during-deferred restore under that `EffectId`
  (PR-045-E-cancel-e58ff33138dd; PR-045-E-security-e58ff33138dd).
  Later persistent completion paths remain mapped to PR-048.
  Verified — PR-048 crash-prefix restore and
  G5-D-durable-beta-a9568bd869b5 close the mapped durability work.
