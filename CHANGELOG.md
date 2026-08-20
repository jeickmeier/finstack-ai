# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Semantic core crates, Python/JavaScript binding distributions, and
bundled first-party leaf crates share one lockstep workspace version.
Local tag `v1.0.0` exists at `6e9ec39fae89a70f696ee740de2d2094670cba3e`.
G8 passed via `G8-D-general-availability-a889a29a3f54`. The last
pushed GitHub tag remains `v0.1.0`. crates.io / PyPI / npm stay
unpublished.

## [Unreleased]

### Added

- Kernel constructors for crate-owned literals: `ErrorCode::from_static`,
  `Key::from_static` (including `ComponentId` / `ToolId` / `LimitKey`),
  `Digest::from_fixed_domain`, and the `UNIX_EPOCH` timestamp constant.
- Crate-level `forbid(unsafe_code)` (except FFI/host crates) plus Clippy
  denials for `unwrap` / `expect` / `panic` / `unreachable` in production
  library and binary code. Unit tests remain allowed.
- `verify_authority(&ToolCallContext)` on the runtime Tool port (ADR-048).
- `mise run check-public-api` compares `cargo-public-api` dumps for kernel, runtime, `finstack-ai`, and every `extensions/**` crate. Python/JS name lists stay in `scripts/compat/public_items.py`.
- Tools may defer a first-pass call: `ToolStreamItem::Deferred` suspends under the original effect id. `ToolSpec` gains `deferral`; stream item enums are `#[non_exhaustive]`.
- `AgentRun` child-run and `complete_external` facades plus `ChildRunBridge` for binding a deferred effect to a child run (PR-079 Rust half).
- Derived poll scheduling from committed `EffectDeferred` (`due_polls` / `drive_due_polls`); expiry uses `tool_deferral_expired`.
- Internal `ProcessConfinement` service (not a port) for fail-closed
  local process confinement. `finstack-ai-tools-shell` can consume it;
  the labeled unconfined `std::process` runner stays reachable. The
  shell crate remains T1. No E2B or remote sandbox.
- Optional same-`effect_id` model-driver retry (`SameIdentityRetryPolicy` on
  `ModelTaskConfig`). Default is zero extra attempts. Retryable provider
  errors may be retried; validation and limit errors are never retried.
  Journals of first-attempt success and success after retries stay
  identical. This is driver behavior; `check_model_conformance` is
  unchanged.
- Model-assisted compaction is a runtime-owned phase between
  `PrepareContext` and `BeforeModel` (ADR-042, RFC-0001). `summarize`
  commits a child model effect under `EffectPurpose::CompactionSummary`,
  charges the same run budget, and re-enters the chain with the summary.
  Middleware stays non-effect-bearing.
- Named credentials (`SecretString`, `Authentication`, `CredentialStore`)
  live in `provider_util` and are re-exported by the dedicated OpenAI,
  Anthropic, and Ollama crates.
- Native mid-run capability activation (`capability_list` /
  `capability_activate` in `finstack-ai-tools-skills`) unions onto the
  run-start variant without re-resolving the agent (ADR-041). Python
  `Capability` stays instruction-only. Prompt-cache invalidation after
  activation is recorded, not solved.
- `finstack-ai-tools-mcp` is an opt-in MCP client Toolset and
  `ContextProvider` for protocol revision `2026-07-28` (`tools/list` +
  `Toolset::call`; `resources/list` snapshot + untrusted collect).
  Sampling and elicitation stay unimplemented. The default `finstack-ai`
  crate does not depend on this leaf.
- `Lane::run` / `suspend` / `resume` complete the PR-047 minimum verbs:
  idle `run` uses the existing `Agent::start_on_lane` / `AcceptRun` path,
  `suspend` parks the driver without dropping the journal, and `resume`
  respawns `RunTaskOwner` through `WorkflowSession::with_ports`.
- Restored `finstack-ai-workflow-local` as the in-process `WorkflowSession`
  driver, with adapter-owned durable cron (run-once catch-up against
  `ExternalClock`, tenant-scoped table in the journal sqlite file).
