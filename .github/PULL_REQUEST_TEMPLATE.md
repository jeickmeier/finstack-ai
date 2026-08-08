## Summary

<!-- What does this change and why? -->

## Logical PR / plan reference

<!-- e.g. PR-001 from docs/planning/04-finstack-ai-implementation-plan.md -->

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
- [ ] Scope matches one coherent logical PR; deferred work not pulled in early
- [ ] Engineering Standards and Security/Threat Model review triggers considered ([Eng Standards](../docs/planning/00-finstack-ai-engineering-standards.md), [Threat Model](../docs/planning/06-finstack-ai-security-threat-model.md))
- [ ] Schema/contract changes use the [change-classification template](../docs/implementation/schema-change-template.md) when applicable
- [ ] Exceptions/waivers recorded when required ([exceptions register](../docs/implementation/exceptions-register.md), [architecture allowlist](../tools/architecture/allowlist.toml))
- [ ] Acceptance evidence updated when criteria are claimed ([evidence register](../docs/implementation/evidence-register.md))
- [ ] Commits are DCO signed-off (`git commit -s`)
- [ ] Affected `mise run` / `cargo` checks documented below (include `mise run architecture` when boundaries change; `mise run schema-governance` when ADRs/schemas/fixtures change)
- [ ] Hosted CI workflows considered (see [`.github/ci/README.md`](./ci/README.md)); supply-chain/secret jobs name TM/ENG controls when security-relevant

## Verification

```text
# Prefer: mise run ci
# Commands run and results
```

## Risk / follow-ups

-
