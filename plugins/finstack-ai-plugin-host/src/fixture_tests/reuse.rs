use super::*;

async fn adapter(guest: &str, identity: &str) -> WasmToolsetAdapter {
    WasmToolsetAdapter::try_new(
        host(InstancePolicy::Serialized, 1),
        &fixture_wasm(guest),
        parse_manifest(&manifest_bytes(identity, &["toolset-plugin"])).expect("manifest"),
        &construction(identity),
        Arc::new(NoopPluginHooks),
    )
    .await
    .expect("adapter")
}

#[tokio::test]
async fn serialized_reuse_replenishes_fuel_for_every_invocation() {
    let adapter = adapter("echo-toolset", "finstack.plugin.echo.toolset").await;
    for _ in 0..500 {
        let mut result = adapter
            .call(
                tool_ctx(),
                validated_call("finstack.plugin.add", br#"{"a":1,"b":1}"#),
            )
            .await
            .expect("call");
        assert!(matches!(
            result.next().await,
            Some(Ok(ToolStreamItem::Completed(_)))
        ));
    }
}

#[tokio::test]
async fn serialized_resource_failure_discards_the_unusable_instance() {
    let adapter = adapter("fuel-burner", "finstack.plugin.fuel.toolset").await;
    for _ in 0..2 {
        let error = adapter
            .call(tool_ctx(), validated_call("finstack.plugin.burn", b"{}"))
            .await
            .err()
            .expect("fuel exhaustion");
        assert_eq!(error.code(), "plugin_resource_limit");
        assert!(adapter.hold_serialized_slot().expect("slot").is_none());
    }
}

#[tokio::test]
async fn dropping_an_in_flight_serialized_call_discards_its_instance() {
    let adapter = adapter("fuel-burner", "finstack.plugin.fuel.toolset").await;
    // Cooperative fuel yielding makes the real guest suspend during its call.
    let mut call = adapter.call(tool_ctx(), validated_call("finstack.plugin.burn", b"{}"));
    assert!(futures_util::poll!(&mut call).is_pending());
    drop(call);
    assert!(
        adapter
            .hold_serialized_slot()
            .expect("released slot")
            .is_none()
    );
    let error = adapter
        .call(tool_ctx(), validated_call("finstack.plugin.burn", b"{}"))
        .await
        .err()
        .expect("fresh guest exhausts fuel");
    assert_eq!(error.code(), "plugin_resource_limit");
}

#[tokio::test]
async fn cancelling_an_in_flight_serialized_call_discards_its_instance() {
    let adapter = adapter("fuel-burner", "finstack.plugin.fuel.toolset").await;
    let ctx = tool_ctx();
    let cancel = ctx.run.cancellation.clone();
    let mut call = adapter.call(ctx, validated_call("finstack.plugin.burn", b"{}"));
    assert!(futures_util::poll!(&mut call).is_pending());
    cancel.cancel();
    let error = call.await.err().expect("cancelled");
    assert_eq!(error.code(), "plugin_lifecycle_timeout");
    assert!(
        adapter
            .hold_serialized_slot()
            .expect("released slot")
            .is_none()
    );
}
