//! Deny-by-default argv shell implementation of the public `Toolset` port.

#![warn(missing_docs)]
#![cfg_attr(
    not(unix),
    allow(
        dead_code,
        reason = "the public constructor fails closed when no capability-safe backend exists"
    )
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use finstack_ai_runtime::ToolStreamItem;
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, ArtifactMetadata, ArtifactScope, ArtifactStore, Bytes,
    ErrorCategory, Metadata, PortFuture, RawJson, Sensitivity, SideEffectClass, Timestamp,
    ToolCallContext, ToolError, ToolEventStream, ToolExecutionMode, ToolId, ToolResult, ToolSpec,
    Toolset, ToolsetDescriptor, ValidatedToolCall, stage_required_artifact,
};
#[cfg(unix)]
use futures_util::stream;
use serde::Deserialize;
use thiserror::Error;

/// Stable unsupported-platform code.
pub const SHELL_UNSUPPORTED: &str = "shell_unsupported";
/// Stable invalid-argument code.
pub const SHELL_INVALID_ARGUMENTS: &str = "shell_invalid_arguments";
/// Stable deny-by-default policy code.
pub const SHELL_POLICY_DENIED: &str = "shell_policy_denied";
/// Stable timeout/cancellation code.
pub const SHELL_TIMEOUT: &str = "shell_timeout";
/// Stable output-flood code.
pub const SHELL_LIMIT_EXCEEDED: &str = "shell_limit_exceeded";
/// Stable required-artifact-service code.
pub const SHELL_ARTIFACT_REQUIRED: &str = "shell_artifact_required";
/// Stable I/O code.
pub const SHELL_IO_ERROR: &str = "shell_io_error";

const TOOL_ID: &str = "finstack.tools.shell.exec";
const TOOL_NAME: &str = "shell_exec";
const MAX_ARGV: usize = 64;
const MAX_ARG_BYTES: usize = 8 * 1024;
const MAX_INLINE_RESULT_BYTES: usize = 64 * 1024;

/// Explicit shell resource ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellLimits {
    /// Maximum wall time for one execution.
    pub timeout: Duration,
    /// Maximum combined stdout+stderr bytes retained.
    pub max_output_bytes: usize,
    /// Maximum successful JSON result retained inline.
    pub inline_result_bytes: usize,
}

impl Default for ShellLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            max_output_bytes: 64 * 1024,
            inline_result_bytes: 8 * 1024,
        }
    }
}

/// Deny-by-default executable policy.
#[derive(Debug, Clone)]
pub struct ShellPolicy {
    allowed: Arc<[Arc<str>]>,
    allow_path_search: bool,
    search_path: Arc<[Arc<str>]>,
    extra_env: BTreeMap<Arc<str>, Arc<str>>,
}

impl ShellPolicy {
    /// Allow only the listed basenames or exact paths.
    ///
    /// # Errors
    ///
    /// Rejects an empty allowlist or empty/NUL program names.
    pub fn try_new<I, S>(allowed: I) -> Result<Self, ShellError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let allowed = allowed
            .into_iter()
            .map(|value| Arc::<str>::from(value.as_ref()))
            .collect::<Vec<_>>();
        if allowed.is_empty()
            || allowed.iter().any(|value| {
                value.is_empty() || value.len() > 4_096 || value.as_bytes().contains(&0)
            })
        {
            return Err(ShellError::Configuration {
                reason: "invalid_shell_allowlist",
            });
        }
        Ok(Self {
            allowed: allowed.into(),
            allow_path_search: false,
            search_path: Arc::from([]),
            extra_env: BTreeMap::new(),
        })
    }

    /// Opt in to a bounded PATH search for basename-only programs.
    ///
    /// # Errors
    ///
    /// Rejects empty or non-absolute search directories.
    pub fn try_with_path_search<I, S>(mut self, search_path: I) -> Result<Self, ShellError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let search_path = search_path
            .into_iter()
            .map(|value| Arc::<str>::from(value.as_ref()))
            .collect::<Vec<_>>();
        if search_path.is_empty()
            || search_path.iter().any(|value| {
                !value.starts_with('/')
                    || value.contains("//")
                    || value.contains('\0')
                    || value.split('/').any(|component| component == "..")
            })
        {
            return Err(ShellError::Configuration {
                reason: "invalid_shell_search_path",
            });
        }
        self.allow_path_search = true;
        self.search_path = search_path.into();
        Ok(self)
    }

    /// Add the documented locale pair. Host environment values are never copied.
    #[must_use]
    pub fn with_locale_env(mut self) -> Self {
        self.extra_env
            .insert(Arc::from("LANG"), Arc::from("C.UTF-8"));
        self.extra_env.insert(Arc::from("LC_ALL"), Arc::from("C"));
        self
    }

    fn authorize(&self, program: &str) -> Result<PathBuf, ToolError> {
        if program.is_empty() || program.as_bytes().contains(&0) {
            return Err(invalid_error("shell program is invalid"));
        }
        let has_separator = program.contains('/') || program.contains('\\');
        if has_separator {
            if self
                .allowed
                .iter()
                .any(|allowed| allowed.as_ref() == program)
            {
                return Ok(PathBuf::from(program));
            }
            return Err(policy_error("shell program path is not allowlisted"));
        }
        if self
            .allowed
            .iter()
            .any(|allowed| allowed.as_ref() == program)
        {
            if self.allow_path_search {
                return self.search(program);
            }
            return Err(policy_error(
                "shell basename requires an exact allowlisted path",
            ));
        }
        if self.allow_path_search
            && self.allowed.iter().any(|allowed| {
                Path::new(allowed.as_ref())
                    .file_name()
                    .and_then(|name| name.to_str())
                    == Some(program)
            })
        {
            return self.search(program);
        }
        Err(policy_error("shell program is not allowlisted"))
    }

    fn search(&self, program: &str) -> Result<PathBuf, ToolError> {
        for directory in self.search_path.iter() {
            let candidate = Path::new(directory.as_ref()).join(program);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        Err(policy_error(
            "shell program was not found on the search path",
        ))
    }

    fn environment(&self) -> BTreeMap<String, String> {
        self.extra_env
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }
}

