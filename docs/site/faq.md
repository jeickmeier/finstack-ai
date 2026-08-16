# FAQ

Workspace version is **1.0.0**. Local tag `v1.0.0` exists. The last pushed
GitHub tag is `v0.1.0`. crates.io / PyPI / npm packages are not published.

## How do I install finstack-ai?

From this repository. Rust crates are path dependencies until crates.io
publishes. Python: build or editable-install
`bindings/finstack-ai-python`. JavaScript: `mise run stage-wasm` or
`mise run generate-wasm`, then consume the packed tarball. See
[Rust](rust.md), [Python](python.md), and [WASM](wasm.md).

## Is 1.0.0 published?

No. Version fields and the local tag are `1.0.0`. Registries and the
pushed GitHub tag have not caught up. Do not treat a staged wheel or
tarball as a crates.io / PyPI / npm release.

## Which crate do I depend on?

`finstack-ai` (SDK/facade). Kernel, runtime, and protocol crates are not a
second constructor path. Start at [docs/site/rust.md](rust.md).

## How do model capabilities activate?

Explicitly. Pass `AgentRunRequest.capability` (Rust) or `capability=` on
`Agent.start` / `Agent.run` (Python). `None` runs the agent that was
called. An unknown catalog id fails closed. Word overlap in user input
does not select a capability.

## Does `open_session` continue a parked run?

No. `Session::open` (Rust) and `Agent.open_session` (Python / WASM)
rebuild a handle from the journal and do not respawn parked runs. See
[durability](durability.md).

## Are in-process tools a sandbox?

No. Native, Python, and JavaScript extensions inherit host authority
([T1](security-trust-levels.md) / [T2](security-trust-levels.md)). Isolated
WIT/Wasmtime guests are [T3](security-trust-levels.md) when loaded through
`finstack-ai-plugin-host`.

## Where do secrets go?

Never in `AgentSpec`, model context, journals, errors, or browser bundles.
Use secret references and terminate credentials at a trusted proxy. See
[provider security](provider-security.md).

## Is this LTS?

No. Support windows for `1.0.x` are documented in [support](support.md).
The `release/1.0` maintenance branch is not created in this tree.

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
