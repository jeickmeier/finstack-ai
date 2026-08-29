# Python Binding Full Composition Parity Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans, task-by-task. Checkbox steps.

**Goal:** Everything a Rust host can compose, Python can compose: every
composable extension gets a native wrapper registrable through the existing
`Agent` factory parameters, the artifact store becomes durable-capable, native
capability activation lands, and a CI parity guard makes every future
extension fail the build until it is wrapped or explicitly waived. Terminal
proof: `_knowledge.py`'s divergence docstring says "none", and the parity
guard's waiver list contains only deliberate trust boundaries.

**Architecture:** No new abstractions — the binding already has one proven
wrapper recipe (`src/memory.rs`, `src/fetch.rs`, `src/elicitation.rs`):
a `#[pyclass]` handle owning the configured native component (or the inputs
to build it at agent-assembly time when it needs the shared artifact store),
accepted by the existing `toolsets=[...]`/`context_providers=[...]`/
`middleware=[...]`/`observers=[...]` parameters via the `Py*Arg` unions in
`src/agent.rs`, registered under the component id/version the native
descriptor self-declares. Every task follows that recipe; the plan varies
only the component, its constructor mapping, and its tests.

**Tech Stack:** PyO3 (existing binding), the extension crates under
`extensions/`, pytest (binding test conventions in
`bindings/finstack-ai-python/tests/`), `scripts/compat/public_items.py`
name-list guard, maturin rebuild loop from
`examples/python-notebooks/README.md`.

**Spec:** This plan is self-contained; the motivating gap record is the
divergence docstring in `examples/python-notebooks/_knowledge.py` and the
knowledge-agent spec's option-a decision
(`docs/superpowers/specs/2026-08-28-knowledge-agent-design.md` §6).

## Global Constraints

- **The wrapper recipe is law.** Before any task, read `src/memory.rs`
  (multi-handle extension), `src/fetch.rs` (config-struct toolset), and
  `src/elicitation.rs` (builder toolset); copy their structure, error
  mapping (`configuration_error`/`FinstackError` taxonomy), `#[pyclass]`
  attributes (`frozen`, module name), and doc-comment style exactly.
- **Registration identity must match the native descriptor.** Context
  providers, middleware, and observers self-declare `(component id,
  version)`; register under exactly that pair (the knowledge CLI learned
  this the hard way — see `apps/finstack-knowledge/src/compose.rs`
  `versioned(...)` and its comment). Toolset/model/store registrations are
  caller-named.
- **The binding never reads environment variables.** Secrets and paths
  arrive as explicit Python values (`api_key=`, `path=`), matching the
  existing provider factories.
