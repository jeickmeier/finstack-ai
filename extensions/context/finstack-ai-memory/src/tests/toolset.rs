use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{
    Digest, EffectId, EffectOutputContract, EffectOutputKind, LaneId, OperationLocator,
    PrincipalRef, RawJson, RunId, SessionId, ToolBatchId, ToolCallBlock, ToolCallId,
    ToolFailurePolicy, UNIX_EPOCH,
};
use finstack_ai_runtime::ports::model::{AuthorizationContext, CancellationSignal, RunCallContext};
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolStreamItem, Toolset};
use futures_util::StreamExt;

use crate::record::MemoryScope;
use crate::store::{InProcessArtifactStore, InProcessMemoryStore, MemoryPage, MemoryStore};
use crate::toolset::{MemoryPolicy, MemoryToolset};

fn toolset_with_policy(policy: MemoryPolicy) -> (MemoryToolset, Arc<InProcessMemoryStore>) {
    let store = Arc::new(InProcessMemoryStore::new());
    let artifact_store = Arc::new(InProcessArtifactStore::default());
    let scope = MemoryScope::try_new("tenant-a").expect("scope");
    let clock: crate::record::MemoryClock = Arc::new(|| UNIX_EPOCH);
    let toolset = MemoryToolset::try_new(
        store.clone() as Arc<dyn MemoryStore>,
        artifact_store,
        scope,
        policy,
        clock,
    )
    .expect("toolset");
    (toolset, store)
}

fn context(effect_id: EffectId) -> ToolCallContext {
    ToolCallContext {
        run: run_context(effect_id),
        tool_batch_id: ToolBatchId::from_bytes([5; 16]),
        tool_call_id: ToolCallId::from_bytes([6; 16]),
    }
}

/// A call context whose committed locator (and principal) belong to
/// `tenant`, for exercising the toolset's tenant re-check.
fn context_for_tenant(effect_id: EffectId, tenant: &str) -> ToolCallContext {
    ToolCallContext {
        run: run_context_for_tenant(effect_id, tenant),
        tool_batch_id: ToolBatchId::from_bytes([5; 16]),
        tool_call_id: ToolCallId::from_bytes([6; 16]),
    }
}

fn run_context(effect_id: EffectId) -> RunCallContext {
    run_context_for_tenant(effect_id, "tenant-a")
}

fn run_context_for_tenant(effect_id: EffectId, tenant: &str) -> RunCallContext {
    RunCallContext {
        relation_depth: 0,
        locator: OperationLocator::try_new(
            tenant,
            SessionId::from_bytes([1; 16]),
            LaneId::from_bytes([2; 16]),
            RunId::from_bytes([3; 16]),
        )
        .expect("locator"),
        authorization: AuthorizationContext {
            principal: PrincipalRef::try_new("issuer", "subject", Some(tenant)).expect("principal"),
            authentication_method: Arc::from("test"),
            assurance_level: Arc::from("test"),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([Arc::from(tenant)]),
            safe_claims: finstack_ai_kernel::Metadata::empty(),
            policy_version: Arc::from("policy-v1"),
            decision_id: Arc::from("decision-v1"),
        },
        effect_id,
        attempt: 1,
        deadline: None,
        budget_scope_id: None,
        cancellation: CancellationSignal::new(),
    }
}

