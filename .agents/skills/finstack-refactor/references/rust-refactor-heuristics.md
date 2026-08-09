# Rust refactor heuristics

Use this reference when choosing the smallest Rust change that improves structure without creating abstraction debt.

## Prefer these operations first

- Extract a private helper function when a block has one clear responsibility.
- Extract a small private struct when several fields move together or a local invariant becomes hard to describe with separate variables.
- Introduce a params struct when a function is at or beyond the repo's `too_many_arguments` threshold of 7 and the arguments form a coherent concept.
- Split a module by responsibility when one file mixes unrelated concerns or forces callers through a large surface.
- Replace raw strings, numbers, or booleans with an enum or newtype when the type itself carries a reusable invariant.

## Avoid these unless clearly justified

- New traits introduced only to avoid a direct call.
- Strategy-pattern rewrites for two or three straightforward branches.
- Macro-heavy cleanup that hides data flow.
- Builder types for cheap one-shot internal construction.
- Params structs that are just unnamed argument bags.

## Extraction heuristics

Extract a function when:

- the name of the block is more informative than the block itself
- the block has a single reason to change
- extracting it reduces branching or indentation in the caller
- the caller becomes easier to scan in top-down order

Do not extract when:

- the helper name adds no semantic value
- the extraction forces awkward borrowing or excessive cloning
- the caller becomes a long list of thin wrappers with no visible flow

## Struct vs helper

Choose a struct when:

- there is state with lifecycle or invariants
- multiple helpers need the same context
- the grouping clarifies ownership or enforces a valid construction path

Choose plain helpers when:

- the logic is stateless
- the data is already available at the call site
- creating a new type would add ceremony without protecting anything

## Borrow-checker-aware refactoring

When extracting in Rust, preserve ownership simplicity.

Prefer:

- passing references instead of cloning by default
- extracting computations that consume already-owned values near the consuming site
- moving repeated matching logic into methods on the owned type
- using small local structs to reduce parallel variable movement

Be careful when an extraction would:

- extend a mutable borrow across more code
- require cloning large data just to satisfy lifetimes
- hide ownership transfer that was obvious in the caller
- turn a simple local sequence into a web of references

## Clippy and docs constraints

Keep these repo constraints in mind while refactoring:

- clippy warnings are errors
- `too_many_arguments` threshold is 7
- public APIs need docs
- avoid `#[allow(...)]` unless there is no reasonable structural fix

In binding code specifically:

- do not introduce `unwrap`, `expect`, or panic-based flows
- preserve centralized error mapping

## Decision shortcuts

- If the pain is readability, extract.
- If the pain is invalid state, add a type.
- If the pain is duplicated orchestration, centralize one canonical path.
- If the pain is public API noise, keep the facade and simplify behind it.
- If the pain is only local and one-off, stop before introducing a reusable abstraction.
