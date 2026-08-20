# Policy Instructions Middleware — Design

**Date:** 2026-08-20
**Status:** Approved for planning
**Plan:** `docs/superpowers/plans/2026-08-20-policy-instructions-middleware.md`

## Problem

Tenants need per-run policy text injected into the model context: a compliance
footer, an as-of date, a locale tag, tenant rules. This is not retrieval — a
`ContextProvider` retrieves items in response to a `ContextRequest`; this
feature injects fixed, application-authored policy. The natural seam is the
existing `prepare_context` middleware stage, which already supports the
`AddInstructions` outcome (`ports/middleware/validate.rs:97-99`) but has no
production participant today.

## Decision

A new leaf crate `extensions/middleware/finstack-ai-middleware-instructions`
exposing `InstructionsMiddleware`: an ordinary `Middleware`
(`MiddlewareRole::Standard`, `OrderTier::Standard`) that declares only
`Stage::PrepareContext` and returns
`StageOutcome::AddInstructions(Arc<[ContextItem]>)` with items precomputed at
construction. Component id: `finstack.middleware.instructions`.

### Why prepare_context + AddInstructions (not a provider, not BeforeModel)

- **Not a `ContextProvider`.** Providers model retrieval (collect/reconcile/
  reconstruct, budgets, effect ids). Policy text has no retrieval semantics;
  a provider would carry dead contract surface.
- **Runs strictly before compaction.** The `ContextCompactor` role is unique
  per chain and runs at `BeforeModel` in `OrderTier::ContextCompaction`.
  Instructions landed at `prepare_context` are already in the array the
  compactor sees.
- **Protected without fighting the compactor.** `apply_context_prepared`
  (`exec/stage_settlement/apply.rs:208-227`) mints `AddInstructions` items as
  `MessageRole::System` messages. System messages are structurally protected
  (`exec/context_driver/collect.rs:224-248`), so a compactor must preserve
  them byte-identically (`ports/middleware/validate.rs:198-205`). No ordering
  edges against the compactor are needed.

### Placement (final semantics)

At `prepare_context`, `apply_context_prepared` inserts `AddInstructions`
items (as System messages) and `AddContext` items (as User messages)
**immediately before the trailing current-user message**, mirroring the
provider convention in `exec/context_driver/collect.rs::rebuild_messages`.
The current user message therefore remains **last and structurally
protected**, which is what keeps a `BeforeModel` `CompactContext` landable:
`validate_compaction_result` requires the last source entry to be a
protected user. The compactor must preserve the injected System messages
byte-identically. When the array is empty or does not end with a user
message (e.g. after a `Replace` reshaped it), additions fall back to
appending at the tail — the fold does not invent structure. An earlier
draft appended at the tail unconditionally; that broke composition with any
registered compactor and was rejected in final review. Leading-prefix
placement (ahead of pre-existing system messages) remains out of scope.

## Configuration

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyEntry {
    /// Stable label; becomes `ContextProvenance.source_id` ("policy:{label}").
    pub label: String,
    /// The instruction text injected as a System message.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyInstructionsConfig {
    pub entries: Vec<PolicyEntry>,
}
```

- As-of date, locale, tenant rules are all just entries; the application
  composes them at agent-build time (e.g. `PolicyEntry { label: "as-of",
  text: "Treat 2026-08-20 as the current date." }`). The middleware stays
  **pure and deterministic**: no Clock, no locale lookup, no store — the
  values are frozen into the config, per the runtime purity requirement
  (`exec/middleware_driver/mod.rs:33-46`), and `InvocationRecovery::RecomputeSafe`
  is sound.
- `configuration_digest = Digest::raw_json(serde_json::to_vec(&config))`
  (same convention as the compaction leaf), so two tenants with different
  policies produce different digests.
- Validation at `try_new`: 1..=16 entries (fold caps
  `instructions + context ≤ SEMANTIC_ARRAY_MAX_ITEMS` per stage; 16 leaves
  headroom for other participants), non-empty label/text; item construction
  errors surface as a stable `Configuration` error.

## Item shape

One `ContextItem` per entry, built once in `try_new`:
`kind: Instruction`, `authority: TrustedApplication`, `priority: 0`,
`estimated_tokens: text.len()/4`, `sensitivity: Internal`, `protected: true`,
`provenance: { source_id: "policy:{label}", source_ref: None, external: false }`.

## Out of scope

- Dynamic per-request templating (would break purity/digest stability).
- Prefix placement via `Replace`.
- Any store, capture, or observer integration.
