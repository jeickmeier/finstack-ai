# ADR-038: ciborium test interop

## Status

Accepted

## Date

2026-08-17

## Accountable role

Durability/ecosystem lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

ADR-015 freezes the v1 canonical-CBOR profile. Technical Design §28 still
names `ciborium` behind a project-owned codec wrapper. Production
encode/decode in `finstack-ai-protocol` already uses a project-owned writer;
`ciborium` is used only for a public `to_ciborium()` helper and one
structural interop test. Treating the library writer as part of the public
protocol surface keeps an unused production dependency and a removable
public item.

This record does not edit `docs/planning/` and does not supersede ADR-015.

## Decision

The project-owned protocol codec is the canonical CBOR writer. `ciborium`
is test-only interop for structural comparison against the library value
tree. `to_ciborium` is not part of the public protocol API.

ADR-015 remains the profile: definite lengths, shortest lossless
integer/float forms, RFC 8949 map-key order, finite floats with preserved
negative zero, no bignum tags 2/3, no duplicate keys, and v1 depth/size
limits. The library encoder is not an end-to-end canonical serializer.

## Consequences

`ciborium` moves to protocol `[dev-dependencies]`. Production encode/decode
paths do not import it. Journal, WIT, and protocol meaning are unchanged.
Unexporting `to_ciborium` is a protocol-crate public removal; it is not a
frozen kernel/runtime/SDK public item.

## Rejected alternatives

Keeping `ciborium` and `to_ciborium` on the production public surface so
TDD §28's crate name remains a runtime dependency.

Replacing the project-owned writer with `ciborium::ser::into_writer`.

## Compatibility and schema-change classification

Protocol crate public-API cleanup only. Canonical journal bytes, diagnostic
JSON/JSONL, and the ADR-015 profile are unchanged.

## Security classification

- References: SEC-INV-007, SEC-INV-010; TM-09, TM-12, TM-16
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: the canonical writer and
  profile stay project-owned; only the unused public interop shim and
  production crate edge are removed.

## Affected requirements, design, and delivery

- Affected requirements: FR-DUR, NFR-COMP
- Affected Technical Design: Technical Design §28 (canonical CBOR). TDD
  §28 still names `ciborium`; this ADR records that the wrapper is the
  writer and the crate is test interop only. Planning files stay read-only.
- Architecture Specification §25 decision summary remains ADR-015
- Does not supersede ADR-015

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded. It
specializes ADR-015's implementation dependency without changing the
profile.

## Reconsideration conditions

May change only through a new superseding ADR. Restoring `ciborium` as a
production writer, or treating its encoder as canonical, requires that
supersession plus reconciliation of every affected primary planning
document.

## Approval and implementation-evidence links

- Approval: accepted as implementation change control for demoting
  `ciborium` to test-only interop (2026-08-17)
- Implementation evidence: pending protocol unexport of `to_ciborium` and
  `ciborium` `[dev-dependencies]` move
