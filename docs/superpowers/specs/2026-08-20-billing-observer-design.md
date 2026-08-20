# Billing ledger observer design: `finstack-ai-observer-billing`

- **Date:** 2026-08-20
- **Status:** Approved for planning
- **Source of truth for the shape:** `docs/planning/05-finstack-ai-future-capabilities-design-validation.md` §2.2 (billing is an `Observer` use case), §3.6 (pricing and shared-ledger policy stay out of the kernel), §18.7 (observers are best-effort; correctness-critical facts live in journal records); `docs/planning/03-finstack-ai-technical-design.md` observer contract (line ~3366).

## 1. Overview

A read-only observer leaf crate that aggregates the kernel's typed `CostAmount`
(integer micros + unit + `pricing_policy_version`, from
`crates/finstack-ai-kernel/src/primitives/usage.rs`) into an in-memory spend
ledger keyed by **session, run, model, unit, and pricing policy version**. The
existing `finstack-ai-observer-metrics` leaf only sums global token counters;
this crate is the FinStack-shaped spend projection.

```text
finstack-ai-observer-billing (extensions/observers/finstack-ai-observer-billing)
  BillingObserver        — Observer port adapter (component id finstack.observer.billing)
  LedgerSnapshot         — point-in-time projection: spend rows + usage rows + coverage counters
  SpendEntry / UsageEntry— per-(session, run, model[, unit, policy]) aggregates
  export_jsonl()         — deterministic JSONL export; micros rendered as decimal strings
```

## 2. Money rules (non-negotiable)

1. **No floating point anywhere in the money path.** Micros accumulate in
   `u128` (`saturating_add` from `u64` inputs; a real overflow is unreachable
   but must not wrap or panic). Token counters stay `u64` saturating.
2. **Never merge across `unit` or `pricing_policy_version`.** A spend cell is
   keyed by both; "1_000_000 USD-micros under prices-v1" and "… under
   prices-v2" are different rows forever.
3. **Never synthesize cost.** If `Usage.cost` is absent, the effect is counted
   in `uncosted_effects` for its attribution key — no pricing table, no
   token→cost conversion. Cost emission is the model provider's job
   (`Usage.cost` on `EffectCompleted`); today no in-repo provider emits it, so
   against current providers the ledger reports usage rows and
   `uncosted_effects` honestly instead of fake zeros.
4. **Exports render micros as decimal integer strings**, matching the kernel's
   canonical `CostAmount` JSON serialization — never as JSON numbers that a
   consumer might parse into a double.

## 3. Semantics

### 3.1 Attribution (session, run, model)

Every `RunEvent` carries `session_id` and `run_id`. Model identity is not a
typed field on events, so the observer correlates:

- On `EffectRequested` whose body is `EffectRequested` with
  `EffectInput::Model { request }`: record `effect_id → EffectOrigin`, where
  `EffectOrigin` holds the provider `ComponentId` + `Version` (from
  `ComponentInvocation`, if present) and the model name parsed from the
  canonical request JSON's top-level `"model"` string member (if present).
  Non-model effects are ignored for origin tracking (tool/context effects can
  still complete with usage; they aggregate with `model: None`).
- On `EffectCompleted` / `EffectFailed`: remove the pending origin entry
  (settlement consumes it for attribution — `EffectFailed` carries usage too
  and is aggregated the same way).
- On `EffectCancelled`: remove the pending origin entry (the effect will never
  settle, so nothing should keep referencing it).
- On `EffectDeferred`: the pending origin entry is **kept**, not removed.
  Deferral followed by eventual completion under the same `EffectId` is the
  kernel's intended lifecycle for long-running effects — the origin recorded
  at request time must still be present when the deferred effect finally
  settles.

The pending-origin map is bounded (`MAX_PENDING_EFFECTS = 4096`). When full, a
new model effect's origin still gets tracked: the **oldest** pending entry is
evicted first (`EffectId` is UUIDv7-shaped, so map order approximates
insertion time) to make room, and a `billing_pending_saturated` diagnostic is
stored. The evicted entry's eventual completion aggregates under `model: None`
and bumps `unattributed_effects`; no origin request is ever silently dropped
on arrival.

`unattributed_effects` counts only **model** effects (output contract kind
`ModelResponse`) that settle (`EffectCompleted`/`EffectFailed`) without a
tracked pending origin — whether because the origin was never recorded (no
matching `EffectRequested`) or because its pending entry was evicted to make
room under the bound. Non-model (tool, context) effects settling with no
pending origin is expected by design — they are never origin-tracked — so it
never bumps `unattributed_effects`; they still aggregate usage/effects/
uncosted under `(session, run, None)` per §3.2.

A model request whose canonical JSON has no usable top-level `"model"` string
(missing, empty, oversized, or containing a NUL byte, per the kernel's shared
label validity rule) still gets a pending entry — with `model: None` — and a
`billing_model_name_invalid` diagnostic is stored. Its eventual settlement
aggregates under `model: None` like any other effect with no model name; this
is not counted as unattributed, since the origin (including its `None`
provider/model) was tracked.

### 3.2 Aggregation

On `EffectCompleted` (and `EffectFailed`) with `usage`:

- key = `(session_id, run_id, model)` where `model` is the origin's model name
  or `None`.
- Always: `input_tokens += usage.input_tokens().unwrap_or(0)` (saturating),
  same for output; `effects += 1`.
