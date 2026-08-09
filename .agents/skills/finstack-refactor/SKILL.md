---
name: finstack-refactor
description: Improves existing code structure without changing externally observable behavior. Use for behavior-preserving implementation refactors after scope is clear: extracting helpers/modules, renaming symbols, tightening types, splitting large files, or removing understood dead paths. Prefer finstack-simplify for finstack-wide dedupe/API-surface audits, finstack-binding-parity-reviewer for cross-language API drift, and finstack-documentation-maintainer for doc-only work.
---

# Refactor

## Goal

Preserve behavior while making the next change cheaper, safer, and easier to reason about. Prefer the smallest structural change that materially improves local clarity, ownership, or extension cost.

## Load references selectively

Do not load every reference by default. Open only the file that matches the current refactor.

- `references/repo-refactor-boundaries.md`: open when deciding whether code belongs in Rust core crates, bindings, stubs, exports, or parity surfaces.
- `references/rust-refactor-heuristics.md`: open when refactoring Rust internals or choosing between helpers, structs, enums, params structs, modules, or traits.
- `references/binding-layer-patterns.md`: open when touching `finstack-quant-py` binding code, Python-facing API shape, registration, or error mapping.
- `references/refactor-sync-checklist.md`: open before finishing a refactor that changes public APIs, exports, module layout, names, or cross-language surfaces.
- `references/repo-examples.md`: open when you want a repo-shaped example before choosing the operation.

## Operating rules

- Separate refactoring from feature work. If the user asks for both, identify the minimal structural change that unlocks the feature and avoid opportunistic cleanup outside that boundary.
- Preserve public behavior unless the user explicitly asks for behavioral changes. Treat outputs, error semantics, side effects, ordering, and performance expectations as invariants.
- Keep public APIs stable unless there is a clear user-approved reason to change them.
- Stay within the existing architecture and style unless the current pattern is the problem.
- Prefer deletion over abstraction. Add helpers, types, traits, or layers only when they remove real duplication or clarify ownership.
- Define the exact change boundary before editing: files, functions, callers, invariants, and follow-on surfaces such as docs, bindings, or stubs.
- Reduce scope when behavior is unclear and there is no reliable way to validate the change.

## Refactoring workflow

1. Audit the target.
   - Identify the real pain: duplication, oversized functions, naming drift, mixed responsibilities, hidden invariants, brittle control flow, type misuse, or dead paths.
   - State what must not change.
   - Choose the smallest viable scope.
2. Choose the operation.
   - Extract function, type, or module.
   - Inline a useless wrapper or indirection layer.
   - Rename toward domain language.
   - Introduce a small parameter or result struct when the data is cohesive.
   - Move behavior to the type or module that owns the data.
   - Replace primitive state with an enum, newtype, or domain type when it enforces a real invariant.
   - Flatten control flow with guard clauses or a clearer dispatch mechanism.
   - Collapse duplicate pathways into one canonical path.
   - Remove dead code once ownership and callers are understood.
3. Implement mechanically.
   - Make the change easy to review.
   - Change one conceptual thing at a time.
   - Preserve call order, side effects, allocation and borrowing behavior, and error mapping.
   - Avoid style churn unrelated to the refactor.
   - Update comments, docs, exports, and stubs only where the structure actually changed.
4. Finish cleanly.
   - Summarize the structural improvement.
   - Name the invariants you preserved.
   - Call out residual risk such as unvalidated paths, wide call surfaces, or performance-sensitive code.

## Smell-to-operation guide

- Long function: extract coherent blocks by responsibility; name helpers by intent, not implementation detail.
- Duplication: unify the business rule first, then share the mechanism; do not force near-duplicates together if they are diverging for legitimate reasons.
- Large module: split by responsibility and dependency direction; avoid creating a generic `utils` dump.
- Long parameter list: introduce a small struct only for cohesive data; do not hide unrelated arguments in a bag type.
- Naming drift: rename toward established domain language and match adjacent modules.
- Primitive obsession: introduce a domain type only when it clarifies meaning or enforces an invariant used in multiple places.
- Nested branching: prefer guard clauses, focused validation helpers, or a simple dispatch table; do not reach for polymorphism by default.
- Indirection overload: inline wrappers that add no policy, safety, or reuse value.
- Dead code: remove it once live callers are understood; use `finstack-quality-gate-triage` for failure-driven cleanup or `finstack-production-release-prep` for release-wide dead-code sweeps.
- API surface bloat: converge on one obvious path and remove wrappers when the user scope allows it.

## Pattern selection rules

- Prefer extraction before introducing new abstractions.
- Prefer a plain function or small private helper over a new class, trait, or strategy object.
- Prefer a struct or enum when it makes invalid states harder to represent.
- Prefer explicit data flow over hidden mutation.
- Prefer moving logic to the layer that owns the invariant, not the layer that happens to call it.
- Prefer local simplification over architectural rewrites.

## Repo-specific constraints

- Keep business and valuation logic in the Rust core crates. Keep Python and WASM bindings thin: conversion, wrapper construction, registration, and error mapping only.
- Maintain parity across Rust, Python, and WASM surfaces when refactoring shared API shape.
- Keep manually maintained `.pyi` stubs in sync when binding signatures, names, or exports change.
- Respect Python binding module conventions: `register()`, `__all__`, `__doc__`, wrapper types with `inner`, and centralized error conversion.
- Follow existing API conventions such as `get_*` accessors, builder chaining, and established metric-key formats.
- In Rust, prefer small private helpers or focused structs over macro-heavy or pattern-heavy abstractions.
- Do not introduce `unwrap`, `expect`, or panic-based flows into non-test binding code.

## Do not do these things

- Do not rewrite from scratch when a surgical change is enough.
- Do not mix refactoring with unrelated feature work.
- Do not add framework-like abstractions to future-proof simple code.
- Do not replace straightforward conditionals with polymorphism unless the branching complexity truly justifies it.
- Do not split code into many tiny helpers that hide the main flow.
- Do not move logic across architectural boundaries unless the boundary itself is the problem.
- Do not claim a behavioral refactor when semantics changed; say so explicitly.

## Coordinate with adjacent skills when needed

- Use `finstack-simplify` when the primary goal is converging multiple finstack pathways into one obvious path.
- Use `finstack-consistency-reviewer` when the main problem is cross-module naming or pattern drift.
- Use `finstack-quality-gate-triage` when the task is driven by a failing lint/test/pre-commit/CI output.
- Use `finstack-documentation-maintainer` when the refactor changes public APIs, docs, examples, or stub surfaces significantly.

## Output expectations

When using this skill, report:

- the exact refactor target
- the invariant or behavior being preserved
- the structural operations chosen
- the files or surfaces that must stay in sync
- any recommended validation the user may want to run
