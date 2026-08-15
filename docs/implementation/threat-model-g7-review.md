# Threat Model G7 implemented-control review

Date: 2026-08-15
Owner: me@jeickmeier.com
Workspace: **0.1.0 unpublished** (experimental WIT package names stay `@0.0.4`)

This file is the implemented-control matrix for G7 evidence collection.
It does **not** rewrite
[`docs/planning/06-finstack-ai-security-threat-model.md`](../planning/06-finstack-ai-security-threat-model.md),
which remains the pre-implementation design contract.

Cited by `G7-D-public-preview-f7c7e70b9e04`. Rows that wait on a public
tag or registry publish are **accepted residuals** for the unpublished
`0.1.0` preview, matching the G6 unpublished-checkpoint precedent.
Independent review (Threat Model §13.3) is G8 / PR-065 and is out of scope.

Trust labels: [docs/site/security-trust-levels.md](../site/security-trust-levels.md).
Deployment gates: [docs/site/security-deployment.md](../site/security-deployment.md).

## SEC-INV-001–013

| Control | Evidence ID or test path | Residual (TM §16) | G7 disposition |
| --- | --- | --- | --- |
| SEC-INV-001 Content never grants authority | PR-016 / PR-018 / PR-056 tool-policy and capability fixtures | Trusted in-process code can still subvert policy | evidenced |
| SEC-INV-002 No effect before committed intent | PR-014 / PR-020 / PR-048 commit-before-effect and crash-prefix | At-least-once external effects remain | evidenced |
| SEC-INV-003 Authenticated external completion and interaction | PR-042 / PR-043 / PR-044 / PR-048 | Application must bind principals | evidenced |
| SEC-INV-004 Idempotent duplicates; fail-closed conflict | PR-014 / PR-042 / PR-043 / PR-048 | Uncertainty may suspend | evidenced |
| SEC-INV-005 Secrets not in ordinary records/events/exports | PR-003 canaries; PR-024 / PR-057 redaction | Trusted code can still read process memory | evidenced |
| SEC-INV-006 Isolated extensions have no ambient access | PR-052 / PR-054 deny-by-default WASI and lockfile | In-process guests are not T3 | evidenced |
| SEC-INV-007 All untrusted lengths/counts/time/fuel bounded | PR-013 / PR-020 / PR-052 / PR-058 | Deployer must keep limits | evidenced |
| SEC-INV-008 Principal/tenant/run/plugin identity not swapped | PR-011 / PR-047 / PR-058 / PR-059 TM-19 | Application must not add user-selected ownership | evidenced |
| SEC-INV-009 Terminal records immutable | PR-011 / PR-017 observers non-semantic | — | evidenced |
| SEC-INV-010 Journal corruption fails closed | PR-039 / PR-040 / PR-041 / PR-048 | Checksums are not an authentication chain | evidenced |
| SEC-INV-011 Grants and privileged actions observable without secrets | PR-052 / PR-057 redacted export | — | evidenced |
| SEC-INV-012 Minimal/default builds omit privileged batteries | `tools/wasm_package/check.py graph`; cargo tree on kernel/runtime/SDK | — | evidenced |
| SEC-INV-013 Compaction preserves policy/provenance | PR-018 / PR-023 / PR-056 / PR-057 | One late-tier `before_model` owner | evidenced |

## TM-01–TM-21

| Control | Evidence ID or test path | Residual (TM §16) | G7 disposition |
| --- | --- | --- | --- |
| TM-01 Prompt injection | PR-016, PR-018, PR-056 | Content is T5 | evidenced |
| TM-02 Fabricated/mutated tool calls | PR-010, PR-011, PR-015 | — | evidenced |
| TM-03 Filesystem/shell escape | PR-025, PR-056 | Native tools remain T1 | evidenced |
| TM-04 Secret leak | PR-003, PR-024, PR-057, this PR's site/SECURITY sweep | Trusted process memory | evidenced |
| TM-05 Browser credentials / persistent data | PR-034, PR-037, PR-038; [wasm.md](../site/wasm.md) | Origin-scoped IndexedDB | evidenced |
| TM-06 In-process mistaken for isolated | [security-trust-levels.md](../site/security-trust-levels.md); starter labels | Trusted-code residual | evidenced |
| TM-07 WIT/plugin escape or exhaustion | PR-049–PR-054; G6 | — | evidenced |
| TM-08 Plugin substitution / ABI | PR-052–PR-054 lockfile/digest | — | evidenced |
| TM-09 Remote frame abuse | PR-058 | Auth mechanism is a deployment gate | evidenced |
| TM-10 Spoofed external completion | PR-042, PR-043, PR-048 | — | evidenced |
| TM-11 Unauthorized interaction resolution | PR-044, PR-048 | — | evidenced |
| TM-12 Journal tamper/truncation | PR-039–PR-041, PR-048 | Checksums ≠ authentication | accepted residual |
| TM-13 Snapshot forgery | PR-041 | Snapshots never authoritative | evidenced |
| TM-14 Cancel/deadline race | PR-011, PR-045, PR-048 | At-least-once | evidenced |
| TM-15 Child-run policy evasion | PR-011, PR-046, PR-047 | — | evidenced |
| TM-16 Parser/size DoS | PR-013, PR-020, PR-039, PR-058 | — | evidenced |
| TM-17 Observer leak or blockage | PR-017, PR-057 | — | evidenced |
| TM-18 Release/dependency compromise | [release-rehearsal.md](release-rehearsal.md); PR-061 rehearsal provenance; `G7-D-public-preview-f7c7e70b9e04` | Public tag, registry publish, hosted provenance | accepted residual |
| TM-19 Tenant/session swap | PR-058, PR-059 | — | evidenced |
| TM-20 Artifact reference confusion | PR-022, PR-037, PR-056 | Scanning is a deployment gate | evidenced |
| TM-21 Compaction safety/provenance | PR-018, PR-023, PR-056, PR-057 | — | evidenced |

## Threat Model §§12–14 G7 bullets

| Bullet | Evidence | G7 disposition |
| --- | --- | --- |
| §12 release rehearsal of contents, feature sets, ownership, reproducibility | [release-rehearsal.md](release-rehearsal.md); PR-061 two-run comparable sha256 `c41a0fdd5d4fd0592af64a2e475ecfd5258bda45993df46ad120efa6a4c14de0` | evidenced locally; public tag **accepted residual** |
| §13.2 updated threat model | This file (implemented-control). Planning TM stays design-time | evidenced; named `G7-D-public-preview-f7c7e70b9e04` |
| §13.2 external surface review | [docs/site](../site/README.md) guides + trust/deployment pages | evidenced |
| §13.2 SBOM / provenance | Local rehearsal checksums/SBOMs; not published | local evidenced; tagged provenance **accepted residual** |
| §13.2 vulnerability process | [SECURITY.md](../../SECURITY.md) | evidenced; supported-version table covers unpublished `0.1.0`; tag **accepted residual** |
| §13.2 security deployment guide | [security-deployment.md](../site/security-deployment.md) | evidenced |
| §13.3 independent review | G8 / PR-065 | out of scope |
| §14 revoke releases / issue advisories across registries | Process exists; requires published artifacts to exercise | **accepted residual** |
| §14 logs have stable identifiers without sensitive payloads | PR-057 redacted export | evidenced |

## Out of scope

- Independent security audit (G8 / PR-065)
- `0.1.0` tag or registry publish (separately named external actions)
- `@1.0.0` WIT worlds (PR-062)
