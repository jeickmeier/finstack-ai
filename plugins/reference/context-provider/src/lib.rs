//! Published reference context-provider component.

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

const QUOTED_JSON: &[u8] = br#"{"kind":"quoted_source","content":[{"kind":"text","text":"reference quoted source"}],"provenance":{"source_id":"finstack.plugin.reference.context","source_ref":null,"external":true},"authority":"untrusted","priority":1,"sensitivity":"internal","protected":false}"#;
const REFERENCE_JSON: &[u8] = br#"{"kind":"reference","content":[{"kind":"text","text":"reference locator"}],"provenance":{"source_id":"finstack.plugin.reference.context","source_ref":null,"external":true},"authority":"untrusted","priority":0,"sensitivity":"internal","protected":false}"#;

struct ReferenceContext;

impl Guest for ReferenceContext {
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

fn map_err(error: finstack_ai_guest_sdk::GuestError) -> PluginError {
    PluginError {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
    }
}
