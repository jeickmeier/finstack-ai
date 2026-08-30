//! `MemoryToolset`: the capability-gated tool surface over a [`MemoryStore`].
//!
//! Five tools — `remember`, `search_memory`, `inspect_memory`,
//! `forget_memory`, `correct_memory` — are always constructed, but
//! [`Toolset::tools`] exposes only the subset a [`MemoryPolicy`] permits.
//! Scope is bound at construction time from the toolset's configuration and
//! is never accepted from tool arguments.

use std::sync::Arc;

use finstack_ai_embeddings::embedder::TextEmbedder;
use finstack_ai_embeddings::vector::truncate_to_bytes;
use finstack_ai_kernel::{
    ErrorCategory, Metadata, RawJson, RetrySafety, Sensitivity, ToolExecutionMode, ToolId,
    ValidatedToolCall,
};
use finstack_ai_runtime::artifact::{
    ArtifactMetadata, ArtifactPersistence, ArtifactScope, ArtifactStore, stage_required_artifact,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, ReconcileContext, RunCallContext, SideEffectClass,
    ToolDeferralSupport, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{
    PendingToolEffect, ToolCallContext, ToolError, ToolEventStream, ToolReconcileResult,
    ToolResult, ToolStreamItem, Toolset, ToolsetDescriptor,
};
use futures_util::stream;
use serde::Deserialize;

use crate::record::{
    ExtractionMethod, MemoryBody, MemoryClock, MemoryError, MemoryId, MemoryProvenance,
    MemoryRecord, MemoryScope, RetentionPolicy, preview_of,
};
use crate::store::{
    MatchEvidence, MemoryQuery, MemoryStore, MemoryStoreError, PutOutcome,
    reconcile_memory_artifacts, reconcile_memory_embeddings,
};

/// Inline-vs-blob threshold for a memory record body, in bytes.
///
/// Re-exported from [`crate::record`], which owns the single definition
/// shared with the observer's capture path.
pub use crate::record::INLINE_BODY_MAX_BYTES;

const REMEMBER_TOOL_ID: &str = "finstack.tools.memory.remember";
const SEARCH_TOOL_ID: &str = "finstack.tools.memory.search";
const INSPECT_TOOL_ID: &str = "finstack.tools.memory.inspect";
const FORGET_TOOL_ID: &str = "finstack.tools.memory.forget";
const CORRECT_TOOL_ID: &str = "finstack.tools.memory.correct";

const REMEMBER_NAME: &str = "remember";
const SEARCH_NAME: &str = "search_memory";
const INSPECT_NAME: &str = "inspect_memory";
const FORGET_NAME: &str = "forget_memory";
const CORRECT_NAME: &str = "correct_memory";

/// Stable invalid-argument error code.
pub const MEMORY_TOOL_INVALID_ARGUMENTS: &str = "memory_tool_invalid_arguments";
/// Stable store-unavailable error code.
pub const MEMORY_TOOL_UNAVAILABLE: &str = "memory_tool_unavailable";
/// Stable not-found payload code (carried inside an `is_error` result, never
/// a [`ToolError`]).
pub const MEMORY_TOOL_NOT_FOUND: &str = "memory_not_found";
/// Stable id-conflict code, raised when `remember` names an identifier that
/// is already taken. Carried inside an `is_error` result so the model can
/// recover by calling `correct_memory` instead.
pub const MEMORY_TOOL_ID_CONFLICT: &str = "memory_id_conflict";
/// Stable self-supersession code, raised when `correct_memory`'s replacement
/// body would derive the identifier it is meant to supersede.
pub const MEMORY_TOOL_SELF_SUPERSESSION: &str = "memory_self_supersession";
/// Stable policy-denied error code.
pub const MEMORY_TOOL_POLICY_DENIED: &str = "memory_policy_denied";
/// Stable semantic-unavailable error code, raised when an explicit
/// `mode: "semantic"` search cannot be served: the toolset has no embedder,
/// the embedder fails (retryable), or the store has no embedding index.
/// Implicit recall degrades silently; an explicit semantic ask fails with
/// this reason instead.
pub const MEMORY_TOOL_SEMANTIC_UNAVAILABLE: &str = "memory_semantic_unavailable";

/// Bounded per-mutation embedding-index drain size.
///
/// Small on purpose: the drain runs inline after each mutating tool call,
/// so it must stay cheap; anything it does not reach stays pending for the
/// next drain (or an application-driven backfill).
const EMBEDDING_DRAIN_LIMIT: usize = 16;

const REMEMBER_INPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string","minLength":1,"maxLength":256},"keywords":{"type":"array","maxItems":64,"items":{"type":"string","minLength":1,"maxLength":128}},"body":{"type":"string","minLength":1,"maxLength":4194304},"sensitivity":{"type":"string"}},"required":["keywords","body"]}"#;
const REMEMBER_OUTPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"outcome":{"type":"string","enum":["inserted","already_applied"]}},"required":["id","outcome"]}"#;

