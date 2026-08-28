# Knowledge agent design: `finstack-ai-knowledge` (three surfaces)

- **Date:** 2026-08-28
- **Status:** Draft for review
- **Source of truth for the shape:** tau comparison session 2026-08-28; `AGENTS.md`; existing composition surface in `crates/finstack-ai`, `bindings/finstack-ai-python`, `bindings/finstack-ai-wasm`.

## 1. Overview

Build the first first-party product on the engine: a **knowledge agent** — ingest
documents, remember facts across sessions, answer questions with retrieval and
citations — delivered through **three surfaces over one definition**:

1. **CLI** (`finstack-know`, Rust) — the operator path.
2. **Python notebooks** (`k01`–`k05`) — the analyst path.
3. **TypeScript browser example** — the embedded path.

The initiative exists to prove the structure, not to ship a knowledge startup.
The claim under test: all three surfaces consume the **same `RunEventKind`
stream**, the CLI and notebooks share the **same journaled sqlite session**,
and all three run the **same agent definition**. Every point of friction found
while building is a defect in `crates/finstack-ai` or a binding, fixed there —
never worked around in a surface.

Deliberately **not** a coding agent: it avoids competing with tau/Pi-class
tools and stresses the differentiated parts of this stack (context providers,
memory, document ingest, journal, provenance, audit).

## 2. Goals

1. A `finstack-know` binary exercising all six ports, sessions, lanes, the
   event stream, and interactions, backed by `finstack-ai-store-sqlite`.
2. A notebook track that is a **product narrative** (ingest → recall →
   cite → inspect trace → open a CLI-created session), distinct from the
   existing `01`–`11` API tour.
3. A browser example whose distinct job is **JS-supplied ports**: a
   TypeScript retrieval toolset over host data via the existing `host_*`
   adapter surface, journaled in IndexedDB.
4. A published **parity matrix** and a shared **golden-questions fixture**
   run by all three surfaces, so drift is a CI failure, not a discovery.
5. Bundled **self-docs**: the agent answers questions about finstack-ai
   itself, offline, via the existing `finstack-ai-context-repository`
   provider.
6. A new `apps/` workspace category with an `AGENTS.md` amendment, so the
   product is not smuggled into `examples/`.

## 3. Non-goals

- **Steering / follow-up queues.** Kernel change → separate change-control
  initiative. The CLI REPL ships interactions + cancel only; steering slots
  in later without redesign.
- **Serialized cross-language agent definition** (option b). v1 composes the
  definition per-surface in each language (option a), with drift held by the
  conformance fixture. Loading a serialized `AgentSpec`/`LockedBundle` from
  Python/TS is the recorded follow-up initiative; `NativeAgentBuilder`
  already creates a strict `AgentSpec` + lock internally, so the seam exists.
- **TUI.** Two renderers (text, `--json` NDJSON) over the event stream are
  the contract proof. No widget toolkit enters any public surface (tau's
  component-seam retrospective is the cautionary record).
- **RPC server / protocol compatibility.** `finstack-ai-server` and any
  Pi/ACP-compatible wire surface are a separate decision.
- **Vector/embedding retrieval.** Memory stays keyword/FTS as shipped;
  the `MemoryStore` trait already reserves the extension point.
- **Kernel or runtime changes.** The composition uses only existing ports.
  If one is discovered to be required, that workstream stops and goes to
  change control per `AGENTS.md`.
- **replay / lock-verify subcommands.** Deferred; `sessions show` covers
  inspection in v1.

## 4. Surfaces and parity matrix

Publish this matrix in `apps/finstack-knowledge/README.md`; asymmetries are
documented boundaries, not bugs.

| | CLI (Rust) | Python notebooks | TypeScript browser |
|---|---|---|---|
| Journal | sqlite | sqlite (same file as CLI) | IndexedDB |
| Providers | linked native (ollama default; anthropic/openai/openrouter by config) | linked native | host adapter (scripted default; live optional) |
| Document ingest | `tools-document` + `middleware-document-ingest` | same | File API + host toolset |
| Memory | full extension | full extension | host-backed store adapter |
| Cross-surface session | opens notebook sessions | opens CLI sessions | inspect/export only (browser storage is origin-local) |
| Confinement / net-guard | available | available | n/a (browser sandbox) |
| Golden questions | yes (CI, offline) | yes (CI, offline) | yes (CI, scripted) |

## 5. Workspace placement

New top-level category:

