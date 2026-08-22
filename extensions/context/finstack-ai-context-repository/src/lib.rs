//! Allowlisted repository-instruction `ContextProvider`.

#![warn(missing_docs)]
#![cfg_attr(
    not(unix),
    allow(
        dead_code,
        reason = "the public constructor fails closed when no capability-safe backend exists"
    )
)]
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

use std::path::Path;
use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ContentBlock, Digest, InvocationRecovery, Metadata,
    Sensitivity, TextBlock, Version,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::context::{
    ContextAuthority, ContextCallContext, ContextContribution, ContextError, ContextItem,
    ContextItemKind, ContextOverflowPolicy, ContextProvenance, ContextProvider,
    ContextProviderDescriptor, ContextRequest,
};
use thiserror::Error;

/// Stable unsupported-platform code.
pub const REPOSITORY_UNSUPPORTED: &str = "repository_unsupported";
/// Stable unsafe-path error code.
pub const REPOSITORY_PATH_UNSAFE: &str = "repository_path_unsafe";
/// Stable file-read error code.
pub const REPOSITORY_READ_FAILED: &str = "repository_read_failed";
/// Stable oversized-file error code.
pub const REPOSITORY_FILE_TOO_LARGE: &str = "repository_file_too_large";
/// Stable invalid-text error code.
pub const REPOSITORY_TEXT_INVALID: &str = "repository_text_invalid";

const DEFAULT_FILES: [&str; 3] = ["AGENTS.md", "README.md", ".finstack/instructions.md"];
const MAX_FILE_BYTES: usize = 64 * 1024;
const MAX_ALLOWLIST_FILES: usize = 32;
const MAX_ALLOWLIST_NAME_BYTES: usize = 256;
const MAX_ALLOWLIST_DEPTH: usize = 16;

/// Repository provider construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RepositoryError {
    /// The target lacks capability-safe directory primitives.
    #[error("{REPOSITORY_UNSUPPORTED}: safe repository primitives are unavailable")]
    Unsupported,
    /// Configuration is malformed.
    #[error("repository_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// The authorized root could not be opened safely.
    #[error("repository_root_unavailable: authorized root could not be opened safely")]
    RootUnavailable,
}

/// Trusted native repository-instruction provider.
pub struct RepositoryContextProvider {
    descriptor: ContextProviderDescriptor,
    allowlist: Arc<[Arc<str>]>,
    #[cfg(unix)]
    root: unix::Root,
    #[cfg(not(unix))]
    root: (),
}

impl RepositoryContextProvider {
    /// Open an explicit root and the default instruction-file allowlist.
    ///
    /// # Errors
    ///
    /// Fails closed when the root cannot be opened without following a symlink
    /// or a checked-in identity is invalid.
    #[cfg(unix)]
    pub fn try_new(root: impl AsRef<Path>) -> Result<Self, RepositoryError> {
        Self::try_with_allowlist(root, DEFAULT_FILES)
    }

    /// Reject construction without capability-safe primitives.
    ///
    /// # Errors
    ///
    /// Always returns [`RepositoryError::Unsupported`].
    #[cfg(not(unix))]
    pub fn try_new(_root: impl AsRef<Path>) -> Result<Self, RepositoryError> {
        Err(RepositoryError::Unsupported)
    }

    /// Open an explicit root with a caller-supplied filename allowlist.
    ///
    /// # Errors
    ///
    /// Rejects empty, absolute, or traversing names and unsafe roots.
    #[cfg(unix)]
    pub fn try_with_allowlist<I, S>(
        root: impl AsRef<Path>,
        allowlist: I,
    ) -> Result<Self, RepositoryError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let allowlist = allowlist
            .into_iter()
            .map(|value| Arc::<str>::from(value.as_ref()))
            .collect::<Vec<_>>();
        if allowlist.is_empty()
            || allowlist.len() > MAX_ALLOWLIST_FILES
            || allowlist.iter().any(|value| {
                value.is_empty()
                    || value.len() > MAX_ALLOWLIST_NAME_BYTES
                    || value.starts_with('/')
                    || value.contains('\\')
                    || value.as_bytes().contains(&0)
                    || value.split('/').count() > MAX_ALLOWLIST_DEPTH
                    || value.split('/').any(|component| {
                        component.is_empty() || component == "." || component == ".."
                    })
            })
            || allowlist
                .iter()
                .enumerate()
                .any(|(index, value)| allowlist[..index].contains(value))
        {
            return Err(RepositoryError::Configuration {
                reason: "invalid_repository_allowlist",
            });
        }
        let root = unix::Root::open(root.as_ref())?;
        let configuration = configuration_digest(&root, &allowlist)?;
        Ok(Self {
            descriptor: ContextProviderDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse("finstack.context.repository").map_err(|_| {
                        RepositoryError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    version: Version {
                        major: 0,
                        minor: 0,
                        patch: 4,
                    },
                    configuration_digest: configuration,
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                trusted_application_instructions: false,
                metadata: Metadata::empty(),
            },
            allowlist: allowlist.into(),
            root,
        })
    }
}