- Every task updates, in the same commit: the `.pyi` stubs (full
  docstrings + typed signatures per `.agents/rules/04-python-coding.md`),
  the frozen Python name-list baseline checked by
  `scripts/compat/public_items.py`, and the crate's row in
  `fixtures/compatibility/binding-parity/v1/extensions.json` (the existing
  parity catalog — see Task P0.2's resolution).
- Rebuild the native module before running tests when Rust changed:
  `PYO3_PYTHON="$(uv python find 3.14)" uv run --no-project --with maturin==1.14.1 maturin develop --locked --manifest-path bindings/finstack-ai-python/Cargo.toml --features extension-module`
- Verify per task: `cargo clippy -p finstack-ai-python --all-targets --locked -- -D warnings`,
  `uv run pytest bindings/finstack-ai-python/tests/<new test file> -q`, and
  `mise run check-python`. Never run workspace-wide tests.
- Offline tests only: scripted `PythonModel` callbacks; native components
  needing I/O get loopback servers (copy `test_elicitation.py` /
  knowledge-CLI loopback patterns) or tmp-dir stores.
- One commit per task; short imperative subject; do not push.

## Gap inventory (verified 2026-08-28)

Linked today: memory (full extension), document toolset + ingest middleware
(auto-registered), elicitation, http-fetch, e2b-sandbox, all five providers,
memory/sqlite journal stores, `Capability` (instruction-only).

| Component | Kind | Phase |
|---|---|---|
| Durable artifact store (`finstack-ai-store-artifact` local; in-process today) | store | P0 |
| Parity guard (roster vs exports, waiver file) | CI | P0 |
| `finstack-ai-middleware-instructions` | middleware | P1 |
| `finstack-ai-middleware-compaction` (sliding window, large tool output) | middleware | P1 |
| `finstack-ai-context-repository` | context | P1 |
| `finstack-ai-observer-log` | observer | P1 |
| `finstack-ai-tools-skills` + `NativeCapabilityHost` activation | toolset/capability | P1 |
| `finstack-ai-middleware-verify` | middleware | P2 |
| `finstack-ai-middleware-redaction` | middleware | P2 |
| `finstack-ai-middleware-tool-policy` | middleware | P2 |
| `finstack-ai-observer-metrics` / `-otel` / `-notify` / `-billing` | observers | P2 |
| `finstack-ai-tools-calculator` | toolset | P3 |
| `finstack-ai-tools-skill-import` | toolset | P3 |
| `finstack-ai-tools-filesystem` | toolset (security-sensitive) | P3 |
| `finstack-ai-tools-shell` | toolset (security-sensitive) | P3 |
| `finstack-ai-tools-subagent` | toolset | P3 |
| `finstack-ai-tools-mcp` | toolset | P3 |
| `finstack-ai-tools-openai-media` / `-openrouter-media` | toolsets | P3 |
| `finstack-ai-store-postgres` journal | store | P4 |
| `finstack-ai-store-artifact` S3 backend | store | P4 |
| Compaction `summarize` strategy (needs authorized model + budget scope) | middleware option | P4 |
| Divergence burn-down + notebook/docs updates | docs | P5 |

Out of scope, recorded as **deliberate waivers** in the guard (not gaps):
`extensions/interop/*` (child-process interop), `extensions/workflow/*`
(worker/HITL services), `extensions/net/finstack-ai-net-guard` (process-level
confinement a Python host configures at the OS tier), `finstack-ai-server`,
`finstack-ai-provider-wire` (shared machinery, not a port), plugin hosts.
Each waiver line carries its one-sentence rationale; removing a waiver later
is a normal task against this same recipe.

---

## Phase P0 — Foundation (unblocks everything; fixes a real bug)

### Task P0.1: Durable artifact store option

The binding hardwires `InProcessArtifactStore`
(`src/agent.rs::document_ingest_ports`), so a session persisted with
`sqlite_path=` cannot resolve its historical attachments from a fresh
process — the exact failure the knowledge CLI hit and fixed with
`LocalArtifactStore` (see `apps/finstack-knowledge/src/compose.rs::open_artifact_store`
and commit `91807cc`).

**Files:**
- Modify: `bindings/finstack-ai-python/Cargo.toml` (add
  `finstack-ai-store-artifact = { workspace = true, features = ["local"] }`)
- Modify: `bindings/finstack-ai-python/src/agent.rs` (every factory gains
  keyword `artifact_path: str | None = None`; when set, the shared store is
  `LocalArtifactStore::try_new(path)` instead of in-process — one code
  path, chosen before `document_ingest_ports` builds the toolset/middleware)
- Modify: `python/finstack_ai/_finstack_ai.pyi`
- Test: `bindings/finstack-ai-python/tests/test_artifact_durability.py`

**Interfaces:** `Agent.from_python(..., sqlite_path=..., artifact_path=...)`
(same keyword on every factory). Staging, the document toolset, and the
ingest middleware all share the chosen store, as today.

- [ ] **Step 1:** Failing test: agent A (sqlite journal + artifact_path in
  one tmp dir) runs with a csv `Attachment` on a session; a **new** agent B
  over the same two paths opens the session and runs a follow-up turn; the
  follow-up succeeds and the model-visible request contains the converted
  Markdown (capture-callback assertion, pattern of notebook 08). Also:
  without `artifact_path`, the same flow's follow-up turn surfaces the
  documented in-process limitation (assert the stable error code — this
  pins today's behavior as explicit rather than surprising).
- [ ] **Step 2:** Implement; keep `InProcessArtifactStore` the default.
- [ ] **Step 3:** Rebuild, tests green, clippy, `mise run check-python`.
- [ ] **Step 4:** Commit `Add durable artifact store option to python agents`

### Task P0.2: Parity guard — RESOLVED: already exists (adapted 2026-08-29)

Execution finding: the repo already ships this guard.
`scripts/compat/public_items.py` (run by `mise run check-public-api`, part
of CI) validates `fixtures/compatibility/binding-parity/v1/extensions.json`
— a catalog with one row per extension crate carrying a python and js
disposition (`direct` / `composed` / `facade` / `host_adapter` /
`unavailable` + reason). It already enforces everything this task planned:
a new extension crate anywhere in the workspace fails the check until its
row is decided; `direct`/`composed` rows must be Cargo dependencies of the
binding; `direct` rows must name exported surfaces that actually exist in
the `.pyi`/`__init__` name lists; `unavailable` rows cannot be
dependencies (this fired on P0.1's new `finstack-ai-store-artifact`
dependency, proving the guard live). The `--write` mode maintains mutation
fixtures (`invalid--extension-surface.json`) that self-test the guard.

Therefore no `scripts/compat/python_parity.py` and no `PARITY.md` are
created — a second ledger would drift. **The catalog is this plan's status
table**: each P1–P4 task flips its crate's python row from `unavailable`
to `direct` (with `surfaces` naming the new exported classes) or
`composed` (with reason) in the same commit as the wrapper. P5.1's closure
check becomes: every python row that this plan scoped is `direct`,
`composed`, or `facade`, and remaining `unavailable` rows are exactly the
deliberate waivers listed under "Out of scope".

- [x] **Step 1:** Reconcile P0.1: `finstack-ai-store-artifact` python row
  → `composed` (durable store behind `artifact_path=`).
- [x] **Step 2:** Guard green (`public_items.py --check` exit 0); plan
  updated to route later tasks through the existing catalog.
- [x] **Step 3:** Commit `Reconcile binding parity catalog with artifact_path`

---

## Phase P1 — Knowledge-agent divergences (empties the `_knowledge.py` list)

Every task here follows the wrapper recipe; per-task notes list only the
constructor mapping and the test's essential assertions. All constructors
were verified against the crates' public API fixtures on 2026-08-28.

### Task P1.1: `InstructionsMiddleware`

- Python: `finstack_ai.InstructionsMiddleware(entries=[("label", "text"), ...])`
  → `PolicyInstructionsConfig { entries: Vec<PolicyEntry{label, text}> }`;
  register under `finstack.middleware.instructions` v1.0.0; accepted by
  `middleware=[...]` (extend the arg union like `PyObserverArg` does for
  memory).
- Test: scripted capture-model asserts the policy line appears in the
  model-visible request; empty entries rejected as `ConfigurationError`.
- [ ] Steps: failing test → implement (+pyi, public_items, PARITY row) →
  green/clippy/check-python → commit
  `Add native instructions middleware to python binding`.

### Task P1.2: `CompactionMiddleware` (sliding window, large tool output)

- Python: `finstack_ai.CompactionMiddleware.sliding_window(threshold_tokens, hysteresis_tokens)`
  and `.large_tool_output(threshold_tokens, hysteresis_tokens, max_body_bytes)`
  → the crate's same-named constructors; register under
  `finstack.middleware.compaction` v0.0.4 (the crate's declared version —
  re-verify against source at implementation time).
  `summarize` is deferred to P4 (needs an authorized model ref + budget
  scope surface).
- Test: construct + register both strategies on a scripted agent; run
  completes (`Continue` path); a tiny-threshold sliding-window run on a
  multi-turn lane session compacts (assert via `RunResult.trace` record
  kinds — copy the compaction lane test's expected kinds rather than
  inventing).
- [ ] Steps as P1.1; commit
  `Add native compaction middleware to python binding`.

### Task P1.3: `RepositoryContextProvider`

- Python: `finstack_ai.RepositoryContextProvider(path)` →
  `RepositoryContextProvider::try_new(path)`; register under
  `finstack.context.repository` v0.0.4; accepted by
  `context_providers=[...]`.
- Test: point at a tmp docs dir; capture-model asserts the doc text is
  contributed; nonexistent path → `ConfigurationError`.
- [ ] Steps as P1.1; commit
  `Add repository instructions provider to python binding`.

### Task P1.4: `LogObserver`

- Python: `finstack_ai.LogObserver(path, payload_mode="metadata_only")` —
  file-sink construction (the native `Arc<Mutex<dyn Write>>` wraps an owned
  `File`; stderr variant `LogObserver.stderr(...)`); register under
  `finstack.observer.log` v0.0.4.
- Test: run once; the file contains NDJSON lines whose `kind` values
  include `run_completed`; payload_mode round-trips.
- [ ] Steps as P1.1; commit `Add native log observer to python binding`.

### Task P1.5: Native skills + capability activation

The largest P1 task: Python `Capability` stays, but gains component
references, and the model can activate capabilities through the native
`SkillsToolset`.

- Python surface:
  - `finstack_ai.CapabilityHost()` wrapping `NativeCapabilityHost` (catalog
    string assembled from the declared capabilities at agent build, exactly
    like `apps/finstack-knowledge/src/compose.rs::skills_host` — that
    closure bridge moves INTO the binding so no Python host rewrites it).
  - `finstack_ai.SkillsToolset(host)` in `toolsets=[...]`.
  - `Capability(..., toolsets=[...], context_providers=[...], middleware=[...])`
    optional component-name references (matching registered components) so
    Python capabilities stop being instruction-only.
- Test: port the essential cases of
  `crates/finstack-ai/tests/capability_activation.rs` to pytest — scripted
  model activates a capability; its instruction takes effect on the next
  turn; `RunResult.active_capabilities` lists it; a capability-gated
  toolset's tools appear only after activation.
- [ ] Steps as P1.1; commit
  `Add native skills toolset and capability activation to python binding`.

### Task P1.6: Burn down the divergence docstring (knowledge track)

- Modify `examples/python-notebooks/_knowledge.py`: compose the real
  instructions middleware, compaction, both repository roots (self-docs
  materialized via a small helper cell — decide whether to expose
  `materialize_self_docs` from the app crate through a script or duplicate
  the five docs; prefer reading them from `apps/finstack-knowledge/docs/`
  by relative path), log observer, and native skills; shrink the docstring
  to the remaining true boundaries (none expected).
- Golden pytest and all five k-notebooks re-execute green.
- [ ] Commit `Compose full knowledge parity in python k-track`.

---

## Phase P2 — Remaining middleware and observers

One task per crate, same recipe. Constructor mappings to verify from each
crate's fixture/source at implementation time; tests assert the component's
observable behavior, not construction alone:

- [ ] **P2.1 `VerifyMiddleware`** — verification outcome visible in trace
  on a scripted violation. Commit `Add verify middleware to python binding`.
- [ ] **P2.2 `RedactionMiddleware`** — a seeded secret pattern is redacted
  in the model-visible request. Commit
  `Add redaction middleware to python binding`.
- [ ] **P2.3 `ToolPolicyMiddleware`** — a denied tool call fails with the
  crate's stable code; an allowed one passes. Commit
  `Add tool-policy middleware to python binding`.
- [ ] **P2.4 observers** (`metrics`, `otel`, `notify`, `billing`; one task
  each or one task if their constructors are trivially parallel — split if
  any needs a sink/server double). Metrics/billing assert counters via
  their read surface; otel uses the crate's in-memory exporter if present,
  else construct+run smoke; notify uses a channel double. Commits
  `Add <name> observer to python binding`.

---

## Phase P3 — Remaining toolsets

Same recipe; each exposes the crate's existing config surface verbatim —
the binding adds **no policy of its own** (deny-by-default configs stay
deny-by-default; Python supplies values explicitly):

- [ ] **P3.1 `CalculatorToolset()`** — scripted tool-call round trip
  (mirror `examples/rust-minimal::run_tool_loop`). 
- [ ] **P3.2 `SkillImportToolset`** — import a fixture skill; catalog
  reflects it.
- [ ] **P3.3 `FilesystemToolset(root, ...)`** — reads confined to the
  explicit root; escape attempt fails with the stable code. Security note
  block in the pyi docstring (T2 trusted, not sandboxed).
- [ ] **P3.4 `ShellToolset(...)`** — expose the crate's allowlist/limits
  config; denied command fails closed. Same security docstring discipline.
- [x] **P3.5 `SubagentToolset`** — DEFERRED (stop-and-file per Known Risk
  4's rule, 2026-08-29): the toolset requires an `AgentInvoker`, and the
  SDK has no production in-process implementation — interop ships remote/
  codex invokers, and every in-process consumer (the subagent lane test
  included) hand-rolls an invoker bound to a live parent `AgentRun`,
  which the binding's `Agent.run` path never holds. Python-initiated
  child runs remain available via `Run.start_child`. Unblock: an SDK
  in-process invoker (change-control), then this task is the standard
  wrapper recipe. Recorded as an explicit waiver in the parity catalog.
- [ ] **P3.6 `McpToolset`** — stdio transport against a scripted MCP server
  double (copy the crate's own test double). Network transports stay
  config-driven.
- [ ] **P3.7 media toolsets** (`openai-media`, `openrouter-media`) —
  construct + register against their provider configs; loopback fetch
  test.
- Commits: `Add <name> toolset to python binding` each.

---

## Phase P4 — Stores and deferred options

- [ ] **P4.1 Postgres journal** — `sqlite_path`/`sqlite_durability` grows a
  sibling: `journal=finstack_ai.PostgresJournal(dsn=...)` (explicit value,
  never env); gated behind a `postgres` cargo feature mirroring the
  workspace's TLS choices; test against a dockerless double is not
  possible, so the pytest is `@pytest.mark.skipif` without
  `FINSTACK_TEST_POSTGRES_DSN` — construct-only otherwise (mirrors how the
  store crate's own tests gate).
- [ ] **P4.2 S3 artifact store** — `artifact=finstack_ai.S3ArtifactStore(...)`
  behind the store crate's `s3` feature; gated live test as P4.1,
  local-path store remains the durable default.
- [ ] **P4.3 Compaction `summarize`** — now that models/budgets are
  wrapped: `CompactionMiddleware.summarize(model=<registered component
  name>, ...)`; port the summarize lane test's assertions.
- Commits per task.

---

## Phase P5 — Docs and closure

- [ ] **P5.1** `PARITY.md` final sweep: every row `wrapped` or a permanent
  waiver; guard green with zero `waived-until: this plan` rows.
- [ ] **P5.2** Docs: notebook README trust table gains the new surfaces; a
  new `12_full_composition.ipynb` (optional, decide at the time) or an
  extension of notebook 02 shows one fully-parity composition; the
  knowledge-agent parity matrix row for Python notes "full extension
  parity". CHANGELOG entry.
- [ ] **P5.3** Final verification: binding pytest suite, `mise run
  check-python`, clippy, golden pytest, k-notebooks, and the parity guard —
  outputs recorded in the task report. Commit `Close python binding parity
  initiative`.

## Known Risks

1. **Version constants drift** (`v0.0.4`/`v1.0.0` registrations): never
   hardcode from this plan — read each crate's declared version at
   implementation time, and prefer adding a `pub const COMPONENT_VERSION`
   to any extension that lacks one (tiny upstream fix, additive fixture
   line) so bindings stop copying magic numbers.
2. **Arg-union churn in `agent.rs`**: every phase widens `PyToolsetArg`/
   `PyContextProviderArg`/`PyObserverArg`/middleware unions. Land P1.1's
   widening pattern first and copy it verbatim; if the unions become
   unwieldy, a `NativeComponent` trait-object escape hatch is a design
   change — stop and get it reviewed rather than improvising.
3. **Security-sensitive toolsets (P3.3/P3.4)**: exposing them is scope-
   approved, but their pyi docstrings and the notebook trust table must
   carry the same T2-not-sandboxed language the shell/filesystem crates'
   READMEs use. If any crate's config cannot express deny-by-default from
   Python, stop and fix the crate, not the binding.
4. **Capability component references (P1.5)** may reveal that
   `Agent.from_python`'s capability path can't reference natively-wrapped
   components by name. If the SDK needs a change beyond the binding, that
   is change-control per `AGENTS.md` — stop and file.
