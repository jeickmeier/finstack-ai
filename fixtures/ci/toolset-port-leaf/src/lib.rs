//! Leaf-crate compile proof for the public target-correct `Toolset` ABI.

use core::pin::Pin;
use core::task::{Context, Poll};
use std::sync::Arc;

use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, Metadata, PortFuture, RawJson, RetrySafety,
    SideEffectClass, ToolCallContext, ToolDeferralSupport, ToolError, ToolEventStream,
    ToolExecutionMode, ToolId, ToolResult, ToolSpec, ToolStreamItem, Toolset, ToolsetDescriptor,
    ValidatedToolCall,
};
use futures_core::Stream;

/// Minimal external Toolset leaf using only the public runtime contract.
pub struct LeafToolset;

impl LeafToolset {
    /// Public deterministic tool specification used by the sample.
    ///
    /// # Panics
    ///
    /// Panics only if the checked-in tool identity or schema constants become invalid.
    #[must_use]
    pub fn spec() -> ToolSpec {
        ToolSpec {
            id: ToolId::parse("finstack.tools.leaf_echo").expect("tool id"),
            model_name: Arc::from("leaf_echo"),
            title: Arc::from("Leaf echo"),
            description: Arc::from("Public conformance sample tool"),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{},"type":"object"}"#,
            )
            .expect("input schema"),
            output_schema: None,
            execution: ToolExecutionMode::Sequential,
            side_effect: SideEffectClass::ReadOnly,
            retry_safety: RetrySafety::SafeToRetry,
            approval: ApprovalMetadata {
                requirement: ApprovalRequirement::NotRequired,
                reason: None,
                attributes: Metadata::empty(),
            },
            max_result_bytes: 1_024,
            metadata: Metadata::empty(),
            deferral: ToolDeferralSupport::Never,
        }
    }

    /// Deterministic normalized result returned by the sample.
    ///
    /// # Panics
    ///
    /// Panics only if the checked-in canonical result constant becomes invalid.
    #[must_use]
    pub fn result() -> ToolResult {
        ToolResult {
            output: RawJson::parse(br#"{"ok":true}"#).expect("tool output"),
            is_error: false,
        }
    }
}

impl Toolset for LeafToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        ToolsetDescriptor {
            name: Arc::from("leaf-toolset"),
            metadata: Metadata::empty(),
        }
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::from([Self::spec()])
    }

    fn call(
        &self,
        _ctx: ToolCallContext,
        _call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        Box::pin(async {
            Ok(Box::pin(SingleItemStream {
                item: Some(Ok(ToolStreamItem::Completed(Self::result()))),
            }) as ToolEventStream)
        })
    }
}

struct SingleItemStream {
    item: Option<Result<ToolStreamItem, ToolError>>,
}

impl Stream for SingleItemStream {
    type Item = Result<ToolStreamItem, ToolError>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.item.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_runtime::{
        AssembledToolStream, AuthorizationContext, CancellationSignal, Digest,
        EffectOutputContract, EffectOutputKind, LaneId, OperationLocator, PrincipalRef,
        RunCallContext, RunId, SessionId, ToolBatchId, ToolCallBlock, ToolCallId,
        ToolFailurePolicy, ToolStreamLimits, Usage,
    };
    use finstack_ai_test::{ToolsetConformanceCase, check_toolset_conformance};

    fn session_id(value: u64) -> SessionId {
        SessionId::parse(&format!("00000000-0000-7000-8000-{value:012x}")).expect("session")
    }

    fn lane_id(value: u64) -> LaneId {
        LaneId::parse(&format!("00000000-0000-7000-8000-{value:012x}")).expect("lane")
    }

    fn run_id(value: u64) -> RunId {
        RunId::parse(&format!("00000000-0000-7000-8000-{value:012x}")).expect("run")
    }

    fn tool_batch_id(value: u64) -> ToolBatchId {
        ToolBatchId::parse(&format!("00000000-0000-7000-8000-{value:012x}")).expect("tool batch")
    }

    fn tool_call_id(value: u64) -> ToolCallId {
        ToolCallId::parse(&format!("00000000-0000-7000-8000-{value:012x}")).expect("tool call")
    }

    #[tokio::test]
    async fn public_leaf_toolset_passes_test_kit_conformance() {
        let toolset = LeafToolset;
        let spec = LeafToolset::spec();
        let call_id = tool_call_id(6);
        let principal =
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
        let context = ToolCallContext {
            run: RunCallContext {
                locator: OperationLocator::try_new(
                    "tenant-a",
                    session_id(1),
                    lane_id(2),
                    run_id(3),
                )
                .expect("locator"),
                authorization: AuthorizationContext {
                    principal,
                    authentication_method: Arc::from("test"),
                    assurance_level: Arc::from("test"),
                    roles: Arc::from([]),
                    permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                    safe_claims: Metadata::empty(),
                    policy_version: Arc::from("policy-v1"),
                    decision_id: Arc::from("decision-v1"),
                },
                effect_id: finstack_ai_runtime::EffectId::parse(
                    "00000000-0000-7000-8000-000000000004",
                )
                .expect("effect"),
                attempt: 1,
                deadline: None,
                budget_scope_id: None,
                cancellation: CancellationSignal::new(),
            },
            tool_batch_id: tool_batch_id(5),
            tool_call_id: call_id,
        };
        let call = ValidatedToolCall {
            call: ToolCallBlock::try_new(
                call_id,
                "leaf_echo",
                RawJson::parse(b"{}").expect("args"),
            )
            .expect("call"),
            tool_id: spec.id,
            component: None,
            output_contract: EffectOutputContract {
                kind: EffectOutputKind::ToolResult,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"{}"),
            },
            retry_safety: RetrySafety::SafeToRetry,
            deadline: None,
            execution: ToolExecutionMode::Sequential,
            failure_policy: ToolFailurePolicy::ReturnToModel,
        };
        let expected = AssembledToolStream {
            progress: Arc::from([]),
            usage: None,
            result: LeafToolset::result(),
        };
        let assembled = check_toolset_conformance(
            &toolset,
            ToolsetConformanceCase {
                context,
                call,
                expected: expected.clone(),
                stream_limits: ToolStreamLimits::default(),
                max_result_bytes: 1_024,
            },
        )
        .await
        .expect("public Toolset contract");
        assert_eq!(assembled, expected);
        assert_eq!(assembled.usage, None::<Usage>);
    }
}
