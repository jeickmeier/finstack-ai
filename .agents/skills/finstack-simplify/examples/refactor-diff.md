# Refactor Diff — Phase 3 output template

One of these per slice. Short. The code is the source of truth; this note explains *why*, for reviewers.

---

# Slice N — <theme>

**Plan reference:** Consolidation Plan dated YYYY-MM-DD, Slice N.
**Tier:** <1/2/3/4>
**Files changed:** list.
**Net LOC:** −<n> added, +<n> removed.

## What died

Bullet list of things deleted. Each bullet: **what** and **why it was redundant**.

- `finstack-quant/statements/src/checks/runner.rs` (186 lines) — orphaned `LegacyCheckRunner`, zero callers. Pattern: dead code (slop-patterns §11).
- `CheckRunner` trait in `traits.rs` (22 lines) — single-impl trait, inlined into `CheckSuite`. Pattern: single-impl trait (slop-patterns §5).
- `pub use checks::runner::*` in `prelude.rs` — re-export of deleted module.

## What moved

Bullet list of things relocated or renamed. For each: `from` → `to` and why.

- `CheckSuite::add_check` → `CheckSuite::register` — rename to match the only remaining registration convention in the crate.

## What changed shape

Bullet list of signature / behavior changes on items that survived.

- `CheckSuite::run` was `fn run(&self, ctx: &Context) -> Vec<CheckResult>`; now `fn run(&self, ctx: &Context) -> Result<Vec<CheckResult>, CheckError>`. Previously swallowed per-check errors silently.

## Before → after at the main call-site

Show the most representative call-site's diff. Not the whole codebase — just the one that captures the user's ergonomic win.

**Before:**
```rust
let runner = LegacyCheckRunner::new(config);
let suite = CheckSuite::from_runner(&runner);
let results = suite.run_all();
for r in &results {
    if r.is_err() { /* silently lost — logged only */ }
}
```

**After:**
```rust
let suite = CheckSuite::new(config);
let results = suite.run(&ctx)?;
```

## Wrappers that survived (if any)

For each wrapper you decided to keep, one sentence justifying it.

- `CheckSuite::new` stays because `CheckSuite` has 8 fields (> AGENTS.md threshold of 7), so a constructor-with-params pattern is warranted.

If the list is empty, write "None."

## Invariants checked

- [ ] Decimal equality: not touched by this slice.
- [ ] FX policy: not touched.
- [ ] Serde field names: not touched.
- [ ] Parity contract: no public Rust symbol changed that's in the contract.
- [ ] Parallel ≡ serial: not touched.
- [ ] Evaluator precedence: not touched.

Mark [x] for "actually verified" not just "I think it's fine." If the slice touches an invariant, it should show up in the Verify output below.

## Verify output

Paste the last 5–10 lines of each command. **Do not paraphrase.**

```
$ mise run rust-lint
... last lines ...
Finished `release` profile [optimized] target(s) in 12.34s
cargo clippy --workspace -- -D warnings: no warnings emitted
```

```
$ mise run rust-test
... last lines ...
test result: ok. 847 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Repeat for every relevant verify command in the slice's plan entry.

## Risks identified during execution

If anything surprised you during the refactor (e.g., a call-site that was reaching into private state, a test that was passing by coincidence), note it. These are candidate findings for the next audit.

- <or "None" if clean>.

## Ready to commit?

- [ ] All verify commands green.
- [ ] Binding triplet is consistent (if applicable).
- [ ] Parity contract updated (if applicable).
- [ ] `.pyi` stubs updated (if applicable).
- [ ] Commit message drafted.

**Suggested commit message:**

```
refactor(<crate>): <slice theme>

<2–4 line body referencing audit findings and verify commands>
```

## Next

Offer the user three options:

- **(a)** Continue to the next slice in the plan.
- **(b)** Re-audit the touched area to confirm no new slop crept in.
- **(c)** Stop and commit.

Do not decide for them.