```text
apps/
  README.md                  # what belongs here; trust class
  finstack-knowledge/        # crate finstack-ai-knowledge, publish = false
    Cargo.toml               # [lib] definition + [[bin]] finstack-know
    README.md                # product doc + parity matrix
    fixtures/golden.json     # shared golden-questions fixture
    docs/                    # self-docs sources (embedded via include_str!)
    src/
      lib.rs                 # definition: config, composition, security
      docs.rs                # embedded self-docs + materialization
      golden.rs              # fixture loader shared by tests
      bin/finstack_know.rs   # CLI entry
      cli/                   # arg parsing, commands, renderers, repl
```

`AGENTS.md` amendment (one sentence in Project Structure): applications that
compose released components into end-user products live under `apps/`; they
are trusted native code (same class as `extensions/`), may not implement
ports except by composing existing extensions, and never appear in
`crates/` dependency graphs.

Rationale for lib + bin in one crate: the *definition* (composition,
self-docs, golden fixture) is what notebooks and the browser example mirror
and what tests import; the CLI is one consumer of it. Two crates would be a
speculative seam.

## 6. Agent definition (the shared thing)

`finstack-ai-knowledge::lib` owns:

```rust
pub struct KnowledgeConfig {
    pub data_dir: PathBuf,          // default: $FINSTACK_KNOW_HOME or $HOME/.finstack-know
    pub provider: ProviderChoice,   // Ollama { base_url, model } | Anthropic | OpenAi | OpenRouter (api_key required, never from env inside the lib)
    pub fetch_allowlist: Vec<HostPattern>, // default: empty (fetch disabled)
}

pub async fn build_agent(config: &KnowledgeConfig) -> Result<Agent, KnowledgeError>;
pub fn security(os_user: &str) -> Result<RunSecurityContext, KnowledgeError>;
```

Composition (all existing components; ids `finstack.know.*`, version 1.0.0):

| Port | Component |
|---|---|
| model | `finstack-ai-provider-{ollama,anthropic,openai,openrouter}` per config |
| journal | `finstack-ai-store-sqlite` at `<data_dir>/journal.sqlite3` |
| context | `finstack-ai-context-repository` × 2 (self-docs root, project root) + `finstack-ai-memory` recall provider |
| middleware | `middleware-instructions`, `middleware-document-ingest`, `middleware-compaction` (`sliding_window`) |
| tool | `tools-document`, `tools-fetch` (allowlist-gated, off by default), `finstack-ai-memory` toolset, `tools-skills` |
| observer | `observer-log`, `finstack-ai-memory` observer |

Python (`k01`–`k05`) and TypeScript mirror this composition explicitly in
their own languages. **Decision (option a):** drift is controlled by the
golden-questions fixture plus a composition checklist in the README, not by
a shared serialized artifact. Option (b) is the follow-up initiative.

## 7. Self-docs

Markdown sources live in `apps/finstack-knowledge/docs/` (architecture,
ports, sessions/lanes, memory, ingestion, CLI usage), embedded with
`include_str!`. On startup the library **materializes** them to
`<data_dir>/self-docs/` (write-if-changed) and points the second
`RepositoryInstructions` instance at that root. This reuses the existing
allowlisted provider untouched — no new port implementation, no custom
context provider in an app. (An earlier draft planned to fix a stale doc
comment in `extensions/context/finstack-ai-memory/src/lib.rs`; verified
2026-08-28 that the crate docs are already accurate — no change needed.)

## 8. CLI design (`finstack-know`)

Argument parsing: **`clap` (derive)** — new workspace dependency, checked
against `deny.toml`. An end-user product justifies the standard tool;
core crates gain no new dependency.

```text
finstack-know ask <question> [--session <id>] [--json] [--model <name>] [--data-dir <path>]
finstack-know ingest <path>  [--session <id>] [--json]
finstack-know sessions list | show <id> [--json] | name <id> <name>
finstack-know repl [--session <id>]
finstack-know docs [<topic>]
```

- `ask` without `--session` creates a session (`Session::create`) and prints
  its id; with `--session` opens it (`Session::open`) and appends on `main`.
- Streaming: `Lane::run(&agent, request)` → `AgentRun`, then `AgentRun::next_event_batch()` feeding
  one of two renderers over the **same** event stream: human text (deltas
  inline, tool lines, final result) and `--json` (one NDJSON object per
  `RunEvent`, kind names verbatim). This is the events-are-the-contract
  proof; renderer code never touches the journal.
