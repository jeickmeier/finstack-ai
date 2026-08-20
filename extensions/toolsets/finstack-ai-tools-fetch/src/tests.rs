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
