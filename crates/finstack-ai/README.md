# `finstack-ai`

Public Rust SDK and composition facade for the finstack-ai engine. Application
code constructs agents, sessions, lanes, and runs here. The kernel, runtime,
and protocol crates define lower-level contracts; they are not parallel agent
construction APIs.

Workspace manifests are staged at **2.0.0**. Build from the repository until a
2.0 release is published.

## Quick start

Run the deterministic offline starter:

```bash
cargo run -p finstack-ai-native-examples --bin minimal --offline --locked
```

The starter constructs an explicit model port, journal store, security context,
and `AgentRunRequest`; no credential or external service is required. See
[`examples/rust-minimal/src/lib.rs`](../../examples/rust-minimal/src/lib.rs) for
the complete composition.

The primary flow is:

1. Resolve concrete model and journal-store ports.
2. Build an `Agent` with stable component and bundle identities.
3. Construct `AgentRunRequest` with input, model identity, and
   `RunSecurityContext`.
4. Call `Agent::start` for a control handle or `Agent::run` to await the retained
   terminal result.
5. Consume event batches explicitly; dropping a run handle does not cancel the
   durable run.

## Features

The default feature is `native-tokio`. Provider and tool batteries are opt-in so
applications pay only for the HTTP stacks and capabilities they ship.

- `provider-openai`, `provider-openrouter`, `provider-anthropic`,
  `provider-gemini`, `provider-ollama` — trusted native model batteries.
- `tool-openrouter-media`, `tool-openai-media`, `tool-e2b`,
  `tool-video-compose` — optional trusted tool batteries.
- `workflow-media-pipeline` — the MoviePlan render pipeline over the
  OpenRouter media and video-compose toolsets, which it enables in turn.
- `linked-providers`, `linked-tools`, `linked-all` — curated convenience sets.
- `vendored-tls` — vendored native TLS where the selected providers support it.
- `wasm-host` — browser/WASM host-driven runtime; disable default features.

Provider factories do not grant authority by themselves. Supply credentials,
network policy, stores, and host callbacks explicitly.

## Lifecycle guarantees

- Recoverable effect intent is committed before dispatch.
- Duplicate semantic identities are idempotent; conflicts fail closed.
- Cancellation, deadlines, and uncertain outcomes remain distinct.
- Sessions and lanes are journal-backed; run handles are observation/control
  handles, not ownership of durable execution.
- Child runs preserve the committed parent/effect mapping and explicit
  placement policy.
- Rust owns semantic behavior across the Python and WASM bindings.

## Extension model

Trusted native batteries implement runtime ports under `extensions/`. Directory
families are organizational: inspect a crate's trait implementations to learn
which ports it provides. Untrusted components belong behind the WIT/Wasmtime
plugin host rather than in the SDK process.

Useful references:

- [Workspace architecture and developer setup](../../README.md)
- [Rust examples](../../examples/rust-minimal/)
- [Durable interaction example](../../examples/durable-interaction/)
- [Media pipeline](../../extensions/workflow/finstack-ai-workflow-media-pipeline/README.md)
- [Plugin host](../../plugins/finstack-ai-plugin-host/README.md)
- [Changelog](../../CHANGELOG.md)

## Validation

From the repository root:

```bash
mise run ci-rust
```

This builds rustdoc with warnings denied, checks public API inventories and
layering, runs workspace tests and doctests, and applies dependency policy.

## License and governance

[MIT](../../licenses/LICENSE-MIT) OR
[Apache-2.0](../../licenses/LICENSE-APACHE).
[DCO](../../CONTRIBUTING.md). [Maintainers](../../GOVERNANCE.md).
