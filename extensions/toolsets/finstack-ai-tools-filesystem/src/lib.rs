//! Capability-scoped filesystem implementation of the public `Toolset` port.

#![warn(missing_docs)]
#![cfg_attr(
    not(unix),
    allow(
        dead_code,
        reason = "the public constructor fails closed when no capability-safe backend exists"
    )
)]

mod operation;
mod policy;
#[cfg(unix)]
mod unix;

use std::path::Path;
use std::sync::Arc;

use finstack_ai_kernel::{
    ErrorCategory, Metadata, RawJson, RetrySafety, Sensitivity, ToolExecutionMode, ToolId,
    ValidatedToolCall,
};
#[cfg(unix)]
use finstack_ai_runtime::ToolStreamItem;
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, ArtifactMetadata, ArtifactScope, ArtifactStore, Bytes,
    PortFuture, SideEffectClass, ToolCallContext, ToolDeferralSupport, ToolError, ToolEventStream,
    ToolResult, ToolSpec, Toolset, ToolsetDescriptor, stage_required_artifact, verify_authority,
};
#[cfg(unix)]
use futures_util::stream;
use serde::Deserialize;
use thiserror::Error;

use crate::operation::{FileOperation, OperationOutput};
use crate::policy::{ProtectedPaths, ValidatedPath};

/// Stable unsupported-platform/configuration code.
pub const FILESYSTEM_UNSUPPORTED: &str = "filesystem_unsupported";
/// Stable invalid-argument code.
pub const FILESYSTEM_INVALID_ARGUMENTS: &str = "filesystem_invalid_arguments";
/// Stable path-policy denial code.
pub const FILESYSTEM_POLICY_DENIED: &str = "filesystem_policy_denied";
/// Stable missing-object code.
pub const FILESYSTEM_NOT_FOUND: &str = "filesystem_not_found";
/// Stable non-secret I/O code.
pub const FILESYSTEM_IO_ERROR: &str = "filesystem_io_error";
/// Stable resource-bound code.
pub const FILESYSTEM_LIMIT_EXCEEDED: &str = "filesystem_limit_exceeded";
/// Stable required-artifact-service code.
pub const FILESYSTEM_ARTIFACT_REQUIRED: &str = "filesystem_artifact_required";

const READ_ID: &str = "finstack.tools.filesystem.read";
const WRITE_ID: &str = "finstack.tools.filesystem.write";
const EDIT_ID: &str = "finstack.tools.filesystem.edit";
const LIST_ID: &str = "finstack.tools.filesystem.list";
const GLOB_ID: &str = "finstack.tools.filesystem.glob";
const SEARCH_ID: &str = "finstack.tools.filesystem.search";
const MAX_INLINE_RESULT_BYTES: usize = 64 * 1024;

/// Explicit filesystem resource ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSystemLimits {
    /// Maximum bytes read from or written to one file.
    pub file_bytes: usize,
    /// Maximum filesystem objects visited by list/glob/search.
    pub visited_entries: usize,
    /// Maximum returned search matches.
    pub search_matches: usize,
    /// Maximum recursion depth for glob/search.
    pub recursion_depth: usize,
    /// Maximum successful JSON result retained inline.
    pub inline_result_bytes: usize,
}

impl Default for FileSystemLimits {
    fn default() -> Self {
        Self {
            file_bytes: 1024 * 1024,
            visited_entries: 16_384,
            search_matches: 2_048,
            recursion_depth: 64,
            inline_result_bytes: 64 * 1024,
        }
    }
}

impl FileSystemLimits {
    fn validate(self) -> Result<Self, FileSystemError> {
        if self.file_bytes == 0
            || self.file_bytes > finstack_ai_runtime::MAX_ARTIFACT_BYTES
            || self.visited_entries == 0
            || self.visited_entries > 1_000_000
            || self.search_matches == 0
            || self.search_matches > self.visited_entries
            || self.recursion_depth == 0
            || self.recursion_depth > 256
            || self.inline_result_bytes == 0
            || self.inline_result_bytes > MAX_INLINE_RESULT_BYTES
        {
            return Err(FileSystemError::Configuration {
                reason: "invalid_filesystem_limits",
            });
        }
        Ok(self)
    }
}