/// Authorized command presented to a [`CommandSandbox`].
#[derive(Debug, Clone)]
pub struct SandboxedCommand {
    /// Resolved executable path.
    pub program: PathBuf,
    /// Argument vector including `argv[0]`.
    pub argv: Arc<[Arc<str>]>,
    /// Explicit environment; never a host dump.
    pub env: BTreeMap<String, String>,
    /// Optional authorized working-directory path.
    pub cwd: Option<PathBuf>,
    /// Relative timeout.
    pub timeout: Duration,
    /// Combined output ceiling.
    pub max_output_bytes: usize,
}

/// Captured sandbox output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxedOutput {
    /// Standard output bytes.
    pub stdout: Vec<u8>,
    /// Standard error bytes.
    pub stderr: Vec<u8>,
    /// Process exit code.
    pub exit_code: i32,
}

/// Optional host-injected execution adapter. The default is in-process.
pub trait CommandSandbox: Send + Sync {
    /// Execute one authorized command.
    fn run(
        &self,
        request: SandboxedCommand,
        cancellation: finstack_ai_runtime::CancellationSignal,
        deadline: Option<Timestamp>,
    ) -> PortFuture<Result<SandboxedOutput, ToolError>>;
}

/// Default in-process `std::process` sandbox.
#[derive(Debug, Default)]
pub struct ProcessCommandSandbox;

impl CommandSandbox for ProcessCommandSandbox {
    fn run(
        &self,
        request: SandboxedCommand,
        cancellation: finstack_ai_runtime::CancellationSignal,
        deadline: Option<Timestamp>,
    ) -> PortFuture<Result<SandboxedOutput, ToolError>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || run_process(&request, &cancellation, deadline))
                .await
                .map_err(|_| {
                    tool_error(
                        SHELL_IO_ERROR,
                        ErrorCategory::Internal,
                        "shell worker failed",
                    )
                })?
        })
    }
}

/// Shell construction or policy failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ShellError {
    /// The target lacks the required capability-safe primitive set.
    #[error("{SHELL_UNSUPPORTED}: safe shell primitives are unavailable")]
    Unsupported,
    /// Configuration is malformed or outside fixed ceilings.
    #[error("shell_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// The authorized root could not be opened safely.
    #[error("{SHELL_IO_ERROR}: authorized root could not be opened safely")]
    RootUnavailable,
}

/// Trusted native argv shell toolset.
pub struct ShellToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    tool_id: ToolId,
    policy: ShellPolicy,
    limits: ShellLimits,
    sandbox: Arc<dyn CommandSandbox>,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    sensitivity: Sensitivity,
    #[cfg(unix)]
    root: Option<unix::Root>,
    #[cfg(not(unix))]
    root: Option<()>,
}

impl core::fmt::Debug for ShellToolset {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ShellToolset")
            .field("limits", &self.limits)
            .field("artifact_store", &self.artifact_store.is_some())
            .finish_non_exhaustive()
    }
}

