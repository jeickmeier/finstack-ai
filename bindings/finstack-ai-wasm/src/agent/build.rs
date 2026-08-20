use std::sync::Arc;
use std::time::Duration;

use finstack_ai::runtime::{
    ArtifactStore, CommitCoordinator, JournalStore, LoadRequest, Middleware, ModelName,
    ModelSettings, StoreError, Toolset,
};
use finstack_ai::{
    Agent as FacadeAgent, CapabilitySpec, ChildRunPolicy, LinkedAgentPorts, LinkedCommon,
};
use finstack_ai_kernel::{CapabilityId, ComponentId, ComponentRef, RawJson, Version};
use finstack_ai_kernel::{ContentBlock, SessionId, TerminalState};
use finstack_ai_middleware_document_ingest::{AttachmentIndex, DocumentIngestMiddleware};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_tools_document::DocumentToolset;
use wasm_bindgen::prelude::*;

use crate::document_store::DocumentArtifactStore;

use super::agent::Agent;
use super::errors::{agent_error, configuration_error};
use super::request::component;

/// `DocumentIngestMiddleware`'s checked-in invocation version
/// (`INGEST_VERSION` in `finstack-ai-middleware-document-ingest::lib`).
/// Descriptor validation requires the registered `ComponentRef` to match the
/// handle's own reported `(component id, version)` exactly, so this must
/// track that crate's constant rather than the binding's generic
/// `PREVIEW_VERSION`. `DocumentToolset` has no equivalent version check, but
/// the same value is reused for its registration for consistency (mirrors
/// `finstack-ai-python`'s `DOCUMENT_INGEST_VERSION`).
const DOCUMENT_INGEST_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

fn document_ingest_component(id: &str) -> Result<ComponentRef, JsValue> {
    Ok(ComponentRef::new(
        ComponentId::parse(id)
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        Some(DOCUMENT_INGEST_VERSION),
    ))
}

/// Shared artifact store, attachment index, and registered document
/// toolset/middleware ports.
///
/// Constructing these once per `Agent` and sharing the same
/// `Arc<dyn ArtifactStore>` / `Arc<AttachmentIndex>` across attachment
/// staging, `DocumentToolset`, and `DocumentIngestMiddleware` is required so
/// all three resolve the exact same staged `ArtifactRef` (mirrors
/// `finstack-ai-python`'s `document_ingest_ports`).
struct DocumentIngestPorts {
    artifact_store: Arc<dyn ArtifactStore>,
    attachment_index: Arc<AttachmentIndex>,
    toolset: (ComponentRef, Arc<dyn Toolset>),
    middleware: (ComponentRef, Arc<dyn Middleware>),
}

fn document_ingest_ports() -> Result<DocumentIngestPorts, JsValue> {
    let artifact_store = Arc::new(DocumentArtifactStore::default());
    let attachment_index = Arc::new(AttachmentIndex::default());
    let dyn_store: Arc<dyn ArtifactStore> = Arc::clone(&artifact_store) as Arc<dyn ArtifactStore>;
    let toolset = DocumentToolset::try_new()
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?
        .with_artifact_store(Arc::clone(&dyn_store));
    let middleware =
        DocumentIngestMiddleware::try_new(Arc::clone(&dyn_store), Arc::clone(&attachment_index))
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    Ok(DocumentIngestPorts {
        artifact_store: dyn_store,
        attachment_index,
        toolset: (
            document_ingest_component("finstack.tools.document")?,
            Arc::new(toolset) as Arc<dyn Toolset>,
        ),
        middleware: (
            document_ingest_component("finstack.middleware.document-ingest")?,
            Arc::new(middleware) as Arc<dyn Middleware>,
        ),
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "wasm-bindgen create forwards each host handle and capability list distinctly"
)]
pub(super) async fn build_agent(
    model_name: ModelName,
    model_component: ComponentRef,
    model: Arc<dyn finstack_ai::runtime::Model>,
    mut toolsets: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Toolset>)>,
    context_providers: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::ContextProvider>)>,
    mut middleware: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Middleware>)>,
    observers: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Observer>)>,
    instruction: Option<String>,
    store: Option<Arc<dyn JournalStore>>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
    approval_grant: finstack_ai::ApprovalGrantMode,
) -> Result<Agent, JsValue> {
    let document_ingest = document_ingest_ports()?;
    toolsets.push(document_ingest.toolset);
    middleware.push(document_ingest.middleware);
    let (store_component, store) = match store {
        Some(store) => (component("js.store.host")?, store),
        None => {
            let memory = MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 64,
                batches_per_session: 256,
                records_per_session: 4_096,
                snapshot_bytes: 64 * 1_024,
            })
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
            (
                component("js.store.memory")?,
                Arc::new(memory) as Arc<dyn JournalStore>,
            )
        }
    };
    let settings = ModelSettings {
        values: RawJson::parse(b"{}")
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
    };
    let built = FacadeAgent::builder(
        finstack_ai_kernel::AgentId::parse("js.agent.host")
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        finstack_ai_kernel::BundleId::parse("js.bundle.host")
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        (model_component, model),
        (store_component, store),
    )
    .build_linked(
        LinkedCommon {
            instruction,
            capabilities,
            active_capabilities,
            ports: LinkedAgentPorts {
                toolsets,
                context_providers,
                middleware,
                observers,
                artifact_store: Some(Arc::clone(&document_ingest.artifact_store)),
                output_schema: None,
            },
            child_runs: ChildRunPolicy::Deny,
            approval_grant,
        },
        model_name.clone(),
        settings,
        Duration::from_secs(30),
    )
    .await
    .map_err(|error| agent_error(&error, None))?;
    Ok(Agent {
        inner: Arc::new(built.agent),
        model: model_name,
        artifact_store: document_ingest.artifact_store,
        attachment_index: document_ingest.attachment_index,
    })
}