const SEARCH_INPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string","minLength":1,"maxLength":256},"keywords":{"type":"array","minItems":1,"maxItems":64,"items":{"type":"string","minLength":1,"maxLength":128}},"text":{"type":"string","minLength":1,"maxLength":4096},"limit":{"type":"integer","minimum":1,"maximum":25}},"required":[]}"#;
/// [`SEARCH_INPUT_SCHEMA`] plus the `mode` property. Advertised only when
/// the toolset holds an embedder; `mode` on an embedder-less toolset is not
/// part of the model-facing contract.
const SEARCH_INPUT_SCHEMA_WITH_MODE: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string","minLength":1,"maxLength":256},"keywords":{"type":"array","minItems":1,"maxItems":64,"items":{"type":"string","minLength":1,"maxLength":128}},"text":{"type":"string","minLength":1,"maxLength":4096},"mode":{"type":"string","enum":["lexical","semantic"]},"limit":{"type":"integer","minimum":1,"maximum":25}},"required":[]}"#;

const SEARCH_DESCRIPTION: &str = "Search memory records by exact id, keywords, or full text.";
const SEARCH_DESCRIPTION_WITH_MODE: &str =
    "Search memory records by exact id, keywords, or full text. With text, \
     set mode to \"semantic\" to rank by embedding similarity instead of \
     lexical matching; mode defaults to \"lexical\".";
const SEARCH_OUTPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"hits":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"preview":{"type":"string"},"score":{"type":"integer"},"matched":{"type":"string"}},"required":["id","preview","score","matched"]}}},"required":["hits"]}"#;

const INSPECT_INPUT_SCHEMA: &[u8] =
    br#"{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string","minLength":1,"maxLength":256}},"required":["id"]}"#;

const FORGET_INPUT_SCHEMA: &[u8] =
    br#"{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string","minLength":1,"maxLength":256}},"required":["id"]}"#;
const FORGET_OUTPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"tombstoned":{"type":"boolean"}},"required":["id","tombstoned"]}"#;

const CORRECT_INPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"old_id":{"type":"string","minLength":1,"maxLength":256},"keywords":{"type":"array","maxItems":64,"items":{"type":"string","minLength":1,"maxLength":128}},"body":{"type":"string","minLength":1,"maxLength":4194304},"sensitivity":{"type":"string"}},"required":["old_id","keywords","body"]}"#;
const CORRECT_OUTPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"old_id":{"type":"string"},"new_id":{"type":"string"}},"required":["old_id","new_id"]}"#;

/// Which memory tools a [`MemoryToolset`] exposes.
///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct MemoryPolicy {
    /// Gates `search_memory` and `inspect_memory`.
    pub read: bool,
    /// Gates `remember`.
    pub write: bool,
    /// Gates `forget_memory` and `correct_memory`.
    pub manage: bool,
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            read: true,
            write: true,
            manage: false,
        }
    }
}

/// Capability-gated tool surface over a [`MemoryStore`].
#[derive(Clone)]
pub struct MemoryToolset {
    store: Arc<dyn MemoryStore>,
    artifact_store: Arc<dyn ArtifactStore>,
    scope: MemoryScope,
    policy: MemoryPolicy,
    clock: MemoryClock,
    embedder: Option<Arc<dyn TextEmbedder>>,
    descriptor: ToolsetDescriptor,
    specs: Arc<[ToolSpec]>,
}

