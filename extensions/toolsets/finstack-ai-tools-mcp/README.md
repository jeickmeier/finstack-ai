# finstack-ai-tools-mcp

Trusted native Model Context Protocol client behind the public `Toolset`
port. The crate speaks protocol revision **`2026-07-28`**. There is **no
`initialize` handshake** — `initialize` and `notifications/initialized`
were removed by SEP-2575. Every request carries `params._meta` with
`io.modelcontextprotocol/protocolVersion` and
`io.modelcontextprotocol/clientCapabilities`.

This is a third-party protocol client, **not** the finstack process-plugin
surface (handshake-only in the 1.0 compatibility policy). It is not an MCP
server catalogue; connection targets come from a host-supplied
deny-by-default allowlist at construction.

## Scope

Implemented: `tools/list` at construction and `Toolset::call` for
`tools/call`.

Not implemented:

- **Sampling.** MCP sampling inverts control and would create a model
  request from inside a committed tool effect with no locked context
  profile, no budget parent, and an effect graph the kernel cannot
  linearize. Sampling is unsupported and will stay unsupported.
- Elicitation / MRTR (`input_required`).
- `resources/list` / `ContextProvider`.
- Mid-run catalog mutation. `notifications/tools/list_changed` is
  observer-only; adopters who need new tools re-resolve the agent.

## Catalog freeze

`tools/list` runs inside `McpToolsetFactory::construct` /
`McpToolset::connect` exactly once. The resulting `Arc<[ToolSpec]>` is
frozen. The run hot path does no network and no registry work.

## Classification

MCP `ToolAnnotations` are untrusted hints. They are never mapped onto
`SideEffectClass` or `RetrySafety`. Every tool defaults to
side-effecting, not retry-safe, and approval-required. Hosts may relax
named tools in `McpConfig`; the default is never permissive.

`reconcile()` returns `NonRepeatable` unless the tool is explicitly
retry-safe in config. Crash recovery therefore `SuspendUncertain` for
most MCP tools — that is expected.

Network `$ref` values in `inputSchema` are rejected. They are never
resolved.

Oversize results are truncated client-side at `max_result_bytes` with a
recorded `mcp_limit_exceeded` marker.

## Transports and trust

- **stdio** — spawned subprocess, empty child environment, newline-framed
  JSON. Trust class **T1**. This crate is not a sandbox.
- **Streamable HTTP** — POST only. Trust class **T4**.
- **SSE-only** servers are rejected.

Do not describe either transport as isolated. FR-08 confinement is a
follow-on for stdio, not a precondition.

The invocation digest covers server identity (command line or URL) and
the resolved tool-name set so the agent lock detects catalog drift.

## Non-ASCII tool names

Non-ASCII `Mcp-Name` values use the spec sentinel
`=?base64?{Base64EncodedValue}?=`. The crate takes `base64` for that
path instead of refusing spec-legal names.

```rust
use finstack_ai_tools_mcp::{HttpConfig, McpConfig, McpToolsetFactory};

# async fn demo() -> Result<(), finstack_ai_tools_mcp::McpError> {
let config = McpConfig::default()
    .allow_url("https://mcp.example.invalid/mcp")?
    .http(HttpConfig::try_new("https://mcp.example.invalid/mcp")?)?;
let _factory = McpToolsetFactory::new(config);
# Ok(())
# }
```
