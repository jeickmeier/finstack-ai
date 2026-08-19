# MCP Client Toolset Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `finstack-ai-tools-mcp`, a trusted native leaf crate that speaks
Model Context Protocol revision `2026-07-28` to allowlisted servers and surfaces
their tools as a single registered `Toolset`.

**Architecture:** One `Toolset` registration owns N `ToolSpec`s — exactly the
coarse-collection shape the port asks for. The client connects and enumerates
inside `ComponentFactory::construct`, which is async and runs exactly once at
agent build, then freezes the catalog into `Arc<[ToolSpec]>` so the run hot path
does zero network and zero registry work. The MCP server is a separate OS
process or remote endpoint, so untrusted code is never in-address-space; the
trust posture is `finstack-ai-tools-shell`'s (T1 native), not the plugin host's.

**Tech Stack:** Rust 1.97.1, tokio, `reqwest` (already a workspace dependency via
`finstack-ai-provider-anthropic`), `serde` / `serde_json`, `thiserror`,
`finstack-ai-runtime` with `native-tokio`.

## Global Constraints

- **Protocol revision is `2026-07-28`.** There is **no `initialize` handshake** —
  `initialize` and `notifications/initialized` were removed by SEP-2575. A
  client that blocks on an initialize round-trip will hang against a compliant
  server. Per-request `params._meta` replaces it.
- **`params._meta` is REQUIRED on every request**, carrying
  `io.modelcontextprotocol/protocolVersion` (string) and
  `io.modelcontextprotocol/clientCapabilities` (object). A request missing
  either is malformed: the server MUST reject with `-32602`, and over HTTP with
  `400`.
- **Scope excludes any server catalogue or discovery surface.** Connection
  targets come from a host-supplied deny-by-default allowlist passed at
  construction. `docs/planning/01-finstack-ai-product-requirements.md:258` names
  "an MCP server catalogue" as a non-goal; a protocol client behind the `Toolset`
  port is not a catalogue, but a registry would be.
- **Tool annotations are hints and are explicitly untrustworthy.** The MCP schema
  says verbatim that clients "should never make tool use decisions based on
  `ToolAnnotations` received from untrusted servers." Never map `readOnlyHint` or
  `idempotentHint` directly onto `SideEffectClass` / `RetrySafety`.
- **Never dereference network `$ref`.** SEP-2106 allows `$ref` to absolute URIs
  in `inputSchema`. Resolution must be disabled with no opt-in.
- **No host secret forwarding.** Mirror `finstack-ai-tools-shell`: empty child
  environment, deny-by-default allowlist.
- **Leaf crates carry no literal dependency versions.** Every dependency is
  `{ workspace = true }`; add the pin to `[workspace.dependencies]` if missing.
- **This crate is native-only** and must be added to the `FORBIDDEN_WASM`
  frozenset in `scripts/wasm_package/check.py`, next to the
  `finstack-ai-tools-shell` entry.
- `scripts/compat/public_items.py` does **not** cover extension leaves — its
  `RUST_LIBS` tuple is fixed to the three core `lib.rs` files. No frozen-surface
  risk here.
- Run `mise run ci` before opening a pull request (`CONTRIBUTING.md:18`).
- Do not commit unless the user explicitly requests a commit.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `extensions/toolsets/finstack-ai-tools-mcp/Cargo.toml` | Manifest; copy of the shell manifest with a changed name, description, and dependency set. |
| `.../src/lib.rs` | Public config, stable error codes, the `Toolset` impl, catalog freeze, and `ComponentFactory`. |
| `.../src/protocol.rs` | Wire DTOs — **crate-private**. JSON-RPC envelope, `_meta`, `Tool`, `CallToolResult`, `ContentBlock`. |
| `.../src/transport.rs` | `McpTransport` trait plus `StdioTransport` and `HttpTransport`. |
| `.../src/classify.rs` | Conservative `SideEffectClass` / `RetrySafety` assignment and the catalog digest. |
| `.../src/tests.rs` | Unit tests over a scripted in-memory transport. |
| `.../README.md` | Trust posture, allowlist, catalog-freeze semantics, protocol revision. |
| `.../tests/fixtures/*.json` | Recorded server responses; no live network in tests. |

