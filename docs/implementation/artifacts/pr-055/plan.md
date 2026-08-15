# PR-055 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Active branch: `codex/pr-055-anthropic-ollama-providers`
Baseline: local `main` at `0a4b477c1eb9b04d2e86294a9c01b36855ddd74c`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-055. The closed PR-054
envelope is not reused. PR-055 is the only active logical PR.
Phase 8 entrance is recorded. Do not start PR-056+. Do not write
`G7-D-*`.

## Execution envelope

Range authorized 2026-08-15; **PR-055 admitted**. Owner text:

```
Run PR-055 through PR-061 sequentially; mode=integrated; target=main;
local branch/commit/merge authorized; external actions=none;
stop before any gate crossing unless a separate passing gate decision exists.
```

Baseline: local `main` at `0a4b477c1eb9b04d2e86294a9c01b36855ddd74c`.
This envelope authorizes sequential local integrate of PR-055 then
PR-056–PR-061 after each predecessor is `Done`. Phase 8 entrance is
now recorded. It does not authorize `G7-D-*`. Do not infer G7 from
the range sentence.

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, crates.io
publish, tag, G7 inference. Do not write `G7-D-*`. Do not start
PR-056+. Do not cut or publish `0.1.0`. Do not bump the lockstep
workspace version off `0.0.4`.

## Admission (recorded)

Phase 8 entrance is `Passed` (2/2). Both bullets are recorded.
This admit does not start PR-056+ and does not write `G7-D-*`.

### Entrance bullet 1 — preceding gates

Implementation Plan Phase 8 entrance: "Native preview, Python alpha,
WASM alpha, durability beta, and plugin alpha gates passed."

| Gate | Meaning | Current state |
| --- | --- | --- |
| G3 | Native developer preview | `Passed` via `G3-D-native-preview-14a386c7db24` |
| G4 | Binding parity alpha (Python + WASM) | `Passed` via `G4-D-binding-parity-101224c5eb60` |
| G5 | Durable beta | `Passed` via `G5-D-durable-beta-a9568bd869b5` |
| G6 | Plugin alpha | `Passed` via `G6-D-plugin-alpha-018aaea9aa00` |

G3, G4, G5, and G6 are named. Entrance bullet 1 is recorded as
`PH8-E-entrance-gates-400228a63790`. Do not infer G7 from G5 or
from this admit. Do not re-record G3/G4/G5/G6.

### Entrance bullet 2 — public API change backlog

Implementation Plan Phase 8 entrance: "Public API change backlog
triaged."

Triage artifact exists at
[`public-api-change-backlog.md`](../../public-api-change-backlog.md).
Compatibility governance and the stale
[`native-preview-binding-surface.md`](../../native-preview-binding-surface.md)
are not that triage; the backlog cites them as sources.

Entrance bullet 2 is recorded as
`PH8-E-entrance-api-backlog-267035e95daa`. The backlog classifies
open public-surface deltas before `0.1.0`; it is **not** the
published preview compatibility policy (that is later Phase 8 /
PR-061 / G7).

Do not mark Phase 8 `Done`. Do not write `G7-D-*`.

### Other admission checks

- Dependency PR-004/ADR-023 is `Verified`. Dependency PR-024 is
  `Done`. Dependencies PR-032 and PR-038 are `Done`. The Model port
  is stable; do not reshape it.
- Phase 7 is `Done` with exit `Passed` (4/4). Do not re-record Phase
  7 entrance or exit.
- G5 is `Passed` via `G5-D-durable-beta-a9568bd869b5`. G7 stays
  `Not ready`. This PR is not G7.
- ADR-023 stays the Chat Completions reference plus scripted
  semantic reference. Adding Anthropic does not replace ADR-023 and
  does not require a new ADR if the Model port stays
  provider-neutral and wire DTOs stay crate-private.
- ADR-008 / ADR-020 are `Implemented` / `Verified` at G5. This PR
  validates the already-shipped catalog; it does not reopen those
  ADRs.
- ADR-014 stays `Missing`: WIT worlds remain neither remote nor
  process DTOs.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  adds no seventh port, does not move I/O into the kernel, does not
  change journal/WIT/remote policy, and does not add a native dylib
  loader.
- Threat Model section 18 is triggered (secret-bearing network
  adapter). Primary **TM-04** / **SEC-INV-005**. Also **SEC-INV-012**
  (minimal/default builds stay provider-free) and **TM-05** (do not
  add a browser Anthropic key path). Complete the review before
  merge.