impl ShellToolset {
    /// Construct a Unix shell toolset with an optional authorized cwd root.
    ///
    /// # Errors
    ///
    /// Fails closed when a checked-in descriptor is invalid, the policy is
    /// empty, or a supplied root cannot be opened without following a symlink.
    #[cfg(unix)]
    pub fn try_new(policy: ShellPolicy, root: Option<&Path>) -> Result<Self, ShellError> {
        let (tools, tool_id) = build_tools()?;
        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-shell"),
                metadata: Metadata::empty(),
            },
            tools,
            tool_id,
            policy,
            limits: ShellLimits::default(),
            sandbox: Arc::new(ProcessCommandSandbox),
            artifact_store: None,
            sensitivity: Sensitivity::Internal,
            root: root.map(unix::Root::open).transpose()?,
        })
    }

    /// Reject construction on targets without the required primitives.
    ///
    /// # Errors
    ///
    /// Always returns [`ShellError::Unsupported`].
    #[cfg(not(unix))]
    pub fn try_new(_policy: ShellPolicy, _root: Option<&Path>) -> Result<Self, ShellError> {
        Err(ShellError::Unsupported)
    }

    /// Replace resource ceilings.
    ///
    /// # Errors
    ///
    /// Rejects a zero timeout or inverted output bounds.
    pub fn try_with_limits(mut self, limits: ShellLimits) -> Result<Self, ShellError> {
        if limits.timeout.is_zero()
            || limits.max_output_bytes == 0
            || limits.inline_result_bytes == 0
            || limits.inline_result_bytes > MAX_INLINE_RESULT_BYTES
            || limits.inline_result_bytes > limits.max_output_bytes
        {
            return Err(ShellError::Configuration {
                reason: "invalid_shell_limits",
            });
        }
        self.limits = limits;
        Ok(self)
    }

    /// Inject a host sandbox adapter.
    #[must_use]
    pub fn with_sandbox(mut self, sandbox: Arc<dyn CommandSandbox>) -> Self {
        self.sandbox = sandbox;
        self
    }

    /// Attach the artifact store used for oversized output.
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

