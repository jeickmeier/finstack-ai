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

use std::borrow::Borrow;
use std::collections::{BTreeMap, VecDeque};
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
use finstack_ai_tools_document::parser::{self, DocumentFormat, DocumentLimits};
use thiserror::Error;

const COMPONENT_ID: &str = "finstack.middleware.document-ingest";
const INGEST_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Maximum entries retained by the per-instance parse memo before FIFO
/// eviction. Sized for the handful of distinct attachments a conversation
/// realistically carries; the memo exists to avoid re-parsing the same
/// attachment at every `BeforeModel` cycle of a multi-cycle run.
const PARSE_CACHE_CAPACITY: usize = 32;

/// Static skip-note used when a generated note cannot be constructed.
const FALLBACK_NOTE_TEXT: &str = "[attached document note unavailable]";

/// Key identifying one pure parse-to-note computation.
///
/// `digest` is the digest of the *fetched* bytes (verified against the wire
/// blob's declared digest when present), so a hit is exactly equivalent to
/// re-running the parse on the same bytes. `media_type` and `name` are
/// included because both feed the generated note text (`media_type` also
/// selects the format hint).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ParseCacheKey {
    digest: Digest,
    media_type: String,
    name: String,
}

/// Bounded FIFO memo of parse-derived note text, shared across the clones
/// of one middleware instance.
///
/// Only the pure `bytes -> note text` computation is cached: the blob is
/// still fetched and digest-verified on every invocation, so fail-soft
/// semantics for index misses, store failures, and digest mismatches are
/// unchanged. Cached and recomputed notes are byte-identical because the
/// note is a deterministic function of the key (the configured
/// [`DocumentLimits`] are fixed per instance). Plain `std::sync::Mutex`, no
/// tokio, so this stays usable on `wasm32` hosts.
type ParseCache = BoundedFifoMap<ParseCacheKey, String, PARSE_CACHE_CAPACITY>;

/// Bounded FIFO map: new keys append, reinsertion refreshes the value
/// without changing eviction order, and a poisoned lock is recovered.
///
/// Capacity is a type-level constant so parse-cache and attachment-index
/// uses share one implementation while keeping their distinct bounds.
#[derive(Debug)]
struct BoundedFifoMap<K, V, const CAP: usize> {
    state: Mutex<BoundedFifoMapState<K, V>>,
}

#[derive(Debug)]
struct BoundedFifoMapState<K, V> {
    entries: BTreeMap<K, V>,
    insertion_order: VecDeque<K>,
}

impl<K, V> Default for BoundedFifoMapState<K, V> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            insertion_order: VecDeque::new(),
        }
    }
}

impl<K, V, const CAP: usize> Default for BoundedFifoMap<K, V, CAP> {
    fn default() -> Self {
        Self {
            state: Mutex::new(BoundedFifoMapState::default()),
        }
    }
}

impl<K, V, const CAP: usize> BoundedFifoMap<K, V, CAP> {
    #[must_use]
    fn lookup<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q> + Ord,
        Q: Ord + ?Sized,
        V: Clone,
    {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.entries.get(key).cloned()
    }

    fn insert(&self, key: K, value: V)
    where
        K: Clone + Ord,
    {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.entries.contains_key(&key) {
            state.insertion_order.push_back(key.clone());
        }
        state.entries.insert(key, value);
        while state.insertion_order.len() > CAP {
            if let Some(oldest) = state.insertion_order.pop_front() {
                state.entries.remove(&oldest);
            }
        }
    }

    #[cfg(test)]
    fn poison(&self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            panic!("bounded-fifo-map test poison");
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

fn limits_configuration_digest(limits: &DocumentLimits) -> Result<Digest, DocumentIngestError> {
    Ok(Digest::raw_json(&limits_identity_bytes(limits)?))
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
        Self::try_with_limits(store, limits)
    }

    /// Construct with explicit parse limits.
    ///
    /// # Errors
    ///
    /// Rejects an invalid checked-in identity or limits that cannot be
    /// encoded into the descriptor configuration digest.
    pub fn try_with_limits(
        store: Arc<dyn ArtifactStore>,
        limits: DocumentLimits,
    ) -> Result<Self, DocumentIngestError> {
        let configuration_digest = limits_configuration_digest(&limits)?;
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

    /// The parse limits this instance was constructed with.
    #[cfg(test)]
    pub(crate) fn limits(&self) -> &DocumentLimits {
        &self.limits
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
                stable_error("document ingest replacement could not be canonicalized")
            })?;
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
    /// constructed. Index, store, and parser failures remain fail-soft notes.
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
        let (bytes, digest) = self.fetch_blob(scope, blob, cancellation).await?;
        // Memoize only the pure `bytes -> note text` computation. Every
        // fail-soft branch above (index miss, store failure, digest
        // mismatch) is transient and stays uncached; the branches below are
        // deterministic functions of the fetched bytes, media type, name,
        // and the per-instance limits, so a cached note is byte-identical
        // to a recomputed one.
        let key = ParseCacheKey {
            digest,
            media_type: blob.media_type().to_owned(),
            name: name.clone(),
        };
        if let Some(note) = self.parse_cache.lookup(&key) {
            return note_block(&note);
        }
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        let note = match parser::parse(&bytes, Some(blob.media_type()), &self.limits) {
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
        note_block(&note)
    }

    /// Resolve a `BlobRef` to its exact reference and bytes through the
    /// artifact store. Also returns the fetched
    /// content's digest, which doubles as the parse-memo key.
    ///
    /// # Errors
    ///
    /// Returns a stable middleware error when lookup, scope, reference, or
    /// content integrity validation fails.
    async fn fetch_blob(
        &self,
        scope: &ArtifactScope,
        blob: &BlobRef,
        cancellation: &CancellationSignal,
    ) -> Result<(Bytes, Digest), MiddlewareError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        let read = self
            .store
            .get_by_blob(scope.clone(), blob.clone())
            .await
            .map_err(|_| stable_error("document ingest artifact lookup failed"))?;
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        if read.reference.blob() != blob {
            return Err(stable_error("document ingest artifact reference mismatch"));
        }
        let digest = Digest::blob_content(&read.content);
        if let Some(expected) = blob.digest()
            && digest != *expected
        {
            return Err(stable_error("document ingest artifact integrity mismatch"));
        }
        Ok((read.content, digest))
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
    .map_err(|_| stable_error("document ingest message could not be constructed"))
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
    TextBlock::try_new(FALLBACK_NOTE_TEXT)
        .map_err(|_| stable_error("document ingest note could not be constructed"))
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