impl std::fmt::Debug for MemoryToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryToolset")
            .field("scope", &self.scope)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl MemoryToolset {
    /// Construct the toolset and its five cached tool specifications, with
    /// lexical search only.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::Configuration`] when a checked-in tool
    /// identity or schema constant is invalid.
    pub fn try_new(
        store: Arc<dyn MemoryStore>,
        artifact_store: Arc<dyn ArtifactStore>,
        scope: MemoryScope,
        policy: MemoryPolicy,
        clock: MemoryClock,
    ) -> Result<Self, MemoryError> {
        Self::build(store, artifact_store, scope, policy, clock, None)
    }

    /// Construct a toolset whose `search_memory` additionally accepts
    /// `mode: "semantic"` served through `embedder`, and whose mutating
    /// tools drain the store's embedding index best-effort after each
    /// write.
    ///
    /// An explicit semantic search that cannot be served fails with the
    /// stable [`MEMORY_TOOL_SEMANTIC_UNAVAILABLE`] reason; the
    /// post-mutation index drain never fails a tool.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::Configuration`] when a checked-in tool
    /// identity or schema constant is invalid.
    pub fn try_new_with_embedder(
        store: Arc<dyn MemoryStore>,
        artifact_store: Arc<dyn ArtifactStore>,
        scope: MemoryScope,
        policy: MemoryPolicy,
        clock: MemoryClock,
        embedder: Arc<dyn TextEmbedder>,
    ) -> Result<Self, MemoryError> {
        Self::build(store, artifact_store, scope, policy, clock, Some(embedder))
    }

    fn build(
        store: Arc<dyn MemoryStore>,
        artifact_store: Arc<dyn ArtifactStore>,
        scope: MemoryScope,
        policy: MemoryPolicy,
        clock: MemoryClock,
        embedder: Option<Arc<dyn TextEmbedder>>,
    ) -> Result<Self, MemoryError> {
        // `mode` joins the model-facing contract only when an embedder can
        // actually serve it.
        let (search_description, search_input_schema) = if embedder.is_some() {
            (SEARCH_DESCRIPTION_WITH_MODE, SEARCH_INPUT_SCHEMA_WITH_MODE)
        } else {
            (SEARCH_DESCRIPTION, SEARCH_INPUT_SCHEMA)
        };
        let specs = Arc::from([
            build_spec(
                REMEMBER_TOOL_ID,
                REMEMBER_NAME,
                "Remember",
                "Store a memory record for later recall. Bodies over 4096 \
                 bytes are stored as blob artifacts: only the bounded \
                 preview is searchable and recallable afterwards, so keep \
                 bodies that must be readable later under that ceiling.",
                REMEMBER_INPUT_SCHEMA,
                Some(REMEMBER_OUTPUT_SCHEMA),
                SideEffectClass::IdempotentWrite,
            )?,
            build_spec(
                SEARCH_TOOL_ID,
                SEARCH_NAME,
                "Search Memory",
                search_description,
                search_input_schema,
                Some(SEARCH_OUTPUT_SCHEMA),
                SideEffectClass::ReadOnly,
            )?,
            build_spec(
                INSPECT_TOOL_ID,
                INSPECT_NAME,
                "Inspect Memory",
                "Fetch full metadata for one memory record by id. Inline \
                 bodies are returned in full; blob bodies return only the \
                 artifact name, not the stored content.",
                INSPECT_INPUT_SCHEMA,
                None,
                SideEffectClass::ReadOnly,
            )?,
            build_spec(
                FORGET_TOOL_ID,
                FORGET_NAME,
                "Forget Memory",
                "Tombstone (soft-delete) one memory record by id.",
                FORGET_INPUT_SCHEMA,
                Some(FORGET_OUTPUT_SCHEMA),
                SideEffectClass::IdempotentWrite,
            )?,
            build_spec(
                CORRECT_TOOL_ID,
                CORRECT_NAME,
                "Correct Memory",
                "Supersede one memory record with a corrected replacement.",
                CORRECT_INPUT_SCHEMA,
                Some(CORRECT_OUTPUT_SCHEMA),
                SideEffectClass::IdempotentWrite,
            )?,
        ]);
        Ok(Self {
            store,
            artifact_store,
            scope,
            policy,
            clock,
            embedder,
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-memory"),
                metadata: Metadata::empty(),
            },
            specs,
        })
    }
}

fn build_spec(
    id: &str,
    model_name: &str,
    title: &str,
    description: &str,
    input_schema: &[u8],
    output_schema: Option<&[u8]>,
    side_effect: SideEffectClass,
) -> Result<ToolSpec, MemoryError> {
    let tool_id = ToolId::parse(id).map_err(|_| MemoryError::Configuration {
        reason: "invalid_tool_id",
    })?;
    let input_schema = RawJson::parse(input_schema).map_err(|_| MemoryError::Configuration {
        reason: "invalid_input_schema",
    })?;
    let output_schema =
        output_schema
            .map(RawJson::parse)
            .transpose()
            .map_err(|_| MemoryError::Configuration {
                reason: "invalid_output_schema",
            })?;
    let spec = ToolSpec {
        id: tool_id,
        model_name: Arc::from(model_name),
        title: Arc::from(title),
        description: Arc::from(description),
        input_schema,
        output_schema,
        execution: ToolExecutionMode::Parallel,
        side_effect,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 16_384,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    };
    spec.validate().map_err(|_| MemoryError::Configuration {
        reason: "invalid_tool_spec",
    })?;
    Ok(spec)
}

impl Toolset for MemoryToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        let filtered: Vec<ToolSpec> = self
            .specs
            .iter()
            .filter(|spec| self.tool_permitted(spec.model_name.as_ref()))
            .cloned()
            .collect();
        Arc::from(filtered)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let toolset = self.clone();
        Box::pin(async move {
            let name = call.call.tool_name().to_owned();
            let expected_id = toolset
                .specs
                .iter()
                .find(|spec| spec.model_name.as_ref() == name)
                .map(|spec| spec.id.clone())
                .ok_or_else(|| invalid_arguments("unknown memory tool name"))?;
            if !toolset.tool_permitted(&name) {
                return Err(invalid_arguments_code(
                    MEMORY_TOOL_POLICY_DENIED,
                    "memory tool is disabled by policy",
                ));
            }
            toolset.validate_call_context(&ctx, &call, &expected_id)?;
            let arguments = call.call.arguments().as_bytes().to_vec();
            let result = toolset.dispatch(&ctx.run, &name, &arguments).await?;
            Ok(Box::pin(stream::once(async move {
                Ok(ToolStreamItem::Completed(result))
            })) as ToolEventStream)
        })
    }

    fn reconcile(
        &self,
        ctx: ReconcileContext,
        effect: PendingToolEffect,
    ) -> PortFuture<Result<ToolReconcileResult, ToolError>> {
        let toolset = self.clone();
        Box::pin(async move {
            let name = effect.call.call.tool_name().to_owned();
            if !matches!(name.as_str(), REMEMBER_NAME | FORGET_NAME | CORRECT_NAME) {
                return Ok(ToolReconcileResult::Unknown);
            }
            if !toolset.tool_permitted(&name) {
                return Err(invalid_arguments_code(
                    MEMORY_TOOL_POLICY_DENIED,
                    "memory tool is disabled by policy",
                ));
            }
            // Reconciliation replays a committed effect, so the configured
            // tenant is re-checked against the committed context here exactly
            // as `call` does; a toolset bound to another tenant must never
            // finish someone else's write.
            toolset.validate_tenant(&ctx.run)?;
            let arguments = effect.call.call.arguments().as_bytes().to_vec();
            let result = toolset.dispatch(&ctx.run, &name, &arguments).await?;
            Ok(ToolReconcileResult::Completed(result))
        })
    }
}

