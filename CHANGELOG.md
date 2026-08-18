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

- Model-assisted compaction is a runtime-owned phase between
  `PrepareContext` and `BeforeModel` (ADR-042, RFC-0001). `summarize`
  commits a child model effect under `EffectPurpose::CompactionSummary`,
  charges the same run budget, and re-enters the chain with the summary.
  Middleware stays non-effect-bearing.
- `finstack-ai-provider-gateway` is a config-driven `Model` adapter over
  `openai_responses`, `openai_chat`, `anthropic_messages`, and
  `ollama_chat`. Required profile fields fail at construction; credential
  references resolve per request with no environment fallback. Official
  vendor crates stay as reference implementations.
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

### Changed

- Official OpenAI integration now uses stateless Responses requests through
  `finstack-ai-provider-openai` and Python `Agent.openai`. Ollama now uses its
  native `/api/chat` protocol through `finstack-ai-provider-ollama`.

### Removed

- Removed the generic OpenAI-compatible Chat Completions crate, Python factory,
  browser adapter, and vLLM/LM Studio/gateway endpoint surface. This breaking
  migration remains unpublished.

### Fixed

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
