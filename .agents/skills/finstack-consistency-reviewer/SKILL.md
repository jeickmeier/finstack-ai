---
name: finstack-consistency-reviewer
description: Reviews finstack code for cross-module consistency: naming conventions, pattern drift, Rust/Python/WASM naming triplets, builder/error/module conventions, and convention inventory updates. Use when the user asks to make patterns consistent, unify naming, check conventions, or find pattern drift. Prefer finstack-simplify for dedupe/API-surface consolidation and finstack-refactor for implementation changes.
---

# Consistency Reviewer

Reviews code for consistency across the codebase. Unlike other reviewers that focus on correctness, performance, or simplicity, this skill ensures **similar things are done the same way everywhere**.

## Quick Start

When asked to review for consistency, produce output in this format:

```
## Consistency Review: [scope]

### Summary
[1-2 sentence overview of findings]

### Findings

#### [SEVERITY] [Category]: [Title]
**Where:** file1.rs:L10, file2.rs:L20
**Pattern A:** `description of one approach`
**Pattern B:** `description of divergent approach`
**Recommendation:** [which pattern to standardize on and why]
```

### Severity Levels

| Level | Meaning |
|-------|---------|
| **Blocker** | Public API inconsistency that confuses users or breaks expectations |
| **Major** | Internal pattern divergence that increases maintenance burden |
| **Minor** | Cosmetic inconsistency or missed std-lib opportunity |
| **Nit** | Style preference, acceptable if intentional |

## Review Checklist

Work through each category. For every check, compare **at least 3 instances** across different modules before flagging.

### 1. Naming Conventions

**Rust:**
- [ ] Functions: `snake_case` consistently (no camelCase leaks)
- [ ] Types/Structs/Enums: `PascalCase` consistently
- [ ] Constants: `SCREAMING_SNAKE_CASE` for all `pub const`, including private constants within the same scope
- [ ] Builder methods: either all use `set_` prefix or none do
- [ ] Error constructors: consistent pattern (`Type::new()` vs `Type::variant_name()` vs helper methods)
- [ ] Trait names: consistent verb/noun form
- [ ] Module names: consistent singular vs plural

**Python bindings:**
- [ ] Wrapper types: consistent prefix/naming relative to the Rust type
- [ ] Method names: `snake_case` matching Rust names where possible
- [ ] `__repr__` / `__str__`: consistent format across all `#[pyclass]` types

**WASM bindings:**
- [ ] Wrapper types: consistent prefix/naming relative to the Rust type
- [ ] `js_name`: `camelCase` for functions, `PascalCase` for types
- [ ] Constructor naming: consistent `new()` vs `create()` vs `from_*()` patterns

**Cross-binding:**
- [ ] Same concept uses same name in Rust, Python, and WASM (modulo casing rules)
- [ ] Feature parity: if Rust has it, bindings should expose it (or document why not)
- [ ] Error `code` strings and record/event kind names are identical across bindings

### 2. Pattern Consistency

**Error handling:**
- [ ] Error location pattern: consistent `error.rs` vs `error/mod.rs` across crates
- [ ] Error variant naming: consistent `{Source}Error` vs `{Description}` style
- [ ] `#[from]` and `#[error(transparent)]` used consistently for wrapped errors
- [ ] Stable machine-readable codes stay identical across languages

**Builders:**
- [ ] Entry point: `Type::builder(...)` consistently (vs `TypeBuilder::new()` vs `TypeBuilder::default()`)
- [ ] Terminal method: `.build()` consistently (vs `.finish()` vs `.create()`)
- [ ] Required vs optional fields: same validation approach across all builders
- [ ] Return types: `Result<T>` vs `T` consistent for similar builder complexity

**Trait implementations:**
- [ ] `Send + Sync` bounds: applied consistently to all public traits of the same kind
- [ ] `Clone + Debug`: applied consistently to all public types
- [ ] `Display` vs `Debug`: consistent approach for user-facing vs developer-facing output
- [ ] `Default`: implemented where appropriate, consistently across similar types
- [ ] `From`/`Into` conversions: bidirectional where expected, consistent direction

**Module structure:**
- [ ] `mod.rs` vs file-per-module: consistent threshold for when to split
- [ ] Re-export patterns: consistent use of `prelude.rs` vs root `pub use`
- [ ] `pub(crate)` vs `pub`: consistent boundary between internal and public APIs
- [ ] Test organization: `#[cfg(test)] mod tests` in same file vs separate `tests/` directory

