use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;

use finstack_ai_runtime::{CancellationSignal, ErrorCategory, TOOL_CANCELLED, ToolError};
use rustix::fd::OwnedFd;
use rustix::fs::{Dir, FileType, Mode, OFlags, fstat, open, openat};
use serde::Serialize;

use crate::operation::{FileOperation, OperationOutput};
use crate::policy::{ProtectedPaths, ValidatedPath, glob_matches};
use crate::{
    FILESYSTEM_NOT_FOUND, FILESYSTEM_POLICY_DENIED, FileSystemError, FileSystemLimits,
    fs_tool_error, invalid_error, io_error, limit_error, policy_error,
};

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const READ_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC)
    .union(OFlags::NONBLOCK);

#[derive(Clone)]
pub(crate) struct Root {
    fd: Arc<OwnedFd>,
    #[cfg(test)]
    hooks: Arc<std::sync::Mutex<TestHooks>>,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestHooks {
    pub(crate) before_final_open: Option<Arc<dyn Fn() + Send + Sync>>,
    pub(crate) after_final_open: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Root {
    pub(crate) fn open(path: &Path) -> Result<Self, FileSystemError> {
        let fd = open(path, DIRECTORY_FLAGS, Mode::empty())
            .map_err(|_| FileSystemError::RootUnavailable)?;
        Ok(Self {
            fd: Arc::new(fd),
            #[cfg(test)]
            hooks: Arc::new(std::sync::Mutex::new(TestHooks::default())),
        })
    }

    pub(crate) fn execute(
        &self,
        operation: FileOperation,
        limits: FileSystemLimits,
        protected: &ProtectedPaths,
        cancellation: &CancellationSignal,
    ) -> Result<OperationOutput, ToolError> {
        check_cancelled(cancellation)?;
        match operation {
            FileOperation::Read(path) => self.read(&path, limits, cancellation),
            FileOperation::Write { path, content } => self.write(&path, &content, cancellation),
            FileOperation::Edit {
                path,
                old,
                new,
                replace_all,
            } => self.edit(&path, &old, &new, replace_all, limits, cancellation),
            FileOperation::List(path) => self.list(&path, limits, protected, cancellation),
            FileOperation::Glob { pattern } => self.glob(&pattern, limits, protected, cancellation),
            FileOperation::Search { path, query, glob } => self.search(
                &path,
                &query,
                glob.as_deref(),
                limits,
                protected,
                cancellation,
            ),
        }
    }

    fn read(
        &self,
        path: &ValidatedPath,
        limits: FileSystemLimits,
        cancellation: &CancellationSignal,
    ) -> Result<OperationOutput, ToolError> {
        let fd = self.open_leaf(path, READ_FLAGS, Mode::empty())?;
        self.after_final_open();
        ensure_regular(&fd)?;
        let bytes = read_bounded(fd, limits.file_bytes, cancellation)?;
        let content =
            String::from_utf8(bytes).map_err(|_| io_error("filesystem file is not valid UTF-8"))?;
        OperationOutput::try_new(
            &ReadOutput {
                path: path.normalized(),
                bytes: content.len(),
                content: &content,
            },
            "filesystem-read.json",
        )
    }

    fn write(
        &self,
        path: &ValidatedPath,
        content: &[u8],
        cancellation: &CancellationSignal,
    ) -> Result<OperationOutput, ToolError> {
        check_cancelled(cancellation)?;
        let flags = OFlags::WRONLY
            | OFlags::CREATE
            | OFlags::TRUNC
            | OFlags::NOFOLLOW
            | OFlags::CLOEXEC
            | OFlags::NONBLOCK;
        let fd = self.open_leaf(path, flags, Mode::RUSR | Mode::WUSR)?;
        self.after_final_open();
        ensure_regular(&fd)?;
        check_cancelled(cancellation)?;
        let mut file = File::from(fd);
        file.write_all(content)
            .map_err(|_| io_error("filesystem write failed"))?;
        OperationOutput::try_new(
            &WriteOutput {
                path: path.normalized(),
                bytes_written: content.len(),
            },
            "filesystem-write.json",
        )
    }

    fn edit(
        &self,
        path: &ValidatedPath,
        old: &str,
        new: &str,
        replace_all: bool,
        limits: FileSystemLimits,
        cancellation: &CancellationSignal,
    ) -> Result<OperationOutput, ToolError> {
        let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        let fd = self.open_leaf(path, flags, Mode::empty())?;
        self.after_final_open();
        ensure_regular(&fd)?;
        let bytes = read_bounded_fd(&fd, limits.file_bytes, cancellation)?;
        let content =
            String::from_utf8(bytes).map_err(|_| io_error("filesystem file is not valid UTF-8"))?;
        let replacements = content.matches(old).count();
        if replacements == 0 {
            return Err(invalid_error("filesystem edit target was not found"));
        }
        let updated = if replace_all {
            content.replace(old, new)
        } else {
            content.replacen(old, new, 1)
        };
        if updated.len() > limits.file_bytes {
            return Err(limit_error(
                "filesystem edit exceeds the configured byte limit",
            ));
        }
        check_cancelled(cancellation)?;
        let mut file = File::from(fd);
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.set_len(0))
            .and_then(|()| file.write_all(updated.as_bytes()))
            .map_err(|_| io_error("filesystem edit write failed"))?;
        OperationOutput::try_new(
            &EditOutput {
                path: path.normalized(),
                replacements: if replace_all { replacements } else { 1 },
                bytes_written: updated.len(),
            },
            "filesystem-edit.json",
        )
    }

    fn list(
        &self,
        path: &ValidatedPath,
        limits: FileSystemLimits,
        protected: &ProtectedPaths,
        cancellation: &CancellationSignal,
    ) -> Result<OperationOutput, ToolError> {
        let fd = self.open_directory(path)?;
        let mut entries = Vec::new();
        let mut directory =
            Dir::read_from(&fd).map_err(|_| io_error("filesystem directory could not be read"))?;
        for entry in &mut directory {
            check_cancelled(cancellation)?;
            let entry = entry.map_err(|_| io_error("filesystem directory entry failed"))?;
            let name = entry_name(&entry)?;
            if name == "." || name == ".." {
                continue;
            }
            let relative = join_relative(path.normalized(), name);
            if protected.denies(&relative) {
                continue;
            }
            if entries.len() >= limits.visited_entries {
                return Err(limit_error(
                    "filesystem list exceeds the configured entry limit",
                ));
            }
            let opened = match openat(&fd, name, READ_FLAGS, Mode::empty()) {
                Ok(opened) => opened,
                Err(error) if skippable_child_open(error) => continue,
                Err(error) => return Err(map_open_error(error)),
            };
            let kind = opened_kind(&opened)?;
            entries.push(ListEntry {
                name: name.to_owned(),
                kind,
            });
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        OperationOutput::try_new(
            &ListOutput {
                path: path.normalized(),
                entries,
            },
            "filesystem-list.json",
        )
    }

    fn glob(
        &self,
        pattern: &str,
        limits: FileSystemLimits,
        protected: &ProtectedPaths,
        cancellation: &CancellationSignal,
    ) -> Result<OperationOutput, ToolError> {
        let base = ValidatedPath::try_directory("", protected)?;
        let entries = self.walk(&base, limits, protected, cancellation)?;
        let paths = entries
            .into_iter()
            .filter(|entry| glob_matches(pattern, &entry.path))
            .map(|entry| entry.path)
            .collect::<Vec<_>>();
        OperationOutput::try_new(&GlobOutput { pattern, paths }, "filesystem-glob.json")
    }

    fn search(
        &self,
        base: &ValidatedPath,
        query: &str,
        glob: Option<&str>,
        limits: FileSystemLimits,
        protected: &ProtectedPaths,
        cancellation: &CancellationSignal,
    ) -> Result<OperationOutput, ToolError> {
        let entries = self.walk(base, limits, protected, cancellation)?;
        let mut matches = Vec::new();
        for entry in entries {
            check_cancelled(cancellation)?;
            if entry.kind != EntryKind::File
                || glob.is_some_and(|pattern| !glob_matches(pattern, &entry.path))
            {
                continue;
            }
            let path = ValidatedPath::try_file(&entry.path, protected)?;
            let Ok(fd) = self.open_leaf(&path, READ_FLAGS, Mode::empty()) else {
                continue;
            };
            if ensure_regular(&fd).is_err() {
                continue;
            }
            let bytes = read_bounded(fd, limits.file_bytes, cancellation)?;
            let Ok(content) = String::from_utf8(bytes) else {
                continue;
            };
            for (line_index, line) in content.lines().enumerate() {
                if !line.contains(query) {
                    continue;
                }
                if matches.len() >= limits.search_matches {
                    return Err(limit_error(
                        "filesystem search exceeds the configured match limit",
                    ));
                }
                matches.push(SearchMatch {
                    path: entry.path.clone(),
                    line: line_index + 1,
                    preview: bounded_preview(line),
                });
            }
        }
        OperationOutput::try_new(&SearchOutput { query, matches }, "filesystem-search.json")
    }

    fn walk(
        &self,
        base: &ValidatedPath,
        limits: FileSystemLimits,
        protected: &ProtectedPaths,
        cancellation: &CancellationSignal,
    ) -> Result<Vec<TreeEntry>, ToolError> {
        let base_fd = self.open_directory(base)?;
        let mut stack = vec![(base_fd, base.normalized().to_owned(), 0_usize)];
        let mut output = Vec::new();
        while let Some((fd, relative, depth)) = stack.pop() {
            check_cancelled(cancellation)?;
            let mut directory = Dir::read_from(&fd)
                .map_err(|_| io_error("filesystem directory could not be read"))?;
            let mut children = Vec::new();
            for entry in &mut directory {
                let entry = entry.map_err(|_| io_error("filesystem directory entry failed"))?;
                let name = entry_name(&entry)?;
                if name == "." || name == ".." {
                    continue;
                }
                let child_path = join_relative(&relative, name);
                if protected.denies(&child_path) {
                    continue;
                }
                if output.len() >= limits.visited_entries {
                    return Err(limit_error(
                        "filesystem walk exceeds the configured entry limit",
                    ));
                }
                let opened = match openat(&fd, name, READ_FLAGS, Mode::empty()) {
                    Ok(opened) => opened,
                    Err(error) if skippable_child_open(error) => continue,
                    Err(_) => return Err(io_error("filesystem entry open failed")),
                };
                let kind = opened_kind(&opened)?;
                output.push(TreeEntry {
                    path: child_path.clone(),
                    kind,
                });
                if kind == EntryKind::Directory {
                    if depth >= limits.recursion_depth {
                        return Err(limit_error("filesystem walk exceeds the configured depth"));
                    }
                    children.push((opened, child_path, depth + 1));
                }
            }
            children.sort_by(|left, right| right.1.cmp(&left.1));
            stack.extend(children);
        }
        output.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(output)
    }

    fn open_directory(&self, path: &ValidatedPath) -> Result<OwnedFd, ToolError> {
        let mut current = openat(self.fd.as_ref(), ".", DIRECTORY_FLAGS, Mode::empty())
            .map_err(map_open_error)?;
        for component in path.components() {
            current = openat(&current, component.as_ref(), DIRECTORY_FLAGS, Mode::empty())
                .map_err(map_open_error)?;
        }
        Ok(current)
    }

    fn open_leaf(
        &self,
        path: &ValidatedPath,
        flags: OFlags,
        mode: Mode,
    ) -> Result<OwnedFd, ToolError> {
        let Some((name, parents)) = path.components().split_last() else {
            return Err(invalid_error("filesystem file path is empty"));
        };
        let parent = ValidatedPath::from_components_for_open(parents);
        let directory = self.open_directory(&parent)?;
        self.before_final_open();
        openat(&directory, name.as_ref(), flags, mode).map_err(map_open_error)
    }

    #[cfg(test)]
    pub(crate) fn set_test_hooks(&self, hooks: TestHooks) {
        if let Ok(mut current) = self.hooks.lock() {
            *current = hooks;
        }
    }

    fn before_final_open(&self) {
        let _ = &self.fd;
        #[cfg(test)]
        if let Ok(hooks) = self.hooks.lock()
            && let Some(hook) = &hooks.before_final_open
        {
            hook();
        }
    }

    fn after_final_open(&self) {
        let _ = &self.fd;
        #[cfg(test)]
        if let Ok(hooks) = self.hooks.lock()
            && let Some(hook) = &hooks.after_final_open
        {
            hook();
        }
    }
}

impl ValidatedPath {
    fn from_components_for_open(components: &[Arc<str>]) -> Self {
        let normalized = components
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<&str>>()
            .join("/");
        Self::from_validated_parts(Arc::from(components.to_vec()), Arc::from(normalized))
    }
}

fn skippable_child_open(error: rustix::io::Errno) -> bool {
    error == rustix::io::Errno::LOOP
        || error == rustix::io::Errno::ACCESS
        || error == rustix::io::Errno::PERM
}

fn map_open_error(error: rustix::io::Errno) -> ToolError {
    if error == rustix::io::Errno::NOENT {
        fs_tool_error(
            FILESYSTEM_NOT_FOUND,
            ErrorCategory::Tool,
            "filesystem object was not found",
        )
    } else if error == rustix::io::Errno::LOOP || error == rustix::io::Errno::NOTDIR {
        fs_tool_error(
            FILESYSTEM_POLICY_DENIED,
            ErrorCategory::Tool,
            "filesystem symbolic links are not authorized",
        )
    } else {
        io_error("filesystem object could not be opened safely")
    }
}

fn ensure_regular(fd: &OwnedFd) -> Result<(), ToolError> {
    if FileType::from_raw_mode(
        fstat(fd)
            .map_err(|_| io_error("filesystem object metadata failed"))?
            .st_mode,
    )
    .is_file()
    {
        Ok(())
    } else {
        Err(policy_error("filesystem object is not a regular file"))
    }
}

fn opened_kind(fd: &OwnedFd) -> Result<EntryKind, ToolError> {
    let file_type = FileType::from_raw_mode(
        fstat(fd)
            .map_err(|_| io_error("filesystem object metadata failed"))?
            .st_mode,
    );
    Ok(if file_type.is_file() {
        EntryKind::File
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else {
        EntryKind::Other
    })
}

fn read_bounded(
    fd: OwnedFd,
    limit: usize,
    cancellation: &CancellationSignal,
) -> Result<Vec<u8>, ToolError> {
    let mut file = File::from(fd);
    read_bounded_file(&mut file, limit, cancellation)
}

fn read_bounded_fd(
    fd: &OwnedFd,
    limit: usize,
    cancellation: &CancellationSignal,
) -> Result<Vec<u8>, ToolError> {
    let duplicate =
        rustix::io::dup(fd).map_err(|_| io_error("filesystem handle duplication failed"))?;
    read_bounded(duplicate, limit, cancellation)
}

fn read_bounded_file(
    file: &mut File,
    limit: usize,
    cancellation: &CancellationSignal,
) -> Result<Vec<u8>, ToolError> {
    check_cancelled(cancellation)?;
    let take_limit = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    file.take(take_limit)
        .read_to_end(&mut bytes)
        .map_err(|_| io_error("filesystem read failed"))?;
    check_cancelled(cancellation)?;
    if bytes.len() > limit {
        Err(limit_error(
            "filesystem file exceeds the configured byte limit",
        ))
    } else {
        Ok(bytes)
    }
}

fn check_cancelled(cancellation: &CancellationSignal) -> Result<(), ToolError> {
    if cancellation.is_cancelled() {
        Err(fs_tool_error(
            TOOL_CANCELLED,
            ErrorCategory::Cancellation,
            "filesystem operation was cancelled",
        ))
    } else {
        Ok(())
    }
}

fn entry_name(entry: &rustix::fs::DirEntry) -> Result<&str, ToolError> {
    std::ffi::OsStr::from_bytes(entry.file_name().to_bytes())
        .to_str()
        .ok_or_else(|| io_error("filesystem entry name is not valid UTF-8"))
}

fn join_relative(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn bounded_preview(line: &str) -> String {
    let mut end = line.len().min(512);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum EntryKind {
    File,
    Directory,
    Other,
}

struct TreeEntry {
    path: String,
    kind: EntryKind,
}

#[derive(Serialize)]
struct ReadOutput<'a> {
    path: &'a str,
    bytes: usize,
    content: &'a str,
}

#[derive(Serialize)]
struct WriteOutput<'a> {
    path: &'a str,
    bytes_written: usize,
}

#[derive(Serialize)]
struct EditOutput<'a> {
    path: &'a str,
    replacements: usize,
    bytes_written: usize,
}

#[derive(Serialize)]
struct ListEntry {
    name: String,
    kind: EntryKind,
}

#[derive(Serialize)]
struct ListOutput<'a> {
    path: &'a str,
    entries: Vec<ListEntry>,
}

#[derive(Serialize)]
struct GlobOutput<'a> {
    pattern: &'a str,
    paths: Vec<String>,
}

#[derive(Serialize)]
struct SearchMatch {
    path: String,
    line: usize,
    preview: String,
}

#[derive(Serialize)]
struct SearchOutput<'a> {
    query: &'a str,
    matches: Vec<SearchMatch>,
}