impl MemoryToolset {
    /// All five tool specs regardless of policy, for tests that need to
    /// build a [`ValidatedToolCall`] without depending on which policy the
    /// toolset was constructed with.
    #[cfg(test)]
    pub(crate) fn tools_unfiltered(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.specs)
    }

    fn tool_permitted(&self, model_name: &str) -> bool {
        match model_name {
            SEARCH_NAME | INSPECT_NAME => self.policy.read,
            REMEMBER_NAME => self.policy.write,
            FORGET_NAME | CORRECT_NAME => self.policy.manage,
            _ => false,
        }
    }

    async fn dispatch(
        &self,
        run: &RunCallContext,
        name: &str,
        arguments: &[u8],
    ) -> Result<ToolResult, ToolError> {
        match name {
            REMEMBER_NAME => self.remember(run, arguments).await,
            SEARCH_NAME => self.search(run, arguments).await,
            INSPECT_NAME => self.inspect(run, arguments).await,
            FORGET_NAME => self.forget(run, arguments).await,
            CORRECT_NAME => self.correct(run, arguments).await,
            _ => Err(invalid_arguments("unknown memory tool name")),
        }
    }

    fn idempotency_key(run: &RunCallContext) -> Arc<str> {
        Arc::from(format!("tool:{}", run.effect_id.to_canonical_string()))
    }

    /// Drain the artifact-ownership outbox. Mutating tools only.
    async fn reconcile_artifacts(&self) -> Result<(), ToolError> {
        let limit = self.store.descriptor().limits.max_artifact_actions.min(256);
        reconcile_memory_artifacts(self.store.as_ref(), self.artifact_store.as_ref(), limit)
            .await
            .map(|_| ())
            .map_err(|error| map_store_error(&error))
    }

    /// Drain a bounded batch of the embedding index after a mutation, when
    /// an embedder is configured.
    ///
    /// Deliberately error-swallowed, in contrast with
    /// [`reconcile_artifacts`](Self::reconcile_artifacts): artifact
    /// pin/unpin is ownership-critical and fails the tool, while the
    /// embedding index is derived data — a failed drain leaves records
    /// pending for the next drain and must never fail the write that
    /// triggered it.
    async fn drain_embeddings(&self) {
        if let Some(embedder) = &self.embedder {
            let _ = reconcile_memory_embeddings(
                self.store.as_ref(),
                embedder.as_ref(),
                EMBEDDING_DRAIN_LIMIT,
            )
            .await;
        }
    }

    /// Embed an explicit `mode: "semantic"` query text into an `Embedding`
    /// store query.
    ///
    /// Unlike the recall provider's implicit leg, this path fails honestly:
    /// no configured embedder is a non-retryable
    /// [`MEMORY_TOOL_SEMANTIC_UNAVAILABLE`], and an embedder failure is the
    /// retryable form of the same reason.
    async fn semantic_query(&self, text: &str) -> Result<MemoryQuery, ToolError> {
        let Some(embedder) = &self.embedder else {
            return Err(invalid_arguments_code(
                MEMORY_TOOL_SEMANTIC_UNAVAILABLE,
                "semantic search requires a configured embedder",
            ));
        };
        let descriptor = embedder.descriptor();
        let truncated = truncate_to_bytes(text, descriptor.max_input_bytes);
        let vectors = embedder
            .embed(vec![Arc::from(truncated)])
            .await
            .map_err(|_| semantic_unavailable_retryable())?;
        let vector = vectors
            .into_iter()
            .next()
            .ok_or_else(semantic_unavailable_retryable)?;
        Ok(MemoryQuery::Embedding {
            embedder_id: descriptor.embedder_id,
            vector,
        })
    }

    async fn remember(
        &self,
        run: &RunCallContext,
        arguments: &[u8],
    ) -> Result<ToolResult, ToolError> {
        let args: RememberArguments = parse_arguments(arguments)?;
        validate_memory_body(
            &args.body,
            self.artifact_store.descriptor().limits.max_artifact_bytes,
        )?;
        validate_memory_keywords(&args.keywords)?;
        let sensitivity = parse_sensitivity(args.sensitivity.as_deref())?;
        let id = match args.id {
            Some(id) => id,
            None => derive_id(&self.scope, &args.body)?,
        };
        let memory_id =
            MemoryId::parse(&id).map_err(|_| invalid_arguments("memory id is invalid"))?;
        let (body, preview) = self
            .build_body(run, &memory_id, &args.body, sensitivity)
            .await?;
        let keywords: Arc<[Arc<str>]> = Arc::from(
            args.keywords
                .iter()
                .map(|k| Arc::from(k.as_str()))
                .collect::<Vec<_>>(),
        );
        let timestamp = (self.clock)();
        let record = MemoryRecord {
            id: memory_id.clone(),
            scope: self.scope.clone(),
            keywords,
            body,
            preview,
            sensitivity,
            provenance: MemoryProvenance {
                source_session: Some(Arc::from(run.locator.session_id.to_canonical_string())),
                source_run: Some(Arc::from(run.locator.run_id.to_canonical_string())),
                source_ref: None,
                extraction: ExtractionMethod::ToolWrite,
                confidence: 100,
            },
            created_at: timestamp,
            last_confirmed_at: timestamp,
            supersedes: None,
            superseded_by: None,
            retention: RetentionPolicy::KeepUntilDeleted,
            tombstoned: false,
        };
        let outcome = match self.store.put(Self::idempotency_key(run), record).await {
            Ok(outcome) => outcome,
            // Surfaced to the model as a recoverable result rather than a
            // tool failure: the fix is a `correct_memory` call, which records
            // the supersession instead of destroying the existing record.
            Err(MemoryStoreError::IdConflict) => return id_conflict_result(),
            Err(error) => return Err(map_store_error(&error)),
        };
        self.reconcile_artifacts().await?;
        self.drain_embeddings().await;
        let outcome_str = match outcome {
            PutOutcome::Inserted => "inserted",
            PutOutcome::AlreadyApplied => "already_applied",
        };
        ok_result(&serde_json::json!({ "id": memory_id.as_str(), "outcome": outcome_str }))
    }

    /// Store small bodies inline; stage larger ones as blobs. Blob content
    /// is not hydrated back through inspect or search.
    async fn build_body(
        &self,
        run: &RunCallContext,
        id: &MemoryId,
        body: &str,
        sensitivity: Sensitivity,
    ) -> Result<(MemoryBody, Arc<str>), ToolError> {
        let preview: Arc<str> = preview_of(body);
        if body.len() <= INLINE_BODY_MAX_BYTES {
            return Ok((MemoryBody::Inline(Arc::from(body)), preview));
        }
        if !self.store.descriptor().manages_artifact_ownership {
            return Err(tool_error(
                MEMORY_TOOL_UNAVAILABLE,
                ErrorCategory::Validation,
                false,
                "memory store does not support blob artifact ownership",
            ));
        }
        if self.store.descriptor().durable
            && self.artifact_store.descriptor().persistence != ArtifactPersistence::Durable
        {
            return Err(tool_error(
                MEMORY_TOOL_UNAVAILABLE,
                ErrorCategory::Validation,
                false,
                "durable memory requires a durable artifact store for blob bodies",
            ));
        }
        let artifact_scope = ArtifactScope {
            tenant_scope: Arc::clone(&self.scope.tenant),
            session_id: run.locator.session_id,
            run_id: Some(run.locator.run_id),
            sensitivity,
        };
        let artifact = stage_required_artifact(
            self.artifact_store.as_ref(),
            artifact_scope.clone(),
            finstack_ai_runtime::Bytes::copy_from_slice(body.as_bytes()),
            ArtifactMetadata {
                kind: Arc::from("memory-record"),
                media_type: Arc::from("text/plain"),
                name: Some(Arc::from(id.as_str())),
                attributes: Metadata::empty(),
            },
        )
        .await
        .map_err(|_| {
            tool_error(
                MEMORY_TOOL_UNAVAILABLE,
                ErrorCategory::Tool,
                true,
                "memory artifact staging failed",
            )
        })?;
        Ok((
            MemoryBody::Blob {
                scope: artifact_scope,
                artifact,
            },
            preview,
        ))
    }

    async fn search(
        &self,
        _run: &RunCallContext,
        arguments: &[u8],
    ) -> Result<ToolResult, ToolError> {
        let args: SearchArguments = parse_arguments(arguments)?;
        let semantic = match args.mode.as_deref() {
            None | Some("lexical") => false,
            Some("semantic") => true,
            Some(_) => {
                return Err(invalid_arguments_code(
                    "memory_query_invalid",
                    "mode must be \"lexical\" or \"semantic\"",
                ));
            }
        };
        if args.mode.is_some() && args.text.is_none() {
            return Err(invalid_arguments_code(
                "memory_query_invalid",
                "mode applies to text queries only",
            ));
        }
        let provided = [
            args.id.is_some(),
            args.keywords.is_some(),
            args.text.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        if provided != 1 {
            return Err(invalid_arguments_code(
                "memory_query_invalid",
                "exactly one of id, keywords, or text is required",
            ));
        }
        let query = if let Some(id) = args.id {
            let memory_id =
                MemoryId::parse(&id).map_err(|_| invalid_arguments("memory id is invalid"))?;
            MemoryQuery::ExactId(memory_id)
        } else if let Some(keywords) = args.keywords {
            let keywords: Arc<[Arc<str>]> = Arc::from(
                keywords
                    .iter()
                    .map(|k| Arc::from(k.as_str()))
                    .collect::<Vec<_>>(),
            );
            MemoryQuery::Keywords(keywords)
        } else if let Some(text) = args.text {
            // An empty or whitespace-only needle matches every record in a
            // substring-matching store, turning `search_memory` into an
            // enumeration of memories the caller never named.
            if text.trim().is_empty() {
                return Err(invalid_arguments_code(
                    "memory_query_invalid",
                    "text must contain at least one non-whitespace character",
                ));
            }
            if semantic {
                self.semantic_query(&text).await?
            } else {
                MemoryQuery::FullText(Arc::from(text.as_str()))
            }
        } else {
            return Err(invalid_arguments_code(
                "memory_query_invalid",
                "exactly one of id, keywords, or text is required",
            ));
        };
        let limit = args.limit.unwrap_or(8).clamp(1, 25) as usize;
        let hits = self
            .store
            .search(self.scope.clone(), query, limit)
            .await
            .map_err(|error| map_search_error(&error, semantic))?;
        let hits_json: Vec<serde_json::Value> = hits
            .iter()
            .map(|hit| {
                serde_json::json!({
                    "id": hit.record.id.as_str(),
                    "preview": hit.record.preview.as_ref(),
                    "score": hit.score,
                    "matched": matched_str(&hit.matched),
                })
            })
            .collect();
        ok_result(&serde_json::json!({ "hits": hits_json }))
    }

    async fn inspect(
        &self,
        _run: &RunCallContext,
        arguments: &[u8],
    ) -> Result<ToolResult, ToolError> {
        let args: InspectArguments = parse_arguments(arguments)?;
        let memory_id =
            MemoryId::parse(&args.id).map_err(|_| invalid_arguments("memory id is invalid"))?;
        let record = self
            .store
            .get(self.scope.clone(), memory_id)
            .await
            .map_err(|error| map_store_error(&error))?;
        match record {
            Some(record) => {
                let mut value = serde_json::to_value(&record).map_err(|_| {
                    tool_error(
                        MEMORY_TOOL_UNAVAILABLE,
                        ErrorCategory::Internal,
                        false,
                        "memory record serialization failed",
                    )
                })?;
                if let serde_json::Value::Object(ref mut map) = value {
                    let body_value = match &record.body {
                        MemoryBody::Inline(text) => serde_json::json!({ "inline": text.as_ref() }),
                        MemoryBody::Blob { artifact, .. } => serde_json::json!({
                            "blob_name": artifact.blob().name().unwrap_or_else(|| artifact.blob().id()),
                        }),
                    };
                    map.insert("body".to_owned(), body_value);
                }
                ok_result(&value)
            }
            None => not_found_result(),
        }
    }

    async fn forget(
        &self,
        run: &RunCallContext,
        arguments: &[u8],
    ) -> Result<ToolResult, ToolError> {
        let args: ForgetArguments = parse_arguments(arguments)?;
        let memory_id =
            MemoryId::parse(&args.id).map_err(|_| invalid_arguments("memory id is invalid"))?;
        match self
            .store
            .forget(
                Self::idempotency_key(run),
                self.scope.clone(),
                memory_id.clone(),
            )
            .await
        {
            Ok(()) => {
                self.reconcile_artifacts().await?;
                self.drain_embeddings().await;
                ok_result(&serde_json::json!({ "id": memory_id.as_str(), "tombstoned": true }))
            }
            Err(MemoryStoreError::NotFound | MemoryStoreError::ScopeMismatch) => not_found_result(),
            Err(error) => Err(map_store_error(&error)),
        }
    }

    async fn correct(
        &self,
        run: &RunCallContext,
        arguments: &[u8],
    ) -> Result<ToolResult, ToolError> {
        let args: CorrectArguments = parse_arguments(arguments)?;
        validate_memory_body(
            &args.body,
            self.artifact_store.descriptor().limits.max_artifact_bytes,
        )?;
        validate_memory_keywords(&args.keywords)?;
        let old_id = MemoryId::parse(&args.old_id)
            .map_err(|_| invalid_arguments("memory old_id is invalid"))?;
        let sensitivity = parse_sensitivity(args.sensitivity.as_deref())?;
        let new_id_str = derive_id(&self.scope, &args.body)?;
        let new_id =
            MemoryId::parse(&new_id_str).map_err(|_| invalid_arguments("memory id is invalid"))?;
        // The replacement id is derived from the replacement body, so an
        // unchanged body derives the id being corrected. Writing that record
        // would make it supersede itself and vanish from recall, so reject it
        // here with a payload the model can act on.
        if new_id == old_id {
            return self_supersession_result();
        }
        let (body, preview) = self
            .build_body(run, &new_id, &args.body, sensitivity)
            .await?;
        let keywords: Arc<[Arc<str>]> = Arc::from(
            args.keywords
                .iter()
                .map(|k| Arc::from(k.as_str()))
                .collect::<Vec<_>>(),
        );
        let timestamp = (self.clock)();
        let replacement = MemoryRecord {
            id: new_id.clone(),
            scope: self.scope.clone(),
            keywords,
            body,
            preview,
            sensitivity,
            provenance: MemoryProvenance {
                source_session: Some(Arc::from(run.locator.session_id.to_canonical_string())),
                source_run: Some(Arc::from(run.locator.run_id.to_canonical_string())),
                source_ref: None,
                extraction: ExtractionMethod::ToolWrite,
                confidence: 100,
            },
            created_at: timestamp,
            last_confirmed_at: timestamp,
            supersedes: None,
            superseded_by: None,
            retention: RetentionPolicy::KeepUntilDeleted,
            tombstoned: false,
        };
        match self
            .store
            .correct(
                Self::idempotency_key(run),
                self.scope.clone(),
                old_id.clone(),
                replacement,
            )
            .await
        {
            Ok(()) => {
                self.reconcile_artifacts().await?;
                self.drain_embeddings().await;
                ok_result(&serde_json::json!({
                    "old_id": old_id.as_str(),
                    "new_id": new_id.as_str(),
                }))
            }
            Err(MemoryStoreError::NotFound | MemoryStoreError::ScopeMismatch) => not_found_result(),
            Err(error) => Err(map_store_error(&error)),
        }
    }
}

