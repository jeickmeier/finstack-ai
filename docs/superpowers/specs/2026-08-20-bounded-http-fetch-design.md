# Bounded HTTP Fetch Toolset — Design

Date: 2026-08-20
Status: Approved design, pre-implementation
Related: `extensions/toolsets/finstack-ai-tools-openrouter-media/src/url.rs`
(resolve-and-pin precedent), `extensions/toolsets/finstack-ai-tools-shell`
(deny-by-default policy precedent), `extensions/toolsets/finstack-ai-tools-mcp`
(allowlist error-code precedent),
`docs/planning/06-finstack-ai-security-threat-model.md` (§6 invariants, §7
register, §15 deployment gates, §18 review triggers),
`docs/implementation/artifacts/dep-graph/d1-tls-spike.md` (rustls NO-GO)

## 1. Problem and goals

Research and knowledge agents need to read web pages. Today the only path is
teaching the model to shell out (`curl` through
`finstack-ai-tools-shell`), which forfeits every network control the library
otherwise enforces: no host allowlist, no private-address deny, no response
size cap, and raw HTML burned into the context window. The two media toolsets
each carry a private copy of a vetted-download pipeline
(HTTPS-only-except-loopback, private-IP deny, DNS resolve-and-pin, bounded
body read) that a fetch toolset would need to copy a third time.

Goals:

1. A **`finstack-ai-tools-fetch`** toolset leaf exposing one tool,
   `http_fetch`, that retrieves a page over HTTPS under a deny-by-default
   host allowlist.
2. A shared **`finstack-ai-net-guard`** crate holding the vetted-egress
   primitives (URL vetting, private-IP deny, resolve-and-pin, bounded body
   read) so security-critical logic stops being copy-pasted per toolset.
3. SSRF resistance as a design property: allowlist before parse, vet before
   resolve, pin before connect, re-vet every redirect hop, bound every read.
4. Readable output: HTML converted to markdown by default so research loops
   spend tokens on content, not markup.
5. Fail-closed configuration satisfying threat-model §15: an unconfigured or
   empty allowlist refuses to construct.

Non-goals (v1): JavaScript rendering, HTTP caching, proxy support, cookies
beyond explicitly configured static headers, POST/PUT or any request body,
HEAD requests, robots.txt interpretation, WASM exposure (both crates join
`FORBIDDEN_WASM`, like every reqwest-bearing crate), migrating the media
toolsets or the MCP HTTP transport onto `finstack-ai-net-guard` (named
follow-ups; their behavior must not churn in this change), and any new
`ComponentKind`.

## 2. Placement and architecture

Two new crates:

- `extensions/net/finstack-ai-net-guard` — a new `extensions/net/` area. The
  guard serves toolsets today and plausibly stores/providers later, so it
  does not belong under `toolsets/`. No runtime port; it is a plain library
  crate with a small public API and its own public-api baseline file.
- `extensions/toolsets/finstack-ai-tools-fetch` — the toolset leaf, built on
  the calculator skeleton (lint header, stable error-code consts,
  `validate_call_context`, cached `ToolSpec`) with the shell toolset's
  fail-closed policy constructor.

Dependency direction: `finstack-ai-tools-fetch` → `finstack-ai-net-guard` →
(reqwest, tokio, url). The guard does not depend on runtime or kernel crates;
it returns its own error enum which the toolset maps to `ToolError` codes.
Workspace `reqwest` with `native-tls` (rustls is a settled cargo-deny NO-GO
per the D1 spike); both crates expose the standard
`vendored-tls = ["reqwest/native-tls-vendored"]` feature.

## 3. `finstack-ai-net-guard` API

