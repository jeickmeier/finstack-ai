# PR-063 execution plan

Date: 2026-08-15
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-063-perf-memory-startup-budgets`
Intended baseline: local `main` at `0a4b477c1eb9b04d2e86294a9c01b36855ddd74c`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-063. The closed PR-054
envelope is not reused. The PR-055–PR-062 envelopes are not reused.
PR-063 is the only active logical PR once admitted. This planning
file does not admit the PR, start Phase 8 or Phase 9, cut `0.1.0`
or `1.0.0`, or record G5 / G7 / G8.

## Execution envelope

Not authorized. Suggested text when the owner is ready:

```
Run PR-063; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

`implement the plan` is enough only if it names that same local-only
integrated envelope **and** the admission checks below are already
true. Do not infer authorization from this planning file, from
`continue`, from Phase 7 `Done`, from G6 `Passed`, from Phase 8
or Phase 9 plans existing, or from a phase name.

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, crates.io
publish, tag, G5 inference, G7 inference, G8 inference. Do not
write `G5-D-*`, `G7-D-*`, or `G8-D-*`. Do not start PR-055–PR-062
or PR-064+. Do not cut or publish `1.0.0` (PR-066). Do not invent
`0.1.0` baselines, adopter usage, or exception approval.

## Admission (when authorized)

Do not admit coding until all of the following are true. Planning
this file does not record them.

### Phase 9 entrance is `Passed` (2/2)

Implementation Plan §17 entrance. Current state is `0/2`.

| Entrance bullet | Current state |
| --- | --- |
| `0.1.0` used by external adopters | **Blocked.** Unpublished lockstep `0.1.0` exists and G7 is `Passed`. No external project depends on it. First-party starters do not count. |
| Preview telemetry, issue patterns, API pain points, and migration needs reviewed | **First-party review exists** at [`preview-feedback-review.md`](../../preview-feedback-review.md). That review is not external soak and does not make entrance `2/2`. |

Phase 8 is `Done` (entrance `2/2`, exit `4/4`). G7 is `Passed` via
`G7-D-public-preview-f7c7e70b9e04`. Do not infer Phase 9 entrance
from Phase 8 `Done`, from G7, or from the first-party review.

If the owner says `implement the plan` while Phase 9 entrance is
still `0/2`, **stop**. Do not fabricate adopter usage, preview
feedback, or `0.1.0` baselines. A local `0.1.0` candidate with no
external use does **not** satisfy bullet 1.

Phase 9 entrance, once Passed, is recorded by the first admitted
Phase 9 PR. If PR-062 lands first, do not re-record
`PH9-E-entrance-*` here.

### Phase 8 is `Done` and G7 is `Passed`

PR-055–PR-061 must be `Done`. `G7-D-*` must exist. PR-061's
`0.1.0` baselines must be checked in. Keep one active logical PR.

### PR-061 is `Done` and `0.1.0` baselines exist

Implementation Plan lists PR-061 as the code dependency. A01
compares against **`0.1.0` baselines**. Those baselines must be
the PR-061 / G7 corpus (Criterion metadata, Python fast-path
report, WASM crossing/bundle-size, idle-session reference). The
current `0.0.4` warning corpus (PR-020 / PR-029 / PR-038) is
method evidence, not the 1.0 comparison line.

### PR-062 is `Done` unless the owner parallelizes

Implementation Plan lists PR-062 then PR-063. Do not admit
PR-063 while PR-062 is `Todo` unless the owner explicitly
authorizes parallel Phase 9 work in the same sentence. PR-063
does not freeze contracts or generate WIT `@1.0.0` worlds.

### Other admission checks

- NFR-PERF-001–006 are warning/regression evidence until this PR
  ratifies them as 1.0 warning/failure budgets (Implementation
  Plan §18.2). Do not claim they already pass.
- Native idle-session reference on `main` is 61,824 bytes versus
  the 32,768-byte NFR-PERF-005 target (PR-029
  `python-fast-path-report.json`; delivery-ledger warning pending
  this PR). Meet 32 KiB on the ratified workload or request an
  approved unexpired exception. Do not silently raise the target.
- ADR-030 stays boxed public port futures unless Phase 3-and-later
  benches show material dispatch cost **and** a superseding ADR
  is accepted. Do not unbox the public ABI from a hunch.
- ADR-032 compact snapshot projection stays out unless PR-041
  evidence plus a new ADR. Snapshots remain disposable caches.
