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

- `crates/finstack-ai-kernel/src/reducer/legacy.rs` (186 lines) — orphaned helper, zero callers. Pattern: dead code (slop-patterns §11).
- Single-impl trait in `traits.rs` (22 lines) — inlined into the concrete type. Pattern: single-impl trait (slop-patterns §5).

## What moved

Bullet list of things relocated or renamed. For each: `from` → `to` and why.

- `ApplyHelper::add` → `ApplyHelper::register` — rename to match the only remaining registration convention in the crate.

## What changed shape

Bullet list of signature / behavior changes on items that survived.

- `apply_record` was `fn apply_record(&self, rec: &Record) -> Vec<Event>`; now `fn apply_record(&self, rec: &Record) -> Result<Vec<Event>, Error>`. Previously swallowed errors silently.

## Before → after at the main call-site

Show the most representative call-site's diff. Not the whole codebase — just the one that captures the user's ergonomic win.

**Before:**
```rust
let helper = LegacyApply::new(config);
let events = helper.apply_all();
for e in &events {
    if e.is_err() { /* silently lost — logged only */ }
}
```

**After:**
```rust
let helper = ApplyHelper::new(config);
let events = helper.apply_record(&record)?;
```

## Wrappers that survived (if any)

For each wrapper you decided to keep, one sentence justifying it.

- `ApplyHelper::new` stays because the type has many fields, so a constructor-with-params pattern is warranted.

If the list is empty, write "None."

## Invariants checked

- [ ] Kernel I/O-freedom: not touched by this slice.
- [ ] Commit-before-effect: not touched.
- [ ] Serde field names: not touched.
- [ ] Public items: no public Rust symbol changed that's in the inventory.
- [ ] Generated artifacts: not touched.
- [ ] Observer/compaction ownership: not touched.

Mark [x] for "actually verified" not just "I think it's fine." If the slice touches an invariant, it should show up in the Verify output below.

## Verify output

Paste the last 5–10 lines of each command. **Do not paraphrase.**

```
$ mise run check-all
... last lines ...
```

```
$ mise run test-all
... last lines ...
test result: ok. N passed; 0 failed
```

Repeat for every relevant verify command in the slice's plan entry.

## Risks identified during execution

If anything surprised you during the refactor (e.g., a call-site that was reaching into private state, a test that was passing by coincidence), note it. These are candidate findings for the next audit.

- <or "None" if clean>.

## Ready to commit?

- [ ] All verify commands green.
- [ ] Binding triplet is consistent (if applicable).
- [ ] Public-item inventory updated (if applicable).
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
