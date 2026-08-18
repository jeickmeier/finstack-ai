#[tokio::test]
async fn panicking_sink_does_not_fail_the_run() {
    let (_agent, parent, store, _effect_id) = deferred_tool_parent().await;
    let child = child_agent(Arc::clone(&store)).await;
    let invoker = Arc::new(RecordingInvoker::new(child));
    let request = isolated_child_request(Arc::clone(&store), &parent).await;
    let bridge = ChildRunBridge::new(
        vec![Arc::new(ClaimingPlanner::new(request))],
        Arc::clone(&invoker) as Arc<dyn finstack_ai_runtime::AgentInvoker>,
        Arc::clone(&invoker) as Arc<dyn finstack_ai::ChildRunResolver>,
    )
    .with_event_sink(Arc::new(PanickingSink));
    let outcomes = bridge.recover(&parent).await.expect("recover");
    assert_eq!(outcomes, vec![ChildSettleOutcome::Completed]);
    let after = CommitCoordinator::recover(store, parent.locator().session_id)
        .await
        .expect("recover after settle");
    assert!(outstanding_deferrals(after.state()).is_empty());
}

#[tokio::test]
async fn blocking_sink_does_not_fail_the_run() {
    let (_agent, parent, store, _effect_id) = deferred_tool_parent().await;
    let child = child_agent(Arc::clone(&store)).await;
    let invoker = Arc::new(RecordingInvoker::new(child));
    let request = isolated_child_request(Arc::clone(&store), &parent).await;
    let bridge = ChildRunBridge::new(
        vec![Arc::new(ClaimingPlanner::new(request))],
        Arc::clone(&invoker) as Arc<dyn finstack_ai_runtime::AgentInvoker>,
        Arc::clone(&invoker) as Arc<dyn finstack_ai::ChildRunResolver>,
    )
    .with_event_sink(Arc::new(BlockingSink));
    let outcomes = tokio::time::timeout(Duration::from_secs(5), bridge.recover(&parent))
        .await
        .expect("blocking sink must not stall settle")
        .expect("recover");
    assert_eq!(outcomes, vec![ChildSettleOutcome::Completed]);
    let after = CommitCoordinator::recover(store, parent.locator().session_id)
        .await
        .expect("recover after settle");
    assert!(outstanding_deferrals(after.state()).is_empty());
}
