//! NFR-PERF-007: construction compiles schemas once; scripted calls do not.

use std::sync::Arc;

use finstack_ai::runtime::{
    AgentId, BundleId, ComponentId, ComponentRef, JournalStore, JsonSchemaToolValidatorCompiler,
    Model, ModelContextProfile, ModelName, ModelResponse, ModelStreamItem, RawJson,
    TokenEstimatorRef, TokenEstimatorSource, Toolset, Version,
};
use finstack_ai::{Agent, AgentRunRequest, PrincipalRef, RunSecurityContext};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
use finstack_ai_tools_calculator::CalculatorToolset;

const VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};
const RUNS: usize = 8;

#[tokio::test]
async fn agent_compiles_tool_and_output_schemas_once_across_scripted_calls() {
    JsonSchemaToolValidatorCompiler::reset_thread_compile_count();
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed(r#"{"ok":true}"#); RUNS],
    ));
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 16,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 8_192,
        })
        .expect("store"),
    );
    let calculator: Arc<dyn Toolset> = Arc::new(CalculatorToolset::try_new().expect("calculator"));
    let output_schema = RawJson::parse(
        br#"{"type":"object","properties":{"ok":{"type":"boolean"}},"additionalProperties":true}"#,
    )
    .expect("output schema");
    let agent = Agent::builder(
        AgentId::parse("test.agent.nfr-perf-007").expect("agent"),
        BundleId::parse("test.bundle.nfr-perf-007").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.nfr-perf-007").expect("model"),
                Some(VERSION),
            ),
            Arc::clone(&model) as Arc<dyn Model>,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.nfr-perf-007").expect("store"),
                Some(VERSION),
            ),
            store,
        ),
    )
    .toolset(
        ComponentRef::new(
            ComponentId::parse("test.tools.calculator").expect("toolset"),
            Some(VERSION),
        ),
        calculator,
    )
    .build()
    .await
    .expect("agent")
    .try_with_output_schema(&output_schema)
    .expect("output schema");
    let compiled_at_construction = JsonSchemaToolValidatorCompiler::thread_compile_count();
    assert!(
        compiled_at_construction >= 2,
        "construction must compile the tool schema and the output schema"
    );

    for index in 0..RUNS {
        agent
            .run(request(&format!("nfr-perf-007 run {index}")))
            .await
            .expect("scripted run");
    }
    assert_eq!(
        JsonSchemaToolValidatorCompiler::thread_compile_count(),
        compiled_at_construction,
        "scripted calls must not compile schemas again"
    );
}

fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: Arc::from("scripted"),
        model: ModelName::try_new("preview-1").expect("model"),
        hard_input_bytes: 1_048_576,
        context_window_tokens: 8_192,
        max_output_tokens: 512,
        reserved_output_tokens: 128,
        provider_overhead_tokens: 32,
        estimator: TokenEstimatorRef {
            id: Arc::from("scripted.utf8"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    }
}

fn completed(text: &str) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(
            ModelResponse {
                assistant_content: Arc::from([finstack_ai::runtime::ContentBlock::Json(
                    finstack_ai::runtime::JsonBlock::new(
                        RawJson::parse(text.as_bytes()).expect("json candidate"),
                    ),
                )]),
                tool_calls: Arc::from([]),
                usage: finstack_ai::runtime::Usage::empty(),
                provider_ids: finstack_ai::runtime::ProviderIds::empty(),
                completion_id: Arc::from("nfr-perf-007-complete"),
                continuation_state: None,
            },
        )))],
    }
}

fn request(input: &str) -> AgentRunRequest {
    AgentRunRequest::try_new(
        ModelName::try_new("preview-1").expect("model name"),
        input,
        RunSecurityContext::try_new(
            "nfr-perf-007",
            PrincipalRef::try_new("issuer", "subject", Some("nfr-perf-007")).expect("principal"),
            "local",
            "test",
            "nfr-perf-007-policy",
            "nfr-perf-007-decision",
            None,
        )
        .expect("security"),
    )
    .expect("request")
}
