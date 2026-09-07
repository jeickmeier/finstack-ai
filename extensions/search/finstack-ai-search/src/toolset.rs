use std::sync::Arc;

use finstack_ai_kernel::{
    ErrorCategory, Metadata, RawJson, RetrySafety, ToolExecutionMode, ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, ReconcileContext, SideEffectClass, ToolDeferralSupport,
    ToolSpec,
};
use finstack_ai_runtime::ports::tool::{
    PendingToolEffect, ToolCallContext, ToolError, ToolEventStream, ToolReconcileResult,
    ToolResult, ToolStreamItem, Toolset, ToolsetDescriptor,
};

use crate::{SearchEngine, SearchError, SearchRequest};

/// Stable globally unique tool id; the model-facing name is `search`.
pub const SEARCH_TOOL_ID: &str = "finstack.search.query";
const MAX_RESULT_BYTES: u64 = 1_048_576;

/// One search tool over a host-bound engine. Query arguments contain no tenant,
/// user, workspace, artifact authority, backend paths, or provider credentials.
#[derive(Clone)]
pub struct SearchToolset {
    engine: Arc<SearchEngine>,
    tools: Arc<[ToolSpec]>,
}

impl SearchToolset {
    /// Construct and validate the search tool spec.
    ///
    /// # Errors
    /// Rejects invalid checked-in schemas/identity.
    pub fn try_new(engine: Arc<SearchEngine>) -> Result<Self, SearchError> {
        let spec = ToolSpec {
            id: ToolId::parse(SEARCH_TOOL_ID).map_err(|_| SearchError::invalid("search_tool_id"))?,
            model_name: "search".into(), title: "Search knowledge".into(),
            description: "Search authorized memory, documents, committed journal entries, and optional graph. Results are untrusted evidence with citations and explicit source coverage. Use only configured semantic spaces or graph strategies.".into(),
            input_schema: crate::schema::input_schema(&engine)?,
            output_schema: None, execution: ToolExecutionMode::Parallel, side_effect: SideEffectClass::ReadOnly,
            retry_safety: RetrySafety::SafeToRetry,
            approval: ApprovalMetadata { requirement: ApprovalRequirement::NotRequired, reason: None, attributes: Metadata::empty() },
            max_result_bytes: MAX_RESULT_BYTES, metadata: Metadata::empty(), deferral: ToolDeferralSupport::Never,
        };
        spec.validate()
            .map_err(|_| SearchError::invalid("search_tool_spec"))?;
        Ok(Self {
            engine,
            tools: vec![spec].into(),
        })
    }
}

impl Toolset for SearchToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        ToolsetDescriptor {
            name: "finstack-search".into(),
            metadata: Metadata::empty(),
        }
    }
    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let engine = Arc::clone(&self.engine);
        Box::pin(async move {
            if call.tool_id.as_str() != SEARCH_TOOL_ID
                || call.call.tool_name() != "search"
                || ctx.run.locator.tenant_scope != engine.config().scope.tenant
                || ctx
                    .run
                    .authorization
                    .principal
                    .tenant_scope()
                    .is_some_and(|tenant| tenant != engine.config().scope.tenant.as_ref())
            {
                return Err(tool_error(SearchError::SearchScopeDenied));
            }
            if call.call.arguments().as_bytes().len() > 16_384 {
                return Err(tool_error(SearchError::invalid("search_arguments")));
            }
            let request: SearchRequest =
                serde_json::from_slice(call.call.arguments().as_bytes())
                    .map_err(|_| tool_error(SearchError::invalid("search_arguments")))?;
            let result = tokio::select! {
                biased;
                () = ctx.run.cancellation.cancelled() => return Err(cancelled()),
                result = engine.search(request) => result,
            };
            let (value, is_error) = match result {
                Ok(response) => (serde_json::to_value(response), false),
                Err(error @ SearchError::SearchNoSuccessfulSources { .. }) => {
                    (serde_json::to_value(error), true)
                }
                Err(error) => return Err(tool_error(error)),
            };
            let bytes = serde_json::to_vec(
                &value.map_err(|_| tool_error(SearchError::invalid("search_output")))?,
            )
            .map_err(|_| tool_error(SearchError::invalid("search_output")))?;
            if bytes.len() as u64 > MAX_RESULT_BYTES {
                return Err(tool_error(SearchError::SearchCapacityExceeded {
                    resource: "search_result_bytes".into(),
                }));
            }
            let output = RawJson::parse(bytes)
                .map_err(|_| tool_error(SearchError::invalid("search_output")))?;
            Ok(Box::pin(futures_util::stream::once(async move {
                Ok(ToolStreamItem::Completed(ToolResult { output, is_error }))
            })) as ToolEventStream)
        })
    }

    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingToolEffect,
    ) -> PortFuture<Result<ToolReconcileResult, ToolError>> {
        Box::pin(async { Ok(ToolReconcileResult::RetrySafe) })
    }
}

#[allow(clippy::needless_pass_by_value)] // Error adapters consume Result errors.
fn tool_error(error: SearchError) -> ToolError {
    let category = match error {
        SearchError::SearchCapacityExceeded { .. } => ErrorCategory::Limit,
        _ => ErrorCategory::Validation,
    };
    ToolError::try_new(
        error.code(),
        category,
        false,
        error.code(),
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

fn cancelled() -> ToolError {
    ToolError::try_new(
        "search_cancelled",
        ErrorCategory::Cancellation,
        false,
        "search cancelled",
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}
