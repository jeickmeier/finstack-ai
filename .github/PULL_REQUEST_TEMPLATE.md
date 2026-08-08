## Summary

<!-- What does this change and why? -->

## Logical PR / plan reference

<!-- e.g. PR-001 from docs/planning/04-finstack-ai-implementation-plan.md -->

-

## Checklist

- [ ] Architecture review checklist completed: [.github/ARCHITECTURE_REVIEW_CHECKLIST.md](./ARCHITECTURE_REVIEW_CHECKLIST.md)
- [ ] Dependency direction preserved: `kernel <- runtime <- SDK/bindings`; protocol stays outward (kernel DTOs only as needed)
- [ ] No forbidden kernel I/O or host-language binding dependencies introduced
- [ ] Scope matches one coherent logical PR; deferred work not pulled in early
- [ ] Engineering Standards and Security/Threat Model review triggers considered ([Eng Standards](../docs/planning/00-finstack-ai-engineering-standards.md), [Threat Model](../docs/planning/06-finstack-ai-security-threat-model.md))
- [ ] Exceptions/waivers recorded when required ([exceptions register](../docs/implementation/exceptions-register.md), [architecture allowlist](../tools/architecture/allowlist.toml))
- [ ] Acceptance evidence updated when criteria are claimed ([evidence register](../docs/implementation/evidence-register.md))
- [ ] Commits are DCO signed-off (`git commit -s`)
- [ ] Affected `mise run` / `cargo` checks documented below (include `mise run architecture` when boundaries change)
- [ ] Hosted CI workflows considered (see [`.github/ci/README.md`](./ci/README.md)); supply-chain/secret jobs name TM/ENG controls when security-relevant

## Verification

```text
# Prefer: mise run ci
# Commands run and results
```

## Risk / follow-ups

-
