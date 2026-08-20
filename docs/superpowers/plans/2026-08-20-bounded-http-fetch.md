# Bounded HTTP Fetch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A deny-by-default `http_fetch` tool (crate `finstack-ai-tools-fetch`) built on a shared vetted-egress crate (`finstack-ai-net-guard`) so research agents can read allowlisted HTTPS pages without shelling out to curl.

**Architecture:** `finstack-ai-net-guard` (new `extensions/net/` area) owns the security-critical primitives — URL vetting, private-IP deny, DNS resolve-and-pin, bounded body reads — behind an injectable `HostResolver` seam. `finstack-ai-tools-fetch` is a standard toolset leaf (calculator skeleton, shell-style fail-closed config) that composes those primitives into a GET-only pipeline with manual, re-vetted redirects and HTML→markdown output.

**Tech Stack:** Rust workspace; reqwest 0.13 with **native-tls** (rustls is a settled cargo-deny NO-GO, see `docs/implementation/artifacts/dep-graph/d1-tls-spike.md`); `url` 2.x (already in Cargo.lock via reqwest); tokio; pyo3 for the Python binding.

**Spec:** `docs/superpowers/specs/2026-08-20-bounded-http-fetch-design.md` — read it before starting any task.

## Global Constraints

- Crate lint header on every new `lib.rs`, copied verbatim from `extensions/toolsets/finstack-ai-tools-calculator/src/lib.rs:1-22` (`#![warn(missing_docs)]`, `#![forbid(unsafe_code)]`, `#![deny(clippy::unwrap_used)]`, `#![deny(clippy::expect_used)]`, `#![deny(clippy::panic)]`, `#![deny(clippy::unreachable)]`, plus the `cfg_attr(test, allow(...))` block).
- Both new crates: `version.workspace = true` etc. package header and `[lints] workspace = true`, matching the calculator's `Cargo.toml`; network crate adds `vendored-tls = ["reqwest/native-tls-vendored"]`.
- reqwest from `[workspace.dependencies]` only — never a crate-local version, never rustls features.
- Frozen error codes (spec §4.2): `fetch_invalid_arguments`, `fetch_host_not_allowlisted`, `fetch_destination_blocked`, `fetch_redirect_denied`, `fetch_transport_failed`, `fetch_limit_exceeded`, `fetch_timeout`. Do not rename.
- Config ceilings (spec §4.3): `max_response_bytes` default 2 MiB / ceiling 8 MiB; `request_timeout` default 30 s / ceiling 120 s; `max_redirects` default 3 / ceiling 5. Out-of-range is a construction **error**, never a clamp.
- No silent truncation anywhere: over-cap reads are errors.
- Error messages never echo secrets or unbounded remote content; remote rejection detail is capped (4096-byte read, 300-char message), mirroring `endpoint_rejected` in `extensions/toolsets/finstack-ai-tools-openrouter-media/src/http.rs`.
- Both crates are native-only: added to `FORBIDDEN_WASM` in `scripts/wasm_package/check.py` (Task 11). No wasm binding surface.
- `docs/planning/` is read-only during normal coding — Task 12's threat-register row is a change-control handoff, not a direct edit you land silently.
- Verification commands: `cargo test -p <crate>`, `cargo clippy -p <crate> --all-targets`, `mise run check-public-api`, `cargo deny check licenses` (after any new dependency).
- Commit after every green task; conventional-commit messages as shown in each task.

---

### Task 1: `extensions/net/finstack-ai-net-guard` — crate scaffold and error type

**Files:**
- Create: `extensions/net/finstack-ai-net-guard/Cargo.toml`
- Create: `extensions/net/finstack-ai-net-guard/README.md`
- Create: `extensions/net/finstack-ai-net-guard/src/lib.rs`
- Create: `extensions/net/finstack-ai-net-guard/src/tests.rs`
- Modify: `Cargo.toml` (workspace root — `members` list near line 43, `[workspace.dependencies]` near line 117, and add `url` to workspace deps)

**Interfaces:**
- Produces: `NetGuardError` enum (variants below) — every later net-guard task returns it; Task 7 maps it onto tool error codes.

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "finstack-ai-net-guard"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Vetted outbound HTTP primitives for finstack-ai: URL vetting, private-IP deny, DNS resolve-and-pin, bounded reads"
readme = "README.md"

[features]
default = []
vendored-tls = ["reqwest/native-tls-vendored"]

[dependencies]
futures-util = { workspace = true }
reqwest = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["net"] }
url = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["io-util", "macros", "net", "rt", "rt-multi-thread", "time"] }

[lints]
workspace = true
```

- [ ] **Step 2: Wire the workspace**

In the root `Cargo.toml`:
- Add `"extensions/net/finstack-ai-net-guard",` to `members` (after the `extensions/interop/` entry, keeping the grouped ordering).
- Add to `[workspace.dependencies]`: `finstack-ai-net-guard = { path = "extensions/net/finstack-ai-net-guard", version = "1.0.0" }` (match the exact version used by sibling extension entries at lines 117-124) and `url = "2"` (`url` is already in `Cargo.lock` via reqwest; this only promotes it to a direct dep).

- [ ] **Step 3: Write `src/lib.rs` with the lint header and error enum**

```rust
//! Vetted outbound HTTP primitives: URL vetting, private-address deny,
//! DNS resolve-and-pin, and bounded body reads.

// <calculator lint header block here, verbatim>

use thiserror::Error;

/// Stable invalid-URL code prefix.
pub const NET_GUARD_INVALID_URL: &str = "net_guard_invalid_url";
/// Stable blocked-destination code prefix.
pub const NET_GUARD_DESTINATION_BLOCKED: &str = "net_guard_destination_blocked";

