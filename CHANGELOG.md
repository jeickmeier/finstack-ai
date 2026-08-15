# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Through pre-1.0, semantic core crates, Python/JavaScript binding distributions,
and bundled first-party leaf crates share one lockstep workspace version. The
current staged unpublished candidate is `0.1.0`. Named G6 has passed.
Named G7, publish, and tag remain owner decisions.

## [Unreleased]

## [0.1.0] - 2026-08-15

Unpublished lockstep public-preview candidate. This section is not a
named G7 decision, git tag, or registry publish.

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
- Schema/API compatibility governance: contract registry, reserved schema/fixture roots, change-classification template, and `mise run schema-governance` enforcement (PR-004)
- Pull request template API/schema/performance/security impact sections (PR-004)
- Golden-trace and scripted-input schemas, fixtures, and Rust conformance harness (PR-005)
- Criterion benchmark groups with machine-readable metadata and non-blocking `benchmark.yml` (PR-005)
- `mise run conformance`, `benchmark`, and `benchmark-smoke` tasks (PR-005)
- Native `Agent` execution facade and strict direct-handle builder, offline OpenAI-compatible model/tool-loop examples, calculator and capability-scoped filesystem batteries, and reproducible `0.0.1-dev` staging (PR-026)

### Changed

- Nest trusted native leaf batteries under `extensions/`; keep isolated WIT/Wasmtime packages under `plugins/` (documentation pack v0.11)
- Centralize canonical MIT and Apache-2.0 texts under `licenses/` and reference them from package/repository metadata (documentation pack v0.12)
