//! An offline approval workflow resumed by an independent process.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;
use std::sync::Arc;

use finstack_ai::durable::{DurableHost, DurableHostBuilder, ResolutionInput};
use finstack_ai::{Agent, AgentId, AgentRunRequest, BundleId};
use finstack_ai_kernel::{
    AuthorizationEvidence, ComponentId, ComponentRef, ContentBlock, Id, IdTag, InteractionKind,
    InteractionRequest, Metadata, OperationLocator, PrincipalRef, ProviderIds, RawJson,
    RetrySafety, RunSecurityContext, TerminalState, TextBlock, Timestamp, ToolExecutionMode,
    ToolId, Usage, Version,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::journal::StoreLimits;
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, Model, ModelCapabilities, ModelContextProfile,
    ModelDescriptor, ModelError, ModelEventStream, ModelName, ModelRequest, ModelResponse,
    ModelStreamItem, ModelTokenEstimate, ModelToolCall, SideEffectClass, TextDelta,
    TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta, ToolDeferralSupport, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{ToolResult, ToolStreamItem, Toolset};
use finstack_ai_runtime::workflow::{WorkflowCheckpoint, WorkflowWait};
use finstack_ai_test::{
    ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction, ScriptedToolPlan,
    ScriptedToolset,
};
use finstack_ai_workflow_hitl::{HitlInboxStore, InteractionStatus, SqliteHitlStore, capture};
use finstack_ai_workflow_worker::{InboxStore, SqliteWorkerStore, WakeIndexStore};

type BoxError = Box<dyn Error + Send + Sync>;
const TENANT: &str = "tenant-a";
const WORKFLOW_KIND: &str = "durable-interaction";
const VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}
fn timestamp(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).unwrap_or(finstack_ai_kernel::UNIX_EPOCH)
}
fn component(name: &str) -> Result<ComponentRef, BoxError> {
    Ok(ComponentRef::new(ComponentId::parse(name)?, Some(VERSION)))
}
fn security() -> Result<RunSecurityContext, BoxError> {
    Ok(RunSecurityContext::try_new(
        TENANT,
        PrincipalRef::try_new("local", "operator", Some(TENANT))?,
        "local",
        "application",
        "example-policy-v1",
        "example-decision-v1",
        None,
    )?)
}
fn limits() -> StoreLimits {
    StoreLimits {
        sessions: 16,
        batches_per_session: 256,
        records_per_session: 2048,
        snapshot_bytes: 64 * 1024,
    }
}
fn profile() -> Result<ModelContextProfile, BoxError> {
    Ok(ModelContextProfile {
        provider: Arc::from("scripted"),
        model: ModelName::try_new("scripted-1")?,
        hard_input_bytes: 2_000_000,
        context_window_tokens: 3_000_000,
        max_output_tokens: 1_000,
        reserved_output_tokens: 1_000,
        provider_overhead_tokens: 0,
        estimator: TokenEstimatorRef {
            id: Arc::from("scripted-bytes-upper-bound"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    })
}

/// Identical definition in both processes: select a response solely from the
/// committed messages supplied by the SDK. No process-stage flag or retained
/// model request counter controls the response.
struct OfflineModel {
    metadata: ScriptedModel,
    profile: ModelContextProfile,
    call: ScriptedModelPlan,
    final_reply: ScriptedModelPlan,
}
impl Model for OfflineModel {
    fn descriptor(&self) -> ModelDescriptor {
        self.metadata.descriptor()
    }
    fn capabilities(&self, model: &ModelName) -> ModelCapabilities {
        self.metadata.capabilities(model)
    }
    fn estimate_input_tokens(
        &self,
        model: &ModelName,
        bytes: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        self.metadata.estimate_input_tokens(model, bytes)
    }
    fn request(&self, request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        let has_result = request
            .draft
            .messages
            .iter()
            .flat_map(finstack_ai_kernel::Message::content)
            .any(|block| matches!(block, ContentBlock::ToolResult(_)));
        let plan = if has_result {
            self.final_reply.clone()
        } else {
            self.call.clone()
        };
        ScriptedModel::from_plans(self.profile.clone(), vec![plan]).request(request)
    }
}
fn model() -> Result<Arc<dyn Model>, BoxError> {
    let arguments = RawJson::parse(br#"{"value":1}"#)?;
    let call = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("echo")),
                arguments_delta: Arc::from(arguments.as_str()),
                provider_call_id: Some(Arc::from("echo-call")),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                assistant_content: Arc::from([]),
                tool_calls: Arc::from([ModelToolCall {
                    name: Arc::from("echo"),
                    arguments,
                    provider_call_id: Some(Arc::from("echo-call")),
                }]),
                usage: Usage::empty(),
                provider_ids: ProviderIds::empty(),
                completion_id: Arc::from("echo-request"),
                continuation_state: None,
            }))),
        ],
    };
    let final_reply = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from("done"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                assistant_content: Arc::from([ContentBlock::Text(TextBlock::try_new("done")?)]),
                tool_calls: Arc::from([]),
                usage: Usage::empty(),
                provider_ids: ProviderIds::empty(),
                completion_id: Arc::from("echo-finished"),
                continuation_state: None,
            }))),
        ],
    };
    Ok(Arc::new(OfflineModel {
        metadata: ScriptedModel::from_plans(profile()?, vec![]),
        profile: profile()?,
        call,
        final_reply,
    }))
}
fn tools() -> Result<Arc<dyn Toolset>, BoxError> {
    let spec = ToolSpec { id: ToolId::parse("finstack.tools.echo")?, model_name: Arc::from("echo"), title: Arc::from("Echo"), description: Arc::from("Return one approved value"), input_schema: RawJson::parse(br#"{"type":"object","properties":{"value":{"type":"integer"}},"required":["value"],"additionalProperties":false}"#)?, output_schema: None, execution: ToolExecutionMode::Parallel, side_effect: SideEffectClass::ReadOnly, retry_safety: RetrySafety::SafeToRetry, approval: ApprovalMetadata { requirement: ApprovalRequirement::Required, reason: Some(Arc::from("Operator must approve this example")), attributes: Metadata::empty() }, max_result_bytes: 4096, metadata: Metadata::empty(), deferral: ToolDeferralSupport::Never };
    Ok(Arc::new(ScriptedToolset::new(
        Arc::from([spec]),
        vec![ScriptedToolPlan {
            panic_on_call: None,
            actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Completed(
                ToolResult {
                    output: RawJson::parse(br#"{"ok":true,"value":1}"#)?,
                    is_error: false,
                },
            )))],
        }],
    )))
}
async fn host(path: &Path) -> Result<DurableHost, BoxError> {
    let builder = DurableHostBuilder::try_open(TENANT, path, limits())?;
    let agent = Agent::builder(
        AgentId::parse("example.approval")?,
        BundleId::parse("example.approval.bundle")?,
        (component("example.model")?, model()?),
        (component("example.journal")?, builder.journal_store()),
    )
    .toolset(component("example.echo")?, tools()?)
    .build()
    .await?;
    Ok(builder.register(WORKFLOW_KIND, agent).await?.build()?)
}

