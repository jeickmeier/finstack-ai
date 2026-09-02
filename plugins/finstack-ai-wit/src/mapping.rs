//! Fail-closed mapping from experimental WIT values to native runtime types.

use std::sync::Arc;

use finstack_ai_kernel::{Digest, Metadata, RawJson, RetrySafety, ToolExecutionMode, ToolId};
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, RunCallContext, SideEffectClass, ToolDeferralSupport,
    ToolSpec as NativeToolSpec,
};
use serde_json::Value;

use crate::error::WitMapError;
use crate::generated::{CallContext, ToolCatalog, ToolSpec};
use crate::limits::{
    MAX_METADATA_BYTES, MAX_RAW_JSON_BYTES, MAX_STRING_BYTES, reject_before_allocation,
};

/// Project a native run-call context to the sanitized WIT `call-context`.
///
/// The projection never includes credentials, full claims, cancellation, or
/// attempt counters.
#[must_use]
pub fn sanitize_call_context(run: &RunCallContext) -> CallContext {
    CallContext {
        effect_id: run.effect_id.to_canonical_string(),
        session_id: run.locator.session_id.to_canonical_string(),
        lane_id: run.locator.lane_id.to_canonical_string(),
        run_id: run.locator.run_id.to_canonical_string(),
        tenant_scope: run.locator.tenant_scope.to_string(),
        principal_issuer: run.authorization.principal.issuer().to_owned(),
        principal_subject: run.authorization.principal.subject().to_owned(),
        authorization_decision_id: run.authorization.decision_id.to_string(),
        permitted_scopes: run
            .authorization
            .permitted_scopes
            .iter()
            .map(|scope| scope.as_ref().to_owned())
            .collect(),
        budget_scope_id: run.budget_scope_id.map(|id| id.to_canonical_string()),
        deadline_unix_ms: run.deadline.map(finstack_ai_kernel::Timestamp::as_unix_ms),
    }
}

/// Verify the guest catalog digest and map every tool to native [`NativeToolSpec`].
///
/// # Errors
///
/// Returns [`WitMapError`] when the digest is missing or wrong, a security
/// field is empty or unknown, a payload exceeds its ceiling, or native
/// validation fails.
pub fn register_catalog(catalog: &ToolCatalog) -> Result<Vec<NativeToolSpec>, WitMapError> {
    reject_before_allocation(
        catalog.digest.as_bytes(),
        MAX_STRING_BYTES,
        "catalog.digest",
    )?;
    if catalog.digest.is_empty() {
        return Err(WitMapError::RegistrationInvalid(
            "catalog digest is omitted",
        ));
    }
    let expected = catalog_digest_hex(&catalog.tools)?;
    if catalog.digest != expected {
        return Err(WitMapError::CatalogDigestMismatch);
    }
    catalog.tools.iter().map(map_tool_spec).collect()
}

/// Compute the host catalog digest over canonical tool bytes.
///
/// # Errors
///
/// Returns [`WitMapError`] when a tool payload exceeds its ceiling or is not
/// valid JSON.
pub fn catalog_digest_hex(tools: &[ToolSpec]) -> Result<String, WitMapError> {
    let mut items = Vec::with_capacity(tools.len());
    for tool in tools {
        items.push(canonical_tool_value(tool)?);
    }
    let encoded = serde_json::to_vec(&serde_json::json!({ "tools": items }))
        .map_err(|_| WitMapError::RegistrationInvalid("catalog canonicalization failed"))?;
    reject_before_allocation(&encoded, MAX_RAW_JSON_BYTES, "catalog-json")?;
    let raw = RawJson::parse(&encoded)
        .map_err(|_| WitMapError::RegistrationInvalid("catalog JSON is invalid"))?;
    Ok(Digest::raw_json(raw.as_bytes()).to_hex())
}

/// Map one WIT tool spec to native metadata.
///
/// # Errors
///
/// Returns [`WitMapError`] for omitted/unknown security metadata, oversized
/// payloads, or native validation failure.
pub fn map_tool_spec(spec: &ToolSpec) -> Result<NativeToolSpec, WitMapError> {
    reject_tool_payloads(spec)?;
    if spec.execution_mode.is_empty()
        || spec.side_effect.is_empty()
        || spec.retry_safety.is_empty()
        || spec.approval_policy_json.is_empty()
    {
        return Err(WitMapError::RegistrationInvalid(
            "security metadata is omitted",
        ));
    }
    if spec.max_result_bytes == 0 {
        return Err(WitMapError::RegistrationInvalid(
            "max-result-bytes must be non-zero",
        ));
    }
    let native = NativeToolSpec {
        id: ToolId::parse(&spec.id)
            .map_err(|_| WitMapError::RegistrationInvalid("tool id is invalid"))?,
        model_name: Arc::from(spec.model_name.as_str()),
        title: Arc::from(spec.title.as_str()),
        description: Arc::from(spec.description.as_str()),
        input_schema: parse_raw_json(&spec.input_schema_json, "input-schema-json is invalid")?,
        output_schema: spec
            .output_schema_json
            .as_deref()
            .map(|bytes| parse_raw_json(bytes, "output-schema-json is invalid"))
            .transpose()?,
        execution: parse_execution_mode(&spec.execution_mode)?,
        side_effect: parse_side_effect(&spec.side_effect)?,
        retry_safety: parse_retry_safety(&spec.retry_safety)?,
        approval: parse_approval(&spec.approval_policy_json)?,
        max_result_bytes: spec.max_result_bytes,
        metadata: parse_metadata(&spec.metadata_json)?,
        deferral: ToolDeferralSupport::Never,
    };
    native
        .validate()
        .map_err(|_| WitMapError::ToolSpecInvalid("native tool spec validation failed"))?;
    Ok(native)
}

