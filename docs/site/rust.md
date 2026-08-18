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

That binary builds an `Agent` over a process-local native Ollama loopback
provider, calls `run`, and prints the completion. No credential is required.

`Agent::builder` registers the model and journal store, then `build().await`
resolves once. `Agent::run` / `start` execute one run. `request.capability`
selects a model-activated variant; `None` runs the `Agent` that was called.
`Session::open` inspects an existing journal and does not continue a parked
run. `Lane::run` starts a new root on an idle lane through
`Agent::start_on_lane`. `Lane::suspend` parks the in-process driver without
dropping the journal; `Lane::resume` respawns `RunTaskOwner` through
`WorkflowSession::with_ports`. `Lane::append_text` still does not start a
run. `AgentRun::start_child` prepares and accepts a child through
`ChildRunPrepared` and `AgentInvoker`. Prefer isolated placement so the
parent journal stays operable. `AgentRun::complete_external`
routes through `WorkflowSession::complete_external`. Python exposes the
same child-run and completion surfaces. WASM lane verbs remain a
tracked deferral. `RemoteChildSession` dispatch stays excluded.

## Starters

| Binary | Role |
| --- | --- |
| `minimal` | Model-only loopback run |
| `coding` | Calculator, filesystem, shell, context, sliding-window compaction, verifier. Summarize compaction stays unlandable; see [why summarize compaction cannot complete](middleware.md#why-summarize-compaction-cannot-complete) |
| `service` | Resolve once, health, one request |
| `diagnostic` | Credential-free `AgentSpec` and lock fingerprints |

See [examples/rust-minimal](../../examples/rust-minimal/README.md).
Trust class: [T1](security-trust-levels.md).

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
