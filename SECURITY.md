# Security policy

## Reporting a vulnerability

Report security issues **privately** to the security response owner:

- Email: `me@jeickmeier.com`

Do **not** open a public GitHub issue for vulnerabilities that could harm users or deployments.

Please include:

- a description of the issue and its impact;
- steps to reproduce or a proof of concept when safe; and
- affected package versions or commit SHAs when known.

## Response owner

| Role | Contact |
| --- | --- |
| Security response owner | `me@jeickmeier.com` |

This owner is responsible for vulnerability triage, incident coordination, and disclosure.

## Severity rubric

| Severity | Meaning | Initial target |
| --- | --- | --- |
| Critical | Remote code execution, credential theft, durable-history forgery, or cross-tenant breakout in a supported configuration | Acknowledge within one business day; begin mitigation immediately |
| High | Privilege escalation, unauthorized privileged tool/plugin dispatch, or confidentiality loss of secrets/journal contents | Acknowledge within two business days; prioritize a fix or mitigating guidance |
| Medium | Integrity, availability, or isolation defects with constrained preconditions or limited blast radius | Acknowledge within three business days; schedule a fix for the next supported release train |
| Low | Defense-in-depth gaps, hardening opportunities, or issues requiring uncommon local trust assumptions | Acknowledge within five business days; track for a later hardening release |

Severity may be raised or lowered after triage when impact, exploitability, or deployment assumptions change.

## Acknowledgement and disclosure

- The response owner aims to acknowledge private reports within **three business days**, and sooner for Critical/High issues per the rubric above.
- Coordinated disclosure and embargo details are agreed case by case. Default embargo expectation is **90 days** from acknowledgement, or until a patched release is available, whichever comes first; Critical issues may use a shorter embargo when users remain exposed.
- Security fixes that alter durable meaning or public protocols still require compatibility handling under project governance.

## Supported versions

Supported-version policy for the staged `2.0.0` default branch, tagged local
`1.0.0`, and the `0.1.0` preview line. Local tag `v1.0.0` is
`6e9ec39fae89a70f696ee740de2d2094670cba3e`.
The last pushed GitHub tag remains `v0.1.0`. crates.io / PyPI / npm
stay unpublished. This is not LTS.

| Version | Supported |
| --- | --- |
| `2.0.x` staging line (`main` / trunk) | Security fixes accepted before the first `v2.0.0` tag |
| `1.0.x` (local tag `v1.0.0`) | Security fixes accepted on this lockstep line |
| `0.1.x` preview (tag `v0.1.0`) | Security-only for 90 days after 2026-08-15 |
| Historical unpublished snapshots (`0.0.4` and earlier) | Not supported |

## Trust boundaries

Trust labels used throughout the repository describe where authority executes:

| Label | Boundary |
| --- | --- |
| T0 | Deterministic kernel semantics; no I/O or ambient authority |
| T1 | Trusted native Rust running in-process with host-granted authority |
| T2 | Trusted host-language callbacks or adapters running in the process/page |
| T3 | Wasmtime component guest constrained by explicit host capabilities |
| T4 | Authenticated remote principal, service, or sandbox boundary |
| T5 | Untrusted content, model output, journal payload, or external data |

T1 and T2 code is not sandboxed. Content and model output never grant
authority. Privileged actions require an authenticated principal, tenant or
scope, exact target, and action, and fail closed when a binding is absent or
mismatched.

## Related documents

- [Repository governance](GOVERNANCE.md)
- [Engineering rules](.agents/rules/01-engineering-conformance.md)
- Dual-license: [MIT](licenses/LICENSE-MIT) OR [Apache-2.0](licenses/LICENSE-APACHE)
