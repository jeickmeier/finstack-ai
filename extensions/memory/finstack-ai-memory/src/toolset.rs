//! `MemoryToolset`: the capability-gated tool surface over a [`MemoryStore`].
//!
//! Five tools — `remember`, `search_memory`, `inspect_memory`,
//! `forget_memory`, `correct_memory` — are always constructed, but
//! [`Toolset::tools`] exposes only the subset a [`MemoryPolicy`] permits.
//! Scope is bound at construction time from the toolset's configuration and
//! is never accepted from tool arguments.

use std::sync::Arc;

use finstack_ai_kernel::{
    Digest, ErrorCategory, Metadata, RawJson, RetrySafety, Sensitivity, ToolExecutionMode, ToolId,
    ValidatedToolCall,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, ArtifactMetadata, ArtifactScope, ArtifactStore,
    PendingToolEffect, PortFuture, ReconcileContext, RunCallContext, SideEffectClass,
    ToolCallContext, ToolDeferralSupport, ToolError, ToolEventStream, ToolReconcileResult,
    ToolResult, ToolSpec, ToolStreamItem, Toolset, ToolsetDescriptor, stage_required_artifact,
};
use futures_util::stream;
use serde::Deserialize;

use crate::record::{
    ExtractionMethod, MemoryBody, MemoryClock, MemoryError, MemoryId, MemoryProvenance,
    MemoryRecord, MemoryScope, RetentionPolicy,
};
use crate::store::{MatchEvidence, MemoryQuery, MemoryStore, MemoryStoreError, PutOutcome};

/// Inline-vs-blob threshold for a memory record body, in bytes.
pub const INLINE_BODY_MAX_BYTES: usize = 4096;

/// Bounded number of characters kept in a staged blob's preview.
const PREVIEW_CHAR_LIMIT: usize = 256;

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

const REMEMBER_INPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"keywords":{"type":"array","items":{"type":"string"}},"body":{"type":"string"},"sensitivity":{"type":"string"}},"required":["keywords","body"]}"#;
const REMEMBER_OUTPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"outcome":{"type":"string","enum":["inserted","already_applied"]}},"required":["id","outcome"]}"#;

const SEARCH_INPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"keywords":{"type":"array","items":{"type":"string"}},"text":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":25}},"required":[]}"#;
const SEARCH_OUTPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"hits":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"preview":{"type":"string"},"score":{"type":"integer"},"matched":{"type":"string"}},"required":["id","preview","score","matched"]}}},"required":["hits"]}"#;

const INSPECT_INPUT_SCHEMA: &[u8] =
    br#"{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"}},"required":["id"]}"#;

const FORGET_INPUT_SCHEMA: &[u8] =
    br#"{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"}},"required":["id"]}"#;
const FORGET_OUTPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"tombstoned":{"type":"boolean"}},"required":["id","tombstoned"]}"#;

const CORRECT_INPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"old_id":{"type":"string"},"keywords":{"type":"array","items":{"type":"string"}},"body":{"type":"string"},"sensitivity":{"type":"string"}},"required":["old_id","keywords","body"]}"#;
const CORRECT_OUTPUT_SCHEMA: &[u8] = br#"{"type":"object","additionalProperties":false,"properties":{"old_id":{"type":"string"},"new_id":{"type":"string"}},"required":["old_id","new_id"]}"#;

