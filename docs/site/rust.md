# Rust guide

Public crate: `finstack-ai` (SDK/facade). Kernel, runtime, and protocol
crates are not a second constructor path; start here.

Workspace version is **1.0.0** unpublished. The last public tag is
`v0.1.0`. G7 passed. crates.io publication remains blocked on owner
registry credentials.

## Quick start

```bash
cargo run -p finstack-ai-native-examples --bin minimal --offline --locked
```

That binary builds an `Agent` over a process-local loopback OpenAI-compatible
provider, calls `run`, and prints the completion. No credential is required.

`Agent::builder` registers the model and journal store, then `build().await`
resolves once. `Agent::run` / `start` execute one run. `open_session`
inspects an existing journal and does not continue a parked run.

## Starters

| Binary | Role |
| --- | --- |
| `minimal` | Model-only loopback run |
| `coding` | Calculator, filesystem, shell, context, compaction, verifier |
| `service` | Resolve once, health, one request |
| `diagnostic` | Credential-free `AgentSpec` and lock fingerprints |

See [examples/rust-minimal](../../examples/rust-minimal/README.md).
Trust class: [T1](security-trust-levels.md).

## License and governance

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[DCO](../../CONTRIBUTING.md). [Maintainers](../../GOVERNANCE.md).
[ADRs](../implementation/adr-register.md). [RFCs](../rfcs/README.md).
