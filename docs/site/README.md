# finstack-ai public documentation

Workspace version **0.0.4** is unpublished. `0.1.0` is the forthcoming
public-preview cut (PR-061). Nothing on this site is a published package,
a git tag, or a passed G7 decision.

Planning files under [`docs/planning/`](../planning/README.md) remain the
implementation contract. Delivery status lives in
[`docs/implementation/`](../implementation/README.md). This directory is
the public index and does not replace either layer.

## Guides

| Guide | Topic |
| --- | --- |
| [Concept](concept.md) | Kernel, runtime, SDK, leaves, six ports, commit-before-effect |
| [Rust](rust.md) | Native SDK quick start |
| [Python](python.md) | Staged wheel, rust-backed vs callback |
| [WASM](wasm.md) | `@finstack/ai` worker default |
| [Durability](durability.md) | Journals, inspect-not-continue, at-least-once |
| [Providers](provider.md) | Separate crates; no secrets in `AgentSpec` |
| [Toolsets](toolset.md) | Calculator, filesystem, shell |
| [Plugins](plugin.md) | Experimental `@0.0.4` WIT |
| [Server](server.md) | Loopback/Unix reference server |
| [Migration](migration.md) | Compatibility policy and release rehearsal |
| [Architecture](architecture.md) | Native composition diagram |
| [Provider security](provider-security.md) | Endpoint and credential rules |

## Security

| Page | Topic |
| --- | --- |
| [Trust levels](security-trust-levels.md) | T0–T5 matrix (single owner) |
| [Deployment gates](security-deployment.md) | Threat Model §15 choices |

## Governance

- Dual-licensed [MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE)
- [DCO / contributing](../../CONTRIBUTING.md)
- [Maintainers](../../GOVERNANCE.md)
- [ADRs](../implementation/adr-register.md)
- [Public RFCs](../rfcs/README.md)
- [Security policy](../../SECURITY.md)

## Local verification

```text
mise run docs-links
mise run docs-quickstarts
mise run check-plugin-template
```
