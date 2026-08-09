---
name: finstack-simplify
description: Simplifies, dedupes, audits, and refactors finstack Rust crates and Python/WASM bindings when the user mentions slop, wrapper bloat, parallel APIs, registry drift, builder sprawl, API-surface reduction, or over-engineering. Runs a phased Audit -> Plan -> Refactor -> Verify loop that preserves Rust-canonical behavior, binding parity, Decimal/currency/serde invariants, and project `mise run` verification.
---

# finstack-simplify

A phased simplification workflow for the **finstack** workspace (Rust core + Python PyO3 bindings + WASM wasm-bindgen bindings).

**Goal:** turn sloppy, vibe-coded, multi-pathway, over-abstracted code into **one obvious way to do each thing**, without breaking determinism, Decimal equality, FX policy visibility, serde stability, or Rust ↔ Python ↔ WASM parity.

## When to use this skill

Trigger when the user is asking to simplify, audit, refactor, or dedupe _any_ part of finstack_quant. Even weak signals count: "this feels over-engineered", "why are there two ways to build a curve", "the checks module is a mess", "find dead code in margin". If the user is clearly working on something finstack-adjacent (Rust workspace, PyO3 bindings, wasm-bindgen bindings, parity, builders, registries), default to using this skill over a generic simplify skill.

**Do NOT use** for: generic non-finstack-quant code, pure bug hunting with no simplicity angle (use `finstack-quality-gate-triage`), performance tuning without a simplicity angle (use `finstack-performance-reviewer`), or new feature work.

## The core loop

```
Audit  →  Plan  →  Refactor  →  Verify
  ^                                 │
  └─────────────────────────────────┘
         (next slice)
```

Each phase has an explicit checkpoint with the user. **Do not skip phases.** Do not combine Audit and Refactor into one pass — the point of phasing is that the user sees the map before any code moves, and sees the plan before any commits land.

### Phase 1 — Audit (read-only)

Read the target scope. Identify complexity, duplication, parallel pathways, binding drift, and dead code. Produce a written **Audit Report** using the template in `examples/audit-report.md`. **Do not edit code.**

Checklist (all must be considered; not all will apply):

- Surface area inventory (public fns / types / re-exports per crate domain).
- Redundancy clusters (multiple pathways collapsing to a canonical target).
- Wrapper bloat (thin forwarders, degenerate builders, `try_*` layers that only add validation).
- Dead code (unused variants, unreachable branches, commented blocks, unused helpers).
- Binding drift (Rust↔PyO3↔WASM mismatches; logic that leaked out of Rust into bindings).
- Vibe-coding artifacts (half-migrated APIs, speculative abstractions, single-impl traits, single-instantiation generics).
- Financial-invariant risks (does simplification touch Decimal equality / FX policy / serde names / parity contract?).

Rules for what to look for, with concrete examples, are in `references/slop-patterns.md`. Financial safety rules are in `references/financial-invariants.md`. Binding-drift checks are in `references/binding-drift.md`.

Present the report and **stop**. Ask the user to confirm scope and priorities before planning.

### Phase 2 — Plan

Based on the audit + user priorities, produce a **Consolidation Plan** using `examples/consolidation-plan.md`. The plan breaks work into **PR-sized slices**, each of which:

- has a single focused theme (one redundancy cluster, or one dead-code sweep, or one wrapper collapse),
- lists files touched,
- lists the verify commands that must pass,
- lists the risk tier (see `references/workflow.md`),
- calls out any parity-contract / serde-name / binding-shape impact.

Prefer **many small reversible slices** over one heroic PR. Deletes before renames before signature changes.

Present the plan and **stop**. Wait for the user to pick which slice to execute next.

### Phase 3 — Refactor (one slice at a time)

Execute exactly one slice. Apply the refactor tactics in `references/refactor-tactics.md`. Bias to **deletion** over abstraction. If a wrapper survives, it must earn its keep with one sentence of justification in the diff description.

After the edit, produce a short **Refactor Diff** note in the format of `examples/refactor-diff.md`: what moved / what died / what the before→after looks like for public call-sites. Do not proceed to the next slice without Verify passing.

### Phase 4 — Verify

Run the **full finstack verify stack** for the affected layers. These commands are project-specific and non-negotiable:

- Rust touched: `mise run rust-lint && mise run rust-test`
- WASM touched: `mise run wasm-lint && mise run wasm-test` (and `mise run wasm-build` if you changed WASM bindings)
- WASM UI touched: `mise run lint-ui && mise run test-ui`
- Python touched: `mise run python-lint && mise run python-test` (and `mise run python-build` if you changed Rust code that PyO3 binds — debug builds are too slow for portfolio valuation; AGENTS.md mandates release profile)
- Parity impact: re-run `finstack-quant-py/tests/parity` and check `parity_contract.toml` is still green.

Prefer the repo `mise run` tasks from `AGENTS.md` for verification. Use focused checks while iterating and broader `mise run all-*` gates only when the slice is broad enough to justify them.