#[cfg(unix)]
fn configuration_digest(
    root: &unix::Root,
    allowlist: &[Arc<str>],
) -> Result<Digest, RepositoryError> {
    let mut bytes = root.identity()?;
    for name in allowlist {
        let length = u64::try_from(name.len()).map_err(|_| RepositoryError::Configuration {
            reason: "invalid_repository_allowlist",
        })?;
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(name.as_bytes());
    }
    Digest::domain_separated("repository-provider", 1, &bytes).map_err(|_| {
        RepositoryError::Configuration {
            reason: "invalid_repository_configuration",
        }
    })
}

impl ContextProvider for RepositoryContextProvider {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        _ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        #[cfg(not(unix))]
        {
            let _ = request;
            return Box::pin(async {
                Err(ContextError::try_new(
                    REPOSITORY_UNSUPPORTED,
                    finstack_ai_kernel::ErrorCategory::Configuration,
                    "safe repository primitives are unavailable",
                    Metadata::empty(),
                )
                .unwrap_or_else(Into::into))
            });
        }

        #[cfg(unix)]
        {
            let allowlist = Arc::clone(&self.allowlist);
            let root = self.root.clone();
            Box::pin(async move { collect_files(&root, &allowlist, &request) })
        }
    }
}

#[cfg(unix)]
fn collect_files(
    root: &unix::Root,
    allowlist: &[Arc<str>],
    request: &ContextRequest,
) -> Result<ContextContribution, ContextError> {
    let mut items = Vec::new();
    for (index, name) in allowlist.iter().enumerate() {
        let text = match root.read_file(name) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(error) => return Err(repository_read_error(error)),
        };
        let content_digest = Digest::raw_json(text.as_bytes());
        let estimated = estimate_tokens(&text);
        let item = ContextItem::try_new(
            ContextItemKind::QuotedSource,
            vec![ContentBlock::Text(
                TextBlock::try_new(text).map_err(|_| contribution_invalid())?,
            )],
            ContextProvenance {
                source_id: Arc::from("finstack.context.repository"),
                source_ref: Some(Arc::clone(name)),
                external: true,
            },
            ContextAuthority::Untrusted,
            i32::try_from(allowlist.len().saturating_sub(index)).unwrap_or(0),
            estimated,
            Sensitivity::Internal,
            false,
        )?;
        items.push(RepositoryItem {
            name: Arc::clone(name),
            content_digest,
            item,
        });
    }
    apply_budget(items, request)
}

#[cfg(unix)]
struct RepositoryItem {
    name: Arc<str>,
    content_digest: Digest,
    item: ContextItem,
}