fn reject_tool_payloads(spec: &ToolSpec) -> Result<(), WitMapError> {
    reject_before_allocation(spec.id.as_bytes(), MAX_STRING_BYTES, "tool.id")?;
    reject_before_allocation(
        spec.model_name.as_bytes(),
        MAX_STRING_BYTES,
        "tool.model-name",
    )?;
    reject_before_allocation(spec.title.as_bytes(), MAX_STRING_BYTES, "tool.title")?;
    reject_before_allocation(
        spec.description.as_bytes(),
        MAX_STRING_BYTES,
        "tool.description",
    )?;
    reject_before_allocation(
        spec.input_schema_json.as_slice(),
        MAX_RAW_JSON_BYTES,
        "input-schema-json",
    )?;
    if let Some(output) = spec.output_schema_json.as_deref() {
        reject_before_allocation(output, MAX_RAW_JSON_BYTES, "output-schema-json")?;
    }
    reject_before_allocation(
        spec.approval_policy_json.as_slice(),
        MAX_RAW_JSON_BYTES,
        "approval-policy-json",
    )?;
    reject_before_allocation(
        spec.metadata_json.as_slice(),
        MAX_METADATA_BYTES,
        "metadata-json",
    )?;
    Ok(())
}

fn canonical_tool_value(spec: &ToolSpec) -> Result<Value, WitMapError> {
    reject_tool_payloads(spec)?;
    Ok(serde_json::json!({
        "id": spec.id,
        "model-name": spec.model_name,
        "title": spec.title,
        "description": spec.description,
        "input-schema-json": parse_json_value(&spec.input_schema_json, "input-schema-json is invalid")?,
        "output-schema-json": spec
            .output_schema_json
            .as_deref()
            .map(|bytes| parse_json_value(bytes, "output-schema-json is invalid"))
            .transpose()?,
        "execution-mode": spec.execution_mode,
        "side-effect": spec.side_effect,
        "retry-safety": spec.retry_safety,
        "approval-policy-json": parse_json_value(&spec.approval_policy_json, "approval-policy-json is invalid")?,
        "max-result-bytes": spec.max_result_bytes,
        "metadata-json": parse_json_value(&spec.metadata_json, "metadata-json is invalid")?,
    }))
}

fn parse_json_value(bytes: &[u8], invalid: &'static str) -> Result<Value, WitMapError> {
    serde_json::from_slice(bytes).map_err(|_| WitMapError::RegistrationInvalid(invalid))
}

fn parse_raw_json(bytes: &[u8], invalid: &'static str) -> Result<RawJson, WitMapError> {
    RawJson::parse(bytes).map_err(|_| WitMapError::RegistrationInvalid(invalid))
}

fn parse_metadata(bytes: &[u8]) -> Result<Metadata, WitMapError> {
    Metadata::parse(bytes).map_err(|_| WitMapError::RegistrationInvalid("metadata-json is invalid"))
}

fn parse_approval(bytes: &[u8]) -> Result<ApprovalMetadata, WitMapError> {
    serde_json::from_slice(bytes)
        .map_err(|_| WitMapError::RegistrationInvalid("approval-policy-json is invalid"))
}

fn parse_execution_mode(value: &str) -> Result<ToolExecutionMode, WitMapError> {
    match value {
        "parallel" => Ok(ToolExecutionMode::Parallel),
        "sequential" => Ok(ToolExecutionMode::Sequential),
        "barrier" => Ok(ToolExecutionMode::Barrier),
        _ => Err(WitMapError::RegistrationInvalid(
            "execution-mode is unknown",
        )),
    }
}

fn parse_side_effect(value: &str) -> Result<SideEffectClass, WitMapError> {
    match value {
        "read_only" => Ok(SideEffectClass::ReadOnly),
        "idempotent_write" => Ok(SideEffectClass::IdempotentWrite),
        "non_idempotent_write" => Ok(SideEffectClass::NonIdempotentWrite),
        _ => Err(WitMapError::RegistrationInvalid("side-effect is unknown")),
    }
}

