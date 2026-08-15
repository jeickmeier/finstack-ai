//! Test-only `context-plugin` guest. Not a published SDK or template.

wit_bindgen::generate!({
    world: "context-plugin",
    path: "../../../wit",
    generate_all,
});

use exports::finstack::ai_context::context_provider::{ContextItem, ContextQuery, Guest};
use finstack::ai_types::types::PluginError;

const QUOTED_JSON: &[u8] = br#"{"kind":"quoted_source","content":[{"kind":"text","text":"reference quoted source"}],"provenance":{"source_id":"finstack.plugin.reference.context","source_ref":null,"external":true},"authority":"untrusted","priority":1,"sensitivity":"internal","protected":false}"#;
const REFERENCE_JSON: &[u8] = br#"{"kind":"reference","content":[{"kind":"text","text":"reference locator"}],"provenance":{"source_id":"finstack.plugin.reference.context","source_ref":null,"external":true},"authority":"untrusted","priority":0,"sensitivity":"internal","protected":false}"#;

struct ReferenceContext;

impl Guest for ReferenceContext {
    fn collect(query: ContextQuery) -> Result<Vec<ContextItem>, PluginError> {
        if query.context.tenant_scope.is_empty()
            || query.context.authorization_decision_id.is_empty()
        {
            return Err(PluginError {
                code: "plugin_call_context_invalid".to_owned(),
                message: "sanitized call-context is incomplete".to_owned(),
                retryable: false,
            });
        }
        if query.budget.max_items == 0 {
            return Err(PluginError {
                code: "plugin_context_item_invalid".to_owned(),
                message: "max-items must be non-zero".to_owned(),
                retryable: false,
            });
        }
        Ok(vec![
            ContextItem {
                item_json: QUOTED_JSON.to_vec(),
                blobs: Vec::new(),
                estimated_tokens: 8,
                bytes: 50,
            },
            ContextItem {
                item_json: REFERENCE_JSON.to_vec(),
                blobs: Vec::new(),
                estimated_tokens: 6,
                bytes: 44,
            },
        ])
    }
}

export!(ReferenceContext);