/// Vetted-egress failure. Reasons are stable, non-secret strings.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NetGuardError {
    /// The URL is malformed or violates scheme/component policy.
    #[error("{NET_GUARD_INVALID_URL}: {reason}")]
    InvalidUrl {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// A literal or resolved address is loopback, private, link-local,
    /// or unique-local.
    #[error("{NET_GUARD_DESTINATION_BLOCKED}: {reason}")]
    DestinationBlocked {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// DNS resolution failed or returned no addresses.
    #[error("net_guard_resolution_failed")]
    ResolutionFailed,
    /// The pinned HTTP client could not be constructed.
    #[error("net_guard_client_build_failed")]
    ClientBuildFailed,
    /// The transport failed mid-read.
    #[error("net_guard_transport_failed")]
    TransportFailed,
    /// The body exceeded the caller's byte cap.
    #[error("net_guard_limit_exceeded")]
    LimitExceeded,
}

#[cfg(test)]
mod tests;
```

`src/tests.rs` starts with one display test:

```rust
use super::NetGuardError;

#[test]
fn error_display_names_stable_codes() {
    let error = NetGuardError::InvalidUrl { reason: "scheme_not_allowed" };
    assert_eq!(
        error.to_string(),
        "net_guard_invalid_url: scheme_not_allowed"
    );
}
```

- [ ] **Step 4: Run and verify**

Run: `cargo test -p finstack-ai-net-guard` → PASS; `cargo clippy -p finstack-ai-net-guard --all-targets` → clean.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/net
git commit -m "feat(net): scaffold finstack-ai-net-guard crate"
```

---

### Task 2: net-guard — `UrlPolicy`, `VettedUrl`, `parse_and_vet_url`

**Files:**
- Create: `extensions/net/finstack-ai-net-guard/src/vet.rs`
- Modify: `extensions/net/finstack-ai-net-guard/src/lib.rs` (add `mod vet; pub use vet::{UrlPolicy, VettedUrl, parse_and_vet_url};`)
- Test: `extensions/net/finstack-ai-net-guard/src/tests.rs`

**Interfaces:**
- Consumes: `NetGuardError` (Task 1).
- Produces:
  - `pub struct UrlPolicy { pub allow_loopback_http: bool }`
  - `pub struct VettedUrl { pub url: url::Url, pub host: String, pub port: u16, pub is_loopback: bool }` (host is the parser's ASCII/punycode form; `is_loopback` true only when the host is `localhost`, `127.0.0.0/8`, or `::1` **and** the policy allowed plaintext for it)
  - `pub fn parse_and_vet_url(value: &str, policy: &UrlPolicy) -> Result<VettedUrl, NetGuardError>`

- [ ] **Step 1: Write the failing tests** (hostile corpus; add to `tests.rs`)

```rust
use super::{UrlPolicy, parse_and_vet_url};

fn strict() -> UrlPolicy {
    UrlPolicy { allow_loopback_http: false }
}

#[test]
fn https_url_is_vetted() {
    let vetted = parse_and_vet_url("https://docs.rs/reqwest", &strict()).expect("vet");
    assert_eq!(vetted.host, "docs.rs");
    assert_eq!(vetted.port, 443);
    assert!(!vetted.is_loopback);
}

#[test]
fn plaintext_http_is_rejected_without_loopback_policy() {
    parse_and_vet_url("http://docs.rs/", &strict()).expect_err("http");
    // Loopback host does not help when the policy is strict.
    parse_and_vet_url("http://127.0.0.1:8080/", &strict()).expect_err("http loopback");
}

#[test]
fn loopback_http_is_allowed_only_when_policy_says_so() {
    let policy = UrlPolicy { allow_loopback_http: true };
    let vetted = parse_and_vet_url("http://127.0.0.1:8080/x", &policy).expect("fixture");
    assert!(vetted.is_loopback);
    assert_eq!(vetted.port, 8080);
    // Non-loopback plaintext stays forbidden even under the fixture policy.
    parse_and_vet_url("http://docs.rs/", &policy).expect_err("http public");
}

#[test]
fn forbidden_components_and_schemes_are_rejected() {
    for bad in [
        "ftp://docs.rs/",
        "file:///etc/passwd",
        "https://user:pw@docs.rs/",
        "https://docs.rs/page#frag",
        "https://",
        "docs.rs/no-scheme",
        "https://docs.rs:8443/", // non-443 https port
    ] {
        parse_and_vet_url(bad, &strict()).unwrap_or_else(|_| panic!("{bad} must fail")) /* expect_err */;
    }
}

#[test]
fn unicode_hosts_normalize_to_punycode() {
    let vetted = parse_and_vet_url("https://bücher.example/", &strict()).expect("idna");
    assert_eq!(vetted.host, "xn--bcher-kva.example");
}
```

(Write the loop with `expect_err`, not the panic closure — shown compressed here.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-net-guard` → FAIL: `parse_and_vet_url` not found.

- [ ] **Step 3: Implement `src/vet.rs`**

```rust
//! URL parsing and scheme/component policy.

use std::net::IpAddr;

use url::Url;

use crate::NetGuardError;

/// Scheme/host rules for one outbound request.
#[derive(Debug, Clone, Copy)]
pub struct UrlPolicy {
    /// Plaintext HTTP permitted for loopback destinations (test fixtures).
    pub allow_loopback_http: bool,
}

/// Parsed, policy-vetted URL.
#[derive(Debug, Clone)]
pub struct VettedUrl {
    /// The parsed URL (serialization is what gets sent).
    pub url: Url,
    /// ASCII/punycode host as produced by the parser.
    pub host: String,
    /// Effective port (default 443 for https, scheme default otherwise).
    pub port: u16,
    /// True when the destination is loopback under a permissive policy.
    pub is_loopback: bool,
}

fn invalid(reason: &'static str) -> NetGuardError {
    NetGuardError::InvalidUrl { reason }
}

/// True for `localhost` and literal loopback addresses.
#[must_use]
pub fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|addr| addr.to_canonical().is_loopback())
}

/// Parse and vet: https only (http iff loopback allowed and host is
/// loopback), forbid userinfo and fragment, https port 443 only.
pub fn parse_and_vet_url(value: &str, policy: &UrlPolicy) -> Result<VettedUrl, NetGuardError> {
    let url = Url::parse(value).map_err(|_| invalid("url_malformed"))?;
    let scheme = url.scheme();
    let https = scheme == "https";
    let http = scheme == "http";
    if !https && !http {
        return Err(invalid("scheme_not_allowed"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("userinfo_forbidden"));
    }
    if url.fragment().is_some() {
        return Err(invalid("fragment_forbidden"));
    }
    let host = url.host_str().ok_or_else(|| invalid("host_missing"))?.to_owned();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| invalid("port_missing"))?;
    let loopback = is_loopback_host(&host);
    if http && !(policy.allow_loopback_http && loopback) {
        return Err(invalid("plaintext_http_forbidden"));
    }
    if https && port != 443 {
        return Err(invalid("https_port_forbidden"));
    }
    Ok(VettedUrl {
        url,
        host,
        port,
        is_loopback: http && loopback,
    })
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p finstack-ai-net-guard` → PASS (all Task 2 tests).

- [ ] **Step 5: Commit**

```bash
git add extensions/net/finstack-ai-net-guard
git commit -m "feat(net): URL vetting with https-only and loopback-fixture policy"
```

---

### Task 3: net-guard — private-address deny, `HostResolver`, `resolve_and_pin`

**Files:**
- Create: `extensions/net/finstack-ai-net-guard/src/resolve.rs`
- Modify: `extensions/net/finstack-ai-net-guard/src/lib.rs` (add `mod resolve; pub use resolve::{HostResolver, SystemResolver, is_forbidden_destination, resolve_and_pin};`)
- Test: `extensions/net/finstack-ai-net-guard/src/tests.rs`

**Interfaces:**
- Consumes: `VettedUrl` (Task 2), `NetGuardError` (Task 1).
- Produces:
  - `pub fn is_forbidden_destination(addr: IpAddr) -> bool`
  - `pub trait HostResolver: Send + Sync { fn resolve(&self, host: &str, port: u16) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + '_>>; }`
  - `pub struct SystemResolver;` (implements `HostResolver` via `tokio::net::lookup_host`)
  - `pub async fn resolve_and_pin(vetted: &VettedUrl, resolver: &dyn HostResolver) -> Result<SocketAddr, NetGuardError>`

- [ ] **Step 1: Write the failing tests**

```rust
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use super::{SystemResolver, is_forbidden_destination, resolve_and_pin};

struct ScriptedResolver(Vec<SocketAddr>);

impl super::HostResolver for ScriptedResolver {
    fn resolve(
        &self,
        _host: &str,
        _port: u16,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + '_>> {
        let addrs = self.0.clone();
        Box::pin(async move { Ok(addrs) })
    }
}

#[test]
fn forbidden_destination_covers_private_ranges() {
    for addr in [
        "127.0.0.1", "10.0.0.1", "172.16.0.1", "192.168.1.1", "169.254.1.1",
        "::1", "fd00::1", "fe80::1",
        "::ffff:10.0.0.1", // IPv4-mapped must canonicalize before checking
    ] {
        assert!(is_forbidden_destination(addr.parse::<IpAddr>().unwrap()), "{addr}");
    }
    assert!(!is_forbidden_destination("93.184.216.34".parse::<IpAddr>().unwrap()));
    assert!(!is_forbidden_destination("2606:2800:220:1::1".parse::<IpAddr>().unwrap()));
}

#[tokio::test]
async fn one_private_address_rejects_the_whole_set() {
    let vetted = super::parse_and_vet_url(
        "https://example.com/",
        &super::UrlPolicy { allow_loopback_http: false },
    )
    .unwrap();
    // Public first, private second: the rebinding shape. Must still fail.
    let resolver = ScriptedResolver(vec![
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)), 443),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 443),
    ]);
    resolve_and_pin(&vetted, &resolver).await.expect_err("mixed set");
}

#[tokio::test]
async fn first_public_address_is_pinned() {
    let vetted = super::parse_and_vet_url(
        "https://example.com/",
        &super::UrlPolicy { allow_loopback_http: false },
    )
    .unwrap();
    let a = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)), 443);
    let b = SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0x2606, 0x2800, 0x220, 1, 0, 0, 0, 1)), 443);
    let resolver = ScriptedResolver(vec![a, b]);
    assert_eq!(resolve_and_pin(&vetted, &resolver).await.unwrap(), a);
}

#[tokio::test]
async fn literal_hosts_skip_resolution_but_not_the_deny_check() {
    let policy = super::UrlPolicy { allow_loopback_http: true };
    // Loopback literal allowed under the fixture policy…
    let vetted = super::parse_and_vet_url("http://127.0.0.1:9/", &policy).unwrap();
    let pinned = resolve_and_pin(&vetted, &SystemResolver).await.unwrap();
    assert_eq!(pinned, SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9));
    // …but a private literal is blocked even then.
    let vetted = super::parse_and_vet_url("https://10.0.0.1/", &super::UrlPolicy { allow_loopback_http: false }).unwrap();
    resolve_and_pin(&vetted, &SystemResolver).await.expect_err("private literal");
}

#[tokio::test]
async fn empty_resolution_fails() {
    let vetted = super::parse_and_vet_url(
        "https://example.com/",
        &super::UrlPolicy { allow_loopback_http: false },
    )
    .unwrap();
    resolve_and_pin(&vetted, &ScriptedResolver(Vec::new())).await.expect_err("empty");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-net-guard` → FAIL: `resolve` module not found.

- [ ] **Step 3: Implement `src/resolve.rs`**

```rust
//! Destination vetting and DNS resolve-and-pin.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;

use crate::NetGuardError;
use crate::vet::VettedUrl;

/// Injectable DNS seam. Production uses [`SystemResolver`]; tests script
/// answer sets to exercise rebinding shapes.
pub trait HostResolver: Send + Sync {
    /// Resolve `host:port` to socket addresses.
    fn resolve(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + '_>>;
}

/// System resolver over `tokio::net::lookup_host`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

impl HostResolver for SystemResolver {
    fn resolve(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + '_>> {
        let host = host.to_owned();
        Box::pin(async move {
            Ok(tokio::net::lookup_host((host.as_str(), port)).await?.collect())
        })
    }
}

/// True when the canonicalized address is loopback, private, link-local,
/// or unique-local. IPv4-mapped IPv6 is canonicalized first.
#[must_use]
pub fn is_forbidden_destination(addr: IpAddr) -> bool {
    match addr.to_canonical() {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

fn blocked(addr: IpAddr, allow_loopback: bool) -> bool {
    let addr = addr.to_canonical();
    if allow_loopback && addr.is_loopback() {
        return false;
    }
    is_forbidden_destination(addr)
}

/// Resolve and vet every address; return the address to pin.
///
/// Every resolved address must pass — one bad address rejects the set,
/// so a rebinding resolver cannot smuggle a private answer in later.
///
/// # Errors
///
/// [`NetGuardError::DestinationBlocked`] when any address is forbidden;
/// [`NetGuardError::ResolutionFailed`] on resolver failure or empty sets.
pub async fn resolve_and_pin(
    vetted: &VettedUrl,
    resolver: &dyn HostResolver,
) -> Result<SocketAddr, NetGuardError> {
    let allow_loopback = vetted.is_loopback;
    let bare = vetted.host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        if blocked(ip, allow_loopback) {
            return Err(NetGuardError::DestinationBlocked { reason: "literal_address_forbidden" });
        }
        return Ok(SocketAddr::new(ip, vetted.port));
    }
    let addrs = resolver
        .resolve(&vetted.host, vetted.port)
        .await
        .map_err(|_| NetGuardError::ResolutionFailed)?;
    let mut chosen = None;
    for addr in addrs {
        if blocked(addr.ip(), allow_loopback) {
            return Err(NetGuardError::DestinationBlocked { reason: "resolved_address_forbidden" });
        }
        if chosen.is_none() {
            chosen = Some(addr);
        }
    }
    chosen.ok_or(NetGuardError::ResolutionFailed)
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p finstack-ai-net-guard` → PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/net/finstack-ai-net-guard
git commit -m "feat(net): private-address deny and DNS resolve-and-pin"
```

---

### Task 4: net-guard — `pinned_client` and `read_body_bounded`

**Files:**
- Create: `extensions/net/finstack-ai-net-guard/src/client.rs`
- Modify: `extensions/net/finstack-ai-net-guard/src/lib.rs` (add `mod client; pub use client::{pinned_client, read_body_bounded};`)
- Test: `extensions/net/finstack-ai-net-guard/src/tests.rs`

**Interfaces:**
- Consumes: `VettedUrl` (Task 2), pinned `SocketAddr` (Task 3).
- Produces:
  - `pub fn pinned_client(vetted: &VettedUrl, addr: SocketAddr, timeout: Duration) -> Result<reqwest::Client, NetGuardError>`
  - `pub async fn read_body_bounded(response: reqwest::Response, cap: usize) -> Result<Vec<u8>, NetGuardError>`

- [ ] **Step 1: Write the failing tests**

Use the raw-`TcpListener` fixture pattern from `extensions/toolsets/finstack-ai-tools-openrouter-media/src/lib.rs:646-668` (accept one connection, read the request, write a canned `HTTP/1.1` response with `Connection: close`):

```rust
async fn serve_once(listener: tokio::net::TcpListener, status: u16, headers: &str, body: &[u8]) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut stream, _) = listener.accept().await.expect("accept");
    let mut buf = vec![0_u8; 8_192];
    let _ = stream.read(&mut buf).await.expect("read");
    let mut response = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    stream.write_all(&response).await.expect("write");
    stream.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn pinned_client_fetches_within_the_cap() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_once(listener, 200, "", b"hello"));
    let policy = super::UrlPolicy { allow_loopback_http: true };
    let vetted = super::parse_and_vet_url(&format!("http://127.0.0.1:{}/x", addr.port()), &policy).unwrap();
    let pinned = super::resolve_and_pin(&vetted, &super::SystemResolver).await.unwrap();
    let client = super::pinned_client(&vetted, pinned, std::time::Duration::from_secs(5)).unwrap();
    let response = client.get(vetted.url.as_str()).send().await.unwrap();
    let body = super::read_body_bounded(response, 5).await.unwrap();
    assert_eq!(body, b"hello");
}

