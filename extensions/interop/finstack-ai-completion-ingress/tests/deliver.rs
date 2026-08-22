//! End-to-end delivery tests: mint a token against a real deferred effect on a
//! real journal-backed run, then deliver completions through the router.

#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod helpers {
    //! Scripted-deferral setup ported from
    //! `crates/finstack-ai-test/tests/deferred_bridge/helpers/mod.rs` and the
    //! `RecordingSink` from `crates/finstack-ai-test/tests/crash_prefix/helpers/mod.rs`.

    use std::sync::Arc;
    use std::time::Duration;

    use finstack_ai::{Agent, AgentRun, AgentRunRequest};
    use finstack_ai_kernel::{
        AgentId, BundleId, ComponentId, ComponentRef, ContentBlock, EffectId, ErrorCategory,
        ErrorDescriptor, ExternalEffectOutcome, ExternalHandleRef, Metadata, PrincipalRef, RawJson,
        ReconciliationPolicy, RetrySafety, RunPhase, RunSecurityContext, TextBlock,
        ToolExecutionMode, ToolId, Version,
    };
    use finstack_ai_runtime::audit::{
        SecurityAuditError, SecurityAuditEvent, SecurityAuditHealth, SecurityAuditReceipt,
        SecurityAuditSink,
    };
    use finstack_ai_runtime::commit::CommitCoordinator;
    use finstack_ai_runtime::ports::PortFuture;
    use finstack_ai_runtime::ports::journal::JournalStore;
    use finstack_ai_runtime::ports::model::{
        ApprovalMetadata, ApprovalRequirement, Model, ModelContextProfile, ModelName,
        ModelResponse, ModelStreamItem, ModelToolCall, SecretString, SideEffectClass, TextDelta,
        TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta, ToolDeferralSupport, ToolSpec,
    };
    use finstack_ai_runtime::ports::tool::{ToolDeferral, ToolStreamItem, Toolset};
    use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
    use finstack_ai_test::{
        ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction,
        ScriptedToolPlan, ScriptedToolset,
    };

    use finstack_ai_completion_ingress::CompletionIngressConfig;

    pub(crate) const VERSION: Version = Version {
        major: 0,
        minor: 0,
        patch: 1,
    };

    pub(crate) fn profile() -> ModelContextProfile {
        ModelContextProfile {
            provider: Arc::from("scripted"),
            model: ModelName::try_new("preview-1").expect("model name"),
            hard_input_bytes: 1_048_576,
            context_window_tokens: 1_048_576,
            max_output_tokens: 256,
            reserved_output_tokens: 256,
            provider_overhead_tokens: 32,
            estimator: TokenEstimatorRef {
                id: Arc::from("bytes-upper-bound"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        }
    }

    pub(crate) fn completed(text: &str) -> ScriptedModelPlan {
        ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from(text),
                }))),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                    assistant_content: Arc::from([ContentBlock::Text(
                        TextBlock::try_new(text).expect("assistant text"),
                    )]),
                    tool_calls: Arc::from([]),
                    usage: finstack_ai_kernel::Usage::empty(),
                    provider_ids: finstack_ai_kernel::ProviderIds::empty(),
                    completion_id: Arc::from("preview-completion"),
                    continuation_state: None,
                }))),
            ],
        }
    }

    pub(crate) fn echo_tool_call() -> ScriptedModelPlan {
        let arguments = RawJson::parse(br#"{"value":1}"#).expect("arguments");
        ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                    index: 0,
                    name: Some(Arc::from("echo")),
                    arguments_delta: Arc::from(arguments.as_str()),
                    provider_call_id: Some(Arc::from("call-echo")),
                }))),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                    assistant_content: Arc::from([]),
                    tool_calls: Arc::from([ModelToolCall {
                        name: Arc::from("echo"),
                        arguments,
                        provider_call_id: Some(Arc::from("call-echo")),
                    }]),
                    usage: finstack_ai_kernel::Usage::empty(),
                    provider_ids: finstack_ai_kernel::ProviderIds::empty(),
                    completion_id: Arc::from("echo-tool-completion"),
                    continuation_state: None,
                }))),
            ],
        }
    }

    pub(crate) fn echo_tool_spec() -> ToolSpec {
        ToolSpec {
            id: ToolId::parse("finstack.tools.echo").expect("tool id"),
            model_name: Arc::from("echo"),
            title: Arc::from("echo"),
            description: Arc::from("scripted deterministic tool"),
            input_schema: RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"value":{"type":"integer"}},"required":["value"],"type":"object"}"#,
            )
            .expect("input schema"),
            output_schema: Some(
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"ok":{"type":"boolean"},"value":{"type":"integer"}},"required":["ok","value"],"type":"object"}"#,
                )
                .expect("output schema"),
            ),
            execution: ToolExecutionMode::Parallel,
            side_effect: SideEffectClass::ReadOnly,
            retry_safety: RetrySafety::SafeToRetry,
            approval: ApprovalMetadata {
                requirement: ApprovalRequirement::NotRequired,
                reason: None,
                attributes: Metadata::empty(),
            },
            max_result_bytes: 4_096,
            metadata: Metadata::empty(),
            deferral: ToolDeferralSupport::Supported,
        }
    }

    pub(crate) fn security() -> RunSecurityContext {
        RunSecurityContext::try_new(
            "tenant-preview",
            PrincipalRef::try_new("preview-tests", "developer", Some("tenant-preview"))
                .expect("principal"),
            "local",
            "test",
            "preview-policy-v1",
            "preview-decision-v1",
            None,
        )
        .expect("security")
    }

    pub(crate) fn agent_request(input: &str) -> AgentRunRequest {
        AgentRunRequest::try_new(
            ModelName::try_new("preview-1").expect("model name"),
            input,
            security(),
        )
        .expect("request")
    }

    pub(crate) fn memory_store() -> Arc<dyn JournalStore> {
        Arc::new(
            MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 8,
                batches_per_session: 256,
                records_per_session: 1024,
                snapshot_bytes: 64 * 1024,
            })
            .expect("store"),
        )
    }

    include!("support/fixtures.rs");

    /// A `Failed` outcome body, serialized through the kernel's own serde so the
    /// wire shape can never drift from the deserializer the ingress uses.
    pub(crate) fn failed_outcome_body_with_message(message: &str) -> Vec<u8> {
        let outcome = ExternalEffectOutcome::Failed {
            error: ErrorDescriptor::new("provider_failed", message, ErrorCategory::Model, true)
                .expect("error"),
        };
        let outcome_value = serde_json::to_value(&outcome).expect("outcome");
        serde_json::to_vec(&serde_json::json!({ "outcome": outcome_value })).expect("body")
    }

    pub(crate) fn failed_outcome_body() -> Vec<u8> {
        failed_outcome_body_with_message("provider failed")
    }

    /// Start a run that parks on a deferred tool call and return its effect id.
    pub(crate) async fn deferred_tool_parent() -> (Agent, AgentRun, Arc<dyn JournalStore>, EffectId)
    {
        let toolset: Arc<dyn Toolset> = Arc::new(ScriptedToolset::new(
            Arc::from([echo_tool_spec()]),
            vec![ScriptedToolPlan {
                panic_on_call: None,
                actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(
                    ToolDeferral {
                        handle: ExternalHandleRef::try_new(
                            ComponentId::parse("finstack.tool.scripted").expect("component"),
                            "job-1",
                            RawJson::parse(b"{}").expect("metadata"),
                        )
                        .expect("handle"),
                        reconciliation: ReconciliationPolicy::CallbackOrPoll,
                        next_poll_at: None,
                        expires_at: None,
                    },
                )))],
            }],
        ));
        let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
            profile(),
            vec![echo_tool_call(), completed("unused parent retry")],
        ));
        let store = memory_store();
        let agent = Agent::builder(
            AgentId::parse("test.agent.deferred-bridge").expect("agent"),
            BundleId::parse("test.bundle.deferred-bridge").expect("bundle"),
            (
                ComponentRef::new(
                    ComponentId::parse("test.model.deferred-bridge").expect("model"),
                    Some(VERSION),
                ),
                model,
            ),
            (
                ComponentRef::new(
                    ComponentId::parse("test.store.deferred-bridge").expect("store"),
                    Some(VERSION),
                ),
                Arc::clone(&store),
            ),
        )
        .toolset(
            ComponentRef::new(
                ComponentId::parse("test.tools.echo").expect("toolset"),
                Some(VERSION),
            ),
            toolset,
        )
        .build()
        .await
        .expect("agent");
        let parent = agent
            .start(agent_request("defer the tool"))
            .expect("parent start");
        let effect_id = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(commit) =
                    CommitCoordinator::recover(Arc::clone(&store), parent.locator().session_id)
                        .await
                    && commit.state().phase() == Some(RunPhase::AwaitingExternal)
                    && let Some(batch) = commit.state().active_tool_batch()
                    && let Some(call) = batch.calls.first()
                    && let finstack_ai_kernel::ActiveToolCallStatus::Requested {
                        deferred: Some(deferred),
                        ..
                    } = &call.status
                {
                    return deferred.effect_id;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("deferred tool effect");
        (agent, parent, store, effect_id)
    }
}

