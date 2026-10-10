use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use finstack_ai_kernel::ErrorCategory;
use finstack_ai_runtime::ports::model::CancellationSignal;
use finstack_ai_runtime::ports::tool::{TOOL_CANCELLED, ToolError};
use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, fstat, open, openat, statat};
use serde::Serialize;

use crate::operation::{FileOperation, FileSystemCeilings, OperationOutput};
use crate::policy::{GlobMatcher, ProtectedPaths, ValidatedPath, literal_prefix_directory};
use crate::{
    FILESYSTEM_LIMIT_EXCEEDED, FILESYSTEM_NOT_FOUND, FILESYSTEM_POLICY_DENIED, FileSystemError,
    FileSystemLimits, fs_tool_error, invalid_error, io_error, limit_error, policy_error,
};

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const READ_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC)
    .union(OFlags::NONBLOCK);
/// Entries inspected between interruption checks in the glob filter loop.
const GLOB_CHECK_STRIDE: usize = 256;

#[derive(Clone, Copy)]
struct SearchControl<'a> {
    protected: &'a ProtectedPaths,
    cancellation: &'a CancellationSignal,
    deadline: Option<Instant>,
}

#[derive(Clone)]
pub(crate) struct Root {
    fd: Arc<OwnedFd>,
    #[cfg(test)]
    hooks: Arc<std::sync::Mutex<TestHooks>>,
    #[cfg(test)]
    walk_gauge: Arc<WalkGauge>,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestHooks {
    pub(crate) before_final_open: Option<Arc<dyn Fn() + Send + Sync>>,
    pub(crate) after_final_open: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Test-only gauge of directory handles a walk holds open at once.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct WalkGauge {
    open: std::sync::atomic::AtomicUsize,
    peak: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl WalkGauge {
    fn opened(&self) {
        use std::sync::atomic::Ordering;
        let open = self.open.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(open, Ordering::SeqCst);
    }

    fn closed(&self) {
        self.open.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Most directory handles any walk on this root held open simultaneously.
    pub(crate) fn peak(&self) -> usize {
        self.peak.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// One directory handle owned by a walk; releases its gauge slot on drop.
struct WalkDir {
    fd: OwnedFd,
    #[cfg(test)]
    gauge: Arc<WalkGauge>,
}

#[cfg(test)]
impl Drop for WalkDir {
    fn drop(&mut self) {
        self.gauge.closed();
    }
}

/// A directory the walk has yet to read: either already open (the base) or a
/// `(parent, name)` pair opened only when popped, so open handles are bounded
/// by depth rather than by fan-out.
enum PendingDir {
    Open(WalkDir),
    Child { parent: Arc<WalkDir>, name: String },
}

struct WalkFrame {
    dir: PendingDir,
    path: String,
    depth: usize,
}

impl Root {
    pub(crate) fn open(path: &Path) -> Result<Self, FileSystemError> {
        let fd = open(path, DIRECTORY_FLAGS, Mode::empty())
            .map_err(|_| FileSystemError::RootUnavailable)?;
        Ok(Self {
            fd: Arc::new(fd),
            #[cfg(test)]
            hooks: Arc::new(std::sync::Mutex::new(TestHooks::default())),
            #[cfg(test)]
            walk_gauge: Arc::new(WalkGauge::default()),
        })
    }

    #[cfg(test)]
    pub(crate) fn walk_gauge(&self) -> &WalkGauge {
        &self.walk_gauge
    }

    fn walk_dir(&self, fd: OwnedFd) -> WalkDir {
        let _ = &self.fd;
        #[cfg(test)]
        self.walk_gauge.opened();
        WalkDir {
            fd,
            #[cfg(test)]
            gauge: Arc::clone(&self.walk_gauge),
        }
    }

    pub(crate) fn execute(
        &self,
        operation: FileOperation,
        ceilings: FileSystemCeilings,
        protected: &ProtectedPaths,
        cancellation: &CancellationSignal,
        deadline: Option<Instant>,
    ) -> Result<OperationOutput, ToolError> {
        check_interrupted(cancellation, deadline)?;
        let output = match operation {
            FileOperation::Read(path) => self.read(&path, ceilings, cancellation),
            FileOperation::Write { path, content } => {
                self.write(&path, &content, ceilings, cancellation)
            }
            FileOperation::Edit {
                path,
                old,
                new,
                replace_all,
            } => self.edit(&path, &old, &new, replace_all, ceilings, cancellation),
            FileOperation::List(path) => self.list(&path, ceilings, protected, cancellation),
            FileOperation::Glob { pattern } => {
                self.glob(&pattern, ceilings, protected, cancellation, deadline)
            }
            FileOperation::Search { path, query, glob } => self.search(
                &path,
                &query,
                glob.as_deref(),
                ceilings,
                SearchControl {
                    protected,
                    cancellation,
                    deadline,
                },
            ),
        }?;
        check_interrupted(cancellation, deadline)?;
        Ok(output)
    }

    fn read(
        &self,
        path: &ValidatedPath,
        ceilings: FileSystemCeilings,
        cancellation: &CancellationSignal,
    ) -> Result<OperationOutput, ToolError> {
        let fd = self.open_leaf(path, READ_FLAGS, Mode::empty())?;
        self.after_final_open();
        ensure_regular(&fd)?;
        let bytes = read_bounded(fd, ceilings.limits.file_bytes, cancellation)?;
        let content =
            String::from_utf8(bytes).map_err(|_| io_error("filesystem file is not valid UTF-8"))?;
        OperationOutput::try_new(
            &ReadOutput {
                path: path.normalized(),
                bytes: content.len(),
                content: &content,
            },
            "filesystem-read.json",
            ceilings.max_artifact_bytes,
        )
    }

    fn write(
        &self,
        path: &ValidatedPath,
        content: &[u8],
        ceilings: FileSystemCeilings,
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
            ceilings.max_artifact_bytes,
        )
    }

    fn edit(
        &self,
        path: &ValidatedPath,
        old: &str,
        new: &str,
        replace_all: bool,
        ceilings: FileSystemCeilings,
        cancellation: &CancellationSignal,
    ) -> Result<OperationOutput, ToolError> {
        let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        let fd = self.open_leaf(path, flags, Mode::empty())?;
        self.after_final_open();
        ensure_regular(&fd)?;
        let bytes = read_bounded_fd(&fd, ceilings.limits.file_bytes, cancellation)?;
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
        if updated.len() > ceilings.limits.file_bytes {
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
            ceilings.max_artifact_bytes,
        )
    }

    fn list(
        &self,
        path: &ValidatedPath,
        ceilings: FileSystemCeilings,
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
            if entries.len() >= ceilings.limits.visited_entries {
                return Err(limit_error(
                    "filesystem list exceeds the configured entry limit",
                ));
            }
            let Some(kind) = classify_child(&fd, name)? else {
                continue;
            };
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
            ceilings.max_artifact_bytes,
        )
    }

    fn glob(
        &self,
        pattern: &str,
        ceilings: FileSystemCeilings,
        protected: &ProtectedPaths,
        cancellation: &CancellationSignal,
        deadline: Option<Instant>,
    ) -> Result<OperationOutput, ToolError> {
        // Only the subtree under the literal prefix can match, so the walk
        // starts there. A prefix that is protected, missing, or not a
        // directory matches nothing, exactly as the full walk would have.
        let prefix = literal_prefix_directory(pattern);
        let mut paths = Vec::new();
        if let Ok(base) = ValidatedPath::try_directory(prefix, protected) {
            let base_fd = match self.open_directory(&base) {
                Ok(fd) => Some(fd),
                Err(error)
                    if error.code() == FILESYSTEM_NOT_FOUND
                        || error.code() == FILESYSTEM_POLICY_DENIED =>
                {
                    None
                }
                Err(error) => return Err(error),
            };
            if let Some(base_fd) = base_fd {
                let entries = self.walk_from(
                    base_fd,
                    &base,
                    ceilings.limits,
                    protected,
                    cancellation,
                    deadline,
                )?;
                let mut matcher = GlobMatcher::new(pattern);
                for (index, entry) in entries.into_iter().enumerate() {
                    if index % GLOB_CHECK_STRIDE == 0 {
                        check_interrupted(cancellation, deadline)?;
                    }
                    if matcher.matches(&entry.path) {
                        paths.push(entry.path);
                    }
                }
            }
        }
        OperationOutput::try_new(
            &GlobOutput { pattern, paths },
            "filesystem-glob.json",
            ceilings.max_artifact_bytes,
        )
    }

    fn search(
        &self,
        base: &ValidatedPath,
        query: &str,
        glob: Option<&str>,
        ceilings: FileSystemCeilings,
        control: SearchControl<'_>,
    ) -> Result<OperationOutput, ToolError> {
        let entries = self.walk(
            base,
            ceilings.limits,
            control.protected,
            control.cancellation,
            control.deadline,
        )?;
        let mut matches = Vec::new();
        let mut scanned_bytes = 0_usize;
        let mut skipped_oversize = 0_usize;
        let mut glob = glob.map(GlobMatcher::new);
        for entry in entries {
            check_interrupted(control.cancellation, control.deadline)?;
            if entry.kind != EntryKind::File
                || glob
                    .as_mut()
                    .is_some_and(|matcher| !matcher.matches(&entry.path))
            {
                continue;
            }
            let path = ValidatedPath::try_file(&entry.path, control.protected)?;
            let Ok(fd) = self.open_leaf(&path, READ_FLAGS, Mode::empty()) else {
                continue;
            };
            if ensure_regular(&fd).is_err() {
                continue;
            }
            let file_size = fstat(&fd)
                .ok()
                .and_then(|stat| usize::try_from(stat.st_size).ok())
                .unwrap_or(usize::MAX);
            // A file over the per-file ceiling is never read, so it neither
            // fails the search nor consumes the aggregate scan budget.
            if file_size > ceilings.limits.file_bytes {
                skipped_oversize = skipped_oversize.saturating_add(1);
                continue;
            }
            let remaining = ceilings.limits.scan_bytes.saturating_sub(scanned_bytes);
            if remaining == 0 || file_size > remaining {
                return Err(limit_error(
                    "filesystem search exceeds the aggregate scan byte limit",
                ));
            }
            let bytes = match read_bounded(fd, ceilings.limits.file_bytes, control.cancellation) {
                Ok(bytes) => bytes,
                // The file grew past the ceiling between `fstat` and the read.
                Err(error) if error.code() == FILESYSTEM_LIMIT_EXCEEDED => {
                    skipped_oversize = skipped_oversize.saturating_add(1);
                    continue;
                }
                Err(error) => return Err(error),
            };
            scanned_bytes = scanned_bytes.saturating_add(bytes.len());
            let Ok(content) = String::from_utf8(bytes) else {
                continue;
            };
            for (line_index, line) in content.lines().enumerate() {
                check_interrupted(control.cancellation, control.deadline)?;
                if !line.contains(query) {
                    continue;
                }
                if matches.len() >= ceilings.limits.search_matches {
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
        OperationOutput::try_new(
            &SearchOutput {
                query,
                matches,
                skipped_oversize,
            },
            "filesystem-search.json",
            ceilings.max_artifact_bytes,
        )
    }

    fn walk(
        &self,
        base: &ValidatedPath,
        limits: FileSystemLimits,
        protected: &ProtectedPaths,
        cancellation: &CancellationSignal,
        deadline: Option<Instant>,
    ) -> Result<Vec<TreeEntry>, ToolError> {
        let base_fd = self.open_directory(base)?;
        self.walk_from(base_fd, base, limits, protected, cancellation, deadline)
    }

    /// Depth-first walk below an already-opened `base_fd`.
    ///
    /// Children are classified with `statat(SYMLINK_NOFOLLOW)` and only
    /// directories are opened, when their frame is popped. Symbolic links are
    /// never followed and never reported; sockets, FIFOs, and devices are
    /// reported as [`EntryKind::Other`]. Entries that vanish or are
    /// unreadable between classification and open are skipped.
    fn walk_from(
        &self,
        base_fd: OwnedFd,
        base: &ValidatedPath,
        limits: FileSystemLimits,
        protected: &ProtectedPaths,
        cancellation: &CancellationSignal,
        deadline: Option<Instant>,
    ) -> Result<Vec<TreeEntry>, ToolError> {
        let mut stack = vec![WalkFrame {
            dir: PendingDir::Open(self.walk_dir(base_fd)),
            path: base.normalized().to_owned(),
            depth: 0,
        }];
        let mut output = Vec::new();
        while let Some(frame) = stack.pop() {
            check_interrupted(cancellation, deadline)?;
            let current = match frame.dir {
                PendingDir::Open(dir) => dir,
                PendingDir::Child { parent, name } => {
                    let opened = openat(&parent.fd, name.as_str(), DIRECTORY_FLAGS, Mode::empty());
                    drop(parent);
                    match opened {
                        Ok(fd) => self.walk_dir(fd),
                        Err(error) if skippable_directory_open(error) => continue,
                        Err(_) => return Err(io_error("filesystem entry open failed")),
                    }
                }
            };
            let mut directory = Dir::read_from(&current.fd)
                .map_err(|_| io_error("filesystem directory could not be read"))?;
            let mut children = Vec::new();
            for entry in &mut directory {
                check_interrupted(cancellation, deadline)?;
                let entry = entry.map_err(|_| io_error("filesystem directory entry failed"))?;
                let name = entry_name(&entry)?;
                if name == "." || name == ".." {
                    continue;
                }
                let child_path = join_relative(&frame.path, name);
                if protected.denies(&child_path) {
                    continue;
                }
                if output.len() >= limits.visited_entries {
                    return Err(limit_error(
                        "filesystem walk exceeds the configured entry limit",
                    ));
                }
                let Some(kind) = classify_child(&current.fd, name)? else {
                    continue;
                };
                output.push(TreeEntry {
                    path: child_path.clone(),
                    kind,
                });
                if kind == EntryKind::Directory {
                    if frame.depth >= limits.recursion_depth {
                        return Err(limit_error("filesystem walk exceeds the configured depth"));
                    }
                    children.push((name.to_owned(), child_path));
                }
            }
            drop(directory);
            if !children.is_empty() {
                children.sort_by(|left, right| right.1.cmp(&left.1));
                let parent = Arc::new(current);
                stack.extend(children.into_iter().map(|(name, path)| WalkFrame {
                    dir: PendingDir::Child {
                        parent: Arc::clone(&parent),
                        name,
                    },
                    path,
                    depth: frame.depth + 1,
                }));
            }
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
        let directory = self.open_directory(&ValidatedPath::from_components(parents))?;
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

/// Classify one directory child without opening or following it.
///
/// Returns `None` for entries that should be skipped: symbolic links (which
/// the toolset never follows or reports) and entries whose metadata is
/// unreadable or that vanished under the walk.
fn classify_child(directory: &OwnedFd, name: &str) -> Result<Option<EntryKind>, ToolError> {
    let stat = match statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(error) if skippable_child_stat(error) => return Ok(None),
        Err(_) => return Err(io_error("filesystem entry metadata failed")),
    };
    let file_type = FileType::from_raw_mode(stat.st_mode);
    Ok(if file_type.is_symlink() {
        None
    } else if file_type.is_file() {
        Some(EntryKind::File)
    } else if file_type.is_dir() {
        Some(EntryKind::Directory)
    } else {
        Some(EntryKind::Other)
    })
}

fn skippable_child_stat(error: rustix::io::Errno) -> bool {
    error == rustix::io::Errno::LOOP
        || error == rustix::io::Errno::ACCESS
        || error == rustix::io::Errno::PERM
        || error == rustix::io::Errno::NOENT
}

/// A child classified as a directory may be unreadable, gone, or replaced by
/// a non-directory before its frame is popped; all of those are skipped.
fn skippable_directory_open(error: rustix::io::Errno) -> bool {
    skippable_child_stat(error) || error == rustix::io::Errno::NOTDIR
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

fn read_bounded(
    fd: OwnedFd,
    limit: usize,
    cancellation: &CancellationSignal,
) -> Result<Vec<u8>, ToolError> {
    check_cancelled(cancellation)?;
    let take_limit = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    File::from(fd)
        .take(take_limit)
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

fn read_bounded_fd(
    fd: &OwnedFd,
    limit: usize,
    cancellation: &CancellationSignal,
) -> Result<Vec<u8>, ToolError> {
    let duplicate =
        rustix::io::dup(fd).map_err(|_| io_error("filesystem handle duplication failed"))?;
    read_bounded(duplicate, limit, cancellation)
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

fn check_interrupted(
    cancellation: &CancellationSignal,
    deadline: Option<Instant>,
) -> Result<(), ToolError> {
    check_cancelled(cancellation)?;
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(fs_tool_error(
            crate::FILESYSTEM_TIMEOUT,
            ErrorCategory::Deadline,
            "filesystem operation timed out",
        ));
    }
    Ok(())
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
    /// Regular files skipped because they exceed the per-file byte ceiling.
    skipped_oversize: usize,
}
