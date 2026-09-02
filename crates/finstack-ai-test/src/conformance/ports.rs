//! Public conformance functions for the six primary extension ports.
//!
//! Callers supply ordinary public request values. The helpers never reach into
//! registry or runtime internals, so the same cases can be copied into a leaf
//! provider, toolset, store, middleware, context, or observer crate.

use std::fmt;
use std::sync::Arc;

use finstack_ai_kernel::{AppendRequest, CommittedBatch, RunEvent, ValidatedToolCall};
use finstack_ai_runtime::ports::context::{
    ContextCallContext, ContextContribution, ContextProvider, ContextRequest,
};
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::ports::middleware::{
    Middleware, MiddlewareContext, StageInput, StageOutcome, validate_stage_outcome,
};
use finstack_ai_runtime::ports::model::{
    AssembledModelStream, Model, ModelName, ModelRequest, ModelStreamAssembler, ModelStreamLimits,
    ModelTerminal,
};
use finstack_ai_runtime::ports::observer::Observer;
use finstack_ai_runtime::ports::tool::{
    AssembledToolStream, ToolCallContext, ToolStreamAssembler, ToolStreamLimits, Toolset,
};

/// Published suite version printed on every port-conformance failure.
pub const PORT_CONFORMANCE_SUITE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// One precise extension-contract failure.
///
/// Display names the port, stable contract id, and suite version so a
/// failed run is enough to identify the violated contract (port-conformance failure case).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortConformanceFailure {
    /// Stable primary-port name.
    pub port: &'static str,
    /// Stable contract label suitable for assertions and CI diagnostics.
    pub contract: &'static str,
    /// Suite version that defined `contract`.
    pub suite_version: &'static str,
    /// Safe human-readable explanation.
    pub detail: String,
}

impl PortConformanceFailure {
    pub(crate) fn new(
        port: &'static str,
        contract: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            port,
            contract,
            suite_version: PORT_CONFORMANCE_SUITE_VERSION,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for PortConformanceFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} contract `{}` suite {} violated: {}",
            self.port, self.contract, self.suite_version, self.detail
        )
    }
}

impl std::error::Error for PortConformanceFailure {}

/// Public inputs for one complete Model-port conformance call.
#[derive(Debug, Clone)]
pub struct ModelConformanceCase {
    /// Descriptor model selected for capability and estimator checks.
    pub model: ModelName,
    /// Fully normalized public request.
    pub request: ModelRequest,
    /// Expected normalized terminal result.
    pub expected_terminal: ModelTerminal,
    /// Explicit stream bounds.
    pub stream_limits: ModelStreamLimits,
}

/// Run one deterministic Model-port conformance case.
///
/// # Errors
///
/// Names the exact descriptor, capability, estimator, request, stream, or
/// stability contract that failed.
pub async fn check_model_conformance(
    model: &dyn Model,
    case: ModelConformanceCase,
) -> Result<AssembledModelStream, PortConformanceFailure> {
    let descriptor = model.descriptor();
    ensure(
        descriptor
            .models
            .iter()
            .any(|candidate| candidate == &case.model),
        "Model",
        "model.descriptor.selected_model",
        "selected model is absent from the immutable descriptor",
    )?;
    let capabilities = model.capabilities(&case.model);
    ensure(
        capabilities.context_profile.provider == descriptor.provider
            && capabilities.context_profile.model == case.model,
        "Model",
        "model.capabilities.context_profile",
        "capability profile does not match the selected descriptor model",
    )?;
    ensure(
        case.request.draft.model == case.model,
        "Model",
        "model.request.selected_model",
        "request model differs from the conformance case model",
    )?;
    let canonical = case.request.draft.canonical_bytes().map_err(|error| {
        PortConformanceFailure::new("Model", "model.request.canonical", error.to_string())
    })?;
    let first_estimate = model
        .estimate_input_tokens(&case.model, &canonical)
        .map_err(|error| {
            PortConformanceFailure::new(
                "Model",
                "model.estimator.accepts_request",
                error.to_string(),
            )
        })?;
    let second_estimate = model
        .estimate_input_tokens(&case.model, &canonical)
        .map_err(|error| {
            PortConformanceFailure::new("Model", "model.estimator.repeatable", error.to_string())
        })?;
    ensure(
        first_estimate == second_estimate,
        "Model",
        "model.estimator.deterministic",
        "equal canonical requests produced unequal token estimates",
    )?;
    ensure(
        first_estimate.estimator == capabilities.context_profile.estimator,
        "Model",
        "model.estimator.identity",
        "estimate did not retain the locked estimator identity",
    )?;

    let stream = model.request(case.request).await.map_err(|error| {
        PortConformanceFailure::new("Model", "model.request.starts", error.to_string())
    })?;
    let assembled = ModelStreamAssembler::new(case.stream_limits)
        .map_err(|error| {
            PortConformanceFailure::new("Model", "model.stream.limits", error.to_string())
        })?
        .assemble(stream)
        .await
        .map_err(|error| {
            PortConformanceFailure::new("Model", "model.stream.normalized", error.to_string())
        })?;
    ensure(
        assembled.terminal == case.expected_terminal,
        "Model",
        "model.stream.expected_terminal",
        "normalized terminal differs from the expected public result",
    )?;
    ensure(
        model.descriptor() == descriptor && model.capabilities(&case.model) == capabilities,
        "Model",
        "model.descriptor.stable",
        "descriptor or capabilities mutated after request execution",
    )?;
    Ok(assembled)
}

