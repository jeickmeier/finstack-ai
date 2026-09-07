//! Shared deterministic evaluation model and agent fixtures.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
use finstack_ai::runtime::ports::{journal::JournalStore, model::*};
use finstack_ai::{Agent, AgentRunRequest};
use finstack_ai_eval::*;
use finstack_ai_kernel::*;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
use std::sync::Arc;
pub(crate) fn spec() -> EvalSpec {
    serde_json::from_str(include_str!(
        "../../../../fixtures/compatibility/eval/v1/valid--spec-minimal.json"
    ))
    .unwrap()
}
pub(crate) fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: Arc::from("scripted"),
        model: ModelName::try_new("eval-1").unwrap(),
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
pub(crate) fn plan(cost: Option<(&str, u64)>) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from("answer"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                assistant_content: Arc::from([ContentBlock::Text(
                    TextBlock::try_new("answer").unwrap(),
                )]),
                tool_calls: Arc::from([]),
                usage: Usage::try_new(
                    Some(10),
                    Some(4),
                    Some(14),
                    cost.map(|(unit, micros)| CostAmount::try_new(unit, micros, "v1").unwrap()),
                    std::collections::BTreeMap::default(),
                )
                .unwrap(),
                provider_ids: ProviderIds::empty(),
                completion_id: Arc::from("completion"),
                continuation_state: None,
            }))),
        ],
    }
}
pub(crate) fn request() -> AgentRunRequest {
    let security = RunSecurityContext::try_new(
        "eval-tenant",
        PrincipalRef::try_new("eval-tests", "developer", Some("eval-tenant")).unwrap(),
        "local",
        "test",
        "policy-v1",
        "decision-v1",
        None,
    )
    .unwrap();
    AgentRunRequest::try_new(ModelName::try_new("eval-1").unwrap(), "input", security).unwrap()
}
pub(crate) async fn setup(
    plans: Vec<ScriptedModelPlan>,
) -> (Agent, Arc<ScriptedModel>, Arc<dyn JournalStore>) {
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 32,
            batches_per_session: 256,
            records_per_session: 4096,
            snapshot_bytes: 262_144,
        })
        .unwrap(),
    );
    setup_on_store(plans, store).await
}
pub(crate) async fn setup_on_store(
    plans: Vec<ScriptedModelPlan>,
    store: Arc<dyn JournalStore>,
) -> (Agent, Arc<ScriptedModel>, Arc<dyn JournalStore>) {
    setup_with_tools(plans, store, None).await
}
pub(crate) async fn setup_with_tools(
    plans: Vec<ScriptedModelPlan>,
    store: Arc<dyn JournalStore>,
    tools: Option<Arc<dyn finstack_ai::runtime::ports::tool::Toolset>>,
) -> (Agent, Arc<ScriptedModel>, Arc<dyn JournalStore>) {
    let unit = plans
        .iter()
        .flat_map(|plan| &plan.actions)
        .find_map(|action| match action {
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(response))) => {
                response.usage.cost().map(|cost| cost.unit().to_owned())
            }
            _ => None,
        });
    let mut limits = RunLimits::empty();
    limits.max_cost = Some(
        CostLimit::try_new(
            unit.unwrap_or_else(|| "USD".to_owned()),
            1_000_000,
            "v1",
            UnknownUsagePolicy::AllowWithinReservedMaximum,
        )
        .unwrap(),
    );
    let model = Arc::new(ScriptedModel::from_plans(profile(), plans));

    let component = |id: &str| {
        ComponentRef::new(
            ComponentId::parse(id).unwrap(),
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
        )
    };
    let model_port: Arc<dyn Model> = model.clone();
    let mut builder = Agent::builder(
        AgentId::parse("test.agent.eval").unwrap(),
        BundleId::parse("test.bundle.eval").unwrap(),
        (component("test.model.eval"), model_port),
        (component("test.store.eval"), Arc::clone(&store)),
    )
    .limits(limits)
    .policy(finstack_ai::RunPolicy {
        child_runs: finstack_ai::ChildRunPolicy::Allow { max_depth: 1 },
        ..finstack_ai::RunPolicy::default()
    });
    if let Some(tools) = tools {
        builder = builder.toolset(component("test.tools.eval"), tools);
    }
    let agent = builder.build().await.unwrap();
    (agent, model, store)
}

#[cfg(feature = "sqlite")]
pub(crate) fn disk_journal(path: &std::path::Path) -> Arc<dyn JournalStore> {
    use finstack_ai_store_sqlite::{
        SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits,
    };
    Arc::new(
        SqliteJournalStore::try_open(SqliteStoreConfig::new(
            path.join("journal.sqlite"),
            SqliteDurability::Durable,
            SqliteStoreLimits {
                sessions: 32,
                batches_per_session: 256,
                records_per_session: 4096,
                snapshot_bytes: 262_144,
            },
        ))
        .unwrap(),
    )
}
