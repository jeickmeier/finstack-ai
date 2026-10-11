//! `BeforeModel` middleware that converts attached documents to Markdown.
//!
//! The canonical conversation keeps its `ContentBlock::File` blocks. Only the
//! model-visible `ModelRequestDraft` is rewritten: each `File` block whose
//! media type is a supported document format is replaced by a `Text` block
//! containing extracted Markdown (or a fail-soft note). Providers therefore
//! never see media blocks they cannot map.
//!
//! # Scoped blob resolution
//!
//! A `ContentBlock::File` carries only a `BlobRef`. The artifact store resolves
//! that blob inside the authorized run scope and returns both the exact
//! store-owned `ArtifactRef` and verified bytes. No process-local attachment
//! index participates in correctness.

#![warn(missing_docs)]
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

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use finstack_ai_kernel::{
    BlobRef, ComponentId, ComponentInvocation, ContentBlock, Digest, ErrorCategory,
    InvocationRecovery, Message, MessageRole, Metadata, RawJson, Stage, TextBlock, Version,
};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{ArtifactScope, ArtifactStore};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::middleware::{
    MIDDLEWARE_OUTCOME_NOT_ALLOWED, Middleware, MiddlewareContext, MiddlewareDescriptor,
    MiddlewareError, MiddlewareOrder, MiddlewareRole, OrderTier, StageInput, StageMask,
    StageOutcome,
};
use finstack_ai_runtime::ports::model::{CancellationSignal, ModelRequestDraft};
use finstack_ai_tools_document::parser::{
    self, DocumentFormat, DocumentLimits, DocumentParseError, ParsedDocument,
};
use thiserror::Error;

/// Stable code: the attachment's blob could not be resolved through the
/// artifact store inside the run's scope (missing, out of scope, or the
/// store itself failed).
pub const DOCUMENT_INGEST_LOOKUP_FAILED: &str = "document_ingest_lookup_failed";
/// Stable code: the store resolved the blob, but the wire reference or the
/// fetched content does not match what the attachment declared.
pub const DOCUMENT_INGEST_INTEGRITY_MISMATCH: &str = "document_ingest_integrity_mismatch";

const COMPONENT_ID: &str = "finstack.middleware.document-ingest";
const INGEST_VERSION: Version = Version {
    major: 1,
    minor: 1,
    patch: 0,
};

/// Maximum entries retained by the per-instance parse memo before FIFO
/// eviction. Sized for the handful of distinct attachments a conversation
/// realistically carries; the memo exists to avoid re-parsing the same
/// attachment at every `BeforeModel` cycle of a multi-cycle run.
const PARSE_CACHE_CAPACITY: usize = 32;

/// Static skip-note used when a generated note cannot be constructed.
const FALLBACK_NOTE_TEXT: &str = "[attached document note unavailable]";

/// Key identifying one validated attachment and its parse-derived note.
///
/// The key is consulted *before* the artifact store is read, so it must
/// carry everything the skipped fetch would have checked:
///
/// - `tenant_scope` is the committed run's tenant. The store resolves blobs
///   inside that scope, so a note computed for one tenant must never be
///   served to another, even for byte-identical content.
/// - `blob` is the exact wire [`BlobRef`] (id, media type, length, declared
///   digest, name). Its digest (required by [`DocumentIngestMiddleware`]
///   before any lookup) is what makes a hit equivalent to re-parsing the
///   same bytes; the remaining fields feed the note text and reproduce the
///   reference-equality check the fetch path performs. A reference that
///   differs in any field misses and goes through the store again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ParseCacheKey {
    tenant_scope: Arc<str>,
    blob: BlobRef,
}

