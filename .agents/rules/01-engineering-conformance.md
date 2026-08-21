---
trigger: always_on
description: Enforce finstack-ai architecture, boundaries, naming, generated-artifact, lifecycle, security, and hard-stop constraints while coding.
globs:
---

# Engineering conformance

Apply this rule to code, schemas, build configuration, generated artifacts, and implementation review. Start with the active Codex workstream's requested behavior, architecture decisions, public contracts, standards, and security controls, then follow directly affected cross-references. Do not audit unrelated documentation. Current contracts define behavior; code cannot reinterpret them silently.

Language-specific coding and docstring rules live in `03-rust-coding.md`, `04-python-coding.md`, and `05-typescript-coding.md`. Verification and handoff rules live in `02-testing-and-delivery.md`.

## Establish the coding slice

- Prefer the smallest implementation that completes the selected behavior. New abstractions require a current requirement and a concrete use; placeholder frameworks and unused extension seams are prohibited.

## Execute planned workstreams

Before editing:

- Pin the active Codex plan, starting branch and commit, worktree state, authorized local actions, and any separately authorized external actions.
- Read the complete active step and its directly affected contracts, architecture decisions, security controls, acceptance criteria, dependencies, and exclusions.
- Keep each workstream a coherent review unit. Parallel work requires explicit, non-overlapping file or subsystem ownership.
- Snapshot existing changes. Never stash, discard, reset, rebase, commit, or overwrite unrelated work. If concurrent changes overlap owned files, generated outputs, manifests, or public contracts and ownership cannot be proved, stop and request direction.

At each transition, recheck the plan, validation evidence, decision state,
worktree, branch, dependencies, and exclusions. A blocked dependency is not
permission to skip ahead or weaken a control.

## Preserve package and semantic boundaries

- `finstack-ai-kernel` is synchronous and I/O-free. It owns semantic IDs, state, normalized inputs, records, events, effects, and the pure decision/apply model.
- `finstack-ai-runtime` owns the six port contracts and effect execution. The SDK owns registration, resolution, composition, and ergonomic public APIs.
- Protocol, persistence, providers, tools, observers, bindings, and plugin hosts stay outside the kernel. Trusted native leaf batteries live under `extensions/`; isolated WIT/Wasmtime packages live under `plugins/`. Leaf integrations depend on contracts, never the reverse.
- `finstack-ai-runtime` has `default = []`; the facade defaults to `native-tokio`; WASM uses the facade with `default-features = false` plus `wasm-host`; contract-only leaves disable drivers. Protocol may depend on kernel DTOs only for codec or journal needs; runtime and SDK do not depend on protocol for in-process values. Persistent-store implementations and remote, server, client, or process adapters may combine the applicable runtime or SDK contracts with protocol.
- Do not add a miscellaneous shared-types or utilities crate to conceal a dependency cycle. A shared utility abstraction requires an accepted current need and at least three independent crate consumers.
- Review every new direct kernel dependency for determinism, target portability, transitive size, maintenance, license, and minimal-graph impact. Optional integrations must not enlarge the minimal dependency graph.
- A seventh primary port, an eighth middleware stage, native dynamic loading, kernel I/O, per-token boundary callback, commit-order change, forbidden kernel dependency, compatibility-policy change, or stronger effect guarantee requires the mandated ADR before merge.
- Rust owns continuation, recovery, interaction, lineage, ordering, and error semantics. Python and JavaScript adapters perform coarse conversion and host calls only.
- Resolve registered components once and retain direct typed handles on execution paths; ordinary turns do not perform registry lookup.

## Keep naming and public surfaces aligned

- Derive public names from the shared semantic vocabulary in the planning/design contracts (types, ports, records, events, error codes, and the published class/method lists). Do not invent parallel public APIs per language.
- Use idiomatic case per language while preserving the same semantic identity: Rust and Python `snake_case` for functions/methods/modules, Rust `PascalCase` types, Python `PascalCase` classes, TypeScript `PascalCase` types/classes and `camelCase` methods/properties.
- Keep stable error `code` strings, durable record/event kind names, and protocol/WIT identifiers identical across bindings. Local display messages may be language-idiomatic; codes and kind names must not diverge.
- When a public name, signature shape, error code, event/record shape, or user-facing example changes in any published binding, update the other published bindings in the same change, or record an explicit tracked deferral with owner and follow-up. Update shared conformance fixtures whenever semantic meaning changes.

## Own generated artifacts

- Keep generated output (PyO3 packaging artifacts, `.pyi` generation if used, wasm-bindgen/JS glue, WIT bindgen, schema codegen, lockstep package metadata) separate from hand-authored logic.
- Every generated tree has a documented regeneration command and an owner surface in the producing package. Regenerate via that command only; do not hand-edit generated files.
- CI and the active workstream's validation must fail on a dirty generated tree after regeneration. Treat uncommitted generator drift as a broken change, not as an acceptable local shortcut.

## Preserve lifecycle invariants

- `decide` does not mutate state; only committed records are applied.
- Commit recoverable effect intent before execution, including in-memory execution. Preserve original effect/batch identity, idempotent duplicate handling, fail-closed conflicts, explicit uncertainty, and run lineage.
- Immediately before privileged external dispatch, recheck committed effect state, cancellation, and deadline; fail closed if execution is no longer authorized.
- Parallel tool calls may complete out of order, but durable history and final semantic events preserve model source order.
- Observers never change behavior or terminal state. `before_finalize` is the final behavior-changing stage.
- Model-context compaction has at most one active late-tier `before_model` middleware owner per resolved agent, preserves protected content and canonical history, and records required versioned outcomes or checkpoints.
- Browser WASM remains host-driven and target-correct; native/Python/JavaScript extensions are trusted unless an explicit isolation boundary says otherwise.

## Enforce security boundaries

- Treat content as data, never authority. Privileged actions bind an authenticated principal, tenant or scope, exact target, and action, and fail closed when any binding is absent or mismatched.
- Keep secrets as references; do not place secret material in model context, durable records, telemetry, errors, or generated fixtures.
- Isolated extensions receive no ambient authority. Grant only explicit, bounded capabilities with applicable timeout, size, destination, and tenant constraints.

## Keep large modules from growing

New kernel, runtime, or SDK behavior goes in a sibling module when the natural home already exceeds about 1500 lines. Do not split existing large files solely to relocate unchanged behavior.

## Hard stops

Stop the affected implementation when it conflicts with a current public
contract, requires an unresolved architecture or compatibility decision, lacks
required authority, or cannot satisfy a security control. A blocked workstream
does not authorize skipping its dependency or weakening the requirement.

Changes to journal meaning, event order, WIT worlds, remote protocols, primary
port count, middleware stages, kernel I/O, or effect guarantees require an
explicit architecture decision before implementation. A governed compatibility
change also requires migration guidance. Security review does not itself stop
coding when the design is resolved, but it blocks handoff until the applicable
controls, fixtures, denial cases, redaction tests, and review are complete.

Resume stopped work only after the decision is accepted, affected durable
contracts are reconciled, the active workstream is reread, and affected
validation is rerun.
