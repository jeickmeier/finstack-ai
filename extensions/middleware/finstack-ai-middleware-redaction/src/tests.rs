use finstack_ai_kernel::Stage;
use finstack_ai_runtime::{Middleware as _, OrderTier};

use crate::{OutputPolicy, RedactionConfig, RedactionError, RedactionMiddleware};

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
