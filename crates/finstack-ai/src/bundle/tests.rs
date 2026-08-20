use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{BundleId, ComponentId, ComponentRef, Digest, RawJson, Version};

use super::secret::ensure_secret_free_config;
use super::*;

#[test]
fn strict_bundle_and_lock_round_trip_reject_unknown_fields() {
    let lock = ResolvedAgentLock {
        schema_version: BUNDLE_SCHEMA_VERSION,
        engine_version: Version {
            major: 0,
            minor: 0,
            patch: 1,
        },
        agent_spec_digest: Digest::raw_json(b"agent"),
        bundle: Some(LockedBundle {
            id: BundleId::parse("finstack.bundle.test").expect("bundle"),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
        }),
        components: Arc::from([LockedComponent {
            component: ComponentRef::new(
                ComponentId::parse("finstack.model.test").expect("component"),
                Some(Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                }),
            ),
            kind: LockedComponentKind::Model,
            config_digest: None,
            configuration_schema: None,
        }]),
        capabilities: Arc::from([]),
        effective_config_digest: Digest::raw_json(b"config"),
        middleware_chain_digest: Digest::raw_json(b"middleware"),
        schema_digests: Arc::from([]),
        required_services: RequiredServices::default(),
    };
    let bytes = lock.to_json().expect("lock JSON");
    let decoded = ResolvedAgentLock::from_json(&bytes).expect("lock round trip");
    assert_eq!(decoded, lock);
    assert_eq!(decoded.fingerprint(), lock.fingerprint());

    let mut unknown: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
    unknown["credential"] = serde_json::json!("secret-canary");
    assert!(ResolvedAgentLock::from_json(&serde_json::to_vec(&unknown).expect("JSON")).is_err());
}

#[test]
fn configuration_secret_canary_fails_closed_but_refs_are_allowed() {
    let component = ComponentId::parse("finstack.model.test").expect("component");
    let bad = BTreeMap::from([(
        component.clone(),
        RawJson::parse(br#"{"api_key":"secret-canary"}"#).expect("JSON"),
    )]);
    assert!(ensure_secret_free_config(&bad).is_err());
    let good = BTreeMap::from([(
        component,
        RawJson::parse(br#"{"api_key_ref":"vault://model"}"#).expect("JSON"),
    )]);
    ensure_secret_free_config(&good).expect("secret reference");
    for key in [
        "apikey",
        "auth",
        "authorization",
        "secretkey",
        "client_secret",
    ] {
        let body = format!(r#"{{"{key}":"secret-canary"}}"#);
        let bad = BTreeMap::from([(
            ComponentId::parse("finstack.model.test").expect("component"),
            RawJson::parse(body.as_bytes()).expect("JSON"),
        )]);
        assert!(
            ensure_secret_free_config(&bad).is_err(),
            "{key} must fail closed"
        );
        let ref_body = format!(r#"{{"{key}_ref":"vault://model"}}"#);
        let allowed = BTreeMap::from([(
            ComponentId::parse("finstack.model.test").expect("component"),
            RawJson::parse(ref_body.as_bytes()).expect("JSON"),
        )]);
        ensure_secret_free_config(&allowed).unwrap_or_else(|_| panic!("{key}_ref must be allowed"));
    }
    for key in ["oauth_client_id", "author", "authority"] {
        let body = format!(r#"{{"{key}":"not-a-secret"}}"#);
        let allowed = BTreeMap::from([(
            ComponentId::parse("finstack.model.test").expect("component"),
            RawJson::parse(body.as_bytes()).expect("JSON"),
        )]);
        ensure_secret_free_config(&allowed).unwrap_or_else(|_| panic!("{key} must stay allowed"));
    }
    let auth_token = BTreeMap::from([(
        ComponentId::parse("finstack.model.test").expect("component"),
        RawJson::parse(br#"{"auth_token":"secret-canary"}"#).expect("JSON"),
    )]);
    assert!(ensure_secret_free_config(&auth_token).is_err());
}

#[test]
fn version_ranges_and_required_services_are_finite() {
    let requirement = VersionRequirement::Range {
        min_inclusive: Version {
            major: 1,
            minor: 2,
            patch: 0,
        },
        max_exclusive: Version {
            major: 2,
            minor: 0,
            patch: 0,
        },
    };
    assert!(requirement.matches(Version {
        major: 1,
        minor: 9,
        patch: 0,
    }));
    assert!(!requirement.matches(Version {
        major: 2,
        minor: 0,
        patch: 0,
    }));
    let services = RuntimeServices::default();
    assert!(
        services
            .validate(RequiredServices {
                budget_ledger: true,
                ..RequiredServices::default()
            })
            .is_err()
    );
    services
        .validate(RequiredServices::default())
        .expect("minimal agent needs no services");
}

#[test]
fn missing_required_object_store_fails_resolution() {
    let services = RuntimeServices::default();
    let error = services
        .validate(RequiredServices {
            object_store: true,
            ..RequiredServices::default()
        })
        .expect_err("missing object store must fail");
    match error {
        BundleResolutionError::Missing { item } => assert_eq!(&*item, "object_store"),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn present_object_store_satisfies_the_requirement() {
    let services = RuntimeServices {
        object_store: Some(Arc::new(
            finstack_ai_test::object_store::FakeObjectStore::default(),
        )),
        ..RuntimeServices::default()
    };
    services
        .validate(RequiredServices {
            object_store: true,
            ..RequiredServices::default()
        })
        .expect("present object store satisfies the requirement");
}
