use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, Digest, EffectDeferred, EffectId, EffectOutputContract, EffectOutputKind,
    ExternalHandleRef, OperationLocator, RawJson, ReconciliationPolicy,
};
use finstack_ai_runtime::{ChildRunRequest, PortFuture};

use super::{ChildPlanContext, DeferredChildPlanner, DeferredPlanError};

struct UnownedPlanner;

impl DeferredChildPlanner for UnownedPlanner {
    fn plan(
        &self,
        _context: &ChildPlanContext,
    ) -> PortFuture<Result<Option<ChildRunRequest>, DeferredPlanError>> {
        Box::pin(async { Ok(None) })
    }
}

fn plan_context() -> ChildPlanContext {
    let effect_id = EffectId::from_bytes([
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x2c,
    ]);
    ChildPlanContext {
        parent: OperationLocator::try_new(
            "tenant-preview",
            finstack_ai_kernel::SessionId::from_bytes([
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x01,
            ]),
            finstack_ai_kernel::LaneId::from_bytes([
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x02,
            ]),
            finstack_ai_kernel::RunId::from_bytes([
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x03,
            ]),
        )
        .expect("parent locator"),
        deferred: EffectDeferred {
            effect_id,
            handle: ExternalHandleRef::try_new(
                ComponentId::parse("finstack.tool.scripted").expect("component"),
                "unowned",
                RawJson::parse(b"{}").expect("metadata"),
            )
            .expect("handle"),
            reconciliation: ReconciliationPolicy::CallbackOnly,
            next_poll_at: None,
            expires_at: None,
            output_contract: EffectOutputContract {
                kind: EffectOutputKind::ToolResult,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"tool-result"),
            },
        },
    }
}

#[tokio::test]
async fn planner_may_return_none_for_an_unowned_handle() {
    let planner = UnownedPlanner;
    let claim = planner
        .plan(&plan_context())
        .await
        .expect("unowned planner stays available");
    assert_eq!(claim, None);
    let _resolver: Option<Arc<dyn super::ChildRunResolver>> = None;
}