- `sessions list` consumes `JournalStore::scan` (already implemented by the
  sqlite store); `sessions name` uses `write_metadata` with CAS. `show`
  renders `LaneInspect.history`.
- `repl`: readline loop; each turn is a run on `main`; Ctrl-C →
  `AgentRun::cancel()`; pending `InteractionRequest`s (elicitation) are
  listed and resolved via `resolve_interaction`. No steering (non-goal).
- `docs` prints bundled self-doc topics/bodies (works with no provider).
- Exit codes: 0 success, 1 run failed, 2 configuration/usage.
- Credentials: `--api-key-env <VAR>` names the variable to read; the
  library itself never reads the environment (matches binding policy).

## 9. Python notebook track

`examples/python-notebooks/k01_knowledge_ingest.ipynb` … following the
existing conventions (`_support.py` live-gating, offline-first cells,
`DEFAULT_OLLAMA_MODEL`):

- `k01` — build the knowledge composition (`Agent.ollama(...)` + memory +
  document toolsets, `SqliteDurability`); ingest a document; ask; cite.
- `k02` — the event stream: `EventBatchIterator`, event kinds, `RunResult.trace`.
- `k03` — memory across sessions: remember in one session, recall in a fresh one.
- `k04` — provenance: show context items and their provenance for an answer.
- `k05` — **cross-surface**: open a CLI-created sqlite journal via
  `SqliteDurability` + `Agent.open_session`; inspect lanes/history; continue
  the conversation; then re-open in the CLI. This notebook is the proof
  artifact for the initiative's central claim.

The existing `01`–`11` tour is untouched.

## 10. TypeScript track

`examples/browser-knowledge/` (sibling of `browser-minimal`, same build
flow): worker + `connectWorker`, IndexedDB adapters, and its distinct job —
a **TypeScript-implemented retrieval toolset** registered through the host
adapter surface (`host_toolset`), searching an in-page corpus (a few bundled
markdown docs). UI: question box, live event-kind stream, answer with
citations, `inspectSession` panel, golden-questions button. Scripted
scenario by default so CI needs no provider; a live provider hookup stays
out of scope.

## 11. Cross-surface conformance

`apps/finstack-knowledge/fixtures/golden.json`: ~10 entries
`{ id, question, corpus_refs, must_contain, event_kinds_expected }` (offline
answers via scripted/ollama-loopback responses, mirroring
`examples/rust-minimal`'s `serve_ndjson` pattern).

- Rust: `golden.rs` loader + `#[tokio::test]` per entry in the app crate.
- Python: a pytest module under the notebooks dir runs the same fixture
  against the k-track composition (offline).
- TS: the example's Playwright test replays the fixture against the
  scripted scenario and asserts the event-kind sequence.

An event kind observed by one surface and absent from another is a failing
conformance test, not a note.

## 12. Security posture

Local single-user: `RunSecurityContext` with tenant `local`, principal from
the OS user, auth method `local`, explicit policy/decision labels (pattern
of `examples/rust-minimal::security()`). Fetch is deny-by-default via the
`HttpFetchConfig` allowlist. Filesystem tools are not part of the v1
composition (ingest takes explicit paths). Confinement/net-guard
integration is available but not wired in v1 — recorded in the README.

## 13. Validation

- `cargo nextest run -p finstack-ai-knowledge`; `cargo clippy -p finstack-ai-knowledge --all-targets --locked -- -D warnings`
- `cargo deny check` after the `clap` addition
- Notebook execution per `examples/python-notebooks/README.md` flow; golden pytest via the repo venv
- Browser example via the existing wasm build + Playwright flow (`mise run test-wasm` scope)
- No workspace-wide test runs from task workers; per-crate commands only

## 14. Delivery

Four plans, separable ownership boundaries (parallel-safe per `AGENTS.md`
after Plan A lands):

- **Plan A** — workspace: `apps/` category, definition crate, self-docs,
  golden fixture, memory doc-comment fix.
- **Plan B** — CLI: commands, renderers, sessions, ingest, repl.
- **Plan C** — Python: k-track notebooks + golden pytest.
- **Plan D** — TypeScript: browser-knowledge example + Playwright fixture.

## 15. Open questions

1. Binary name `finstack-know` vs `fsk` — bikeshed, defaulting to the former.
2. Does `tools-skills` earn its place in v1, or wait until a second
   capability exists? Default: include (it is the capability-catalog demo).
3. `observer-metrics` in v1 or defer? Default: defer; `observer-log` only.