```rust
/// Scheme/host rules for one outbound request.
pub struct UrlPolicy {
    /// Plaintext HTTP permitted for loopback destinations (test fixtures).
    pub allow_loopback_http: bool,
}

/// Parsed, policy-vetted URL: scheme, host, port, serialized target.
pub struct VettedUrl { /* host, port, is_loopback, url */ }

/// Parse and vet: https only (http iff loopback allowed and host is
/// loopback), forbid userinfo and fragment, extract host and port.
pub fn parse_and_vet_url(value: &str, policy: &UrlPolicy)
    -> Result<VettedUrl, NetGuardError>;

/// Resolve the host and reject any address that is loopback (unless
/// explicitly allowed), private, link-local, unique-local, or an
/// IPv4-mapped form of those (canonicalized first). Every resolved
/// address must pass; one bad address rejects the whole set. Returns the
/// address to pin.
pub async fn resolve_and_pin(vetted: &VettedUrl, resolver: &dyn HostResolver)
    -> Result<SocketAddr, NetGuardError>;

/// System resolver (tokio lookup_host). Tests inject scripted resolvers.
pub struct SystemResolver;

/// Build a reqwest client pinned to the vetted address:
/// `resolve(host, addr)`, `redirect::Policy::none()`, `http1_only`,
/// per-request timeout.
pub fn pinned_client(vetted: &VettedUrl, addr: SocketAddr, timeout: Duration)
    -> Result<reqwest::Client, NetGuardError>;

/// Stream the body up to `cap` bytes; exceeding the cap is an error,
/// never a truncation.
pub async fn read_body_bounded(response: reqwest::Response, cap: usize)
    -> Result<Vec<u8>, NetGuardError>;
```

Design notes:

- The IP-deny predicate generalizes
  `is_forbidden_destination`/`destination_blocked` from the openrouter-media
  crate, including `to_canonical()` so IPv4-mapped IPv6 literals cannot dodge
  the v4 checks.
- `HostResolver` is the injectable seam (the `CommandSandbox` pattern from
  the shell toolset): DNS-rebinding tests script a resolver that answers a
  public address first and a private one second, and assert the pinned
  client never re-resolves.
- Literal-IP URLs skip resolution but still pass the same deny predicate.
- `NetGuardError` variants carry stable non-secret `&'static str` reasons,
  matching house error style; no URL contents appear in error messages
  beyond what the caller already supplied.

## 4. `finstack-ai-tools-fetch` toolset

### 4.1 Tool contract

One tool. Identity: tool id `finstack.tools.http_fetch` (verify against
`ToolId::parse` at implementation; fall back to the calculator's separator
convention if underscores are rejected), model name `http_fetch`, toolset
descriptor name `finstack-fetch`.

Input schema (`deny_unknown_fields`):

```json
{
  "url":       "string (required)",
  "mode":      "auto | text | markdown | artifact (default auto)",
  "max_bytes": "integer (optional; may only lower the configured cap)"
}
```

GET only. No request body parameter exists: a body plus any allowlisted host
is an exfiltration channel, and research reads do not need it.

Output:

```json
{
  "url":         "as requested",
  "final_url":   "after redirects",
  "status":      200,
  "media_type":  "from Content-Type, lowercased essence",
  "byte_length": 12345,
  "content":     "inline text/markdown (exclusive with artifact)",
  "artifact":    { "ArtifactRef …" }
}
```

Mode semantics:

- `auto` (default): `text/html` → markdown extraction; other `text/*`,
  `application/json`, `application/xml`, `+json`/`+xml` suffixes → inline
  text; anything else (or invalid UTF-8) → artifact when a store is
  attached, otherwise a `fetch_limit_exceeded` error whose message tells
  the model the content is binary and requires an artifact store.
- `text`: inline body as UTF-8 text, no markdown conversion.
- `markdown`: force HTML→markdown even for `application/xhtml+xml`.
- `artifact`: always stage bytes to the `ArtifactStore`
  (`stage_required_artifact`, `Sensitivity::Internal`, kind `tool-output`),
  return the reference. Requires an attached store.

No silent truncation anywhere: a body exceeding the effective cap is a
`fetch_limit_exceeded` error (media-toolset precedent).

`ToolSpec` posture: `SideEffectClass::ReadOnly`,
`RetrySafety::SafeToRetry`, `ApprovalRequirement::NotRequired` (the
deployer's allowlist is the gate; host `ToolExecutionPolicy` can still
upgrade any tool to `RequireApproval`), `ToolExecutionMode::Parallel`,
`ToolDeferralSupport::Never`. The `ToolSpec` declares
`max_result_bytes` = configured `max_response_bytes` plus a 4 KiB envelope
margin, and the toolset itself enforces that same ceiling: after
serializing a successful result to JSON, `call()` rejects any output whose
serialized size exceeds it with `fetch_limit_exceeded`, rather than letting
the runtime's generic oversize-result rejection fire. This check exists
because JSON string escaping (`\n` → `\\n`, etc.) can inflate the
serialized `content` field well past the raw byte count already bounded on
read — the raw-byte budget alone does not guarantee the wire-serialized
result stays under the declared ceiling.

