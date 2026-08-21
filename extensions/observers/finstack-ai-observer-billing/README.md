# finstack-ai-observer-billing

Read-only spend-ledger observer. Aggregates the kernel's typed `CostAmount`
(integer micros, unit, `pricing_policy_version`) from settled effects into a
ledger keyed by session, run, and model. Integer money only — no floats, no
pricing tables, no cross-unit totals.

This ledger is a best-effort **projection** for dashboards and operators.
Observer delivery is best effort; authoritative billing must
re-derive spend from journal records (`EffectCompleted.usage` and its
`usage_digest`). Cost appears only when the model provider emits
`Usage.cost`; effects without a cost are reported as `uncosted_effects`,
never priced locally.

```rust
use std::sync::Arc;

use finstack_ai_observer_billing::BillingObserver;
let billing = BillingObserver::try_new(10_000).expect("billing observer");
// Register with AgentBuilder::observer(component_ref, Arc::new(billing)), then:
// let snapshot = billing.snapshot();
// let jsonl = billing.export_jsonl();
let _ = Arc::new(billing);
```

`last_diagnostic()` surfaces `billing_ledger_saturated` when the entry bound is
hit; new attribution keys are dropped while existing keys keep aggregating.