#[tokio::test]
async fn body_over_cap_is_an_error_not_a_truncation() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_once(listener, 200, "", b"hello!"));
    let policy = super::UrlPolicy { allow_loopback_http: true };
    let vetted = super::parse_and_vet_url(&format!("http://127.0.0.1:{}/x", addr.port()), &policy).unwrap();
    let pinned = super::resolve_and_pin(&vetted, &super::SystemResolver).await.unwrap();
    let client = super::pinned_client(&vetted, pinned, std::time::Duration::from_secs(5)).unwrap();
    let response = client.get(vetted.url.as_str()).send().await.unwrap();
    assert_eq!(
        super::read_body_bounded(response, 5).await.unwrap_err(),
        super::NetGuardError::LimitExceeded
    );
}

#[tokio::test]
async fn pinned_client_does_not_follow_redirects() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_once(listener, 302, "Location: https://docs.rs/\r\n", b""));
    let policy = super::UrlPolicy { allow_loopback_http: true };
    let vetted = super::parse_and_vet_url(&format!("http://127.0.0.1:{}/x", addr.port()), &policy).unwrap();
    let pinned = super::resolve_and_pin(&vetted, &super::SystemResolver).await.unwrap();
    let client = super::pinned_client(&vetted, pinned, std::time::Duration::from_secs(5)).unwrap();
    let response = client.get(vetted.url.as_str()).send().await.unwrap();
    assert_eq!(response.status(), 302); // surfaced, not followed
}
```

- [ ] **Step 2: Run to verify failure** → FAIL: `client` module not found.

- [ ] **Step 3: Implement `src/client.rs`**

```rust
//! Address-pinned client construction and bounded body reads.

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::StreamExt;

