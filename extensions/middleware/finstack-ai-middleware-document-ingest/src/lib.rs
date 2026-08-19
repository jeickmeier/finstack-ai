//! `BeforeModel` middleware that converts attached documents to Markdown.
//!
//! The canonical conversation keeps its `ContentBlock::File` blocks. Only the
//! model-visible `ModelRequestDraft` is rewritten: each `File` block whose
//! media type is a supported document format is replaced by a `Text` block
//! containing extracted Markdown (or a fail-soft note). Providers therefore
//! never see media blocks they cannot map.
//!
//! # `BlobRef` -> `ArtifactRef` resolution
//!
//! A `ContentBlock::File` only ever carries a `BlobRef` on the wire, and no
//! `ArtifactStore` implementation guarantees that a full `ArtifactRef` (id,
//! kind, digests, metadata) can be reconstructed from that bare `BlobRef`
//! alone (spec decision 19). This middleware instead consults an
//! [`AttachmentIndex`] populated wherever attachments are staged: the caller
//! that staged the artifact hands its exact `ArtifactRef` to
//! [`AttachmentIndex::insert`], and this middleware looks it up by blob id.
//! A miss (never staged, or evicted) is treated as fail-soft "could not be
//! read".

#![warn(missing_docs)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use finstack_ai_kernel::{
    ArtifactRef, BlobRef, ComponentId, ComponentInvocation, ContentBlock, Digest, ErrorCategory,
    InvocationRecovery, Message, MessageRole, Metadata, RawJson, Stage, TextBlock, Version,
};
use finstack_ai_runtime::{
    ArtifactScope, ArtifactStore, Bytes, MIDDLEWARE_OUTCOME_NOT_ALLOWED, Middleware,
    MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder, MiddlewareRole,
    ModelRequestDraft, OrderTier, PortFuture, StageInput, StageMask, StageOutcome,
};
use finstack_ai_tools_document::parser::{self, DocumentFormat, DocumentLimits};
use thiserror::Error;

const COMPONENT_ID: &str = "finstack.middleware.document-ingest";
const INGEST_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Maximum entries retained by [`AttachmentIndex`] before FIFO eviction.
const ATTACHMENT_INDEX_CAPACITY: usize = 1024;

/// Bounded blob-id -> `ArtifactRef` map populated wherever attachments are
/// staged.
///
/// `ArtifactStore` implementations verify exact `ArtifactRef` identity
/// against store-assigned state that a bare `BlobRef` cannot reproduce
/// (spec decision 19), so this middleware cannot reconstruct an `ArtifactRef`
/// from the `BlobRef` carried on `ContentBlock::File`. Instead, whoever
/// staged the attachment (the run/session attachment path, or a toolset)
/// records the exact `ArtifactRef` it received from `stage_put` here, keyed
/// by the staged blob's id, so the ingest middleware can look it back up.
///
/// FIFO-capped at [`ATTACHMENT_INDEX_CAPACITY`] entries: once full, the
/// oldest insertion is evicted to keep memory bounded for long-lived
/// processes. Interior mutability is a plain `std::sync::Mutex` (no tokio),
/// so this type stays usable on `wasm32` hosts.
#[derive(Debug, Default)]
pub struct AttachmentIndex {
    state: Mutex<AttachmentIndexState>,
}

#[derive(Debug, Default)]
struct AttachmentIndexState {
    entries: BTreeMap<String, ArtifactRef>,
    insertion_order: VecDeque<String>,
}

impl AttachmentIndex {
    /// Record a staged artifact, keyed by its blob id.
    ///
    /// Re-inserting the same blob id refreshes the stored `ArtifactRef`
    /// without moving it in FIFO order. Once the index holds
    /// [`ATTACHMENT_INDEX_CAPACITY`] distinct blob ids, the oldest entry is
    /// evicted first.
    pub fn insert(&self, artifact: ArtifactRef) {
        let key = artifact.blob().id().to_owned();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.entries.contains_key(&key) {
            state.insertion_order.push_back(key.clone());
        }
        state.entries.insert(key, artifact);
        while state.insertion_order.len() > ATTACHMENT_INDEX_CAPACITY {
            if let Some(oldest) = state.insertion_order.pop_front() {
                state.entries.remove(&oldest);
            }
        }
    }

    /// Look up the staged `ArtifactRef` for a `BlobRef`, keyed by
    /// `blob.id()`.
    ///
    /// Returns `None` when the blob was never staged through
    /// [`AttachmentIndex::insert`], or has since been evicted.
    #[must_use]
    pub fn lookup(&self, blob: &BlobRef) -> Option<ArtifactRef> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.entries.get(blob.id()).cloned()
    }
}

/// Construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DocumentIngestError {
    /// A checked-in identity constant is invalid.
    #[error("document_ingest_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Fail-soft document ingest middleware.
#[derive(Clone)]
pub struct DocumentIngestMiddleware {
    descriptor: MiddlewareDescriptor,
    store: Arc<dyn ArtifactStore>,
    index: Arc<AttachmentIndex>,
    limits: DocumentLimits,
}