use std::sync::Arc;
use std::time::Duration;

use finstack_ai_completion_ingress::{CompletionGrant, CompletionIngress, IngressError};
use finstack_ai_kernel::{
    AuthorizationEvidence, EffectId, LaneId, OperationLocator, RunId, SessionId, Timestamp,
};
use finstack_ai_runtime::audit::{SecurityAuditGate, SecurityAuditSink};
use finstack_ai_runtime::ingress::ExternalRouteOutcome;
use finstack_ai_runtime::ports::journal::IdempotencyHorizon;

fn ts(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("timestamp")
}

/// Wall-clock base for delivery timestamps.
///
/// Deliveries append journal records, and the journal enforces timestamp
/// monotonicity against the records the live run already wrote at the real
/// clock — so an epoch-relative literal like `ts(1_000)` routes as
/// `Unavailable { reason_code: "runtime" }`. Every delivery timestamp is
/// therefore derived from `now`, matching what `AgentRun::complete_external`
/// does with `NativeIds::now()`.
fn base_ms() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_millis();
    i64::try_from(now).expect("clock fits i64") + 5_000
}

fn grant_for(
    parent: &finstack_ai::AgentRun,
    effect_id: EffectId,
    expires_at: Timestamp,
) -> CompletionGrant {
    let security = helpers::security();
    CompletionGrant {
        locator: parent.locator().clone(),
        principal: security.principal().clone(),
        authorization: AuthorizationEvidence::try_new(
            security.authorization_policy_version(),
            security.authorization_decision_id(),
        )
        .expect("auth"),
        effect_id,
        expires_at,
    }
}