/// Filesystem construction or policy failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FileSystemError {
    /// The target lacks the required capability-safe primitive set.
    #[error("{FILESYSTEM_UNSUPPORTED}: safe filesystem primitives are unavailable")]
    Unsupported,
    /// Configuration is malformed or outside fixed ceilings.
    #[error("filesystem_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// The authorized root could not be opened safely.
    #[error("{FILESYSTEM_IO_ERROR}: authorized root could not be opened safely")]
    RootUnavailable,
}

/// Trusted native filesystem toolset rooted in one already-opened capability.
pub struct FileSystemToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    tool_ids: Arc<[ToolId]>,
    limits: FileSystemLimits,
    protected: ProtectedPaths,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    sensitivity: Sensitivity,
    #[cfg(unix)]
    root: unix::Root,
    #[cfg(not(unix))]
    root: (),
}

impl core::fmt::Debug for FileSystemToolset {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("FileSystemToolset")
            .field("limits", &self.limits)
            .field("protected", &self.protected)
            .field("artifact_store", &self.artifact_store.is_some())
            .field("sensitivity", &self.sensitivity)
            .finish_non_exhaustive()
    }
}

impl FileSystemToolset {
    /// Open an explicit root with default bounds and protected names.
    ///
    /// # Errors
    ///
    /// Fails closed when the root cannot be opened without following a
    /// symlink, the target lacks the required safe primitives, or a checked-in
    /// tool descriptor is invalid.
    #[cfg(unix)]
    pub fn try_new(root: impl AsRef<Path>) -> Result<Self, FileSystemError> {
        let root = unix::Root::open(root.as_ref())?;
        let (tools, tool_ids) = build_tools()?;
        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-filesystem"),
                metadata: Metadata::empty(),
            },
            tools,
            tool_ids,
            limits: FileSystemLimits::default(),
            protected: ProtectedPaths::defaults(),
            artifact_store: None,
            sensitivity: Sensitivity::Internal,
            root,
        })
    }

    /// Reject construction on targets without the required capability-safe
    /// filesystem primitive set.
    ///
    /// # Errors
    ///
    /// Always returns [`FileSystemError::Unsupported`].
    #[cfg(not(unix))]
    pub fn try_new(root: impl AsRef<Path>) -> Result<Self, FileSystemError> {
        let _ = root;
        Err(FileSystemError::Unsupported)
    }

    /// Replace the resource ceilings.
    ///
    /// # Errors
    ///
    /// Rejects zero or excessive bounds.
    pub fn try_with_limits(mut self, limits: FileSystemLimits) -> Result<Self, FileSystemError> {
        self.limits = limits.validate()?;
        Ok(self)
    }

    /// Replace the default protected path patterns.
    ///
    /// Plain component names protect that name at any depth. Patterns may use
    /// `*`, `**`, and `?` over slash-separated relative UTF-8 paths.
    ///
    /// # Errors
    ///
    /// Rejects empty, absolute, parent-bearing, NUL-bearing, or excessive
    /// patterns.
    pub fn try_with_protected_paths<I, S>(mut self, patterns: I) -> Result<Self, FileSystemError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.protected = ProtectedPaths::try_new(patterns)?;
        Ok(self)
    }

    /// Attach the scoped store required for results above the inline ceiling.
    #[must_use]
    pub fn with_artifact_store(
        mut self,
        store: Arc<dyn ArtifactStore>,
        sensitivity: Sensitivity,
    ) -> Self {
        self.artifact_store = Some(store);
        self.sensitivity = sensitivity;
        self
    }
}

impl Toolset for FileSystemToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        #[cfg(not(unix))]
        {
            let _ = (ctx, call);
            Box::pin(async {
                Err(fs_tool_error(
                    FILESYSTEM_UNSUPPORTED,
                    ErrorCategory::Configuration,
                    "safe filesystem primitives are unavailable",
                ))
            })
        }

        #[cfg(unix)]
        {
            let tool_ids = Arc::clone(&self.tool_ids);
            let limits = self.limits;
            let protected = self.protected.clone();
            let artifact_store = self.artifact_store.clone();
            let sensitivity = self.sensitivity;
            let root = self.root.clone();
            Box::pin(async move {
                verify_authority(&ctx)?;
                let operation = decode_operation(&call, &tool_ids, &protected, limits)?;
                let cancellation = ctx.run.cancellation.clone();
                let output = tokio::task::spawn_blocking(move || {
                    operation.execute(&root, limits, &protected, &cancellation)
                })
                .await
                .map_err(|_| {
                    fs_tool_error(
                        FILESYSTEM_IO_ERROR,
                        ErrorCategory::Internal,
                        "filesystem worker failed",
                    )
                })??;

                let result =
                    normalize_output(output, &ctx, limits, artifact_store, sensitivity).await?;
                Ok(Box::pin(stream::once(async move {
                    Ok(ToolStreamItem::Completed(result))
                })) as ToolEventStream)
            })
        }
    }
}

