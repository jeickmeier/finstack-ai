# Security deployment gates

These choices are required before deploying the affected surface. They do
not block kernel implementation. Unconfigured choices fail closed or stay
disabled. Source: Threat Model §15.

| Choice | Owner | Fail-closed default |
| --- | --- | --- |
| Remote authentication | Server/application owner | No accept without `SecurityAuditGate`; no bearer over plaintext TCP |
| Tenant authorization and identity mapping | Deploying application | Journal-authoritative tenant scope; unknown locator on mismatch |
| Journal/artifact encryption and key rotation | Store/deployment owner | Disabled until configured; do not claim encryption from checksums |
| Retention, deletion, export, legal hold | Product/deployment owner | No user data until the owner sets policy |
| Plugin publisher trust roots | Host administrator | Deny-by-default; no third-party enablement without roots |
| Network egress for tools/providers | Deployment owner | Explicit endpoints only; no ambient network in T3 |
| Shell/computer-use sandbox | Battery/deployment owner | Native in-process execution remains trusted (T1) |
| Artifact malware/content scanning | Application owner | Optional hook; off unless required |

## Vulnerability process

Private reports go to `me@jeickmeier.com`. See [SECURITY.md](../../SECURITY.md).
Local tag `v1.0.0` exists. The last pushed GitHub tag is `v0.1.0`.
Tagged registry support begins when crates.io / PyPI / npm publication
is completed. Until then, the local tag, the last pushed GitHub tag, and
the default-branch tip are the security contact surface.

## Trust

Read [trust levels](security-trust-levels.md) before registering any
extension.