## Traceability

Implementation Plan PR-055; FR-MDL-001–004; FR-CAP-003–004;
Architecture §13.6; TDD §14.3–14.6, §25.8; ADR-023.
TM-04 / SEC-INV-005 / SEC-INV-012.
G7 is Phase 8's gate and is out of scope.

## Acceptance mapping

Five Implementation Plan bullets map 1:1 to A01–A05.

- PR-055-A01: Text, tool, usage, structured-output, cancellation,
  and error fixtures pass for **each** curated provider path
  (Anthropic Messages, OpenAI-compatible, and Ollama/local). Proof
  is keyless loopback + recorded SSE/event fixtures, same pattern as
  `extensions/providers/finstack-ai-provider-openai-compatible/tests/provider_fixtures.rs`.
  Optional `#[ignore]` live smokes may exist; they are not the gate.
- PR-055-A02: Provider-specific reasoning/tool extension fields
  round-trip safely. Anthropic thinking / `tool_use` / cache
  counters and OpenAI-compatible `reasoning_content` / tool-call
  fragments normalize to existing `ReasoningDelta`, `ToolCallDelta`,
  `Usage.extension_counters`, and `OpaqueBlock` / namespaced
  settings. No new kernel `ContentBlock` variant. No new `Usage`
  field. Secrets never appear in `Debug`, errors, fixtures, or
  traces.
- PR-055-A03: Rust users depend only on the provider crate they
  need. Kernel, runtime, default SDK, `finstack-ai-wit`,
  `finstack-ai-native-examples`, and `check-wasm` graphs stay free
  of both provider crates and `reqwest`. Python users get Anthropic
  and Ollama/local in the one curated wheel through lazy submodules.
  `import finstack_ai` still constructs no HTTP client, Tokio
  runtime, or socket. `linked_providers()` becomes
  `("openai-compatible", "anthropic", "ollama")`.
- PR-055-A04: Provider diversity does not change shared activation
  traces. The same `CapabilitySpec` set yields the same
  `compact_capability_catalog()` bytes and the same
  `CapabilitiesActivated` / instruction prefix under scripted,
  OpenAI-compatible, Anthropic, and Ollama models. Repeated
  activation does not rewrite the stable prompt prefix. Cache
  breakpoints attach to existing blocks; they do not reorder
  instructions.
- PR-055-A05: Cross-provider model traces normalize to the same
  kernel semantics. A text+tool+usage fixture played through each
  adapter produces the same reducer-visible assistant/tool/usage
  shapes as the scripted model (scripted remains the semantic
  reference). Provider wire DTOs are not public runtime contracts.

Principal changes that are not extra acceptance IDs, but are
required to prove the five bullets:

- New leaf crate `finstack-ai-provider-anthropic` replacing the
  placeholder README at
  `extensions/providers/finstack-ai-provider-anthropic/`.
- First-class Ollama/local configuration on the existing
  OpenAI-compatible crate (TDD §2 tree has no `provider-ollama`
  directory; do not invent one).
- Capability negotiation plus in-provider metadata refresh hooks
  that do **not** change the `Model` trait.
- Catalog validation fixtures against OpenAI-compatible and
  Anthropic prompt/tool/cache mapping.
- Provider authoring guidance based on the three implementations
  (`ScriptedModel`, OpenAI-compatible, Anthropic).
- Python lazy submodules and `Agent.anthropic` / `Agent.ollama`
  constructors.

## Locked design

### Crate and graph

Follow Technical Design §2. Create only the Anthropic crate that
the tree already names. Ollama/native-local is a supported
`EndpointKind::Ollama` configuration of
`finstack-ai-provider-openai-compatible`, plus a Python lazy
submodule and a Rust convenience constructor.

```text
extensions/providers/finstack-ai-provider-anthropic/   # NEW crate
  Cargo.toml, README.md, src/{lib,config,error,provider,request,sse}.rs
  tests/provider_fixtures.rs
  benches/request_overhead.rs                          # warning-only

extensions/providers/finstack-ai-provider-openai-compatible/
  config/quirks already have EndpointKind::Ollama
  add ollama_local convenience + Ollama fixtures
  do not add a second HTTP stack

extensions/providers/README.md                         # authoring guide
extensions/providers/finstack-ai-provider-test/        # placeholder only; do not implement

bindings/finstack-ai-python/
  Cargo.toml links anthropic with vendored-tls
  src/lib.rs linked_providers + Agent.anthropic + Agent.ollama
  python/finstack_ai/providers/{anthropic,ollama}.py
```