async fn normalize_output(
    output: OperationOutput,
    ctx: &ToolCallContext,
    limits: FileSystemLimits,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    sensitivity: Sensitivity,
) -> Result<ToolResult, ToolError> {
    if output.json.len() <= limits.inline_result_bytes {
        return Ok(ToolResult {
            output: RawJson::parse(output.json).map_err(|_| {
                fs_tool_error(
                    FILESYSTEM_IO_ERROR,
                    ErrorCategory::Internal,
                    "filesystem result normalization failed",
                )
            })?,
            is_error: false,
        });
    }
    let store = artifact_store.ok_or_else(|| {
        fs_tool_error(
            FILESYSTEM_ARTIFACT_REQUIRED,
            ErrorCategory::Limit,
            "filesystem result requires an artifact store",
        )
    })?;
    let effect_id = ctx.run.effect_id;
    let attributes = Metadata::parse(
        serde_json::to_vec(&serde_json::json!({ "effect_id": effect_id.to_string() })).map_err(
            |_| {
                fs_tool_error(
                    FILESYSTEM_IO_ERROR,
                    ErrorCategory::Internal,
                    "artifact metadata serialization failed",
                )
            },
        )?,
    )
    .map_err(|_| {
        fs_tool_error(
            FILESYSTEM_IO_ERROR,
            ErrorCategory::Internal,
            "artifact metadata normalization failed",
        )
    })?;
    let artifact = stage_required_artifact(
        store.as_ref(),
        ArtifactScope {
            tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
            session_id: ctx.run.locator.session_id,
            run_id: Some(ctx.run.locator.run_id),
            sensitivity,
        },
        Bytes::from(output.json),
        ArtifactMetadata {
            kind: Arc::from("tool-output"),
            media_type: Arc::from("application/json"),
            name: Some(Arc::from(output.artifact_name)),
            attributes,
        },
    )
    .await
    .map_err(|_| {
        fs_tool_error(
            FILESYSTEM_ARTIFACT_REQUIRED,
            ErrorCategory::Tool,
            "filesystem artifact staging failed",
        )
    })?;
    let reference =
        serde_json::to_vec(&serde_json::json!({ "artifact": artifact })).map_err(|_| {
            fs_tool_error(
                FILESYSTEM_IO_ERROR,
                ErrorCategory::Internal,
                "artifact reference serialization failed",
            )
        })?;
    Ok(ToolResult {
        output: RawJson::parse(reference).map_err(|_| {
            fs_tool_error(
                FILESYSTEM_IO_ERROR,
                ErrorCategory::Internal,
                "artifact reference normalization failed",
            )
        })?,
        is_error: false,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArguments {
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArguments {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditArguments {
    path: String,
    old: String,
    new: String,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArguments {
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GlobArguments {
    pattern: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArguments {
    query: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    glob: Option<String>,
}

fn decode_operation(
    call: &ValidatedToolCall,
    tool_ids: &[ToolId],
    protected: &ProtectedPaths,
    limits: FileSystemLimits,
) -> Result<FileOperation, ToolError> {
    let raw = call.call.arguments().as_bytes();
    let position = tool_ids
        .iter()
        .position(|id| id == &call.tool_id)
        .ok_or_else(|| {
            fs_tool_error(
                FILESYSTEM_INVALID_ARGUMENTS,
                ErrorCategory::Validation,
                "unknown filesystem tool identity",
            )
        })?;
    let expected_name = [
        "filesystem_read",
        "filesystem_write",
        "filesystem_edit",
        "filesystem_list",
        "filesystem_glob",
        "filesystem_search",
    ][position];
    if call.call.tool_name() != expected_name {
        return Err(fs_tool_error(
            FILESYSTEM_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            "filesystem tool name does not match its identity",
        ));
    }
    match position {
        0 => {
            let args: PathArguments = decode(raw)?;
            Ok(FileOperation::Read(ValidatedPath::try_file(
                &args.path, protected,
            )?))
        }
        1 => {
            let args: WriteArguments = decode(raw)?;
            if args.content.len() > limits.file_bytes {
                return Err(limit_error(
                    "filesystem write exceeds the configured byte limit",
                ));
            }
            Ok(FileOperation::Write {
                path: ValidatedPath::try_file(&args.path, protected)?,
                content: args.content.into_bytes(),
            })
        }
        2 => {
            let args: EditArguments = decode(raw)?;
            if args.old.is_empty() || args.new.len() > limits.file_bytes {
                return Err(invalid_error("filesystem edit arguments are invalid"));
            }
            Ok(FileOperation::Edit {
                path: ValidatedPath::try_file(&args.path, protected)?,
                old: args.old,
                new: args.new,
                replace_all: args.replace_all,
            })
        }
        3 => {
            let args: ListArguments = decode(raw)?;
            Ok(FileOperation::List(ValidatedPath::try_directory(
                &args.path, protected,
            )?))
        }
        4 => {
            let args: GlobArguments = decode(raw)?;
            policy::validate_glob(&args.pattern)?;
            Ok(FileOperation::Glob {
                pattern: args.pattern,
            })
        }
        5 => {
            let args: SearchArguments = decode(raw)?;
            if args.query.is_empty()
                || args.query.len() > 1_024
                || args.query.as_bytes().contains(&0)
            {
                return Err(invalid_error("filesystem search query is invalid"));
            }
            if let Some(pattern) = &args.glob {
                policy::validate_glob(pattern)?;
            }
            Ok(FileOperation::Search {
                path: ValidatedPath::try_directory(&args.path, protected)?,
                query: args.query,
                glob: args.glob,
            })
        }
        _ => Err(invalid_error("unknown filesystem operation")),
    }
}

fn decode<T: for<'de> Deserialize<'de>>(raw: &[u8]) -> Result<T, ToolError> {
    serde_json::from_slice(raw).map_err(|_| invalid_error("filesystem arguments are invalid"))
}

type BuiltTools = (Arc<[ToolSpec]>, Arc<[ToolId]>);

#[expect(
    clippy::too_many_lines,
    reason = "the six immutable public descriptors stay together for direct review"
)]
fn build_tools() -> Result<BuiltTools, FileSystemError> {
    let definitions = [
        (
            READ_ID,
            "filesystem_read",
            "Read file",
            "Read one UTF-8 file under the authorized root.",
            read_schema(),
            SideEffectClass::ReadOnly,
            RetrySafety::SafeToRetry,
            ApprovalRequirement::NotRequired,
        ),
        (
            WRITE_ID,
            "filesystem_write",
            "Write file",
            "Create or replace one UTF-8 file under the authorized root.",
            write_schema(),
            SideEffectClass::IdempotentWrite,
            RetrySafety::IdempotentWithKey,
            ApprovalRequirement::Policy,
        ),
        (
            EDIT_ID,
            "filesystem_edit",
            "Edit file",
            "Replace exact UTF-8 content in one file under the authorized root.",
            edit_schema(),
            SideEffectClass::NonIdempotentWrite,
            RetrySafety::AtMostOnce,
            ApprovalRequirement::Policy,
        ),
        (
            LIST_ID,
            "filesystem_list",
            "List directory",
            "List one directory under the authorized root without following symlinks.",
            list_schema(),
            SideEffectClass::ReadOnly,
            RetrySafety::SafeToRetry,
            ApprovalRequirement::NotRequired,
        ),
        (
            GLOB_ID,
            "filesystem_glob",
            "Glob files",
            "Find bounded relative paths under the authorized root.",
            glob_schema(),
            SideEffectClass::ReadOnly,
            RetrySafety::SafeToRetry,
            ApprovalRequirement::NotRequired,
        ),
        (
            SEARCH_ID,
            "filesystem_search",
            "Search content",
            "Find bounded literal UTF-8 matches under the authorized root.",
            search_schema(),
            SideEffectClass::ReadOnly,
            RetrySafety::SafeToRetry,
            ApprovalRequirement::NotRequired,
        ),
    ];
    let mut tools = Vec::with_capacity(definitions.len());
    let mut ids = Vec::with_capacity(definitions.len());
    for (id, name, title, description, schema, side_effect, retry_safety, requirement) in
        definitions
    {
        let id = ToolId::parse(id).map_err(|_| FileSystemError::Configuration {
            reason: "invalid_tool_id",
        })?;
        let spec = ToolSpec {
            id: id.clone(),
            model_name: Arc::from(name),
            title: Arc::from(title),
            description: Arc::from(description),
            input_schema: RawJson::parse(schema.as_bytes()).map_err(|_| {
                FileSystemError::Configuration {
                    reason: "invalid_tool_schema",
                }
            })?,
            output_schema: None,
            execution: if id.as_str() == WRITE_ID || id.as_str() == EDIT_ID {
                ToolExecutionMode::Sequential
            } else {
                ToolExecutionMode::Parallel
            },
            side_effect,
            retry_safety,
            approval: ApprovalMetadata {
                requirement,
                reason: (requirement != ApprovalRequirement::NotRequired)
                    .then(|| Arc::from("Filesystem mutation is subject to host policy.")),
                attributes: Metadata::empty(),
            },
            max_result_bytes: u64::try_from(MAX_INLINE_RESULT_BYTES).unwrap_or(65_536),
            metadata: Metadata::empty(),
            deferral: ToolDeferralSupport::Never,
        };
        spec.validate()
            .map_err(|_| FileSystemError::Configuration {
                reason: "invalid_tool_spec",
            })?;
        ids.push(id);
        tools.push(spec);
    }
    Ok((Arc::from(tools), Arc::from(ids)))
}

fn object_schema(properties: &str, required: &str) -> String {
    format!(
        r#"{{"additionalProperties":false,"properties":{{{properties}}},"required":[{required}],"type":"object"}}"#
    )
}

fn read_schema() -> String {
    object_schema(r#""path":{"type":"string"}"#, r#""path""#)
}
fn write_schema() -> String {
    object_schema(
        r#""content":{"type":"string"},"path":{"type":"string"}"#,
        r#""path","content""#,
    )
}
fn edit_schema() -> String {
    object_schema(
        r#""new":{"type":"string"},"old":{"type":"string"},"path":{"type":"string"},"replace_all":{"default":false,"type":"boolean"}"#,
        r#""path","old","new""#,
    )
}
fn list_schema() -> String {
    object_schema(r#""path":{"default":"","type":"string"}"#, "")
}
fn glob_schema() -> String {
    object_schema(r#""pattern":{"type":"string"}"#, r#""pattern""#)
}
fn search_schema() -> String {
    object_schema(
        r#""glob":{"type":"string"},"path":{"default":"","type":"string"},"query":{"type":"string"}"#,
        r#""query""#,
    )
}

pub(crate) fn invalid_error(message: &'static str) -> ToolError {
    fs_tool_error(
        FILESYSTEM_INVALID_ARGUMENTS,
        ErrorCategory::Validation,
        message,
    )
}

pub(crate) fn policy_error(message: &'static str) -> ToolError {
    fs_tool_error(FILESYSTEM_POLICY_DENIED, ErrorCategory::Tool, message)
}

pub(crate) fn limit_error(message: &'static str) -> ToolError {
    fs_tool_error(FILESYSTEM_LIMIT_EXCEEDED, ErrorCategory::Limit, message)
}

pub(crate) fn io_error(message: &'static str) -> ToolError {
    fs_tool_error(FILESYSTEM_IO_ERROR, ErrorCategory::Tool, message)
}

pub(crate) fn fs_tool_error(
    code: &'static str,
    category: ErrorCategory,
    message: &'static str,
) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}

#[cfg(all(test, unix))]
mod tests;

#[cfg(all(test, not(unix)))]
mod unsupported_platform_tests {
    use super::{FileSystemError, FileSystemToolset};

    #[test]
    fn construction_fails_closed_without_capability_safe_primitives() {
        assert!(matches!(
            FileSystemToolset::try_new("."),
            Err(FileSystemError::Unsupported)
        ));
    }
}
