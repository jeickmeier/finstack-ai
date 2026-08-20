use super::NetGuardError;

#[test]
fn error_display_names_stable_codes() {
    let error = NetGuardError::InvalidUrl { reason: "scheme_not_allowed" };
    assert_eq!(
        error.to_string(),
        "net_guard_invalid_url: scheme_not_allowed"
    );
}
