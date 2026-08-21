---
name: finstack-production-release-prep
description: Prepares finstack for production release by orchestrating deprecated API removal, documentation readiness, semver review, dependency/security audits, performance regression checks, version metadata, release notes, and final quality gates. Use for release readiness, tagging, or publish-prep work; use finstack-simplify for narrow cleanup tasks.
---

# Production Release Preparation

## Quick start

When preparing a release, execute each phase in order. Copy the master checklist and track progress:

```
Release Prep Progress:
- [ ] Phase 1: Dead code audit
- [ ] Phase 2: Deprecated code removal
- [ ] Phase 3: Documentation audit
- [ ] Phase 4: Semver & breaking change analysis
- [ ] Phase 5: Performance regression check
- [ ] Phase 6: Release hygiene
- [ ] Phase 7: Final verification & tagging
```

After each phase, run `mise run ci-all` to confirm nothing is broken before proceeding.

## Phase 1: Dead code audit

Find and remove code that is never called, never compiled, or unreachable.

### 1a. Compiler-level dead code (all components)

**Rust** - Run these sequentially and fix each batch:

```bash
# Unused imports, variables, functions (compiler warnings)
cargo build --workspace 2>&1 | grep -E "warning.*unused|warning.*dead_code|warning.*unreachable"

# Unreachable pub items — items marked pub but never used outside the crate
cargo build --workspace 2>&1 | grep "never used\|never read"
```

**Python** - Check for unused imports and variables:

```bash
uv run --no-project --with ruff==0.12.11 ruff check bindings/finstack-ai-python --select F401,F841 --no-fix
```

**TypeScript** - Unused exports and variables:

```bash
cd bindings/finstack-ai-wasm/js && npx tsc --noEmit 2>&1 | grep -i "declared but"
```

### 1b. Deep dead code detection

Search for patterns that compilers miss:

1. **Pub items with zero external callers**: Search for `pub fn`, `pub struct`, `pub enum` in library crates and grep for their usage across the workspace. If no call site exists outside tests, either make private or remove.

2. **Feature-gated dead code**: Check `#[cfg(feature = "...")]` blocks — verify the features are actually used in `Cargo.toml` or by downstream crates.

3. **Test-only helpers leaked into lib**: Items in `src/` used only by `tests/` should be behind `#[cfg(test)]` or `#[cfg(feature = "test-utils")]`.

4. **Commented-out code**: Search for large blocks of commented code (`// ...` spanning 5+ lines). Remove — git has history.

5. **TODO/FIXME/HACK markers**: Audit all `TODO`, `FIXME`, `HACK`, `XXX` comments. Resolve or file issues for each.

```bash
rg -n "TODO|FIXME|HACK|XXX" --type rust --type python --type ts -g '!target/' -g '!node_modules/'
```

### 1c. Unused dependencies

```bash
# Rust — find deps declared but not imported
cargo install cargo-machete 2>/dev/null; cargo machete

# Python — check for unused imports
uv run --no-project --with ruff==0.12.11 ruff check bindings/finstack-ai-python --select F401
```

### Severity guide

- **Remove immediately**: Unused imports, dead functions, commented-out code, unused deps.
- **Gate with cfg(test)**: Test-only helpers in lib code.
- **Evaluate**: Pub items with no callers — may be part of the public API contract.

## Phase 2: Deprecated code removal

### 2a. Find all deprecation markers

```bash
# Rust deprecated attributes
rg '#\[deprecated' --type rust -g '!target/' -l

# Rust deprecated doc comments
rg '@deprecated|DEPRECATED' --type rust -g '!target/'

# Python deprecation warnings
rg 'DeprecationWarning|deprecated|@deprecated' --type python -g '!__pycache__/'

# TypeScript
rg '@deprecated|deprecated' --type ts -g '!node_modules/'
```

### 2b. For each deprecated item

1. **Verify replacement exists**: The deprecation message should point to a canonical replacement. Confirm the replacement is implemented and tested.
2. **Find all internal call sites**: Grep for usage across the workspace. Migrate each call site to the replacement.
3. **Update CHANGELOG.md**: Add the removal to `[Unreleased] > Removed` with migration instructions.
4. **Update Python/WASM bindings**: If the deprecated Rust API was exposed via bindings, remove the binding too.
5. **Remove the deprecated code**: Delete the function/struct/method and its tests.