fn parse_retry_safety(value: &str) -> Result<RetrySafety, WitMapError> {
    match value {
        "safe_to_retry" => Ok(RetrySafety::SafeToRetry),
        "idempotent_with_key" => Ok(RetrySafety::IdempotentWithKey),
        "at_most_once" => Ok(RetrySafety::AtMostOnce),
        "unknown" => Ok(RetrySafety::Unknown),
        _ => Err(WitMapError::RegistrationInvalid("retry-safety is unknown")),
    }
}

#[cfg(test)]
mod tests {
    use super::{catalog_digest_hex, map_tool_spec, register_catalog, sanitize_call_context};
    use crate::generated::{ToolCatalog, ToolSpec};
    use crate::limits::MAX_RAW_JSON_BYTES;
    use finstack_ai_kernel::{
        EffectId, LaneId, Metadata, OperationLocator, PrincipalRef, RunId, SessionId,
        ToolExecutionMode,
    };
    use finstack_ai_runtime::ports::model::{
        ApprovalRequirement, AuthorizationContext, CancellationSignal, RunCallContext,
        SideEffectClass,
    };

    fn sample_spec() -> ToolSpec {
        ToolSpec {
            id: "finstack.plugin.add".to_owned(),
            model_name: "add".to_owned(),
            title: "Add".to_owned(),
            description: "Add two integers".to_owned(),
            input_schema_json: br#"{"type":"object"}"#.to_vec(),
            output_schema_json: Some(br#"{"type":"object"}"#.to_vec()),
            execution_mode: "sequential".to_owned(),
            side_effect: "read_only".to_owned(),
            retry_safety: "safe_to_retry".to_owned(),
            approval_policy_json: br#"{"requirement":"not_required"}"#.to_vec(),
            max_result_bytes: 1024,
            metadata_json: b"{}".to_vec(),
        }
    }

    fn run_context() -> RunCallContext {
        RunCallContext {
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
                authentication_method: "secret-method".into(),
                assurance_level: "high".into(),
                roles: ["admin".into()].into(),
                permitted_scopes: ["tenant-a".into()].into(),
                safe_claims: Metadata::empty(),
                policy_version: "policy-v1".into(),
                decision_id: "decision-v1".into(),
            },
            effect_id: EffectId::from_bytes([4; 16]),
            attempt: 9,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
            relation_depth: 0,
        }
    }

    #[test]
    fn sanitized_context_omits_secrets_and_attempt() {
        let projected = sanitize_call_context(&run_context());
        let encoded = format!("{projected:?}");
        assert!(!encoded.contains("secret-method"));
        assert!(!encoded.contains("admin"));
        assert!(!encoded.contains("attempt"));
        assert_eq!(projected.authorization_decision_id, "decision-v1");
        assert_eq!(projected.permitted_scopes, ["tenant-a"]);
        assert_eq!(projected.tenant_scope, "tenant-a");
        assert_eq!(projected.principal_issuer, "issuer");
    }

    #[test]
    fn catalog_registers_when_digest_matches() {
        let spec = sample_spec();
        let digest = catalog_digest_hex(std::slice::from_ref(&spec)).expect("digest");
        let native = register_catalog(&ToolCatalog {
            digest,
            tools: vec![spec],
        })
        .expect("register");
        assert_eq!(native[0].execution, ToolExecutionMode::Sequential);
        assert_eq!(native[0].side_effect, SideEffectClass::ReadOnly);
        assert_eq!(
            native[0].approval.requirement,
            ApprovalRequirement::NotRequired
        );
    }

    #[test]
    fn omitted_and_unknown_security_metadata_fail_closed() {
        let mut omitted = sample_spec();
        omitted.execution_mode.clear();
        assert_eq!(
            map_tool_spec(&omitted).expect_err("empty").code(),
            "plugin_registration_invalid"
        );
        let mut unknown = sample_spec();
        unknown.side_effect = "read-only".to_owned();
        assert_eq!(
            map_tool_spec(&unknown).expect_err("unknown").code(),
            "plugin_registration_invalid"
        );
        let mut zero = sample_spec();
        zero.max_result_bytes = 0;
        assert_eq!(
            map_tool_spec(&zero).expect_err("zero").code(),
            "plugin_registration_invalid"
        );
        let digest = sample_spec();
        assert_eq!(
            register_catalog(&ToolCatalog {
                digest: "00".repeat(32),
                tools: vec![digest],
            })
            .expect_err("mismatch")
            .code(),
            "plugin_catalog_digest_mismatch"
        );
    }

    #[test]
    fn oversized_schema_is_rejected_before_parse() {
        let mut spec = sample_spec();
        spec.input_schema_json = vec![b'x'; MAX_RAW_JSON_BYTES + 1];
        let error = map_tool_spec(&spec).expect_err("oversize");
        assert_eq!(error.code(), "plugin_payload_too_large");
    }
}