pub(crate) fn matched_str(matched: &MatchEvidence) -> String {
    match matched {
        MatchEvidence::ExactId => "exact_id".to_owned(),
        MatchEvidence::Keyword(keyword) => keyword.to_string(),
        MatchEvidence::FullText => "full_text".to_owned(),
        MatchEvidence::Semantic => "semantic".to_owned(),
    }
}

/// Derive a content-addressed id for `body` within the complete scope.
///
/// The tenant is part of the digest domain so two tenants storing identical
/// text land on distinct ids: a shared store must not turn one tenant's
/// first write into a conflict with another tenant's record, nor let the
/// conflict reveal that some other scope holds that exact body.
fn derive_id(scope: &MemoryScope, body: &str) -> Result<String, ToolError> {
    let encoded = serde_json_canonicalizer::to_vec(&(scope, body)).map_err(|_| {
        tool_error(
            MEMORY_TOOL_UNAVAILABLE,
            ErrorCategory::Internal,
            false,
            "memory identity encoding failed",
        )
    })?;
    let digest =
        finstack_ai_kernel::Digest::domain_separated("memory-tool-derived-id", 1, &encoded)
            .map_err(|_| {
                tool_error(
                    MEMORY_TOOL_UNAVAILABLE,
                    ErrorCategory::Internal,
                    false,
                    "memory identity derivation failed",
                )
            })?;
    let hex = digest.to_hex();
    Ok(format!("mem-{}", &hex[..16.min(hex.len())]))
}