impl Toolset for ShellToolset {
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
            return Box::pin(async {
                Err(tool_error(
                    SHELL_UNSUPPORTED,
                    ErrorCategory::Configuration,
                    "safe shell primitives are unavailable",
                ))
            });
        }

        #[cfg(unix)]
        {
            let policy = self.policy.clone();
            let limits = self.limits;
            let sandbox = Arc::clone(&self.sandbox);
            let artifact_store = self.artifact_store.clone();
            let sensitivity = self.sensitivity;
            let root = self.root.clone();
            let expected_id = self.tool_id.clone();
            Box::pin(async move {
                if call.tool_id != expected_id || call.call.tool_name() != TOOL_NAME {
                    return Err(invalid_error("shell call identity is invalid"));
                }
                verify_authority(&ctx)?;
                let arguments: ExecArguments =
                    serde_json::from_slice(call.call.arguments().as_bytes())
                        .map_err(|_| invalid_error("shell arguments are invalid"))?;
                let request = authorize_command(&policy, &arguments, limits, root.as_ref())?;
                let output = sandbox
                    .run(request, ctx.run.cancellation.clone(), ctx.run.deadline)
                    .await?;
                let result =
                    normalize_output(output, &ctx, limits, artifact_store, sensitivity).await?;
                Ok(Box::pin(stream::once(async move {
                    Ok(ToolStreamItem::Completed(result))
                })) as ToolEventStream)
            })
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecArguments {
    argv: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
}

fn authorize_command(
    policy: &ShellPolicy,
    arguments: &ExecArguments,
    limits: ShellLimits,
    root: Option<&unix::Root>,
) -> Result<SandboxedCommand, ToolError> {
    if arguments.argv.is_empty()
        || arguments.argv.len() > MAX_ARGV
        || arguments.argv.iter().any(|value| {
            value.is_empty() || value.len() > MAX_ARG_BYTES || value.as_bytes().contains(&0)
        })
    {
        return Err(invalid_error("shell argv is invalid"));
    }
    let program = policy.authorize(&arguments.argv[0])?;
    let cwd = match arguments.cwd.as_deref() {
        None => None,
        Some(path) => {
            let root = root.ok_or_else(|| policy_error("shell cwd requires an authorized root"))?;
            Some(root.authorize_cwd(path)?)
        }
    };
    Ok(SandboxedCommand {
        program,
        argv: arguments
            .argv
            .iter()
            .map(|value| Arc::<str>::from(value.as_str()))
            .collect::<Vec<_>>()
            .into(),
        env: policy.environment(),
        cwd,
        timeout: limits.timeout,
        max_output_bytes: limits.max_output_bytes,
    })
}

fn run_process(
    request: &SandboxedCommand,
    cancellation: &finstack_ai_runtime::CancellationSignal,
    deadline: Option<Timestamp>,
) -> Result<SandboxedOutput, ToolError> {
    if cancellation.is_cancelled() || deadline_elapsed(deadline) {
        return Err(timeout_error());
    }
    let mut command = Command::new(&request.program);
    if request.argv.len() > 1 {
        command.args(request.argv[1..].iter().map(AsRef::as_ref));
    }
    command
        .env_clear()
        .envs(&request.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    if let Some(cwd) = &request.cwd {
        apply_authorized_cwd(&mut command, cwd)?;
    }
    let mut child = command.spawn().map_err(|_| {
        tool_error(
            SHELL_IO_ERROR,
            ErrorCategory::Tool,
            "shell process could not be started",
        )
    })?;
    let started = Instant::now();
    loop {
        if cancellation.is_cancelled()
            || deadline_elapsed(deadline)
            || started.elapsed() >= request.timeout
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(timeout_error());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut pipe) = child.stdout.take() {
                    std::io::Read::read_to_end(&mut pipe, &mut stdout).ok();
                }
                if let Some(mut pipe) = child.stderr.take() {
                    std::io::Read::read_to_end(&mut pipe, &mut stderr).ok();
                }
                if stdout.len().saturating_add(stderr.len()) > request.max_output_bytes {
                    return Err(tool_error(
                        SHELL_LIMIT_EXCEEDED,
                        ErrorCategory::Limit,
                        "shell output exceeds the configured byte limit",
                    ));
                }
                return Ok(SandboxedOutput {
                    stdout,
                    stderr,
                    exit_code: status.code().unwrap_or(1),
                });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                return Err(tool_error(
                    SHELL_IO_ERROR,
                    ErrorCategory::Tool,
                    "shell process status is unavailable",
                ));
            }
        }
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn apply_authorized_cwd(command: &mut Command, cwd: &Path) -> Result<(), ToolError> {
    use std::os::unix::process::CommandExt;

    let fd = unix::open_no_follow_dir(cwd)?;
    // SAFETY: `pre_exec` runs only in the forked child before `exec`. `fchdir`
    // changes that child's working directory using an already-authorized
    // directory file descriptor. No parent memory is mutated.
    unsafe {
        command.pre_exec(move || {
            rustix::process::fchdir(&fd).map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    Ok(())
}

fn deadline_elapsed(deadline: Option<Timestamp>) -> bool {
    let Some(deadline) = deadline else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        });
    now >= deadline.as_unix_ms()
}

async fn normalize_output(
    output: SandboxedOutput,
    ctx: &ToolCallContext,
    limits: ShellLimits,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    sensitivity: Sensitivity,
) -> Result<ToolResult, ToolError> {
    let json = serde_json::to_vec(&serde_json::json!({
        "exit_code": output.exit_code,
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    }))
    .map_err(|_| {
        tool_error(
            SHELL_IO_ERROR,
            ErrorCategory::Internal,
            "shell result serialization failed",
        )
    })?;
    if json.len() <= limits.inline_result_bytes {
        return Ok(ToolResult {
            output: RawJson::parse(json).map_err(|_| {
                tool_error(
                    SHELL_IO_ERROR,
                    ErrorCategory::Internal,
                    "shell result normalization failed",
                )
            })?,
            is_error: output.exit_code != 0,
        });
    }
    let store = artifact_store.ok_or_else(|| {
        tool_error(
            SHELL_ARTIFACT_REQUIRED,
            ErrorCategory::Limit,
            "shell result requires an artifact store",
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
        Bytes::from(json),
        ArtifactMetadata {
            kind: Arc::from("tool-output"),
            media_type: Arc::from("application/json"),
            name: Some(Arc::from("shell-output")),
            attributes: Metadata::empty(),
        },
    )
    .await
    .map_err(|_| {
        tool_error(
            SHELL_ARTIFACT_REQUIRED,
            ErrorCategory::Tool,
            "shell artifact staging failed",
        )
    })?;
    let reference =
        serde_json::to_vec(&serde_json::json!({ "artifact": artifact })).map_err(|_| {
            tool_error(
                SHELL_IO_ERROR,
                ErrorCategory::Internal,
                "artifact reference serialization failed",
            )
        })?;
    Ok(ToolResult {
        output: RawJson::parse(reference).map_err(|_| {
            tool_error(
                SHELL_IO_ERROR,
                ErrorCategory::Internal,
                "artifact reference normalization failed",
            )
        })?,
        is_error: output.exit_code != 0,
    })
}

fn verify_authority(ctx: &ToolCallContext) -> Result<(), ToolError> {
    if ctx
        .run
        .authorization
        .principal
        .tenant_scope()
        .is_some_and(|scope| scope != ctx.run.locator.tenant_scope.as_ref())
    {
        return Err(policy_error(
            "shell principal scope does not match the committed effect",
        ));
    }
    Ok(())
}

fn build_tools() -> Result<(Arc<[ToolSpec]>, ToolId), ShellError> {
    let tool_id = ToolId::parse(TOOL_ID).map_err(|_| ShellError::Configuration {
        reason: "invalid_tool_id",
    })?;
    let spec = ToolSpec {
        id: tool_id.clone(),
        model_name: Arc::from(TOOL_NAME),
        title: Arc::from("Shell exec"),
        description: Arc::from(
            "Execute one allowlisted argv vector under an empty host environment.",
        ),
        input_schema: RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"argv":{"items":{"type":"string"},"maxItems":64,"minItems":1,"type":"array"},"cwd":{"type":"string"}},"required":["argv"],"type":"object"}"#,
        )
        .map_err(|_| ShellError::Configuration {
            reason: "invalid_input_schema",
        })?,
        output_schema: None,
        execution: ToolExecutionMode::Sequential,
        side_effect: SideEffectClass::NonIdempotentWrite,
        retry_safety: finstack_ai_runtime::RetrySafety::AtMostOnce,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::Policy,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 65_536,
        metadata: Metadata::empty(),
    };
    spec.validate().map_err(|_| ShellError::Configuration {
        reason: "invalid_tool_spec",
    })?;
    Ok((Arc::from([spec]), tool_id))
}

fn invalid_error(message: &'static str) -> ToolError {
    tool_error(SHELL_INVALID_ARGUMENTS, ErrorCategory::Validation, message)
}

fn policy_error(message: &'static str) -> ToolError {
    tool_error(SHELL_POLICY_DENIED, ErrorCategory::Tool, message)
}

fn timeout_error() -> ToolError {
    tool_error(
        SHELL_TIMEOUT,
        ErrorCategory::Deadline,
        "shell execution exceeded its deadline or was cancelled",
    )
}

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty())
        .unwrap_or_else(|error| error)
}