- If `usage.cost()` is `Some(cost)`: the spend cell
  `(unit, pricing_policy_version)` under that key gets
  `micros += u128::from(cost.micros())`, `costed_effects += 1`. The provider
  `ComponentRef` seen first for the key is retained for reporting — spend from
  the same `(session, run, model)` key later served by a different provider is
  still attributed to that first provider (first-provider-wins).
- Else: `uncosted_effects += 1` on the key.

The attribution map is bounded (`max_entries`, constructor argument, clamped to
`1..=1_000_000` like the queue). When full, events for **new** keys are counted
in a global `overflowed_events` counter and a `billing_ledger_saturated`
diagnostic is stored; existing keys keep aggregating. Nothing is evicted —
a billing projection must not silently lose committed spend for keys it has.

### 3.3 Observer contract

- Payload handling follows the `finstack-ai-observer-metrics` precedent:
  descriptor declares `ObserverPayloadMode::MetadataOnly`; ingest reads only
  numeric usage/cost fields, identifiers, and the `"model"` string — it never
  retains message content, tool payloads, or request bodies, and the JSONL
  export contains identifiers and integers only (canary-tested).
- Best-effort per §18.7: this ledger is a **projection for operators and spend
  dashboards**, not the invoicing source of truth. Authoritative billing must
  re-derive from journal records (`EffectCompleted.usage` + `usage_digest`);
  the README states this explicitly.
- Backpressure/diagnostics mirror the metrics leaf: bounded `ObserverQueue`,
  `dropped()`, `last_diagnostic()`, `OBSERVER_QUEUE_OVERFLOW` on queue drops.
- No Prometheus surface. Per-session/run labels are unbounded cardinality;
  the export is a snapshot API plus JSONL. Operators who want Prometheus keep
  using the metrics leaf.

## 4. Public API (complete)

```rust
pub enum BillingObserverError { Configuration { reason: &'static str } }

pub struct BillingObserver { /* descriptor, queue, Mutex<LedgerState>, max_entries, Mutex<Option<ObserverDiagnostic>> */ }

impl BillingObserver {
    /// queue_capacity, backpressure as the metrics leaf; max_entries bounds distinct
    /// (session, run, model) attribution keys, 1..=1_000_000.
    pub fn try_new(queue_capacity: usize, backpressure: ObserverBackpressure, max_entries: usize)
        -> Result<Self, BillingObserverError>;
    pub fn dropped(&self) -> u64;
    pub fn last_diagnostic(&self) -> Option<ObserverDiagnostic>;
    pub fn snapshot(&self) -> LedgerSnapshot;
    pub fn export_jsonl(&self) -> String;
}
impl Observer for BillingObserver { /* descriptor(), observe() — sync ingest under mutex, Box::pin(async Ok) */ }

pub const BILLING_LEDGER_SATURATED: ObserverDiagnostic; // code "billing_ledger_saturated"
pub const BILLING_PENDING_SATURATED: ObserverDiagnostic; // code "billing_pending_saturated" (oldest pending origin evicted)
pub const BILLING_MODEL_NAME_INVALID: ObserverDiagnostic; // code "billing_model_name_invalid"

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendEntry {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub model: Option<Arc<str>>,
    pub provider: Option<ComponentRef>, // first provider seen for the key (first-provider-wins)
    pub unit: Arc<str>,
    pub pricing_policy_version: Arc<str>,
    pub micros: u128,
    pub costed_effects: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageEntry {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub model: Option<Arc<str>>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub effects: u64,
    pub uncosted_effects: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LedgerSnapshot {
    pub spend: Vec<SpendEntry>,      // deterministic order: key BTreeMap iteration
    pub usage: Vec<UsageEntry>,
    pub unattributed_effects: u64,
    pub overflowed_events: u64,
    pub dropped_events: u64,         // observer queue drops
}
```

JSONL export: one object per spend row then per usage row, e.g.
`{"kind":"spend","session_id":"…","run_id":"…","model":"gpt-x","provider":"finstack.model.openrouter@1.0.0","unit":"USD","pricing_policy_version":"prices-v1","micros":"1250000","costed_effects":"3"}` —
all numerics as decimal strings, trailing summary line
`{"kind":"summary","unattributed_effects":"0","overflowed_events":"0","dropped_events":"0"}`.

## 5. Non-goals

- **Pricing tables / cost computation.** Providers own `Usage.cost` emission;
  a follow-up may add cost to `finstack-ai-model-openrouter`, not here.
- **Authoritative invoicing or persistence.** In-memory projection only;
  durable export is an external pipeline over `export_jsonl()`/journal replay.
- **Currency conversion or cross-unit totals.**
- **`BudgetScopeId` reserve/charge ledger** (`records/policy/budget.rs`) —
  that is the runtime budget service; this crate only observes recorded usage.
- **Kernel/runtime changes.** Zero. Existing `Observer` port only.

## 6. Crate placement and dependencies

`extensions/observers/finstack-ai-observer-billing`, workspace member +
`[workspace.dependencies]` entry. Deps: `finstack-ai-kernel`,
`finstack-ai-runtime` (default-features off, `native-tokio` — same as metrics),
`serde_json` (parse the model request's `"model"` member from canonical
`RawJson`), `thiserror`. Dev-deps: `finstack-ai-test`, `tokio` (rt, macros).
Same lint header as the metrics leaf. Public-api baseline regenerated via
`uv run --no-project python scripts/compat/public_items.py --write`
(`scripts/compat/public_api.py` auto-globs `extensions/*/**/Cargo.toml`).
