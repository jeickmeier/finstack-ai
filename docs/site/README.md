# finstack-ai public documentation

Workspace version **1.0.0**. Local tag `v1.0.0` exists. The last
pushed GitHub tag is `v0.1.0`. crates.io / PyPI / npm packages are not
published. Permanent WIT worlds are `@1.0.0`. Experimental WIT package
names stay `@0.0.4`.

Planning files under [`docs/planning/`](../planning/README.md) remain the
implementation contract. Delivery status lives in
[`docs/implementation/`](../implementation/README.md). This directory is
the public index and does not replace either layer.

## Guides

| Guide | Topic |
| --- | --- |
| [Concept](concept.md) | Kernel, runtime, SDK, leaves, six ports, commit-before-effect |
| [FAQ](faq.md) | Install, capabilities, sessions, secrets, support |
| [Troubleshooting](troubleshooting.md) | Stable error codes and common failures |
| [Rust](rust.md) | Native SDK quick start |
| [Python](python.md) | Staged wheel, rust-backed vs callback |
| [WASM](wasm.md) | `@finstack/ai` worker default |
| [Durability](durability.md) | Journals, inspect-not-continue, at-least-once |
| [Providers](provider.md) | Separate crates; no secrets in `AgentSpec` |
| [Toolsets](toolset.md) | Calculator, filesystem, MCP, shell |
| [Middleware](middleware.md) | Stage fold, what can land, compaction status |
| [Plugins](plugin.md) | Frozen `@1.0.0` WIT; experimental `@0.0.4` stays loadable |
| [Server](server.md) | Loopback/Unix reference server |
| [Migration](migration.md) | Compatibility policy and release rehearsal |
| [Conformance](conformance.md) | Published suites and badge process |
| [Support](support.md) | Support windows; not LTS |
| [Performance](performance.md) | Framework vs model time, batching, reuse |
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
- [Code of Conduct](../../CODE_OF_CONDUCT.md)
- [Maintainers](../../GOVERNANCE.md)
- [ADRs](../implementation/adr-register.md)
- [Public RFCs](../rfcs/README.md)
- [Security policy](../../SECURITY.md)

## Local verification

```text
mise run docs-links
mise run docs-quickstarts
mise run docs-notebooks
mise run check-plugin-template
```