/// Parse the optional JS factory string. Omitted or `per_call` is the
/// default. Unknown values fail closed so hosts cannot invent a mode.
pub(super) fn parse_approval_grant(
    value: Option<&str>,
) -> Result<finstack_ai::ApprovalGrantMode, JsValue> {
    match value {
        None | Some("") | Some("per_call") => Ok(finstack_ai::ApprovalGrantMode::PerCall),
        Some("informed_batch") => Ok(finstack_ai::ApprovalGrantMode::InformedBatch),
        Some(other) => Err(agent_error(
            &configuration_error(format!(
                "approval_grant must be per_call or informed_batch: {other}"
            )),
            None,
        )),
    }
}

pub(super) async fn inspect_session_inner(
    store: Arc<dyn JournalStore>,
    session_id: String,
) -> Result<JsValue, JsValue> {
    let session_id = SessionId::parse(&session_id)
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    let loaded = store
        .load(LoadRequest { session_id })
        .await
        .map_err(store_error_js)?;
    if loaded.head_sequence == 0 && loaded.committed_batches.is_empty() {
        return inspect_object(&session_id, 0, "empty", None, None);
    }
    let recovered = CommitCoordinator::recover(Arc::clone(&store), session_id)
        .await
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    let state = recovered.state();
    let phase = match state.terminal.as_ref() {
        Some(TerminalState::Completed(_)) => "completed",
        Some(TerminalState::Failed(_)) => "failed",
        Some(TerminalState::Cancelled(_)) => "cancelled",
        None if state.phase.is_none() && loaded.head_sequence == 0 => "empty",
        None => "in_progress",
    };
    let result_text = match state.terminal.as_ref() {
        Some(TerminalState::Completed(completed)) => state
            .messages
            .iter()
            .find(|message| message.id() == &completed.result_message_id)
            .map(message_text),
        _ => None,
    };
    let last_record_kind = loaded
        .committed_batches
        .iter()
        .rev()
        .flat_map(|batch| batch.records.iter().rev())
        .next()
        .map(|record| record.body().kind_name().to_owned());
    inspect_object(
        &session_id,
        loaded.head_sequence,
        phase,
        result_text.as_deref(),
        last_record_kind.as_deref(),
    )
}

fn message_text(message: &finstack_ai_kernel::Message) -> String {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect()
}

fn inspect_object(
    session_id: &SessionId,
    head_sequence: u64,
    phase: &str,
    result_text: Option<&str>,
    last_record_kind: Option<&str>,
) -> Result<JsValue, JsValue> {
    let object = js_sys::Object::new();
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("sessionId"),
        &JsValue::from_str(&session_id.to_string()),
    )?;
    #[allow(
        clippy::cast_precision_loss,
        reason = "inspect sequences stay well below the 2^53 JS integer limit"
    )]
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("headSequence"),
        &JsValue::from(head_sequence as f64),
    )?;
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("phase"),
        &JsValue::from_str(phase),
    )?;
    if let Some(result_text) = result_text {
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("resultText"),
            &JsValue::from_str(result_text),
        )?;
    }
    if let Some(last_record_kind) = last_record_kind {
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("lastRecordKind"),
            &JsValue::from_str(last_record_kind),
        )?;
    }
    Ok(object.into())
}

fn store_error_js(error: StoreError) -> JsValue {
    let object = js_sys::Error::new(&error.to_string());
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("name"),
        &JsValue::from_str("FinstackError"),
    );
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("code"),
        &JsValue::from_str(error.code()),
    );
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("retryable"),
        &JsValue::from_bool(false),
    );
    match &error {
        StoreError::Conflict {
            expected_sequence,
            actual_next_sequence,
        } => {
            let _ = js_sys::Reflect::set(
                &object,
                &JsValue::from_str("expectedSequence"),
                &JsValue::from(*expected_sequence),
            );
            let _ = js_sys::Reflect::set(
                &object,
                &JsValue::from_str("actualNextSequence"),
                &JsValue::from(*actual_next_sequence),
            );
        }
        StoreError::Corruption { reason_code }
        | StoreError::InvalidRequest { reason_code }
        | StoreError::Unavailable { reason_code }
        | StoreError::Integrity { reason_code } => {
            let _ = js_sys::Reflect::set(
                &object,
                &JsValue::from_str("reasonCode"),
                &JsValue::from_str(reason_code),
            );
        }
        StoreError::LimitExceeded { resource, limit } => {
            let _ = js_sys::Reflect::set(
                &object,
                &JsValue::from_str("resource"),
                &JsValue::from_str(resource),
            );
            let _ = js_sys::Reflect::set(
                &object,
                &JsValue::from_str("limit"),
                &JsValue::from(u64::try_from(*limit).unwrap_or(u64::MAX)),
            );
        }
        StoreError::AmbiguousAcknowledgement => {}
    }
    object.into()
}
