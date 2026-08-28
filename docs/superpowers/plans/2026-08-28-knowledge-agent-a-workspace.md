# Plan A: `apps/` category and the knowledge definition crate

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans, task-by-task. Steps use checkbox syntax.

**Goal:** The `apps/` workspace category exists with governance cover; `finstack-ai-knowledge` builds the composed knowledge agent from `KnowledgeConfig`, materializes self-docs, and passes offline golden-question tests. No CLI yet.

**Architecture:** One crate `apps/finstack-knowledge` (lib now, bin in Plan B). Composition-only: no port implementations, no kernel/runtime changes. Offline tests mirror `examples/rust-minimal`'s `serve_ndjson` Ollama-loopback pattern.

**Tech Stack:** Rust; `finstack-ai` (`native-tokio`), `finstack-ai-store-sqlite`, `finstack-ai-context-repository`, `finstack-ai-memory`, `finstack-ai-middleware-{instructions,document-ingest,compaction}`, `finstack-ai-tools-{document,fetch,skills}`, `finstack-ai-observer-log`, `tokio`, `serde`/`serde_json`, `thiserror`.

**Spec:** `docs/superpowers/specs/2026-08-28-knowledge-agent-design.md`

## Global Constraints

- Standard extension lint header (`#![forbid(unsafe_code)]`, deny `unwrap`/`expect`/`panic`/`unreachable`, `#![warn(missing_docs)]`); workspace lints; `publish = false`.
- Component ids `finstack.know.*`, `Version { 1, 0, 0 }`.
- The library never reads environment variables or the network at import/construct time; `build_agent` opens only `data_dir`.
- Verify per task: `cargo nextest run -p finstack-ai-knowledge --locked` and `cargo clippy -p finstack-ai-knowledge --all-targets --locked -- -D warnings`. Never run workspace-wide tests.
- One commit per task; short imperative subject; do not push.

---

### Task A1: `apps/` category + workspace registration + AGENTS.md amendment

**Files:**
- Modify: `Cargo.toml` (root: add `apps/finstack-knowledge` to `members`; crate to `[workspace.dependencies]`)
- Create: `apps/README.md` (what belongs here; trust class = trusted native, same as `extensions/`; apps compose, never implement ports)
- Create: `apps/finstack-knowledge/{Cargo.toml,README.md,src/lib.rs}` (lint header, empty exports)
- Modify: `AGENTS.md` (one sentence in Project Structure per spec §5)

- [ ] **Step 1:** Failing check — `cargo check -p finstack-ai-knowledge --locked` fails (crate absent).
- [ ] **Step 2:** Create the scaffold; README carries the parity matrix from spec §4.
- [ ] **Step 3:** `cargo check -p finstack-ai-knowledge --locked` passes; clippy clean.
- [ ] **Step 4:** Commit `Add apps category with finstack-ai-knowledge scaffold`

### Task A2: `KnowledgeConfig`, `security()`, error type

**Files:**
- Create: `apps/finstack-knowledge/src/config.rs`
- Modify: `apps/finstack-knowledge/src/lib.rs`
- Test: `apps/finstack-knowledge/src/tests.rs`

**Interfaces:** `KnowledgeConfig { data_dir: PathBuf, provider: ProviderChoice, fetch_allowlist: Vec<HostPattern> }`; `ProviderChoice::{Ollama{base_url,model}, Anthropic{api_key,model}, OpenAi{..}, OpenRouter{..}}`; `KnowledgeConfig::resolve(data_dir: Option<PathBuf>) -> Result<Self, KnowledgeError>` (default dir from `$FINSTACK_KNOW_HOME` else `$HOME/.finstack-know` — resolution takes the env *values as arguments*, a thin `from_env()` helper does the reads so the core stays env-free); `security(os_user: &str) -> Result<RunSecurityContext, KnowledgeError>` (tenant `local`, pattern of `examples/rust-minimal::security()`); `KnowledgeError` via `thiserror`, stable non-secret reason strings.

- [ ] **Step 1:** Failing tests: default dir resolution, empty-user rejection, allowlist default empty, api-key never in `Debug` output.
- [ ] **Step 2:** Implement; `Debug` for provider choices redacts keys.
- [ ] **Step 3:** Tests green; clippy.
- [ ] **Step 4:** Commit `Add knowledge config and local security projection`

### Task A3: Self-docs embed + materialization

**Files:**
- Create: `apps/finstack-knowledge/docs/{architecture.md,sessions.md,memory.md,ingestion.md,cli.md}`
- Create: `apps/finstack-knowledge/src/docs.rs`
- Test: `src/tests.rs`

(The spec's earlier draft included a doc-comment fix in
`extensions/context/finstack-ai-memory/src/lib.rs`; verified 2026-08-28 the
crate docs are already accurate — do not touch that crate.)

**Interfaces:** `pub const SELF_DOCS: &[(&str, &str)]` (topic, `include_str!` body); `materialize_self_docs(data_dir: &Path) -> Result<PathBuf, KnowledgeError>` — writes `<data_dir>/self-docs/<topic>.md` write-if-changed, returns the root.

- [ ] **Step 1:** Failing tests: materialize creates files; second call is a no-op (mtimes unchanged); changed embedded content overwrites.
- [ ] **Step 2:** Implement; write docs content (accurate module map, ports, session/lane model, memory, ingest, CLI stub section).
- [ ] **Step 3:** Green; clippy.
- [ ] **Step 4:** Commit `Embed knowledge self-docs with materialization`

### Task A4: `build_agent` composition

**Files:**
- Create: `apps/finstack-knowledge/src/compose.rs`
- Test: `src/tests.rs`

**Interfaces:** `pub async fn build_agent(config: &KnowledgeConfig) -> Result<Agent, KnowledgeError>` — sqlite journal at `<data_dir>/journal.sqlite3`; two `RepositoryInstructions` context providers (self-docs root, optional project root); memory provider/toolset/observer; instructions + document-ingest + compaction (`sliding_window`) middleware; document toolset; fetch toolset only when allowlist non-empty; skills toolset; log observer. Component ids per spec §6.

- [ ] **Step 1:** Failing tests (ollama-loopback per `rust-minimal::serve_ndjson`): agent builds; a scripted run round-trips; `compact_capability_catalog()` non-empty; empty allowlist ⇒ no fetch tool in catalog.
- [ ] **Step 2:** Implement composition.
- [ ] **Step 3:** Green; clippy.
- [ ] **Step 4:** Commit `Compose the knowledge agent from released components`

### Task A5: Golden-questions fixture + loader + offline conformance test

**Files:**
- Create: `apps/finstack-knowledge/fixtures/golden.json` (~10 entries: `{id, question, scripted_response, must_contain, event_kinds_expected}`)
- Create: `apps/finstack-knowledge/src/golden.rs`
- Test: `src/tests.rs`

- [ ] **Step 1:** Failing tests: loader validates the fixture (unique ids, non-empty fields); one test per entry runs the scripted composition, asserts `must_contain` in the result text and the observed event-kind sequence ⊇ `event_kinds_expected`.
- [ ] **Step 2:** Implement loader + test harness; author fixture entries (include at least one self-docs question and one ingestion question).
- [ ] **Step 3:** Green; clippy; `cargo deny check` still clean.
- [ ] **Step 4:** Commit `Add golden-questions fixture and offline conformance tests`