### 2c. Legacy code patterns to remove

Search for and eliminate:

- `_v2`, `_v3`, `_old`, `_legacy`, `_compat` suffixed items
- `#[allow(deprecated)]` suppressions — resolve the underlying deprecation
- Shim modules or compatibility layers from previous migrations
- Feature flags that gate old behavior (`legacy`, `compat`, `v1`)

```bash
rg '_v2|_v3|_old|_legacy|_compat|allow\(deprecated\)' --type rust -g '!target/'
```

## Phase 3: Documentation audit

### 3a. Public API documentation

Run the documentation maintainer for detailed coverage:

> For detailed API doc standards, see the [finstack-documentation-maintainer skill](../finstack-documentation-maintainer/SKILL.md).

Quick automated check:

```bash
# Rust — build docs and check for missing doc warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps 2>&1 | head -50

# Python — type-check bindings
uv run --no-project --with ty==0.0.72 \
  ty check bindings/finstack-ai-python/python
```

### 3b. README and high-level docs

Verify these files are current and accurate:

| File | Check |
|------|-------|
| `README.md` | Badges, install instructions, quick start example, feature list |
| `CHANGELOG.md` | All changes since last release documented; `[Unreleased]` section populated |
| Crate-level `README.md` | Each crate's README reflects current API |

### 3c. Examples

All examples must compile and produce correct output:

```bash

```

### 3d. Migration guides

If deprecated APIs were removed (Phase 2), ensure:

- `CHANGELOG.md` contains before/after code snippets
- Migration is straightforward (no multi-step workarounds)
- All combinations of old usage patterns are covered

## Phase 4: Semver & breaking change analysis

Determines the correct version bump (patch / minor / major).

### 4a. Run cargo-semver-checks locally

```bash
# Compare current branch against the last release tag
cargo install cargo-semver-checks --locked 2>/dev/null
cargo semver-checks check-release -p finstack-ai-kernel --baseline-rev <last-release-tag>
```

If semver-checks reports breaking changes, the release must be a **major** bump. If new public API was added, at least **minor**. Otherwise **patch**.

### 4b. Manual breaking change review

Semver-checks catches type/signature changes but misses behavioral breaks. Also review:

- Default values that changed (e.g., strict mode on/off)
- Error conditions that changed (functions that now return `Err` where they previously returned `Ok`)
- Deterministic output changes (golden test drift, journal/record shape)
- Removed or renamed feature flags

### 4c. Feature flag matrix

Verify the crate builds under key feature combinations:

```bash
# Default features only
cargo build --workspace --locked

# WASM host graph
mise run check-wasm
```

### 4d. Frozen public items

```bash
uv run --no-project python scripts/compat/public_items.py --check
```

A removed frozen public item is a breaking change unless the inventory is intentionally updated in the same release.

## Phase 5: Performance regression check

### 5a. Run benchmarks against baseline

```bash
# WASM package size and browser benches when the release touches host or bindings
uv run --no-project python scripts/perf/check_size_budgets.py
mise run bench-wasm
```

### 5b. Review for regressions

Flag any benchmark that regressed >10% from the saved baseline. Common causes:
- New allocations in hot paths
- Additional validation in kernel or runtime loops
- Extra serialization or binding conversions on the turn path

### 5c. Deterministic fixtures

Behavioral drift is a release blocker. Verify:

- Conformance fixtures still pass
- Journal/record replay tests still match
- Golden or snapshot tests are unchanged unless the release notes the change

```bash
cargo test -p finstack-ai-test --locked --lib -- conformance::ports::tests
mise run test-all
```

## Phase 6: Release hygiene

### 6a. Version and metadata

- [ ] `workspace.package.version` in root `Cargo.toml` bumped per Phase 4 analysis
- [ ] `CHANGELOG.md` `[Unreleased]` header renamed to `[X.Y.Z] - YYYY-MM-DD`
- [ ] Python package version matches (`pyproject.toml` / `maturin` config)
- [ ] WASM package version matches (`package.json`)
- [ ] License headers present where required

### 6b. Dependency audits (comprehensive)