use crate::NetGuardError;
use crate::vet::VettedUrl;

/// Build a reqwest client pinned to the vetted address. Redirects are
/// disabled; callers follow them manually so every hop is re-vetted.
pub fn pinned_client(
    vetted: &VettedUrl,
    addr: SocketAddr,
    timeout: Duration,
) -> Result<reqwest::Client, NetGuardError> {
    reqwest::Client::builder()
        .http1_only()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&vetted.host, addr)
        .build()
        .map_err(|_| NetGuardError::ClientBuildFailed)
}

/// Stream the body up to `cap` bytes; exceeding the cap is an error,
/// never a truncation.
pub async fn read_body_bounded(
    response: reqwest::Response,
    cap: usize,
) -> Result<Vec<u8>, NetGuardError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| NetGuardError::TransportFailed)?;
        if body.len().saturating_add(chunk.len()) > cap {
            return Err(NetGuardError::LimitExceeded);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
```

Note: `reqwest::Client::resolve` takes the hostname without brackets — pass `vetted.host` as stored (the `url` crate returns IPv6 hosts bracket-free from `host_str()`; verify with a bracketed-literal test if in doubt).

- [ ] **Step 4: Run to verify pass** → `cargo test -p finstack-ai-net-guard` PASS; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add extensions/net/finstack-ai-net-guard
git commit -m "feat(net): pinned client and bounded body reads"
```

---

### Task 5: `extensions/toolsets/finstack-ai-tools-fetch` — scaffold, `HostPattern`, `HttpFetchConfig`

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-fetch/Cargo.toml`
- Create: `extensions/toolsets/finstack-ai-tools-fetch/README.md`
- Create: `extensions/toolsets/finstack-ai-tools-fetch/src/lib.rs`
- Create: `extensions/toolsets/finstack-ai-tools-fetch/src/config.rs`
- Create: `extensions/toolsets/finstack-ai-tools-fetch/src/tests.rs`
- Modify: `Cargo.toml` (workspace root: member + `finstack-ai-tools-fetch` workspace dep entry)

**Interfaces:**
- Consumes: nothing from net-guard yet (config is standalone).
- Produces:
  - `pub struct HostPattern` with `pub fn parse(entry: &str) -> Result<Self, HttpFetchError>` and `pub fn matches(&self, host: &str) -> bool`
  - `pub struct HttpFetchConfig { pub allowlist: Vec<String>, pub max_response_bytes: usize, pub request_timeout: Duration, pub max_redirects: usize, pub per_host_headers: BTreeMap<String, Vec<(String, String)>>, pub allow_loopback_http: bool, pub user_agent: Option<String> }` plus `impl Default` (empty allowlist — unusable until filled; defaults 2 MiB / 30 s / 3)
  - `pub enum HttpFetchError { Configuration { reason: &'static str } }` (thiserror, calculator style)
  - `pub struct HttpFetchToolset` with `pub fn try_new(config: HttpFetchConfig) -> Result<Self, HttpFetchError>` — Task 6 fills the Toolset impl; this task validates config and stores parsed patterns.
  - Frozen constants: `FETCH_INVALID_ARGUMENTS`, `FETCH_HOST_NOT_ALLOWLISTED`, `FETCH_DESTINATION_BLOCKED`, `FETCH_REDIRECT_DENIED`, `FETCH_TRANSPORT_FAILED`, `FETCH_LIMIT_EXCEEDED`, `FETCH_TIMEOUT` (`pub const … : &str` with the spec §4.2 values).

**Cargo.toml** — copy the openrouter-media shape (Task 1 header conventions), deps: `finstack-ai-kernel`, `finstack-ai-runtime` (default-features off, `native-tokio`), `finstack-ai-net-guard`, `futures-util`, `reqwest`, `serde`, `serde_json`, `thiserror`, `tokio` (`net`, `time`), `url`; dev-deps: `finstack-ai-context-memory` (for `InProcessArtifactStore`), tokio full test features; `vendored-tls` feature forwarding.

- [ ] **Step 1: Write the failing tests**

```rust
use std::time::Duration;

use super::{HostPattern, HttpFetchConfig, HttpFetchToolset};

const HEADER_CANARY: &str = "fetch-secret-canary-091";

fn config_with(hosts: &[&str]) -> HttpFetchConfig {
    HttpFetchConfig {
        allowlist: hosts.iter().map(|h| (*h).to_owned()).collect(),
        ..HttpFetchConfig::default()
    }
}

#[test]
fn empty_allowlist_refuses_to_construct() {
    HttpFetchToolset::try_new(config_with(&[])).expect_err("deny by default");
}

#[test]
fn out_of_ceiling_values_are_errors_not_clamps() {
    for config in [
        HttpFetchConfig { max_response_bytes: 9 * 1_048_576, ..config_with(&["docs.rs"]) },
        HttpFetchConfig { max_response_bytes: 0, ..config_with(&["docs.rs"]) },
        HttpFetchConfig { request_timeout: Duration::from_secs(121), ..config_with(&["docs.rs"]) },
        HttpFetchConfig { request_timeout: Duration::ZERO, ..config_with(&["docs.rs"]) },
        HttpFetchConfig { max_redirects: 6, ..config_with(&["docs.rs"]) },
    ] {
        HttpFetchToolset::try_new(config).expect_err("ceiling");
    }
}

#[test]
fn host_patterns_match_exact_and_wildcard() {
    let exact = HostPattern::parse("docs.rs").unwrap();
    assert!(exact.matches("docs.rs"));
    assert!(exact.matches("DOCS.RS"));
    assert!(!exact.matches("sub.docs.rs"));
    let wild = HostPattern::parse("*.wikipedia.org").unwrap();
    assert!(wild.matches("en.wikipedia.org"));
    assert!(wild.matches("a.b.wikipedia.org"));
    assert!(!wild.matches("wikipedia.org")); // never the bare apex
    assert!(!wild.matches("evilwikipedia.org"));
    for bad in ["", "*", "*.", "*.*.x", "docs.rs/path", "https://docs.rs", "doc s.rs"] {
        HostPattern::parse(bad).expect_err(bad);
    }
}

#[test]
fn debug_redacts_per_host_headers() {
    let mut config = config_with(&["docs.rs"]);
    config.per_host_headers.insert(
        "docs.rs".to_owned(),
        vec![("Cookie".to_owned(), HEADER_CANARY.to_owned())],
    );
    assert!(!format!("{config:?}").contains(HEADER_CANARY));
    let toolset = HttpFetchToolset::try_new(config).unwrap();
    assert!(!format!("{toolset:?}").contains(HEADER_CANARY));
}
```

- [ ] **Step 2: Run to verify failure** → FAIL: crate compiles but symbols missing (or crate absent).

- [ ] **Step 3: Implement**

`src/config.rs` essentials:

```rust
pub(crate) const MAX_RESPONSE_BYTES_CEILING: usize = 8 * 1_048_576;
pub(crate) const REQUEST_TIMEOUT_CEILING: Duration = Duration::from_secs(120);
pub(crate) const MAX_REDIRECTS_CEILING: usize = 5;

/// One allowlist entry: exact host or `*.suffix` subdomain wildcard.
#[derive(Clone, PartialEq, Eq)]
pub struct HostPattern(PatternKind);

#[derive(Clone, PartialEq, Eq)]
enum PatternKind {
    Exact(String),          // stored lowercase
    SubdomainOf(String),    // stored lowercase, no leading dot
}

impl HostPattern {
    pub fn parse(entry: &str) -> Result<Self, HttpFetchError> {
        let entry = entry.to_ascii_lowercase();
        let (wildcard, host) = match entry.strip_prefix("*.") {
            Some(rest) => (true, rest),
            None => (false, entry.as_str()),
        };
        let valid = !host.is_empty()
            && !host.contains(['*', '/', ':', '?', '#', '@', ' '])
            && host.split('.').all(|label| !label.is_empty());
        if !valid {
            return Err(HttpFetchError::Configuration { reason: "invalid_allowlist_entry" });
        }
        Ok(Self(if wildcard {
            PatternKind::SubdomainOf(host.to_owned())
        } else {
            PatternKind::Exact(host.to_owned())
        }))
    }

    #[must_use]
    pub fn matches(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        match &self.0 {
            PatternKind::Exact(exact) => host == *exact,
            PatternKind::SubdomainOf(suffix) => host
                .strip_suffix(suffix.as_str())
                .and_then(|prefix| prefix.strip_suffix('.'))
                .is_some_and(|rest| !rest.is_empty()),
        }
    }
}
```

`HttpFetchConfig` derives `Clone` and hand-implements `Debug` (redact `per_host_headers` values as `"<redacted>"`). `HttpFetchToolset::try_new` parses every allowlist entry into `Vec<HostPattern>`, checks the three ceilings and non-zero floors, validates each `per_host_headers` key parses as a `HostPattern::parse` exact entry and each header name/value with `reqwest::header::HeaderName::from_bytes` / `HeaderValue::from_str`, and stores everything; `Debug` for the toolset also redacts. `lib.rs` gets the calculator lint header, the frozen `pub const` error-code strings, and `mod config; mod tests;` wiring plus re-exports listed in **Produces**.

- [ ] **Step 4: Run to verify pass** → `cargo test -p finstack-ai-tools-fetch` PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/toolsets/finstack-ai-tools-fetch
git commit -m "feat(fetch): fail-closed HttpFetchConfig and host allowlist patterns"
```

---

### Task 6: tools-fetch — `ToolSpec`, `Toolset` impl skeleton, argument parsing

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-fetch/src/toolset.rs`
- Modify: `extensions/toolsets/finstack-ai-tools-fetch/src/lib.rs`
- Test: `extensions/toolsets/finstack-ai-tools-fetch/src/tests.rs`

**Interfaces:**
- Consumes: `HttpFetchToolset` internals (Task 5).
- Produces:
  - `impl Toolset for HttpFetchToolset` (`descriptor()` name `finstack-fetch`; `tools()` one cached `ToolSpec`; `call()` dispatching to `execute_fetch` — a stub returning `fetch_transport_failed` until Task 7).
  - `pub(crate) struct FetchArguments { url: String, mode: FetchMode, max_bytes: Option<usize> }` with `#[serde(deny_unknown_fields)]`; `pub(crate) enum FetchMode { Auto, Text, Markdown, Artifact }` (`rename_all = "snake_case"`, default `Auto`).
  - `pub fn with_artifact_store(self, store: Arc<dyn ArtifactStore>) -> Self` builder hook (media-toolset pattern) used by Task 9.

Mirror `extensions/toolsets/finstack-ai-tools-calculator/src/lib.rs:145-298` exactly for: `try_new`-built cached `ToolSpec`, `validate_call_context` (identity + tenant-scope check, reworded messages with `fetch_invalid_arguments`), the `call()` shape returning a one-item `ToolStreamItem::Completed` stream, and `tool_error()`. Spec values (design §4.1): tool id `finstack.tools.http_fetch` (**first verify `ToolId::parse` accepts underscores** — open item 1; if not, use `finstack.tools.http-fetch` and record the choice in the crate docs), model name `http_fetch`, `SideEffectClass::ReadOnly`, `RetrySafety::SafeToRetry`, `ApprovalRequirement::NotRequired`, `ToolExecutionMode::Parallel`, `ToolDeferralSupport::Never`, `max_result_bytes = config.max_response_bytes + 4096`.

Input schema (inline `RawJson::parse` like the calculator):

```json
{"additionalProperties":false,"properties":{"url":{"type":"string"},"mode":{"enum":["auto","text","markdown","artifact"],"type":"string"},"max_bytes":{"minimum":1,"type":"integer"}},"required":["url"],"type":"object"}
```

- [ ] **Step 1: Write the failing tests** — copy the `tool_context()` and `call_for()` helpers **verbatim** from `extensions/toolsets/finstack-ai-tools-openrouter-media/src/lib.rs:583-636` into `tests.rs` (they build a `ToolCallContext` and `ValidatedToolCall` from kernel/runtime test constructors). Then:

```rust
#[test]
fn tool_spec_is_valid_and_read_only() {
    let toolset = HttpFetchToolset::try_new(config_with(&["docs.rs"])).unwrap();
    let tools = toolset.tools();
    assert_eq!(tools.len(), 1);
    let spec = &tools[0];
    assert_eq!(spec.model_name.as_ref(), "http_fetch");
    assert!(spec.validate().is_ok());
}

#[tokio::test]
async fn unknown_argument_fields_are_rejected() {
    let toolset = HttpFetchToolset::try_new(config_with(&["docs.rs"])).unwrap();
    let spec = toolset.tools()[0].clone();
    let call = call_for(&spec, br#"{"url":"https://docs.rs/","surprise":1}"#);
    let error = drive_to_error(&toolset, call).await; // helper: poll the stream, expect Err
    assert_eq!(error.code().as_ref(), super::FETCH_INVALID_ARGUMENTS);
}
```

(`drive_to_error`: call `toolset.call(tool_context(), call).await` and unwrap the `Err`, or collect the stream if the error is deferred to the stream — match how the calculator tests in `extensions/toolsets/finstack-ai-tools-calculator/src/tests.rs` assert errors, and reuse their assertion shape.)

- [ ] **Step 2: Run to verify failure** → FAIL: no `Toolset` impl.
- [ ] **Step 3: Implement** as described in Interfaces; `execute_fetch` stub returns `Err(tool_error(FETCH_TRANSPORT_FAILED, ErrorCategory::Tool, "http fetch is not wired yet"))`.
- [ ] **Step 4: Run to verify pass** → PASS.
- [ ] **Step 5: Commit**

```bash
git add extensions/toolsets/finstack-ai-tools-fetch
git commit -m "feat(fetch): http_fetch ToolSpec and Toolset skeleton"
```

---

### Task 7: tools-fetch — request pipeline (allowlist → vet → pin → GET → bounded read)

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-fetch/src/pipeline.rs`
- Modify: `extensions/toolsets/finstack-ai-tools-fetch/src/toolset.rs` (`execute_fetch` calls the pipeline)
- Test: `extensions/toolsets/finstack-ai-tools-fetch/src/tests.rs`

**Interfaces:**
- Consumes: net-guard `parse_and_vet_url`, `resolve_and_pin`, `pinned_client`, `read_body_bounded`, `SystemResolver`, `HostResolver`; config from Task 5; `FetchArguments` from Task 6.
- Produces: `pub(crate) async fn execute_fetch(state: &FetchState, ctx: &ToolCallContext, args: FetchArguments) -> Result<serde_json::Value, ToolError>` where `FetchState` bundles parsed patterns, config, `Arc<dyn HostResolver>`, and (Task 9) the optional artifact store. `FetchState` holds `resolver: Arc<dyn HostResolver>` defaulting to `SystemResolver` with a `#[cfg(test)] pub(crate) fn with_resolver(...)` hook.

Pipeline order per spec §4.4 (each numbered step maps to an error code):
1. Cancellation/deadline gate — copy `deadline_elapsed`/`wait_deadline`/`timeout_error` from `extensions/toolsets/finstack-ai-tools-openrouter-media/src/http.rs:286-321`, substituting `FETCH_TIMEOUT`.
2. Allowlist match on the **parsed** URL's host (parse first via net-guard, then match patterns against `vetted.host`) → `FETCH_HOST_NOT_ALLOWLISTED`. Loopback fixture hosts (`vetted.is_loopback`) bypass the allowlist **only** when `allow_loopback_http` is set — document this in the config docs; it is what lets tests run without allowlisting `127.0.0.1`.
3. Vet errors map: `InvalidUrl → FETCH_INVALID_ARGUMENTS`, `DestinationBlocked → FETCH_DESTINATION_BLOCKED`, `ResolutionFailed → FETCH_TRANSPORT_FAILED`.
4. `resolve_and_pin`, `pinned_client(vetted, addr, config.request_timeout)`.
5. GET with User-Agent (default `finstack-ai-tools-fetch/<CARGO_PKG_VERSION> (+https://github.com/jeickmeier/finstack-ai)`, or config override) plus the host's `per_host_headers` entry (exact-host lookup on `vetted.host`); wrap `send()` in the `tokio::select!` cancellation/deadline race (openrouter-media `download_bytes` shape, `src/url.rs:188-221`).
6. Non-2xx, non-3xx → `FETCH_TRANSPORT_FAILED` with bounded rejection detail (port `endpoint_rejected`/`rejection_detail` from openrouter-media `http.rs:330-395`, dropping the OpenRouter-specific JSON unwrapping — surface the first 300 chars of the trimmed body).
7. 2xx → `read_body_bounded` with `effective_cap = min(config.max_response_bytes, args.max_bytes.unwrap_or(usize::MAX))`; `LimitExceeded → FETCH_LIMIT_EXCEEDED`, `TransportFailed → FETCH_TRANSPORT_FAILED`.
8. Success (this task): return inline text JSON `{url, final_url, status, media_type, byte_length, content}` — treat every body as UTF-8 text for now (`String::from_utf8_lossy`); Task 9 adds content-type routing and Task 10 markdown. 3xx handling is Task 8; until then a 3xx returns `FETCH_REDIRECT_DENIED`.

- [ ] **Step 1: Write the failing tests** (reuse `serve_once` from Task 4, copied into this crate's `tests.rs`; enable `allow_loopback_http: true` in the fixture config):

```rust
#[tokio::test]
async fn fetch_returns_inline_text() { /* fixture serves 200 "hello"; assert output JSON fields */ }

#[tokio::test]
async fn non_allowlisted_host_is_denied_before_any_connection() {
    // No fixture server at all: a denied host must fail without I/O.
    // config allowlist ["docs.rs"], url "https://example.com/" → FETCH_HOST_NOT_ALLOWLISTED
}

#[tokio::test]
async fn private_destination_is_blocked() {
    // allowlist ["internal.example"], scripted resolver answering 10.0.0.1
    // → FETCH_DESTINATION_BLOCKED
}

#[tokio::test]
async fn oversize_body_is_a_limit_error() { /* fixture serves > max_bytes → FETCH_LIMIT_EXCEEDED */ }

#[tokio::test]
async fn non_success_status_reports_bounded_detail() {
    /* fixture serves 503 with a 10 KiB body; error message contains "503",
       is shorter than 500 chars, and does not contain the body's tail marker */
}

#[tokio::test]
async fn cancelled_or_deadline_expired_calls_return_fetch_timeout() {
    /* pre-cancel ctx.run.cancellation before calling execute_fetch →
       FETCH_TIMEOUT without any connection; second case: deadline in the
       past (Timestamp at unix ms 1) → FETCH_TIMEOUT. Mirror the
       cancellation assertions in the openrouter-media tests. */
}

#[tokio::test]
async fn per_host_headers_are_sent_to_the_matching_host() {
    /* fixture asserts the received request contains "X-Api: canary" when
       per_host_headers maps the fixture host; a second fixture without an
       entry must not receive it (capture requests via the mpsc channel
       pattern from openrouter-media respond()) */
}
```

- [ ] **Step 2: Run to verify failure** → new tests FAIL against the Task 6 stub.
- [ ] **Step 3: Implement `pipeline.rs`** per the numbered order above.
- [ ] **Step 4: Run to verify pass** → `cargo test -p finstack-ai-tools-fetch` PASS.
- [ ] **Step 5: Commit**

```bash
git add extensions/toolsets/finstack-ai-tools-fetch
git commit -m "feat(fetch): vetted GET pipeline with allowlist and bounded reads"
```

---

### Task 8: tools-fetch — manual redirects with full re-vetting

**Files:**
- Modify: `extensions/toolsets/finstack-ai-tools-fetch/src/pipeline.rs`
- Test: `extensions/toolsets/finstack-ai-tools-fetch/src/tests.rs`

**Interfaces:**
- Consumes: Task 7 pipeline.
- Produces: redirect loop inside `execute_fetch`; output gains accurate `final_url`.

Behavior (spec §4.4 step 7): statuses 301/302/303/307/308 read the `Location` header, resolve it against the current URL with `url::Url::join` (handles relative Locations), then re-enter the **entire** pipeline from the allowlist step — new vet, new resolve, new pinned client. Hop count capped by `config.max_redirects` → `FETCH_REDIRECT_DENIED`; missing/invalid `Location` → `FETCH_INVALID_ARGUMENTS`? No — the remote sent it: `FETCH_TRANSPORT_FAILED` with reason "redirect location invalid". A hop to a non-allowlisted host → `FETCH_HOST_NOT_ALLOWLISTED`. `per_host_headers` are looked up per hop's host, so credentials never travel across hosts.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn same_host_redirect_is_followed() { /* fixture A: 302 → /b, then 200; assert final_url ends in /b */ }

#[tokio::test]
async fn cross_host_redirect_to_unlisted_host_is_denied() { /* fixture A 302 → https://evil.example/ → FETCH_HOST_NOT_ALLOWLISTED, no connection attempt to evil.example */ }

#[tokio::test]
async fn redirect_limit_is_enforced() { /* fixture loops /a → /a with max_redirects 3 → FETCH_REDIRECT_DENIED after 3 hops (4 requests total) */ }

#[tokio::test]
async fn headers_do_not_cross_hosts_on_redirect() {
    /* two loopback fixtures on different ports; per_host_headers keyed to
       fixture A's host:port form; A 302s to B; assert B's captured request
       lacks the header. NOTE: loopback fixtures share the host "127.0.0.1",
       so key the header by a distinct header value per fixture instead —
       assert B never sees A's value. */
}
```

(For the cross-host loopback limitation: since both fixtures are `127.0.0.1`, exercise the *header isolation* with same-host-different-value setups, and exercise the *allowlist hop denial* with a Location naming a public DNS host that is not allowlisted — the deny fires before any connection, so no server is needed.)

- [ ] **Step 2: Run to verify failure** → redirect tests FAIL (`FETCH_REDIRECT_DENIED` from the Task 7 placeholder).
- [ ] **Step 3: Implement the loop** (a `for hop in 0..=max_redirects` around the Task 7 body; carry `current: VettedUrl`).
- [ ] **Step 4: Run to verify pass** → PASS.
- [ ] **Step 5: Commit**

```bash
git add extensions/toolsets/finstack-ai-tools-fetch
git commit -m "feat(fetch): manual redirects with per-hop re-vetting"
```

---

### Task 9: tools-fetch — mode handling and artifact staging

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-fetch/src/deliver.rs`
- Modify: `extensions/toolsets/finstack-ai-tools-fetch/src/pipeline.rs`
- Test: `extensions/toolsets/finstack-ai-tools-fetch/src/tests.rs`

**Interfaces:**
- Consumes: pipeline body bytes + `Content-Type` (Task 7/8), `with_artifact_store` hook (Task 6).
- Produces: `pub(crate) async fn deliver(body: Vec<u8>, media_type: &str, mode: FetchMode, store: Option<&Arc<dyn ArtifactStore>>, ctx: &ToolCallContext) -> Result<DeliveredContent, ToolError>` where `DeliveredContent` is `Inline(String)` or `Artifact(serde_json::Value)`; pipeline merges it into the output JSON (`content` xor `artifact`).

Routing (spec §4.1), keyed on the lowercased media-type essence (strip parameters after `;`):
- inline-text set: `text/*`, `application/json`, `application/xml`, essence ending `+json` or `+xml`;
- `text/html` and `application/xhtml+xml` in `Auto`/`Markdown` → markdown conversion (Task 10 — until then, inline text);
- everything else, or invalid UTF-8: `Artifact` when a store is attached (port the `stage_required_artifact` call from openrouter-media `http.rs:110-133` — scope `Sensitivity::Internal`, kind `tool-output`, name `http-fetch-body`, `media_type` from the response); otherwise `FETCH_LIMIT_EXCEEDED` with message "content is binary and requires an artifact store";
- `Artifact` mode without a store: same error;
- `Text` mode on any body: `String::from_utf8_lossy` inline.

- [ ] **Step 1: Write the failing tests** — binary body (`[0u8,159,146,150]`, media type `application/octet-stream`) with `InProcessArtifactStore` attached → output has `artifact` and no `content`; same without store → `FETCH_LIMIT_EXCEEDED`; `mode:"artifact"` on a text body stages it; JSON body inlines; `max_bytes` argument lower than config cap is honored (fixture body sized between the two → `FETCH_LIMIT_EXCEEDED`).
- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement `deliver.rs`.**
- [ ] **Step 4: Run to verify pass.**
- [ ] **Step 5: Commit**

```bash
git add extensions/toolsets/finstack-ai-tools-fetch
git commit -m "feat(fetch): content routing and artifact staging"
```

---

### Task 10: tools-fetch — HTML → markdown

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-fetch/src/markdown.rs`
- Create: `extensions/toolsets/finstack-ai-tools-fetch/fixtures/` (HTML inputs + golden `.md` outputs)
- Modify: `extensions/toolsets/finstack-ai-tools-fetch/src/deliver.rs`, `Cargo.toml` (crate + workspace dep for the converter)
- Test: `extensions/toolsets/finstack-ai-tools-fetch/src/tests.rs`

**Interfaces:**
- Consumes: `deliver` routing (Task 9).
- Produces: `pub(crate) fn html_to_markdown(html: &str, output_cap: usize) -> Option<String>` — `None` on conversion failure (caller falls back to inline text, never errors); output truncation **is** allowed here only via refusing (`None`) when the converted markdown exceeds `output_cap` (`max_result_bytes − 4096`), falling back to text which is already under the byte cap.

- [ ] **Step 1: License gate (do this before writing code).** Add `htmd = "0.2"` (check latest) to `[workspace.dependencies]` and the crate, run `cargo deny check licenses`. If it fails, remove it and use `scraper` (html5ever ecosystem) to walk the DOM and emit headings/paragraphs/lists/links as minimal markdown by hand. Record the outcome in the crate README. (Spec open item 2.)
- [ ] **Step 2: Write the failing tests** — fixture HTML with nested lists, a table, `<script>`/`<style>` blocks, and broken markup; assert golden markdown, scripts/styles absent, and the fallback path (`html_to_markdown` returning `None` on pathological input → inline text delivery).
- [ ] **Step 3: Implement `markdown.rs`** and wire the `Auto`/`Markdown` branches in `deliver.rs`.
- [ ] **Step 4: Run to verify pass** → `cargo test -p finstack-ai-tools-fetch` PASS; `cargo deny check licenses` PASS.
- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/toolsets/finstack-ai-tools-fetch
git commit -m "feat(fetch): HTML-to-markdown extraction with text fallback"
```

---

### Task 11: Python binding — `PyHttpFetchToolset`

**Files:**
- Create: `bindings/finstack-ai-python/src/fetch.rs`
- Modify: `bindings/finstack-ai-python/src/lib.rs` (module registration, near the `PyElicitationToolset` registration at line ~130)
- Modify: `bindings/finstack-ai-python/Cargo.toml` (dep on `finstack-ai-tools-fetch`)
- Modify: `bindings/finstack-ai-python/python/finstack_ai/__init__.py` (export the class)
- Test: the binding crate's existing Python-side test convention (find the elicitation-toolset tests and mirror them)

**Interfaces:**
- Consumes: `HttpFetchToolset::try_new`, `HttpFetchConfig` (Task 5), `with_artifact_store` (Task 6).
- Produces: `PyHttpFetchToolset` constructible from a config-JSON string (`{"allowlist": [...], "max_response_bytes": ..., "request_timeout_ms": ..., "max_redirects": ..., "per_host_headers": {...}, "user_agent": ...}` — `allow_loopback_http` **is** exposed for test parity but documented as fixtures-only), registrable wherever `PyElicitationToolset` is.

- [ ] **Step 1: Read `bindings/finstack-ai-python/src/elicitation.rs` end-to-end** and copy its structure: pyo3 class shape, error mapping, how the inner `Arc<dyn Toolset>` is handed to agent/registrar plumbing, and where its tests live.
- [ ] **Step 2: Write the failing test** (Python side): construct from JSON with an allowlist → ok; empty allowlist → raises; assert the class is exported from `finstack_ai`.
- [ ] **Step 3: Implement `fetch.rs`** following the elicitation template; add a serde `HttpFetchConfigSnapshot` in the tools-fetch crate (`pub fn from_json(bytes: &[u8]) -> Result<HttpFetchConfig, HttpFetchError>`, MCP-toolset precedent) so the binding stays thin.
- [ ] **Step 4: Run the binding test suite** the way `mise` tasks do for the Python package (see `mise.toml` python test tasks) → PASS.
- [ ] **Step 5: Commit**

```bash
git add bindings/finstack-ai-python extensions/toolsets/finstack-ai-tools-fetch
git commit -m "feat(python): PyHttpFetchToolset binding"
```

---

### Task 12: Portability, baselines, docs, changelog

**Files:**
- Modify: `scripts/wasm_package/check.py` (add `finstack-ai-net-guard` and `finstack-ai-tools-fetch` to `FORBIDDEN_WASM`, alphabetical position)
- Create: `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-net-guard.txt` and `finstack-ai-tools-fetch.txt` (generated, not hand-written)
- Modify: `fixtures/compatibility/breaking/python/` name list (regenerated)
- Modify: `docs/site/toolset.md` (one row; respect the ~100-line NFR-DX-002 registration budget framing), `extensions/toolsets/README.md` (row: `finstack-ai-tools-fetch` | Deny-by-default HTTPS allowlist; resolve-and-pin; bounded reads), `CHANGELOG.md`

**Interfaces:** none new — this task freezes the surfaces Tasks 1–11 created.

- [ ] **Step 1: Regenerate baselines** via the `mise run check-public-api` flow (it names the regeneration command when baselines are missing; follow it rather than hand-editing).
- [ ] **Step 2: Run the wasm-exclusion check** (`scripts/wasm_package/check.py` via its mise task) → PASS.
- [ ] **Step 3: Write the doc rows and CHANGELOG entry.**
- [ ] **Step 4: Full verification:** `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `mise run check-public-api`, `cargo deny check` → all PASS.
- [ ] **Step 5: Commit**

```bash
git add scripts/wasm_package/check.py fixtures/compatibility docs/site/toolset.md extensions/toolsets/README.md CHANGELOG.md
git commit -m "chore(fetch): wasm exclusion, api baselines, docs, changelog"
```

---

### Task 13: Threat-model register row and security review

**Files:**
- Create: `docs/implementation/artifacts/<pr-dir>/security-review.txt` (whichever pr-NNN/topic dir this work is tracked under; follow the repository's current artifact convention)
- Prepare (do **not** land directly): the TM-22 row text for `docs/planning/06-finstack-ai-security-threat-model.md` §7

**Interfaces:** none — governance closure.

- [ ] **Step 1: Draft the TM-22 row** (columns `ID | Threat | Required controls | Primary verification and delivery`):

> TM-22 | SSRF / private-network egress via model-supplied URLs in the fetch toolset | Deny-by-default host allowlist (empty list fails construction); HTTPS-only except loopback fixtures; literal and resolved loopback/private/link-local/unique-local deny with IPv4-mapped canonicalization; DNS resolve-and-pin closing the rebinding TOCTOU; client redirects disabled with manual per-hop re-vetting under a hop cap; bounded body reads with no silent truncation; per-host-only credential headers | Unit corpus + scripted-resolver rebinding tests in `finstack-ai-net-guard`; pipeline tests in `finstack-ai-tools-fetch`; delivery: this change

- [ ] **Step 2: Hand the row to the repository owner** as a change-control item (`docs/planning/` is read-only during normal coding); reference spec §5 and note the SEC-INV-006/007 mapping and the §15 fail-closed gate satisfaction.
- [ ] **Step 3: Run the security review** per the §18 trigger (network egress + high-privilege battery): review the two crates against the TM-22 controls and the spec's §4.4 ordering; write findings to `security-review.txt` following the format of `docs/implementation/artifacts/pr-056/security-review.txt`.
- [ ] **Step 4: Commit**

```bash
git add docs/implementation/artifacts
git commit -m "docs(fetch): security review artifact and TM-22 handoff"
```

---

## Known Risks (read before starting)

1. **`ToolId::parse` may reject underscores** — verify in Task 6 before freezing `finstack.tools.http_fetch`; fall back to hyphens. Whichever lands is frozen forever (public-api + baselines).
2. **`htmd` license** may fail `cargo-deny` — Task 10 gates on it and names the `scraper` fallback. Do not broaden `deny.toml` to force it (the D1-spike stop-rule spirit applies).
3. **Loopback fixture bypass** (Task 7 step 2) is the one deliberate allowlist exception; it exists only under `allow_loopback_http: true`. Keep the two flags coupled — a config with fixtures enabled must never reach production, and the config doc must say so.
4. **`url` crate IDNA behavior**: hosts arrive punycoded from the parser; allowlist entries must be ASCII/punycode. The spec was updated to match — if a reviewer wants Unicode allowlist entries, punycode them in `HostPattern::parse` rather than comparing Unicode forms.
5. **`reqwest::Client` per call** is intentional (the pin differs per destination); do not "optimize" into a shared client — that reopens rebinding.
6. **Media-toolset and MCP-transport migration onto net-guard is out of scope** (spec §8 item 5) — resist drive-by refactors; file follow-ups instead.
7. **`ipv6` stability**: `Ipv6Addr::is_unique_local`/`is_unicast_link_local` are stable on the workspace toolchain (openrouter-media already uses them) — if a toolchain change breaks this, copy that crate's approach rather than inventing range math.
