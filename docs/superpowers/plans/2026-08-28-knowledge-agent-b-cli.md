# Plan B: `finstack-know` CLI

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans, task-by-task. Checkbox steps.

**Goal:** A `finstack-know` binary over the Plan A definition: one-shot ask, streamed rendering (text + `--json` NDJSON over the same `RunEvent` stream), session list/show/name, ingest, and a REPL with interaction resolution and cancel. Every API awkwardness found is fixed in `crates/finstack-ai` (or filed for change control), never worked around here.

**Architecture:** `[[bin]] finstack-know` inside `apps/finstack-knowledge`; `src/cli/` owns arg parsing (`clap` derive — new workspace dep), command dispatch, two renderers, and the REPL. Renderers consume only `AgentRun::next_event_batch()` + `RunResult`; session commands consume only `Session`/`Lane`/`JournalStore::{scan,write_metadata}` (already implemented by the sqlite store). No journal parsing in the CLI.

**Tech Stack:** clap (derive), tokio, the Plan A library. No TUI, no color crate in v1.

**Spec:** `docs/superpowers/specs/2026-08-28-knowledge-agent-design.md` §8

**Depends on:** Plan A complete.

## Global Constraints

- Exit codes: 0 success, 1 run failure, 2 config/usage. Errors to stderr; results to stdout; `--json` mode emits NDJSON events to stdout only.
- API keys only via `--api-key-env <VAR>` (bin reads the var, library receives the value). Never log secrets.
- Offline tests only (ollama-loopback / scripted); live providers are manual.
- Verify per task: `cargo nextest run -p finstack-ai-knowledge --locked`; `cargo clippy -p finstack-ai-knowledge --all-targets --locked -- -D warnings`. Plus Task B1: `cargo deny check`.
- One commit per task; short imperative subject; do not push.

---

### Task B1: clap workspace dep + bin scaffold + `docs` command

**Files:**
- Modify: root `Cargo.toml` (`[workspace.dependencies] clap = { version = "4", features = ["derive"] }`), `apps/finstack-knowledge/Cargo.toml` (`[[bin]] finstack-know`, path `src/bin/finstack_know.rs`)
- Create: `src/bin/finstack_know.rs`, `src/cli/mod.rs`, `src/cli/args.rs`
- Test: `src/cli/tests.rs` (parse-level tests; `clap`'s `try_parse_from`)

**Interfaces:** `Cli { command: Command, data_dir: Option<PathBuf>, json: bool }`; `Command::{Ask{..}, Ingest{..}, Sessions{..}, Repl{..}, Docs{topic: Option<String>}}` per spec §8. `Docs` prints embedded topics/bodies from `SELF_DOCS` — works with no provider, no data dir.

- [ ] **Step 1:** Failing parse tests (subcommands, `--session`, `--json`, unknown topic error) + `docs` output test.
- [ ] **Step 2:** Implement; `main` maps `KnowledgeError`→2, run failure→1.
- [ ] **Step 3:** Green; clippy; `cargo deny check` passes with clap.
- [ ] **Step 4:** Commit `Add finstack-know binary scaffold with docs command`

### Task B2: Event renderers (text + NDJSON)

**Files:**
- Create: `src/cli/render.rs`
- Test: `src/cli/tests.rs`

**Interfaces:** `trait EventSink { fn on_batch(&mut self, batch: &EventBatch); fn finish(&mut self, result: &RunResultView); }` with `TextRenderer` (deltas inline; tool start/settle lines; usage summary) and `JsonRenderer` (one NDJSON object per `RunEvent`, kind names verbatim, no reordering). Both are pure over `(events, result) -> String` for testability; the bin owns actual stdout.

- [ ] **Step 1:** Failing tests: fixed synthetic event sequence renders expected text; NDJSON round-trips through `serde_json` with kinds preserved; both renderers see identical event counts.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green; clippy.
- [ ] **Step 4:** Commit `Add text and ndjson event renderers`

### Task B3: `ask` (create/open session, stream, print)

**Files:**
- Create: `src/cli/ask.rs`
- Test: `src/cli/tests.rs` (ollama-loopback end-to-end)

**Interfaces:** `run_ask(config, AskArgs, sink) -> Result<ExitCode, KnowledgeError>`. No `--session`: `Session::create`, print `session: <id>` to stderr. With `--session`: `Session::open` + lane `main`, then `Lane::run(&agent, request)` (the run-on-lane API — there is no `Agent::start_on_lane`), drive `next_event_batch()` into the sink, `result()` at the end.

- [ ] **Step 1:** Failing tests: fresh ask creates a session and answers; second ask with the printed id continues the same session (history length grows); `--json` mode emits ≥ the golden `event_kinds_expected` set.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green; clippy. Record any SDK friction found (e.g. missing convenience on `Session`) as a follow-up note in the task report — fix in `crates/finstack-ai` if small, file otherwise.
- [ ] **Step 4:** Commit `Add ask command with session create and resume`

### Task B4: `sessions list | show | name`

**Files:**
- Create: `src/cli/sessions.rs`
- Test: `src/cli/tests.rs`

**Interfaces:** `list` renders `JournalStore::scan` pages (id, name-metadata, updated, lane count; `--json` emits raw page rows); `show <id>` renders `LaneInspect.history` per lane; `name <id> <name>` uses `write_metadata` CAS, retrying once on conflict.

- [ ] **Step 1:** Failing tests: two created sessions appear in `list`; `name` round-trips and shows in `list`; CAS conflict path (concurrent write simulated) retries then errors cleanly; `show` on unknown id → exit 2.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green; clippy.
- [ ] **Step 4:** Commit `Add sessions list show and name commands`

### Task B5: `ingest`

**Files:**
- Create: `src/cli/ingest.rs`
- Test: `src/cli/tests.rs` (fixture: small md + pdf from existing repo fixtures if present, else md only)

**Interfaces:** `run_ingest(config, IngestArgs, sink)` — attaches the file to a run on the target session (`AttachmentInput` path from the SDK), letting `middleware-document-ingest` + `tools-document` do conversion; the instruction asks for a one-paragraph summary + memory capture (`remember`) of key facts.

- [ ] **Step 1:** Failing test: ingest a markdown fixture; subsequent `ask` in the same session answers a question about it (scripted responses steer the loop); memory store contains ≥1 record scoped to the session's tenant.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green; clippy.
- [ ] **Step 4:** Commit `Add ingest command backed by document pipeline and memory`

### Task B6: `repl` with interactions and cancel

**Files:**
- Create: `src/cli/repl.rs`
- Test: `src/cli/tests.rs`

**Interfaces:** readline loop (std stdin; no rustyline dep in v1) over one session; per turn: start run, stream to renderer; on `InteractionRequested` (elicitation toolset), render the request, read the reply, `resolve_interaction`; Ctrl-C during a run → `AgentRun::cancel()` and return to prompt (second Ctrl-C at prompt exits). `:q` quits, `:session` prints id.

- [ ] **Step 1:** Failing tests (drive the loop as a function over injected stdin/stdout, not a PTY): scripted elicitation round-trip resolves and the run completes; cancel mid-run settles the run cancelled and the loop survives.
- [ ] **Step 2:** Implement; note in the report any place the interaction API forced awkwardness (candidate SDK fixes; steering gap is expected and stays a spec non-goal).
- [ ] **Step 3:** Green; clippy.
- [ ] **Step 4:** Commit `Add repl with interaction resolution and cancel`
