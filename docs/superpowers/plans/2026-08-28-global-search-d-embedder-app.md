# Plan D: ollama embedder and knowledge-app wiring

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans, task-by-task. Steps use checkbox syntax.

**Goal:** A production embedder exists
(`finstack-ai-embedder-ollama`, `POST /api/embed`), and the knowledge agent
opts in via `KnowledgeConfig.memory_embedder` (default `None` — zero
behavior change), with a best-effort startup backfill and one offline
semantic golden question.

**Architecture:** The embedder crate mirrors the ollama *provider's* HTTP
conventions (own `reqwest` client, redirects disabled, `vendored-tls`
passthrough) — not `net-guard`, whose loopback/private-IP vetting is the
wrong default for a localhost daemon. It depends on
`finstack-ai-embeddings`, never on `finstack-ai-memory`. Configuring an
embedder is the app's explicit egress decision; the default remains no
embedder. Spec §4, §6.1, §8.

**Tech Stack:** Rust; `finstack-ai-embeddings`, `finstack-ai-runtime`
(`native-tokio`), existing workspace `reqwest`/`serde`/`serde_json`/
`thiserror`/`tokio`. **No new workspace dependencies.**

**Spec:** `docs/superpowers/specs/2026-08-28-global-search-design.md`
**Depends on:** Plans A–C complete.

## Global Constraints

- Standard extension lint header; `[lints] workspace = true`;
  `publish = false`. The embedder crate and the app library never read
  environment variables; configuration values arrive as arguments.
- Verify per task: `cargo nextest run -p <crate> --locked` and matching
  clippy `-D warnings`. `cargo deny check` in any task touching
  dependencies. Never run workspace-wide tests.
- Baselines via `mise run write-public-api` when public surfaces change.
- One commit per task; short imperative subject; do not push.

---

### Task D1: `finstack-ai-embedder-ollama` scaffold

**Files:**
- Modify: `Cargo.toml` (root: member
  `extensions/embeddings/finstack-ai-embedder-ollama`; `[workspace.dependencies]` entry)
- Create: `extensions/embeddings/finstack-ai-embedder-ollama/{Cargo.toml,README.md,src/lib.rs}`
  — deps: `finstack-ai-embeddings`, `finstack-ai-runtime`
  (`default-features = false, features = ["native-tokio"]`), `reqwest`,
  `serde`, `serde_json`, `thiserror`; `[features] vendored-tls = ["reqwest/native-tls-vendored"]`
  (mirroring `finstack-ai-provider-ollama`); module stubs `config`,
  `embedder`.
- Modify: `extensions/embeddings/README.md` (list the new crate).

- [ ] **Step 1:** Failing check — `cargo check -p finstack-ai-embedder-ollama --locked`.
- [ ] **Step 2:** Scaffold + registrations.
- [ ] **Step 3:** Check + clippy; `mise run check-layering`;
  `cargo deny check`.
- [ ] **Step 4:** Commit `Add finstack-ai-embedder-ollama scaffold`

### Task D2: config + `OllamaEmbedder`

**Files:**
- Create: `src/config.rs`, `src/embedder.rs`
- Modify: `src/lib.rs`
- Test: `src/tests.rs`

**Interfaces:**

```rust
// config.rs — mirrors the ollama provider's config discipline
const DEFAULT_EMBED_PATH: &str = "/api/embed";
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(2);
const DEFAULT_MAX_INPUT_BYTES: usize = 8_192;
pub struct OllamaEmbedderConfig { /* base_url, model, dimensions, timeout, max_input_bytes */ }
impl OllamaEmbedderConfig {
    pub fn try_new(base_url: &str, model: &str, dimensions: usize) -> Result<Self, EmbedError>;
    // validates url shape, non-empty model, 1..=EMBEDDING_MAX_DIMENSIONS
}
// embedder.rs
pub struct OllamaEmbedder { /* client (redirects disabled), config */ }
impl OllamaEmbedder { pub fn try_new(config: OllamaEmbedderConfig) -> Result<Self, EmbedError>; }
impl TextEmbedder for OllamaEmbedder {
    // descriptor: embedder_id "embed.ollama.<model>.<dims>", dimensions,
    //   max_input_bytes from config
    // embed: one POST {"model", "input": [texts]} → {"embeddings": [[f32]]};
    //   validates count == inputs and every vector's dims == config;
    //   non-2xx / transport / malformed body → Unavailable with stable,
    //   non-secret message; vectors via EmbeddingVector::try_new
}
```

