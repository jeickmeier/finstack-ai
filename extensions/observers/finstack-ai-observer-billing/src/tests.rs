use std::sync::Arc;

use finstack_ai_runtime::ObserverBackpressure;
use finstack_ai_test::check_observer_conformance;

use super::BillingObserver;

#[tokio::test]
async fn conformance_accepts_an_empty_batch() {
    let billing =
        BillingObserver::try_new(8, ObserverBackpressure::DropProgress, 64).expect("billing");
    check_observer_conformance(&billing, Arc::from([]))
        .await
        .expect("conformance");
}

#[tokio::test]
async fn invalid_bounds_are_rejected() {
    assert!(BillingObserver::try_new(0, ObserverBackpressure::DropProgress, 64).is_err());
    assert!(BillingObserver::try_new(8, ObserverBackpressure::DropProgress, 0).is_err());
    assert!(BillingObserver::try_new(8, ObserverBackpressure::DropProgress, 1_000_001).is_err());
}
