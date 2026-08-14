# ADR-019: browser host adapter

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

Browser host interfaces remain canonical and an optional fetch/SSE adapter ships as a battery

## Consequences

Browser credentials stay in same-origin proxies; host adapters remain the contract.

## Rejected alternatives

Embedding provider credentials in browser bundles or making fetch/SSE the only contract.

## Compatibility and schema-change classification

Browser host ABI and optional adapter surface; host interfaces remain canonical.

## Security classification

- References: TM-05
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-WASM
- Affected Technical Design: Technical Design §26 (browser host adapter)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-034, PR-038
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

### PRD §18 delivery detail

- Product decision: Keep browser host interfaces as the contract and ship one optional fetch/SSE OpenAI-compatible adapter for same-origin proxies.
- Delivery point: PR-034.
- Source: [PRD §18](../../planning/01-finstack-ai-product-requirements.md#18-resolved-foundational-product-decisions)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Implemented — PR-034 host ABI and same-origin fetch/SSE battery at local merge `6bd1979c7ae01bd0cf497ba434abca3c9f72dea2`; PR-035 Agent/Run handles at local merge `a7d47fad5cc0757f03f6d56d9faebde97c693244`; PR-038 local merge `04407192289c24cbfb087357b1e2ca928a8f3b55` adds Chromium/Firefox/WebKit goldens, browser-security.md, and staged unpublished npm 0.0.2. G4 remains a separate named decision.
