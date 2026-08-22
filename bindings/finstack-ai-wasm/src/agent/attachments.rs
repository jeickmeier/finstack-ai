//! Run-attachment staging shared by `Agent::start`, `Agent::run`, and `Lane::run`.
//!
//! `attachments` arrives from JS as `[{ data, mediaType, name? }]`
//! (`data: Uint8Array`, `mediaType`/`name: string`). Each entry is staged
//! into the agent's internal [`crate::document_store::DocumentArtifactStore`]
//! and recorded in the [the artifact store] shared with `DocumentToolset` and
//! `DocumentIngestMiddleware`, mirroring `finstack-ai-python`'s
//! `stage_attachments`.

use std::sync::Arc;

use finstack_ai::runtime::Bytes;
use finstack_ai::runtime::artifact::{
    ArtifactMetadata, ArtifactScope, ArtifactStore, stage_required_artifact,
};
use finstack_ai::{AttachmentInput, MAX_RUN_ATTACHMENTS};
use finstack_ai_kernel::{Metadata, Sensitivity, SessionId};
use wasm_bindgen::prelude::*;

use super::errors::{agent_error, configuration_error};

struct RawAttachment {
    data: Vec<u8>,
    media_type: String,
    name: Option<String>,
}

/// Canonical tenant-bound pre-run upload scope. The session/run identity is
/// unknown until acceptance, so the all-zero session plus `run_id: None` is
/// the exact scope the document-ingest middleware later resolves.
fn attachment_scope() -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::from("js-local"),
        session_id: SessionId::from_bytes([0_u8; 16]),
        run_id: None,
        sensitivity: Sensitivity::Internal,
    }
}

fn parse_attachments(value: &JsValue) -> Result<Vec<RawAttachment>, JsValue> {
    if value.is_undefined() || value.is_null() {
        return Ok(Vec::new());
    }
    if !js_sys::Array::is_array(value) {
        return Err(agent_error(
            &configuration_error("attachments must be an array"),
            None,
        ));
    }
    let array = js_sys::Array::from(value);
    let mut parsed = Vec::with_capacity(array.length() as usize);
    for item in array.iter() {
        let data_value = js_sys::Reflect::get(&item, &JsValue::from_str("data"))
            .map_err(|_| agent_error(&configuration_error("attachment.data is required"), None))?;
        let data = js_sys::Uint8Array::new(&data_value).to_vec();
        let media_type = js_sys::Reflect::get(&item, &JsValue::from_str("mediaType"))
            .ok()
            .and_then(|value| value.as_string())
            .ok_or_else(|| {
                agent_error(
                    &configuration_error("attachment.mediaType is required"),
                    None,
                )
            })?;
        let name = js_sys::Reflect::get(&item, &JsValue::from_str("name"))
            .ok()
            .and_then(|value| value.as_string());
        parsed.push(RawAttachment {
            data,
            media_type,
            name,
        });
    }
    Ok(parsed)
}

/// Stage every JS attachment into `store`, recording each in `index` so
/// `DocumentIngestMiddleware` can resolve the `BlobRef` it later sees on the
/// journaled `File` block back to the exact staged `ArtifactRef`.
pub(super) fn stage_attachments(
    store: &dyn ArtifactStore,
    attachments: &JsValue,
) -> Result<Vec<AttachmentInput>, JsValue> {
    let attachments = parse_attachments(attachments)?;
    if attachments.len() > MAX_RUN_ATTACHMENTS {
        return Err(agent_error(
            &configuration_error("run attachments exceed MAX_RUN_ATTACHMENTS"),
            None,
        ));
    }
    let scope = attachment_scope();
    let mut staged = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let future = stage_required_artifact(
            store,
            scope.clone(),
            Bytes::from(attachment.data),
            ArtifactMetadata {
                kind: Arc::from("attachment"),
                media_type: Arc::from(attachment.media_type),
                name: attachment.name.map(Arc::from),
                attributes: Metadata::empty(),
            },
        );
        let artifact = block_on_ready(future)?
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
        staged.push(AttachmentInput { artifact });
    }
    Ok(staged)
}

/// Poll an immediately ready future to completion.
///
/// The internal document-attachment store never suspends: `stage_put` and
/// `get` only touch a `std::sync::Mutex`-guarded map with no real async I/O,
/// so this future always resolves on first poll. A `Pending` result would
/// indicate that invariant broke, reported as a structured host error rather
/// than a panic.
fn block_on_ready<T>(future: impl core::future::Future<Output = T>) -> Result<T, JsValue> {
    use core::task::{Context, Poll, Waker};
    let mut future = core::pin::pin!(future);
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => Ok(value),
        Poll::Pending => Err(agent_error(
            &configuration_error("attachment staging did not complete synchronously"),
            None,
        )),
    }
}