async fn park(path: &Path) -> Result<(), BoxError> {
    let host = host(path).await?;
    host.start(
        WORKFLOW_KIND,
        AgentRunRequest::try_new(
            ModelName::try_new("scripted-1")?,
            "Echo the value after approval",
            security()?,
        )?,
    )
    .await?;
    let tick = host.tick().await?;
    if tick.failures != 0 || tick.sessions_reparked != 1 || host.pending()?.len() != 1 {
        return Err("expected one durable approval wait".into());
    }
    println!("approval parked; terminating first process");
    // The parent launches a new executable after this process exits. No Rust
    // owner or application controller is carried across the boundary.
    std::process::exit(0)
}
async fn resume(path: &Path) -> Result<(), BoxError> {
    let host = host(path).await?;
    let pending = host
        .pending()?
        .into_iter()
        .next()
        .ok_or("pending interaction missing after restart")?;
    let locator =
        OperationLocator::try_new(TENANT, pending.session_id, pending.lane_id, pending.run_id)?;
    let accepted = security()?;
    host.resolve(
        &pending.interaction_id,
        ResolutionInput {
            resolution_id: Arc::from("approved-on-restart"),
            principal: accepted.principal().clone(),
            evidence: AuthorizationEvidence::try_new(
                accepted.authorization_policy_version(),
                accepted.authorization_decision_id(),
            )?,
            payload: RawJson::parse(br#"{"approved":true}"#)?,
            note: None,
        },
    )?;
    let tick = host.tick().await?;
    let inspected = host.inspect(&locator).await?;
    if tick.failures != 0
        || tick.sessions_resumed != 1
        || !matches!(
            inspected.state.terminal(),
            Some(TerminalState::Completed(_))
        )
    {
        return Err("approved run did not complete after restart".into());
    }
    let inbox = SqliteHitlStore::try_open(path)?;
    let adapters = SqliteWorkerStore::try_open(path)?;
    if inbox
        .load(TENANT, &pending.interaction_id)?
        .is_none_or(|row| row.status != InteractionStatus::Accepted)
    {
        return Err("expected authoritative Accepted delivery outcome".into());
    }
    if !adapters.load_tenant(TENANT)?.is_empty()
        || InboxStore::load(
            &adapters,
            TENANT,
            locator.session_id,
            &pending.interaction_id,
        )?
        .is_some()
    {
        return Err("expected wake and inbox cleanup".into());
    }
    if host.sweep_interactions()?.reconciled != 0 {
        return Err("sweep changed an accepted delivery".into());
    }
    println!(
        "durable interaction {} resolved and run completed after restart",
        pending.interaction_id
    );
    check_abandoned_interaction(&inbox, &host)?;
    host.shutdown().await;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), BoxError> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [phase, path] if phase == "--park" => park(Path::new(path)).await,
        [phase, path] if phase == "--resume" => resume(Path::new(path)).await,
        [] => {
            let directory = tempfile::tempdir()?;
            let path = directory.path().join("durable.sqlite");
            let executable = std::env::current_exe()?;
            for phase in ["--park", "--resume"] {
                if !std::process::Command::new(&executable)
                    .arg(phase)
                    .arg(&path)
                    .status()?
                    .success()
                {
                    return Err(format!("durable child failed: {phase}").into());
                }
            }
            Ok(())
        }
        _ => Err("usage: durable-interaction [--park|--resume sqlite-path]".into()),
    }
}
/// Model an inbox hint retained after an unrelated interaction's wake was removed.
fn check_abandoned_interaction(
    inbox: &dyn HitlInboxStore,
    host: &DurableHost,
) -> Result<(), BoxError> {
    let interaction_id = id(900);
    let version = Version {
        major: 1,
        minor: 0,
        patch: 0,
    };
    let request = InteractionRequest::try_new(
        1,
        interaction_id,
        id(901),
        InteractionKind::Approval,
        vec![ContentBlock::Text(TextBlock::try_new(
            "Approve abandoned work",
        )?)],
        RawJson::parse(
            r#"{"type":"object","properties":{"approved":{"type":"boolean"}},"required":["approved"],"additionalProperties":false}"#,
        )?,
        ComponentRef::new(
            ComponentId::parse("finstack.policy.approval")?,
            Some(version),
        ),
        version,
        None,
        None,
        false,
        Metadata::empty(),
    )?;
    capture(
        inbox,
        &WorkflowCheckpoint {
            tenant_scope: Arc::from(TENANT),
            session_id: id(902),
            lane_id: id(903),
            run_id: id(904),
            last_applied_seq: 1,
            external_handles: BTreeMap::new(),
        },
        &WorkflowWait::Interaction {
            interaction_id,
            request,
        },
        &security()?,
        timestamp(201_000),
    )?;
    if host.sweep_interactions()?.reconciled != 1
        || inbox
            .load(TENANT, &interaction_id.to_canonical_string())?
            .is_none_or(|row| row.status != InteractionStatus::Closed)
    {
        return Err("expected abandoned interaction to reconcile to Closed".into());
    }
    println!("abandoned interaction reconciled to Closed");
    Ok(())
}
