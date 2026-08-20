//! Per-call CPU cost of every elicitation call path.
//!
//! Elicitation latency is human-dominated (a call parks the run until a
//! person answers); these benches bound the framework overhead per call.

#![allow(clippy::expect_used, clippy::unwrap_used, missing_docs)]

use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use finstack_ai_kernel::{
    Digest, EffectId, EffectOutputContract, EffectOutputKind, LaneId, Metadata, OperationLocator,
    PrincipalRef, RawJson, RunId, SessionId, ToolBatchId, ToolCallBlock, ToolCallId,
    ToolFailurePolicy, ValidatedToolCall,
};
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, RunCallContext, ToolCallContext, Toolset,
};
use finstack_ai_tools_elicitation::{ElicitationKind, ElicitationToolDef, ElicitationToolset};

fn context() -> ToolCallContext {
    ToolCallContext {
        run: RunCallContext {
            locator: OperationLocator::try_new(
                "tenant-a",
                SessionId::from_bytes([1; 16]),
                LaneId::from_bytes([2; 16]),
                RunId::from_bytes([3; 16]),
            )
            .expect("locator"),
            authorization: AuthorizationContext {
                principal: PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                    .expect("principal"),
                authentication_method: Arc::from("bench"),
                assurance_level: Arc::from("bench"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id: EffectId::from_bytes([4; 16]),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
        },
        tool_batch_id: ToolBatchId::from_bytes([5; 16]),
        tool_call_id: ToolCallId::from_bytes([6; 16]),
    }
}

fn call(toolset: &ElicitationToolset, tool_name: &str, arguments: &[u8]) -> ValidatedToolCall {
    let spec = toolset
        .tools()
        .iter()
        .find(|spec| spec.model_name.as_ref() == tool_name)
        .expect("registered tool")
        .clone();
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            ToolCallId::from_bytes([6; 16]),
            tool_name,
            RawJson::parse(arguments).expect("arguments"),
        )
        .expect("call"),
        tool_id: spec.id.clone(),
        component: None,
        output_contract: EffectOutputContract {
            kind: EffectOutputKind::ToolResult,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"{}"),
        },
        retry_safety: spec.retry_safety,
        deadline: None,
        execution: spec.execution,
        failure_policy: ToolFailurePolicy::ReturnToModel,
    }
}

fn make_toolset() -> ElicitationToolset {
    ElicitationToolset::builder()
        .with_ask_user()
        .tool(ElicitationToolDef {
            name: "confirm_trade_params".into(),
            title: "Confirm trade parameters".into(),
            description: "Ask the operator to confirm trade parameters.".into(),
            prompt: "Please confirm the trade parameters.".into(),
            kind: ElicitationKind::Form,
            response_schema: serde_json::json!({
                "type": "object",
                "properties": {"confirmed": {"type": "boolean"}},
                "required": ["confirmed"]
            }),
        })
        .build()
        .expect("toolset")
}

fn bench_paths(criterion: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let toolset = make_toolset();
    let cases: &[(&str, &str, &[u8])] = &[
        (
            "park_free_text",
            "ask_user",
            br#"{"prompt":"What is the position limit?"}"#,
        ),
        (
            "park_choice",
            "ask_user",
            br#"{"prompt":"Which account?","kind":"choice","options":["cash","margin"]}"#,
        ),
        (
            "park_form",
            "ask_user",
            br#"{"prompt":"Fill in the trade.","kind":"form","response_schema":{"type":"object","properties":{"qty":{"type":"number"}},"required":["qty"]}}"#,
        ),
        (
            "park_typed",
            "confirm_trade_params",
            br#"{"context":"Buy 100 AAPL @ market."}"#,
        ),
        (
            "resume_free_text",
            "ask_user",
            br#"{"prompt":"What is the position limit?","answer":"250k USD"}"#,
        ),
        (
            "resume_typed",
            "confirm_trade_params",
            br#"{"context":"Buy 100 AAPL @ market.","answer":{"confirmed":true}}"#,
        ),
    ];
    for (name, tool_name, arguments) in cases {
        criterion.bench_function(name, |bencher| {
            bencher.iter(|| {
                let result = runtime.block_on(toolset.call(
                    black_box(context()),
                    black_box(call(&toolset, tool_name, arguments)),
                ));
                black_box(result).map(drop).map_err(drop)
            });
        });
    }
    criterion.bench_function("build_toolset", |bencher| {
        bencher.iter(|| black_box(make_toolset()));
    });
}

criterion_group!(benches, bench_paths);
criterion_main!(benches);
