fn poison_mutex<T>(mutex: &Mutex<T>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = mutex.lock().expect("lock");
        panic!("poison");
    }));
}

#[test]
fn cancel_registered_effect_poison_returns_dispatch_error() {
    let active = Mutex::new(BTreeMap::new());
    poison_mutex(&active);
    let error = cancel_registered_effect(&active, id(1), "model_effect_registry_unavailable")
        .expect_err("poisoned registry");
    assert_eq!(error.code, "model_effect_registry_unavailable");
}

#[test]
fn cancel_registered_effect_missing_id_is_idempotent() {
    let active = Mutex::new(BTreeMap::new());
    assert!(
        cancel_registered_effect(&active, id(1), "model_effect_registry_unavailable")
            .expect("lock")
            .is_none()
    );
}

#[test]
fn cancel_registered_effect_returns_registered_signal() {
    let signal = crate::CancellationSignal::new();
    let mut map = BTreeMap::new();
    map.insert(id(7), signal.clone());
    let active = Mutex::new(map);
    let found = cancel_registered_effect(&active, id(7), "model_effect_registry_unavailable")
        .expect("lock")
        .expect("registered");
    found.cancel();
    assert!(signal.is_cancelled());
}