impl std::fmt::Debug for DocumentIngestMiddleware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentIngestMiddleware")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl DocumentIngestMiddleware {
    /// Construct with default limits.
    ///
    /// # Errors
    ///
    /// Rejects an invalid checked-in identity.
    pub fn try_new(
        store: Arc<dyn ArtifactStore>,
        index: Arc<AttachmentIndex>,
    ) -> Result<Self, DocumentIngestError> {
        Self::try_with_limits(store, index, DocumentLimits::default())
    }

    /// Construct with explicit parse limits.
    ///
    /// # Errors
    ///
    /// Rejects an invalid checked-in identity.
    pub fn try_with_limits(
        store: Arc<dyn ArtifactStore>,
        index: Arc<AttachmentIndex>,
        limits: DocumentLimits,
    ) -> Result<Self, DocumentIngestError> {
        Ok(Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse(COMPONENT_ID).map_err(|_| {
                        DocumentIngestError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    version: INGEST_VERSION,
                    configuration_digest: Digest::raw_json(b"document-ingest-v1"),
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                stages: StageMask::from_stages([Stage::BeforeModel]),
                order: MiddlewareOrder {
                    tier: OrderTier::ContextMutation,
                    priority: 0,
                    before: Arc::from([]),
                    after: Arc::from([]),
                },
                role: MiddlewareRole::Standard,
                metadata: Metadata::empty(),
            },
            store,
            index,
            limits,
        })
    }
}

impl Middleware for DocumentIngestMiddleware {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let middleware = self.clone();
        Box::pin(async move {
            let StageInput::BeforeModel(before_model) = input else {
                return Ok(StageOutcome::Continue);
            };
            let scope = run_scope(&ctx);
            let mut changed = false;
            let mut messages: Vec<Message> =
                Vec::with_capacity(before_model.request.messages.len());
            for message in before_model.request.messages.iter() {
                let (rewritten, message_changed) =
                    middleware.rewrite_message(message, &scope).await;
                changed |= message_changed;
                messages.push(rewritten);
            }
            if !changed {
                return Ok(StageOutcome::Continue);
            }
            let draft = ModelRequestDraft {
                messages: messages.into(),
                ..before_model.request.clone()
            };
            let bytes = serde_json_canonicalizer_bytes(&draft)?;
            Ok(StageOutcome::Replace(RawJson::parse(bytes).map_err(
                |_| stable_error("document ingest replacement could not be normalized"),
            )?))
        })
    }
}

impl DocumentIngestMiddleware {
    /// Rewrite a single message's `File` blocks to Markdown text.
    ///
    /// Per spec decision 14, only `User`-role messages carry attachments
    /// (`ContentBlock::File` is only valid on `User`/`Assistant` per
    /// [`finstack_ai_kernel`]'s role/block rules, and only the `User` role
    /// is the attachment ingestion surface); other roles are returned
    /// unchanged without inspecting their content.
    async fn rewrite_message(&self, message: &Message, scope: &ArtifactScope) -> (Message, bool) {
        if message.role() != MessageRole::User {
            return (message.clone(), false);
        }
        let mut changed = false;
        let mut blocks: Vec<ContentBlock> = Vec::with_capacity(message.content().len());
        for block in message.content() {
            match block {
                ContentBlock::File(media)
                    if DocumentFormat::is_supported_media_type(media.blob().media_type()) =>
                {
                    blocks.push(self.ingest_block(media, scope).await);
                    changed = true;
                }
                other => blocks.push(other.clone()),
            }
        }
        if !changed {
            return (message.clone(), false);
        }
        (rebuild_message(message, blocks), true)
    }

    async fn ingest_block(
        &self,
        media: &finstack_ai_kernel::MediaRef,
        scope: &ArtifactScope,
    ) -> ContentBlock {
        let blob = media.blob();
        let name = blob.name().unwrap_or("attachment").to_owned();
        let Ok(bytes) = self.fetch_blob(scope, blob).await else {
            return note_block(&format!(
                "[Attached document \"{name}\" could not be read; it was skipped.]"
            ));
        };
        match parser::parse(&bytes, Some(blob.media_type()), &self.limits) {
            Ok(parsed) if parsed.requires_ocr && parsed.markdown.is_empty() => {
                note_block(&format!(
                    "[Attached document \"{name}\" is a scanned PDF; text extraction requires OCR, which is not enabled.]"
                ))
            }
            Ok(parsed) => {
                let pages = parsed
                    .page_count
                    .map(|count| format!(", {count} pages"))
                    .unwrap_or_default();
                let truncated = if parsed.truncated { ", truncated" } else { "" };
                note_block(&format!(
                    "Attached document \"{name}\" ({format:?}{pages}{truncated}), converted to Markdown:\n\n{markdown}",
                    format = parsed.format,
                    markdown = parsed.markdown,
                ))
            }
            Err(_) => note_block(&format!(
                "[Attached document \"{name}\" could not be parsed; it was skipped.]"
            )),
        }
    }

