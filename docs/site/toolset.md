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
| `finstack-ai-tools-subagent` | `subagent_start` / `subagent_status` / `subagent_cancel` over `AgentInvoker` |
| `finstack-ai-tools-skills` | `capability_list` / additions-only `capability_activate` |
| `finstack-ai-tools-skill-import` | Composition-time `SKILL.md` importer; catalog default-off |
| `finstack-ai-tools-fetch` | `http_fetch` tool (`finstack-fetch` descriptor); deny-by-default HTTPS host allowlist, resolve-and-pin + redirect re-vetting, bounded reads, HTML→Markdown for inline delivery |

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

## Deferring a tool call

A tool that may suspend its first pass declares
`ToolSpec.deferral = Supported`. On the first pass the stream must emit
`ToolStreamItem::Deferred` as its **sole** terminal item — not alongside
`Completed`, stream errors, or further progress. The runtime commits the
deferral under the original effect id and the run enters
`AwaitingExternal`.

Completion has two paths. An external actor may finish the work and
submit through `complete_external` (for example via a workflow driver
session). For poll-backed deferrals the runtime derives the next wake
from the committed `next_poll_at` and calls `Toolset::reconcile` when
due (`due_polls` / `drive_due_polls`). A reconcile that returns
`StillRunning` keeps its replacement `next_poll_at` process-local;
`Completed` or a stream error settles the effect.

### Stable codes

Match on the `code` string; display messages may differ.

| Code | Meaning |
| --- | --- |
| `tool_deferral_not_declared` | Stream emitted `Deferred` but the spec did not declare `Supported`. |
| `tool_deferral_invalid` | Empty handle or `next_poll_at` after `expires_at`. |
| `tool_deferral_expired` | Committed deferral reached `expires_at`. |

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