```bash
# cargo-deny: licenses, advisories, bans, and sources (uses deny.toml)
cargo-deny check

# Lint all components (no auto-fix — must pass clean)
mise run check-all
```

### 6c. Publish dry-run

Verify crate packaging is correct (metadata, included files, no missing deps):

```bash
cargo publish -p finstack-ai-kernel --dry-run
```

### 6d. Lock file hygiene

- [ ] `uv.lock` is committed and up to date when the repo uses one
- [ ] `package-lock.json` / `pnpm-lock.yaml` is committed and up to date when present
- [ ] `Cargo.lock` is committed and current so CI's explicit OSV scan can read it

### 6e. Binary and package size check

```bash
uv run --no-project python scripts/perf/check_size_budgets.py
```

Review wheel, WASM, and CLI size budgets for unexpected regressions from the previous release.

### 6f. API parity

```bash
uv run --no-project python scripts/compat/public_items.py --check
mise run build-wasm -- release
mise run check-wasm
```

Verify Python and WASM bindings expose all intended public APIs.

### 6g. Release notes

Create `RELEASE_NOTES_X.Y.Z.md` following the established template if one exists. Include:

- Executive summary and "who should upgrade"
- Breaking changes with migration code snippets
- New features and improvements
- Bug fixes
- Test/doc additions

## Phase 7: Final verification & tagging

### 7a. Full test suite

```bash
mise run test-all
```

### 7b. CI and quality gates

```bash
mise run ci-all
```

### 7c. Pre-release checklist

- [ ] `git status` is clean (all changes committed)
- [ ] CI pipeline passes on the branch
- [ ] No `#[allow(dead_code)]` or `#[allow(unused)]` suppressions added during cleanup
- [ ] No new `TODO`/`FIXME` comments introduced
- [ ] CHANGELOG is complete and reviewed
- [ ] Release notes written and reviewed
- [ ] Version numbers consistent across all components

### 7d. Tag and release

```bash
# Create annotated tag
git tag -a vX.Y.Z -m "Release vX.Y.Z"
git push origin vX.Y.Z

# Create GitHub release (attaches release notes)
gh release create vX.Y.Z --title "Finstack vX.Y.Z" --notes-file RELEASE_NOTES_X.Y.Z.md
```

### 7e. Post-release

- [ ] Add new `[Unreleased]` section to `CHANGELOG.md`
- [ ] Bump version to next dev pre-release if desired

## Output format

After completing the audit, produce a release readiness report:

```markdown
## Release Readiness Report — vX.Y.Z

### Version bump rationale
- Bump type: patch / minor / major
- Reason: <semver-checks output summary>

### Dead code removed
- <count> unused functions removed
- <count> unused imports cleaned
- <count> unused dependencies removed
- <count> TODO/FIXME resolved

### Deprecated APIs removed
- <list of removed APIs with replacement pointers>

### Documentation status
- Rust pub API coverage: <percentage>
- Python stub coverage: <percentage>
- Examples: all passing / <N> failures
- CHANGELOG: complete / needs attention
- Migration guide: included / not needed
- Release notes: drafted / not needed

### Performance
- Benchmarks: no regressions / <list regressions>
- Conformance/golden tests: all passing / <N> failures
- Package size delta: +/- <KB>

### Quality gates
| Check | Status |
|-------|--------|
| `mise run ci-all` | pass/fail |
| `mise run check-all` | pass/fail |
| `cargo-deny check` | pass/fail |
| `mise run test-all` | pass/fail |
| `mise run check-wasm` | pass/fail |
| Publish dry-run | pass/fail |
| Semver checks | pass/fail |
| `uv run --no-project python scripts/compat/public_items.py --check` | pass/fail |
| Feature flag matrix | pass/fail |

### Remaining items
- [ ] <any unresolved items>
```

## Additional resources

- Dead code and simplification: see [finstack-simplify](../finstack-simplify/SKILL.md)
- API documentation standards: see [finstack-documentation-maintainer](../finstack-documentation-maintainer/SKILL.md)
- Naming and pattern consistency: see [finstack-consistency-reviewer](../finstack-consistency-reviewer/SKILL.md)
- Release checklist: see [release-checklist.md](references/release-checklist.md)