- No Implementation Plan section 6.3 ADR trigger applies if
  optimizations keep determinism, commit-before-effect, checksums,
  and six ports. An optimization that weakens those needs an ADR
  (explicit exclusion).
- Threat Model section 18 is not a default trigger. If a change
  alters queue bounds, retention, or secret-bearing telemetry in
  flamegraphs, complete the review. Flamegraphs must not contain
  secrets (TM-04).
- Do not restore `tools/architecture/`. Do not invent
  `mise run schema-governance`. Do not invent
  `mise run benchmark-smoke` if it is still absent from root
  `mise.toml` at admit (historical docs name it; current root
  tasks include `benchmark-wasm` only).

## Traceability

Implementation Plan PR-063 and §18.2; Phase 9 exit "Performance
budgets enforced" (this PR's evidence, not `G8-D-*`); PRD
NFR-PERF-001–007 and NFR-REL-001–005 (REL is traceability: do
not weaken bounded queues); TDD §33; Engineering Standards §2
principle 10, ENG-ARCH-004/005, and G8 "enforced budgets";
`schemas/benchmark-report/` candidate-v1.
G8 is Phase 9's gate and is out of scope. PR-066 cuts `1.0.0`.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

- PR-063-A01: No unexplained benchmark regression remains versus
  `0.1.0` baselines. Re-run the TDD §33.1–33.3 corpus on the
  ratified reference environment. Every group that existed at
  `0.1.0` is compared. A regression needs a cause note or a fix.
  Do not hide model/network latency inside framework numbers
  (principle 10).
- PR-063-A02: Published evidence covers NFR-PERF-001 through
  NFR-PERF-007. Each applicable 1.0 target or budget passes on
  its versioned reference workload/environment **or** has an
  approved unexpired `EX-*` row, and repeated schema/validator
  compilation fails conformance. NFR-PERF-007 is a conformance
  rule, not a numeric budget.
- PR-063-A03: Minimal binary and WASM bundle size targets are
  measured and enforced. Ratify numbers from the `0.1.0` /
  PR-038 bundle-size corpus, the existing 10 MiB Python wheel
  budget, and a measured minimal native CLI binary. Publish the
  table. CI fails on exceed. Do not invent a tighter number
  without a measured baseline.
- PR-063-A04: Memory growth under long streams and repeated runs
  is bounded. 1,000 idle sessions and 100 active sessions (TDD
  §33.2 items 10–11) show framework-owned bytes that do not grow
  unboundedly with stream length or run count. Queues stay
  bounded (NFR-REL-004).

Principal changes that are not extra acceptance IDs, but are
required to prove the four bullets:

- Targeted optimizations: reducer allocations, raw JSON
  handling, stream batching, registry resolution, provider
  reuse, and store replay.
- Ratify reference workloads/environments and activate
  NFR-PERF-001–006 as 1.0 warning/failure budgets (native,
  Python fast path, browser WASM, applicable WIT).
- Profile 1,000 idle + high-concurrency active sessions.
- Publish representative flamegraphs and tuning guidance
  (secret-free).

## Locked design

### Layout

```text
docs/implementation/perf-budgets.md              # NEW; reference env + fail budgets + size table
docs/site/performance.md                         # NEW; tuning guidance (PR-060 site has no page)
docs/implementation/artifacts/pr-063/            # flamegraphs, ratification reports, SHA256SUMS
schemas/benchmark-report/v1/                     # extend; do not invent a second family
fixtures/compatibility/benchmark-report/         # keep reject-unknown
crates/finstack-ai-test/benches/                 # extend native_runtime + conformance
bindings/finstack-ai-python/src/benchmark_fixture.rs
bindings/finstack-ai-wasm/js/src/benchmark.test.ts
plugins/finstack-ai-plugin-host/benches/plugin_host.rs
```

Do not invent a second bench harness or docs root. Do not restore
`tools/architecture/`. Do not invent `mise run schema-governance`.
Do not edit `docs/planning/*`.

### Numeric targets (PRD §10.1; do not rewrite)

