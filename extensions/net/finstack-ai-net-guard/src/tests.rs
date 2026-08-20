use super::{NetGuardError, UrlPolicy, parse_and_vet_url, reject_literal_destination};

#[test]
fn error_display_names_stable_codes() {
    let error = NetGuardError::InvalidUrl {
        reason: "scheme_not_allowed",
    };
    assert_eq!(
        error.to_string(),
        "net_guard_invalid_url: scheme_not_allowed"
    );
}

fn strict() -> UrlPolicy {
    UrlPolicy {
        allow_loopback_http: false,
        allow_nonstandard_https_port: false,
    }
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
    let policy = UrlPolicy {
        allow_loopback_http: true,
        allow_nonstandard_https_port: false,
    };
    let vetted = parse_and_vet_url("http://127.0.0.1:8080/x", &policy).expect("fixture");
    assert!(vetted.is_loopback);
    assert_eq!(vetted.port, 8080);
    // Non-loopback plaintext stays forbidden even under the fixture policy.
    parse_and_vet_url("http://docs.rs/", &policy).expect_err("http public");
}

#[test]
fn loopback_scope_is_exact_not_whole_127_8() {
    // 127.77.1.2 is within 127.0.0.0/8 (IpAddr::is_loopback would accept it)
    // but is not the exact conventional loopback address, so it must not be
    // treated as loopback: plaintext http to it is forbidden even under the
    // fixture policy.
    let policy = UrlPolicy {
        allow_loopback_http: true,
        allow_nonstandard_https_port: false,
    };
    parse_and_vet_url("http://127.77.1.2:8080/x", &policy).expect_err("not exact loopback");
    // The exact addresses still work.
    let vetted = parse_and_vet_url("http://127.0.0.1:8080/x", &policy).expect("exact loopback");
    assert!(vetted.is_loopback);
    let vetted = parse_and_vet_url("http://[::1]:8080/x", &policy).expect("exact ipv6 loopback");
    assert!(vetted.is_loopback);
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
        parse_and_vet_url(bad, &strict()).expect_err(bad);
    }
}

#[test]
fn nonstandard_https_port_is_rejected_by_default_and_allowed_when_opted_in() {
    // 443-only is the right default for a model-supplied URL: a
    // non-standard port is left rejected unless the caller opts in.
    parse_and_vet_url("https://docs.rs:8443/", &strict()).expect_err("default policy is 443-only");
    let policy = UrlPolicy {
        allow_loopback_http: false,
        allow_nonstandard_https_port: true,
    };
    let vetted = parse_and_vet_url("https://media.example.com:8443/a.mp3", &policy)
        .expect("opted-in caller may use a non-standard https port");
    assert_eq!(vetted.port, 8443);
    // The opt-in does not relax anything else: userinfo is still forbidden.
    parse_and_vet_url("https://user:pw@docs.rs:8443/", &policy)
        .expect_err("userinfo stays forbidden regardless of the port setting");
}

#[test]
fn unicode_hosts_normalize_to_punycode() {
    let vetted = parse_and_vet_url("https://bücher.example/", &strict()).expect("idna");
    assert_eq!(vetted.host, "xn--bcher-kva.example");
}

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use super::{SystemResolver, is_forbidden_destination, resolve_and_pin};

struct ScriptedResolver(Vec<SocketAddr>);

impl super::HostResolver for ScriptedResolver {
    fn resolve(
        &self,
        _host: &str,
        _port: u16,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + '_>,
    > {
        let addrs = self.0.clone();
        Box::pin(async move { Ok(addrs) })
    }
}

#[test]
fn forbidden_destination_covers_private_ranges() {
    for addr in [
        "127.0.0.1",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.1.1",
        "0.0.0.0",
        "255.255.255.255",
        "224.0.0.1",
        "::1",
        "fd00::1",
        "fe80::1",
        "::",
        "ff02::1",
        "::ffff:10.0.0.1", // IPv4-mapped must canonicalize before checking
    ] {
        assert!(
            is_forbidden_destination(addr.parse::<IpAddr>().unwrap()),
            "{addr}"
        );
    }
    assert!(!is_forbidden_destination(
        "93.184.216.34".parse::<IpAddr>().unwrap()
    ));
    assert!(!is_forbidden_destination(
        "2606:2800:220:1::1".parse::<IpAddr>().unwrap()
    ));
}

#[tokio::test]
async fn one_private_address_rejects_the_whole_set() {
    let vetted = super::parse_and_vet_url(
        "https://example.com/",
        &super::UrlPolicy {
            allow_loopback_http: false,
            allow_nonstandard_https_port: false,
        },
    )
    .unwrap();
    // Public first, private second: the rebinding shape. Must still fail.
    let resolver = ScriptedResolver(vec![
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)), 443),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 443),
    ]);
    resolve_and_pin(&vetted, &resolver)
        .await
        .expect_err("mixed set");
}

#[tokio::test]
async fn first_public_address_is_pinned() {
    let vetted = super::parse_and_vet_url(
        "https://example.com/",
        &super::UrlPolicy {
            allow_loopback_http: false,
            allow_nonstandard_https_port: false,
        },
    )
    .unwrap();
    let a = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)), 443);
    let b = SocketAddr::new(
        IpAddr::V6(Ipv6Addr::new(0x2606, 0x2800, 0x220, 1, 0, 0, 0, 1)),
        443,
    );
    let resolver = ScriptedResolver(vec![a, b]);
    assert_eq!(resolve_and_pin(&vetted, &resolver).await.unwrap(), a);
}

