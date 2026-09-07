use super::*;

struct Planner(ChildRunRequest);

impl DeferredChildPlanner for Planner {
    fn plan(
        &self,
        _: &ChildPlanContext,
    ) -> PortFuture<Result<Option<ChildRunRequest>, DeferredPlanError>> {
        let request = self.0.clone();
        Box::pin(async move { Ok(Some(request)) })
    }
}

struct UnreachableResolver;

impl ChildRunResolver for UnreachableResolver {
    fn resolve(&self, _: &ChildRunLocator) -> PortFuture<Result<AgentRun, ChildRunBridgeError>> {
        panic!("denied child must never reach the resolver")
    }
}

#[tokio::test]
async fn child_admission_denies_direct_and_bridge_starts_before_commit_or_dispatch() {
    for policy in [ChildRunPolicy::Deny, ChildRunPolicy::Allow { max_depth: 0 }] {
        let gate = Arc::<str>::from("child-policy-parent");
        let mut plan = completed("parent");
        plan.actions
            .insert(0, ScriptedModelAction::Block(Arc::clone(&gate)));
        let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
        let control = model.control();
        let (agent, store) = model_only_agent_with_child_policy(model, policy).await;
        let store: Arc<dyn JournalStore> = store;
        let parent = agent.start(request("parent work")).expect("start");
        wait_accepted(&store, &parent).await;
        let effect_id = super::super::prepare::NativeIds::generate::<EffectTag>().expect("id");
        let request = isolated_child_request(Arc::clone(&store), &parent).await;
        let invoker = Arc::new(RecordingEffectInvoker {
            starts: AtomicUsize::new(0),
        });
        for _ in 0..2 {
            let error = parent
                .start_or_attach_child(invoker.clone(), effect_id, request.clone())
                .await
                .expect_err("policy denial, including retry");
            assert_eq!(
                error.code(),
                finstack_ai_runtime::child::AGENT_INVOKE_INVALID_ACCEPTANCE
            );
        }
        let bridge = ChildRunBridge::new(
            vec![Arc::new(Planner(request))],
            invoker.clone(),
            Arc::new(UnreachableResolver),
        );
        let deferred = finstack_ai_kernel::EffectDeferred {
            effect_id,
            handle: ExternalHandleRef::try_new(
                ComponentId::parse("test.tool.child").expect("component"),
                "job",
                RawJson::parse(b"{}").expect("json"),
            )
            .expect("handle"),
            reconciliation: ReconciliationPolicy::CallbackOnly,
            next_poll_at: None,
            expires_at: None,
            output_contract: finstack_ai_kernel::EffectOutputContract {
                kind: finstack_ai_kernel::EffectOutputKind::ToolResult,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"tool-result"),
            },
        };
        bridge
            .settle(&parent, &deferred)
            .await
            .expect_err("bridge policy denial");
        assert_eq!(invoker.starts.load(Ordering::SeqCst), 0);
        let recovered = CommitCoordinator::recover_run(
            store,
            parent.locator().session_id,
            Some(parent.locator().run_id),
        )
        .await
        .expect("recover");
        assert!(
            recovered
                .session()
                .child_mapping(parent.locator().run_id, effect_id)
                .is_none()
        );
        control.release(&gate);
        parent.result().await.expect("parent completes");
    }
}