fn parse_sensitivity(value: Option<&str>) -> Result<Sensitivity, ToolError> {
    match value {
        None => Ok(Sensitivity::Internal),
        Some(raw) => serde_json::from_value(serde_json::Value::String(raw.to_owned()))
            .map_err(|_| invalid_arguments("memory sensitivity is invalid")),
    }
}

fn validate_memory_body(body: &str, max_bytes: usize) -> Result<(), ToolError> {
    if body.is_empty()
        || (body.len() > INLINE_BODY_MAX_BYTES && body.len() > max_bytes)
        || body.as_bytes().contains(&0)
    {
        return Err(invalid_arguments("memory body is invalid"));
    }
    Ok(())
}

fn validate_memory_keywords(keywords: &[String]) -> Result<(), ToolError> {
    if keywords.len() > crate::record::KEYWORDS_MAX_COUNT
        || keywords.iter().any(|keyword| {
            keyword.is_empty()
                || keyword.len() > crate::record::KEYWORD_MAX_BYTES
                || keyword.as_bytes().contains(&0)
        })
    {
        return Err(invalid_arguments("memory keywords are invalid"));
    }
    for (index, keyword) in keywords.iter().enumerate() {
        if keywords[..index]
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(keyword))
        {
            return Err(invalid_arguments("memory keywords are invalid"));
        }
    }
    Ok(())
}

