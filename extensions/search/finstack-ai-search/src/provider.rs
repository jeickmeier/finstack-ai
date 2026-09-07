use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ContentBlock, ErrorCategory, InvocationRecovery, Metadata,
    Sensitivity, TextBlock, Version,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::context::{
    ContextAuthority, ContextCallContext, ContextContribution, ContextError, ContextItem,
    ContextItemKind, ContextOverflowPolicy, ContextProvenance, ContextProvider,
    ContextProviderDescriptor, ContextRequest,
};

use crate::{SearchEngine, SearchError, SearchRequest, configuration_digest};

const COMPONENT: &str = "finstack.search.recall";

/// Optional cross-source recall. Emits untrusted references plus explicit
/// coverage metadata, bounded by the committed context budget. Register this
/// instead of memory recall when composing global search.
#[derive(Clone)]
pub struct SearchContextProvider {
    engine: Arc<SearchEngine>,
    descriptor: ContextProviderDescriptor,
    max_hits: usize,
}

impl SearchContextProvider {
    /// Bind a search engine and recall count; no indexing or embedding occurs.
    ///
    /// # Errors
    /// Rejects invalid hit counts or identity encoding.
    pub fn try_new(engine: Arc<SearchEngine>, max_hits: usize) -> Result<Self, SearchError> {
        if max_hits == 0 || max_hits > engine.config().limits.max_results {
            return Err(SearchError::invalid("recall_hits"));
        }
        let digest =
            configuration_digest("search-recall", &(engine.configuration_digest(), max_hits))?;
        let descriptor = ContextProviderDescriptor {
            invocation: ComponentInvocation {
                component: ComponentId::parse(COMPONENT)
                    .map_err(|_| SearchError::invalid("recall_id"))?,
                version: Version {
                    major: 0,
                    minor: 1,
                    patch: 0,
                },
                configuration_digest: digest,
                recovery: InvocationRecovery::RecomputeSafe,
            },
            trusted_application_instructions: false,
            metadata: Metadata::empty(),
        };
        Ok(Self {
            engine,
            descriptor,
            max_hits,
        })
    }
}

impl ContextProvider for SearchContextProvider {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        let engine = Arc::clone(&self.engine);
        let max_hits = self.max_hits;
        Box::pin(async move {
            if request.session_id != ctx.run.locator.session_id
                || request.lane_id != ctx.run.locator.lane_id
                || request.run_id != ctx.run.locator.run_id
                || ctx.run.locator.tenant_scope != engine.config().scope.tenant
                || ctx
                    .run
                    .authorization
                    .principal
                    .tenant_scope()
                    .is_some_and(|tenant| tenant != engine.config().scope.tenant.as_ref())
            {
                return Err(context_error(
                    "search_scope_denied",
                    ErrorCategory::Validation,
                ));
            }
            let mut text = String::new();
            for block in request.user_input.iter() {
                if let ContentBlock::Text(block) = block {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(block.text());
                    if text.len() > engine.config().limits.max_query_bytes {
                        return Err(context_error("search_query_limit", ErrorCategory::Limit));
                    }
                }
            }
            if text.trim().is_empty() {
                return ContextContribution::try_new(Vec::new(), None::<&str>);
            }
            let mut search = SearchRequest::text(text);
            search.limit = Some(max_hits);
            let response = tokio::select! {
                biased;
                () = ctx.run.cancellation.cancelled() => return Err(context_error("search_cancelled", ErrorCategory::Cancellation)),
                response = engine.search(search) => response,
            };
            let (mut coverage, hits) = match response {
                Ok(response) => (
                    serde_json::json!({"search_coverage":response.outcomes,"results_truncated":response.results_truncated}),
                    response.hits,
                ),
                Err(SearchError::SearchNoSuccessfulSources { outcomes }) => (
                    serde_json::json!({"search_error":"search_no_successful_sources","search_coverage":outcomes}),
                    Vec::new(),
                ),
                Err(error) => return Err(context_error(error.code(), ErrorCategory::Validation)),
            };
            coverage["context_omitted_hits"] = serde_json::json!(0);
            let mut items = vec![item(
                ContextItemKind::Metadata,
                &coverage,
                None,
                Sensitivity::Internal,
            )?];
            for hit in hits {
                let key = hit
                    .reference
                    .key()
                    .map_err(|error| context_error(error.code(), ErrorCategory::Validation))?;
                items.push(item(
                    ContextItemKind::Reference,
                    &hit,
                    Some(key.into()),
                    hit.sensitivity,
                )?);
            }
            apply_budget(items, &request)
        })
    }
}

fn item<T: serde::Serialize>(
    kind: ContextItemKind,
    value: &T,
    reference: Option<Arc<str>>,
    sensitivity: Sensitivity,
) -> Result<ContextItem, ContextError> {
    let text = serde_json::to_string(value)
        .map_err(|_| context_error("search_context_encoding", ErrorCategory::Validation))?;
    let tokens = (text.len() as u64).div_ceil(4).max(1);
    ContextItem::try_new(
        kind,
        vec![ContentBlock::Text(TextBlock::try_new(text).map_err(
            |_| context_error("search_context_text", ErrorCategory::Validation),
        )?)],
        ContextProvenance {
            source_id: COMPONENT.into(),
            source_ref: reference,
            external: true,
        },
        ContextAuthority::Untrusted,
        0,
        tokens,
        sensitivity,
        false,
    )
}

fn apply_budget(
    items: Vec<ContextItem>,
    request: &ContextRequest,
) -> Result<ContextContribution, ContextError> {
    let original_len = items.len();
    let mut accepted = items;
    loop {
        let omitted = original_len.saturating_sub(accepted.len());
        if omitted > 0 {
            let first = accepted
                .first_mut()
                .ok_or_else(|| context_error("context_budget_exceeded", ErrorCategory::Limit))?;
            let Some(ContentBlock::Text(text)) = first.content.first() else {
                return Err(context_error(
                    "search_context_encoding",
                    ErrorCategory::Validation,
                ));
            };
            let mut coverage: serde_json::Value = serde_json::from_str(text.text())
                .map_err(|_| context_error("search_context_encoding", ErrorCategory::Validation))?;
            coverage["context_omitted_hits"] = serde_json::json!(omitted);
            *first = item(
                ContextItemKind::Metadata,
                &coverage,
                None,
                Sensitivity::Internal,
            )?;
        }
        let tokens: u64 = accepted.iter().map(|item| item.estimated_tokens).sum();
        let bytes: u64 = accepted.iter().map(|item| item.bytes).sum();
        if accepted.len() <= request.budget.max_items
            && tokens <= request.budget.max_tokens
            && bytes <= request.budget.max_bytes
        {
            break;
        }
        if accepted.len() <= 1 || request.budget.overflow == ContextOverflowPolicy::Reject {
            return Err(context_error(
                "context_budget_exceeded",
                ErrorCategory::Limit,
            ));
        }
        accepted.pop();
    }
    let digest = configuration_digest("search-context-items", &accepted)
        .map_err(|_| context_error("search_context_encoding", ErrorCategory::Validation))?;
    ContextContribution::try_new(accepted, Some(digest.to_hex()))
}

fn context_error(code: &'static str, category: ErrorCategory) -> ContextError {
    ContextError::try_new(code, category, code, Metadata::empty()).unwrap_or_else(Into::into)
}