#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use rustix::fd::OwnedFd;
    use rustix::fs::{Mode, OFlags, open, openat};

    use super::{ShellError, ToolError, invalid_error, policy_error};

    const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);

    #[derive(Clone)]
    pub(super) struct Root {
        fd: Arc<OwnedFd>,
        path: PathBuf,
    }

    impl Root {
        pub(super) fn open(path: &Path) -> Result<Self, ShellError> {
            let fd = open(path, DIRECTORY_FLAGS, Mode::empty())
                .map_err(|_| ShellError::RootUnavailable)?;
            Ok(Self {
                fd: Arc::new(fd),
                path: path.to_path_buf(),
            })
        }

        pub(super) fn authorize_cwd(&self, relative: &str) -> Result<PathBuf, ToolError> {
            if relative.is_empty()
                || relative.starts_with('/')
                || relative.contains('\\')
                || relative.contains("//")
                || relative.as_bytes().contains(&0)
                || relative
                    .split('/')
                    .any(|component| component.is_empty() || component == "." || component == "..")
            {
                return Err(invalid_error("shell cwd is invalid"));
            }
            let mut current = rustix::io::dup(&self.fd)
                .map_err(|_| tool_io("shell cwd root could not be duplicated"))?;
            for component in relative.split('/') {
                current = openat(&current, component, DIRECTORY_FLAGS, Mode::empty())
                    .map_err(|_| policy_error("shell cwd is not an authorized directory"))?;
            }
            drop(current);
            Ok(self.path.join(relative))
        }
    }

    pub(super) fn open_no_follow_dir(path: &Path) -> Result<OwnedFd, ToolError> {
        open(path, DIRECTORY_FLAGS, Mode::empty())
            .map_err(|_| policy_error("shell cwd could not be reopened safely"))
    }

    fn tool_io(message: &'static str) -> ToolError {
        super::tool_error(
            super::SHELL_IO_ERROR,
            finstack_ai_runtime::ErrorCategory::Tool,
            message,
        )
    }
}

#[cfg(test)]
mod tests;
