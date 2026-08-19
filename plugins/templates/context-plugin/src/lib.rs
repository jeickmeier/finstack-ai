//! Template `context-plugin` guest. Copy this crate to start a new component.

#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

finstack_ai_guest_sdk::context_plugin!();

use exports::finstack::ai_context::context_provider::{ContextItem, ContextQuery, Guest};
use finstack::ai_types::types::PluginError;
use finstack_ai_guest_sdk::{plugin_error, require_sanitized_context};

const ITEM_JSON: &[u8] = br#"{"kind":"quoted_source","content":[{"kind":"text","text":"template quoted source"}],"provenance":{"source_id":"finstack.plugin.template.context","source_ref":null,"external":true},"authority":"untrusted","priority":0,"sensitivity":"internal","protected":false}"#;

struct TemplateContext;

impl Guest for TemplateContext {
    fn collect(query: ContextQuery) -> Result<Vec<ContextItem>, PluginError> {
        require_sanitized_context(
            &query.context.tenant_scope,
            &query.context.authorization_decision_id,
        )
        .map_err(map_err)?;
        if query.budget.max_items == 0 {
            return Err(map_err(plugin_error(
                "plugin_context_item_invalid",
                "max-items must be non-zero",
            )));
        }
        Ok(vec![ContextItem {
            item_json: ITEM_JSON.to_vec(),
            blobs: Vec::new(),
            estimated_tokens: 6,
            bytes: 48,
        }])
    }
}

export!(TemplateContext);

fn map_err(error: finstack_ai_guest_sdk::GuestError) -> PluginError {
    PluginError {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
    }
}