fn parse_arguments<T: for<'de> Deserialize<'de>>(arguments: &[u8]) -> Result<T, ToolError> {
    serde_json::from_slice(arguments).map_err(|_| invalid_arguments("memory arguments are invalid"))
}

fn ok_result(value: &serde_json::Value) -> Result<ToolResult, ToolError> {
    let output = serde_json::to_vec(value).map_err(|_| {
        tool_error(
            MEMORY_TOOL_UNAVAILABLE,
            ErrorCategory::Internal,
            false,
            "memory result serialization failed",
        )
    })?;
    Ok(ToolResult {
        output: RawJson::parse(output).map_err(|_| {
            tool_error(
                MEMORY_TOOL_UNAVAILABLE,
                ErrorCategory::Internal,
                false,
                "memory result normalization failed",
            )
        })?,
        is_error: false,
    })
}

fn not_found_result() -> Result<ToolResult, ToolError> {
    error_payload(&serde_json::json!({ "code": MEMORY_TOOL_NOT_FOUND }))
}

fn id_conflict_result() -> Result<ToolResult, ToolError> {
    error_payload(&serde_json::json!({
        "code": MEMORY_TOOL_ID_CONFLICT,
        "message": "a memory record already exists under this id; \
                    use correct_memory to supersede it, or omit id to \
                    store a new record",
    }))
}

fn self_supersession_result() -> Result<ToolResult, ToolError> {
    error_payload(&serde_json::json!({
        "code": MEMORY_TOOL_SELF_SUPERSESSION,
        "message": "the corrected body is identical to the record being \
                    corrected; change the body or leave the record as is",
    }))
}