### 3. Missed Rust Std-Lib & Language Features

- [ ] `Iterator` methods: manual loops where `.map()`, `.filter()`, `.fold()`, `.collect()` suffice
- [ ] `Option` combinators: `if let Some(x) = opt { ... }` where `.map()`, `.and_then()`, `.unwrap_or()` is cleaner
- [ ] `Result` combinators: match blocks where `?`, `.map_err()`, `.context()` is cleaner
- [ ] `From`/`Into`: manual conversion functions where `impl From<A> for B` enables `.into()`
- [ ] `Display`: custom `to_string()` methods where `impl Display` is standard
- [ ] `Default`: manual `new()` with no args where `Default` is idiomatic
- [ ] `AsRef`/`Borrow`: accepting `&String` where `&str` / `impl AsRef<str>` is idiomatic
- [ ] `Cow<str>`: unnecessary `String` allocation where `Cow` avoids cloning
- [ ] `std::mem::take`/`replace`: verbose swap patterns where `take()` is cleaner
- [ ] `#[must_use]`: missing on functions that return values that should not be ignored
- [ ] `impl Into<T>` parameters: accepting concrete types where `impl Into<T>` is more ergonomic
- [ ] Slice patterns: index-based access where slice destructuring is clearer
- [ ] `matches!` macro: verbose match blocks that return `true`/`false`
- [ ] `let-else`: match/if-let with early return where `let ... else { return }` is cleaner

### 4. Duplicate & Near-Duplicate Functionality

- [ ] Utility functions: same logic implemented in multiple modules
- [ ] Type aliases: same type aliased differently in different modules
- [ ] Validation logic: same checks repeated instead of shared validators
- [ ] Configuration parsing: same config patterns re-implemented per module
- [ ] Test helpers: same setup/assertion patterns duplicated across test modules
- [ ] Conversion functions: same `A -> B` conversion in multiple places

### 5. API Surface Consistency

- [ ] Similar types expose similar method sets
- [ ] Similar constructors accept similar parameters and return similar result types
- [ ] Related query types expose consistent interfaces
- [ ] All public types implement the same baseline traits (`Clone`, `Debug`, `Serialize`, `Deserialize`) when they are part of the same family
- [ ] Error messages follow consistent formatting patterns

### 6. Documentation Consistency

- [ ] All `pub` items have doc comments (or none do within a module -- pick one)
- [ ] Doc comment style: `///` vs `//!` used consistently
- [ ] Examples in docs: consistent format (`# Examples` section with fenced Rust code blocks)
- [ ] Module-level docs: consistent presence and format across similar modules

## Review Process

1. **Scope**: Identify the modules/crates under review
2. **Inventory**: List all instances of each pattern category within scope
3. **Compare**: For each category, identify the dominant pattern and any deviations
4. **Classify**: Is the deviation intentional (documented) or accidental (drift)?
5. **Recommend**: For each inconsistency, recommend which pattern to standardize on

## Decision Framework

When two patterns conflict, prefer:

1. **Majority wins**: The pattern used in more places (lower migration cost)
2. **Std-lib idiomatic**: The pattern closer to Rust conventions
3. **Public-facing**: The pattern used in public APIs (harder to change)
4. **Newer code**: The pattern in recently-written code (likely reflects current intent)

## Output Template

```
## Consistency Review: [scope]

### Summary
Found X findings (Y blocker, Z major, W minor, V nit) across N files.

### Findings

#### [BLOCKER] Naming: Builder method prefix inconsistency
**Where:** agent.rs:L120 (`set_timeout()`), session.rs:L45 (`timeout()`)
**Pattern A:** `set_` prefix on setter methods (3 occurrences)
**Pattern B:** bare name for setter methods (12 occurrences)
**Recommendation:** Standardize on bare names (majority pattern, more ergonomic)

#### [MAJOR] Pattern: Error module location
**Where:** kernel/src/error.rs, runtime/src/error.rs, protocol/src/error.rs
**Pattern A:** `error/mod.rs` with submodules (1 crate)
**Pattern B:** `error.rs` flat file (3 crates)
**Recommendation:** Use `error.rs` for crates with <5 error variants, `error/mod.rs` for larger

### Convention Inventory
[Optional: table of all patterns found and their locations for reference]
```

## Additional Resources

- For codebase-specific conventions, see [conventions.md](conventions.md)