/// Bounded FIFO memo of parse-derived note text, shared across the clones
/// of one middleware instance.
///
/// A hit skips the artifact fetch, the integrity check, and the parse for
/// an attachment this instance has already resolved and verified under the
/// same tenant: multi-cycle runs hit `BeforeModel` once per cycle with the
/// same attachments, and every cycle after the first costs one map lookup.
/// Only successful resolutions are memoized; lookup and integrity failures
/// are never cached, so a transient store failure is retried on the next
/// cycle. Cached and recomputed notes are byte-identical because the note
/// is a deterministic function of the key (the configured
/// [`DocumentLimits`] are fixed per instance). New keys append, reinsertion
/// refreshes the value without changing eviction order, and a poisoned lock
/// is recovered. Plain `std::sync::Mutex`, no tokio, so this stays usable on
/// `wasm32` hosts.
#[derive(Debug, Default)]
struct ParseCache {
    state: Mutex<ParseCacheState>,
}

#[derive(Debug, Default)]
struct ParseCacheState {
    entries: HashMap<ParseCacheKey, String>,
    insertion_order: VecDeque<ParseCacheKey>,
}

impl ParseCache {
    fn lookup(&self, key: &ParseCacheKey) -> Option<String> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.entries.get(key).cloned()
    }

    fn insert(&self, key: ParseCacheKey, value: String) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.entries.contains_key(&key) {
            state.insertion_order.push_back(key.clone());
        }
        state.entries.insert(key, value);
        while state.insertion_order.len() > PARSE_CACHE_CAPACITY {
            if let Some(oldest) = state.insertion_order.pop_front() {
                state.entries.remove(&oldest);
            }
        }
    }

    #[cfg(test)]
    fn poison(&self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            panic!("parse-cache test poison");
        }));
    }
}

/// Construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DocumentIngestError {
    /// Descriptor identity could not be constructed.
    #[error("document_ingest_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Explicit version tag hashed with the three parse limits.
const LIMITS_IDENTITY_VERSION: &str = "document-ingest-limits-v1";

/// Private identity hashed into `configuration_digest`. Field names are the
/// digest's JSON keys. `DocumentLimits` is not serialized for this purpose.
#[derive(serde::Serialize)]
struct DocumentIngestLimitsIdentity {
    max_input_bytes: u64,
    max_output_bytes: u64,
    max_pages: u32,
    version: &'static str,
}

fn limits_identity_bytes(limits: &DocumentLimits) -> Result<Vec<u8>, DocumentIngestError> {
    let identity = DocumentIngestLimitsIdentity {
        max_input_bytes: limits.max_input_bytes,
        max_output_bytes: limits.max_output_bytes,
        max_pages: limits.max_pages,
        version: LIMITS_IDENTITY_VERSION,
    };
    serde_json_canonicalizer::to_vec(&identity).map_err(|_| DocumentIngestError::Configuration {
        reason: "invalid_configuration_encoding",
    })
}

/// Fail-soft document ingest middleware.
#[derive(Clone)]
pub struct DocumentIngestMiddleware {
    descriptor: MiddlewareDescriptor,
    store: Arc<dyn ArtifactStore>,
    limits: DocumentLimits,
    parse_cache: Arc<ParseCache>,
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
    /// Rejects an invalid checked-in identity or limits that cannot be
    /// encoded into the descriptor configuration digest.
    pub fn try_new(store: Arc<dyn ArtifactStore>) -> Result<Self, DocumentIngestError> {
        let limits = DocumentLimits {
            max_input_bytes: u64::try_from(store.limits().max_artifact_bytes).unwrap_or(u64::MAX),
            ..DocumentLimits::default()
        };
        Self::try_new_with_limits(store, limits)
    }

