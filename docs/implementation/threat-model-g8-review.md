# Threat Model G8 implemented-control review

Date: 2026-08-15
Owner: me@jeickmeier.com
Workspace: **1.0.0** (local tag `v1.0.0`; registries unpublished; experimental WIT package names stay `@0.0.4`; permanent worlds are `@1.0.0`)

This file is the implemented-control matrix for PR-064 / G8 evidence
collection. It does **not** rewrite
[`docs/planning/06-finstack-ai-security-threat-model.md`](../planning/06-finstack-ai-security-threat-model.md),
which remains the pre-implementation design contract. It does **not**
record `G8-D-*`. G8 later passed via
`G8-D-general-availability-a889a29a3f54` against
`6e9ec39fae89a70f696ee740de2d2094670cba3e`. This file remains the
review matrix, not the decision.

The G7 matrix
([threat-model-g7-review.md](threat-model-g7-review.md)) treated
Threat Model §13.3 as out of scope. This file owns §13.3 and
re-reviews TM-01–TM-21 against the PR-064 independent review.

Trust labels: [docs/site/security-trust-levels.md](../site/security-trust-levels.md).
Deployment gates: [docs/site/security-deployment.md](../site/security-deployment.md).
Independent review: [artifacts/pr-064/independent-review.md](artifacts/pr-064/independent-review.md).
Findings: [artifacts/pr-064/findings.md](artifacts/pr-064/findings.md).

Disposition vocabulary:

| Disposition | Meaning |
| --- | --- |
| evidenced | Implemented control plus a first-party test or checked-in artifact |
| accepted residual | Documented permanent or unpublished-preview residual; not an Engineering Standards waiver |
| finding | Links a `FIND-064-*` row that is still `Open` |

## TM-01–TM-21

