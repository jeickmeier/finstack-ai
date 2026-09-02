use std::sync::Arc;
use std::time::Duration;

use finstack_ai::runtime::artifact::ArtifactStore;
use finstack_ai::runtime::ports::journal::JournalStore;
use finstack_ai::runtime::ports::middleware::Middleware;
use finstack_ai::runtime::ports::model::{ModelName, ModelSettings};
use finstack_ai::runtime::ports::tool::Toolset;
use finstack_ai::{
    Agent as FacadeAgent, CapabilitySpec, ChildRunPolicy, LinkedAgentPorts, LinkedCommon,
};
use finstack_ai_kernel::{CapabilityId, ComponentId, ComponentRef, RawJson, SessionId, Version};
use finstack_ai_middleware_document_ingest::DocumentIngestMiddleware;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_tools_document::DocumentToolset;
use wasm_bindgen::prelude::*;

use crate::document_store::MemoryArtifactStore;

use super::agent::Agent;
use super::errors::{agent_error, configuration_error, session_error};
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
/// `Arc<dyn ArtifactStore>` across attachment
/// staging, `DocumentToolset`, and `DocumentIngestMiddleware` is required so
/// all three resolve the exact same staged `ArtifactRef` (mirrors
/// `finstack-ai-python`'s `document_ingest_ports`).
struct DocumentIngestPorts {
    artifact_store: Arc<dyn ArtifactStore>,
    toolset: (ComponentRef, Arc<dyn Toolset>),
    middleware: (ComponentRef, Arc<dyn Middleware>),
}

fn document_ingest_ports() -> Result<DocumentIngestPorts, JsValue> {
    let artifact_store = Arc::new(MemoryArtifactStore::document());
    let dyn_store: Arc<dyn ArtifactStore> = Arc::clone(&artifact_store) as Arc<dyn ArtifactStore>;
    let toolset = DocumentToolset::try_new(Arc::clone(&dyn_store))
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    let middleware = DocumentIngestMiddleware::try_new(Arc::clone(&dyn_store))
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    Ok(DocumentIngestPorts {
        artifact_store: dyn_store,
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
    model: Arc<dyn finstack_ai::runtime::ports::model::Model>,
    mut toolsets: Vec<(
        ComponentRef,
        Arc<dyn finstack_ai::runtime::ports::tool::Toolset>,
    )>,
    context_providers: Vec<(
        ComponentRef,
        Arc<dyn finstack_ai::runtime::ports::context::ContextProvider>,
    )>,
    mut middleware: Vec<(
        ComponentRef,
        Arc<dyn finstack_ai::runtime::ports::middleware::Middleware>,
    )>,
    observers: Vec<(
        ComponentRef,
        Arc<dyn finstack_ai::runtime::ports::observer::Observer>,
    )>,
    instruction: Option<String>,
    store: Option<Arc<dyn JournalStore>>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
    approval_grant: finstack_ai::ApprovalGrantMode,
    output_schema_json: Option<String>,
    child_runs_json: Option<String>,
) -> Result<Agent, JsValue> {
    let output_schema = output_schema_json
        .map(|schema| {
            RawJson::parse(schema)
                .map_err(|error| agent_error(&configuration_error(error.to_string()), None))
        })
        .transpose()?;
    let child_runs = child_runs_json
        .map(|policy| {
            serde_json::from_str::<ChildRunPolicy>(&policy)
                .map_err(|error| agent_error(&configuration_error(error.to_string()), None))
        })
        .transpose()?
        .unwrap_or_default();
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
                output_schema,
            },
            child_runs,
            approval_grant,
            // Native-only toolsets. `Agent.create` exposes no field for
            // them, and the facade rejects a non-`None` value on
            // `wasm-host` with `agent_run_unsupported_plan`.
            openrouter_media: None,
            video_compose: None,
            media_pipeline: None,
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
    let snapshot = finstack_ai::runtime::session::inspect_session(store, session_id)
        .await
        .map_err(|error| session_error(&error))?;
    inspect_object(&snapshot)
}

fn inspect_object(
    snapshot: &finstack_ai::runtime::session::SessionInspectSnapshot,
) -> Result<JsValue, JsValue> {
    let object = js_sys::Object::new();
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("sessionId"),
        &JsValue::from_str(&snapshot.session_id.to_string()),
    )?;
    let head_sequence = super::errors::js_safe_integer(snapshot.head_sequence, "head sequence")
        .map_err(|error| agent_error(&error, None))?;
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("headSequence"),
        &JsValue::from(head_sequence),
    )?;
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("phase"),
        &JsValue::from_str(snapshot.phase.as_str()),
    )?;
    if let Some(result_text) = snapshot.result_text.as_deref() {
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("resultText"),
            &JsValue::from_str(result_text),
        )?;
    }
    if let Some(last_record_kind) = snapshot.last_record_kind.as_deref() {
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("lastRecordKind"),
            &JsValue::from_str(last_record_kind),
        )?;
    }
    Ok(object.into())
}