#[cfg(unix)]
fn apply_budget(
    items: Vec<RepositoryItem>,
    request: &ContextRequest,
) -> Result<ContextContribution, ContextError> {
    let mut accepted = Vec::new();
    let mut cache_material = Vec::new();
    let mut tokens = 0_u64;
    let mut bytes = 0_u64;
    for entry in items {
        let next_tokens = tokens.saturating_add(entry.item.estimated_tokens);
        let next_bytes = bytes.saturating_add(entry.item.bytes);
        let exceeds = accepted.len() >= request.budget.max_items
            || next_tokens > request.budget.max_tokens
            || next_bytes > request.budget.max_bytes;
        if exceeds {
            return match request.budget.overflow {
                ContextOverflowPolicy::Reject => Err(ContextError::try_new(
                    finstack_ai_runtime::ports::context::CONTEXT_BUDGET_EXCEEDED,
                    finstack_ai_kernel::ErrorCategory::Limit,
                    "repository contribution exceeds the committed budget",
                    Metadata::empty(),
                )
                .unwrap_or_else(Into::into)),
                ContextOverflowPolicy::TruncateWithDiagnostic => {
                    break;
                }
            };
        }
        tokens = next_tokens;
        bytes = next_bytes;
        cache_material.extend_from_slice(
            &u64::try_from(entry.name.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        cache_material.extend_from_slice(entry.name.as_bytes());
        cache_material.extend_from_slice(entry.content_digest.as_bytes());
        accepted.push(entry.item);
    }
    let cache_key = Digest::domain_separated("repository-content", 1, &cache_material)
        .map_err(|_| contribution_invalid())?
        .to_hex();
    ContextContribution::try_new(accepted, Some(cache_key))
}

fn estimate_tokens(text: &str) -> u64 {
    u64::try_from(text.len().div_ceil(4)).unwrap_or(1).max(1)
}

fn contribution_invalid() -> ContextError {
    ContextError::try_new(
        finstack_ai_runtime::ports::context::CONTEXT_CONTRIBUTION_INVALID,
        finstack_ai_kernel::ErrorCategory::Validation,
        "repository file text is invalid",
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

#[cfg(unix)]
fn repository_read_error(error: unix::ReadError) -> ContextError {
    let (code, category, message) = match error {
        unix::ReadError::UnsafePath => (
            REPOSITORY_PATH_UNSAFE,
            finstack_ai_kernel::ErrorCategory::Context,
            "repository path is not a safe regular file",
        ),
        unix::ReadError::TooLarge => (
            REPOSITORY_FILE_TOO_LARGE,
            finstack_ai_kernel::ErrorCategory::Limit,
            "repository file exceeds the size limit",
        ),
        unix::ReadError::InvalidText => (
            REPOSITORY_TEXT_INVALID,
            finstack_ai_kernel::ErrorCategory::Validation,
            "repository file is not valid text",
        ),
        unix::ReadError::ReadFailed => (
            REPOSITORY_READ_FAILED,
            finstack_ai_kernel::ErrorCategory::Context,
            "repository file could not be read",
        ),
    };
    ContextError::try_new(code, category, message, Metadata::empty()).unwrap_or_else(Into::into)
}

#[cfg(unix)]
mod unix {
    use std::path::Path;
    use std::sync::Arc;

    use rustix::fd::OwnedFd;
    use rustix::fs::{Mode, OFlags, fstat, open, openat};

    use super::{MAX_FILE_BYTES, RepositoryError};

    const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);
    const FILE_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);

    #[derive(Clone)]
    pub(super) struct Root {
        fd: Arc<OwnedFd>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum ReadError {
        UnsafePath,
        TooLarge,
        InvalidText,
        ReadFailed,
    }

    impl Root {
        pub(super) fn open(path: &Path) -> Result<Self, RepositoryError> {
            let fd = open(path, DIRECTORY_FLAGS, Mode::empty())
                .map_err(|_| RepositoryError::RootUnavailable)?;
            Ok(Self { fd: Arc::new(fd) })
        }

        pub(super) fn identity(&self) -> Result<Vec<u8>, RepositoryError> {
            let stat = fstat(&self.fd).map_err(|_| RepositoryError::RootUnavailable)?;
            let mut identity = stat.st_dev.to_be_bytes().to_vec();
            identity.extend_from_slice(&stat.st_ino.to_be_bytes());
            Ok(identity)
        }

        pub(super) fn read_file(&self, relative: &str) -> Result<Option<String>, ReadError> {
            let mut current = rustix::io::dup(&self.fd).map_err(|_| ReadError::ReadFailed)?;
            let mut components = relative.split('/').peekable();
            while let Some(component) = components.next() {
                let last = components.peek().is_none();
                let flags = if last { FILE_FLAGS } else { DIRECTORY_FLAGS };
                current = match openat(&current, component, flags, Mode::empty()) {
                    Ok(fd) => fd,
                    Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
                    Err(error)
                        if error == rustix::io::Errno::LOOP
                            || error == rustix::io::Errno::NOTDIR =>
                    {
                        return Err(ReadError::UnsafePath);
                    }
                    Err(_) => return Err(ReadError::ReadFailed),
                };
                if last {
                    let mut bytes = vec![0_u8; MAX_FILE_BYTES.saturating_add(1)];
                    let mut read = 0_usize;
                    loop {
                        match rustix::io::read(&current, &mut bytes[read..]) {
                            Ok(0) => break,
                            Ok(count) => {
                                read = read.saturating_add(count);
                                if read > MAX_FILE_BYTES {
                                    return Err(ReadError::TooLarge);
                                }
                            }
                            Err(_) => return Err(ReadError::ReadFailed),
                        }
                    }
                    bytes.truncate(read);
                    if bytes.contains(&0) {
                        return Err(ReadError::InvalidText);
                    }
                    return String::from_utf8(bytes)
                        .map(Some)
                        .map_err(|_| ReadError::InvalidText);
                }
            }
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests;