### 4.2 Stable error codes

| Code | Category | Meaning |
| --- | --- | --- |
| `fetch_invalid_arguments` | Validation | Malformed arguments or URL |
| `fetch_host_not_allowlisted` | Validation | Host — initial or redirect hop — matches no allowlist entry |
| `fetch_destination_blocked` | Validation | Resolved/literal address is private, loopback, link-local, or unique-local |
| `fetch_redirect_denied` | Tool | Redirect limit exceeded |
| `fetch_transport_failed` | Tool | Connect/TLS/read failure or non-2xx status (message names the status and a bounded reason, `endpoint_rejected` style) |
| `fetch_limit_exceeded` | Limit | Body exceeds the effective byte cap |
| `fetch_timeout` | Deadline | Cancellation, run deadline, or request timeout |

### 4.3 Configuration

```rust
pub struct HttpFetchConfig {
    /// Deny-by-default host allowlist. `try_new` rejects an empty list.
    pub allowlist: Vec<HostPattern>,
    /// Default 2 MiB; hard ceiling 8 MiB.
    pub max_response_bytes: usize,
    /// Default 30 s; hard ceiling 120 s. Also the per-request client timeout.
    pub request_timeout: Duration,
    /// Default 3; hard ceiling 5.
    pub max_redirects: usize,
    /// Static headers attached only when the request host exactly matches
    /// the key. Redacted from Debug. This is the sole cookie/auth door.
    pub per_host_headers: BTreeMap<String, Vec<(String, String)>>,
    /// Plaintext-HTTP loopback fixtures for tests. Default false.
    pub allow_loopback_http: bool,
    /// Optional User-Agent override; default names the crate and repo,
    /// like the media toolsets' DOWNLOAD_USER_AGENT.
    pub user_agent: Option<String>,
}
```

`HostPattern` parses either an exact host (`docs.rs`) or an explicit
subdomain wildcard (`*.wikipedia.org`). A wildcard matches proper subdomains
only — never the bare apex; list the apex separately when wanted. Matching is
ASCII-case-insensitive and operates on the URL parser's ASCII/punycode host
form (the `url` crate applies IDNA); allowlist entries must be ASCII or
punycode — a Unicode entry never matches. Ports are not part of patterns: HTTPS
URLs may use 443 only; loopback-HTTP fixtures may use any port.
`HttpFetchConfig::try_new` clamps nothing — out-of-ceiling values are
construction errors, matching the fail-closed shell precedent.

Constructor surface mirrors the MCP toolset: a typed builder plus a
JSON-snapshot constructor for the bindings.

### 4.4 Request flow

Per call, in order — every step before the socket connect:

1. Cancellation/deadline gate (`ctx.run`), as in the media toolsets.
2. Allowlist match on the URL's host → `fetch_host_not_allowlisted`.
3. `parse_and_vet_url` → `fetch_invalid_arguments` (scheme, userinfo,
   fragment) — HTTPS only unless loopback fixtures are enabled and the host
   is loopback.
4. Literal-host deny check, then `resolve_and_pin` →
   `fetch_destination_blocked`.
5. `pinned_client` (redirects off, HTTP/1.1, pinned address, timeout), GET
   with User-Agent and the host's `per_host_headers` entry, wrapped in the
   `tokio::select!` cancellation/deadline race.
6. 2xx → `read_body_bounded` with the effective cap
   (`min(config, max_bytes)`), then mode handling (§4.1).
7. 3xx → manual redirect: read `Location`, resolve it against the current
   URL, then run the *entire* pipeline again from step 2 — allowlist,
   vetting, fresh resolve, fresh pinned client. Hop count capped by
   `max_redirects` → `fetch_redirect_denied`. `per_host_headers` are looked
   up per hop, so credentials configured for host A never travel to host B
   (threat-model §10 credential-drift control). Non-3xx/non-2xx →
   `fetch_transport_failed` with bounded rejection detail.

### 4.5 HTML → markdown

