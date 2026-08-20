# Gemini Native Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A native Gemini `generateContent` Model-port leaf (`finstack-ai-provider-gemini`) with thought-signature replay, Search grounding, code execution, context-cache accounting, and audio/video/file input, with Vertex AI as an auth/endpoint variant.

**Architecture:** New leaf crate under `extensions/providers/`, shaped byte-for-byte like the Anthropic leaf (config/error/request/provider/sse modules), with the stream normalizer as a shared `GeminiGenerateContentAssembly` in `finstack-ai-runtime`'s `provider_util`. Thought signatures round-trip through the opaque `continuation_state` blob per ADR-040; grounding/code-execution artifacts surface as `ContentBlock::Opaque`; native tools and cached-content activate via `gemini.*` settings keys.

**Tech Stack:** Rust; `reqwest` (pooled client, no redirects), `serde`/`serde_json`, `tokio`, shared `SseEventParser`; loopback-`TcpListener` fixture tests; `finstack-ai-test` conformance harness; criterion bench.

**Spec:** `docs/superpowers/specs/2026-08-20-gemini-provider-design.md`

## Global Constraints

- Lint preamble identical to other leaves: `#![forbid(unsafe_code)]`, deny `clippy::unwrap_used`, `clippy::expect_used`, `clippy::panic`, `clippy::unreachable` (re-allowed under `cfg(test)`). Copy the exact block from `extensions/providers/finstack-ai-provider-anthropic/src/lib.rs:3-22`.
- All deps `workspace = true`; no new workspace dependencies; no `eventsource` crates (SSE framing is `finstack_ai_runtime`'s `SseEventParser`).
- No env-var reads anywhere; credentials only via `CredentialStore`/`CredentialReference` (ADR-048).
- Error codes are `&'static str` consts prefixed `gemini_`; construct via the leaf-local `error()` helper.
- Continuation blobs and thought signatures are secret-bearing (SEC-INV-005): never in `Debug`, logs, or real fixtures — fixtures use fabricated signatures like `"c2ln-fixture-001"`.
- HTTPS is mandatory whenever a credential or secret header is present; keyless loopback HTTP is the only plaintext path.
- Frozen public name `Agent::gemini` (ADR-045); leaf compiles only under `native-tokio`; crate listed in `FORBIDDEN_WASM`.
- Every task ends with `cargo test -p <touched-crates>` green and a commit. Run `cargo clippy -p <crate> --all-targets` before each commit.
- Workspace version stays `1.0.0` (ADR-040 clause 8 discipline; additive change, no SemVer break).

---

### Task 1: ADR-051 — Gemini native generateContent provider

**Files:**
- Create: `docs/implementation/adrs/ADR-051-gemini-native-provider.md`
- Modify: `docs/implementation/adr-register.md` (append one row, match existing row format)
- Modify: `docs/implementation/adrs/README.md` (append index entry, match existing format)

**Interfaces:**
- Produces: the decision record later tasks cite; no code.

- [ ] **Step 1: Write the ADR**

Follow the structure of `ADR-040-openai-responses-native-ollama.md` (Status/Date/Context/Decision/Rejected alternatives/Security/Affected). Decision clauses (copy verbatim into the ADR):

1. Gemini uses a dedicated native provider (`finstack-ai-provider-gemini`) and public factory `Agent.gemini`. The wire path is `POST {endpoint}:streamGenerateContent?alt=sse`, always streaming.
2. Vertex AI is an endpoint/auth variant of the same crate (`GeminiEndpoint::Vertex { project, location }`), not a fourth protocol. The wire body and stream assembly are shared; only URL construction and credential header shape differ. The crate never mints OAuth tokens; Vertex hosts supply ready tokens via `CredentialStore`.
3. Thought-signature continuation replays complete prior model-turn `Content` objects, including `thoughtSignature` fields, from opaque `ModelResponse.continuation_state` (envelope `gemini.generate-content` v1). Providers decode that blob; the kernel does not interpret it (ADR-040 clause 3 applies).
4. `functionCall.id` is preserved as `provider_call_id` when present and omitted otherwise (ADR-040 clause 5 precedent).
5. Google Search grounding and code execution activate via the settings keys `gemini.google_search` and `gemini.code_execution`; `tools` remains a reserved settings key. Grounding metadata and code-execution parts surface as `ContentBlock::Opaque` with `application/vnd.finstack.gemini.*` media types.
6. Context caching v1 is pass-through and accounting only: `gemini.cached_content` maps to `cachedContent`; `cachedContentTokenCount`/`thoughtsTokenCount` land in `Usage::extension_counters` as `gemini.cached_content_token_count`/`gemini.thoughts_token_count`. `cachedContents` CRUD, Files API upload, and Live API are out of scope.
7. `Agent::gateway` gains a `"gemini_generate_content"` wire-protocol arm.

Security section: thought signatures and continuation blobs are secret-bearing provider state (SEC-INV-005, TM-04) — excluded from logs, observer payloads, and generated fixtures.

- [ ] **Step 2: Add register + index rows**

- [ ] **Step 3: Commit**

```bash
git add docs/implementation/adrs/ADR-051-gemini-native-provider.md docs/implementation/adr-register.md docs/implementation/adrs/README.md
git commit -m "docs: ADR-051 Gemini native generateContent provider"
```

---

### Task 2: Crate scaffold + error module

**Files:**
- Create: `extensions/providers/finstack-ai-provider-gemini/Cargo.toml`
- Create: `extensions/providers/finstack-ai-provider-gemini/src/lib.rs`
- Create: `extensions/providers/finstack-ai-provider-gemini/src/error.rs`
- Create: `extensions/providers/finstack-ai-provider-gemini/README.md` (two paragraphs: what it is, endpoint variants; follow the Anthropic README shape)
- Modify: `/Users/jeickmeier/Projects/finstack-ai/Cargo.toml` (workspace `members` list next to the other providers; `[workspace.dependencies]` path entry `finstack-ai-provider-gemini = { path = "extensions/providers/finstack-ai-provider-gemini", version = "1.0.0" }`)

**Interfaces:**
- Produces: crate `finstack-ai-provider-gemini`; `pub(crate) fn error(code, category, retryable, message) -> ModelError`; error code consts `GEMINI_CONFIG_INVALID`, `GEMINI_REQUEST_INVALID`, `GEMINI_HTTP_ERROR`, `GEMINI_TRANSPORT_ERROR`, `GEMINI_TIMEOUT`, `GEMINI_CANCELLED`, `GEMINI_STREAM_INVALID`, `GEMINI_STREAM_LIMIT_EXCEEDED`, `GEMINI_RESPONSE_INVALID` (string values `gemini_config_invalid` etc.).

- [ ] **Step 1: Cargo.toml** — copy `extensions/providers/finstack-ai-provider-anthropic/Cargo.toml` verbatim, then: name/description → `finstack-ai-provider-gemini` / "Gemini generateContent provider for finstack-ai"; drop the `finstack-ai-provider-openai` dev-dependency for now (Task 10 re-adds cross-provider parity); keep `vendored-tls`, the bench entry, `[lints] workspace = true`.

- [ ] **Step 2: lib.rs** — lint preamble copied from the Anthropic leaf, then:

```rust
mod config;
mod error;
mod provider;
mod request;
mod sse;

pub use config::{GeminiConfig, GeminiEndpoint, GeminiModelConfig, SecretHeader};
pub use provider::GeminiProvider;
```

(config/provider/request/sse land in Tasks 3–6; to keep this task compiling, create the four files as empty `//! placeholder` modules with the `pub use` lines commented, or fold Step 2's re-exports into Task 3. Prefer the second: declare only `mod error;` here and extend `lib.rs` in each later task.)

- [ ] **Step 3: error.rs** — port `extensions/providers/finstack-ai-provider-anthropic/src/error.rs` (47 lines) with `anthropic_` → `gemini_` renames, same `error()` constructor:

```rust
pub(crate) fn error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ModelError {
    ModelError::try_new(code, category, retryable, message, Metadata::empty())
        .unwrap_or_else(ModelError::from)
}
```

- [ ] **Step 4: Verify** — `cargo build -p finstack-ai-provider-gemini` and `cargo clippy -p finstack-ai-provider-gemini` pass.

- [ ] **Step 5: Commit** — `git commit -m "feat(gemini): scaffold provider crate with error taxonomy"`

---

### Task 3: Config — endpoints, auth, model fact table, capabilities

**Files:**
- Create: `extensions/providers/finstack-ai-provider-gemini/src/config.rs`
- Modify: `extensions/providers/finstack-ai-provider-gemini/src/lib.rs` (add `mod config;` + re-exports)
- Test: inline `#[cfg(test)] mod tests` in `config.rs`

**Interfaces:**
- Consumes: `finstack_ai_runtime` `provider_util::{Authentication, CredentialStore, CredentialReference, SecretString, MediaResolver}`; `ModelCapabilities`, `InputCapabilities`, `ModelContextProfile`, `TokenEstimatorRef`, `TokenEstimatorSource`, `StructuredOutputCapability`.
- Produces (exact signatures later tasks use):

```rust
pub enum GeminiEndpoint {
    GenerativeLanguage,                                  // default base https://generativelanguage.googleapis.com
    Vertex { project: Arc<str>, location: Arc<str> },    // base https://{location}-aiplatform.googleapis.com ("global" → aiplatform.googleapis.com)
}

pub struct GeminiConfig { /* private fields */ }
impl GeminiConfig {
    pub fn try_new(base_url: &str) -> Result<Self, ModelError>;      // GenerativeLanguage; base_url validated like the Anthropic leaf
    pub fn try_new_vertex(base_url: &str, project: &str, location: &str) -> Result<Self, ModelError>;
    pub fn with_credentials(self, store: Arc<dyn CredentialStore>, reference: CredentialReference) -> Self;
    pub fn with_authentication(self, auth: Authentication) -> Self;
    pub fn with_secret_header(self, header: SecretHeader) -> Result<Self, ModelError>;
    pub fn with_request_timeout(self, timeout: Duration) -> Self;
    pub fn with_stream_limits(self, max_event_bytes: usize, max_stream_bytes: usize) -> Self;
    pub fn with_media_resolver(self, resolver: Arc<dyn MediaResolver>) -> Self;
    pub(crate) fn header_map(&self) -> Result<HeaderMap, ModelError>;
    pub(crate) fn model_url(&self, model: &ModelName) -> Result<reqwest::Url, ModelError>;
}

pub struct GeminiModelConfig { /* name + numeric fact table + toggles, all with with_* builders */ }
impl GeminiModelConfig {
    pub fn try_new(name: &str, hard_input_bytes: u64, context_window_tokens: u64, max_output_tokens: u64) -> Result<Self, ModelError>;
    pub fn with_reserved_output_tokens(self, v: u64) -> Self;
    pub fn with_provider_overhead_tokens(self, v: u64) -> Self;
    pub fn with_parallel_tool_calls(self, v: bool) -> Self;
    pub fn with_thinking(self, enabled: bool, default_budget_tokens: u64) -> Self;
    pub fn with_input_images(self, v: bool) -> Self;
    pub fn with_input_audio(self, v: bool) -> Self;
    pub fn with_input_files(self, v: bool) -> Self;      // covers video/*
    pub fn with_google_search(self, v: bool) -> Self;
    pub fn with_code_execution(self, v: bool) -> Self;
    pub fn with_cached_content(self, v: bool) -> Self;
    pub fn capabilities(&self, provider: &str) -> ModelCapabilities;
    pub fn apply_capabilities(&mut self, caps: &ModelCapabilities) -> Result<(), ModelError>;
    pub(crate) fn estimator_ref() -> TokenEstimatorRef;  // id "gemini.utf8-byte-upper-bound", version "1", ConservativeUpperBound
}

pub struct SecretHeader { /* same shape as anthropic SecretHeader */ }
impl SecretHeader {
    pub fn try_new(name: &str, value: SecretString) -> Result<Self, ModelError>; // rejects authorization, x-goog-api-key, content-type
}
```

Behavioural rules (port from `anthropic/src/config.rs`, adjusting):
- `header_map()`: `Authentication::ApiKey` → sensitive `x-goog-api-key`; `Authentication::Bearer` → sensitive `Authorization: Bearer <token>`; `Authentication::None` → no auth header. Hard-fail `gemini_config_invalid` if scheme ≠ https while any credential/secret header present.
- `model_url()`: GenerativeLanguage → `{base}/{api_version}/models/{model}:streamGenerateContent?alt=sse`; Vertex → `{base}/{api_version}/projects/{project}/locations/{location}/publishers/google/models/{model}:streamGenerateContent?alt=sse`. Reject model names containing `/` or `:` with `gemini_config_invalid` (they would smuggle path segments).
- `capabilities()`: `input = InputCapabilities { text: true, json: true, images, audio, files }`; `structured_output: Native`; `native_tool_calls: true`; `parallel_tool_calls` from flag; `reasoning` from thinking flag; `prompt_cache` from cached_content; `resumable_stream: false`; `idempotent_requests: false`; `native_capabilities` = `gemini.generate-content`, `gemini.sse`, plus `gemini.google-search`/`gemini.code-execution` per flags.
- Hand-written `Debug` for `GeminiConfig`/`SecretHeader` redacting all secrets.

- [ ] **Step 1: Write failing tests** (inline mod, following `anthropic/src/config.rs:511-533`):

```rust
const CANARY: &str = "AIza-secret-canary-051";

#[test]
fn debug_never_leaks_credentials() { /* build config with ApiKey(CANARY) + SecretHeader(CANARY); assert !format!("{config:?}").contains(CANARY) */ }

#[test]
fn https_required_with_credentials() { /* http base + ApiKey → gemini_config_invalid */ }

#[test]
fn generative_language_url_shape() {
    // model "gemini-2.5-pro" → https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse
}

#[test]
fn vertex_url_shape() {
    // try_new_vertex(".../", "proj-1", "us-central1") →
    // https://us-central1-aiplatform.googleapis.com/v1/projects/proj-1/locations/us-central1/publishers/google/models/gemini-2.5-pro:streamGenerateContent?alt=sse
}

#[test]
fn model_name_with_slash_is_rejected() { /* "models/x" → gemini_config_invalid */ }

#[test]
fn secret_header_rejects_provider_owned_names() { /* x-goog-api-key, authorization, content-type */ }

#[test]
fn capabilities_reflect_flags() { /* toggles map to InputCapabilities + native_capabilities set */ }

#[test]
fn apply_capabilities_round_trips() { /* capabilities() → apply_capabilities() → capabilities() identical */ }
```

- [ ] **Step 2: Run** `cargo test -p finstack-ai-provider-gemini config` — FAIL (unresolved names).
- [ ] **Step 3: Implement** `config.rs` per the interface block above.
- [ ] **Step 4: Run** the same command — PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(gemini): config with GenerativeLanguage/Vertex endpoints and model fact table"`

---

### Task 4: Shared stream assembly in the runtime

**Files:**
- Create: `crates/finstack-ai-runtime/src/ports/model/provider_util/gemini_generate_content.rs`
- Modify: `crates/finstack-ai-runtime/src/ports/model/provider_util/mod.rs` (add `pub mod gemini_generate_content;` — match how `anthropic_messages` is exported)
- Test: inline `#[cfg(test)] mod tests` in the new file

**Interfaces:**
- Consumes: `ModelStreamItem`, `ModelResponse`, `ModelToolCall`, `ContentBlock`, `OpaqueBlock`, `OpaquePayload`, `Usage`, `LimitKey`, `RawJson`, `StreamNormError` (reuse the existing error type from `anthropic_messages`/`openai_responses`; if it is module-private, re-export it from the shared module the same way the others consume it).
- Produces:

```rust
pub const GEMINI_CONTINUATION_PROVIDER: &str = "gemini.generate-content";
pub const GEMINI_GROUNDING_MEDIA_TYPE: &str = "application/vnd.finstack.gemini.grounding-metadata";
pub const GEMINI_EXECUTABLE_CODE_MEDIA_TYPE: &str = "application/vnd.finstack.gemini.executable-code";
pub const GEMINI_CODE_RESULT_MEDIA_TYPE: &str = "application/vnd.finstack.gemini.code-execution-result";
pub const GEMINI_THOUGHTS_TOKENS_KEY: &str = "gemini.thoughts_token_count";
pub const GEMINI_CACHED_TOKENS_KEY: &str = "gemini.cached_content_token_count";

pub struct GeminiGenerateContentAssembly { /* accumulators */ }
impl GeminiGenerateContentAssembly {
    pub fn new(request_id: String, structured: bool) -> Self;
    /// One SSE `data:` payload (a GenerateContentResponse chunk).
    pub fn consume(&mut self, data: &str) -> Result<Vec<ModelStreamItem>, StreamNormError>;
    /// True once a Completed item has been emitted.
    pub fn completed(&self) -> bool;
    /// Called at EOF; error if no finishReason chunk arrived.
    pub fn finish(&mut self) -> Result<(), StreamNormError>;
}
```

Assembly rules (spec §7):
- Parse each chunk as `{ candidates, usageMetadata, promptFeedback, responseId, modelVersion }`. Only `candidates[0]` is read.
- Text parts: `thought != true` → emit `TextDelta` and append to the assistant text accumulator; `thought == true` → emit `ReasoningDelta` only (not persisted).
- `functionCall` parts → emit `ToolCallDelta`; record `ModelToolCall { name, arguments: args as RawJson, provider_call_id: id }`.
- Every part is also appended verbatim (raw JSON) to the model-turn `replay_contents` accumulator so signatures survive untouched.
- `groundingMetadata` / `executableCode` / `codeExecutionResult` → collect as `ContentBlock::Opaque` (media types above, payload `OpaquePayload::Json`).
- `usageMetadata` is cumulative: emit `UsageDelta` per chunk carrying the mapped `Usage`; keep only the latest for the terminal. Extension counters: thoughts/cached keys, zeros suppressed.
- First chunk with `candidates[0].finishReason`:
  - `STOP` / `MAX_TOKENS` → build `Completed(ModelResponse { assistant_content, tool_calls, usage, provider_ids (responseId), completion_id: responseId or request_id fallback, continuation_state: Some(envelope) })`. Envelope: `{"provider":"gemini.generate-content","version":1,"replay_contents":[{"role":"model","parts":[…verbatim…]}]}`.
  - Any other finishReason, or `promptFeedback.blockReason` with no candidates → `StreamNormError` mapping to a non-retryable response error.
- If `structured` is true, the terminal must have non-empty assistant text (the JSON document) — mirror how `OpenAiResponsesAssembly` gates structured terminals.
- `finish()` after no `Completed` → error (leaf maps to `gemini_stream_invalid`).

- [ ] **Step 1: Write failing tests** in the new module (event fixtures as `&str` literals):

```rust
#[test] fn text_stream_assembles_completed_response() { /* 3 chunks: text, text, finishReason STOP + usage; expect TextDelta×2, UsageDelta, Completed with concatenated text and usage 10/5/15 */ }
#[test] fn thought_parts_emit_reasoning_deltas_and_are_not_persisted() { /* thought:true part → ReasoningDelta; Completed.assistant_content has no reasoning text */ }
#[test] fn thought_signature_lands_in_continuation_not_content() { /* part with thoughtSignature "c2ln-fixture-001"; Completed.continuation_state JSON contains it; assistant_content and deltas do not */ }
#[test] fn function_call_maps_to_tool_call_with_optional_id() { /* with id → provider_call_id Some; without → None */ }
#[test] fn grounding_metadata_becomes_opaque_block() { /* groundingMetadata on final chunk → one Opaque block with GEMINI_GROUNDING_MEDIA_TYPE */ }
#[test] fn usage_extension_counters_map_and_suppress_zeros() { /* thoughtsTokenCount 7, cachedContentTokenCount 0 → only gemini.thoughts_token_count present */ }
#[test] fn safety_finish_reason_is_an_error() {}
#[test] fn eof_without_finish_reason_is_an_error() { /* consume one text chunk then finish() → Err */ }
```

- [ ] **Step 2: Run** `cargo test -p finstack-ai-runtime gemini_generate_content` — FAIL.
- [ ] **Step 3: Implement** the assembly.
- [ ] **Step 4: Run** — PASS. Also `cargo test -p finstack-ai-runtime` (no regressions in provider_util).
- [ ] **Step 5: Commit** — `git commit -m "feat(runtime): shared GeminiGenerateContentAssembly stream normalizer"`

---

### Task 5: SSE wrapper

**Files:**
- Create: `extensions/providers/finstack-ai-provider-gemini/src/sse.rs`
- Modify: `src/lib.rs` (`mod sse;`)
- Test: inline tests

**Interfaces:**
- Consumes: `finstack_ai_runtime::SseEventParser`, `SseEvent`, `SseParseError`.
- Produces: `pub(crate) struct GeminiSse` with `new(max_event_bytes: usize)`, `push(&mut self, bytes: &[u8]) -> Result<Vec<String>, ModelError>` returning the `data` payloads. **Unlike the Anthropic wrapper, unnamed events are accepted** (Gemini frames carry no `event:` field); a frame with an event name other than empty/`message` is `gemini_stream_invalid`. `[DONE]`-style sentinels do not exist on this protocol; treat empty `data` frames as no-ops.

- [ ] **Step 1: Write failing tests** — port `anthropic/src/sse.rs` tests: fragmented frame across two `push` calls reassembles; oversize event → `gemini_stream_limit_exceeded`; unnamed `data:` frame is accepted (inverted from Anthropic's test).
- [ ] **Step 2: Run** `cargo test -p finstack-ai-provider-gemini sse` — FAIL.
- [ ] **Step 3: Implement** (≈60 lines, mirror `anthropic/src/sse.rs` minus the named-event requirement).
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(gemini): SSE wrapper accepting unnamed data frames"`

---

### Task 6: Request translation

**Files:**
- Create: `extensions/providers/finstack-ai-provider-gemini/src/request.rs`
- Modify: `src/lib.rs` (`mod request;`)
- Test: inline tests (largest test module in the crate; mirror `anthropic/src/request.rs` coverage)

**Interfaces:**
- Consumes: `ModelRequestDraft`, `Message`, `ContentBlock`, `ToolSpec`, `OutputSpec`, `ModelSettings`, `GeminiModelConfig` (Task 3), `resolve_draft_media` output types (`ResolvedMedia`), `GEMINI_CONTINUATION_PROVIDER` (Task 4).
- Produces:

```rust
pub(crate) struct GenerateContentRequest { /* Serialize, rename_all = "camelCase" */ }
impl GenerateContentRequest {
    pub(crate) fn try_from_draft(
        draft: &ModelRequestDraft,
        model: &GeminiModelConfig,
        continuation: Option<&RawJson>,
        resolved_media: &ResolvedMediaMap,   // whatever type resolve_draft_media hands the Anthropic leaf — reuse identically
    ) -> Result<Self, ModelError>;
    pub(crate) fn serialize(&self) -> Result<Vec<u8>, ModelError>;
}
```

Wire DTOs (crate-private, `#[derive(Serialize)]`, `#[serde(rename_all = "camelCase")]`, `skip_serializing_if = "Option::is_none"` on every optional):

```rust
struct GenerateContentRequest {
    contents: Vec<WireContent>,
    system_instruction: Option<WireContent>,
    tools: Option<Vec<serde_json::Value>>,     // functionDeclarations entry + native-tool objects
    generation_config: Option<GenerationConfig>,
    cached_content: Option<String>,
    #[serde(flatten)] extra: serde_json::Map<String, serde_json::Value>, // settings passthrough (e.g. safetySettings)
}
struct WireContent { role: Option<&'static str>, parts: Vec<serde_json::Value> }
struct GenerationConfig {
    max_output_tokens: u64,
    candidate_count: u32,                       // always 1
    response_mime_type: Option<&'static str>,
    response_json_schema: Option<serde_json::Value>,
    thinking_config: Option<ThinkingConfig>,
}
struct ThinkingConfig { thinking_budget: u64, include_thoughts: bool }
```

Translation rules (spec §6 — implement each as its own function so tests target them):
- `RESERVED_SETTINGS: &[&str]` = `model, contents, systemInstruction, system_instruction, tools, toolConfig, tool_config, stream, generationConfig, generation_config, cachedContent, cached_content, responseSchema, responseJsonSchema, responseMimeType` → hard `gemini_request_invalid`. Consumed keys (`gemini.google_search`, `gemini.code_execution`, `gemini.cached_content`, `gemini.thinking`, `thinking_level`) are popped before the flatten; everything else flattens top-level.
- `map_messages`: system/developer prefix → `system_instruction`; assistant → `role:"model"`; tool-result messages → `role:"user"` with `functionResponse{name, id?, response}` where `name`/`id` come from scanning earlier messages for the `ToolCall` block with the matching `tool_call_id` (missing match → `gemini_request_invalid`).
- `map_tools`: one `{"functionDeclarations":[{name, description, parameters}]}` entry; append `{"googleSearch":{}}` when `gemini.google_search` is truthy **and** the model flag allows it (flag off → `gemini_request_invalid`); same for `{"codeExecution":{}}`. Native-tool object form (`gemini.google_search: {…}`) passes the object through under the `googleSearch` key.
- `take_thinking`: precedence `gemini.thinking` setting > `thinking_level` allowlist (`low|medium|high` → 1024/4096/8192) > model default; budget must be `< max_output_tokens` else `gemini_request_invalid`; `include_thoughts: true` whenever thinking is on; model flag off + explicit setting → error.
- Structured output: `OutputSpec::JsonSchema` → `response_mime_type: Some("application/json")` + `response_json_schema`; presence of the `finstack.internal.submit_final_output` tool → `gemini_request_invalid` (Native providers reject the prompted-mode tool; copy the OpenAI leaf's check).
- `cached_content`: from `gemini.cached_content` string setting; model flag off → error.
- Continuation replay: `parse_continuation` rejects provider/version mismatch (`gemini_request_invalid`); replay `replay_contents` verbatim as leading `contents` after the last replayed point, appending only messages after the last assistant message — port `map_input` from `extensions/providers/finstack-ai-provider-openai/src/request.rs:260-283`.
- Media: `ContentBlock::Image/Audio/File` → resolved via the media map; `Url` → `fileData{fileUri, mimeType}`, `Bytes` → `inlineData{mimeType, data: base64}`; per-model flags gate each kind (`input_files` gates both `application/*` and `video/*`); flag off → `gemini_request_invalid`. Outbound `ContentBlock::Opaque` → dropped.
- `max_output_tokens = draft.limits.max_output_tokens.min(model.max_output_tokens)`.

- [ ] **Step 1: Write failing tests**:

```rust
#[test] fn reserved_settings_are_rejected() {}
#[test] fn system_prefix_becomes_system_instruction_and_late_system_errors() {}
#[test] fn tool_result_maps_to_function_response_with_name_lookup() {}
#[test] fn google_search_setting_appends_native_tool() { /* body JSON contains {"googleSearch":{}} beside functionDeclarations */ }
#[test] fn google_search_rejected_when_model_flag_off() {}
#[test] fn code_execution_setting_appends_native_tool() {}
#[test] fn thinking_level_maps_to_budget_and_include_thoughts() {}
#[test] fn structured_output_uses_response_json_schema() { /* + submit_final_output tool present → error */ }
#[test] fn cached_content_setting_maps_to_body_field() {}
#[test] fn continuation_replays_contents_verbatim_and_appends_tail() { /* envelope with thoughtSignature; serialized body contains the signature; only post-assistant messages appended */ }
#[test] fn continuation_provider_mismatch_is_rejected() {}
#[test] fn image_url_maps_to_file_data_and_bytes_to_inline_data() {}
#[test] fn video_media_type_rides_on_input_files_flag() {}
#[test] fn audio_rejected_when_flag_off() {}
#[test] fn candidate_count_is_pinned_to_one() {}
```

- [ ] **Step 2: Run** `cargo test -p finstack-ai-provider-gemini request` — FAIL.
- [ ] **Step 3: Implement** `request.rs`.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(gemini): draft-to-generateContent request translation"`

---

### Task 7: Provider struct + Model impl

**Files:**
- Create: `extensions/providers/finstack-ai-provider-gemini/src/provider.rs`
- Modify: `src/lib.rs` (`mod provider;` + full re-export list from Task 2)
- Test: inline tests

**Interfaces:**
- Consumes: everything above plus `Model`, `PortFuture`, `ModelEventStream`, `ModelDescriptor`, `ModelTokenEstimate`, `resolve_draft_media`, `GeminiGenerateContentAssembly`.
- Produces:

```rust
pub struct GeminiProvider {
    client: reqwest::Client,
    config: GeminiConfig,
    models: RwLock<BTreeMap<ModelName, GeminiModelConfig>>,
}
impl GeminiProvider {
    pub fn try_new(config: GeminiConfig, models: Vec<GeminiModelConfig>) -> Result<Self, ModelError>;
    pub fn replace_model_catalog(&self, models: Vec<GeminiModelConfig>) -> Result<(), ModelError>;
    pub fn refresh_model_metadata(&self, name: &ModelName, caps: &ModelCapabilities) -> Result<(), ModelError>;
}
impl Model for GeminiProvider { /* descriptor, capabilities, estimate_input_tokens, request */ }
```

Port `anthropic/src/provider.rs` structure exactly:
- `try_new`: pooled client, `default_headers(config.header_map()?)`, `redirect(Policy::none())`; `catalog_from_models` rejects empty/duplicate catalogs.
- `estimate_input_tokens` = `canonical_request.len()` upper bound with `GeminiModelConfig::estimator_ref()`.
- `request()`: clone what's needed; in the boxed future — resolve media → `GenerateContentRequest::try_from_draft` → POST `config.model_url(&model)` with `x-client-request-id: <ModelRequestId>` + `content-type: application/json` + per-request timeout, wrapped in `tokio::select!` with `cancellation.cancelled()` (→ `gemini_cancelled`). Non-2xx → `gemini_http_error`, `retryable = matches!(status, 408 | 409 | 429 | 500..=599)`. Then `mpsc::channel(32)` + `tokio::spawn(drive_response)` + `ReceiverModelStream` whose `Drop` aborts the task (copy `anthropic/src/provider.rs:326-334`).
- `drive_response`: three-way `select!` (cancellation / `sender.closed()` / next chunk); bytes → `GeminiSse::push` → each data payload → `assembly.consume`; return once `Completed` is forwarded; at EOF call `assembly.finish()` and map errors via `map_norm`; enforce `max_stream_bytes` (`gemini_stream_limit_exceeded`).
- `transport_error(&reqwest::Error)`: `is_timeout()` → `gemini_timeout`/`ErrorCategory::Deadline`/retryable, else `gemini_transport_error`.

- [ ] **Step 1: Write failing tests** (no network — recorded event sequences through the assembly, same style as `anthropic/src/provider.rs` inline tests): `recorded_text_sequence_assembles_response`, `zero_extension_counters_are_not_reported`, `missing_media_resolver_fails_closed`, `duplicate_model_names_rejected`.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** `provider.rs`.
- [ ] **Step 4: Run** `cargo test -p finstack-ai-provider-gemini` — all crate tests PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(gemini): GeminiProvider Model implementation with SSE drive loop"`

---

### Task 8: Wire fixtures + loopback HTTP tests

**Files:**
- Create: `fixtures/compatibility/providers/v1/gemini/valid--text.sse`
- Create: `fixtures/compatibility/providers/v1/gemini/valid--tool.sse`
- Create: `fixtures/compatibility/providers/v1/gemini/valid--structured.sse`
- Create: `fixtures/compatibility/providers/v1/gemini/valid--thinking.sse` (fabricated `thoughtSignature: "c2ln-fixture-001"`)
- Create: `fixtures/compatibility/providers/v1/gemini/valid--grounding.sse`
- Create: `fixtures/compatibility/providers/v1/gemini/error--http-429.json`
- Create: `extensions/providers/finstack-ai-provider-gemini/tests/provider_fixtures.rs`

**Interfaces:**
- Consumes: `GeminiProvider` + configs; fixture files via `include_str!`.
- Produces: the fixture corpus Task 9's conformance test replays.

Fixture shape — unnamed SSE frames, e.g. `valid--text.sse`:

```
data: {"candidates":[{"content":{"role":"model","parts":[{"text":"Hel"}]},"index":0}],"responseId":"resp-fixture-1","modelVersion":"gemini-2.5-pro"}

data: {"candidates":[{"content":{"role":"model","parts":[{"text":"lo"}]},"index":0}],"responseId":"resp-fixture-1"}

data: {"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP","index":0}],"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":4,"totalTokenCount":16},"responseId":"resp-fixture-1"}

```

- [ ] **Step 1: Write the six fixtures.** `valid--thinking.sse` includes a `{"text":"…","thought":true}` part and a signed part; `valid--tool.sse` a `functionCall` with and without `id`; `valid--grounding.sse` a final chunk with `groundingMetadata` (webSearchQueries + one groundingChunk); `valid--structured.sse` a JSON-text answer; usage in every terminal chunk (thinking one adds `thoughtsTokenCount`, grounding one adds `cachedContentTokenCount`).
- [ ] **Step 2: Write failing loopback tests** — port `serve_response(status, content_type, body, hold_open)` from `extensions/providers/finstack-ai-provider-anthropic/tests/provider_fixtures.rs:425` verbatim. Tests (all `#[tokio::test(flavor = "multi_thread")]`):

```rust
#[tokio::test] async fn text_fixture_end_to_end() { /* also assert captured request contains "\"candidateCount\":1" and header x-goog-api-key absent when keyless */ }
#[tokio::test] async fn tool_fixture_yields_tool_calls() {}
#[tokio::test] async fn thinking_fixture_round_trips_signature_into_continuation() { /* run once, take continuation_state, issue second request with it, assert captured body contains "c2ln-fixture-001" */ }
#[tokio::test] async fn grounding_fixture_yields_opaque_block() {}
#[tokio::test] async fn structured_fixture_completes() {}
#[tokio::test] async fn http_429_maps_to_retryable_gemini_http_error() {}
#[tokio::test] async fn api_key_header_sent_and_never_logged() { /* captured contains x-goog-api-key; provider Debug does not contain the key */ }
#[tokio::test] async fn cancellation_aborts_stream() { /* hold_open server + fire CancellationSignal → gemini_cancelled */ }
```

- [ ] **Step 3: Run** `cargo test -p finstack-ai-provider-gemini --test provider_fixtures` — FAIL, then fix translation/assembly gaps until PASS.
- [ ] **Step 4: Commit** — `git commit -m "test(gemini): wire fixtures and loopback provider tests"`

---

### Task 9: Conformance harness

**Files:**
- Create: `extensions/providers/finstack-ai-provider-gemini/tests/conformance.rs`

**Interfaces:**
- Consumes: `finstack_ai_test::conformance::ports::{ModelConformanceCase, check_model_conformance, ModelTerminal, AssembledModelStream}`; fixtures from Task 8.

- [ ] **Step 1: Write the test** — port `extensions/providers/finstack-ai-provider-anthropic/tests/conformance.rs:180-198`: loopback server per fixture, expected terminal built by replaying the same events through `GeminiGenerateContentAssembly`, then `check_model_conformance` for the text, tool, thinking, and structured fixtures. The harness validates descriptor membership, capability/estimator binding, stream ordering, and terminal stability.
- [ ] **Step 2: Run** `cargo test -p finstack-ai-provider-gemini --test conformance` — PASS (fix any ordering violations it surfaces).
- [ ] **Step 3: Commit** — `git commit -m "test(gemini): model-port conformance suite"`

---

### Task 10: SDK wiring — Agent::gemini, gateway arm, feature gating

**Files:**
- Modify: `crates/finstack-ai/Cargo.toml` (`native-tokio` feature adds `dep:finstack-ai-provider-gemini`; `[dependencies]` entry with `features = ["vendored-tls"]`, optional — copy the anthropic lines at `:19-29`/`:36-39`)
- Modify: `crates/finstack-ai/src/agent/linked.rs`:
  - Add `gemini_inner` next to `anthropic_inner` (`:484-524` shape): `GeminiConfig::try_new(spec.endpoint)` (never hardcode the Google host — ADR-047 rule), `Authentication::ApiKey` default, component ids `python.agent.gemini` / `python.bundle.gemini` / `python.model.gemini`, then `build_linked_provider`.
  - Public `Agent::gemini` under `#[cfg(feature = "native-tokio")]`; wasm-host mirror returning `agent_run_unsupported_plan` (copy the block at `:783-810`).
  - `gateway_provider`: add `"gemini_generate_content"` arm to the wire-protocol match (`:600-620`), ApiKey auth shape.
- Modify: wherever `FORBIDDEN_WASM` lists leaf crates (grep `FORBIDDEN_WASM` — per ADR-045 it must include every leaf): add `finstack-ai-provider-gemini`.
- Test: extend the existing gateway/constructor tests in `crates/finstack-ai` (find the tests covering `gateway_provider` protocol strings and add the gemini arm + a `"openai_chat"`-style negative already covered).

**Interfaces:**
- Consumes: `GeminiProvider::try_new`, `GeminiConfig`, `GeminiModelConfig` (Tasks 3/7).
- Produces: `Agent::gemini(spec) -> …` (match the exact parameter/return shape of `Agent::anthropic` — read it first and mirror it); gateway accepts `wire_protocol = "gemini_generate_content"`.

- [ ] **Step 1: Write failing tests** — constructor smoke test (builds an `Agent` with a loopback endpoint + `ScriptedModel`-style catalog and asserts the resolved model component id), gateway arm test (`"gemini_generate_content"` resolves; misspelling stays a configuration error).
- [ ] **Step 2: Run** `cargo test -p finstack-ai` — FAIL.
- [ ] **Step 3: Implement** the wiring.
- [ ] **Step 4: Run** `cargo test -p finstack-ai` and `cargo build -p finstack-ai --no-default-features --features wasm-host` — both green.
- [ ] **Step 5: Commit** — `git commit -m "feat(sdk): Agent::gemini constructor and gateway gemini_generate_content arm"`

---

### Task 11: Capability-catalog parity + bench

**Files:**
- Create: `extensions/providers/finstack-ai-provider-gemini/tests/capability_catalog.rs`
- Create: `extensions/providers/finstack-ai-provider-gemini/benches/request_overhead.rs`
- Modify: `extensions/providers/finstack-ai-provider-anthropic/tests/capability_catalog.rs` (add Gemini to the byte-identical catalog assertion at `:22-35` — this test is the cross-provider registry; keep the dev-dependency direction Anthropic → Gemini and add `finstack-ai-provider-gemini` to Anthropic's `[dev-dependencies]`)
- Modify: `extensions/providers/finstack-ai-provider-gemini/Cargo.toml` (dev-dep on `finstack-ai`, `finstack-ai-test`, `finstack-ai-store-memory` if not already present from Task 2's copy)

**Interfaces:**
- Consumes: `ScriptedModel` from `finstack-ai-test`; `Agent::gemini` from Task 10.

- [ ] **Step 1: Parity test** — build the same Agent against `ScriptedModel` and `GeminiProvider`, assert the resolved capability catalog string is byte-identical (port `anthropic/tests/capability_catalog.rs:22-35`); add `leaf_metadata_refresh_updates_advertised_flags_without_a_network` covering `refresh_model_metadata` (`:38` pattern).
- [ ] **Step 2: Bench** — port `anthropic/benches/request_overhead.rs`: translation + serialization + assembly of the text fixture, no I/O.
- [ ] **Step 3: Run** `cargo test -p finstack-ai-provider-gemini -p finstack-ai-provider-anthropic` and `cargo bench -p finstack-ai-provider-gemini --no-run` — green.
- [ ] **Step 4: Commit** — `git commit -m "test(gemini): capability parity and request-overhead bench"`

---

### Task 12: Bindings, baselines, docs

**Files:**
- Modify: `bindings/finstack-ai-python/Cargo.toml` (add `finstack-ai-provider-gemini` dep, matching the other leaves at `:35`)
- Modify: `bindings/finstack-ai-python/src/protocol.rs` (or wherever `Agent.anthropic` is exposed at `:11-30` — read first): add the `Agent.gemini` argument-mapping facade, same arguments as the Rust constructor, same error codes (ADR-045: no logic in the facade)
- Modify: `bindings/finstack-ai-wasm/src/lib.rs:757-778` region: add the `gemini` method returning `agent_run_unsupported_plan`
- Modify: JS/Python public-api baselines — run the repo's baseline regeneration (see recent commit `306b776 "chore: regenerate public-api baselines"`; find the command in `mise.toml` tasks, likely `mise run <baseline task>`) and commit the diff
- Modify: `docs/site/provider.md` — add a Gemini section next to the OpenRouter one (`:26-33` style): constructor snippet, endpoint variants table, the `gemini.*` settings keys, extension-counter names, out-of-scope list
- Modify: `extensions/providers/finstack-ai-provider-gemini/README.md` — finalize with the settings-key table

**Interfaces:**
- Consumes: `Agent::gemini` (Task 10).

- [ ] **Step 1: Python facade + test** — mirror the existing binding test for `Agent.anthropic` (search `bindings/finstack-ai-python` tests for it) with a keyless loopback config; run the Python test suite per `mise.toml`.
- [ ] **Step 2: WASM stub** — build `bindings/finstack-ai-wasm` and assert the method returns `agent_run_unsupported_plan` in its existing stub test pattern.
- [ ] **Step 3: Regenerate baselines** — run the baseline task; verify only additive `gemini` entries appear in the diff.
- [ ] **Step 4: Docs** — provider.md + README.
- [ ] **Step 5: Full gate** — `cargo test --workspace` (or the `mise` test task the repo uses), `cargo clippy --workspace --all-targets`, plus the wasm-host build from Task 10 Step 4.
- [ ] **Step 6: Commit** — `git commit -m "feat(bindings): Agent.gemini facades, baselines, provider docs"`

---

## Self-review notes

- Spec §2.1–§2.7 ↔ Tasks: crate+protocol (2–9), Vertex variant (3), thought signatures (4, 6, 8), native tools (6, 8), caching (4, 6), constructor/gateway/wasm (10, 12), ADR (1). Fixtures/conformance (8–9) cover spec §8.
- Type-consistency: `GeminiConfig`/`GeminiEndpoint`/`GeminiModelConfig`/`SecretHeader` (Task 3) are the names consumed in 6, 7, 10; `GeminiGenerateContentAssembly` + constants (Task 4) consumed in 7, 9; error consts (Task 2) used throughout.
- Known judgment calls an executor must NOT silently change: unnamed-SSE acceptance (Task 5), `candidateCount` pinned to 1, `idempotent_requests: false`, Opaque media-type strings, settings-key names (`gemini.google_search`, `gemini.code_execution`, `gemini.cached_content`, `gemini.thinking`), extension-counter keys, continuation envelope provider string `gemini.generate-content`.
