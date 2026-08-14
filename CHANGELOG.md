# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Through pre-1.0, semantic core crates, Python/JavaScript binding distributions,
and bundled first-party leaf crates share one lockstep workspace version. The
current staged alpha candidate is `0.0.2`; the exact cross-binding checkpoint
remains gated on PR-038 and G4.

## [Unreleased]

### Changed

- Stage the lockstep `0.0.2` alpha candidate with Python conformance,
  declarative capability activation, complete typing/examples, deterministic
  checksums/SBOM references, and verified hosted keyless signatures. The exact
  cross-binding checkpoint remains gated on PR-038 and G4.

### Added

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