- [ ] **Step 1:** Failing tests against a loopback `std::net::TcpListener`
  test server (the `serve_ndjson` idiom, plain JSON here): happy path
  preserves batch order; dims mismatch from server → `Unavailable`; count
  mismatch → `Unavailable`; HTTP 500 → `Unavailable`; NaN in payload
  rejected by vector validation; descriptor id stable and encodes model +
  dims; config rejections (bad url, empty model, zero/oversize dims).
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green; clippy; `mise run write-public-api` (new crate
  baseline); `cargo deny check`.
- [ ] **Step 4:** Commit `Implement ollama text embedder`

### Task D3: knowledge config + composition wiring

**Files:**
- Modify: `apps/finstack-knowledge/Cargo.toml` (add
  `finstack-ai-embeddings`, `finstack-ai-embedder-ollama`)
- Modify: `apps/finstack-knowledge/src/config.rs` —
  `pub enum EmbedderChoice { Ollama { base_url: String, model: String, dimensions: usize } }`
  (style of `ProviderChoice`); `KnowledgeConfig` gains
  `pub memory_embedder: Option<EmbedderChoice>`, default `None` in
  `resolve()`.
- Modify: `apps/finstack-knowledge/src/compose.rs` — `memory_components()`
  builds `Arc<dyn TextEmbedder>` when configured and switches to
  `try_new_with_embedder` for provider, toolset, and observer; after the
  memory store opens, `build_agent` runs one bounded
  `reconcile_memory_embeddings(store, embedder, 64)` backfill and ignores
  its error (best-effort; a down embedder must never block startup).
- Test: `apps/finstack-knowledge/src/tests.rs`

- [ ] **Step 1:** Failing tests: default config has `memory_embedder: None`
  and every existing composition test passes unmodified; with
  `EmbedderChoice::Ollama` pointed at a scripted loopback, `build_agent`
  succeeds and the memory tool catalog advertises `mode`; with the
  endpoint down (unbound port), `build_agent` still succeeds.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Green; clippy; `cargo deny check`.
- [ ] **Step 4:** Commit `Wire optional memory embedder into knowledge agent`

### Task D4: semantic golden question

**Files:**
- Modify: `apps/finstack-knowledge/fixtures/golden.json` (one new entry)
- Modify: `apps/finstack-knowledge/src/golden.rs` + test harness — the
  scripted loopback server additionally answers `POST /api/embed`
  deterministically (compute vectors with `HashEmbedder` inside the test
  server so responses are reproducible with no model).
- Test: `apps/finstack-knowledge/src/tests.rs`

**Fixture shape:** a scripted flow that first writes a memory (scripted
`remember` tool call), then asks a **paraphrased** question sharing no
keyword or substring with the stored record — designed so lexical recall
misses and only the semantic leg can surface it — and asserts
`must_contain` plus the expected event-kind sequence.

- [ ] **Step 1:** Failing test: the new golden entry fails under a
  lexical-only composition (guard that the fixture genuinely requires the
  semantic leg), passes with the embedder configured.
- [ ] **Step 2:** Implement server route + fixture entry; fixture loader
  validation still passes (unique ids, non-empty fields).
- [ ] **Step 3:** Full plan validation: nextest + clippy for
  `finstack-ai-embeddings`, `finstack-ai-embedder-ollama`,
  `finstack-ai-memory` (`--features sqlite`), `finstack-ai-knowledge`;
  `mise run check-public-api`; `mise run check-layering`;
  `cargo deny check`.
- [ ] **Step 4:** Commit `Add semantic recall golden question`
