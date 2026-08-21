#[test]
fn first_pass_and_reconcile_deferred_settlements_are_byte_identical() {
    let (_plans, coordinator, _store) = prepare_with(&["alpha"], None);
    let seed = coordinator
        .pending_tool_seeds()
        .into_iter()
        .next()
        .expect("pending tool seed");
    let deferral = crate::ToolDeferral {
        handle: finstack_ai_kernel::ExternalHandleRef::try_new(
            ComponentId::parse("finstack.tools.scripted").expect("component"),
            "handle-1",
            RawJson::parse(br#"{"cursor":"opaque"}"#).expect("metadata"),
        )
        .expect("handle"),
        reconciliation: finstack_ai_kernel::ReconciliationPolicy::CallbackOrPoll,
        next_poll_at: Some(fixed_timestamp(1_700)),
        expires_at: Some(fixed_timestamp(2_000)),
    };

    let first_pass = super::tool::build_tool_settlement(ToolDriverResult {
        seed: seed.clone(),
        result: Ok(AssembledToolTerminal {
            usage: Some(crate::Usage::empty()),
            artifacts: std::sync::Arc::from([]),
            terminal: crate::ToolTerminal::Deferred(deferral.clone()),
        }),
    })
    .expect("first-pass settlement");
    let reconcile = finstack_ai_kernel::ToolBatchSettled {
        tool_batch_id: seed.tool_batch_id,
        outcome: finstack_ai_kernel::ToolSettlement::Deferred(super::tool::tool_effect_deferred(
            seed.requested.effect_id(),
            seed.requested.output_contract(),
            &deferral,
        )),
    };

    let first_pass_bytes =
        serde_json_canonicalizer::to_vec(&first_pass).expect("first-pass canonical JSON");
    let reconcile_bytes =
        serde_json_canonicalizer::to_vec(&reconcile).expect("reconcile canonical JSON");

    assert_eq!(first_pass_bytes, reconcile_bytes);
}