/// Public inputs for one complete Toolset-port conformance call.
#[derive(Debug, Clone)]
pub struct ToolsetConformanceCase {
    /// Immutable committed call context.
    pub context: ToolCallContext,
    /// Validated public call plan.
    pub call: ValidatedToolCall,
    /// Expected normalized result.
    pub expected: AssembledToolStream,
    /// Explicit stream bounds.
    pub stream_limits: ToolStreamLimits,
    /// Per-tool result ceiling.
    pub max_result_bytes: u64,
}

/// Run one deterministic Toolset-port conformance case.
///
/// # Errors
///
/// Names the exact descriptor, registration, call, stream, result, or
/// stability contract that failed.
pub async fn check_toolset_conformance(
    toolset: &dyn Toolset,
    case: ToolsetConformanceCase,
) -> Result<AssembledToolStream, PortConformanceFailure> {
    let descriptor = toolset.descriptor();
    let tools = toolset.tools();
    let Some(spec) = tools.iter().find(|spec| spec.id == case.call.tool_id) else {
        return Err(PortConformanceFailure::new(
            "Toolset",
            "toolset.registration.selected_tool",
            "validated tool id is absent from Toolset::tools",
        ));
    };
    ensure(
        spec.model_name.as_ref() == case.call.call.tool_name(),
        "Toolset",
        "toolset.registration.model_name",
        "validated call name differs from the registered model-visible name",
    )?;
    let stream = toolset
        .call(case.context, case.call)
        .await
        .map_err(|error| {
            PortConformanceFailure::new("Toolset", "toolset.call.starts", error.to_string())
        })?;
    let assembled = ToolStreamAssembler::new(case.stream_limits)
        .assemble(stream, None, case.max_result_bytes, spec.deferral)
        .await
        .map_err(|error| {
            PortConformanceFailure::new("Toolset", "toolset.stream.normalized", error.to_string())
        })?;
    ensure(
        assembled == case.expected,
        "Toolset",
        "toolset.stream.expected_result",
        "normalized result differs from the expected public result",
    )?;
    ensure(
        toolset.descriptor() == descriptor && toolset.tools().as_ref() == tools.as_ref(),
        "Toolset",
        "toolset.descriptor.stable",
        "descriptor or tool specifications mutated after execution",
    )?;
    Ok(assembled)
}

/// Public inputs for one `ContextProvider` conformance call.
#[derive(Debug, Clone)]
pub struct ContextConformanceCase {
    /// Immutable committed call context.
    pub context: ContextCallContext,
    /// Bounded public request.
    pub request: ContextRequest,
    /// Expected normalized contribution.
    pub expected: ContextContribution,
}

/// Run one deterministic `ContextProvider` conformance case.
///
/// # Errors
///
/// Names the exact collection, normalization, result, or descriptor contract.
pub async fn check_context_conformance(
    provider: &dyn ContextProvider,
    case: ContextConformanceCase,
) -> Result<ContextContribution, PortConformanceFailure> {
    let descriptor = provider.descriptor();
    let contribution = provider
        .collect(case.context, case.request)
        .await
        .map_err(|error| {
            PortConformanceFailure::new(
                "ContextProvider",
                "context.collect.completed",
                error.to_string(),
            )
        })?;
    contribution.to_raw_json().map_err(|error| {
        PortConformanceFailure::new(
            "ContextProvider",
            "context.contribution.normalized",
            error.to_string(),
        )
    })?;
    ensure(
        contribution == case.expected,
        "ContextProvider",
        "context.contribution.expected",
        "normalized contribution differs from the expected public result",
    )?;
    ensure(
        provider.descriptor() == descriptor,
        "ContextProvider",
        "context.descriptor.stable",
        "descriptor mutated after collection",
    )?;
    Ok(contribution)
}

/// Public inputs for one Middleware conformance call.
#[derive(Debug, Clone)]
pub struct MiddlewareConformanceCase {
    /// Immutable committed call context.
    pub context: MiddlewareContext,
    /// Frozen stage input.
    pub input: StageInput,
    /// Expected normalized outcome.
    pub expected: StageOutcome,
}