/// A validly-signed grant for a run that does not exist.
fn phantom_grant(expires_at: Timestamp) -> CompletionGrant {
    let security = helpers::security();
    CompletionGrant {
        locator: OperationLocator::try_new(
            "tenant-preview",
            SessionId::parse("0192dead-beef-7000-8000-0000000000a1").expect("session"),
            LaneId::parse("0192dead-beef-7000-8000-0000000000a2").expect("lane"),
            RunId::parse("0192dead-beef-7000-8000-0000000000a3").expect("run"),
        )
        .expect("locator"),
        principal: security.principal().clone(),
        authorization: AuthorizationEvidence::try_new(
            security.authorization_policy_version(),
            security.authorization_decision_id(),
        )
        .expect("auth"),
        effect_id: EffectId::parse("0192dead-beef-7000-8000-0000000000a4").expect("effect"),
        expires_at,
    }
}

async fn recording_gate(sink: &Arc<helpers::RecordingSink>) -> Arc<SecurityAuditGate> {
    SecurityAuditGate::enable(
        Some(Arc::clone(sink) as Arc<dyn SecurityAuditSink>),
        Duration::from_millis(100),
    )
    .await
    .expect("gate")
}

#[tokio::test]
async fn delivery_commits_then_replays_idempotently_then_conflicts() {
    let (_agent, parent, store, effect_id) = helpers::deferred_tool_parent().await;
    let sink = Arc::new(helpers::RecordingSink::default());
    let gate = recording_gate(&sink).await;
    let ingress = CompletionIngress::try_new(store, gate, helpers::config()).expect("ingress");
    let base = base_ms();
    let token = ingress
        .mint(&grant_for(&parent, effect_id, ts(base + 60_000)))
        .expect("mint");
    let body = helpers::failed_outcome_body();

    let first = Box::pin(ingress.deliver(token.as_str(), &body, ts(base)))
        .await
        .expect("first");
    assert!(
        matches!(first, ExternalRouteOutcome::Committed(_)),
        "expected Committed, got {first:?}; body was {}",
        String::from_utf8_lossy(&body)
    );

    let replay = Box::pin(ingress.deliver(token.as_str(), &body, ts(base + 1)))
        .await
        .expect("replay");
    assert!(
        matches!(replay, ExternalRouteOutcome::Idempotent { .. }),
        "expected Idempotent, got {replay:?}"
    );

    let conflicting = helpers::failed_outcome_body_with_message("different failure");
    let conflict = Box::pin(ingress.deliver(token.as_str(), &conflicting, ts(base + 2)))
        .await
        .expect("conflict routes");
    assert!(
        matches!(conflict, ExternalRouteOutcome::Rejected { .. }),
        "expected Rejected, got {conflict:?}"
    );
}