All output must be **100% green** before moving to the next slice. Paste the actual command output in your response so the user can verify; never claim green without showing it.

If anything fails: stop. Do not "fix it and keep going" without telling the user. Report the failure, your hypothesis, and a minimal proposed fix, then wait for the user's call.

After Verify passes, offer the user three options: (a) continue to the next slice in the plan, (b) re-audit the touched area to confirm no new slop crept in, or (c) stop and commit.

## Default principles (the shape of "simpler")

- **One obvious way.** Every capability has exactly one canonical public entry point. Convenience wrappers must add semantic value, not just rename/reorder.
- **Rust is canonical.** Logic lives in Rust crates. Python/WASM bindings are type conversion + wrapper construction + error mapping. Logic in bindings is a bug.
- **Bias to deletion.** Removing 50 lines of wrapper is worth more than adding a clever trait.
- **Private complexity, public simplicity.** Helpers can proliferate privately; public surface stays minimal and unsurprising.
- **No parallel universes.** `_v2`, `_ex`, `_new`, "advanced" doubles, `try_*` shadow APIs — pick one. Collapse the rest.
- **Consistency beats flexibility.** Fewer knobs with sharp defaults beats a thousand configs nobody sets.
- **Determinism is load-bearing.** Decimal results, parallel≡serial, FX policy stamping, serde field names — do not refactor these away. See `references/financial-invariants.md`.
- **Binding triplets move together.** If you delete a Rust public API, delete its PyO3 wrapper and its WASM wrapper in the same slice. If you can't, the slice isn't done. See `references/binding-drift.md`.

## Reference files (read these when the audit hits their topic)

- `references/slop-patterns.md` — catalogue of every non-simplicity issue this skill hunts for, with finstack-quant-specific examples (registry sprawl, builder duplication, `_builder.rs` vs `builder/mod.rs` ambiguity, prelude bloat, etc.). **Read before every audit.**
- `references/binding-drift.md` — how to detect and fix drift between `finstack-quant/` (Rust) and `finstack-quant-py/src/bindings/` + `finstack-quant-wasm/src/api/`. Parity-contract considerations and documented host-language name collisions. **Read whenever bindings are in scope.**
- `references/financial-invariants.md` — what you're NOT allowed to change while simplifying: Decimal equality, FX policy stamping, serde field names (unknown-field-deny), rounding context metadata, parallel≡serial, ISDA day-counts. **Read before any refactor that touches numerics, FX, or serde.**
- `references/workflow.md` — the phased loop in detail: risk tiers, commit boundaries, the make targets, when to rebuild bindings, how to handle a failing verify step. **Read at the start of every session.**
- `references/refactor-tactics.md` — the concrete moves you apply in Phase 3: inline / collapse / delete / generic-to-concrete / trait-to-fn / single-canonical-constructor / etc. Each tactic has a before/after. **Read before every refactor slice.**

## Example output formats (use these templates verbatim)

- `examples/audit-report.md` — the Phase 1 deliverable format.
- `examples/consolidation-plan.md` — the Phase 2 deliverable format.
- `examples/refactor-diff.md` — the per-slice Phase 3 deliverable format.
- `outputs/completed-audit-report.md` — sample completed audit output.
- `evals/evals.json` — realistic prompts for trigger and workflow checks.

**Do not invent alternate formats.** The user reviews many of these; consistent shape matters more than creative presentation.

## Escalation and edge cases

- **If the scope is unclear** ("simplify finstack" with no module target): ask the user to pick a crate or subsystem. Whole-workspace audits balloon to noise; per-crate audits are actionable.
- **If a refactor would break the parity contract**: stop, flag it explicitly, and ask whether the user wants to (i) update `parity_contract.toml` as part of the slice, or (ii) drop the refactor.
- **If you find something scary (panic, unsafe, `unwrap` in binding code, broken determinism)**: surface it in the audit under a "Hazards" heading, but do NOT silently fix it as part of a simplification slice. Hazards get their own slice or escalate to a bug-hunting session.
- **If the user asks to "just do it" and skip the audit/plan**: push back once. Explain that unreviewed refactors in a multi-binding financial library destroy more value than they create. If they insist after that, comply — but insist on small slices + Verify between each.
- **If you find yourself writing a new abstraction to simplify things**: stop and re-read `references/refactor-tactics.md`. The answer is almost always to delete, not add.

## What this skill is NOT

- Not a code generator. Outputs are audits, plans, and targeted diffs.
- Not a performance optimizer — use `finstack-performance-reviewer` for that.
- Not a bug finder — use `finstack-quality-gate-triage` for that.
- Not a generic simplifier — use `finstack-refactor` for behavior-preserving implementation or `finstack-senior-code-review` for broad fallback review.
- Not a rewrite-the-world tool. If the answer is "rewrite this crate from scratch", this skill has failed; surface that as a finding and stop.
