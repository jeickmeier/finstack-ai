# Repository Guidelines

## Project Structure and Module Organization

The kernel owns deterministic semantic state, records, events, and effects. The runtime owns ports and effect execution. The SDK owns composition. Protocol codecs and bindings remain outward-facing. Trusted native stores, providers, tools, and observers live under `extensions/`; isolated WIT/Wasmtime hosts live under `plugins/`. Applications that compose released components into end-user products live under `apps/`; they are trusted native code (the same class as `extensions/`), may not implement ports except by composing existing extensions, and never appear in `crates/` dependency graphs.

An `extensions/<family>/` directory names a theme, not a contract. The `impl` block is the contract: a crate may implement several ports (`finstack-ai-memory` implements `ContextProvider`, `Observer`, and `Toolset`), and several crates implement none at all because they are shared machinery for a family (`provider-wire`, `store-common`, `net-guard`). Read the crate to learn which seam it fills; do not infer it from the path.

Agent rules:

- `.agents/rules/01-engineering-conformance.md` — architecture, naming, generated artifacts, lifecycle, security, hard stops (always on)
- `.agents/rules/02-testing-and-delivery.md` — verification and handoff (always on)
- `.agents/rules/03-rust-coding.md` — Rust quality and rustdoc (`**/*.rs`)
- `.agents/rules/04-python-coding.md` — Python bindings, IntelliSense, docstrings (`**/*.{py,pyi}`)
- `.agents/rules/05-typescript-coding.md` — TypeScript bindings, IDE typing, TSDoc (`**/*.{ts,tsx,js,...}`)

## Implementation Workflow

Implementation code and tests are the primary deliverable. Follow this loop:
establish the requested behavior and current worktree, inspect directly affected
contracts, implement the smallest coherent vertical slice, add focused tests,
run affected validation, and hand off truthfully. Use the active Codex plan for
multi-step work; do not create repository-local delivery ledgers or historical
plan documents.

Each Codex plan step is a separate review and validation boundary. Parallel
work is allowed only across non-overlapping ownership boundaries, and shared
public surfaces must be reconciled before validation. Dependencies, exclusions,
architecture decisions, security review, compatibility review, and completion
criteria remain authoritative.

Snapshot the starting branch, commit, and worktree before editing. Never stash,
discard, reset, rebase, commit, or overwrite unrelated work. Pushes, hosted pull
requests or merges, publication, releases, and approval decisions are prohibited
unless separately authorized.

If implementation exposes a genuine architecture or compatibility conflict,
stop the affected workstream and use change control. A security-review trigger
does not automatically stop coding once the design is resolved, but its
controls, tests, and review must be complete before handoff.

## Coding Style and Architecture

Preserve a deterministic, synchronous, I/O-free kernel; six primary ports; commit-before-effect ordering; one Rust-owned semantic engine across bindings; read-only observers; and at most one active late-tier `before_model` compaction owner per resolved agent. Prefer the smallest current-scope implementation. Do not add speculative abstractions, compatibility layers, placeholder infrastructure, or unused extension seams.

Follow the language rules in `.agents/rules/03-rust-coding.md`, `04-python-coding.md`, and `05-typescript-coding.md`. Keep public names aligned to the shared semantic vocabulary with idiomatic case per language; treat IDE typing and hover docs as part of each binding’s public API. Regenerate owned generated artifacts via documented commands and do not hand-edit them.

## Build, Test, and Review

Use checked-in mise tasks and CI commands (`mise run <task>`). Root `mise.toml` owns tool pins and repository tasks; do not add `rust-toolchain.toml` or a Cargo `xtask`. Run focused checks while coding, then every affected crate, feature, target, architecture, compatibility, and security check required before handoff.

Use short imperative commit subjects.
