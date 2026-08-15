//! Map Wasmtime bindgen values onto `finstack-ai-wit` mapping types.

use finstack_ai_wit::generated::{
    BlobRef, CallContext, ContextItem, ContextQuery, PluginError, ToolCatalog, ToolResult, ToolSpec,
};

use crate::bindings::context::exports::finstack::ai_context::context_provider as ctx;
use crate::bindings::toolset::exports::finstack::ai_toolset::toolset as ts;
use crate::bindings::toolset::finstack::ai_types::types as ty;
use crate::bindings::v1::context::exports::finstack::ai_context::context_provider as ctx_v1;
use crate::bindings::v1::toolset::exports::finstack::ai_toolset::toolset as ts_v1;
use crate::bindings::v1::toolset::finstack::ai_types::types as ty_v1;

pub(crate) fn wasm_call_context(context: &CallContext) -> ty::CallContext {
    ty::CallContext {
        effect_id: context.effect_id.clone(),
        session_id: context.session_id.clone(),
        lane_id: context.lane_id.clone(),
        run_id: context.run_id.clone(),
        tenant_scope: context.tenant_scope.clone(),
        principal_issuer: context.principal_issuer.clone(),
        principal_subject: context.principal_subject.clone(),
        authorization_decision_id: context.authorization_decision_id.clone(),
        permitted_scopes: context.permitted_scopes.clone(),
        budget_scope_id: context.budget_scope_id.clone(),
        deadline_unix_ms: context.deadline_unix_ms,
    }
}

pub(crate) fn wasm_context_query(query: &ContextQuery) -> ctx::ContextQuery {
    ctx::ContextQuery {
        context: wasm_call_context(&query.context),
        request_json: query.request_json.clone(),
        budget: ctx::ContextBudget {
            max_tokens: query.budget.max_tokens,
            max_bytes: query.budget.max_bytes,
            max_items: query.budget.max_items,
        },
    }
}

pub(crate) fn wit_tool_catalog(catalog: ts::ToolCatalog) -> ToolCatalog {
    ToolCatalog {
        digest: catalog.digest,
        tools: catalog.tools.into_iter().map(wit_tool_spec).collect(),
    }
}

pub(crate) fn wit_tool_spec(spec: ts::ToolSpec) -> ToolSpec {
    ToolSpec {
        id: spec.id,
        model_name: spec.model_name,
        title: spec.title,
        description: spec.description,
        input_schema_json: spec.input_schema_json,
        output_schema_json: spec.output_schema_json,
        execution_mode: spec.execution_mode,
        side_effect: spec.side_effect,
        retry_safety: spec.retry_safety,
        approval_policy_json: spec.approval_policy_json,
        max_result_bytes: spec.max_result_bytes,
        metadata_json: spec.metadata_json,
    }
}

pub(crate) fn wit_tool_result(result: ts::ToolResult) -> ToolResult {
    ToolResult {
        content_json: result.content_json,
        is_error: result.is_error,
    }
}

pub(crate) fn wit_plugin_error(error: ty::PluginError) -> PluginError {
    PluginError {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
    }
}

pub(crate) fn format_plugin_error(error: &PluginError) -> String {
    format!("{}: {}", error.code, error.message)
}

pub(crate) fn wit_context_item(item: ctx::ContextItem) -> ContextItem {
    ContextItem {
        item_json: item.item_json,
        blobs: item.blobs.into_iter().map(wit_blob_ref).collect(),
        estimated_tokens: item.estimated_tokens,
        bytes: item.bytes,
    }
}

fn wit_blob_ref(reference: ty::BlobRef) -> BlobRef {
    BlobRef {
        id: reference.id,
        media_type: reference.media_type,
        length: reference.length,
        digest: reference.digest,
    }
}

pub(crate) fn wasm_call_context_v1(context: &CallContext) -> ty_v1::CallContext {
    ty_v1::CallContext {
        effect_id: context.effect_id.clone(),
        session_id: context.session_id.clone(),
        lane_id: context.lane_id.clone(),
        run_id: context.run_id.clone(),
        tenant_scope: context.tenant_scope.clone(),
        principal_issuer: context.principal_issuer.clone(),
        principal_subject: context.principal_subject.clone(),
        authorization_decision_id: context.authorization_decision_id.clone(),
        permitted_scopes: context.permitted_scopes.clone(),
        budget_scope_id: context.budget_scope_id.clone(),
        deadline_unix_ms: context.deadline_unix_ms,
    }
}

pub(crate) fn wasm_context_query_v1(query: &ContextQuery) -> ctx_v1::ContextQuery {
    ctx_v1::ContextQuery {
        context: wasm_call_context_v1(&query.context),
        request_json: query.request_json.clone(),
        budget: ctx_v1::ContextBudget {
            max_tokens: query.budget.max_tokens,
            max_bytes: query.budget.max_bytes,
            max_items: query.budget.max_items,
        },
    }
}

pub(crate) fn wit_tool_catalog_v1(catalog: ts_v1::ToolCatalog) -> ToolCatalog {
    ToolCatalog {
        digest: catalog.digest,
        tools: catalog.tools.into_iter().map(wit_tool_spec_v1).collect(),
    }
}

fn wit_tool_spec_v1(spec: ts_v1::ToolSpec) -> ToolSpec {
    ToolSpec {
        id: spec.id,
        model_name: spec.model_name,
        title: spec.title,
        description: spec.description,
        input_schema_json: spec.input_schema_json,
        output_schema_json: spec.output_schema_json,
        execution_mode: spec.execution_mode,
        side_effect: spec.side_effect,
        retry_safety: spec.retry_safety,
        approval_policy_json: spec.approval_policy_json,
        max_result_bytes: spec.max_result_bytes,
        metadata_json: spec.metadata_json,
    }
}

pub(crate) fn wit_tool_result_v1(result: ts_v1::ToolResult) -> ToolResult {
    ToolResult {
        content_json: result.content_json,
        is_error: result.is_error,
    }
}

pub(crate) fn wit_plugin_error_v1(error: ty_v1::PluginError) -> PluginError {
    PluginError {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
    }
}

pub(crate) fn wit_context_item_v1(item: ctx_v1::ContextItem) -> ContextItem {
    ContextItem {
        item_json: item.item_json,
        blobs: item.blobs.into_iter().map(wit_blob_ref_v1).collect(),
        estimated_tokens: item.estimated_tokens,
        bytes: item.bytes,
    }
}

fn wit_blob_ref_v1(reference: ty_v1::BlobRef) -> BlobRef {
    BlobRef {
        id: reference.id,
        media_type: reference.media_type,
        length: reference.length,
        digest: reference.digest,
    }
}
