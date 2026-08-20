use finstack_ai_kernel::Stage;
use finstack_ai_runtime::{Middleware as _, OrderTier};

use crate::detect::Detectors;
use crate::{OutputPolicy, RedactionConfig, RedactionError, RedactionMiddleware};

fn detectors() -> Detectors {
    Detectors::try_new(RedactionConfig::default()).expect("detectors")
}

fn redacted(text: &str) -> String {
    detectors().redact(text).expect("expected a redaction")
}

fn untouched(text: &str) {
    assert_eq!(detectors().redact(text), None, "should not redact: {text}");
}

// ---------------------------------------------------------------------------
// Task 1: descriptor
// ---------------------------------------------------------------------------

#[test]
fn descriptor_identity_and_order() {
    let middleware = RedactionMiddleware::try_new().expect("construct");
    let descriptor = middleware.descriptor();
    assert_eq!(
        descriptor.invocation.component.to_string(),
        "finstack.middleware.redaction"
    );
    assert_eq!(descriptor.order.tier, OrderTier::RequestShaping);
    assert_eq!(descriptor.order.priority, 0);
    assert!(descriptor.stages.contains(Stage::BeforeModel));
    assert!(!descriptor.stages.contains(Stage::AfterModel));
}

#[test]
fn fail_output_policy_declares_after_model() {
    let middleware = RedactionMiddleware::try_with_config(RedactionConfig {
        output_policy: OutputPolicy::Fail,
        ..RedactionConfig::default()
    })
    .expect("construct");
    let descriptor = middleware.descriptor();
    assert!(descriptor.stages.contains(Stage::BeforeModel));
    assert!(descriptor.stages.contains(Stage::AfterModel));
}

#[test]
fn all_detectors_disabled_is_a_configuration_error() {
    let result = RedactionMiddleware::try_with_config(RedactionConfig {
        detect_emails: false,
        detect_api_keys: false,
        detect_account_numbers: false,
        output_policy: OutputPolicy::Off,
    });
    assert_eq!(
        result.err(),
        Some(RedactionError::Configuration {
            reason: "all_detectors_disabled",
        })
    );
}

#[test]
fn configuration_digest_tracks_config() {
    let defaults = RedactionMiddleware::try_new().expect("construct");
    let no_emails = RedactionMiddleware::try_with_config(RedactionConfig {
        detect_emails: false,
        ..RedactionConfig::default()
    })
    .expect("construct");
    assert_ne!(
        defaults.descriptor().invocation.configuration_digest,
        no_emails.descriptor().invocation.configuration_digest,
    );
}

// ---------------------------------------------------------------------------
// Task 2: detection engine
// ---------------------------------------------------------------------------

#[test]
fn emails_are_redacted() {
    assert_eq!(
        redacted("reach me at jane.doe+x@example.co.uk today"),
        "reach me at [REDACTED:email] today"
    );
    untouched("user at host dot com");
}

#[test]
fn vendor_api_keys_are_redacted() {
    for secret in [
        "sk-proj-abcdefghij0123456789",
        "sk-ant-api03-abcdefghij0123456789",
        "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
        "github_pat_11ABCDEFG0123456789_abcdefghij",
        "xoxb-1234567890-abcdefghij",
        "AKIAIOSFODNN7EXAMPLE",
        "AIzaSyA-1234567890abcdefghijklmnopqrstuv",
        "sk_live_abcdefghij0123456789",
    ] {
        let input = format!("token: {secret} end");
        assert_eq!(
            detectors().redact(&input).as_deref(),
            Some("token: [REDACTED:api-key] end"),
            "should redact {secret}"
        );
    }
    untouched("short sk-abc is not a key");
}

#[test]
fn jwts_are_redacted() {
    assert_eq!(
        redacted(
            "bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        ),
        "bearer [REDACTED:jwt]"
    );
    untouched("two segments eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0 only");
}

#[test]
fn pem_private_keys_are_redacted() {
    let block = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----";
    assert_eq!(
        detectors()
            .redact(&format!("cfg:\n{block}\ndone"))
            .as_deref(),
        Some("cfg:\n[REDACTED:private-key]\ndone")
    );
    assert_eq!(
        redacted("-----BEGIN PRIVATE KEY----- truncated paste"),
        "[REDACTED:private-key] truncated paste"
    );
}

#[test]
fn luhn_valid_cards_are_redacted() {
    for card in [
        "4111111111111111",
        "4111 1111 1111 1111",
        "4111-1111-1111-1111",
    ] {
        let input = format!("card {card} on file");
        assert_eq!(
            detectors().redact(&input).as_deref(),
            Some("card [REDACTED:card] on file"),
            "should redact {card}"
        );
    }
}

#[test]
fn invalid_card_candidates_are_untouched() {
    untouched("card 4111111111111112 on file"); // Luhn fails
    untouched("run 411111111111 short"); // 12 digits
    untouched("mixed 4111 1111-1111 1111 separators"); // non-uniform
}

#[test]
fn valid_ibans_are_redacted() {
    assert_eq!(
        redacted("pay GB82WEST12345698765432 now"),
        "pay [REDACTED:iban] now"
    );
    untouched("pay GB82WEST12345698765433 now"); // mod-97 fails
}

#[test]
fn overlapping_matches_redact_once() {
    // The JWT's middle segment contains an api-key-shaped substring; only
    // one marker must be produced, for the longer, earlier match.
    let text = "eyJhbGciOiJIUzI1NiJ9.eyJsk-abcdefghij0123456789In0.dBjftJeZ4CVPmB92K27uhb";
    assert_eq!(redacted(text), "[REDACTED:jwt]");
}

#[test]
fn redaction_is_idempotent() {
    let once = redacted("mail jane@example.com card 4111111111111111");
    assert_eq!(detectors().redact(&once), None);
}

#[test]
fn disabled_detector_groups_do_not_fire() {
    let config = RedactionConfig {
        detect_emails: false,
        ..RedactionConfig::default()
    };
    let detectors = Detectors::try_new(config).expect("detectors");
    assert_eq!(detectors.redact("mail jane@example.com"), None);
    assert!(
        detectors
            .redact("key sk-proj-abcdefghij0123456789")
            .is_some()
    );
}

#[test]
fn kinds_in_reports_detector_kinds() {
    let kinds = detectors().kinds_in("mail jane@example.com key sk-proj-abcdefghij0123456789");
    assert!(kinds.contains("email"));
    assert!(kinds.contains("api-key"));
    assert!(!kinds.contains("card"));
}
