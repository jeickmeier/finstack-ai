use super::{NetGuardError, UrlPolicy, parse_and_vet_url};

#[test]
fn error_display_names_stable_codes() {
    let error = NetGuardError::InvalidUrl { reason: "scheme_not_allowed" };
    assert_eq!(
        error.to_string(),
        "net_guard_invalid_url: scheme_not_allowed"
    );
}

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
        parse_and_vet_url(bad, &strict()).expect_err(bad);
    }
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
