#[test]
fn parent_cancellation_obeys_persisted_child_propagation_authorization() {
    let parent = root_acceptance();
    let parent_run_id = parent.run_id();
    for (policy, decision_id, accepted) in [
        (CancellationPropagation::Cascade, "child-decision", true),
        (
            CancellationPropagation::DetachOnlyIfPreauthorized,
            "child-decision",
            true,
        ),
        (
            CancellationPropagation::DetachOnlyIfPreauthorized,
            "detach:child-decision",
            false,
        ),
    ] {
        let child_run_id = id::<finstack_ai_kernel::RunTag>(30);
        let relation = RunRelation::try_new(
            parent.relation().root_run_id(),
            Some(parent_run_id),
            Some(id::<finstack_ai_kernel::EffectTag>(31)),
            finstack_ai_kernel::RunRelationKind::ChildAgent,
            1,
            None,
            None::<&str>,
        )
        .expect("child relation");
        let security = RunSecurityContext::try_new(
            "tenant-a",
            parent.security().principal().clone(),
            "oidc",
            "high",
            "policy-v1",
            decision_id,
            None,
        )
        .expect("child security");
        let child = RunAccepted::try_new(
            child_run_id,
            relation,
            security,
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: policy,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(br#"{"agent":"child"}"#),
            Some(&parent),
        )
        .expect("child acceptance");
        let mut harness = Harness::default();
        harness.apply_input(
            transition_env(1_000, &[1], &[1], &[], &[], &[], &[]),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<finstack_ai_kernel::SessionTag>(SESSION),
                lane_id: id::<finstack_ai_kernel::LaneTag>(LANE),
                accepted: child,
            }),
        );
        let result = harness.kernel.decide(
            &cancellation_env(1_100, &[2], &[], &[700]),
            KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
                initiator: finstack_ai_kernel::CancellationInitiator::ParentRun { parent_run_id },
                reason: Some(Arc::from("parent_cancelled")),
            }),
        );
        if accepted {
            assert!(matches!(
                result.expect("authorized propagation").records.as_slice(),
                [record] if matches!(record.body(), RecordBody::CancellationRequested(_))
            ));
        } else {
            assert!(matches!(
                result.expect_err("preauthorized detach must reject parent cancellation"),
                KernelError::InvalidInputPayload {
                    field: "initiator",
                    reason_code: "unauthorized"
                }
            ));
        }
    }
}
