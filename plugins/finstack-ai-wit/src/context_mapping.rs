//! Fail-closed mapping from experimental context WIT values to native types.

use finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS;
use finstack_ai_runtime::{
    ContentBlock, ContextAuthority, ContextBudget as NativeBudget, ContextItem as NativeItem,
    ContextItemKind, ContextOverflowPolicy, ContextProvenance, ContextProviderDescriptor,
    ContextRequest, RunCallContext, Sensitivity,
};
use serde::Deserialize;
use serde_json::Value;

use crate::error::WitMapError;
use crate::generated::{
    CallContext, ContextBudget as WitBudget, ContextItem as WitItem, ContextQuery,
};
use crate::limits::{
    MAX_RAW_JSON_BYTES, MAX_STRING_BYTES, reject_before_allocation, reject_declared_len,
};
use crate::manifest::PluginResourceLimits;
use crate::mapping::sanitize_call_context;

const FORBIDDEN_ITEM_KEYS: [&str; 8] = [
    "interaction",
    "deferral",
    "agent",
    "session",
    "run",
    "lineage",
    "suspend",
    "approval",
];
const REQUIRED_ITEM_KEYS: [&str; 7] = [
    "kind",
    "content",
    "provenance",
    "authority",
    "priority",
    "sensitivity",
    "protected",
];

/// Effective native budget sent to a plugin collect call.
///
/// # Errors
///
/// Returns [`WitMapError`] when any ceiling is zero, exceeds a semantic or
/// payload bound, or cannot be represented on the WIT `u32` item field.
pub fn map_budget(
    run: NativeBudget,
    plugin: Option<&PluginResourceLimits>,
) -> Result<NativeBudget, WitMapError> {
    if run.max_items == 0 {
        return Err(WitMapError::ContextItemInvalid(
            "max-items must be non-zero",
        ));
    }
    if run.max_items > SEMANTIC_ARRAY_MAX_ITEMS {
        return Err(WitMapError::ContextItemInvalid(
            "max-items exceeds the semantic array bound",
        ));
    }
    reject_declared_len(run.max_bytes, MAX_STRING_BYTES, "context-budget.max-bytes")?;
    let mut max_bytes = run.max_bytes;
    if let Some(limits) = plugin {
        max_bytes = max_bytes.min(limits.max_output_bytes);
    }
    let mapped = NativeBudget {
        max_items: run.max_items,
        max_tokens: run.max_tokens,
        max_bytes,
        overflow: ContextOverflowPolicy::Reject,
    };
    Ok(mapped)
}

/// Project a host-owned request into the sanitized WIT query.
///
/// # Errors
///
/// Returns [`WitMapError`] when the request cannot be encoded or the budget is
/// invalid. The guest never supplies replacement session, lane, or run identity.
pub fn map_query(
    run: &RunCallContext,
    request: &ContextRequest,
    plugin: Option<&PluginResourceLimits>,
) -> Result<ContextQuery, WitMapError> {
    let budget = map_budget(request.budget, plugin)?;
    let request_json = request
        .to_raw_json()
        .map_err(|_| WitMapError::ContextItemInvalid("context request could not be normalized"))?;
    reject_before_allocation(request_json.as_bytes(), MAX_RAW_JSON_BYTES, "request-json")?;
    validate_request_json(request_json.as_bytes(), request)?;
    let max_items = u32::try_from(budget.max_items)
        .map_err(|_| WitMapError::ContextItemInvalid("max-items exceeds u32"))?;
    Ok(ContextQuery {
        context: sanitize_call_context(run),
        request_json: request_json.as_bytes().to_vec(),
        budget: WitBudget {
            max_tokens: budget.max_tokens,
            max_bytes: budget.max_bytes,
            max_items,
        },
    })
}

/// Reject a request-json payload that replaces host-owned identity.
///
/// # Errors
///
/// Returns [`WitMapError`] when the payload exceeds its ceiling, fails native
/// decode, or changes session, lane, or run identity.
pub fn validate_request_json(
    bytes: &[u8],
    host: &ContextRequest,
) -> Result<ContextRequest, WitMapError> {
    reject_before_allocation(bytes, MAX_RAW_JSON_BYTES, "request-json")?;
    let parsed: ContextRequest = serde_json::from_slice(bytes)
        .map_err(|_| WitMapError::ContextItemInvalid("request-json is invalid"))?;
    if parsed.session_id != host.session_id
        || parsed.lane_id != host.lane_id
        || parsed.run_id != host.run_id
    {
        return Err(WitMapError::ContextItemInvalid(
            "request-json replaced host identity",
        ));
    }
    Ok(parsed)
}

