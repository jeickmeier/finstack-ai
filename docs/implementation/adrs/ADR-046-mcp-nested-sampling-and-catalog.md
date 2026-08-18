# ADR-046: MCP nested sampling and catalog re-resolve

## Status

Accepted

## Date

2026-08-18

## Accountable role

Core/runtime lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

MCP `sampling/createMessage` inverts control: a committed tool asks the
host to run the parent agent's model and return a message. The MCP
toolset currently rejects sampling. Mid-run `list_changed`
notifications are observer-only and must not mutate a live
`ResolvedAgentLock`.

Sampling must not invent a seventh port or a new `RecordBody`. The
parent tool effect already exists. The nested model request is a child
effect of that tool. Catalog growth after connect is a new agent
composition, not a live patch.

## Decision

Sampling is a child model effect of the committed tool.

- `EffectRelation.parent_effect_id` is the parent tool `effect_id`.
- New purpose variant: `EffectPurpose::NestedModel { kind: McpSampling }`
  in `crates/finstack-ai-kernel/src/effects/kinds.rs`.
- The nested request uses `validate_model_request`, the locked context
  profile, and the parent budget. Missing lock or budget rejects
  sampling with today's configuration/budget error.
- Crash-resume uses the ordinary model reconcile path. No special
  sampling port.

`list_changed` does not change `ResolvedAgentLock` mid-run. Observer
emission stays.

Re-resolve is a **new** agent composition. Frozen name:
`Agent::re_resolve`. The MCP factory reconstructs a new lock via
`McpToolsetFactory::reconstruct`. In-flight runs keep the old lock.

Subscriptions refresh only names already present in the frozen
`resources/list` snapshot. Unknown names fail closed. Later `collect`
re-reads those URIs.

No new `RecordBody`. No seventh port.

## Consequences

- Journals that record `NestedModel` are additive 1.1. 1.0.0 readers
  that do not know the variant fail closed. That is expected. This is
  not a 1.0.0 meaning change.
- Both bindings expose `re_resolve`. wasm-host is a thin map over the
  same Rust method.
- Live catalogs stay frozen for the life of a resolved agent.

## Rejected alternatives

**Seventh port for sampling.** Rejected: the Model port already runs
committed model effects.

**New `RecordBody` for sampling.** Rejected: existing
`EffectRequested(Model)` plus `EffectRelation` is sufficient.

**Mutate `ResolvedAgentLock` on `list_changed`.** Rejected: lock
identity is composition-time; mid-run mutation is not a live catalog
patch.

**Subscribe to names absent from the frozen list.** Rejected: fail
closed on unknown names.

**Python-only or WASM-only `re_resolve`.** Rejected: one Rust method;
bindings only map arguments.

## Compatibility and schema-change classification

Additive `EffectPurpose` variant. Existing `CompactionSummary` meaning
is unchanged. No new record kind. 1.0.0 readers fail closed on unknown
purpose tags. Treat the purpose enum as a compatibility event with
this classified additive row; do not invent a 1.0.0 meaning change.

## Security classification

- References: SEC-INV-003, SEC-INV-004; TM-10 (nested model stays a
  committed child effect under the parent tool and parent budget);
  TM-01 (catalog lock does not mutate mid-run)
- Threat Model review trigger: complete nested-model commit,
  budget-charge, and fail-closed subscribe tests with D5–D6. This
  record does **not** invent a review id.
- Residual: sampling still requires an installed model lock; missing
  lock stays rejected.

## Affected requirements, design, and delivery

- Affected Technical Design: six ports, commit-before-effect, exact
  lock. Planning files are not edited.
- Implementation: D5 MCP sampling; D6 subscribe / `re_resolve`
- Unchanged: no seventh port; no new `RecordBody`; observer-only
  `list_changed`

## Supersession metadata

None. This ADR supersedes no prior ADR.

## Reconsideration conditions

May change only through a new superseding ADR. Adding a seventh port,
a new `RecordBody`, live lock mutation, or a binding-only `re_resolve`
requires that path.

## Approval and implementation-evidence links

- Approval: accepted by the decision owner to execute D5–D6 locally
  only, without publication
- Implementation evidence: Partial (local D5 `eafd8ea` / `bcc35b0`,
  D6 `b25fd95` / `66d108b`; no invented evidence or review id)