---

### Task 1: Scaffold the crate and register it in the workspace

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-mcp/Cargo.toml`
- Create: `extensions/toolsets/finstack-ai-tools-mcp/src/lib.rs`
- Create: `extensions/toolsets/finstack-ai-tools-mcp/README.md`
- Modify: `Cargo.toml` (workspace members ~line 18, version pins ~line 75)
- Modify: `scripts/wasm_package/check.py`
- Modify: `extensions/toolsets/README.md`, `docs/site/toolset.md`,
  `docs/site/README.md:25`

**Interfaces:**
- Consumes: nothing
- Produces: crate `finstack-ai-tools-mcp` compiling as a workspace member

- [ ] **Step 1: Write the failing test**

Create `extensions/toolsets/finstack-ai-tools-mcp/src/lib.rs`:

```rust
//! Model Context Protocol client implementation of the public `Toolset` port.
#![warn(missing_docs)]

#[cfg(test)]
mod tests {
    #[test]
    fn crate_is_a_workspace_member() {
        assert_eq!(env!("CARGO_PKG_NAME"), "finstack-ai-tools-mcp");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p finstack-ai-tools-mcp --locked`
Expected: FAIL — `error: package ID specification did not match any packages`.

- [ ] **Step 3: Create the manifest and register it**

Create `extensions/toolsets/finstack-ai-tools-mcp/Cargo.toml`, copying the shell
manifest verbatim and changing only `name`, `description`, and dependencies:

```toml
[package]
name = "finstack-ai-tools-mcp"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Model Context Protocol client Toolset for finstack-ai"
readme = "README.md"

[dependencies]
base64 = { workspace = true }
finstack-ai-runtime = { workspace = true, default-features = false, features = ["native-tokio"] }
futures-util = { workspace = true }
reqwest = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["rt-multi-thread", "test-util"] }

[lints]
workspace = true
```

`base64` is **not** currently in `[workspace.dependencies]` — verified by
`grep -n base64 Cargo.toml`, which returns nothing. It is needed for the
`=?base64?…?=` header sentinel encoding in Task 4. Add the pin to the root
`Cargo.toml` `[workspace.dependencies]` block (lines 99-128) with a comment
naming its purpose, and confirm its license is already in the `deny.toml`
`[licenses] allow` list (lines 25-42) — `base64` is MIT OR Apache-2.0, both
already allowed, so no `deny.toml` edit is required.

If you would rather not take the dependency for one rare code path, the
alternative is to reject non-ASCII tool names at catalog-freeze time with
`MCP_PROTOCOL_VIOLATION` and drop `base64` entirely. That is a smaller
dependency graph at the cost of refusing a spec-legal server; decide before
Task 4 and record the choice in the crate README.

In the root `Cargo.toml`, add `    "extensions/toolsets/finstack-ai-tools-mcp",`
to `[workspace] members` in alphabetical position among the toolsets, and add
the pin to `[workspace.dependencies]`:

```toml
finstack-ai-tools-mcp = { path = "extensions/toolsets/finstack-ai-tools-mcp", version = "1.0.0" }
```

In `scripts/wasm_package/check.py`, add `"finstack-ai-tools-mcp",` to the
`FORBIDDEN_WASM` frozenset — this crate depends on tokio and reqwest and must
never enter a WASM bundle.

Add the index rows: `extensions/toolsets/README.md` (alphabetical, role
"Allowlisted MCP servers; no catalogue"), `docs/site/toolset.md` (same role,
plain backticks, no link), and append `MCP` to the comma list at
`docs/site/README.md:25`.

Create `README.md` covering: protocol revision `2026-07-28`, the deny-by-default
allowlist, catalog-freeze-at-construction semantics, the trust posture
(separate process or remote endpoint; **not** a sandbox), and an explicit note
that this is a third-party protocol client behind the `Toolset` port, **not** the
finstack process-plugin surface frozen as handshake-only at
`docs/implementation/1.0-compatibility-policy.md:31`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p finstack-ai-tools-mcp --locked`
Expected: PASS

- [ ] **Step 5: Regenerate the lockfile and verify the gates**

Run: `cargo generate-lockfile`
Run: `mise run check-wasm`
Expected: PASS — the crate is excluded from WASM.
Run: `mise run docs-links`
Expected: PASS

---

### Task 2: Wire DTOs with a forward-compatible content block

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-mcp/src/protocol.rs`
- Modify: `extensions/toolsets/finstack-ai-tools-mcp/src/lib.rs`
- Test: `.../src/protocol.rs` inline tests

**Interfaces:**
- Consumes: nothing
- Produces: `pub(crate) struct Request`, `pub(crate) struct Meta`,
  `pub(crate) struct Tool`, `pub(crate) struct ListToolsResult`,
  `pub(crate) struct CallToolResult`, `pub(crate) enum ContentBlock` —
  all crate-private, used by Tasks 3–6

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_content_block_type_does_not_fail_deserialization() {
        let json = r#"{"type":"future_block","payload":42}"#;
        let block: ContentBlock = serde_json::from_str(json).expect("unknown type must parse");
        assert!(matches!(block, ContentBlock::Unknown));
    }

    #[test]
    fn empty_string_cursor_is_a_valid_cursor() {
        let json = r#"{"resultType":"complete","tools":[],"nextCursor":""}"#;
        let result: ListToolsResult = serde_json::from_str(json).expect("parses");
        assert_eq!(result.next_cursor.as_deref(), Some(""));
    }

    #[test]
    fn absent_result_type_is_treated_as_complete() {
        let json = r#"{"tools":[]}"#;
        let result: ListToolsResult = serde_json::from_str(json).expect("parses");
        assert_eq!(result.result_type, ResultType::Complete);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-tools-mcp --locked protocol`
Expected: FAIL — module `protocol` does not exist.

- [ ] **Step 3: Write the DTOs**

Create `src/protocol.rs`. Every type is `pub(crate)` — wire DTOs must stay
crate-private, matching the provider leaves' rule.

```rust
//! Crate-private MCP wire types for revision 2026-07-28.

use serde::{Deserialize, Serialize};

/// Wire protocol revision this client speaks.
pub(crate) const PROTOCOL_VERSION: &str = "2026-07-28";

/// Required per-request metadata. Replaces the removed `initialize` handshake.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Meta {
    #[serde(rename = "io.modelcontextprotocol/protocolVersion")]
    pub(crate) protocol_version: &'static str,
    #[serde(rename = "io.modelcontextprotocol/clientCapabilities")]
    pub(crate) client_capabilities: ClientCapabilities,
    #[serde(
        rename = "io.modelcontextprotocol/clientInfo",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) client_info: Option<Implementation>,
}

impl Default for Meta {
    fn default() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            client_capabilities: ClientCapabilities::default(),
            client_info: Some(Implementation {
                name: "finstack-ai-tools-mcp",
                version: env!("CARGO_PKG_VERSION"),
            }),
        }
    }
}

/// Accurately declared client capabilities. Understating these makes the server
/// reject the request with -32021 MissingRequiredClientCapability.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ClientCapabilities {}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Implementation {
    pub(crate) name: &'static str,
    pub(crate) version: &'static str,
}

/// Result freshness discriminator. An ABSENT value means `Complete`; any
/// UNRECOGNIZED value must be treated as an error, not silently accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResultType {
    Complete,
    InputRequired,
    #[serde(other)]
    Unknown,
}

impl Default for ResultType {
    fn default() -> Self {
        Self::Complete
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ListToolsResult {
    #[serde(default)]
    pub(crate) result_type: ResultType,
    #[serde(default)]
    pub(crate) tools: Vec<Tool>,
    /// Opaque. An EMPTY STRING is a valid cursor — terminate only on `None`.
    #[serde(default)]
    pub(crate) next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Tool {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    /// REQUIRED by the schema; root MUST be `type: "object"`.
    pub(crate) input_schema: serde_json::Value,
    #[serde(default)]
    pub(crate) output_schema: Option<serde_json::Value>,
    /// HINTS ONLY. Never map directly onto SideEffectClass or RetrySafety.
    #[serde(default)]
    pub(crate) annotations: Option<ToolAnnotations>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct ToolAnnotations {
    #[serde(default)]
    pub(crate) read_only_hint: Option<bool>,
    #[serde(default)]
    pub(crate) idempotent_hint: Option<bool>,
    #[serde(default)]
    pub(crate) destructive_hint: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CallToolResult {
    #[serde(default)]
    pub(crate) result_type: ResultType,
    #[serde(default)]
    pub(crate) content: Vec<ContentBlock>,
    #[serde(default)]
    pub(crate) structured_content: Option<serde_json::Value>,
    /// Tool-level error channel. Absent means false. This arrives on a JSON-RPC
    /// SUCCESS response, not in the `error` member.
    #[serde(default)]
    pub(crate) is_error: bool,
}

/// Exactly five spec variants plus a forward-compatible fallback.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ContentBlock {
    Text { text: String },
    Image { data: String, mime_type: String },
    Audio { data: String, mime_type: String },
    ResourceLink { uri: String },
    EmbeddedResource { resource: serde_json::Value },
    #[serde(other)]
    Unknown,
}
```

Add `mod protocol;` to `src/lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p finstack-ai-tools-mcp --locked protocol`
Expected: PASS

- [ ] **Step 5: Verify the lint gate**

Run: `cargo clippy -p finstack-ai-tools-mcp --all-targets --locked -- -D warnings`
Expected: PASS

---

### Task 3: Transport trait and a scripted test transport

Both real transports are deferred to Task 4 so this task stays independently
reviewable and every later task can be tested with zero network.

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-mcp/src/transport.rs`
- Test: `.../src/transport.rs` inline tests

**Interfaces:**
- Consumes: `protocol::{Meta, PROTOCOL_VERSION}` (Task 2)
- Produces: `pub(crate) trait McpTransport` with
  `async fn request(&self, method: &str, params: serde_json::Value) ->
  Result<serde_json::Value, McpError>`, and `ScriptedTransport` for tests

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn scripted_transport_returns_the_queued_response() {
    let transport = ScriptedTransport::new(vec![serde_json::json!({
        "resultType": "complete",
        "tools": []
    })]);
    let value = transport
        .request("tools/list", serde_json::json!({}))
        .await
        .expect("request succeeds");
    assert_eq!(value["resultType"], "complete");
}

#[tokio::test]
async fn transport_rejects_a_jsonrpc_error_member() {
    let transport = ScriptedTransport::with_error(-32601, "Method not found");
    let error = transport
        .request("tools/list", serde_json::json!({}))
        .await
        .expect_err("must surface the protocol error");
    assert!(format!("{error}").contains("-32601"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-tools-mcp --locked transport`
Expected: FAIL — module `transport` does not exist.

- [ ] **Step 3: Write the trait and the scripted implementation**

The trait's `request` injects `params._meta` on every call so no caller can
forget it — the single most common client bug in this revision.

```rust
/// One MCP request/response round trip.
///
/// Implementations MUST merge `Meta::default()` into `params._meta` before
/// sending. `protocolVersion` and `clientCapabilities` are REQUIRED on EVERY
/// request; a server MUST reject a request missing either with -32602.
pub(crate) trait McpTransport: Send + Sync {
    fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>>;
}

/// Merge required metadata into a params object.
pub(crate) fn with_meta(mut params: serde_json::Value) -> serde_json::Value {
    let meta = serde_json::to_value(Meta::default()).expect("meta serializes");
    if let Some(object) = params.as_object_mut() {
        object.insert("_meta".to_owned(), meta);
    }
    params
}
```

Implement `ScriptedTransport` over a `Mutex<VecDeque<serde_json::Value>>`,
asserting in its own `request` that `params["_meta"]["io.modelcontextprotocol/protocolVersion"]`
equals `PROTOCOL_VERSION`, so every later task's tests prove metadata injection
for free.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p finstack-ai-tools-mcp --locked transport`
Expected: PASS

---

### Task 4: stdio and streamable-HTTP transports

**Files:**
- Modify: `extensions/toolsets/finstack-ai-tools-mcp/src/transport.rs`
- Create: `.../tests/fixtures/tools_list.json`
- Test: `.../src/transport.rs` inline tests

**Interfaces:**
- Consumes: `McpTransport` (Task 3)
- Produces: `pub struct StdioConfig`, `pub struct HttpConfig` — the only
  transport types in the public API

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn stdio_framing_rejects_an_embedded_newline() {
    let error = StdioTransport::encode_line(&serde_json::json!({"a": "b\nc"}))
        .expect_err("embedded newline must be rejected");
    assert!(format!("{error}").contains(MCP_PROTOCOL_VIOLATION));
}

#[test]
fn http_request_carries_the_mandatory_headers() {
    let headers = HttpTransport::headers_for("tools/call", Some("get_weather"));
    assert_eq!(headers.get("MCP-Protocol-Version").unwrap(), "2026-07-28");
    assert_eq!(headers.get("Mcp-Method").unwrap(), "tools/call");
    assert_eq!(headers.get("Mcp-Name").unwrap(), "get_weather");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-tools-mcp --locked stdio_framing http_request`
Expected: FAIL — no `StdioTransport` / `HttpTransport`.

- [ ] **Step 3: Implement both transports**

**stdio.** Spawn the allowlisted program with `.env_clear()`, mirroring
`finstack-ai-tools-shell/src/lib.rs:512`. Framing is newline-delimited JSON, one
complete message per line. There is no handshake — send `tools/list`
immediately. Read stdout line by line; treat stderr as log output only and
**never** as an error signal. Shutdown: close the child's stdin, wait for exit,
then `SIGTERM` → `SIGKILL`.

```rust
impl StdioTransport {
    /// Encode one message as a single newline-terminated line.
    ///
    /// A message MUST NOT contain an embedded newline; serde_json's compact
    /// form escapes newlines inside strings, so any raw `\n` in the output is a
    /// framing bug and is rejected rather than sent.
    pub(crate) fn encode_line(message: &serde_json::Value) -> Result<String, McpError> {
        let encoded = serde_json::to_string(message)
            .map_err(|error| McpError::stable(MCP_PROTOCOL_VIOLATION, &error.to_string()))?;
        if encoded.contains('\n') {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "stdio message contains an embedded newline",
            ));
        }
        Ok(format!("{encoded}\n"))
    }
}
```

**Streamable HTTP.** POST only, one JSON-RPC message per POST, never a batch.
Do **not** send `Mcp-Session-Id`, do not issue `GET` or `DELETE`, and do not
attempt `Last-Event-ID` resumption — all removed in this revision. If a response
stream breaks, the in-flight request is lost and must be re-issued as a **new**
request with a **new** id; never resume. Handle both response content types, and
ignore SSE comment lines (`:`-prefixed keep-alives).

```rust
impl HttpTransport {
    /// Mandatory headers for one POST.
    ///
    /// The server validates header-vs-body agreement and MUST reject a mismatch
    /// with 400 and -32020, so these are derived from the request, never
    /// hardcoded per call site.
    pub(crate) fn headers_for(method: &str, name: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("MCP-Protocol-Version", HeaderValue::from_static(PROTOCOL_VERSION));
        headers.insert("Mcp-Method", HeaderValue::from_str(method).expect("ascii method"));
        if let Some(name) = name {
            headers.insert("Mcp-Name", encode_header_value(name));
        }
        headers.insert(
            "Accept",
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers
    }
}

/// Non-ASCII or unsafe header values use the sentinel encoding
/// `=?base64?{Base64EncodedValue}?=` required by the spec.
fn encode_header_value(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).unwrap_or_else(|_| {
        HeaderValue::from_str(&format!("=?base64?{}?=", base64_encode(value.as_bytes())))
            .expect("sentinel encoding is ascii")
    })
}
```

`Mcp-Name` is REQUIRED for `tools/call`; pass `Some(tool_name)` there and `None`
for `tools/list`.

Add these stable codes to `src/lib.rs` following the shell crate's constant
pattern (`shell/src/lib.rs:31-44`):

```rust
/// Stable protocol-violation code.
pub const MCP_PROTOCOL_VIOLATION: &str = "mcp_protocol_violation";
/// Stable deny-by-default allowlist code.
pub const MCP_SERVER_NOT_ALLOWLISTED: &str = "mcp_server_not_allowlisted";
/// Stable transport-failure code.
pub const MCP_TRANSPORT_ERROR: &str = "mcp_transport_error";
/// Stable catalog-drift code.
pub const MCP_CATALOG_DRIFT: &str = "mcp_catalog_drift";
/// Stable unsupported-result code.
pub const MCP_RESULT_UNSUPPORTED: &str = "mcp_result_unsupported";
/// Stable output-limit code.
pub const MCP_LIMIT_EXCEEDED: &str = "mcp_limit_exceeded";
/// Stable required-artifact-service code.
pub const MCP_ARTIFACT_REQUIRED: &str = "mcp_artifact_required";
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai-tools-mcp --locked`
Expected: PASS

- [ ] **Step 5: Verify supply chain**

Run: `mise run supply-chain`
Expected: PASS — `reqwest` is already in the allowed graph via
`finstack-ai-provider-anthropic`.

---

### Task 5: Catalog enumeration with pagination and a freeze digest

`ToolsetDescriptor` (`crates/finstack-ai-runtime/src/tool.rs:59`) carries only
`name` and `metadata` — **there is no framework-level catalog digest for native
toolsets.** `catalog_digest` exists only on the WASM plugin path
(`plugins/finstack-ai-guest-sdk/src/lib.rs:105`), and the agent lock records
toolsets as `ComponentRef` identity and version only
(`crates/finstack-ai/src/agent.rs:1468`). This crate must therefore implement
its own freeze-and-compare or drift is silently unguarded.

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-mcp/src/classify.rs`
- Modify: `.../src/lib.rs`
- Test: `.../src/tests.rs`

**Interfaces:**
- Consumes: `McpTransport` (Task 3), `protocol::{ListToolsResult, Tool}` (Task 2)
- Produces: `fn enumerate_catalog(&dyn McpTransport) -> Result<Vec<Tool>, McpError>`,
  `fn catalog_digest(&[Tool]) -> Digest`

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn pagination_follows_cursors_including_the_empty_string() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"a","inputSchema":{"type":"object"}}],"nextCursor":""}),
        serde_json::json!({"resultType":"complete","tools":[{"name":"b","inputSchema":{"type":"object"}}]}),
    ]);
    let tools = enumerate_catalog(&transport).await.expect("enumerates");
    assert_eq!(
        tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"],
        "an empty-string cursor must not terminate pagination"
    );
}

#[tokio::test]
async fn catalog_drift_is_detected_by_digest() {
    let first = vec![tool("a"), tool("b")];
    let second = vec![tool("a")];
    assert_ne!(catalog_digest(&first), catalog_digest(&second));
}

#[tokio::test]
async fn network_ref_in_input_schema_is_rejected() {
    let transport = ScriptedTransport::new(vec![serde_json::json!({
        "resultType":"complete",
        "tools":[{"name":"evil","inputSchema":{"type":"object","$ref":"https://attacker.example/schema.json"}}]
    })]);
    let error = enumerate_catalog(&transport)
        .await
        .expect_err("network $ref must be rejected");
    assert!(format!("{error}").contains(MCP_PROTOCOL_VIOLATION));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-tools-mcp --locked catalog`
Expected: FAIL — no `enumerate_catalog`.

- [ ] **Step 3: Implement enumeration and the digest**

Loop `tools/list`, echoing `nextCursor` verbatim as an opaque token. Terminate
**only** when `nextCursor` is absent or null — never on `is_empty()`. Bound the
loop with a page cap so a server returning a cyclic cursor cannot hang
construction. Reject any `inputSchema` containing a `$ref` whose value parses as
an absolute `http`/`https` URI with `MCP_PROTOCOL_VIOLATION`.

`catalog_digest` hashes the canonical JSON of the ordered
`(name, input_schema, output_schema)` triples using
`finstack_ai_kernel::Digest::raw_json`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai-tools-mcp --locked`
Expected: PASS

---

### Task 6: Conservative `ToolSpec` classification

`tool_retry_allowed` (`crates/finstack-ai-runtime/src/tool.rs:213`) permits
same-identity retry only for `RetrySafety::{SafeToRetry, IdempotentWithKey}`
crossed with `SideEffectClass::{ReadOnly, IdempotentWrite}`. MCP carries no
authoritative equivalent — only non-binding hints — so classification must be
conservative at freeze time. **This is the primary fail-closed gate**;
`reconcile()` is the second line, not the first.

**Files:**
- Modify: `extensions/toolsets/finstack-ai-tools-mcp/src/classify.rs`
- Test: `.../src/tests.rs`

**Interfaces:**
- Consumes: `protocol::Tool` (Task 2)
- Produces: `fn to_tool_spec(&Tool, &McpConfig) -> Result<ToolSpec, McpError>`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn annotations_alone_never_grant_retry_safety() {
    let mut t = tool("weather");
    t.annotations = Some(ToolAnnotations {
        read_only_hint: Some(true),
        idempotent_hint: Some(true),
        destructive_hint: Some(false),
    });
    let spec = to_tool_spec(&t, &McpConfig::default()).expect("spec");
    assert_eq!(spec.side_effect, SideEffectClass::NonIdempotentWrite);
    assert_eq!(spec.retry_safety, RetrySafety::NotSafeToRetry);
}

#[test]
fn host_declared_read_only_tool_is_classified_read_only() {
    let config = McpConfig::default().with_read_only_tools(["weather"]);
    let spec = to_tool_spec(&tool("weather"), &config).expect("spec");
    assert_eq!(spec.side_effect, SideEffectClass::ReadOnly);
    assert_eq!(spec.retry_safety, RetrySafety::SafeToRetry);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-tools-mcp --locked classif`
Expected: FAIL — no `to_tool_spec`.

- [ ] **Step 3: Implement classification**

The rule, in one sentence: **server annotations never widen anything.** Default
every tool to `SideEffectClass::NonIdempotentWrite` and
`RetrySafety::NotSafeToRetry`. Only a host-supplied `read_only_tools` or
`idempotent_tools` allowlist in `McpConfig` may narrow a tool to `ReadOnly` /
`SafeToRetry`. Annotations may be recorded in `ToolSpec.metadata` for
observability but must not influence classification.

Set `approval: ApprovalMetadata::required()` for anything not host-declared
read-only, and `max_result_bytes` from `McpConfig::inline_result_bytes`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai-tools-mcp --locked`
Expected: PASS

---

### Task 7: The `Toolset` impl and `ComponentFactory`

**Files:**
- Modify: `extensions/toolsets/finstack-ai-tools-mcp/src/lib.rs`
- Test: `.../src/tests.rs`

**Interfaces:**
- Consumes: everything from Tasks 2–6
- Produces: `pub struct McpToolset`, `pub struct McpToolsetFactory`,
  `pub struct McpConfig`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn call_forwards_the_committed_effect_id_and_returns_the_result() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"echo","inputSchema":{"type":"object"}}]}),
        serde_json::json!({"resultType":"complete","content":[{"type":"text","text":"pong"}],"isError":false}),
    ]);
    let toolset = McpToolset::connect(Arc::new(transport), McpConfig::default())
        .await
        .expect("connects and enumerates");
    assert_eq!(toolset.tools().len(), 1);

    let result = call(&toolset, "echo", serde_json::json!({})).await.expect("call");
    assert!(result.contains("pong"));
}

#[tokio::test]
async fn is_error_result_becomes_a_tool_error() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"boom","inputSchema":{"type":"object"}}]}),
        serde_json::json!({"resultType":"complete","content":[{"type":"text","text":"failed"}],"isError":true}),
    ]);
    let toolset = McpToolset::connect(Arc::new(transport), McpConfig::default())
        .await
        .expect("connects");
    let error = call(&toolset, "boom", serde_json::json!({}))
        .await
        .expect_err("isError must surface as a ToolError");
    assert!(format!("{error}").contains("tool_output_invalid"));
}

#[tokio::test]
async fn input_required_result_is_rejected() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"ask","inputSchema":{"type":"object"}}]}),
        serde_json::json!({"resultType":"input_required","inputRequests":{}}),
    ]);
    let toolset = McpToolset::connect(Arc::new(transport), McpConfig::default())
        .await
        .expect("connects");
    let error = call(&toolset, "ask", serde_json::json!({}))
        .await
        .expect_err("MRTR is not supported");
    assert!(format!("{error}").contains(MCP_RESULT_UNSUPPORTED));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-tools-mcp --locked`
Expected: FAIL — no `McpToolset`.

- [ ] **Step 3: Implement the port**

`McpToolset::connect` enumerates once, freezes `Arc<[ToolSpec]>` and the catalog
digest, and stores them. `Toolset::tools()` returns the frozen slice — data only,
no I/O. `Toolset::call` forwards `ctx.run.effect_id` as the request id so the
server sees a stable idempotency key (`tool.rs:69`, FR-TLS-004), maps `isError:
true` to `ToolError` with the reserved code `tool_output_invalid`, and rejects
any `resultType` other than `complete` with `MCP_RESULT_UNSUPPORTED` — an
unrecognized value is an error, never silently accepted.

`Toolset::reconcile` returns `ToolReconcileResult::NonRepeatable` for any tool
not host-declared read-only, and `Unknown` otherwise. Document plainly in the
README that this forces `SuspendUncertain` on crash recovery for most tools —
that is correct and must be expected, not discovered in production.

Implement `ComponentFactory<dyn Toolset>` (`registry.rs:467`) so construction is
async and happens once at agent build, and stage oversized results through
`ArtifactStore` exactly as `finstack-ai-tools-shell/src/lib.rs:604-684` does,
failing closed with `MCP_ARTIFACT_REQUIRED` when no store is configured.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai-tools-mcp --locked`
Expected: PASS

- [ ] **Step 5: Run the published toolset conformance suite**

Add `finstack-ai-test = { workspace = true }` to `[dev-dependencies]` and:

```rust
#[tokio::test]
async fn toolset_satisfies_the_published_port_conformance_suite() {
    let toolset = connected_toolset().await;
    finstack_ai_test::check_toolset_conformance(&toolset)
        .await
        .expect("published toolset conformance suite");
}
```

Run: `mise run conformance`
Expected: PASS

---

### Task 8: Record delivery evidence

**Files:**
- Modify: `docs/implementation/delivery-ledger.md`
- Modify: `docs/implementation/evidence-register.md`
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: nothing
- Produces: the closing record for this work

- [ ] **Step 1: Add the task ledger row**

Use the 12-column format at `docs/implementation/delivery-ledger.md:382` and the
`PR-NNN-T-short-slug-xxxxxxxxxxxx` id scheme at `:380`, with 12 randomly
generated lowercase hex characters. State the negative scope in column 12
explicitly: *"Protocol client only; no server catalogue, no MRTR, no
subscriptions."*

- [ ] **Step 2: Update the changelog**

Add under `## [Unreleased]` → `### Added` a bullet naming
`finstack-ai-tools-mcp` and its protocol revision.

- [ ] **Step 3: Run the full gate**

Run: `mise run ci`
Expected: PASS

Run: `mise run supply-chain`
Expected: PASS

Run: `mise run docs-links`
Expected: PASS