/// Map one WIT context item onto a native attributed item.
///
/// # Errors
///
/// Returns [`WitMapError`] for payload ceilings, private-suspension keys,
/// unknown fields, self-elevation, or native validation failure.
pub fn map_context_item(
    item: &WitItem,
    descriptor: &ContextProviderDescriptor,
) -> Result<NativeItem, WitMapError> {
    reject_before_allocation(&item.item_json, MAX_RAW_JSON_BYTES, "item-json")?;
    for blob in &item.blobs {
        reject_before_allocation(blob.id.as_bytes(), MAX_STRING_BYTES, "blob.id")?;
        reject_declared_len(blob.length, MAX_STRING_BYTES, "blob.length")?;
    }
    let value: Value = serde_json::from_slice(&item.item_json)
        .map_err(|_| WitMapError::ContextItemInvalid("item-json is invalid"))?;
    reject_private_protocol(&value)?;
    reject_unknown_item_keys(&value)?;
    let wire: ItemWire = serde_json::from_value(value)
        .map_err(|_| WitMapError::ContextItemInvalid("item-json schema is invalid"))?;
    let kind = parse_kind(&wire.kind)?;
    let authority = parse_authority(&wire.authority, kind, descriptor)?;
    let native = NativeItem::try_new(
        kind,
        wire.content,
        wire.provenance,
        authority,
        wire.priority,
        item.estimated_tokens,
        wire.sensitivity,
        wire.protected,
    )
    .map_err(|_| WitMapError::ContextItemInvalid("native context item validation failed"))?;
    if native.estimated_tokens != item.estimated_tokens || native.bytes != item.bytes {
        return Err(WitMapError::ContextItemInvalid(
            "item token or byte estimate does not match native content",
        ));
    }
    Ok(native)
}

/// Encode a native item as the WIT record the reference guest returns.
///
/// # Errors
///
/// Returns [`WitMapError`] when native construction or JSON encoding fails.
#[allow(clippy::too_many_arguments)]
pub fn encode_guest_item(
    kind: ContextItemKind,
    content: Vec<ContentBlock>,
    provenance: ContextProvenance,
    authority: ContextAuthority,
    priority: i32,
    estimated_tokens: u64,
    sensitivity: Sensitivity,
    protected: bool,
) -> Result<WitItem, WitMapError> {
    let native = NativeItem::try_new(
        kind,
        content,
        provenance,
        authority,
        priority,
        estimated_tokens,
        sensitivity,
        protected,
    )
    .map_err(|_| WitMapError::ContextItemInvalid("reference item is invalid"))?;
    let item_json = serde_json::to_vec(&ItemJson {
        kind: native.kind,
        content: &native.content,
        provenance: &native.provenance,
        authority: native.authority,
        priority: native.priority,
        sensitivity: native.sensitivity,
        protected: native.protected,
    })
    .map_err(|_| WitMapError::ContextItemInvalid("item-json encoding failed"))?;
    reject_before_allocation(&item_json, MAX_RAW_JSON_BYTES, "item-json")?;
    Ok(WitItem {
        item_json,
        blobs: Vec::new(),
        estimated_tokens: native.estimated_tokens,
        bytes: native.bytes,
    })
}

/// Borrow the sanitized call-context from a mapped query.
#[must_use]
pub fn query_call_context(query: &ContextQuery) -> &CallContext {
    &query.context
}

fn reject_private_protocol(value: &Value) -> Result<(), WitMapError> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if FORBIDDEN_ITEM_KEYS.contains(&key.as_str()) {
                    return Err(WitMapError::PrivateSuspension);
                }
                reject_private_protocol(child)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                reject_private_protocol(item)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn reject_unknown_item_keys(value: &Value) -> Result<(), WitMapError> {
    let Some(map) = value.as_object() else {
        return Err(WitMapError::ContextItemInvalid(
            "item-json must be an object",
        ));
    };
    for key in map.keys() {
        if !REQUIRED_ITEM_KEYS.contains(&key.as_str()) {
            return Err(WitMapError::ContextItemInvalid(
                "item-json has an unknown key",
            ));
        }
    }
    for key in REQUIRED_ITEM_KEYS {
        if !map.contains_key(key) {
            return Err(WitMapError::ContextItemInvalid(
                "item-json omitted a required key",
            ));
        }
    }
    Ok(())
}