| Control | Evidence ID or test path | Residual (TM §16) | G8 disposition |
| --- | --- | --- | --- |
| TM-01 Prompt injection | PR-016, PR-018, PR-056 | Content is T5 | evidenced |
| TM-02 Fabricated/mutated tool calls | PR-010, PR-011, PR-015 | — | evidenced |
| TM-03 Filesystem/shell escape | PR-025, PR-056; FIND-064-016 Closed; FIND-064-002 Open | Native tools remain T1; shell cwd re-resolve | finding ([FIND-064-002](artifacts/pr-064/findings.md#find-064-002)) |
| TM-04 Secret leak | PR-003, PR-024, PR-057; FIND-064-017 Closed | Trusted process memory | evidenced |
| TM-05 Browser credentials / persistent data | PR-034, PR-037, PR-038; [wasm.md](../site/wasm.md) | Origin-scoped IndexedDB | evidenced |
| TM-06 In-process mistaken for isolated | [security-trust-levels.md](../site/security-trust-levels.md); FIND-064-008 Accepted | Trusted-code residual | accepted residual |
| TM-07 WIT/plugin escape or exhaustion | PR-049–PR-054; FIND-064-015 Closed; FIND-064-001 Closed | Directory cache is not deserialized | evidenced |
| TM-08 Plugin substitution / ABI | PR-052–PR-054 lockfile/digest; FIND-064-001 Closed | Writable cache dir is not authentication | evidenced |
| TM-09 Remote frame abuse | PR-058; FIND-064-012 Closed; FIND-064-013 Closed; FIND-064-003 Open | Auth mechanism is a deployment gate; receipt map unbounded | finding ([FIND-064-003](artifacts/pr-064/findings.md#find-064-003)) |
| TM-10 Spoofed external completion | PR-042, PR-043, PR-048 | — | evidenced |
| TM-11 Unauthorized interaction resolution | PR-044, PR-048; FIND-064-014 Closed | — | evidenced |
| TM-12 Journal tamper/truncation | PR-039–PR-041, PR-048; FIND-064-009 Accepted; FIND-064-018 Closed | Checksums ≠ authentication | accepted residual |
| TM-13 Snapshot forgery | PR-041 | Snapshots never authoritative | evidenced |
| TM-14 Cancel/deadline race | PR-011, PR-045, PR-048; FIND-064-011 Accepted | At-least-once | accepted residual |
| TM-15 Child-run policy evasion | PR-011, PR-046, PR-047 | — | evidenced |
| TM-16 Parser/size DoS | PR-013, PR-020, PR-039, PR-058; `fuzz/` seven targets | FIND-064-006 Low: smoke not in `mise run ci` | evidenced |
| TM-17 Observer leak or blockage | PR-017, PR-057; FIND-064-019 Closed | — | evidenced |
| TM-18 Release/dependency compromise | [release-engineering.md](release-engineering.md); FIND-064-010 Accepted; FIND-064-004 Closed | Public tag exists; continuous deny/advisory restored; registry publish remains later | closed finding ([FIND-064-004](artifacts/pr-064/findings.md#find-064-004)); accepted residual for unpublished registries |
| TM-19 Tenant/session swap | PR-058, PR-059 | — | evidenced |
| TM-20 Artifact reference confusion | PR-022, PR-037, PR-056 | Scanning is a deployment gate | evidenced |
| TM-21 Compaction safety/provenance | PR-018, PR-023, PR-056, PR-057 | — | evidenced |

## Threat Model §13.3

| Bullet | Evidence | G8 disposition |
| --- | --- | --- |
| Journal / recovery integrity and external completion | [independent-review.md](artifacts/pr-064/independent-review.md) §3.1; FIND-064-009 Accepted; FIND-064-011 Accepted; FIND-064-018 Closed | evidenced, with accepted residuals |
| Remote framing, authentication hooks, authorization context | §3.2; FIND-064-012 Closed; FIND-064-013 Closed; FIND-064-003 Open; FIND-064-005 Open | finding ([FIND-064-003](artifacts/pr-064/findings.md#find-064-003), [FIND-064-005](artifacts/pr-064/findings.md#find-064-005)) |
| Interaction authorization and privileged tool dispatch | §3.3; FIND-064-014 Closed | evidenced |
| Filesystem / shell and artifact boundaries | §3.4; FIND-064-016 Closed; FIND-064-002 Open; FIND-064-007 Open | finding ([FIND-064-002](artifacts/pr-064/findings.md#find-064-002), [FIND-064-007](artifacts/pr-064/findings.md#find-064-007)) |
| WIT / Wasmtime permissions, limits, signatures, cache identity | §3.5; FIND-064-001 Closed; FIND-064-008 Accepted; FIND-064-015 Closed | evidenced, with accepted residual |
| Python / JavaScript callback lifecycle and browser credential guidance | §3.6; FIND-064-020 Closed | evidenced |
| Secret / redaction behaviour | §3.7; FIND-064-017 Closed | evidenced |
| Dependency / build / release provenance | §3.8; FIND-064-010 Accepted; FIND-064-004 Closed; FIND-064-006 Open | closed finding ([FIND-064-004](artifacts/pr-064/findings.md#find-064-004)); open Low ([FIND-064-006](artifacts/pr-064/findings.md#find-064-006)); accepted residual for unpublished registries |

## Threat Model §14 (advisory process)

| Bullet | Evidence | G8 disposition |
| --- | --- | --- |
| `SECURITY.md` identifies supported versions and reporting | [SECURITY.md](../../SECURITY.md) `0.1.0` + default-branch tip; owner `me@jeickmeier.com` | evidenced |
| Maintainers can revoke releases, plugin trust roots, and publishing credentials | Process exists; registries unpublished | accepted residual |
| Release process can issue advisories and patched artifacts | [docs/security/advisories/README.md](../security/advisories/README.md) empty index | evidenced process; published artifacts **accepted residual** |
| Logs have stable identifiers without sensitive payloads | PR-057; FIND-064-017 Closed | evidenced |
| Incident handling documents secret rotation / journal rebuild / plugin invalidation | [SECURITY.md](../../SECURITY.md); [security-deployment.md](../site/security-deployment.md) | evidenced |

## Out of scope

- A `G8-D-*` gate decision
- Cutting or publishing `1.0.0`
- Commissioning an external audit firm
- Hosted ten-minute fuzz soak
- Editing `docs/planning/06-finstack-ai-security-threat-model.md`
