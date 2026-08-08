# Repository Guidelines

## Project Structure and Module Organization

Implement the workspace defined by `docs/planning/03-finstack-ai-technical-design.md` sections 2–4; do not invent a competing layout. The kernel owns deterministic semantic state, records, events, and effects. The runtime owns ports and effect execution. The SDK owns composition. Protocol codecs and bindings remain outward-facing. Trusted native stores, providers, tools, and observers live under `extensions/`; isolated WIT/Wasmtime hosts live under `plugins/`.

`docs/planning/` is the implementation contract and is read-only during normal coding. `docs/implementation/` tracks current work and proof. Agent rules:

- `.agents/rules/01-engineering-conformance.md` — architecture, naming, generated artifacts, lifecycle, security, hard stops (always on)
- `.agents/rules/02-testing-and-delivery.md` — verification and handoff (always on)
- `.agents/rules/03-rust-coding.md` — Rust quality and rustdoc (`**/*.rs`)
- `.agents/rules/04-python-coding.md` — Python bindings, IntelliSense, docstrings (`**/*.{py,pyi}`)
- `.agents/rules/05-typescript-coding.md` — TypeScript bindings, IDE typing, TSDoc (`**/*.{ts,tsx,js,...}`)

## Implementation Workflow

Implementation code and tests are the primary deliverable. Follow this loop: select the smallest eligible logical PR, start with its referenced contracts and follow directly affected cross-references, inspect the code, implement one coherent vertical slice, add focused tests, run affected validation, update execution records, and hand off truthfully. Do not mix adjacent logical PRs for convenience or build deferred capabilities early.

Planning files are read-only during normal coding. If implementation exposes a genuine conflict or ADR trigger, stop the affected work and use change control. A threat-model review trigger does not automatically require an ADR or stop coding; complete its controls, tests, and review before merge. The current phase produces evidence for its own gate. Later-phase work requires preceding gates and entrance criteria unless explicitly parallel.

## Coding Style and Architecture

Preserve a deterministic, synchronous, I/O-free kernel; six primary ports; commit-before-effect ordering; one Rust-owned semantic engine across bindings; read-only observers; and at most one active late-tier `before_model` compaction owner per resolved agent. Prefer the smallest current-scope implementation. Do not add speculative abstractions, compatibility layers, placeholder infrastructure, or unused extension seams.

Follow the language rules in `.agents/rules/03-rust-coding.md`, `04-python-coding.md`, and `05-typescript-coding.md`. Keep public names aligned to the shared semantic vocabulary with idiomatic case per language; treat IDE typing and hover docs as part of each binding’s public API. Regenerate owned generated artifacts via documented commands and do not hand-edit them.

## Build, Test, and Review

Use checked-in mise tasks and CI commands (`mise run <task>`). Root `mise.toml` owns tool pins and repository tasks; do not add `rust-toolchain.toml` or a Cargo `xtask`. During PR-001–PR-003, standard tool commands needed to validate newly created artifacts are allowed; add the canonical mise task with that tooling. Never assume a planned task exists. Run focused checks while coding, then every affected crate, feature, target, architecture, compatibility, and security check required before handoff.

Use short imperative commit subjects. Pull requests follow Implementation Plan sections 6.1–6.2. Update implementation registers only for facts created or changed by the work. Never manufacture ownership, links, evidence, review, approval, or completion.