#[tokio::test]
async fn literal_hosts_skip_resolution_but_not_the_deny_check() {
    let policy = super::UrlPolicy {
        allow_loopback_http: true,
        allow_nonstandard_https_port: false,
    };
    // Loopback literal allowed under the fixture policy…
    let vetted = super::parse_and_vet_url("http://127.0.0.1:9/", &policy).unwrap();
    let pinned = resolve_and_pin(&vetted, &SystemResolver).await.unwrap();
    assert_eq!(pinned, SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9));
    // …but a private literal is blocked even then.
    let vetted = super::parse_and_vet_url(
        "https://10.0.0.1/",
        &super::UrlPolicy {
            allow_loopback_http: false,
            allow_nonstandard_https_port: false,
        },
    )
    .unwrap();
    resolve_and_pin(&vetted, &SystemResolver)
        .await
        .expect_err("private literal");
}

#[tokio::test]
async fn resolved_address_in_127_8_but_not_exact_loopback_is_still_blocked() {
    // "localhost" vets as loopback (allow_loopback flows through to
    // resolve_and_pin), but a resolved answer of 127.77.1.2 — in
    // 127.0.0.0/8 yet not the exact 127.0.0.1 — must still be rejected:
    // the allow-loopback carve-out is scoped to the exact address, not the
    // whole block.
    let policy = super::UrlPolicy {
        allow_loopback_http: true,
        allow_nonstandard_https_port: false,
    };
    let vetted = super::parse_and_vet_url("http://localhost:9/", &policy).unwrap();
    assert!(vetted.is_loopback);
    let resolver = ScriptedResolver(vec![SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(127, 77, 1, 2)),
        9,
    )]);
    resolve_and_pin(&vetted, &resolver)
        .await
        .expect_err("not exact loopback");
}

#[tokio::test]
async fn empty_resolution_fails() {
    let vetted = super::parse_and_vet_url(
        "https://example.com/",
        &super::UrlPolicy {
            allow_loopback_http: false,
            allow_nonstandard_https_port: false,
        },
    )
    .unwrap();
    resolve_and_pin(&vetted, &ScriptedResolver(Vec::new()))
        .await
        .expect_err("empty");
}

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
    let policy = super::UrlPolicy {
        allow_loopback_http: true,
        allow_nonstandard_https_port: false,
    };
    let vetted =
        super::parse_and_vet_url(&format!("http://127.0.0.1:{}/x", addr.port()), &policy).unwrap();
    let pinned = super::resolve_and_pin(&vetted, &super::SystemResolver)
        .await
        .unwrap();
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
    let policy = super::UrlPolicy {
        allow_loopback_http: true,
        allow_nonstandard_https_port: false,
    };
    let vetted =
        super::parse_and_vet_url(&format!("http://127.0.0.1:{}/x", addr.port()), &policy).unwrap();
    let pinned = super::resolve_and_pin(&vetted, &super::SystemResolver)
        .await
        .unwrap();
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
    tokio::spawn(serve_once(
        listener,
        302,
        "Location: https://docs.rs/\r\n",
        b"",
    ));
    let policy = super::UrlPolicy {
        allow_loopback_http: true,
        allow_nonstandard_https_port: false,
    };
    let vetted =
        super::parse_and_vet_url(&format!("http://127.0.0.1:{}/x", addr.port()), &policy).unwrap();
    let pinned = super::resolve_and_pin(&vetted, &super::SystemResolver)
        .await
        .unwrap();
    let client = super::pinned_client(&vetted, pinned, std::time::Duration::from_secs(5)).unwrap();
    let response = client.get(vetted.url.as_str()).send().await.unwrap();
    assert_eq!(response.status(), 302); // surfaced, not followed
}

#[test]
fn reject_literal_destination_catches_an_https_loopback_literal_synchronously() {
    // `parse_and_vet_url` alone does not reject this: `https` scopes
    // `VettedUrl::is_loopback` to `false` regardless of host, so only the
    // synchronous literal check (or the later async `resolve_and_pin`)
    // catches a literal loopback/private host on `https`.
    let vetted = parse_and_vet_url("https://127.0.0.1/a.mp3", &strict()).expect("vets");
    assert!(!vetted.is_loopback);
    reject_literal_destination(&vetted, &strict()).expect_err("https loopback literal");
}

#[test]
fn reject_literal_destination_allows_https_loopback_under_a_permissive_policy() {
    let policy = UrlPolicy {
        allow_loopback_http: true,
        allow_nonstandard_https_port: false,
    };
    let vetted = parse_and_vet_url("https://127.0.0.1/a.mp3", &policy).expect("vets");
    reject_literal_destination(&vetted, &policy).expect("loopback permitted under policy");
}

#[test]
fn reject_literal_destination_rejects_bare_localhost_and_private_literals() {
    let vetted = parse_and_vet_url("https://localhost/a.mp3", &strict()).expect("vets");
    reject_literal_destination(&vetted, &strict()).expect_err("bare localhost");
    let vetted =
        parse_and_vet_url("https://169.254.169.254/latest/meta-data", &strict()).expect("vets");
    reject_literal_destination(&vetted, &strict()).expect_err("link-local literal");
}

#[test]
fn reject_literal_destination_allows_a_public_literal_or_hostname() {
    let vetted = parse_and_vet_url("https://93.184.216.34/a.mp3", &strict()).expect("vets");
    reject_literal_destination(&vetted, &strict()).expect("public literal");
    let vetted = parse_and_vet_url("https://example.com/a.mp3", &strict()).expect("vets");
    reject_literal_destination(&vetted, &strict())
        .expect("a hostname needs resolution, not a literal deny");
}