/// Wrap `value` as a model-visible error result (`is_error`), not a
/// [`ToolError`]: these outcomes are recoverable by the model.
fn error_payload(value: &serde_json::Value) -> Result<ToolResult, ToolError> {
    ok_result(value).map(|mut result| {
        result.is_error = true;
        result
    })
}

fn semantic_unavailable_retryable() -> ToolError {
    tool_error(
        MEMORY_TOOL_SEMANTIC_UNAVAILABLE,
        ErrorCategory::Tool,
        true,
        "memory embedder is unavailable",
    )
}

/// Map a search-time store error, honoring the explicit-semantic contract:
/// a store that rejects the `Embedding` query (`InvalidRequest`, e.g. no
/// embedding index) makes the semantic ask fail as
/// [`MEMORY_TOOL_SEMANTIC_UNAVAILABLE`] — the capability is missing, the
/// caller's arguments are not invalid. Everything else maps as usual.
fn map_search_error(error: &MemoryStoreError, semantic: bool) -> ToolError {
    if semantic && matches!(error, MemoryStoreError::InvalidRequest { .. }) {
        return tool_error(
            MEMORY_TOOL_SEMANTIC_UNAVAILABLE,
            ErrorCategory::Tool,
            false,
            "memory store cannot serve semantic search",
        );
    }
    map_store_error(error)
}

fn map_store_error(error: &MemoryStoreError) -> ToolError {
    match error {
        MemoryStoreError::Unavailable { .. } => tool_error(
            MEMORY_TOOL_UNAVAILABLE,
            ErrorCategory::Tool,
            true,
            "memory store is unavailable",
        ),
        MemoryStoreError::InvalidRecord { .. } => invalid_arguments("memory record is invalid"),
        MemoryStoreError::IdConflict => invalid_arguments_code(
            MEMORY_TOOL_ID_CONFLICT,
            "a memory record already exists under this id",
        ),
        MemoryStoreError::IdempotencyConflict => invalid_arguments_code(
            "memory_idempotency_conflict",
            "the idempotency key was already used for another operation",
        ),
        MemoryStoreError::CapacityExceeded { .. } => tool_error(
            "memory_capacity_exceeded",
            ErrorCategory::Limit,
            false,
            "memory store capacity is exhausted",
        ),
        MemoryStoreError::InvalidRequest { .. } => invalid_arguments("memory request is invalid"),
        MemoryStoreError::NotFound | MemoryStoreError::ScopeMismatch => tool_error(
            MEMORY_TOOL_NOT_FOUND,
            ErrorCategory::Tool,
            false,
            "memory record is not visible",
        ),
    }
}

impl MemoryToolset {
    /// Check the committed call context before any store work: the call must
    /// name the tool it claims to, the authorizing principal must agree with
    /// the committed locator, and the locator's tenant must be the tenant
    /// this toolset was constructed for.
    fn validate_call_context(
        &self,
        ctx: &ToolCallContext,
        call: &ValidatedToolCall,
        expected_id: &ToolId,
    ) -> Result<(), ToolError> {
        if call.tool_id != *expected_id {
            return Err(invalid_arguments("memory call identity is invalid"));
        }
        let locator_scope = ctx.run.locator.tenant_scope.as_ref();
        if ctx
            .run
            .authorization
            .principal
            .tenant_scope()
            .is_some_and(|scope| scope != locator_scope)
        {
            return Err(invalid_arguments(
                "memory principal scope does not match the committed effect",
            ));
        }
        self.validate_tenant(&ctx.run)
    }

    /// Re-check the tenant this toolset is bound to against the committed
    /// call context. Scope is bound at construction and never taken from
    /// arguments, so a divergence here means the toolset is being driven on
    /// behalf of a tenant it was not built for.
    fn validate_tenant(&self, run: &RunCallContext) -> Result<(), ToolError> {
        if run.locator.tenant_scope.as_ref() != self.scope.tenant() {
            return Err(invalid_arguments(
                "memory tenant scope does not match the committed effect",
            ));
        }
        Ok(())
    }
}

fn invalid_arguments(message: &'static str) -> ToolError {
    tool_error(
        MEMORY_TOOL_INVALID_ARGUMENTS,
        ErrorCategory::Validation,
        false,
        message,
    )
}

fn invalid_arguments_code(code: &'static str, message: &'static str) -> ToolError {
    tool_error(code, ErrorCategory::Validation, false, message)
}

fn tool_error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ToolError {
    ToolError::try_new(code, category, retryable, message, Metadata::empty())
        .unwrap_or_else(Into::into)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RememberArguments {
    #[serde(default)]
    id: Option<String>,
    keywords: Vec<String>,
    body: String,
    #[serde(default)]
    sensitivity: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArguments {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    keywords: Option<Vec<String>>,
    #[serde(default)]
    text: Option<String>,
    /// `"lexical"` (the default) or `"semantic"`; valid only with `text`.
    /// Parsed on every toolset but advertised — and servable — only when an
    /// embedder is configured.
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectArguments {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgetArguments {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CorrectArguments {
    old_id: String,
    keywords: Vec<String>,
    body: String,
    #[serde(default)]
    sensitivity: Option<String>,
}
