# `finstack-ai`

Public Rust SDK and composition facade for the finstack-ai engine. Application
code constructs agents, sessions, lanes, and runs here. The kernel, runtime,
and protocol crates define lower-level contracts; they are not parallel agent
construction APIs.

Workspace manifests are staged at **2.0.0**. Build from the repository until a
2.0 release is published.

See the [task capability matrix](../../docs/capabilities.md) and
[staged API migration guide](../../docs/migration-2.0.md) for native hosting,
evaluation, unified search and typed Python counterparts.

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
  `provider-gemini`, `provider-ollama` — trusted native model batteries. Each
  enables its `Agent::linked` constructor independently; gateway routes require
  the feature for their selected wire protocol.
- `tool-openrouter-media`, `tool-openai-media`, `tool-e2b`,
  `tool-video-compose` — optional trusted tool batteries.
- `workflow-media-pipeline` — the MoviePlan render pipeline over the
  OpenRouter media and video-compose toolsets, which it enables in turn.
- `linked-providers`, `linked-tools`, `linked-all` — curated convenience sets.
- `vendored-tls` — vendored native TLS where the selected providers support it.
- `wasm-host` — browser/WASM host-driven runtime; disable default features.

Provider factories do not grant authority by themselves. Supply credentials,
network policy, stores, and host callbacks explicitly.

## Staged 2.0 composition migration

Pass context providers, middleware, and observers directly:
`builder.context_provider(provider).middleware(middleware).observer(observer)`.
Their descriptors supply the component identity and exact version, including
capability-scoped registrations. Remove the former first `ComponentRef` argument.
Models, toolsets, and journal stores still take explicit versioned identities.
A rebuilt lock now uses the declared identity; resolve a fresh composition after
migrating application aliases. Existing journal records are not rewritten.

`LinkedCommon::journal_store` accepts an explicit `(ComponentRef, Arc<dyn
JournalStore>)`. Provider factories default to bounded in-memory storage. When
using `NativeAgentBuilder::build_linked`, an explicit common journal replaces
the builder's store. All other ports pass through the same registration and
lock validation as direct composition.

Media configuration is now independent of provider constructors. Remove
`OpenAiAgentSpec::media_tools`, `OpenRouterAgentSpec::media_tools`, and the media
fields on `LinkedCommon`. The corresponding typed toolset/configuration crates
are exposed under `finstack_ai::media::{openai, openrouter, video, pipeline}` by
their individual tool features. Construct those objects with explicit credentials
and one shared artifact store, then supply `(ComponentRef, Arc<dyn Toolset>)`
registrations in `LinkedAgentPorts::toolsets`. Pass the same media and compose
handles into `MediaPipelineConfig`; keep render-state persistence explicit.
See the [pipeline composition example](../../extensions/workflow/finstack-ai-workflow-media-pipeline/README.md)
for the full native wiring.

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

With the opt-in `durable-host` feature, `durable::DurableHostBuilder::try_open`
opens host-owned SQLite stores. Register resolved agents by workflow kind, then
use `start`, `tick`, `inspect`, `pending`, `resolve`, and `shutdown`. Starting
commits admission and immutable recovery inputs before any model dispatch.
Ticks use the worker's leases, inbox and backoff while sharing the ordinary SDK
stage driver. A fresh process supplies the same definitions and storage paths;
recovery preserves the original input, authority, limits and deadline.

The [durable approval example](../../examples/durable-interaction/) demonstrates
complete process restart. Recovery fails explicitly on drift, missing storage,
or uncertain effects. A successful activation tool followed by process death
before `CapabilitiesActivated` needs reconciliation (`durable_capability_uncertain`);
tool output cannot restore authority. Tick admission reconciliation scans at most
256 descriptors per call and rotates across history. Browser hosting remains
application-driven; this SQLite/Tokio host is native-only.

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