| ID | Budget | Notes |
| --- | --- | --- |
| NFR-PERF-001 | median < 5 µs, p99 < 25 µs | small non-I/O kernel transition on the reference desktop |
| NFR-PERF-002 | < 10 µs | resolved native model/tool dispatch excluding impl work and user-payload alloc |
| NFR-PERF-003 | Python ≤ 10% over native; WASM ≤ 15% | Rust-backed only; exclude host/network |
| NFR-PERF-004 | ≥ 100,000 small in-process progress events/s | before binding batching; no unbounded queues |
| NFR-PERF-005 | < 32 KiB framework-owned / idle session | excludes conversation, provider clients, store caches, app data |
| NFR-PERF-006 | warm FS startup < 25 ms; kernel init < 1 ms | minimal native CLI benchmark |
| NFR-PERF-007 | no per-call schema/validator compile | conformance, not a numeric budget |

PR-038 `wasm-js-crossing.json` stored `target_percent: 10`. The
PRD WASM budget is **15%**. Ratify 15% for browser WASM. Do not
treat that JSON field as the 1.0 budget.

Budgets fail the candidate **on the ratified reference
environment**. Other hosts stay warning/regression only.

### Reference environment and operational register

`docs/implementation/perf-budgets.md` is the operational
register (same role as `compatibility-governance.md` for
compat). It names:

- machine class, OS, CPU, rustc (mise pin is 1.97.1 today),
  Python, Node, `mise.toml` pins, feature set, commit;
- the versioned workload IDs for native / Python fast path /
  browser WASM / WIT;
- which NFR-PERF IDs are fail vs warning on which host;
- the A03 size table and measurement commands.

Extend `schemas/benchmark-report/v1/metadata.schema.json` if a
new required field is needed. Keep reject-unknown. Local Darwin
candidate evidence is allowed. A hosted perf lab is a separately
named external action.

Do not use live model latency.

### TDD §33 corpus (must cover)

**33.1 Microbenchmarks:** kernel `decide`/`apply` by transition
kind; record encode/decode; tool registry lookup; context
assembly; event batching; message history view; snapshot replay.

**33.2 Synthetics (scripted models only):** (1) text-only;
(2) 1/10/100/1,000 model deltas; (3) one fast tool; (4) 100
fast tools; (5) parallel vs sequential batches; (6) output
validation; (7) cancellation at each boundary; (8) approval
suspend/resume; (9) journal restore; (10) 1,000 idle sessions;
(11) 100 active sessions; (12) large result via blob reference.

**33.3 Binding benches (report separately):** native from Rust;
native from Python; one Python tool callback; Python model
stream callback; browser WASM + JS adapters; native host + WIT
toolset.

**33.4 Artifacts:** machine metadata, compiler version, commit,
feature set, raw samples, and flamegraphs for regressions above
the configured threshold.

Reuse and extend; do not invent a second harness:

```text
crates/finstack-ai-test/benches/native_runtime.rs      # idle RSS via ps; --sessions N
crates/finstack-ai-test/benches/conformance.rs         # reducer / stream groups
extensions/toolsets/finstack-ai-tools-calculator/benches/calculator.rs
extensions/providers/finstack-ai-provider-openai-compatible/benches/request_overhead.rs
extensions/stores/finstack-ai-store-sqlite/benches/restore_growth.rs
plugins/finstack-ai-plugin-host/benches/plugin_host.rs
bindings/finstack-ai-python/src/benchmark_fixture.rs
bindings/finstack-ai-wasm/js/src/benchmark.test.ts
```

### Optimizations (smallest first)

Measure before changing. Allowed targets if benches show they
matter:

- reducer / apply allocations (clone/Arc, not semantic changes)
- `RawJson` / JCS reuse after construction
- event batch thresholds already configured (do not drop durable
  completion events)
- registry lookup stays off the hot path (ENG-ARCH-004); fix
  only if a bench proves a leak back into the turn
- provider/client reuse (FR-RT-006 already requires this)
- store replay / snapshot-plus-tail (do not make snapshots
  authoritative)

Forbidden without an ADR: skipping checksums, skipping
commit-before-effect, unbounding queues, per-token binding
callbacks (ENG-ARCH-005), weakening cancellation, compact
snapshot as a second semantic model.

Idle-memory gap (61,824 vs 32,768 bytes): profile first. If the
framework-owned remainder cannot meet 32 KiB without a
determinism/safety ADR, request `EX-*` with every register
field filled (owner, compensating control, expiry, removal
task). The PRD/§18.2 sentence already allows an approved
unexpired exception for these budgets at G8; that is not a
silent requirement change. The exceptions register still
forbids waiving kernel I/O, a seventh port, or unbounded
queues. Do not invent or approve the exception from planning.
Do not edit the 32 KiB sentence in planning files.

