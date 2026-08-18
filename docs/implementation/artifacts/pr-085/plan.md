# PR-085 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-085. Closed PR-001–PR-084
envelopes are not reused. PR-085 is the only active logical PR.
This file authorizes Phase 14 sequencing; it does not admit
PR-086–PR-098 code.

## ID remapping

The extensions remediation draft named this work Phase 13 /
PR-080–PR-093 and pack v0.26 / PLAN-0.24. Those IDs were already
consumed on `main` by kernel remediation (Phase 13 / PR-080–PR-084,
pack v0.26 / PLAN-0.24). Residual risk 8 on
[`pr-080/plan.md`](../pr-080/plan.md) required a stop and reconcile.

This envelope remaps the draft 1:1:

| Draft | This baseline |
| --- | --- |
| Phase 13 | Phase 14 |
| PR-080–PR-093 | PR-085–PR-098 |
| PLAN-0.24 / pack v0.26 | PLAN-0.25 / pack v0.27 |
| ADR-047 / ADR-048 | unchanged (those IDs were free) |

Do not overwrite [`pr-080/plan.md`](../pr-080/plan.md).

## Purpose

Authorize pre-1.0 extensions remediation as Phase 14 and record the
planning-pack amendment. This slice lands ADR-047, ADR-048, the
cargo-public-api freeze gate, and `verify_authority` on the runtime
Tool port. It does not delete the gateway, migrate leaf crates, or
retarget `Agent::gateway`.

## Principal changes

- Add Implementation Plan §17E with PR-085–PR-098.
- Update §5.1 critical-path graph and §22 traceability.
- Bump Plan 0.24→0.25 and pack v0.26→v0.27.
- Author ADR-047 and ADR-048. Update the ADR register and record
  directory.
- Write this envelope. Do not write PR-086–PR-098 envelopes here.
- Pin `cargo-public-api` 0.52.0 and check-only `nightly-2025-08-02`.
  Do not set `RUSTC_BOOTSTRAP`.
- Add `mise run check-public-api` and baselines under
  `fixtures/compatibility/public-rust-api/cargo-public-api/`.
- Land `verify_authority(&ToolCallContext)` on the runtime Tool port.
- Land `provider_util` `SecretString` / credential types on the
  runtime crate root without retargeting `Agent::gateway`. Leaf
  re-exports wait.

## Acceptance mapping

Five Implementation Plan bullets map 1:1 to A01–A05.

- PR-085-A01: Implementation Plan 0.25 contains §17E with
  PR-085–PR-098, each with Purpose, Principal changes, Acceptance
  evidence, Dependencies, Explicitly excluded, and Traceability.
- PR-085-A02: Pack README is v0.27 and lists Implementation Plan 0.25.
- PR-085-A03: ADR-047 and ADR-048 exist. `adr-register.md` and
  `adrs/README.md` index them. ADR-047 keeps `Agent::gateway` and makes
  `openai_chat` a configuration error. ADR-048 rejects
  `ReceiverModelStream`, `apply_budget`, and `tool_catalog_digest`.
- PR-085-A04: This envelope records the ID remapping. Kernel
  `pr-080` through `pr-084` envelopes are unchanged.
- PR-085-A05: `mise run check-public-api` exists. Rust `pub use`
  scrape is retired or delegated. `verify_authority` is a public
  runtime add visible to the new gate. NFR-PORT-* is unchanged.

## Explicit exclusions

PR-086–PR-098 leaf code. Gateway deletion. `Agent::gateway` retarget.
Leaf `SecretString` / `CredentialStore` usage. e2b compensating DELETE.
`AgentInvoker::join`. Temporal engine. Encoder centralization.
NFR-PORT-* amendment. Overwriting Phase 13 / PR-080–PR-084.

## Compatibility class

Additive public runtime function. Docs / tooling for the freeze gate.
Pre-1.0 extensions baselines are new.

## Dependencies

Phase 9 / G8. Phase 13 / PR-080–PR-084 may remain in progress. Pack
v0.26 is already landed. Phase 10 / PR-067 may remain in progress.

## Suggested successor sentence

When this authorization slice is the admitted baseline:

```
Run PR-086; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

Do not infer PR-087–PR-098 admission from this file.