    /// Resolve a `BlobRef` to its exact bytes via the `AttachmentIndex` and
    /// the artifact store, verifying the fetched content against the blob's
    /// declared digest when one is present.
    ///
    /// # Errors
    ///
    /// Returns `Err(())` when the blob was never staged (index miss), the
    /// store cannot return it, or the returned bytes do not match
    /// `blob.digest()`. Every branch collapses to the same fail-soft
    /// "could not be read" note at the call site, so the reason is not
    /// distinguished further.
    async fn fetch_blob(&self, scope: &ArtifactScope, blob: &BlobRef) -> Result<Bytes, ()> {
        let artifact = self.index.lookup(blob).ok_or(())?;
        let bytes = self
            .store
            .get(scope.clone(), artifact)
            .await
            .map_err(|_| ())?;
        if let Some(expected) = blob.digest()
            && Digest::blob_content(&bytes) != *expected
        {
            return Err(());
        }
        Ok(bytes)
    }
}

/// Rebuild a message with new content blocks, preserving role, id, model,
/// provider ids, and metadata exactly. Mirrors the constructor
/// `crates/finstack-ai/src/agent/prepare.rs` uses to build messages.
///
/// # Invariant: the model must never see a supported `File` block
///
/// `blocks` is already the caller's rewritten set (every supported `File`
/// block replaced by a bounded [`TextBlock`] note from [`note_block`], see
/// [`DocumentIngestMiddleware::rewrite_message`]). If `Message::try_new`
/// rejects that rewritten set, falling back to `message.clone()` would
/// silently reintroduce the original `File` blocks this middleware exists
/// to strip — so the fallback below never does that. It instead builds a
/// message carrying a single fixed, always-valid skip note, dropping every
/// other content block rather than risk leaking a `File` block through.
///
/// In practice this fallback should be unreachable: after restricting
/// rewriting to `User`-role messages (spec decision 14), `blocks` has the
/// same length and role as the original (already-valid) message, and every
/// substituted block is `TextBlock`, which is always allowed for `User`
/// (see `validate_role_blocks`) and always within `Message`'s per-item
/// content-count limit. The only other rejection modes
/// (`RoleBlockMismatch`, tool-association checks) do not apply to a
/// `User`-role, non-`Tool` message. No test exercises this branch because
/// there is no constructible input that reaches it without directly
/// violating `Message`'s own validated invariants.
fn rebuild_message(message: &Message, blocks: Vec<ContentBlock>) -> Message {
    Message::try_new(
        *message.id(),
        message.role(),
        blocks,
        message.created_at(),
        message.model().cloned(),
        message.provider_ids().clone(),
        message.metadata().clone(),
    )
    .unwrap_or_else(|_| skip_note_message(message))
}

/// Last-resort fallback for [`rebuild_message`]: a message that keeps the
/// original identity/role/model/provider-ids/metadata but replaces all
/// content with a single fixed skip note. The note is a short static
/// literal (see [`fallback_text_block`]), so this construction cannot fail
/// for any `User`-role `message` — the only role this middleware rewrites.
fn skip_note_message(message: &Message) -> Message {
    let blocks = vec![ContentBlock::Text(fallback_text_block())];
    Message::try_new(
        *message.id(),
        message.role(),
        blocks,
        message.created_at(),
        message.model().cloned(),
        message.provider_ids().clone(),
        message.metadata().clone(),
    )
    .expect(
        "a single bounded static TextBlock note is always valid for a User-role message: \
         TextBlock is within size limits, User allows Text blocks, and non-Tool messages \
         have no tool-association constraints",
    )
}

/// Map the committed run context to the exact `ArtifactScope` used to stage
/// and fetch attachments. Mirrors the toolset's `call_scope`.
fn run_scope(ctx: &MiddlewareContext) -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
        session_id: ctx.run.locator.session_id,
        run_id: Some(ctx.run.locator.run_id),
        sensitivity: finstack_ai_kernel::Sensitivity::Internal,
    }
}

fn note_block(text: &str) -> ContentBlock {
    TextBlock::try_new(text).map_or_else(
        |_| ContentBlock::Text(fallback_text_block()),
        ContentBlock::Text,
    )
}

/// A short static literal is always within `TextBlock`'s validation limits,
/// so this only exists to give `note_block` an infallible fallback rather
/// than panicking on a pathological (e.g. oversized) input.
fn fallback_text_block() -> TextBlock {
    TextBlock::try_new("[attached document note unavailable]")
        .expect("static literal note is always valid")
}

fn serde_json_canonicalizer_bytes(draft: &ModelRequestDraft) -> Result<Vec<u8>, MiddlewareError> {
    serde_json_canonicalizer::to_vec(draft)
        .map_err(|_| stable_error("document ingest replacement could not be canonicalized"))
}

fn stable_error(message: &'static str) -> MiddlewareError {
    MiddlewareError::try_new(
        MIDDLEWARE_OUTCOME_NOT_ALLOWED,
        ErrorCategory::Middleware,
        message,
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

#[cfg(test)]
mod tests;