    /// Construct with explicit parse limits.
    ///
    /// A `try_new` variant, not a builder step: it takes no `self`. The
    /// `try_with_limits` name is reserved for the consuming-builder form
    /// used by the filesystem and shell toolsets.
    ///
    /// # Errors
    ///
    /// Rejects an invalid checked-in identity or limits that cannot be
    /// encoded into the descriptor configuration digest.
    pub fn try_new_with_limits(
        store: Arc<dyn ArtifactStore>,
        limits: DocumentLimits,
    ) -> Result<Self, DocumentIngestError> {
        let configuration_digest = Digest::raw_json(&limits_identity_bytes(&limits)?);
        Ok(Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse(COMPONENT_ID).map_err(|_| {
                        DocumentIngestError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    version: INGEST_VERSION,
                    configuration_digest,
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
            limits,
            parse_cache: Arc::new(ParseCache::default()),
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
                let (rewritten, message_changed) = middleware
                    .rewrite_message(message, &scope, &ctx.run.cancellation)
                    .await?;
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
            let bytes = serde_json_canonicalizer::to_vec(&draft).map_err(|_| {
                stable_error(
                    MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                    "document ingest replacement could not be canonicalized",
                )
            })?;
            Ok(StageOutcome::Replace(RawJson::parse(bytes).map_err(
                |_| {
                    stable_error(
                        MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                        "document ingest replacement could not be normalized",
                    )
                },
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
    ///
    /// Once a supported `File` block is in scope for rewrite, this never
    /// returns the original message. Construction failures become a stable
    /// [`MiddlewareError`] instead of restoring `File` blocks.
    ///
    /// # Errors
    ///
    /// Returns a stable [`MiddlewareError`] when a rewritten note or message
    /// cannot be constructed.
    async fn rewrite_message(
        &self,
        message: &Message,
        scope: &ArtifactScope,
        cancellation: &CancellationSignal,
    ) -> Result<(Message, bool), MiddlewareError> {
        if message.role() != MessageRole::User {
            return Ok((message.clone(), false));
        }
        let mut changed = false;
        let mut blocks: Vec<ContentBlock> = Vec::with_capacity(message.content().len());
        for block in message.content() {
            match block {
                ContentBlock::File(media) => {
                    blocks.push(self.ingest_block(media, scope, cancellation).await?);
                    changed = true;
                }
                other => blocks.push(other.clone()),
            }
        }
        if !changed {
            return Ok((message.clone(), false));
        }
        Ok((rebuild_message(message, blocks)?, true))
    }

    /// # Errors
    ///
    /// Returns a stable [`MiddlewareError`] when a fail-soft note cannot be
    /// constructed. Store and parser failures remain fail-soft notes.
    async fn ingest_block(
        &self,
        media: &finstack_ai_kernel::MediaRef,
        scope: &ArtifactScope,
        cancellation: &CancellationSignal,
    ) -> Result<ContentBlock, MiddlewareError> {
        let blob = media.blob();
        let name = blob.name().unwrap_or("attachment").to_owned();
        if blob.digest().is_none() {
            return note_block(&format!(
                "[document_ingest:digest_required attachment=\"{name}\"]"
            ));
        }
        // The key carries the tenant and the exact wire reference, so a hit
        // is an attachment this instance already fetched, verified, and
        // parsed for this tenant: no store read is needed.
        let key = ParseCacheKey {
            tenant_scope: Arc::clone(&scope.tenant_scope),
            blob: blob.clone(),
        };
        if let Some(note) = self.parse_cache.lookup(&key) {
            return attachment_note_block(&note, blob);
        }
        let bytes = self.fetch_blob(scope, blob, cancellation).await?;
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        let parsed =
            parse_offloaded(bytes, blob.media_type().to_owned(), self.limits.clone()).await;
        let note = match parsed {
            Ok(parsed) if parsed.requires_ocr && parsed.markdown.is_empty() => format!(
                "[Attached document \"{name}\" is a scanned PDF; text extraction requires OCR, which is not enabled.]"
            ),
            Ok(parsed) => {
                let pages = parsed
                    .page_count
                    .map(|count| format!(", {count} pages"))
                    .unwrap_or_default();
                let truncated = if parsed.truncated { ", truncated" } else { "" };
                format!(
                    "Attached document \"{name}\" ({format:?}{pages}{truncated}), converted to Markdown:\n\n{markdown}",
                    format = parsed.format,
                    markdown = parsed.markdown,
                )
            }
            Err(_) if !DocumentFormat::is_supported_media_type(blob.media_type()) => {
                format!("[document_ingest:unsupported attachment=\"{name}\"]")
            }
            Err(_) => format!("[document_ingest:parse_failed attachment=\"{name}\"]"),
        };
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        self.parse_cache.insert(key, note.clone());
        attachment_note_block(&note, blob)
    }

    /// Resolve a `BlobRef` to its exact reference and verified bytes through
    /// the artifact store.
    ///
    /// # Errors
    ///
    /// Returns [`DOCUMENT_INGEST_LOOKUP_FAILED`] when the store cannot
    /// resolve the blob in the run's scope, and
    /// [`DOCUMENT_INGEST_INTEGRITY_MISMATCH`] when the store-owned reference
    /// or the fetched content's digest differs from the wire reference.
    async fn fetch_blob(
        &self,
        scope: &ArtifactScope,
        blob: &BlobRef,
        cancellation: &CancellationSignal,
    ) -> Result<Bytes, MiddlewareError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        let read = self
            .store
            .get_by_blob(scope.clone(), blob.clone())
            .await
            .map_err(|_| {
                stable_error(
                    DOCUMENT_INGEST_LOOKUP_FAILED,
                    "document ingest artifact lookup failed",
                )
            })?;
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        if read.reference.blob() != blob {
            return Err(stable_error(
                DOCUMENT_INGEST_INTEGRITY_MISMATCH,
                "document ingest artifact reference mismatch",
            ));
        }
        if let Some(expected) = blob.digest()
            && Digest::blob_content(&read.content) != *expected
        {
            return Err(stable_error(
                DOCUMENT_INGEST_INTEGRITY_MISMATCH,
                "document ingest artifact integrity mismatch",
            ));
        }
        Ok(read.content)
    }
}

/// Run the parser off the async thread on native hosts; inline on `wasm32`,
/// where there is no blocking pool and no tokio dependency. `bytes` is the
/// store's shared buffer and moves into the worker without copying.
///
/// A worker that cannot report back (panic or runtime shutdown) is a parse
/// failure, which the caller turns into the fail-soft `parse_failed` note.
async fn parse_offloaded(
    bytes: Bytes,
    media_type: String,
    limits: DocumentLimits,
) -> Result<ParsedDocument, DocumentParseError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::task::spawn_blocking(move || parser::parse(&bytes, Some(&media_type), &limits))
            .await
            .unwrap_or_else(|_| {
                Err(DocumentParseError::ParseFailed {
                    message: "document parser worker failed".to_owned(),
                })
            })
    }
    #[cfg(target_arch = "wasm32")]
    parser::parse(&bytes, Some(&media_type), &limits)
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
/// message carrying a single fixed skip note, dropping every other content
/// block rather than risk leaking a `File` block through. If that skip
/// note also cannot be constructed, this returns a stable
/// [`MiddlewareError`] instead of restoring `File` blocks.
///
/// In practice the skip-note fallback should be unreachable for the
/// production `User`-role rewrite path (spec decision 14): `blocks` has
/// the same length and role as the original (already-valid) message, and
/// every substituted block is `TextBlock`, which is always allowed for
/// `User` (see `validate_role_blocks`) and always within `Message`'s
/// per-item content-count limit. The only other rejection modes
/// (`RoleBlockMismatch`, tool-association checks) do not apply to a
/// `User`-role, non-`Tool` message.
///
/// # Errors
///
/// Returns a stable [`MiddlewareError`] when neither the rewritten blocks
/// nor the skip-note fallback can be constructed.
fn rebuild_message(
    message: &Message,
    blocks: Vec<ContentBlock>,
) -> Result<Message, MiddlewareError> {
    match Message::try_new(
        *message.id(),
        message.role(),
        blocks,
        message.created_at(),
        message.model().cloned(),
        message.provider_ids().clone(),
        message.metadata().clone(),
    ) {
        Ok(rebuilt) => Ok(rebuilt),
        Err(_) => skip_note_message(message),
    }
}

/// Last-resort fallback for [`rebuild_message`]: a message that keeps the
/// original identity/role/model/provider-ids/metadata but replaces all
/// content with a single fixed skip note. Construction failure is
/// propagated as a stable [`MiddlewareError`]; callers must not restore
/// the original message.
///
/// # Errors
///
/// Returns a stable [`MiddlewareError`] when the skip note or rebuilt
/// message cannot be constructed.
fn skip_note_message(message: &Message) -> Result<Message, MiddlewareError> {
    let blocks = vec![ContentBlock::Text(fallback_text_block()?)];
    Message::try_new(
        *message.id(),
        message.role(),
        blocks,
        message.created_at(),
        message.model().cloned(),
        message.provider_ids().clone(),
        message.metadata().clone(),
    )
    .map_err(|_| {
        stable_error(
            MIDDLEWARE_OUTCOME_NOT_ALLOWED,
            "document ingest message could not be constructed",
        )
    })
}

/// Map the committed run context to the exact pre-run attachment scope.
///
/// User attachments are staged before session and run ids exist. The binding
/// contract therefore uses the tenant plus the all-zero reserved upload
/// session and `run_id: None`. Tool-produced artifacts remain run-scoped and
/// are not accepted through this input path.
fn run_scope(ctx: &MiddlewareContext) -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
        session_id: finstack_ai_kernel::SessionId::from_bytes([0_u8; 16]),
        run_id: None,
        sensitivity: finstack_ai_kernel::Sensitivity::Internal,
    }
}

// Add reference identity outside the pure parse cache: identical bytes may have
// different blob IDs. The JSON is citation data in the existing user message.
fn attachment_note_block(
    note: &str,
    blob: &finstack_ai_kernel::BlobRef,
) -> Result<ContentBlock, MiddlewareError> {
    let reference = serde_json_canonicalizer::to_string(&std::collections::BTreeMap::from([(
        "attachment",
        blob,
    )]))
    .map_err(|_| {
        stable_error(
            MIDDLEWARE_OUTCOME_NOT_ALLOWED,
            "document attachment reference encoding failed",
        )
    })?;
    note_block(&format!("Document source reference: {reference}\n{note}"))
}

/// Build a model-visible note block. Oversized caller text falls back to
/// [`fallback_text_block`]; if that static note cannot be constructed, the
/// failure is a stable [`MiddlewareError`] rather than a `File` restore.
///
/// # Errors
///
/// Returns a stable [`MiddlewareError`] when the fallback note cannot be
/// constructed.
fn note_block(text: &str) -> Result<ContentBlock, MiddlewareError> {
    match TextBlock::try_new(text) {
        Ok(block) => Ok(ContentBlock::Text(block)),
        Err(_) => fallback_text_block().map(ContentBlock::Text),
    }
}

/// A short static literal is within `TextBlock`'s validation limits. The
/// `Result` exists so production code never panics if that invariant moves.
///
/// # Errors
///
/// Returns a stable [`MiddlewareError`] when the static literal is rejected.
fn fallback_text_block() -> Result<TextBlock, MiddlewareError> {
    TextBlock::try_new(FALLBACK_NOTE_TEXT).map_err(|_| {
        stable_error(
            MIDDLEWARE_OUTCOME_NOT_ALLOWED,
            "document ingest note could not be constructed",
        )
    })
}

/// Build a stable, source-free middleware error under `code`.
fn stable_error(code: &'static str, message: &'static str) -> MiddlewareError {
    MiddlewareError::try_new(code, ErrorCategory::Middleware, message, Metadata::empty())
        .unwrap_or_else(Into::into)
}

fn cancelled_error() -> MiddlewareError {
    MiddlewareError::try_new(
        "document_ingest_cancelled",
        ErrorCategory::Cancellation,
        "document ingest was cancelled",
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

#[cfg(test)]
mod tests;
