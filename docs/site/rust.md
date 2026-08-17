# Rust guide

Public crate: `finstack-ai` (SDK/facade). Kernel, runtime, and protocol
crates are not a second constructor path; start here.

Workspace version is **1.0.0** unpublished. The last public tag is
`v0.1.0`. crates.io publication remains blocked on owner registry
credentials.

## Quick start

```bash
cargo run -p finstack-ai-native-examples --bin minimal --offline --locked
```

That binary builds an `Agent` over a process-local loopback OpenAI-compatible
provider, calls `run`, and prints the completion. No credential is required.

`Agent::builder` registers the model and journal store, then `build().await`
resolves once. `Agent::run` / `start` execute one run. `request.capability`
selects a model-activated variant; `None` runs the `Agent` that was called.
`Session::open` inspects an existing journal and does not continue a parked
run. Python and WASM expose the same inspect path as `Agent.open_session`.

## Starters

| Binary | Role |
| --- | --- |
| `minimal` | Model-only loopback run |
| `coding` | Calculator, filesystem, shell, context, compaction, verifier — the compaction leg fails once the window triggers, see [why compaction cannot complete](middleware.md#why-compaction-cannot-complete) |
| `service` | Resolve once, health, one request |
| `diagnostic` | Credential-free `AgentSpec` and lock fingerprints |

See [examples/rust-minimal](../../examples/rust-minimal/README.md).
Trust class: [T1](security-trust-levels.md).

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