/// Which memory tools a [`MemoryToolset`] exposes.
///
/// `consolidate` and `profile` are reserved for later capabilities: they are
/// accepted here but currently gate nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct MemoryPolicy {
    /// Gates `search_memory` and `inspect_memory`.
    pub read: bool,
    /// Gates `remember`.
    pub write: bool,
    /// Gates `forget_memory` and `correct_memory`.
    pub manage: bool,
    /// Reserved; gates nothing yet.
    pub consolidate: bool,
    /// Reserved; gates nothing yet.
    pub profile: bool,
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            read: true,
            write: true,
            manage: false,
            consolidate: false,
            profile: false,
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
    /// Construct the toolset and its five cached tool specifications.
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
        let specs = Arc::from([
            build_spec(
                REMEMBER_TOOL_ID,
                REMEMBER_NAME,
                "Remember",
                "Store a memory record for later recall.",
                REMEMBER_INPUT_SCHEMA,
                Some(REMEMBER_OUTPUT_SCHEMA),
                SideEffectClass::IdempotentWrite,
            )?,
            build_spec(
                SEARCH_TOOL_ID,
                SEARCH_NAME,
                "Search Memory",
                "Search memory records by exact id, keywords, or full text.",
                SEARCH_INPUT_SCHEMA,
                Some(SEARCH_OUTPUT_SCHEMA),
                SideEffectClass::ReadOnly,
            )?,
            build_spec(
                INSPECT_TOOL_ID,
                INSPECT_NAME,
                "Inspect Memory",
                "Fetch full metadata for one memory record by id.",
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
            validate_call_context(&ctx, &call, &expected_id)?;
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

    async fn remember(
        &self,
        run: &RunCallContext,
        arguments: &[u8],
    ) -> Result<ToolResult, ToolError> {
        let args: RememberArguments = parse_arguments(arguments)?;
        let sensitivity = parse_sensitivity(args.sensitivity.as_deref())?;
        let id = args.id.unwrap_or_else(|| derive_id(&args.body));
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
        let outcome = self
            .store
            .put(Self::idempotency_key(run), record)
            .await
            .map_err(|error| map_store_error(&error))?;
        let outcome_str = match outcome {
            PutOutcome::Inserted => "inserted",
            PutOutcome::AlreadyApplied => "already_applied",
        };
        ok_result(&serde_json::json!({ "id": memory_id.as_str(), "outcome": outcome_str }))
    }

    async fn build_body(
        &self,
        run: &RunCallContext,
        id: &MemoryId,
        body: &str,
        sensitivity: Sensitivity,
    ) -> Result<(MemoryBody, Arc<str>), ToolError> {
        let preview: Arc<str> =
            Arc::from(body.chars().take(PREVIEW_CHAR_LIMIT).collect::<String>());
        if body.len() <= INLINE_BODY_MAX_BYTES {
            return Ok((MemoryBody::Inline(Arc::from(body)), preview));
        }
        let artifact = stage_required_artifact(
            self.artifact_store.as_ref(),
            ArtifactScope {
                tenant_scope: Arc::clone(&self.scope.tenant),
                session_id: run.locator.session_id,
                run_id: Some(run.locator.run_id),
                sensitivity,
            },
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
        Ok((MemoryBody::Blob(artifact), preview))
    }

    async fn search(
        &self,
        _run: &RunCallContext,
        arguments: &[u8],
    ) -> Result<ToolResult, ToolError> {
        let args: SearchArguments = parse_arguments(arguments)?;
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
            MemoryQuery::FullText(Arc::from(text.as_str()))
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
            .map_err(|error| map_store_error(&error))?;
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
                        MemoryBody::Blob(artifact) => serde_json::json!({
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
        let old_id = MemoryId::parse(&args.old_id)
            .map_err(|_| invalid_arguments("memory old_id is invalid"))?;
        let sensitivity = parse_sensitivity(args.sensitivity.as_deref())?;
        let new_id_str = derive_id(&args.body);
        let new_id =
            MemoryId::parse(&new_id_str).map_err(|_| invalid_arguments("memory id is invalid"))?;
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
            Ok(()) => ok_result(&serde_json::json!({
                "old_id": old_id.as_str(),
                "new_id": new_id.as_str(),
            })),
            Err(MemoryStoreError::NotFound | MemoryStoreError::ScopeMismatch) => not_found_result(),
            Err(error) => Err(map_store_error(&error)),
        }
    }
}

fn matched_str(matched: &MatchEvidence) -> String {
    match matched {
        MatchEvidence::ExactId => "exact_id".to_owned(),
        MatchEvidence::Keyword(keyword) => keyword.to_string(),
        MatchEvidence::FullText => "full_text".to_owned(),
    }
}

fn derive_id(body: &str) -> String {
    let digest = Digest::blob_content(body.as_bytes());
    let hex = digest.to_hex();
    format!("mem-{}", &hex[..16.min(hex.len())])
}

fn parse_sensitivity(value: Option<&str>) -> Result<Sensitivity, ToolError> {
    match value {
        None => Ok(Sensitivity::Internal),
        Some(raw) => serde_json::from_value(serde_json::Value::String(raw.to_owned()))
            .map_err(|_| invalid_arguments("memory sensitivity is invalid")),
    }
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
    ok_result(&serde_json::json!({ "code": MEMORY_TOOL_NOT_FOUND })).map(|mut result| {
        result.is_error = true;
        result
    })
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
        MemoryStoreError::NotFound | MemoryStoreError::ScopeMismatch => tool_error(
            MEMORY_TOOL_NOT_FOUND,
            ErrorCategory::Tool,
            false,
            "memory record is not visible",
        ),
    }
}

fn validate_call_context(
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
    Ok(())
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
