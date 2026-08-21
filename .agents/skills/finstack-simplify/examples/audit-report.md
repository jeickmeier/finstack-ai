# Audit Report — Phase 1 output template

Use this format verbatim. The user will read many of these; consistency matters.

---

# Audit Report: `<crate>::<module>`

**Scope:** `<relative path, e.g. crates/finstack-ai-kernel/src/reducer/>`
**Bindings in scope:**
- `bindings/finstack-ai-python/...` (exists / missing)
- `bindings/finstack-ai-wasm/...` (exists / missing)
**Date:** YYYY-MM-DD
**Auditor:** finstack-simplify / Phase 1 (read-only)

## Executive summary

Two or three sentences. What's the shape of the problem? How many findings at each impact tier? What's the single highest-leverage move?

Example:
> The `reducer/` module has three parallel pathways for applying records (`legacy.rs`, `shapes.rs`, and an ad-hoc helper) that were introduced during a stalled migration. Collapsing these into a single `apply_record` entry point would remove ~340 lines, eliminate a name collision that currently forces callers to qualify imports, and let us delete a single-impl trait. Highest-leverage move: delete `legacy.rs` and the unused registry it carries.

## Surface area inventory

Per capability (group related entry points together). For each:

**Capability — `<name>`**

- Canonical entry point (proposed or identified): `<sig + file:line>`
- Alternate pathways found:
  - `<sig + file:line>` — why it exists
  - `<sig + file:line>` — why it exists

Repeat for every capability in scope.

## Findings

One H2 per finding. Sort by (Impact desc, Risk asc).

### F1 — [Category: parallel-api / wrapper-only / try-shadow / dead-code / binding-drift / single-impl-trait / ...]

**Files:**
- `path/to/file.rs:L123-L160`
- `bindings/finstack-ai-python/.../file.rs:L12-L28`

**What:** One or two sentences. The reader should understand the issue without having to open the files.

**Why it's slop:** One or two sentences. Cite the relevant pattern from `slop-patterns.md` if applicable.

**Proposed fix:** One or two sentences. Which tactic from `refactor-tactics.md` applies? What gets deleted, merged, or moved?

**Invariants touched:** [none | kernel | commit-before-effect | serde | public-items | generated | observers]

**Impact:** [H / M / L] — how much does the fix simplify things
**Risk:** [H / M / L] — how likely is the fix to break something
**Tier:** [1 / 2 / 3 / 4] — see `workflow.md`

---

Repeat F1, F2, F3, ... up to however many findings.

## Slop clusters

If multiple findings interact (e.g., a single-impl trait wraps a parallel-API wrapper that has a try_* shadow), group them into a cluster. Collapse together in one slice.

### Cluster A — `<short name>`

**Includes findings:** F3, F5, F7.

**Why it's a cluster:** They all involve the same builder path. Fixing one without the others would leave orphans.

**Recommended consolidation:** <one paragraph>.

## Binding drift

Even if bindings don't show major findings, include this section — absent drift is a signal too.

**Structural drift:**
- <finding or "none">.

**Logic drift (logic that leaked into bindings):**
- <finding or "none">.

**Public-item impact:**
- <list of symbols that would need inventory updates when the findings above are fixed, or "none">.

## Hazards (non-simplicity problems discovered incidentally)

Sometimes an audit turns up bugs: `.unwrap()` in binding code, a panic on a code path, a silently-swallowed error, a potential soundness issue. These are **not** simplification targets — they're separate bugs. Surface them here so the user can schedule a bug-hunting session.

- **H1 —** File:line — short description. Severity: low / med / high.

## Scorecard

Rate the scope on five axes, 1–5 (5 = clean, 1 = deeply sloppy):

- API simplicity: X/5 — <one sentence>
- Redundancy level: X/5 — <one sentence>
- Consistency: X/5 — <one sentence>
- Binding hygiene: X/5 — <one sentence>
- Maintainability: X/5 — <one sentence>

**Overall:** X/5

## Top 5 highest-leverage changes

Not necessarily the five highest-impact findings — the five that most reduce **reader confusion per line changed**. Usually deletions of half-migrated APIs, collapses of parallel pathways, or removals of speculative abstractions.

1. **F<n>** — <short statement>. Removes ~<N> LOC.
2. ...
3. ...
4. ...
5. ...

## Next steps

One sentence. Example:

> Proceed to Phase 2 (Plan) to break these findings into coherent implementation slices, or narrow the scope further if the user wants to focus on a specific cluster.

**Awaiting user input:** confirm scope, pick priorities, or request re-audit with different focus.