A dedicated conversion pass in the toolset crate;
`finstack-ai-tools-document` cannot help (anydoc has no HTML format).
Candidate dependency: `htmd` (html5ever-based HTML→markdown). Gate: the
dependency tree must clear `cargo-deny` licenses before it is locked in;
fallback is plain text extraction over `scraper`/`html5ever` (both already
license-cleared ecosystems, verify at implementation). Conversion runs only
on bodies already under the byte cap; output is additionally capped at
`max_result_bytes` minus envelope. Scripts, styles, and comments are
dropped. Conversion failure degrades to inline text, never an error.

## 5. Threat model and governance

- New §7 register row (TM-22, or an extension of TM-16 if the reviewer
  prefers): *SSRF / private-network egress via model-supplied URLs in the
  fetch toolset* — required controls: deny-by-default host allowlist,
  HTTPS-only except loopback fixtures, literal and resolved private/
  loopback/link-local/unique-local address deny with canonicalization,
  DNS resolve-and-pin (rebinding TOCTOU), redirects off in the client and
  re-vetted manually per hop, bounded body reads, per-host-only credential
  headers. Maps to SEC-INV-006 and SEC-INV-007; §15 gate satisfied by the
  empty-allowlist construction failure.
- `docs/planning/` is read-only during normal coding: the register edit is
  a change-control item for the repository owner, listed in the
  implementation plan as an explicit task with the row text prepared.
- §18 review triggers fire (network egress, high-privilege battery). Per
  AGENTS.md this does not require an ADR or stop coding; the plan ends with
  a security-review task producing the standard `security-review.txt`
  artifact.

## 6. Bindings and portability

- Native-only. Both crates are added to `FORBIDDEN_WASM` in
  `scripts/wasm_package/check.py` (openrouter-media precedent). No wasm
  binding surface.
- Python: `PyHttpFetchToolset` constructed from config JSON, registered like
  `PyElicitationToolset`; name added to the Python baseline list.
- JS/WASM binding: no exposure in v1 (wasm-excluded); the server binding
  path gains nothing new.
- Public-api baselines: new
  `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-net-guard.txt`
  and `finstack-ai-tools-fetch.txt`; regenerate via
  `mise run check-public-api` flow.
- Docs: one row each in `docs/site/toolset.md` and
  `extensions/toolsets/README.md` — "Deny-by-default HTTPS allowlist;
  resolve-and-pin; bounded reads" — plus a CHANGELOG entry.

## 7. Testing

- **Guard unit tests**: hostile URL corpus (userinfo, fragments, IPv4/IPv6
  literals, IPv4-mapped IPv6, `localhost` aliases, missing host, scheme
  case games); resolver-injected private-address sets (all-private,
  mixed public/private, empty); rebinding script (public-then-private)
  asserting the pin; bounded-read exact-cap boundary (cap, cap+1).
- **Toolset tests** against loopback-HTTP fixture servers (existing house
  pattern, `allow_loopback_http` enabled): allowlist hit/miss, wildcard
  vs apex, redirect chains (same-host, cross-host allowlisted, cross-host
  denied, limit exceeded, redirect loop), per-host header isolation across
  a cross-host redirect, size cap → error, artifact staging for binary
  bodies, mode matrix, cancellation and deadline races, non-2xx rejection
  detail bounding.
- **HTML conversion**: fixture pages (nested lists, tables, scripts/styles
  stripped, broken markup) with golden markdown outputs; conversion-failure
  fallback to text.
- **Verification task**: `cargo test`, clippy with the standard lint
  header, `mise run check-public-api`, wasm-exclusion check, cargo-deny.

## 8. Open items carried to the implementation plan

1. Verify `ToolId::parse` accepts `finstack.tools.http_fetch`; otherwise
   adopt the hyphenated form.
2. `htmd` license/deny audit; select fallback if it fails.
3. Exact `extensions/net/` workspace-member wiring and whether
   `deny.toml`/`Cargo.toml` metadata conventions need a new area entry.
4. TM-22 row landing: coordinate the `docs/planning/` change-control edit
   with the repository owner.
5. Follow-ups to file (not in scope): migrate openrouter-media and
   openai-media download paths onto `finstack-ai-net-guard`; harden the MCP
   HTTP transport (size cap, redirect policy, IP vetting).
