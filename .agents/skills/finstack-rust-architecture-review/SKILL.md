---
name: finstack-rust-architecture-review
description: Use when reviewing existing Rust crates, Rust libraries, Rust applications, or multi-crate workspaces for architecture quality, ownership boundaries, async design, error strategy, dependency direction, public API shape, or maintainability risks.
---

# Rust Architecture Review

Review existing Rust code as an architect, not as a style linter. The goal is to decide whether the current structure is correct, idiomatic, maintainable, and safe to extend without hiding production risks behind vague approval.

## Core Standard

Good Rust architecture has:

- Clear crate and module responsibilities.
- Dependency direction that matches domain boundaries.
- Ownership patterns that make aliasing, mutation, and cloning intentional.
- Error types and conversions that preserve useful context at boundaries.
- Async and concurrency patterns that respect cancellation, backpressure, and runtime constraints.
- Public APIs that are small, stable, documented, and hard to misuse.
- Tests, examples, and benchmarks that exercise the real architectural contracts.

Do not treat a clean `cargo clippy` run as an architecture review. Clippy catches local issues; this skill audits system shape.

## Workflow

Track the review with this checklist:

```markdown
Rust Architecture Review:
- [ ] Identify review scope, stability expectations, and target audience
- [ ] Map crates, modules, feature flags, binaries, examples, and tests
- [ ] Trace dependency direction and public API boundaries
- [ ] Inspect ownership, borrowing, cloning, lifetimes, and data layout choices
- [ ] Inspect error handling, panic policy, and boundary conversions
- [ ] Inspect async/concurrency, shared state, cancellation, and backpressure
- [ ] Compare documentation claims against source, tests, examples, and callers
- [ ] Classify findings by severity and recommend ordered remediation
- [ ] List verification commands that should prove the changes
```

### 1. Establish Scope

First determine what is being reviewed:

- Single crate, binary + library, or workspace.
- Library API, application internals, bindings, service runtime, or all of the above.
- Stable shipped interface vs. branch-local work that can be changed freely.
- Performance-sensitive or financial/numerical code that needs stricter invariants.

If the user gives a broad target like "review this Rust workspace", start by mapping structure before judging quality. If the scope is too large for one useful pass, propose a focused slice.

### Time-Boxed Minimum

When the user asks for a fast review, still complete this minimum pass before giving a verdict:

- List crates/modules reviewed and their apparent responsibilities.
- Identify dependency direction and any obvious boundary inversions.
- Identify public API surfaces, exported errors, serde contracts, feature flags, and versioning/semver-sensitive edges.
- Check at least the nearest tests, examples, benches, or integration callers for the reviewed surface.
- State what was not checked because of the time box.

### 2. Map The Architecture

Inventory the concrete architecture before forming conclusions:

- Workspace members, crate roles, binaries, examples, benches, tests, and feature flags.
- Public re-exports, `pub` modules, exported traits, builders, constructors, error types, and serde contracts.
- Internal dependency direction: domain/core code should not depend on infrastructure unless the project intentionally chose that shape.
- Cross-crate flows: how data enters, changes ownership, crosses async boundaries, and exits.
- Compatibility edges: semver promises, Python/WASM/FFI bindings, persisted data, serialized names, documented examples, external callers.

Prefer source and tests over README claims. Documentation is evidence only after source confirms it.

### 3. Review Rust-Specific Design

Use these review lenses:

| Area | Questions |
| --- | --- |
| Crate boundaries | Does each crate have one reason to exist? Are boundaries stable or just file-system ceremony? |
| Module shape | Are files cohesive? Are important concepts hidden behind pass-through modules or giant catch-all files? |
| Ownership | Are APIs borrowing when they only read? Are clones justified? Are lifetimes exposed because callers need them, or because internals leaked out? |
| Types | Do domain invariants live in types, enums, newtypes, and constructors instead of strings and comments? |
| Errors | Do libraries use typed errors? Do applications add context? Are panics limited to impossible states or tests? |
| Async | Is blocking work isolated? Are spawned tasks awaited or supervised? Are cancellation and backpressure explicit? |
| Concurrency | Is shared state minimized? Are `Arc`, `Mutex`, `RwLock`, atomics, and channels used for the right reasons? |
| Public API | Is there one obvious way to perform core actions? Are builders, traits, and generics earning their complexity? |
| Tests | Do unit, integration, doc, property, and benchmark tests cover architecture-level contracts? |
| Performance | Are hot paths allocation-aware and data structures chosen for access patterns, not habit? |

### 4. Severity Rubric

Report findings in severity order:

- **Blocker**: likely correctness bug, unsoundness, data loss, security exposure, runtime deadlock, public API break, or architecture that prevents required behavior.
- **Major**: maintainability or extensibility risk that will make near-term work slower or unsafe; unclear boundaries, duplicated pathways, leaky abstractions, panic-prone library API.
- **Minor**: localized design cleanup, naming/API clarity, test coverage gap, documentation mismatch.
- **Nit**: formatting or style issue that does not affect architecture. Keep these sparse.

Every finding needs a location, evidence, impact, concrete fix, and checked source of evidence. If source, tests, examples, or callers were not checked, say that explicitly. Avoid generic advice like "consider refactoring"; state what should change and why.

## Output Format

Use this format:

```markdown
## Verdict
PASS / PASS WITH CHANGES / NEEDS REWORK, with one sentence explaining why.

## Architecture Map
- Crates/modules reviewed:
- Public boundaries:
- Key dependency flows:
- Tests/examples checked:

## Findings
### Blocker
- `path::symbol`: issue, evidence, impact, recommended fix.

### Major
- `path::symbol`: issue, evidence, impact, recommended fix.

### Minor
- `path::symbol`: issue, evidence, impact, recommended fix.

## What Works
- Specific architectural choices that are sound and should be preserved.

## Remediation Order
1. Highest-leverage fix first.
2. Follow-up cleanup.

## Verification
- Targeted commands or tests that should pass after remediation.
- Any checks not run and why.
```

## Red Flags

Stop and gather more evidence when you catch yourself thinking:

- "The docs say this, so the source probably matches."
- "Clippy passed, so the architecture is good."
- "This is just a quick review; no need to map dependencies."
- "The trait/generic/builder might be useful later."
- "The async task is fire-and-forget, so supervision does not matter."
- "The clone is probably cheap."
- "No one will use the public API that way."

These are review shortcuts. Replace them with source, caller, and test evidence.

## Common Mistakes

- Reviewing Rust like Java or TypeScript and missing ownership/API consequences.
- Spending the review on formatting while ignoring crate boundaries.
- Flagging every `clone` instead of asking whether ownership transfer is justified.
- Rejecting all abstractions instead of checking whether they protect real boundaries.
- Ignoring examples and tests even though they reveal intended public usage.
- Treating branch-local compatibility as sacred. Preserve shipped interfaces, persisted data, and documented public APIs; simplify unshipped work directly.

## Resources

- `references/finstack-workspace-map.md` - crate roles, dependency direction, binding surfaces, and common architecture risks.
- `outputs/architecture-review.md` - example architecture review output.
