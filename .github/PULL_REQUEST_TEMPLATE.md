## Summary

<!-- What does this change and why? -->

## Task / issue reference

<!-- Link the current issue, task, or decision record when one exists. -->

-

## API impact

<!-- Public Rust/Python/JS/WIT surface changes, feature flags, or none -->

-

## Schema / compatibility impact

<!-- Contract family/version, unknown-field behavior, fixtures, migration/rollback -->

-

## Performance / allocation impact

<!-- Hot-path, allocation, queue/bounds, benchmark updates, or none -->

-

## Security / trust-boundary impact

<!-- Threat Model §18 trigger? SEC-INV/TM IDs? secret handling? disposition -->

-

## Checklist

- [ ] Architecture review checklist completed: [.github/ARCHITECTURE_REVIEW_CHECKLIST.md](./ARCHITECTURE_REVIEW_CHECKLIST.md)
- [ ] Dependency direction preserved: `kernel <- runtime <- SDK/bindings`; protocol stays outward (kernel DTOs only as needed)
- [ ] No forbidden kernel I/O or host-language binding dependencies introduced
- [ ] Scope is one coherent behavior change; deferred work is not pulled in early
- [ ] Applicable architecture, compatibility, and security review triggers considered
- [ ] Schema/contract changes include migration or explicit clean-break handling
- [ ] Exceptions or waivers are explicit, owned, and time-bounded
- [ ] Exact validation commands and results are recorded below
- [ ] Commits are DCO signed-off (`git commit -s`)
- [ ] Affected `mise run` / `cargo` checks documented below
- [ ] Hosted CI workflow considered (see [`.github/ci/README.md`](./ci/README.md))

## Verification

```text
# Prefer: mise run test-fast while iterating; mise run ci-all before handoff
# Commands run and results
```

## Risk / follow-ups

-