/// Run one deterministic Middleware conformance case.
///
/// # Errors
///
/// Names the exact invocation, stage matrix, expected outcome, or descriptor contract.
pub async fn check_middleware_conformance(
    middleware: &dyn Middleware,
    case: MiddlewareConformanceCase,
) -> Result<StageOutcome, PortConformanceFailure> {
    let descriptor = middleware.descriptor();
    let outcome = middleware
        .invoke(case.context, case.input.clone())
        .await
        .map_err(|error| {
            PortConformanceFailure::new(
                "Middleware",
                "middleware.invoke.completed",
                error.to_string(),
            )
        })?;
    validate_stage_outcome(&descriptor, &case.input, &outcome).map_err(|error| {
        PortConformanceFailure::new(
            "Middleware",
            "middleware.outcome.stage_matrix",
            error.to_string(),
        )
    })?;
    ensure(
        outcome == case.expected,
        "Middleware",
        "middleware.outcome.expected",
        "normalized outcome differs from the expected public result",
    )?;
    ensure(
        middleware.descriptor() == descriptor,
        "Middleware",
        "middleware.descriptor.stable",
        "descriptor mutated after invocation",
    )?;
    Ok(outcome)
}

/// Run one deterministic Observer conformance call.
///
/// # Errors
///
/// Names the exact delivery or immutable-descriptor contract that failed.
pub async fn check_observer_conformance(
    observer: &dyn Observer,
    events: Arc<[RunEvent]>,
) -> Result<(), PortConformanceFailure> {
    let descriptor = observer.descriptor();
    observer.observe(events).await.map_err(|error| {
        PortConformanceFailure::new("Observer", "observer.batch.accepted", error.to_string())
    })?;
    ensure(
        observer.descriptor() == descriptor,
        "Observer",
        "observer.descriptor.stable",
        "descriptor mutated after observation",
    )
}

/// Public inputs for one `JournalStore` conformance call.
#[derive(Debug, Clone)]
pub struct JournalStoreConformanceCase {
    /// Fresh idempotent append request.
    pub request: AppendRequest,
    /// Expected first assigned sequence.
    pub expected_first_sequence: u64,
    /// Expected last assigned sequence.
    pub expected_last_sequence: u64,
}

/// Run one deterministic `JournalStore` conformance case, including equal retry.
///
/// # Errors
///
/// Names the exact readiness, atomic append, idempotency, or load contract.
pub async fn check_journal_store_conformance(
    store: &dyn JournalStore,
    case: JournalStoreConformanceCase,
) -> Result<CommittedBatch, PortConformanceFailure> {
    let health = store.health().await.map_err(|error| {
        PortConformanceFailure::new("JournalStore", "store.health.available", error.to_string())
    })?;
    ensure(
        health.ready,
        "JournalStore",
        "store.health.ready",
        "store reported not ready",
    )?;
    let committed = store.append(case.request.clone()).await.map_err(|error| {
        PortConformanceFailure::new("JournalStore", "store.append.atomic", error.to_string())
    })?;
    ensure(
        committed.batch_id == case.request.batch_id()
            && committed.first_sequence == case.expected_first_sequence
            && committed.last_sequence == case.expected_last_sequence,
        "JournalStore",
        "store.append.receipt",
        "committed batch identity or assigned sequence range differs",
    )?;
    let equal_retry = store.append(case.request.clone()).await.map_err(|error| {
        PortConformanceFailure::new(
            "JournalStore",
            "store.append.equal_retry",
            error.to_string(),
        )
    })?;
    ensure(
        equal_retry == committed,
        "JournalStore",
        "store.append.idempotent",
        "equal batch retry returned a different committed result",
    )?;
    let loaded = store
        .load(finstack_ai_runtime::ports::journal::LoadRequest {
            session_id: case.request.session_id(),
        })
        .await
        .map_err(|error| {
            PortConformanceFailure::new("JournalStore", "store.load.session", error.to_string())
        })?;
    ensure(
        loaded.session_id == case.request.session_id()
            && loaded.head_sequence == case.expected_last_sequence
            && loaded
                .committed_batches
                .iter()
                .any(|batch| batch == &committed),
        "JournalStore",
        "store.load.committed_batch",
        "loaded session does not contain the exact committed batch",
    )?;
    Ok(committed)
}

fn ensure(
    condition: bool,
    port: &'static str,
    contract: &'static str,
    detail: &'static str,
) -> Result<(), PortConformanceFailure> {
    if condition {
        Ok(())
    } else {
        Err(PortConformanceFailure::new(port, contract, detail))
    }
}

#[cfg(test)]
mod tests {
    use super::{PORT_CONFORMANCE_SUITE_VERSION, PortConformanceFailure};

    #[test]
    fn failure_names_port_contract_and_suite_version() {
        let failure = PortConformanceFailure::new(
            "Model",
            "model.descriptor.lists_model",
            "deliberate suite failure",
        );
        let text = failure.to_string();
        assert!(text.contains("Model"), "{text}");
        assert!(text.contains("model.descriptor.lists_model"), "{text}");
        assert!(text.contains(PORT_CONFORMANCE_SUITE_VERSION), "{text}");
        assert!(!text.contains("assert failed"), "{text}");
    }
}
