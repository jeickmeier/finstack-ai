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
