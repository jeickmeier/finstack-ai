# PR-057 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-057-observer-diagnostics`
Intended baseline: local `main` at `6f4ecfcde5df1e6919c3da121f4b7d8b65df62db`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-057. The closed PR-054
envelope is not reused. The PR-055 and PR-056 envelopes are not
reused. PR-057 is the only active logical PR once admitted. This
planning file does not admit the PR, start Phase 8, or record Phase
8 entrance.

## Execution envelope

Not authorized. Suggested text when the owner is ready:

```
Run PR-057; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

`implement the plan` is enough only if it names that same local-only
integrated envelope **and** the admission checks below are already
true. Do not infer authorization from this planning file, from
`continue`, from Phase 7 `Done`, from G6 `Passed`, or from the
PR-055/PR-056 plans existing.

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, crates.io
publish, tag, G5 inference, G7 inference. Do not write `G5-D-*` or
`G7-D-*`. Do not start PR-055, PR-056, or PR-058+. Do not cut or
publish `0.1.0`. Do not bump the lockstep workspace version off
`0.0.4`.

## Admission (when authorized)

Do not admit coding until all of the following are true. Planning
this file does not record them.

### Phase 8 entrance is `Passed` (2/2)

Recorded by PR-055. Current state is `Passed` (2/2).

| Entrance bullet | Current state |
| --- | --- |
| Native preview, Python alpha, WASM alpha, durability beta, plugin alpha | **Satisfiable.** G3/G4/G5/G6 are named (`G5-D-durable-beta-a9568bd869b5`). Do not record `PH8-E-entrance-gates-*` until PR-055 is admitted. |
| Public API change backlog triaged | **Blocked.** `docs/implementation/public-api-change-backlog.md` does not exist. |

Do not infer G5 from Phase 6 `Done`. Do not write the backlog or
`G5-D-*` from this PR. If the owner says `implement the plan` while
entrance is still `0/2`, **stop**.

Phase 8 entrance, once Passed, is recorded by the first admitted
Phase 8 PR. Do not re-record `PH8-E-entrance-*` here.

### PR-055 and PR-056 are `Done`

Both are planned only and still `Todo`. Implementation Plan lists
PR-055, PR-056, then PR-057. Keep one active logical PR. Do not
admit PR-057 while either predecessor is `Todo` unless the owner
explicitly authorizes parallel Phase 8 work in the same sentence.

PR-057's *code* dependencies are PR-018 and stable event envelopes
(PR-017 hub + PR-009/PR-039 shapes). Those are already on `main`.
`Observer`, `ObserverPayloadMode`, `ObserverEventView`,
`NoopObserver`, `ReferenceObserver`, and
`check_observer_conformance` already exist. Compaction *metric
names* come from TDD §17.6 `CompactionEvidence`; they do not
require the PR-056 leaf to compile, but redacted compaction
diagnostics should be proven against whatever compaction evidence
is on `main` at admit (PR-018 recorded outcomes, plus PR-056 if it
has landed).

### Other admission checks

- No Implementation Plan section 6.3 ADR trigger applies if the work
  adds no seventh port, does not move I/O into the kernel, and does
  not add `opentelemetry` / Prometheus exporters to kernel, runtime,
  or the default SDK.
- Threat Model section 18 is triggered (telemetry payloads /
  retention). Primary **TM-17** and **TM-04** / **SEC-INV-005**.
  Also **TM-21** (compaction diagnostics must not leak
  source/summary content) and **SEC-INV-009/012**. Complete the
  review before merge.
- ADR-009 (observer/middleware separation) stays Verified for the
  port split. This PR does not reopen `after_run`.
- Do not treat `SecurityAuditSink` as an `Observer` (TDD §19).

## Traceability

Implementation Plan PR-057; FR-OBS-001–004; FR-MW-007 (observers
do not own compaction); Architecture §11.5, §19, §21; TDD §19–20,
§31.4; ADR-009.
TM-04 / TM-17 / TM-21; SEC-INV-005 / 009 / 012.
G7 is out of scope.

## Acceptance mapping

Five Implementation Plan bullets map 1:1 to A01–A05.

- PR-057-A01: Observer failure does not change agent semantics. A
  slow, panicking, or `Err` observer cannot block
  `EffectRequested`, alter a terminal record, or change golden
  traces. Reuse the PR-017 isolation proof: the observer route
  never waits on source fan-out. New leaf tests: log/otel/metrics
  `observe` returning error or blocking past the bounded queue
  still yields the same `RunCompleted` / journal prefix as
  `NoopObserver`.
- PR-057-A02: Secrets and protected tool payloads are redacted by
  default from observers and diagnostic/export projections.
  Default adapter mode is `ObserverPayloadMode::Redacted` (or
  `MetadataOnly` for metrics). Canary secrets, `Sensitivity::Secret`
  / `Credential`, and protected tool bodies never appear in log
  lines, OTel attributes, Prometheus labels, JSON/JSONL dumps, or
  support-bundle files. `MetadataOnly` keeps session/lane/run/
  turn/effect/tool-call IDs and kinds. Authoritative journal CBOR
  is unchanged (no field-level redaction of records).
- PR-057-A03: Exporter backpressure is bounded and diagnosed.
  Each adapter owns a finite queue (no v1 spill-to-disk). Map TDD
  `ObserverBackpressure::{BlockBounded, DropProgress, Disconnect}`
  as **adapter config**, not a kernel trait change. Overflow
  emits a diagnostic (not a `RunEvent` kind) and either drops
  progress, disconnects the exporter, or times out the bounded
  block. Queue depth is a metric. Unbounded channels are forbidden.
- PR-057-A04: The minimal bundle contains no telemetry exporter.
  `cargo tree` of `finstack-ai-kernel`, `finstack-ai-runtime`,
  default `finstack-ai`, `finstack-ai-wit`, and
  `finstack-ai-native-examples` (without an explicit observer
  feature) has no `opentelemetry`, `opentelemetry-otlp`,
  `tracing-opentelemetry`, `prometheus`, or the three new observer
  crates. `NoopObserver` / `ReferenceObserver` stay in runtime for
  tests. SEC-INV-012.
- PR-057-A05: Compaction diagnostics expose no compacted
  source/summary content by default. Metrics/events may carry
  trigger reason, strategy/version, estimated tokens before/after,
  checkpoint hit/miss/invalidation, summary usage/latency,
  failure/fallback, and `PromptCacheImpact`. They must not carry
  replacement message text, summary text, or covered-entry
  payloads. Observer failure cannot alter compaction behavior
  (same isolation as A01).

Principal changes that are not extra acceptance IDs, but are
required to prove the five bullets:

- Structured log observer crate.
- OpenTelemetry observer crate.
- Prometheus / reference metrics crate.
- Shared redaction + metadata-only policy for observer streams,
  diagnostic JSON/JSONL, support bundles, and journal *export
  projections*.
- Runtime status / queue / latency / usage / recovery metrics.
- Content-redacted compaction metric set.
- Trace examples and observer conformance tests.

## Locked design

### Layout

Follow Technical Design §2. Implement the two named placeholders
and add one sibling metrics package (PR text: "separate packages";
FR-OBS-003: logs, OpenTelemetry, metrics, test capture). Test
capture is already `ReferenceObserver` in runtime — do not
duplicate it.

```text
extensions/observers/finstack-ai-observer-log/       # implement placeholder
extensions/observers/finstack-ai-observer-otel/      # implement placeholder
extensions/observers/finstack-ai-observer-metrics/   # NEW sibling
extensions/observers/README.md                       # authoring + redaction
```

Do not put exporters in `crates/`. Do not add a hosted collector.
Do not invent `extensions/telemetry/`.

Workspace members + `[workspace.dependencies]` at `0.0.4`.

### Do not reshape the port

`Observer::observe(&self, batch: Arc<[RunEvent]>)` stays. Project
with existing `ObserverEventView::from_event` and
`ObserverPayloadMode`. Default new adapters to `Redacted`.
Metrics default to `MetadataOnly` (labels are identifiers only).

`ObserverBackpressure` is TDD operational config and is **not**
on the trait today. Keep it on each adapter's constructor. Do not
add a seventh port or an eighth middleware stage. Do not add
`RunEventKind` values for diagnostics (TDD §20.2).

### Structured log (`finstack-ai-observer-log`)

- One JSON object per event (or per batch line), stderr or a
  caller-supplied `Write` / tracing subscriber.
- Fields: correlation IDs from `ObserverEventView`, `class`,
  `kind`, `sensitivity`, optional redacted body.
- No `opentelemetry` dependency.
- Prefer `tracing` only if it stays in this leaf; do not add
  `tracing` to kernel/runtime.
- Failures return `ObserverError` and are swallowed by the
  existing hub (A01).

### OpenTelemetry (`finstack-ai-observer-otel`)

- Maps durable-derived events to spans/events; transient progress
  to events or metrics, never to new journal kinds.
- Span names are stable (`finstack.run`, `finstack.effect`,
  `finstack.tool`). Attributes are correlation IDs + kind +
  error code. Bodies follow `payload_mode`.
- OTLP exporter is optional and feature-gated
  (`otlp`). Default crate feature is in-memory / test exporter
  only so `cargo test` needs no collector.
- Bounded export queue inside the crate. Default
  `DropProgress` for transient, `BlockBounded` with a short
  timeout for durable-derived; timeout → diagnostic + drop, not
  a run failure.
- Confirm crate versions at implementation time against current
  OpenTelemetry Rust docs. Pin in workspace.deps. Do not enable
  a default network exporter.

### Prometheus / reference metrics (`finstack-ai-observer-metrics`)

- Pull-based text exposition (`/metrics` renderer or
  `encode_prometheus() -> String`). Do not start an HTTP server
  in the crate (no network listener; that is PR-058 territory).
- Counters/gauges/histograms owned by the crate:

```text
finstack_runtime_status                    # gauge: idle/running/suspended
finstack_runtime_queue_depth               # gauge: hub + exporter
finstack_effect_latency_seconds            # histogram
finstack_store_latency_seconds             # histogram
finstack_usage_tokens                      # counter (input/output)
finstack_recovery_total                    # counter (retry/uncertain/fail)
finstack_observer_dropped_total            # counter (backpressure)
finstack_compaction_trigger_total
finstack_compaction_tokens{bound="before|after"}
finstack_compaction_checkpoint_total{result="hit|miss|invalidated"}
finstack_compaction_summary_latency_seconds
finstack_compaction_failure_total
finstack_compaction_cache_impact_total{impact="..."}
```

- Labels: strategy_id, strategy_version, component_id, and
  identifiers already public on `ObserverEventView`. **No**
  message text, summary text, tool arguments, or secret headers.
- Prefer a tiny hand-rolled exposition encoder over a heavy
  Prometheus client if the latter pulls HTTP. A `prometheus`
  crate is allowed only in this leaf, never in kernel/runtime/SDK.

### Redaction policy (shared)

Add a small helper **in one observer crate or as functions in
`finstack-ai-runtime` that already own `ObserverEventView`**.
Prefer extending the existing projection (already hides
`Credential`) over a new utility crate (TDD §3.1: no miscellaneous
crate unless three independents need it — log, otel, and metrics
do, so a `finstack-ai-observer-redaction` sibling is allowed
**only if** sharing from runtime is insufficient). First choice:
keep projection in runtime; leaves only format.

Apply the same mode to:

- observer streams
- diagnostic JSON/JSONL (`finstack-ai-protocol::to_diagnostic_jsonl`
  wrapped with a redacting view; do not change canonical CBOR)
- support-bundle writer (zip or directory of redacted JSONL +
  versions + metric snapshot; no raw journal CBOR unless the
  caller opts into an **authoritative** copy that is not the
  default bundle)
- journal *export projection* (JSONL), not the store file

Default bundle = metadata-only + redacted events. Canary tests
scan bundle bytes.

### Compaction diagnostics

Read `CompactionEvidence` / recorded middleware outcomes. Emit
the A05 field set. Default path uses `MetadataOnly` / `Redacted`
so `derived_summaries` and `replacement_messages` never leave the
adapter. A `Full` mode may include non-credential evidence
digests, still not summary text unless a future PR explicitly
adds an opt-in debug flag — out of scope here.

If PR-056 has not landed at admit, prove A05 against PR-018
conformance fixtures that already construct `CompactionEvidence`.
Do not implement compaction strategies in this PR.

### Trace examples and conformance

- Each crate calls `check_observer_conformance`.
- Add a failing-observer / stalled-exporter fixture (A01, A03).
- Add a canary-secret fixture (A02).
- Document one end-to-end trace in `extensions/observers/README.md`
  (scripted model + `ReferenceObserver` + log adapter). Do not
  add a Grafana/hosted collector example.
- Optional: feature-gated example under `examples/rust-minimal`
  only if it does not pull otel into the default example graph.
  Prefer docs over a new example directory (TDD example set has
  no `examples/observability/`).

### Graph

Forbidden edges:

- kernel / runtime / default SDK / wit / wasm → observer-log,
  observer-otel, observer-metrics, `opentelemetry*`, `prometheus`
- observer crates → kernel (runtime `Observer` + events only)
- observer-otel → provider crates, plugin-host, wasmtime

Add the three crate names plus `opentelemetry` / `prometheus` to
`tools/wasm_package/check.py` `FORBIDDEN_WASM` / kernel forbidden
set as needed.

Do not restore `tools/architecture/`.
Do not invent `mise run schema-governance`.

## Tasks (when admitted)

Task IDs are minted at admit, not now. Do not start these until
admission checks pass.

1. Tracking: confirm Phase 8 entrance `Passed` (2/2) and PR-055 /
   PR-056 `Done`; open `codex/pr-057-observer-diagnostics` from
   the then-current `main` tip. Mark PR-057 `In progress`. Do not
   re-record Phase 8 entrance.
2. Shared redaction/export projection + canary tests (A02).
3. Log observer + conformance + failure isolation (A01, A04 graph
   start).
4. Metrics observer: runtime + compaction series, bounded queue
   (A03, A05).
5. OTel observer, feature-gated exporter, span mapping, bounded
   queue (A01, A03).
6. Support-bundle / JSONL export defaults, README trace example,
   TM-04/TM-17/TM-21 review, candidate evidence. Stop before
   `G7-D-*`.

## Explicit exclusions

No hosted telemetry service, collector, or Grafana dashboard. No
OTLP default network export. No HTTP metrics server. No observer
spill-to-disk. No new `RunEvent` kinds. No field-level redaction
of authoritative journal records. No seventh port. No compaction
strategy implementation (PR-056). No remote server (PR-058). No
Python/JS exporter batteries in this PR. No G5 or G7 decision.
No `0.1.0` bump, publish, or tag.

## Validation

- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
  — no observer-log/otel/metrics, no `opentelemetry`, no
  `prometheus`
- `cargo tree -p finstack-ai --locked --no-default-features` — same
- `cargo tree -p finstack-ai-observer-log --locked` — no otel, no
  prometheus, no kernel
- `cargo tree -p finstack-ai-observer-otel --locked` — otel only
  in this leaf; no kernel, no wasmtime, no reqwest unless the
  optional `otlp` feature is enabled
- `cargo tree -p finstack-ai-observer-metrics --locked` — no otel
- `cargo test -p finstack-ai-observer-log --offline --locked`
- `cargo test -p finstack-ai-observer-otel --offline --locked`
- `cargo test -p finstack-ai-observer-metrics --offline --locked`
- `cargo test -p finstack-ai-runtime --offline --locked observer`
- `cargo test -p finstack-ai-test --offline --locked observer`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `uv run --no-project python tools/wasm_package/check.py graph`
- `mise run check` after the candidate is otherwise green

Do not require `mise run ci`, Playwright, a hosted collector, or
Criterion numbers.

## Suggested authorization sentence

When Phase 8 entrance is `Passed` (2/2), PR-055 and PR-056 are
`Done`, and the owner is ready:

```
Run PR-057; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

A later, separate sentence is required to record `G5-D-*`, triage
the public API change backlog, admit PR-055/PR-056, or record
`G7-D-*`. Do not infer those from `implement the plan`.