Workspace member + `[workspace.dependencies]` entry for
`finstack-ai-provider-anthropic` at `0.0.4`. `publish = false` stays
on the Python package. Do not add the Anthropic crate to
`examples/rust-minimal`, default `finstack-ai` features, or
`mise run check-wasm`.

Forbidden edges:

- `finstack-ai-kernel` / `finstack-ai-runtime` / default
  `finstack-ai` / `finstack-ai-wit` / `finstack-ai-native-examples`
  / `finstack-ai-wasm` → either provider crate or `reqwest`
- Anthropic crate → kernel (runtime `Model` port only, same as
  OpenAI-compatible)
- WASM/JS → Anthropic crate or provider API keys
- any default SDK feature that pulls a network provider

Add `finstack-ai-provider-anthropic` to
`tools/wasm_package/check.py` `FORBIDDEN_WASM` (openai-compatible is
already there).

### Anthropic wire (crate-private)

Implement Anthropic **Messages** (`POST /v1/messages`), not
Completions and not a Chat Completions shim. Verify the live field
names against current Anthropic docs at implementation time; lock
this shape unless those docs disagree:

- Headers: `anthropic-version`, `x-api-key` (not Bearer by
  default), `content-type: application/json`. Redirects disabled.
  Reused `reqwest::Client`. Bounded response/SSE bytes before
  retention (copy OpenAI-compatible ceilings).
- Request: `model`, `max_tokens`, `stream: true`, `system` (stable
  instruction prefix; optional `cache_control: {type: ephemeral}`
  on the last stable system block), `messages`, `tools`,
  `tool_choice` only from allowlisted settings.
- Map canonical roles: `System` → `system`; `User` → `user`;
  `Assistant` → `assistant`; `Tool` → `user` + `tool_result`
  blocks. Assistant `tool_use` blocks carry provider tool IDs, not
  framework IDs.
- Thinking: `ModelRef.thinking_level` / allowlisted settings map to
  Anthropic thinking. Stream `thinking_delta` →
  `ModelStreamItem::ReasoningDelta`. Thinking is never assistant
  text. Optional thinking signature / cache breakpoint bytes ride
  in `OpaqueBlock` or namespaced `ModelSettings` / metadata, not a
  new kernel variant.
- Tools: `tools[].input_schema` from the committed JSON Schema.
  Structured output keeps using the framework
  `SUBMIT_FINAL_OUTPUT_TOOL` path; if Anthropic has no native JSON
  Schema response format, declare
  `StructuredOutputCapability::Prompted` and keep the tool.
- SSE events: `message_start`, `content_block_start`,
  `content_block_delta`, `content_block_stop`, `message_delta`,
  `message_stop`. Unknown event names fail closed. Duplicate
  completion / item-after-completion use the existing Model stream
  codes (`model_stream_*`) or crate-local `anthropic_stream_*`
  mapped to the same categories as OpenAI-compatible.
- Usage: `input_tokens` / `output_tokens` on `Usage`. Cache
  counters (`cache_creation_input_tokens`,
  `cache_read_input_tokens`) go in `Usage.extension_counters` under
  namespaced keys such as `anthropic.cache_creation_input_tokens`.
  Do not add kernel `Usage` fields.
- Errors: crate-local codes (`anthropic_config_invalid`,
  `anthropic_request_invalid`, `anthropic_http_error`,
  `anthropic_transport_error`, `anthropic_timeout`,
  `anthropic_cancelled`, `anthropic_stream_invalid`,
  `anthropic_stream_limit_exceeded`, `anthropic_response_invalid`).
  Same category/retryability rules as OpenAI-compatible. Canary
  secrets never appear.
- `Debug` redacts `SecretString` / `x-api-key` / custom secret
  headers. No credential persistence helper.

`AnthropicProvider` implements the existing `Model` trait only
(`descriptor`, `capabilities`, `estimate_input_tokens`, `request`,
default `warmup` / `reconcile` → `Unknown`). Do not add provider
methods to the trait.

### Ollama / native-local

`EndpointKind::Ollama` already exists (`developer_role: false`,
`stream_usage: false`, `native_structured_output: false`,
`AuthenticationConvention::Configurable`).

