use super::*;
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, Digest, EffectId, EffectOutputContract,
    EffectOutputKind, LaneId, OperationLocator, PrincipalRef, RunCallContext, RunId, SessionId,
    ToolBatchId, ToolCallBlock, ToolCallId, ToolFailurePolicy,
};
use futures_util::StreamExt;

#[test]
fn arithmetic_is_bounded_and_stable() {
    assert_eq!(evaluate(Operation::Add, &[1.0, 2.0, 3.0]), Ok(6.0));
    assert_eq!(evaluate(Operation::Multiply, &[]), Ok(1.0));
    assert!(matches!(
        evaluate(Operation::Divide, &[1.0, 0.0]),
        Err(CalculatorError::Arithmetic { .. })
    ));
    assert!(matches!(
        evaluate(Operation::Add, &vec![1.0; MAX_OPERANDS + 1]),
        Err(CalculatorError::InvalidArguments { .. })
    ));
}

#[test]
fn tool_specs_are_built_once_and_shared() {
    let toolset = CalculatorToolset::try_new().expect("calculator");
    let first = toolset.tools();
    let second = toolset.tools();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].id.as_str(), TOOL_ID);
}

fn context(principal_scope: &str) -> ToolCallContext {
    ToolCallContext {
        run: RunCallContext {
            locator: OperationLocator::try_new(
                "tenant-a",
                SessionId::from_bytes([1; 16]),
                LaneId::from_bytes([2; 16]),
                RunId::from_bytes([3; 16]),
            )
            .expect("locator"),
            authorization: AuthorizationContext {
                principal: PrincipalRef::try_new("issuer", "subject", Some(principal_scope))
                    .expect("principal"),
                authentication_method: Arc::from("test"),
                assurance_level: Arc::from("test"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id: EffectId::from_bytes([4; 16]),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
        },
        tool_batch_id: ToolBatchId::from_bytes([5; 16]),
        tool_call_id: ToolCallId::from_bytes([6; 16]),
    }
}

fn validated_call(toolset: &CalculatorToolset) -> ValidatedToolCall {
    let spec = &toolset.tools[0];
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            ToolCallId::from_bytes([6; 16]),
            TOOL_NAME,
            RawJson::parse(br#"{"operands":[1,2,3],"operation":"add"}"#).expect("arguments"),
        )
        .expect("call"),
        tool_id: spec.id.clone(),
        component: None,
        output_contract: EffectOutputContract {
            kind: EffectOutputKind::ToolResult,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"{}"),
        },
        retry_safety: spec.retry_safety,
        deadline: None,
        execution: spec.execution,
        failure_policy: ToolFailurePolicy::ReturnToModel,
    }
}

#[tokio::test]
async fn public_call_uses_committed_authority_context() {
    let toolset = CalculatorToolset::try_new().expect("calculator");
    let mut stream = toolset
        .call(context("tenant-a"), validated_call(&toolset))
        .await
        .expect("call");
    let completed = stream.next().await.expect("item").expect("stream");
    assert!(matches!(completed, ToolStreamItem::Completed(_)));

    let Err(error) = toolset
        .call(context("tenant-b"), validated_call(&toolset))
        .await
    else {
        panic!("mismatched principal must fail");
    };
    assert_eq!(error.code(), CALCULATOR_INVALID_ARGUMENTS);
}