fn parse_kind(value: &str) -> Result<ContextItemKind, WitMapError> {
    match value {
        "instruction" => Ok(ContextItemKind::Instruction),
        "quoted_source" => Ok(ContextItemKind::QuotedSource),
        "reference" => Ok(ContextItemKind::Reference),
        "metadata" => Ok(ContextItemKind::Metadata),
        "hidden_application_context" => Ok(ContextItemKind::HiddenApplicationContext),
        "derived_summary" => Ok(ContextItemKind::DerivedSummary),
        _ => Err(WitMapError::ContextItemInvalid(
            "context item kind is unknown",
        )),
    }
}

fn parse_authority(
    value: &str,
    kind: ContextItemKind,
    descriptor: &ContextProviderDescriptor,
) -> Result<ContextAuthority, WitMapError> {
    let authority = match value {
        "untrusted" => ContextAuthority::Untrusted,
        "trusted_application" => ContextAuthority::TrustedApplication,
        _ => {
            return Err(WitMapError::ContextItemInvalid(
                "context authority is unknown",
            ));
        }
    };
    if authority == ContextAuthority::TrustedApplication
        && !descriptor.trusted_application_instructions
    {
        return Err(WitMapError::ContextItemInvalid(
            "guest cannot self-elevate to trusted application instructions",
        ));
    }
    if kind == ContextItemKind::DerivedSummary && authority != ContextAuthority::Untrusted {
        return Err(WitMapError::ContextItemInvalid(
            "derived summary must stay untrusted",
        ));
    }
    Ok(authority)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemWire {
    kind: String,
    content: Vec<ContentBlock>,
    provenance: ContextProvenance,
    authority: String,
    priority: i32,
    sensitivity: Sensitivity,
    protected: bool,
}

#[derive(serde::Serialize)]
struct ItemJson<'a> {
    kind: ContextItemKind,
    content: &'a [ContentBlock],
    provenance: &'a ContextProvenance,
    authority: ContextAuthority,
    priority: i32,
    sensitivity: Sensitivity,
    protected: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        encode_guest_item, map_budget, map_context_item, map_query, validate_request_json,
    };
    use crate::generated::ContextItem as WitItem;
    use crate::limits::MAX_RAW_JSON_BYTES;
    use crate::manifest::PluginResourceLimits;
    use finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS;
    use finstack_ai_runtime::{
        AuthorizationContext, CancellationSignal, ComponentId, ComponentInvocation, ContentBlock,
        ContextAuthority, ContextBudget, ContextItemKind, ContextOverflowPolicy, ContextProvenance,
        ContextProviderDescriptor, ContextRequest, Digest, EffectId, InvocationRecovery, LaneId,
        Metadata, OperationLocator, PrincipalRef, RunCallContext, RunId, Sensitivity, SessionId,
        TextBlock, Version,
    };
    use serde_json::Value;
    use std::sync::Arc;

    fn descriptor(trusted: bool) -> ContextProviderDescriptor {
        ContextProviderDescriptor {
            invocation: ComponentInvocation {
                component: ComponentId::parse("finstack.plugin.reference.context")
                    .expect("component"),
                version: Version {
                    major: 0,
                    minor: 0,
                    patch: 4,
                },
                configuration_digest: Digest::raw_json(b"{}"),
                recovery: InvocationRecovery::RecomputeSafe,
            },
            trusted_application_instructions: trusted,
            metadata: Metadata::empty(),
        }
    }

    fn request() -> ContextRequest {
        ContextRequest {
            session_id: SessionId::from_bytes([1; 16]),
            lane_id: LaneId::from_bytes([2; 16]),
            run_id: RunId::from_bytes([3; 16]),
            user_input: Arc::from([]),
            recent_history: Arc::from([]),
            budget: ContextBudget {
                max_items: 8,
                max_tokens: 128,
                max_bytes: 16_384,
                overflow: ContextOverflowPolicy::TruncateWithDiagnostic,
            },
            active_capabilities: Arc::from([]),
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
        }
    }

    fn sample_item() -> WitItem {
        encode_guest_item(
            ContextItemKind::QuotedSource,
            vec![ContentBlock::Text(
                TextBlock::try_new("quoted").expect("text"),
            )],
            ContextProvenance {
                source_id: Arc::from("finstack.plugin.reference.context"),
                source_ref: None,
                external: true,
            },
            ContextAuthority::Untrusted,
            0,
            4,
            Sensitivity::Internal,
            false,
        )
        .expect("encode")
    }

    #[test]
    fn budget_takes_minimum_and_rejects_zero_or_oversize() {
        let run = ContextBudget {
            max_items: 8,
            max_tokens: 128,
            max_bytes: 16_384,
            overflow: ContextOverflowPolicy::TruncateWithDiagnostic,
        };
        let mapped = map_budget(
            run,
            Some(&PluginResourceLimits {
                max_output_bytes: 1_024,
                call_timeout_ms: 10,
            }),
        )
        .expect("budget");
        assert_eq!(mapped.max_bytes, 1_024);
        assert_eq!(mapped.overflow, ContextOverflowPolicy::Reject);
        let mut zero = run;
        zero.max_items = 0;
        assert_eq!(
            map_budget(zero, None).expect_err("zero").code(),
            "plugin_context_item_invalid"
        );
        let mut over = run;
        over.max_items = SEMANTIC_ARRAY_MAX_ITEMS + 1;
        assert_eq!(
            map_budget(over, None).expect_err("over").code(),
            "plugin_context_item_invalid"
        );
    }

    #[test]
    fn query_uses_host_identity_and_sanitized_context() {
        let query = map_query(&run_context(), &request(), None).expect("query");
        assert_eq!(query.context.tenant_scope, "tenant-a");
        assert!(!format!("{:?}", query.context).contains("secret-method"));
        let host = request();
        validate_request_json(&query.request_json, &host).expect("host json");
        let mut tampered = host.clone();
        tampered.run_id = RunId::from_bytes([9; 16]);
        let bytes = serde_json::to_vec(&tampered).expect("json");
        assert_eq!(
            validate_request_json(&bytes, &host)
                .expect_err("replaced")
                .code(),
            "plugin_context_item_invalid"
        );
    }

    #[test]
    fn item_json_maps_and_rejects_self_elevation_and_private_keys() {
        let native = map_context_item(&sample_item(), &descriptor(false)).expect("map");
        assert_eq!(native.kind, ContextItemKind::QuotedSource);
        assert_eq!(native.authority, ContextAuthority::Untrusted);
        let mut elevated = sample_item();
        let mut value: Value = serde_json::from_slice(&elevated.item_json).expect("json");
        value["authority"] = Value::String("trusted_application".to_owned());
        elevated.item_json = serde_json::to_vec(&value).expect("json");
        assert_eq!(
            map_context_item(&elevated, &descriptor(false))
                .expect_err("elevate")
                .code(),
            "plugin_context_item_invalid"
        );
        let mut private = sample_item();
        let mut value: Value = serde_json::from_slice(&private.item_json).expect("json");
        value["interaction"] = Value::String("nested".to_owned());
        private.item_json = serde_json::to_vec(&value).expect("json");
        assert_eq!(
            map_context_item(&private, &descriptor(false))
                .expect_err("private")
                .code(),
            "plugin_private_suspension"
        );
        let mut oversize = sample_item();
        oversize.item_json = vec![b'x'; MAX_RAW_JSON_BYTES + 1];
        assert_eq!(
            map_context_item(&oversize, &descriptor(false))
                .expect_err("oversize")
                .code(),
            "plugin_payload_too_large"
        );
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/compatibility/wit/v0.0.4/item-json");
        let mut elevated = sample_item();
        elevated.item_json =
            std::fs::read(root.join("invalid--trusted-self-elevation.json")).expect("fixture");
        assert_eq!(
            map_context_item(&elevated, &descriptor(false))
                .expect_err("fixture elevate")
                .code(),
            "plugin_context_item_invalid"
        );
        let mut private = sample_item();
        private.item_json =
            std::fs::read(root.join("invalid--private-suspension.json")).expect("fixture");
        assert_eq!(
            map_context_item(&private, &descriptor(false))
                .expect_err("fixture private")
                .code(),
            "plugin_private_suspension"
        );
    }
}