Add:

- `OpenAiCompatibleConfig::ollama_local(base_url)` (or equivalent
  documented constructor) that sets `EndpointKind::Ollama`,
  `Authentication::None` by default, and the Chat Completions path
  Ollama actually serves. Confirm the path at implementation time
  (`/v1/chat/completions` vs `/api/chat`); if Ollama's native
  `/api/chat` is required for a fixture, that is the only
  justification for extra private DTOs inside the **existing**
  crate. Prefer the OpenAI-compatible path. Do not create
  `finstack-ai-provider-ollama`.
- Recorded Ollama-shaped fixtures under
  `fixtures/compatibility/providers/v1/ollama/` (no
  `stream_options.include_usage`, prompted structured output, keyless
  loopback).
- Python `Agent.ollama(base_url, model, ...)` using
  `EndpointKind::Ollama`, not the current Gateway default used by
  `Agent.openai_compatible`. Do not change
  `Agent.openai_compatible` behavior.

### Capability negotiation and metadata refresh

Keep `Model::capabilities(&self, model: &ModelName)` as the port
read. Refresh is a **leaf** hook on each provider type, for example
`refresh_model_metadata(&self, model: &ModelName, update: ModelCapabilities) -> Result<(), ModelError>`
or a fixture-fed `replace_model_catalog`. Use interior mutability so
`Arc<dyn Model>` stays valid. Ordinary tests refresh from a local
table, never from a live network.

Declared flags:

| Path | `reasoning` | `prompt_cache` | `structured_output` |
| --- | --- | --- | --- |
| OpenAI-compatible (OpenAI/Azure) | true when the configured model says so; today the crate hard-codes `false` — flip only behind explicit model config, not globally | explicit model config only | `Native` when quirks allow |
| Ollama | false unless configured | false | `Prompted` |
| Anthropic | true when thinking is configured | true when cache breakpoints are configured | `Prompted` unless a checked native schema path exists |

Do not claim every endpoint is identical. Do not add a provider
router.

### Capability catalog (already shipped)

Do not redesign catalog UX (PR-022 / PR-032 / PR-038). Add fixtures
that build one agent recipe with Always + Application + Model
capabilities, then swap only the `Arc<dyn Model>`:

1. `compact_capability_catalog()` bytes are identical.
2. First activation writes the same instruction prefix bytes.
3. Second activation does not rewrite that prefix.
4. Anthropic cache_control, when enabled, marks the last stable
   prefix block and does not insert/reorder instructions.
5. OpenAI-compatible prompt/tool/cache mapping, when enabled, is
   likewise prefix-stable.

Use `ScriptedModel` as the semantic control. Provider adapters must
not change activation records.

### Python wheel

Extend the existing lazy namespace; do not eager-import providers
from `finstack_ai/__init__.py`.

```text
finstack_ai.providers.openai_compatible   # existing
finstack_ai.providers.anthropic           # NEW; is_available()
finstack_ai.providers.ollama              # NEW; is_available()
```

`linked_providers()` is the linkage source of truth. Linking the
Anthropic crate into the extension module is required (TDD §25.8 /
unstable Rust ABI). Construction of `AnthropicProvider` /
`reqwest::Client` happens only inside `Agent.anthropic` /
explicit factory calls.

Update `tests/test_import.py`:

- `linked_providers() == ("openai-compatible", "anthropic", "ollama")`
- `providers.anthropic` / `providers.ollama` stay unloaded until
  attribute access
- import/health still creates no threads and no `socket.*` audit
  events

Keep the 10 MiB wheel budget from PR-027–PR-032. Do not raise it.
Do not add Pydantic as a required dependency.

### Authoring guidance

Replace the placeholder `extensions/providers/README.md`. Document
the three implementations:

1. `ScriptedModel` — semantic reference; no HTTP.
2. `OpenAiCompatibleProvider` — Chat Completions + quirks table,
   including Ollama/local.
3. `AnthropicProvider` — Messages API leaf.

Required topics: implement `Model` only; keep wire DTOs private;
secret-safe `Debug`; fail-closed streams; recorded fixtures vs
ignored live smokes; capability flags; why Ollama is a
configuration, not a third crate. Point at the existing OpenAI
README and the new Anthropic README. Do not invent a second guide
under `docs/planning/`.

### Compatibility fixtures

