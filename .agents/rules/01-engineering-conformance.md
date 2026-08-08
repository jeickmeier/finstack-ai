---
trigger: always_on
description: Enforce finstack-ai architecture, boundaries, naming, generated-artifact, lifecycle, security, and hard-stop constraints while coding.
globs:
---

# Engineering conformance

Apply this rule to code, schemas, build configuration, generated artifacts, and implementation review. Start with the active logical PR's traced requirements, ADRs, design sections, standards, and security controls, then follow directly affected contracts and cross-references. Do not audit unrelated documentation. Planning defines behavior; code cannot reinterpret it.

Language-specific coding and docstring rules live in `03-rust-coding.md`, `04-python-coding.md`, and `05-typescript-coding.md`. Verification and handoff rules live in `02-testing-and-delivery.md`.

## Establish the coding slice

- Select one primary logical `PR-NNN` from `docs/planning/04-finstack-ai-implementation-plan.md` and read its complete entry.
- Read the referenced requirements, ADRs, design sections, security controls, acceptance evidence, and exclusions before editing.
- Respect prior gates, phase entrances, dependencies, and explicitly permitted parallel work. The active phase produces evidence for its own exit gate; that gate is not a prerequisite for work within the phase.
- Do not combine unrelated logical PRs or implement deferred scope.
- Prefer the smallest implementation that completes the selected behavior. New abstractions require a current requirement and a concrete use; placeholder frameworks and unused extension seams are prohibited.

## Preserve package and semantic boundaries

- `finstack-ai-kernel` is synchronous and I/O-free. It owns semantic IDs, state, normalized inputs, records, events, effects, and the pure decision/apply model.
- `finstack-ai-runtime` owns the six port contracts and effect execution. The SDK owns registration, resolution, composition, and ergonomic public APIs.
- Protocol, persistence, providers, tools, observers, bindings, and plugin hosts stay outside the kernel. Leaf integrations depend on contracts, never the reverse.
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
- CI or the logical PR's validation must fail on a dirty generated tree after regeneration. Treat uncommitted generator drift as a broken change, not as an acceptable local shortcut.

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

## Hard stops

Stop the affected implementation when it lacks an eligible logical PR, crosses an unopened gate, conflicts with an authoritative document, or depends on an unresolved required decision. Before coding, check Implementation Plan section 6.3 ADR triggers and Security and Threat Model section 18 review triggers. Reviewer unavailability blocks review or merge, not coding, unless pre-implementation approval is explicit or that reviewer must resolve an open decision.

`docs/planning/` is read-only during normal coding. Do not edit planning documents to match implementation preference, paper over conflicts, or “fix” requirements in place. If implementation exposes a genuine conflict or ADR trigger, stop the affected work and use change control. Implementation registers under `docs/implementation/` remain the place for delivery evidence updates.

An ADR trigger stops the design-changing work until the decision and primary-document reconciliation are accepted. A governed compatibility change also requires its migration or compatibility plan. A security review trigger does not itself require an ADR or stop implementation when the design is resolved, but it blocks merge until the applicable threat-model, control, fixture, test, and security-review updates are complete; add an ADR only when a separate ADR trigger applies.