- Add `finstack-ai-tools-document`: `document_parse` and `pdf_classify` tools converting pdf/docx/xlsx/pptx/odf/rtf/epub/csv to Markdown (anydoc + pdf-inspector, no OCR).
- Add `finstack-ai-middleware-document-ingest`: fail-soft `BeforeModel` middleware replacing attached-document `File` blocks with extracted Markdown in the model-visible request.
- Add `AgentRunRequest.attachments` (`AttachmentInput`, max 8 pre-staged artifacts) with Python (`Attachment`) and WASM (`attachments` run option) parity.
- The checked-in `fixtures/documents/` corpus omits a table-heavy PDF (the spec's fixture list calls for one); anydoc's own upstream test corpus already covers table-heavy PDF extraction.
- Debug parse-to-markdown helpers over `finstack-ai-tools-document`'s parser, without an `Agent` or `Run`: Python `parse_document_markdown` / `parse_document`, WASM `parseDocumentMarkdown` / `parseDocument`.
- `examples/python-notebooks/08_document_ingestion.ipynb`: an offline, scripted-model walkthrough of PDF and `.docx` document ingestion — debug parsing, scanned-PDF classification, and an `Agent.run` attachment showing the model-visible Markdown the ingest middleware injects.
- Added `finstack-ai-provider-openrouter`: OpenRouter Responses provider with
  attribution headers, provider-routing passthrough, and a model-catalog
  fetch helper; new `Agent::openrouter` constructor with Python and WASM
  binding parity.
- Added `finstack-ai-tools-openrouter-media` (five tools: image, speech,
  transcription, video generation, and video-status polling with an inline
  `wait_seconds` budget) and `finstack-ai-tools-openai-media` (three tools:
  image, speech, transcription) as native, opt-in T1 toolsets. Registrable
  from every linked constructor: `Agent::openrouter` gains `media_tools:
  bool` (reusing its own key and attribution); `Agent::openai`,
  `Agent::anthropic`, and `Agent::ollama` gain `openrouter_media:
  Option<OpenRouterMediaToolsSpec>`; `Agent::openai` additionally gains its
  own `media_tools: bool` for the native OpenAI toolset, and both may be
  active together. OpenRouter media tool calls are always billed to the
  configured OpenRouter API key. Python factories gain the matching
  `media_tools` / `openrouter_media_api_key` / `openrouter_media_referer`
  / `openrouter_media_title` keyword arguments; both crates stay off the
  wasm-host dependency graph.
- Added a host-supplied `MediaResolver` port-object contract (ADR-049,
  following the ADR-048 `CredentialStore` precedent) resolving a kernel
  `BlobRef` to bytes or a URL. `with_media_resolver` is available on all
  four provider configs (`OpenRouterConfig`, `OpenAIConfig`,
  `AnthropicConfig`, `OllamaConfig`); each also gains per-model
  `with_input_images` / `with_input_audio` / `with_input_files` toggles.
  Without a configured resolver, media-bearing user messages fail closed.
  Modality support differs by provider: OpenRouter and OpenAI accept
  images, files, and audio; Anthropic accepts images and documents but
  not audio; Ollama accepts base64 images only.
- Add `finstack-ai-store-common`: shared journal-store semantics (append
  admission, snapshot/prune admission, chain and window verification, scan
  validation) now used by both the memory and sqlite stores.
- Fix the sqlite store to reject tail-window loads that start mid-batch
  (`load_from_splits_batch` / `snapshot_splits_batch`) instead of returning a
  reconstructed batch that splits a committed one; unify the memory store's
  hole-at-start code to the `gap` reason codes.

### Changed

- `finstack-ai` linked construction finishes on
  `NativeAgentBuilder::build_linked` with shared `LinkedCommon`.
  `ComposeAgentSpec` / `Agent::compose` are gone. Registry factory and
  lifecycle types live at `finstack_ai::registry`.
  `Registrar::{model,toolset,context_provider,middleware,store,observer}_factory`
  are public production APIs; `Registry::resolve` constructs once and
  caches the ready handle. `NativeAgentBuilder` registers itself as the
  native-builder extension. `Agent::try_from_resolved` is crate-private.
  `CapabilityCatalogEntry` implements `Display` as `id: description`.
  Lane suspend/resume state is session-scoped, not process-global.
  `finstack-ai-runtime` no longer re-exports kernel types at the crate
  root. `AgentRun::start_child` / `prepare_child` take an optional remote
  route (the `_routed` twins are gone). `complete_external_at` and
  `AgentRun::child_invoker_starts` are no longer public.
  Capability-contributed ports use `capability_*` instead of
  `install_*`. `Session::pending` is crate-private. Browser WASM no
  longer exposes fail-closed linked factories; use `Agent.create`.
- `finstack-ai-server` crate-root surface is `Server`, `ListenAddr`,
  `RemoteClient`, `ReconnectView`, session replica types, auth, credit
  limits, and `tls13_server_config`. Twin reconnect structs, typed
  `serve_*` wrappers, the `ListenAddr::Loopback` variant, and unused
  re-exports (`CreditWindow`, `ConnectionLimits`, `SERVER_LISTEN_INVALID`)
  are gone. `RemotePostAuth::kind()` is the shared message-tag helper.
  Connection-only `SessionReplica` methods are crate-private.
- `finstack-ai-runtime` crate-root prelude no longer re-exports unused
  error-code constants, reconcile helpers, the jitter trio, or
  `LocalWorkflowDriver`. Dead middleware reconcile types are gone. A
  later pass also demoted unused projection/SSE/observer helpers
  (`assemble_context_projection`, `context_resume_action`,
  `ReferenceObserver`, `SseFrameParser`, `ModelRequestValidation`). The
  wire code `middleware_commit_required` stays. ADR-048
  `SECRET_MAX_BYTES` / `secret_is_valid` stay crate-root public.
- `WorkflowSession::respawn_owner` reinstalls the bound middleware chain
  and context providers after recover. `Lane::resume` binds them from the
  resolved agent so stage folds do not passthrough after suspend.
- `mise run check-public-api` also dumps `finstack-ai-runtime` with
  `--features native-tokio` and `--features wasm-host`.
- `UnknownUsagePolicy::AllowWithinReservedMaximum` records no fabricated
  observed cost. A costless completion fails only when accrued cost is
  already at or above the configured maximum (`unknown_cost_usage`).
  Exact observed equality still does not terminate. TDD 0.20.
- `BudgetRequest.extension_counters` is a `BoundedMap` (32-entry
  deserialize ceiling). Source-breaking; 1.0.0 pre-publication; no ADR;
  no major bump.
- Kernel state-hash `ContentProjection::ToolCall` now includes
  `provider_call_id`. Existing seven pinned kernel-state hashes are unchanged;
  `valid--tool-call-hash.json` pins the new tool-call-bearing digest.
- Official OpenAI integration now uses stateless Responses requests through
  `finstack-ai-provider-openai` and Python `Agent.openai`. Ollama now uses its
  native `/api/chat` protocol through `finstack-ai-provider-ollama`.
- `Agent.gateway` is a thin dispatcher onto the three dedicated providers.
  `openai_chat` is a configuration error.
- Subagent tool `subagent_await` is renamed `subagent_status`.
- `finstack-ai-middleware-verify` is an in-repo fixture (`publish = false`).
- Maintainer helpers live under `scripts/` (formerly `tools/`) so the
  directory is not confused with product toolsets.

### Removed

- Unused maintainer helpers: `scripts/loc/find_long_files.py`,
  `scripts/docs/license_sweep.py`, `scripts/docs/rehearse_release.py`,
  and `scripts/perf/test_python_fast_path.py`.

- Removed unused `FrameworkError` and `diagnostic_contains` from
  `finstack-ai-runtime`.
- Removed unused inherent methods `RecordDraft::validate_run_lineage` and
  `OperationSummary::invocation_effect_id` (1.0.0 pre-publication; no major
  bump). Callers read `RunRelation::parent_effect_id` directly.
- Removed the generic OpenAI-compatible Chat Completions crate, Python factory,
  browser adapter, and vLLM/LM Studio/gateway endpoint surface. This breaking
  migration remains unpublished.
- Removed `finstack-ai-provider-gateway`, `OpenAiChatAssembly`, and
  `provider_util/openai_chat.rs`.
- Removed the non-engine `finstack-ai-workflow-temporal` shim.

### Fixed

- `finstack-ai-server` hashes reference bearer secrets and compares the
  32-byte digests instead of short-circuiting string inequality. Command
  receipts are indexed by `command_id`, fail closed at a retention cap
  (`receipt_cap`), and apply `RemoteCommandOp` as a reference phase machine
  instead of always returning `accepted = true`.
- The Anthropic provider no longer reports zero-valued cache counters in
  `Usage.extension_counters`. Anthropic sends them on every completion, and a
  run that never registered those keys in `RunLimits` faulted with
  `unused_allocated_ids`, so every live Anthropic run failed.
- Python `Agent.anthropic` requests at most 64,000 output tokens instead of
  128,000, which Anthropic rejects for models whose own ceiling is lower.
- Session intern-table poison, sidecar `AmbiguousAcknowledgement`,
  dispatcher cancel-registry poison, and SDK event-lock poison fail
  closed instead of returning a live owner, `Ok(())`, or hanging.
- `SessionRuntime::existing` now returns `Result` and surfaces
  `session_lock_poisoned` when the intern table is unavailable.
- `Agent::start` no longer selects a model capability by incidental word
  overlap in user input. Pass optional `AgentRunRequest.capability` to
  run a model-activated variant; `None` keeps the `Agent` that was called.
  Rust struct literals that listed every `AgentRunRequest` field must
  set `capability` (use `try_new`, which defaults it to `None`).
  `AgentRunRequest` is `#[non_exhaustive]` so a later field is not
  another silent struct-literal break.
- wasm-host event hub uses `event_sequence_mismatch`, a real observer
  audience, and `BlockBounded` waits via the host driver.
- Ready-slot resolution fails closed when a per-request configuration is
  supplied (`AGENT_BUILD_CONFIGURATION_CONFLICT`) instead of ignoring it.
- Capacity preflight runs before transactional apply.
- Secret-free bundle config rejects whole-token secret keys (`auth`,
  `apikey`, `authorization`, `secretkey`, `client_secret`, and the
  existing needles). `oauth_client_id` and `author` stay allowed.
  Values are not scanned; `*_ref` keys remain allowed. Innocuous keys
  that hold credential values stay a host problem (TM-04 residual).

## [1.0.0] - 2026-08-15

Tagged lockstep general-availability cut `v1.0.0` (local; not pushed).
G8 passed via `G8-D-general-availability-a889a29a3f54`. The §21.12
external-soak gap is an accepted G8 residual. This section is not a
crates.io, PyPI, or npm publication, GitHub Release, or announce.

### Added

- 1.0 compatibility matrix and post-1.0 in-repo GA roadmap.
- Support windows written for `1.0.x`; they become in force at GA.

### Changed

- Lockstep crate, Python wheel, and `@finstack/ai` version fields move
  from unpublished `0.1.0` to unpublished `1.0.0`.
- WIT permanent worlds remain `finstack:ai-*@1.0.0` (PR-062).
  Experimental `@0.0.4` stays loadable and labeled.
- Plugin-crate `1.0.0` reservation in the WIT generator is lifted.

## [0.1.0] - 2026-08-15

Tagged lockstep public-preview cut `v0.1.0`. G7 passed via
`G7-D-public-preview-f7c7e70b9e04`. This section is not a crates.io,
PyPI, or npm publication.

### Added

- Adopter-facing preview compatibility policy and in-repo public-preview
  roadmap.
- Phase 8 exit review and G7 readiness pack language
  `READY FOR NAMED DECISION` (PR-061 local A05 step 1).

### Changed

- Lockstep crate, Python wheel, and `@finstack/ai` version fields move
  from unpublished `0.0.4` to unpublished `0.1.0`. Experimental WIT
  package names stay `finstack:ai-*@0.0.4`.
- Stage unpublished lockstep `0.0.4` artifacts for plugin alpha. G6 passed
  via `G6-D-plugin-alpha-018aaea9aa00`. Checkpoint cut, publish, and tag
  remain owner decisions.
- Stage unpublished lockstep `0.0.3` artifacts for the Phase 6 / G5 readiness
  pack. Named G5, checkpoint cut, publish, and tag remain owner decisions.
- Stage the lockstep `0.0.2` alpha candidate with Python conformance,
  declarative capability activation, complete typing/examples, deterministic
  checksums/SBOM references, and verified hosted keyless signatures. The exact
  cross-binding checkpoint remains gated on PR-038 and G4.

### Added

- Local plugin lockfile discovery (`PluginHost::load_enabled`), published
  hostile conformance rows, and the plugin-lock schema family (PR-054).
- JournalStore v1 canonical-CBOR codec, payload/envelope checksums, remaining session/lane/snapshot record variants, memory-store scan/metadata CAS, and Python/JS known-answer helpers (PR-039). SQLite, snapshot acceleration, crash durability, and G5 remain later work.
- Trusted JavaScript host adapters, AbortSignal/stream normalization, `normalizePrebetaShape`, and a tree-shakeable same-origin OpenAI-compatible fetch/SSE battery (PR-034). Agent/Run handles, workers, IndexedDB, npm publish, and G4 remain later work.
- wasm-bindgen `@finstack/ai` preview package, host-driven local executor, six-port JS promise compile fixtures, and a headless Chromium no-op trace (PR-033). Agent/Run handles, JS host adapters, workers, IndexedDB, npm publish, and G4 remain later work.
- Cargo workspace skeleton (`finstack-ai-kernel`, `finstack-ai-runtime`, `finstack-ai`, `finstack-ai-protocol`, `finstack-ai-test`)
- Placeholder Python and browser WASM binding packages
- Leaf directories under `extensions/` (providers, toolsets, stores, observers), plus `plugins/`, `examples/`, and `fixtures/`
- Canonical dual-license texts under `licenses/`, plus DCO, governance, security, and contribution documentation
- Root `mise.toml` toolchain pin with bootstrap and check tasks
- Architecture and dependency enforcement via `mise run architecture` (PR-002)
- Cross-platform CI workflows, supply-chain/secret checks, and private release-smoke binary (PR-003)
- Standalone ADR-001 through ADR-037 records with Threat Model cross-links (PR-004)
- Schema/API compatibility governance: contract registry, reserved schema/fixture roots, change-classification template, and per-family Rust fixture coupling (`crates/finstack-ai-test/tests/public_rust_api.rs`, `journal_v1.rs`, `crates/finstack-ai-protocol/tests/compat_fixtures.rs`) (PR-004)
- Pull request template API/schema/performance/security impact sections (PR-004)
- Golden-trace and scripted-input schemas, fixtures, and Rust conformance harness (PR-005)
- Criterion benchmark groups with machine-readable metadata and non-blocking `benchmark.yml` (PR-005)
- `mise run conformance`, `benchmark`, and `benchmark-smoke` tasks (PR-005)
- Native `Agent` execution facade and strict direct-handle builder, offline OpenAI-compatible model/tool-loop examples, calculator and capability-scoped filesystem batteries, and reproducible `0.0.1-dev` staging (PR-026)

### Changed

- Nest trusted native leaf batteries under `extensions/`; keep isolated WIT/Wasmtime packages under `plugins/` (documentation pack v0.11)
- Centralize canonical MIT and Apache-2.0 texts under `licenses/` and reference them from package/repository metadata (documentation pack v0.12)