```text
fixtures/compatibility/providers/v1/anthropic/
  valid--text.sse
  valid--tool.sse
  valid--structured.sse
  valid--thinking.sse
  error--http-429.json          # or equivalent recorded status
fixtures/compatibility/providers/v1/ollama/
  valid--text.sse
  valid--tool.sse
  valid--structured.sse
```

Reuse the OpenAI-compatible family for non-Ollama compatible
behavior. Do not add provider wire types to the public-rust-api
corpus.

### Benchmarks

Copy the OpenAI-compatible request-translation / reused-client
warm-path Criterion bench onto the Anthropic crate. Warning-only.
No named budget. Not PR-063. Not a merge gate.

## Tasks (when admitted)

Task IDs were minted at admit. Tracking is `Done`. Implementation
tasks remain `Todo`.

1. `PR-055-T-tracking-9cf7aa3ef48b` — Done. Phase 8 entrance
   `PH8-E-entrance-gates-400228a63790` and
   `PH8-E-entrance-api-backlog-267035e95daa` recorded. Branch
   `codex/pr-055-anthropic-ollama-providers` opened from
   `0a4b477c1eb9b04d2e86294a9c01b36855ddd74c`.
2. `PR-055-T-anthropic-0e1cc33e1ced` — Anthropic crate skeleton,
   secret-safe config, fail-closed errors, graph proofs (A03).
3. `PR-055-T-messages-8f738dbd2afb` — Messages request mapping +
   bounded SSE + recorded fixtures (A01, A02).
4. `PR-055-T-ollama-6ac13a08c9df` — Ollama/local constructor +
   recorded Ollama fixtures on the existing compatible crate (A01).
5. `PR-055-T-catalog-7452df6b3dbb` — Leaf metadata refresh +
   catalog prefix-stability fixtures (A04, A05).
6. `PR-055-T-python-4d13222686bb` — Python lazy submodules,
   `Agent.anthropic` / `Agent.ollama`, wheel-budget check (A03).
7. `PR-055-T-evidence-78f0fbe2f379` — Authoring README,
   warning-only benches, TM-04 review, candidate evidence. Stop
   before `G7-D-*`.

## Explicit exclusions

No exhaustive provider catalog or central router. No Responses API
productization (still optional on the compatible crate). No
dedicated `finstack-ai-provider-ollama` crate. No JS/WASM Anthropic
adapter and no browser-embedded provider keys. No OAuth / credential
acquisition. No live network in the default test suite. No seventh
port. No `Model` trait reshape. No new kernel `ContentBlock` /
`Usage` fields. No change to host grant math, WIT worlds, plugin
lockfile, or journal schemas. No `tools/architecture/` restore. No
`mise run schema-governance` invention. No G5 decision. No G7
decision. No `0.1.0` version bump, publish, or tag. Do not start
PR-056+ (filesystem/shell/compaction batteries).

## Validation

- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
  — no `finstack-ai-provider-anthropic`, no
  `finstack-ai-provider-openai-compatible`, no `reqwest`
- `cargo tree -p finstack-ai --locked --no-default-features` — same
- `cargo tree -p finstack-ai-native-examples -p finstack-ai-wit --locked`
  — same
- `cargo tree -p finstack-ai-provider-anthropic --locked` — has
  `finstack-ai-runtime` + `reqwest`; no kernel, no wasmtime, no
  pyo3, no plugin-host
- `cargo tree -p finstack-ai-provider-openai-compatible --locked` —
  still no kernel / wasmtime / pyo3
- `uv run --no-project python tools/wasm_package/check.py graph`
  — both provider crates forbidden on wasm/kernel
- `cargo test -p finstack-ai-provider-anthropic --offline --locked`
- `cargo test -p finstack-ai-provider-openai-compatible --offline --locked`
- `cargo test -p finstack-ai --offline --locked -- capability`
  (catalog / activation prefix fixtures)
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- focused pytest: `test_import.py` plus any new provider tests
- `mise run check` after the candidate is otherwise green

Do not require `mise run ci`, Playwright, a multi-OS hosted matrix,
or Criterion numbers to close the candidate. Live smokes stay
`#[ignore]`.

## Suggested authorization sentence

When entrance is actually `Passed` (2/2) and the owner is ready to
admit and implement:

```
Run PR-055; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

A later, separate sentence is required to record `G5-D-*` (if still
missing), to triage the public API change backlog, to record
`G7-D-*`, or to publish/tag. Do not infer those from
`implement the plan`.
