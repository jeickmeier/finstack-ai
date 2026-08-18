# Toolsets

A first-party toolset stays small: business logic aside, the registration
and policy surface is intended to stay under 100 lines (NFR-DX-002).

| Crate | Role |
| --- | --- |
| `finstack-ai-tools-calculator` | Bounded read-only arithmetic |
| `finstack-ai-tools-filesystem` | Capability-scoped root; no symlink escape |
| `finstack-ai-tools-mcp` | Allowlisted MCP servers; tools and untrusted `ContextProvider`; `construct_with_context` is the one-connection constructor |
| `finstack-ai-sandbox-e2b` | T4 remote sandbox leaf; `Agent.e2b_sandbox()` / `Agent.e2bSandbox()` on both bindings |
| `finstack-ai-tools-shell` | Deny-by-default argv, empty env, timeout |
| `finstack-ai-tools-skills` | `capability_list` / additions-only `capability_activate` |

MCP sampling is a nested committed model child of the open tool
(ADR-046). `resources/subscribe` accepts only names already in the
frozen `resources/list` snapshot; later `collect` re-reads those URIs.
`list_changed` is observed and does not add a tool to the live catalog.
`McpToolsetFactory::reconstruct` / `Agent.re_resolve()` /
`Agent.reResolve()` return a new lock. In-flight runs keep the previous
composition. Both bindings expose `re_resolve`. wasm-host fail-closed is
a Rust platform error, not a missing method.

The `coding` binary in [rust-minimal](../../examples/rust-minimal/README.md)
composes those leaves with repository/memory context, sliding-window
compaction, and a `before_finalize` verifier over a keyless loopback model.
The toolset, context, and sliding-window compaction legs work. Summarize
compaction completes via the runtime-owned phase — see
[how summarize compaction completes](middleware.md#how-summarize-compaction-completes).

Native toolsets are [T1](security-trust-levels.md). They inherit process
authority. Do not call them a sandbox. Untrusted code belongs on the
[plugin](plugin.md) path.

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