### Size budgets (A03)

Ratify, then fail CI:

| Artifact | Starting measured / existing budget | Action |
| --- | --- | --- |
| Python wheel | 10 MiB (PR-027–PR-032; do not raise) | keep and enforce |
| WASM `*_bg.wasm` | PR-038 ~5,950,310 bytes / ~1,811,048 gzip | ratify `0.1.0` measured + documented slack, or `EX-*` |
| Minimal native CLI | no numeric target on `main` | measure `finstack-ai` minimal features, then ratify |

Do not invent a competing size story. WASM is multi-megabyte
today; a hard cut that drops features needs an exception, not a
silent scope change.

### Flamegraphs and tuning

Check in representative flamegraphs (or Criterion HTML) under
`docs/implementation/artifacts/pr-063/`. Add
`docs/site/performance.md` covering: isolate framework vs model
time, batch events, reuse providers, prefer Rust-backed tools,
bound queues. Strip absolute home paths and any secret-bearing
args (TM-04).

### Conformance for NFR-PERF-007

Add a focused test: construct one agent with a tool schema and
output schema; run N scripted calls; assert validator/compile
counters stay at the construction count. Fail the test on a
second compile. This is a release-blocking conformance rule.

### Graph and version

No new heavy profiler crate in kernel/runtime/default SDK.
Criterion stays in benches. Workspace version stays whatever
PR-061 left (`0.1.0`); do not bump to `1.0.0`.

## Tasks (when admitted)

Task IDs are minted at admit, not now. Do not start these until
admission checks pass.

1. Tracking: confirm Phase 9 entrance `Passed` (2/2), Phase 8
   `Done`, `G7-D-*` exists, PR-061 `Done` with `0.1.0`
   baselines, PR-062 `Done` unless parallelized; open
   `codex/pr-063-perf-memory-startup-budgets` from the
   then-current `main` tip. Mark PR-063 `In progress`. Do not
   re-record Phase 9 entrance if PR-062 already did.
2. Write `perf-budgets.md`; ratify workloads/environment;
   capture current vs `0.1.0` (A01).
3. NFR-PERF-007 conformance test (A02).
4. Idle 1,000 / active 100 memory profiles; investigate
   NFR-PERF-005 gap (A04, A02).
5. Smallest justified optimizations; re-measure (A01, A02).
6. Size-budget table + CI fail (A03).
7. Flamegraphs, `docs/site/performance.md`, exceptions if
   needed, candidate evidence. Stop before `G8-D-*`.

## Explicit exclusions

No optimization that weakens determinism or safety without an
ADR. No live-model latency in framework numbers. No seventh
port. No compact-snapshot ADR unless separately accepted. No
public-ABI unbox without ADR-030 reconsideration evidence. No
`1.0.0` cut. No G8 inference. No fabricated `0.1.0` baselines
or adopter usage. No planning-file edits of the NFR numbers.
No second bench harness. No hosted perf lab unless separately
named.

## Validation

Use checked-in tasks and direct commands. Do not assume a
historical task still exists.

- `cargo bench -p finstack-ai-test --offline --locked` and any
  leaf bench this PR touches
- Python fast-path report vs native (NFR-PERF-003 10%)
- `mise run benchmark-wasm` (Playwright Chromium; NFR-PERF-003
  15%)
- idle `--sessions 1000` and 100-active concurrency profile
- NFR-PERF-007 conformance test
- size-budget check against the ratified table (wheel 10 MiB
  path already exists; WASM `tools/wasm_package/check.py size`)
- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
- `uv run --no-project python tools/wasm_package/check.py graph`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `mise run check` after the candidate is otherwise green

If `mise run benchmark-smoke` exists at admit, run it. If it
does not, do not invent it; add a thin mise task only when a
direct one-liner cannot express the ratification suite.

Do not require a hosted perf lab, Temporal, or `mise run ci` on
a multi-OS matrix unless separately named.

## Suggested authorization sentence

When Phase 9 entrance is `Passed` (2/2), Phase 8 is `Done`,
`G7-D-*` exists, PR-061 (and PR-062 unless parallelized) are
`Done`, `0.1.0` baselines exist, and the owner is ready:

```
Run PR-063; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

A later, separate sentence is required to record `G5-D-*`,
`G7-D-*`, `G8-D-*`, or cut `1.0.0`. Do not infer those from
`implement the plan`.