#[tokio::test]
async fn token_for_unknown_session_is_indistinguishable_from_garbage() {
    let (_agent, _parent, store, _effect_id) = helpers::deferred_tool_parent().await;
    let sink = Arc::new(helpers::RecordingSink::default());
    let gate = recording_gate(&sink).await;
    let ingress = CompletionIngress::try_new(store, gate, helpers::config()).expect("ingress");
    // Valid signature, nonexistent session: the router audits the unknown
    // locator and the caller sees the same opaque error as a garbage token.
    let base = base_ms();
    let token = ingress
        .mint(&phantom_grant(ts(base + 60_000)))
        .expect("mint");
    let body = helpers::failed_outcome_body();
    let unknown = Box::pin(ingress.deliver(token.as_str(), &body, ts(base)))
        .await
        .expect_err("unknown session");
    let garbage = Box::pin(ingress.deliver("fcit1.AAAA.BBBB", &body, ts(base)))
        .await
        .expect_err("garbage");
    assert_eq!(unknown, garbage);
}

#[tokio::test]
async fn horizon_expires_deliveries_regardless_of_token_expiry() {
    let (_agent, parent, store, effect_id) = helpers::deferred_tool_parent().await;
    let sink = Arc::new(helpers::RecordingSink::default());
    let gate = recording_gate(&sink).await;
    let base = base_ms();
    let ingress = CompletionIngress::try_new(store, gate, helpers::config()).expect("ingress");
    // Mint while no horizon is configured (mint refuses grants that outlive a
    // configured horizon), then shrink the horizon under the outstanding
    // token: the token stays valid for another 60 seconds; only the horizon
    // expires this delivery.
    let token = ingress
        .mint(&grant_for(&parent, effect_id, ts(base + 60_000)))
        .expect("mint");
    let ingress = ingress.with_horizon(IdempotencyHorizon {
        expire_at: ts(base - 500),
    });
    let body = helpers::failed_outcome_body();
    let error = Box::pin(ingress.deliver(token.as_str(), &body, ts(base)))
        .await
        .expect_err("past horizon");
    assert_eq!(error, IngressError::Rejected);
    let events = sink.events.lock().expect("lock");
    assert!(
        events
            .iter()
            .any(|event| event.reason_code() == "expired_locator"),
        "expected an expired_locator audit event, got {:?}",
        events
            .iter()
            .map(finstack_ai_runtime::audit::SecurityAuditEvent::reason_code)
            .collect::<Vec<_>>()
    );
}