fn validated_call(
    toolset: &MemoryToolset,
    name: &str,
    arguments: &[u8],
) -> finstack_ai_kernel::ValidatedToolCall {
    let specs = toolset.tools_unfiltered();
    let spec = specs
        .iter()
        .find(|spec| spec.model_name.as_ref() == name)
        .expect("spec");
    finstack_ai_kernel::ValidatedToolCall {
        call: ToolCallBlock::try_new(
            ToolCallId::from_bytes([6; 16]),
            name,
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

async fn call_and_extract(
    toolset: &MemoryToolset,
    ctx: ToolCallContext,
    name: &str,
    arguments: &[u8],
) -> finstack_ai_runtime::ports::tool::ToolResult {
    let mut stream = toolset
        .call(ctx, validated_call(toolset, name, arguments))
        .await
        .expect("call");
    match stream.next().await.expect("item").expect("stream") {
        ToolStreamItem::Completed(result) => result,
        other => panic!("unexpected stream item: {other:?}"),
    }
}

#[test]
fn policy_gates_registered_tools() {
    let names = |policy: MemoryPolicy| -> BTreeSet<String> {
        let (toolset, _store) = toolset_with_policy(policy);
        toolset
            .tools()
            .iter()
            .map(|spec| spec.model_name.to_string())
            .collect()
    };

    let read_only = MemoryPolicy {
        read: true,
        write: false,
        manage: false,
    };
    assert_eq!(
        names(read_only),
        BTreeSet::from(["search_memory".to_owned(), "inspect_memory".to_owned()])
    );

    let write_only = MemoryPolicy {
        read: false,
        write: true,
        manage: false,
    };
    assert_eq!(names(write_only), BTreeSet::from(["remember".to_owned()]));

    let manage_only = MemoryPolicy {
        read: false,
        write: false,
        manage: true,
    };
    assert_eq!(
        names(manage_only),
        BTreeSet::from(["forget_memory".to_owned(), "correct_memory".to_owned()])
    );

    assert_eq!(
        names(MemoryPolicy::default()),
        BTreeSet::from([
            "remember".to_owned(),
            "search_memory".to_owned(),
            "inspect_memory".to_owned(),
        ])
    );

    let all_true = MemoryPolicy {
        read: true,
        write: true,
        manage: true,
    };
    assert_eq!(
        names(all_true),
        BTreeSet::from([
            "remember".to_owned(),
            "search_memory".to_owned(),
            "inspect_memory".to_owned(),
            "forget_memory".to_owned(),
            "correct_memory".to_owned(),
        ])
    );
}

#[tokio::test]
async fn policy_is_enforced_at_call_and_reconcile_time() {
    let (toolset, store) = toolset_with_policy(MemoryPolicy::default());
    store
        .put(
            Arc::from("seed"),
            crate::tests::sample_record("m1", "tenant-a"),
        )
        .await
        .expect("seed");
    let args = br#"{"id":"m1"}"#;
    let Err(error) = toolset
        .call(
            context(EffectId::from_bytes([40; 16])),
            validated_call(&toolset, "forget_memory", args),
        )
        .await
    else {
        panic!("disabled direct call must fail");
    };
    assert_eq!(error.code(), crate::toolset::MEMORY_TOOL_POLICY_DENIED);

    let effect = finstack_ai_runtime::ports::tool::PendingToolEffect {
        call: validated_call(&toolset, "forget_memory", args),
    };
    let ctx = finstack_ai_runtime::ports::model::ReconcileContext {
        run: run_context(EffectId::from_bytes([41; 16])),
        original_input_digest: Digest::raw_json(b"{}"),
    };
    let error = toolset
        .reconcile(ctx, effect)
        .await
        .expect_err("disabled reconciliation");
    assert_eq!(error.code(), crate::toolset::MEMORY_TOOL_POLICY_DENIED);
    assert!(
        store
            .get(
                MemoryScope::try_new("tenant-a").expect("scope"),
                crate::record::MemoryId::parse("m1").expect("id"),
            )
            .await
            .expect("get")
            .is_some()
    );
}

#[tokio::test]
async fn remember_is_idempotent_across_replay() {
    let (toolset, store) = toolset_with_policy(MemoryPolicy::default());
    let effect_id = EffectId::from_bytes([9; 16]);
    let args = br#"{"id":"mem-fixed","keywords":["alpha"],"body":"hello world"}"#;

    let first = call_and_extract(&toolset, context(effect_id), "remember", args).await;
    assert!(!first.is_error);
    let second = call_and_extract(&toolset, context(effect_id), "remember", args).await;
    assert!(!second.is_error);

    let listing = store
        .list(
            MemoryScope::try_new("tenant-a").expect("scope"),
            MemoryPage {
                offset: 0,
                limit: 10,
            },
        )
        .await
        .expect("list");
    assert_eq!(listing.records.len(), 1);
}

#[tokio::test]
async fn remember_stages_large_bodies_as_blobs() {
    let (toolset, store) = toolset_with_policy(MemoryPolicy::default());
    let effect_id = EffectId::from_bytes([10; 16]);
    let body = "x".repeat(crate::toolset::INLINE_BODY_MAX_BYTES + 1);
    let args = serde_json::json!({
        "id": "mem-large",
        "keywords": ["alpha"],
        "body": body,
    });
    let result = call_and_extract(
        &toolset,
        context(effect_id),
        "remember",
        serde_json::to_vec(&args).expect("json").as_slice(),
    )
    .await;
    assert!(!result.is_error);

    let record = store
        .get(
            MemoryScope::try_new("tenant-a").expect("scope"),
            crate::record::MemoryId::parse("mem-large").expect("id"),
        )
        .await
        .expect("get")
        .expect("record present");
    assert!(matches!(
        record.body,
        crate::record::MemoryBody::Blob { .. }
    ));
    assert_eq!(record.preview.chars().count(), 256);
}

#[tokio::test]
async fn search_memory_returns_hits_and_never_tombstoned() {
    let (toolset, store) = toolset_with_policy(MemoryPolicy::default());
    let effect_id = EffectId::from_bytes([11; 16]);
    let args = br#"{"id":"mem-searchable","keywords":["widget"],"body":"a widget record"}"#;
    let remember_result = call_and_extract(&toolset, context(effect_id), "remember", args).await;
    assert!(!remember_result.is_error);

    let search_args = br#"{"keywords":["widget"]}"#;
    let hits = call_and_extract(
        &toolset,
        context(EffectId::from_bytes([12; 16])),
        "search_memory",
        search_args,
    )
    .await;
    let value: serde_json::Value = serde_json::from_str(hits.output.as_str()).expect("json");
    assert_eq!(value["hits"].as_array().expect("hits array").len(), 1);

    // Tombstone directly via the store, bypassing the toolset.
    store
        .forget(
            Arc::from("direct-forget"),
            MemoryScope::try_new("tenant-a").expect("scope"),
            crate::record::MemoryId::parse("mem-searchable").expect("id"),
        )
        .await
        .expect("forget");

    let hits_after = call_and_extract(
        &toolset,
        context(EffectId::from_bytes([13; 16])),
        "search_memory",
        search_args,
    )
    .await;
    let value: serde_json::Value = serde_json::from_str(hits_after.output.as_str()).expect("json");
    assert_eq!(value["hits"].as_array().expect("hits array").len(), 0);
}

#[tokio::test]
async fn forget_and_correct_require_ids_and_are_idempotent() {
    let (toolset, store) = toolset_with_policy(MemoryPolicy {
        read: true,
        write: true,
        manage: true,
    });
    let effect_id = EffectId::from_bytes([14; 16]);
    let remember_args = br#"{"id":"mem-forgettable","keywords":["alpha"],"body":"body text"}"#;
    let remember_result =
        call_and_extract(&toolset, context(effect_id), "remember", remember_args).await;
    assert!(!remember_result.is_error);

    let forget_effect = EffectId::from_bytes([15; 16]);
    let forget_args = br#"{"id":"mem-forgettable"}"#;
    let first = call_and_extract(
        &toolset,
        context(forget_effect),
        "forget_memory",
        forget_args,
    )
    .await;
    assert!(!first.is_error);
    let second = call_and_extract(
        &toolset,
        context(forget_effect),
        "forget_memory",
        forget_args,
    )
    .await;
    assert!(!second.is_error);

    let record = store
        .get(
            MemoryScope::try_new("tenant-a").expect("scope"),
            crate::record::MemoryId::parse("mem-forgettable").expect("id"),
        )
        .await
        .expect("get");
    assert!(record.is_none());
}

fn expected_derived_id(tenant: &str, body: &str) -> String {
    let scope = MemoryScope::try_new(tenant).expect("scope");
    let encoded = serde_json_canonicalizer::to_vec(&(scope, body)).expect("canonical identity");
    let digest =
        finstack_ai_kernel::Digest::domain_separated("memory-tool-derived-id", 1, &encoded)
            .expect("digest");
    let hex = digest.to_hex();
    format!("mem-{}", &hex[..16.min(hex.len())])
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn correct_memory_supersedes_and_is_idempotent() {
    let (toolset, store) = toolset_with_policy(MemoryPolicy {
        read: true,
        write: true,
        manage: true,
    });
    let scope = MemoryScope::try_new("tenant-a").expect("scope");

    let remember_effect = EffectId::from_bytes([20; 16]);
    let remember_args = br#"{"id":"mem-old","keywords":["alpha"],"body":"old body text"}"#;
    let remember_result = call_and_extract(
        &toolset,
        context(remember_effect),
        "remember",
        remember_args,
    )
    .await;
    assert!(!remember_result.is_error);

    let correct_effect = EffectId::from_bytes([21; 16]);
    let new_body = "new body text";
    let correct_args = serde_json::json!({
        "old_id": "mem-old",
        "keywords": ["beta"],
        "body": new_body,
    });
    let correct_args_bytes = serde_json::to_vec(&correct_args).expect("json");
    let expected_new_id = expected_derived_id("tenant-a", new_body);

    let first = call_and_extract(
        &toolset,
        context(correct_effect),
        "correct_memory",
        &correct_args_bytes,
    )
    .await;
    assert!(!first.is_error);
    let value: serde_json::Value = serde_json::from_str(first.output.as_str()).expect("json");
    assert_eq!(value["old_id"].as_str(), Some("mem-old"));
    assert_eq!(value["new_id"].as_str(), Some(expected_new_id.as_str()));

    let old_record = store
        .get(
            scope.clone(),
            crate::record::MemoryId::parse("mem-old").expect("id"),
        )
        .await
        .expect("get");
    assert!(old_record.is_none());

    let new_record = store
        .get(
            scope.clone(),
            crate::record::MemoryId::parse(&expected_new_id).expect("id"),
        )
        .await
        .expect("get")
        .expect("new record present");
    assert_eq!(
        new_record
            .supersedes
            .as_ref()
            .map(crate::record::MemoryId::as_str),
        Some("mem-old")
    );

    let before_replay = store
        .list(
            scope.clone(),
            MemoryPage {
                offset: 0,
                limit: 10,
            },
        )
        .await
        .expect("list");
    let total_before = before_replay.total;

    // Replay the same effect id with the same arguments: must succeed
    // (no error) and must not create a duplicate record.
    let replay = call_and_extract(
        &toolset,
        context(correct_effect),
        "correct_memory",
        &correct_args_bytes,
    )
    .await;
    assert!(!replay.is_error);
    let replay_value: serde_json::Value =
        serde_json::from_str(replay.output.as_str()).expect("json");
    assert_eq!(replay_value["old_id"].as_str(), Some("mem-old"));
    assert_eq!(
        replay_value["new_id"].as_str(),
        Some(expected_new_id.as_str())
    );

    let after_replay = store
        .list(
            scope,
            MemoryPage {
                offset: 0,
                limit: 10,
            },
        )
        .await
        .expect("list");
    assert_eq!(after_replay.total, total_before);
}

#[tokio::test]
async fn correct_memory_stages_large_replacement_bodies_as_blobs() {
    let (toolset, store) = toolset_with_policy(MemoryPolicy {
        read: true,
        write: true,
        manage: true,
    });
    let scope = MemoryScope::try_new("tenant-a").expect("scope");

    let remember_effect = EffectId::from_bytes([22; 16]);
    let remember_args = br#"{"id":"mem-old-large","keywords":["alpha"],"body":"old body text"}"#;
    let remember_result = call_and_extract(
        &toolset,
        context(remember_effect),
        "remember",
        remember_args,
    )
    .await;
    assert!(!remember_result.is_error);

    let correct_effect = EffectId::from_bytes([23; 16]);
    let new_body = "y".repeat(crate::toolset::INLINE_BODY_MAX_BYTES + 1);
    let correct_args = serde_json::json!({
        "old_id": "mem-old-large",
        "keywords": ["beta"],
        "body": new_body,
    });
    let expected_new_id = expected_derived_id("tenant-a", &new_body);

    let result = call_and_extract(
        &toolset,
        context(correct_effect),
        "correct_memory",
        serde_json::to_vec(&correct_args).expect("json").as_slice(),
    )
    .await;
    assert!(!result.is_error);

    let new_record = store
        .get(
            scope,
            crate::record::MemoryId::parse(&expected_new_id).expect("id"),
        )
        .await
        .expect("get")
        .expect("new record present");
    assert!(matches!(
        new_record.body,
        crate::record::MemoryBody::Blob { .. }
    ));
    assert_eq!(new_record.preview.chars().count(), 256);
}

#[tokio::test]
async fn call_rejects_a_locator_tenant_other_than_the_configured_one() {
    // The toolset is bound to `tenant-a`; the committed effect belongs to
    // `tenant-b`, so the call must be refused before any store work.
    let (toolset, store) = toolset_with_policy(MemoryPolicy::default());
    let args = br#"{"id":"mem-x","keywords":["alpha"],"body":"body text"}"#;
    let Err(error) = toolset
        .call(
            context_for_tenant(EffectId::from_bytes([30; 16]), "tenant-b"),
            validated_call(&toolset, "remember", args),
        )
        .await
    else {
        panic!("a foreign tenant scope must be rejected");
    };
    assert_eq!(error.code(), crate::toolset::MEMORY_TOOL_INVALID_ARGUMENTS);

    let listing = store
        .list(
            MemoryScope::try_new("tenant-a").expect("scope"),
            MemoryPage {
                offset: 0,
                limit: 10,
            },
        )
        .await
        .expect("list");
    assert_eq!(listing.total, 0);
}

#[tokio::test]
async fn reconcile_rejects_a_locator_tenant_other_than_the_configured_one() {
    let (toolset, store) = toolset_with_policy(MemoryPolicy::default());
    let args = br#"{"id":"mem-x","keywords":["alpha"],"body":"body text"}"#;
    let effect = finstack_ai_runtime::ports::tool::PendingToolEffect {
        call: validated_call(&toolset, "remember", args),
    };
    let ctx = finstack_ai_runtime::ports::model::ReconcileContext {
        run: run_context_for_tenant(EffectId::from_bytes([31; 16]), "tenant-b"),
        original_input_digest: Digest::raw_json(b"{}"),
    };
    let Err(error) = toolset.reconcile(ctx, effect).await else {
        panic!("a foreign tenant scope must be rejected on reconcile");
    };
    assert_eq!(error.code(), crate::toolset::MEMORY_TOOL_INVALID_ARGUMENTS);

    let listing = store
        .list(
            MemoryScope::try_new("tenant-a").expect("scope"),
            MemoryPage {
                offset: 0,
                limit: 10,
            },
        )
        .await
        .expect("list");
    assert_eq!(listing.total, 0);
}

#[tokio::test]
async fn reconcile_replays_a_matching_tenant_call() {
    let (toolset, store) = toolset_with_policy(MemoryPolicy::default());
    let args = br#"{"id":"mem-reconciled","keywords":["alpha"],"body":"body text"}"#;
    let effect = finstack_ai_runtime::ports::tool::PendingToolEffect {
        call: validated_call(&toolset, "remember", args),
    };
    let ctx = finstack_ai_runtime::ports::model::ReconcileContext {
        run: run_context(EffectId::from_bytes([32; 16])),
        original_input_digest: Digest::raw_json(b"{}"),
    };
    let result = toolset.reconcile(ctx, effect).await.expect("reconcile");
    assert!(matches!(
        result,
        finstack_ai_runtime::ports::tool::ToolReconcileResult::Completed(_)
    ));

    let record = store
        .get(
            MemoryScope::try_new("tenant-a").expect("scope"),
            crate::record::MemoryId::parse("mem-reconciled").expect("id"),
        )
        .await
        .expect("get");
    assert!(record.is_some());
}

#[tokio::test]
async fn remember_reports_an_id_conflict_instead_of_clobbering() {
    // A write-only policy has no `correct_memory`; naming an existing id must
    // not become a back door to destroying that record.
    let (toolset, store) = toolset_with_policy(MemoryPolicy {
        read: false,
        write: true,
        manage: false,
    });
    let first = call_and_extract(
        &toolset,
        context(EffectId::from_bytes([33; 16])),
        "remember",
        br#"{"id":"mem-taken","keywords":["alpha"],"body":"original body"}"#,
    )
    .await;
    assert!(!first.is_error);

    let second = call_and_extract(
        &toolset,
        context(EffectId::from_bytes([34; 16])),
        "remember",
        br#"{"id":"mem-taken","keywords":["beta"],"body":"clobbering body"}"#,
    )
    .await;
    assert!(second.is_error);
    let value: serde_json::Value = serde_json::from_str(second.output.as_str()).expect("json");
    assert_eq!(
        value["code"].as_str(),
        Some(crate::toolset::MEMORY_TOOL_ID_CONFLICT)
    );

    let record = store
        .get(
            MemoryScope::try_new("tenant-a").expect("scope"),
            crate::record::MemoryId::parse("mem-taken").expect("id"),
        )
        .await
        .expect("get")
        .expect("record present");
    assert_eq!(record.preview.as_ref(), "original body");
}

#[tokio::test]
async fn correct_memory_rejects_an_unchanged_body() {
    let (toolset, store) = toolset_with_policy(MemoryPolicy {
        read: true,
        write: true,
        manage: true,
    });
    let body = "unchanged body text";
    let old_id = expected_derived_id("tenant-a", body);
    let remember_args = serde_json::json!({
        "id": old_id.clone(),
        "keywords": ["alpha"],
        "body": body,
    });
    let remembered = call_and_extract(
        &toolset,
        context(EffectId::from_bytes([35; 16])),
        "remember",
        serde_json::to_vec(&remember_args).expect("json").as_slice(),
    )
    .await;
    assert!(!remembered.is_error);

    // The replacement id derives from the body, so an unchanged body would
    // derive `old_id` and make the record supersede itself.
    let correct_args = serde_json::json!({
        "old_id": old_id.clone(),
        "keywords": ["beta"],
        "body": body,
    });
    let result = call_and_extract(
        &toolset,
        context(EffectId::from_bytes([36; 16])),
        "correct_memory",
        serde_json::to_vec(&correct_args).expect("json").as_slice(),
    )
    .await;
    assert!(result.is_error);
    let value: serde_json::Value = serde_json::from_str(result.output.as_str()).expect("json");
    assert_eq!(
        value["code"].as_str(),
        Some(crate::toolset::MEMORY_TOOL_SELF_SUPERSESSION)
    );

    let record = store
        .get(
            MemoryScope::try_new("tenant-a").expect("scope"),
            crate::record::MemoryId::parse(&old_id).expect("id"),
        )
        .await
        .expect("get")
        .expect("record present");
    assert!(record.superseded_by.is_none());
}

#[tokio::test]
async fn scope_comes_from_configuration_not_arguments() {
    let (toolset, _store) = toolset_with_policy(MemoryPolicy::default());
    let effect_id = EffectId::from_bytes([16; 16]);
    let args = br#"{"id":"mem-x","keywords":["alpha"],"body":"body text","tenant":"other-tenant"}"#;
    let Err(error) = toolset
        .call(
            context(effect_id),
            validated_call(&toolset, "remember", args),
        )
        .await
    else {
        panic!("extra unknown field must be rejected");
    };
    assert_eq!(error.code(), crate::toolset::MEMORY_TOOL_INVALID_ARGUMENTS);
}

#[tokio::test]
async fn remember_accepts_a_long_multibyte_body() {
    let (toolset, store) = toolset_with_policy(MemoryPolicy::default());
    // Well over PREVIEW_MAX_BYTES once encoded, but only 300 characters:
    // a character-truncated preview would fail record validation.
    let body = "日本語".repeat(100);
    let args = serde_json::json!({ "keywords": ["alpha"], "body": body });
    let result = call_and_extract(
        &toolset,
        context(EffectId::from_bytes([44; 16])),
        "remember",
        serde_json::to_vec(&args).expect("json").as_slice(),
    )
    .await;
    assert!(!result.is_error, "multibyte remember rejected: {result:?}");

    let listing = store
        .list(
            MemoryScope::try_new("tenant-a").expect("scope"),
            crate::store::MemoryPage {
                offset: 0,
                limit: 10,
            },
        )
        .await
        .expect("list");
    assert_eq!(listing.total, 1);
    assert!(listing.records[0].preview.len() <= crate::record::PREVIEW_MAX_BYTES);
}

#[tokio::test]
async fn search_memory_rejects_an_empty_text_query() {
    let (toolset, _store) = toolset_with_policy(MemoryPolicy::default());
    let args = serde_json::json!({ "text": "   " });
    let arguments = serde_json::to_vec(&args).expect("json");
    let error = toolset
        .call(
            context(EffectId::from_bytes([45; 16])),
            validated_call(&toolset, "search_memory", &arguments),
        )
        .await
        .err()
        .expect("empty text must be rejected");
    assert_eq!(error.code(), "memory_query_invalid");
}
