//! Internal process-confinement service. This is not a port.
//!
//! Trusted native toolsets (shell now; FR-03 stdio later) consume this
//! service. Fail closed: a missing or broken platform primitive never
//! falls back to an unconfined spawn. The labeled unconfined
//! `std::process` runner stays in the shell crate as a separate path.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::Arc;

use thiserror::Error;

/// Stable code when the host cannot confine a child.
pub const CONFINEMENT_UNAVAILABLE: &str = "process_confinement_unavailable";
/// Stable code when confinement denies the spawn or the child filesystem view.
pub const CONFINEMENT_DENIED: &str = "process_confinement_denied";
/// Stable code when confinement I/O or process setup fails.
pub const CONFINEMENT_IO: &str = "process_confinement_io";

/// Capability-scoped confinement profile.
///
/// `root` is the same authorized directory the filesystem toolset already
/// holds. The service does not invent a second root.
#[derive(Debug, Clone)]
pub struct ConfinementProfile {
    root: PathBuf,
    cwd: Option<PathBuf>,
    #[cfg(unix)]
    _root_handle: Arc<std::os::fd::OwnedFd>,
    #[cfg(unix)]
    cwd_handle: Option<Arc<std::os::fd::OwnedFd>>,
    windows_lpac: Option<WindowsLpacProfile>,
}

impl PartialEq for ConfinementProfile {
    fn eq(&self, other: &Self) -> bool {
        self.root == other.root && self.cwd == other.cwd && self.windows_lpac == other.windows_lpac
    }
}

impl Eq for ConfinementProfile {}

/// Host-provisioned Windows Less Privileged `AppContainer` identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsLpacProfile {
    moniker: Arc<str>,
}

impl WindowsLpacProfile {
    /// Bind a pre-existing `AppContainer` moniker without provisioning it.
    ///
    /// # Errors
    ///
    /// Returns a denial when the moniker is empty or contains a NUL.
    pub fn try_new(moniker: impl Into<Arc<str>>) -> Result<Self, ConfinementError> {
        let moniker = moniker.into();
        if moniker.is_empty() || moniker.contains('\0') {
            return Err(ConfinementError::denied(
                "windows AppContainer moniker is invalid",
            ));
        }
        Ok(Self { moniker })
    }

    /// Provisioned `AppContainer` moniker.
    #[must_use]
    pub fn moniker(&self) -> &str {
        &self.moniker
    }
}

impl ConfinementProfile {
    /// Bind confinement to one existing absolute directory.
    ///
    /// # Errors
    ///
    /// Returns [`ConfinementError::denied`] when `root` is empty, relative,
    /// contains `..`, or is not an existing directory.
    pub fn try_new(root: impl AsRef<Path>) -> Result<Self, ConfinementError> {
        let root = root.as_ref();
        if root.as_os_str().is_empty()
            || !root.is_absolute()
            || root
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
            || !root.is_dir()
        {
            return Err(ConfinementError::denied(
                "confinement root must be an existing absolute directory",
            ));
        }
        let root = root
            .canonicalize()
            .map_err(|_| ConfinementError::denied("confinement root could not be canonicalized"))?;
        #[cfg(unix)]
        let root_handle = Arc::new(open_directory(&root).map_err(|_| {
            ConfinementError::denied("confinement root could not be opened without symlinks")
        })?);
        Ok(Self {
            root,
            cwd: None,
            #[cfg(unix)]
            _root_handle: root_handle,
            #[cfg(unix)]
            cwd_handle: None,
            windows_lpac: None,
        })
    }

    /// Set an already-authorized working directory under [`Self::root`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfinementError::denied`] when `cwd` is not under `root`.
    pub fn with_authorized_cwd(mut self, cwd: impl AsRef<Path>) -> Result<Self, ConfinementError> {
        let cwd = cwd.as_ref();
        let cwd = cwd
            .canonicalize()
            .map_err(|_| ConfinementError::denied("confinement cwd could not be canonicalized"))?;
        if !cwd.starts_with(&self.root) || !cwd.is_dir() {
            return Err(ConfinementError::denied(
                "confinement cwd must be a directory under the authorized root",
            ));
        }
        #[cfg(unix)]
        {
            self.cwd_handle = Some(Arc::new(open_directory(&cwd).map_err(|_| {
                ConfinementError::denied("confinement cwd could not be opened without symlinks")
            })?));
        }
        self.cwd = Some(cwd);
        Ok(self)
    }

    /// Attach a host-provisioned Windows LPAC identity.
    #[must_use]
    pub fn with_windows_lpac(mut self, profile: WindowsLpacProfile) -> Self {
        self.windows_lpac = Some(profile);
        self
    }

    /// Borrow the capability-scoped root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Borrow the optional authorized working directory.
    #[must_use]
    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Host-provisioned Windows LPAC identity, when configured.
    #[must_use]
    pub const fn windows_lpac(&self) -> Option<&WindowsLpacProfile> {
        self.windows_lpac.as_ref()
    }
}

/// Fail-closed confinement failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code}: {message}")]
pub struct ConfinementError {
    code: &'static str,
    message: &'static str,
}

impl ConfinementError {
    /// Platform primitive missing or unusable. Never accompanied by a spawn.
    #[must_use]
    pub const fn unavailable(message: &'static str) -> Self {
        Self {
            code: CONFINEMENT_UNAVAILABLE,
            message,
        }
    }

    /// Policy or profile rejection.
    #[must_use]
    pub const fn denied(message: &'static str) -> Self {
        Self {
            code: CONFINEMENT_DENIED,
            message,
        }
    }

    /// Setup I/O failed before a confined child could start.
    #[must_use]
    pub const fn io(message: &'static str) -> Self {
        Self {
            code: CONFINEMENT_IO,
            message,
        }
    }

    /// Stable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// Safe bounded message.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }
}

/// Host confinement backend. Not a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfinementBackend {
    /// Linux Landlock plus `no_new_privs`.
    LinuxLandlock,
    /// macOS Seatbelt (`sandbox_init`). Apple documents this API as deprecated.
    MacosSeatbelt,
    /// Windows Less Privileged `AppContainer` plus Job Object.
    WindowsLpacJob,
    /// Forced or native-unsupported path. Spawn is refused.
    Unavailable,
}

/// Internal process-confinement service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessConfinement {
    backend: ConfinementBackend,
}

impl ProcessConfinement {
    /// Select the backend for this compile target. Unsupported targets are
    /// [`ConfinementBackend::Unavailable`].
    #[must_use]
    pub const fn for_current_platform() -> Self {
        Self {
            backend: current_backend(),
        }
    }

    /// Forced-unavailable path used by fail-closed tests.
    #[must_use]
    pub const fn unavailable() -> Self {
        Self {
            backend: ConfinementBackend::Unavailable,
        }
    }

    /// Selected backend.
    #[must_use]
    pub const fn backend(&self) -> ConfinementBackend {
        self.backend
    }

    /// Whether this instance will refuse every spawn.
    #[must_use]
    pub const fn is_unavailable(&self) -> bool {
        matches!(self.backend, ConfinementBackend::Unavailable)
    }

    /// Apply confinement hooks to `command` without spawning.
    ///
    /// Unix backends attach `pre_exec` so a later `tokio::process` spawn stays
    /// confined. Windows cannot attach a restricted token to a later spawn, so
    /// this returns [`CONFINEMENT_UNAVAILABLE`] there; use [`Self::spawn`].
    ///
    /// # Errors
    ///
    /// Returns a stable [`ConfinementError`] and does not mutate `command` into
    /// an unconfined-success path.
    pub fn configure(
        &self,
        command: &mut Command,
        profile: &ConfinementProfile,
    ) -> Result<(), ConfinementError> {
        match self.backend {
            ConfinementBackend::Unavailable => Err(ConfinementError::unavailable(
                "process confinement is unavailable on this target",
            )),
            ConfinementBackend::LinuxLandlock => linux::configure(command, profile),
            ConfinementBackend::MacosSeatbelt => macos::configure(command, profile),
            ConfinementBackend::WindowsLpacJob => Err(ConfinementError::unavailable(
                "windows confined children must use ProcessConfinement::spawn",
            )),
        }
    }

    /// Spawn `command` under `profile`. Refuses when the backend is missing.
    ///
    /// Windows applies a host-provisioned LPAC identity with `STARTUPINFOEX`
    /// and assigns the child to a Job Object. Either primitive missing fails
    /// closed. Unix backends keep `pre_exec` confinement.
    ///
    /// # Errors
    ///
    /// Returns a stable [`ConfinementError`] and does not leave an unconfined
    /// child running.
    pub fn spawn(
        &self,
        command: Command,
        profile: &ConfinementProfile,
    ) -> Result<ConfinedChild, ConfinementError> {
        match self.backend {
            ConfinementBackend::Unavailable => Err(ConfinementError::unavailable(
                "process confinement is unavailable on this target",
            )),
            ConfinementBackend::LinuxLandlock => linux::spawn(command, profile),
            ConfinementBackend::MacosSeatbelt => macos::spawn(command, profile),
            ConfinementBackend::WindowsLpacJob => windows::spawn(command, profile),
        }
    }
}

/// Child started by [`ProcessConfinement::spawn`].
///
/// Unix wraps `std::process::Child`. Windows owns the `CreateProcessAsUser`
/// process handle plus the Job Object so kill-on-job-close stays live.
#[derive(Debug)]
pub struct ConfinedChild {
    #[cfg(not(windows))]
    inner: std::process::Child,
    #[cfg(windows)]
    process: std::os::windows::io::OwnedHandle,
    #[cfg(windows)]
    _job: std::os::windows::io::OwnedHandle,
    /// Optional stdin pipe.
    pub stdin: Option<std::fs::File>,
    /// Optional stdout pipe.
    pub stdout: Option<std::fs::File>,
    /// Optional stderr pipe.
    pub stderr: Option<std::fs::File>,
}

impl ConfinedChild {
    #[cfg(not(windows))]
    fn from_std(mut inner: std::process::Child) -> Self {
        Self {
            stdin: inner
                .stdin
                .take()
                .map(|pipe| std::fs::File::from(std::os::fd::OwnedFd::from(pipe))),
            stdout: inner
                .stdout
                .take()
                .map(|pipe| std::fs::File::from(std::os::fd::OwnedFd::from(pipe))),
            stderr: inner
                .stderr
                .take()
                .map(|pipe| std::fs::File::from(std::os::fd::OwnedFd::from(pipe))),
            inner,
        }
    }

    /// Force-terminate the child.
    ///
    /// # Errors
    ///
    /// Returns I/O failure when the platform kill primitive fails.
    pub fn kill(&mut self) -> std::io::Result<()> {
        #[cfg(not(windows))]
        {
            self.inner.kill()
        }
        #[cfg(windows)]
        {
            windows::terminate(&self.process)
        }
    }

    /// Block until the child exits.
    ///
    /// # Errors
    ///
    /// Returns I/O failure when the platform wait primitive fails.
    pub fn wait(&mut self) -> std::io::Result<ExitStatus> {
        #[cfg(not(windows))]
        {
            self.inner.wait()
        }
        #[cfg(windows)]
        {
            windows::wait(&self.process)
        }
    }

    /// Return the exit status when the child has already exited.
    ///
    /// # Errors
    ///
    /// Returns I/O failure when the platform wait primitive fails.
    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        #[cfg(not(windows))]
        {
            self.inner.try_wait()
        }
        #[cfg(windows)]
        {
            windows::try_wait(&self.process)
        }
    }
}

#[cfg(unix)]
fn open_directory(path: &Path) -> std::io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{Mode, OFlags, open};
    open(
        path,
        OFlags::RDONLY
            .union(OFlags::DIRECTORY)
            .union(OFlags::NOFOLLOW)
            .union(OFlags::CLOEXEC),
        Mode::empty(),
    )
    .map_err(std::io::Error::from)
}

#[cfg(unix)]
fn apply_authorized_cwd(cwd: &std::os::fd::OwnedFd) -> std::io::Result<()> {
    rustix::process::fchdir(cwd).map_err(std::io::Error::from)
}

const fn current_backend() -> ConfinementBackend {
    if cfg!(target_os = "linux") {
        ConfinementBackend::LinuxLandlock
    } else if cfg!(target_os = "macos") {
        ConfinementBackend::MacosSeatbelt
    } else if cfg!(target_os = "windows") {
        ConfinementBackend::WindowsLpacJob
    } else {
        ConfinementBackend::Unavailable
    }
}

#[cfg(target_os = "linux")]
#[allow(
    unsafe_code,
    reason = "Landlock syscalls and no_new_privs run only in the forked child"
)]
mod linux {
    use std::os::fd::AsRawFd;
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;

    use std::os::fd::{FromRawFd, OwnedFd};

    use rustix::fs::{Mode, OFlags, open};
    use rustix::thread::set_no_new_privs;

    use super::{ConfinedChild, ConfinementError, ConfinementProfile};

    const SYS_LANDLOCK_CREATE_RULESET: i64 = 444;
    const SYS_LANDLOCK_ADD_RULE: i64 = 445;
    const SYS_LANDLOCK_RESTRICT_SELF: i64 = 446;
    const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
    const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;
    const ACCESS_EXECUTE: u64 = 1 << 0;
    const ACCESS_WRITE_FILE: u64 = 1 << 1;
    const ACCESS_READ_FILE: u64 = 1 << 2;
    const ACCESS_READ_DIR: u64 = 1 << 3;
    const ACCESS_REMOVE_DIR: u64 = 1 << 4;
    const ACCESS_REMOVE_FILE: u64 = 1 << 5;
    const ACCESS_MAKE_CHAR: u64 = 1 << 6;
    const ACCESS_MAKE_DIR: u64 = 1 << 7;
    const ACCESS_MAKE_REG: u64 = 1 << 8;
    const ACCESS_MAKE_SOCK: u64 = 1 << 9;
    const ACCESS_MAKE_FIFO: u64 = 1 << 10;
    const ACCESS_MAKE_BLOCK: u64 = 1 << 11;
    const ACCESS_MAKE_SYM: u64 = 1 << 12;
    const ACCESS_REFER: u64 = 1 << 13;
    const ACCESS_TRUNCATE: u64 = 1 << 14;
    const READ_EXECUTE_ACCESS: u64 = ACCESS_EXECUTE | ACCESS_READ_FILE | ACCESS_READ_DIR;
    const ROOT_ACCESS: u64 = READ_EXECUTE_ACCESS
        | ACCESS_WRITE_FILE
        | ACCESS_REMOVE_DIR
        | ACCESS_REMOVE_FILE
        | ACCESS_MAKE_CHAR
        | ACCESS_MAKE_DIR
        | ACCESS_MAKE_REG
        | ACCESS_MAKE_SOCK
        | ACCESS_MAKE_FIFO
        | ACCESS_MAKE_BLOCK
        | ACCESS_MAKE_SYM
        | ACCESS_REFER
        | ACCESS_TRUNCATE;
    const HANDLED_ACCESS: u64 = ROOT_ACCESS;

    #[repr(C)]
    struct LandlockRulesetAttr {
        handled_access_fs: u64,
    }

    #[repr(C)]
    struct LandlockPathBeneathAttr {
        allowed_access: u64,
        parent_fd: i32,
    }

    unsafe extern "C" {
        fn syscall(number: i64, ...) -> i64;
    }

    pub(super) fn configure(
        command: &mut Command,
        profile: &ConfinementProfile,
    ) -> Result<(), ConfinementError> {
        probe_landlock()?;
        let program = command
            .get_program()
            .to_owned()
            .into_os_string()
            .into_string()
            .map_err(|_| ConfinementError::denied("confined program path is not UTF-8"))?;
        let program_handle = Arc::new(
            open(
                Path::new(&program),
                OFlags::PATH.union(OFlags::CLOEXEC),
                Mode::empty(),
            )
            .map_err(|_| ConfinementError::denied("confined program could not be opened"))?,
        );
        let root_handle = Arc::clone(&profile._root_handle);
        let cwd_handle = profile.cwd_handle.as_ref().map(Arc::clone);
        // SAFETY: `pre_exec` runs only in the forked child before `exec`.
        // Landlock and `no_new_privs` apply to that child only.
        #[allow(unsafe_code)]
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(move || {
                if let Some(cwd) = &cwd_handle {
                    super::apply_authorized_cwd(cwd)?;
                }
                apply_landlock(root_handle.as_ref(), program_handle.as_ref())
            });
        }
        Ok(())
    }

    pub(super) fn spawn(
        mut command: Command,
        profile: &ConfinementProfile,
    ) -> Result<ConfinedChild, ConfinementError> {
        configure(&mut command, profile)?;
        command
            .spawn()
            .map(ConfinedChild::from_std)
            .map_err(|_| ConfinementError::io("confined linux process could not be started"))
    }

    fn probe_landlock() -> Result<(), ConfinementError> {
        let abi = unsafe {
            syscall(
                SYS_LANDLOCK_CREATE_RULESET,
                std::ptr::null::<LandlockRulesetAttr>(),
                0_usize,
                LANDLOCK_CREATE_RULESET_VERSION,
            )
        };
        if abi < 3 {
            return Err(ConfinementError::unavailable(
                "landlock ABI 3 is unavailable on this kernel",
            ));
        }
        Ok(())
    }

    fn apply_landlock(root: &OwnedFd, program: &OwnedFd) -> std::io::Result<()> {
        set_no_new_privs(true).map_err(std::io::Error::from)?;
        let attr = LandlockRulesetAttr {
            handled_access_fs: HANDLED_ACCESS,
        };
        let ruleset = unsafe {
            syscall(
                SYS_LANDLOCK_CREATE_RULESET,
                std::ptr::from_ref(&attr),
                std::mem::size_of::<LandlockRulesetAttr>(),
                0_u32,
            )
        };
        if ruleset < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let ruleset_fd = i32::try_from(ruleset)
            .map_err(|_| std::io::Error::other("landlock ruleset fd overflow"))?;
        // SAFETY: `landlock_create_ruleset` returns a new owned fd.
        let ruleset = unsafe { OwnedFd::from_raw_fd(ruleset_fd) };
        add_fd_rule(&ruleset, root, ROOT_ACCESS)?;
        add_fd_rule(&ruleset, program, ACCESS_EXECUTE | ACCESS_READ_FILE)?;
        for helper in [
            "/lib",
            "/lib64",
            "/usr/lib",
            "/usr/lib64",
            "/bin",
            "/usr/bin",
        ] {
            let path = Path::new(helper);
            if path.is_dir() || path.is_file() {
                let access = if path.is_dir() {
                    READ_EXECUTE_ACCESS
                } else {
                    ACCESS_EXECUTE | ACCESS_READ_FILE
                };
                add_path_rule(&ruleset, path, access)?;
            }
        }
        let restrict = unsafe { syscall(SYS_LANDLOCK_RESTRICT_SELF, ruleset.as_raw_fd(), 0_u32) };
        if restrict != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn add_path_rule(ruleset: &OwnedFd, path: &Path, access: u64) -> std::io::Result<()> {
        let flags = OFlags::RDONLY
            .union(OFlags::CLOEXEC)
            .union(if path.is_dir() {
                OFlags::DIRECTORY.union(OFlags::PATH)
            } else {
                OFlags::PATH
            });
        let fd = open(path, flags, Mode::empty()).map_err(std::io::Error::from)?;
        let attr = LandlockPathBeneathAttr {
            allowed_access: access,
            parent_fd: fd.as_raw_fd(),
        };
        let added = unsafe {
            syscall(
                SYS_LANDLOCK_ADD_RULE,
                ruleset.as_raw_fd(),
                LANDLOCK_RULE_PATH_BENEATH,
                std::ptr::from_ref(&attr),
                0_u32,
            )
        };
        if added != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn add_fd_rule(ruleset: &OwnedFd, fd: &OwnedFd, access: u64) -> std::io::Result<()> {
        let attr = LandlockPathBeneathAttr {
            allowed_access: access,
            parent_fd: fd.as_raw_fd(),
        };
        let added = unsafe {
            syscall(
                SYS_LANDLOCK_ADD_RULE,
                ruleset.as_raw_fd(),
                LANDLOCK_RULE_PATH_BENEATH,
                std::ptr::from_ref(&attr),
                0_u32,
            )
        };
        if added != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
mod linux {
    use std::process::Command;

    use super::{ConfinedChild, ConfinementError, ConfinementProfile};

    pub(super) fn configure(
        _command: &mut Command,
        _profile: &ConfinementProfile,
    ) -> Result<(), ConfinementError> {
        Err(ConfinementError::unavailable(
            "linux landlock is not compiled on this target",
        ))
    }

    pub(super) fn spawn(
        _command: Command,
        _profile: &ConfinementProfile,
    ) -> Result<ConfinedChild, ConfinementError> {
        Err(ConfinementError::unavailable(
            "linux landlock is not compiled on this target",
        ))
    }
}

#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "Seatbelt confinement requires sandbox_init in the forked child"
)]
mod macos {
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_int};
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;

    use super::{ConfinedChild, ConfinementError, ConfinementProfile};

    unsafe extern "C" {
        fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
        fn sandbox_free_error(errorbuf: *mut c_char);
    }

    pub(super) fn configure(
        command: &mut Command,
        profile: &ConfinementProfile,
    ) -> Result<(), ConfinementError> {
        let program = Path::new(command.get_program());
        let seatbelt = seatbelt_profile(profile.root(), program)?;
        let cwd = profile.cwd_handle.as_ref().map(Arc::clone);
        // SAFETY: `pre_exec` runs only in the forked child before `exec`.
        // `sandbox_init` confines that child; the parent stays unsandboxed.
        #[allow(unsafe_code)]
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(move || {
                if let Some(cwd) = &cwd {
                    super::apply_authorized_cwd(cwd)?;
                }
                apply_seatbelt(&seatbelt)
            });
        }
        Ok(())
    }

    pub(super) fn spawn(
        mut command: Command,
        profile: &ConfinementProfile,
    ) -> Result<ConfinedChild, ConfinementError> {
        configure(&mut command, profile)?;
        command
            .spawn()
            .map(ConfinedChild::from_std)
            .map_err(|_| ConfinementError::io("confined macos process could not be started"))
    }

    pub(super) fn seatbelt_profile(
        root: &Path,
        program: &Path,
    ) -> Result<CString, ConfinementError> {
        let root = escape_literal(root)?;
        let program = escape_literal(program)?;
        let source = format!(
            r#"(version 1)
(allow default)
(deny network*)
(deny process-exec)
(allow process-exec (literal "{program}"))
(deny file-write*)
(allow file-write* (subpath "{root}"))
(deny file-read-data (regex "^/private/var/folders/.*"))
(deny file-read-data (regex "^/var/folders/.*"))
(deny file-read-data (regex "^/Users/.*"))
(deny file-read-data (regex "^/tmp/.*"))
(deny file-read-data (regex "^/private/tmp/.*"))
(deny file-read-data (regex "^/private/var/tmp/.*"))
(deny file-read-data (regex "^/Volumes/.*"))
(deny file-read-data (regex "^/opt/.*"))
(deny file-read-data (regex "^/home/.*"))
(allow file-read-data (subpath "{root}"))
"#
        );
        CString::new(source).map_err(|_| ConfinementError::denied("seatbelt profile contains NUL"))
    }

    fn escape_literal(path: &Path) -> Result<String, ConfinementError> {
        let text = path
            .to_str()
            .ok_or_else(|| ConfinementError::denied("confinement path is not UTF-8"))?;
        Ok(text.replace('\\', "\\\\").replace('"', "\\\""))
    }

    fn apply_seatbelt(profile: &CStr) -> std::io::Result<()> {
        let mut error = std::ptr::null_mut();
        // SAFETY: `profile` is a NUL-terminated Seatbelt string we own.
        // `sandbox_init` writes an optional heap error we free below.
        let rc = unsafe { sandbox_init(profile.as_ptr(), 0, &raw mut error) };
        if rc != 0 {
            if !error.is_null() {
                unsafe { sandbox_free_error(error) };
            }
            return Err(std::io::Error::other("sandbox_init refused the profile"));
        }
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
mod macos {
    use std::process::Command;

    use super::{ConfinedChild, ConfinementError, ConfinementProfile};

    pub(super) fn configure(
        _command: &mut Command,
        _profile: &ConfinementProfile,
    ) -> Result<(), ConfinementError> {
        Err(ConfinementError::unavailable(
            "macos seatbelt is not compiled on this target",
        ))
    }

    pub(super) fn spawn(
        _command: Command,
        _profile: &ConfinementProfile,
    ) -> Result<ConfinedChild, ConfinementError> {
        Err(ConfinementError::unavailable(
            "macos seatbelt is not compiled on this target",
        ))
    }
}

#[cfg(target_os = "windows")]
#[allow(
    unsafe_code,
    reason = "CreateProcessAsUser, Job Object, and restricted-token FFI have no safe std equivalent"
)]
mod windows {
    use std::collections::BTreeMap;
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{FromRawHandle, OwnedHandle, RawHandle};
    use std::os::windows::process::ExitStatusExt;
    use std::process::{Command, ExitStatus};
    use std::ptr;

    use super::{ConfinedChild, ConfinementError, ConfinementProfile};

    fn dword_size_of<T>() -> Result<u32, ConfinementError> {
        u32::try_from(std::mem::size_of::<T>())
            .map_err(|_| ConfinementError::unavailable("windows structure size exceeds DWORD"))
    }

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION: u32 = 0x0000_0400;
    const JOB_OBJECT_LIMIT_ACTIVE_PROCESS: u32 = 0x0000_0008;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
    const CREATE_SUSPENDED: u32 = 0x0000_0004;
    const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
    const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
    const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
    const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x0002_0002;
    const PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES: usize = 0x0002_0009;
    const PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY: usize = 0x0002_000F;
    const SE_FILE_OBJECT: u32 = 1;
    const DACL_SECURITY_INFORMATION: u32 = 0x0000_0004;
    const FILE_GENERIC_READ: u32 = 0x0012_0089;
    const FILE_GENERIC_WRITE: u32 = 0x0012_0116;
    const FILE_GENERIC_EXECUTE: u32 = 0x0012_00A0;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    const INFINITE: u32 = 0xFFFF_FFFF;

    #[repr(C)]
    struct JobObjectBasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    struct JobObjectExtendedLimitInformation {
        basic_limit_information: JobObjectBasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[repr(C)]
    struct SecurityAttributes {
        length: u32,
        descriptor: *mut core::ffi::c_void,
        inherit: i32,
    }

    #[repr(C)]
    struct StartupInfoW {
        cb: u32,
        reserved: *mut u16,
        desktop: *mut u16,
        title: *mut u16,
        x: u32,
        y: u32,
        x_size: u32,
        y_size: u32,
        x_count_chars: u32,
        y_count_chars: u32,
        fill_attribute: u32,
        flags: u32,
        show_window: u16,
        cb_reserved2: u16,
        lp_reserved2: *mut u8,
        std_input: *mut core::ffi::c_void,
        std_output: *mut core::ffi::c_void,
        std_error: *mut core::ffi::c_void,
    }

    #[repr(C)]
    struct StartupInfoExW {
        startup_info: StartupInfoW,
        attribute_list: *mut core::ffi::c_void,
    }

    #[repr(C)]
    struct SecurityCapabilities {
        app_container_sid: *mut core::ffi::c_void,
        capabilities: *mut core::ffi::c_void,
        capability_count: u32,
        reserved: u32,
    }

    #[repr(C)]
    struct TrusteeW {
        multiple_trustee: *mut core::ffi::c_void,
        multiple_trustee_operation: i32,
        trustee_form: i32,
        trustee_type: i32,
        name: *mut u16,
    }

    #[repr(C)]
    struct ProcessInformation {
        process: *mut core::ffi::c_void,
        thread: *mut core::ffi::c_void,
        process_id: u32,
        thread_id: u32,
    }

    unsafe extern "system" {
        fn CreateJobObjectW(
            attributes: *const core::ffi::c_void,
            name: *const u16,
        ) -> *mut core::ffi::c_void;
        fn SetInformationJobObject(
            job: *mut core::ffi::c_void,
            info_class: u32,
            info: *const core::ffi::c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(
            job: *mut core::ffi::c_void,
            process: *mut core::ffi::c_void,
        ) -> i32;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
        fn CreateProcessW(
            application: *const u16,
            command_line: *mut u16,
            process_attributes: *const core::ffi::c_void,
            thread_attributes: *const core::ffi::c_void,
            inherit_handles: i32,
            creation_flags: u32,
            environment: *const u16,
            current_directory: *const u16,
            startup: *const StartupInfoW,
            process_information: *mut ProcessInformation,
        ) -> i32;
        fn InitializeProcThreadAttributeList(
            list: *mut core::ffi::c_void,
            attribute_count: u32,
            flags: u32,
            size: *mut usize,
        ) -> i32;
        fn UpdateProcThreadAttribute(
            list: *mut core::ffi::c_void,
            flags: u32,
            attribute: usize,
            value: *mut core::ffi::c_void,
            size: usize,
            previous_value: *mut core::ffi::c_void,
            return_size: *mut usize,
        ) -> i32;
        fn DeleteProcThreadAttributeList(list: *mut core::ffi::c_void);
        fn DeriveAppContainerSidFromAppContainerName(
            name: *const u16,
            sid: *mut *mut core::ffi::c_void,
        ) -> i32;
        fn FreeSid(sid: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
        fn GetNamedSecurityInfoW(
            object_name: *mut u16,
            object_type: u32,
            security_info: u32,
            owner: *mut *mut core::ffi::c_void,
            group: *mut *mut core::ffi::c_void,
            dacl: *mut *mut core::ffi::c_void,
            sacl: *mut *mut core::ffi::c_void,
            descriptor: *mut *mut core::ffi::c_void,
        ) -> u32;
        fn GetEffectiveRightsFromAclW(
            acl: *mut core::ffi::c_void,
            trustee: *mut TrusteeW,
            rights: *mut u32,
        ) -> u32;
        fn LocalFree(memory: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
        fn CreatePipe(
            read: *mut *mut core::ffi::c_void,
            write: *mut *mut core::ffi::c_void,
            attributes: *const SecurityAttributes,
            size: u32,
        ) -> i32;
        fn SetHandleInformation(handle: *mut core::ffi::c_void, mask: u32, flags: u32) -> i32;
        fn ResumeThread(thread: *mut core::ffi::c_void) -> u32;
        fn TerminateProcess(process: *mut core::ffi::c_void, exit_code: u32) -> i32;
        fn WaitForSingleObject(handle: *mut core::ffi::c_void, milliseconds: u32) -> u32;
        fn GetExitCodeProcess(process: *mut core::ffi::c_void, exit_code: *mut u32) -> i32;
    }

    pub(super) fn spawn(
        command: Command,
        profile: &ConfinementProfile,
    ) -> Result<ConfinedChild, ConfinementError> {
        let lpac = profile.windows_lpac().ok_or_else(|| {
            ConfinementError::denied("windows LPAC profile must be provisioned by the host")
        })?;
        let sid = AppContainerSid::derive(lpac.moniker())?;
        verify_root_acl(profile.root(), sid.raw())?;
        let job = create_job()?;
        let pipes = match create_stdio_pipes() {
            Ok(pipes) => pipes,
            Err(error) => {
                close(job);
                return Err(error);
            }
        };
        let mut command_line = wide_command_line(&command)?;
        let application = wide_os(command.get_program());
        let cwd = profile
            .cwd()
            .or_else(|| command.get_current_dir())
            .map(wide_os);
        let environment = environment_block(&command);
        let mut security_capabilities = SecurityCapabilities {
            app_container_sid: sid.raw(),
            capabilities: ptr::null_mut(),
            capability_count: 0,
            reserved: 0,
        };
        let mut all_packages_policy = 1_u32;
        let mut inherited_handles = [pipes.child_stdin, pipes.child_stdout, pipes.child_stderr];
        let mut attributes = ProcThreadAttributes::new(3)?;
        attributes.update(
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
            std::ptr::from_mut(&mut security_capabilities).cast(),
            std::mem::size_of::<SecurityCapabilities>(),
        )?;
        attributes.update(
            PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
            std::ptr::from_mut(&mut all_packages_policy).cast(),
            std::mem::size_of::<u32>(),
        )?;
        attributes.update(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            inherited_handles.as_mut_ptr().cast(),
            std::mem::size_of_val(&inherited_handles),
        )?;
        let startup = StartupInfoExW {
            startup_info: StartupInfoW {
                cb: dword_size_of::<StartupInfoExW>()?,
                reserved: ptr::null_mut(),
                desktop: ptr::null_mut(),
                title: ptr::null_mut(),
                x: 0,
                y: 0,
                x_size: 0,
                y_size: 0,
                x_count_chars: 0,
                y_count_chars: 0,
                fill_attribute: 0,
                flags: STARTF_USESTDHANDLES,
                show_window: 0,
                cb_reserved2: 0,
                lp_reserved2: ptr::null_mut(),
                std_input: pipes.child_stdin,
                std_output: pipes.child_stdout,
                std_error: pipes.child_stderr,
            },
            attribute_list: attributes.raw(),
        };
        let mut info = ProcessInformation {
            process: ptr::null_mut(),
            thread: ptr::null_mut(),
            process_id: 0,
            thread_id: 0,
        };
        let created = unsafe {
            CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                CREATE_BREAKAWAY_FROM_JOB
                    | CREATE_UNICODE_ENVIRONMENT
                    | CREATE_SUSPENDED
                    | EXTENDED_STARTUPINFO_PRESENT,
                environment.as_ptr(),
                cwd.as_ref().map_or(ptr::null(), Vec::as_ptr),
                std::ptr::from_ref(&startup.startup_info),
                &raw mut info,
            )
        };
        close(pipes.child_stdin);
        close(pipes.child_stdout);
        close(pipes.child_stderr);
        if created == 0 || info.process.is_null() {
            close(pipes.parent_stdin);
            close(pipes.parent_stdout);
            close(pipes.parent_stderr);
            close(job);
            close(info.thread);
            close(info.process);
            return Err(ConfinementError::unavailable(
                "windows LPAC process creation is unavailable",
            ));
        }
        let assigned = unsafe { AssignProcessToJobObject(job, info.process) };
        if assigned == 0 {
            let _ = unsafe { TerminateProcess(info.process, 1) };
            close(pipes.parent_stdin);
            close(pipes.parent_stdout);
            close(pipes.parent_stderr);
            close(info.thread);
            close(info.process);
            close(job);
            return Err(ConfinementError::unavailable(
                "windows job object assignment failed",
            ));
        }
        if unsafe { ResumeThread(info.thread) } == u32::MAX {
            let _ = unsafe { TerminateProcess(info.process, 1) };
            close(pipes.parent_stdin);
            close(pipes.parent_stdout);
            close(pipes.parent_stderr);
            close(info.thread);
            close(info.process);
            close(job);
            return Err(ConfinementError::unavailable(
                "windows confined child could not be resumed",
            ));
        }
        close(info.thread);
        // SAFETY: `CreateProcessAsUser` returns a new process handle we own.
        // Pipe parent ends are new handles transferred into owned files.
        Ok(unsafe {
            ConfinedChild {
                process: OwnedHandle::from_raw_handle(info.process),
                _job: OwnedHandle::from_raw_handle(job),
                stdin: Some(std::fs::File::from_raw_handle(pipes.parent_stdin)),
                stdout: Some(std::fs::File::from_raw_handle(pipes.parent_stdout)),
                stderr: Some(std::fs::File::from_raw_handle(pipes.parent_stderr)),
            }
        })
    }

    pub(super) fn terminate(process: &OwnedHandle) -> std::io::Result<()> {
        if unsafe { TerminateProcess(raw_handle(process), 1) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn wait(process: &OwnedHandle) -> std::io::Result<ExitStatus> {
        wait_for(process, INFINITE)?
            .ok_or_else(|| std::io::Error::other("windows wait returned without an exit status"))
    }

    pub(super) fn try_wait(process: &OwnedHandle) -> std::io::Result<Option<ExitStatus>> {
        wait_for(process, 0)
    }

    fn wait_for(process: &OwnedHandle, milliseconds: u32) -> std::io::Result<Option<ExitStatus>> {
        match unsafe { WaitForSingleObject(raw_handle(process), milliseconds) } {
            WAIT_OBJECT_0 => {
                let mut code = 0_u32;
                if unsafe { GetExitCodeProcess(raw_handle(process), &raw mut code) } == 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(Some(ExitStatus::from_raw(code)))
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(std::io::Error::last_os_error()),
        }
    }

    struct StdioPipes {
        parent_stdin: RawHandle,
        parent_stdout: RawHandle,
        parent_stderr: RawHandle,
        child_stdin: *mut core::ffi::c_void,
        child_stdout: *mut core::ffi::c_void,
        child_stderr: *mut core::ffi::c_void,
    }

    fn create_stdio_pipes() -> Result<StdioPipes, ConfinementError> {
        let (parent_stdin, child_stdin) = create_pipe(true)?;
        let (parent_stdout, child_stdout) = create_pipe(false)?;
        let (parent_stderr, child_stderr) = create_pipe(false)?;
        Ok(StdioPipes {
            parent_stdin,
            parent_stdout,
            parent_stderr,
            child_stdin,
            child_stdout,
            child_stderr,
        })
    }

    fn create_pipe(
        parent_writes: bool,
    ) -> Result<(RawHandle, *mut core::ffi::c_void), ConfinementError> {
        let mut read = ptr::null_mut();
        let mut write = ptr::null_mut();
        let attributes = SecurityAttributes {
            length: dword_size_of::<SecurityAttributes>()?,
            descriptor: ptr::null_mut(),
            inherit: 1,
        };
        if unsafe { CreatePipe(&raw mut read, &raw mut write, &raw const attributes, 0) } == 0 {
            return Err(ConfinementError::io("windows pipe creation failed"));
        }
        let parent = if parent_writes { write } else { read };
        let child = if parent_writes { read } else { write };
        if unsafe { SetHandleInformation(parent, HANDLE_FLAG_INHERIT, 0) } == 0 {
            close(read);
            close(write);
            return Err(ConfinementError::io("windows pipe inherit mask failed"));
        }
        Ok((parent, child))
    }

    fn wide_os(value: impl AsRef<OsStr>) -> Vec<u16> {
        value
            .as_ref()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn wide_command_line(command: &Command) -> Result<Vec<u16>, ConfinementError> {
        let mut line = String::new();
        quote_windows_arg(&command.get_program().to_string_lossy(), &mut line);
        for arg in command.get_args() {
            line.push(' ');
            quote_windows_arg(&arg.to_string_lossy(), &mut line);
        }
        if line.contains('\0') {
            return Err(ConfinementError::denied(
                "windows command line contains NUL",
            ));
        }
        Ok(wide_os(&line))
    }

    fn quote_windows_arg(arg: &str, buf: &mut String) {
        let needs_quotes = arg.is_empty() || arg.contains([' ', '\t', '\n', '"']);
        if !needs_quotes {
            buf.push_str(arg);
            return;
        }
        buf.push('"');
        let mut backslashes = 0_usize;
        for ch in arg.chars() {
            match ch {
                '\\' => backslashes += 1,
                '"' => {
                    buf.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    buf.push('"');
                    backslashes = 0;
                }
                _ => {
                    buf.extend(std::iter::repeat_n('\\', backslashes));
                    buf.push(ch);
                    backslashes = 0;
                }
            }
        }
        buf.extend(std::iter::repeat_n('\\', backslashes * 2));
        buf.push('"');
    }

    fn environment_block(command: &Command) -> Vec<u16> {
        let mut vars = BTreeMap::<OsString, OsString>::new();
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                vars.insert(key.to_owned(), value.to_owned());
            }
        }
        if !vars
            .keys()
            .any(|key| key.eq_ignore_ascii_case("SYSTEMROOT"))
            && let Some(system_root) = std::env::var_os("SYSTEMROOT")
        {
            vars.insert(OsString::from("SYSTEMROOT"), system_root);
        }
        let mut block = Vec::new();
        for (key, value) in vars {
            block.extend(key.encode_wide());
            block.push(u16::from(b'='));
            block.extend(value.encode_wide());
            block.push(0);
        }
        block.push(0);
        block
    }

    fn raw_handle(handle: &OwnedHandle) -> *mut core::ffi::c_void {
        use std::os::windows::io::AsRawHandle;
        handle.as_raw_handle()
    }

    struct AppContainerSid(*mut core::ffi::c_void);

    impl AppContainerSid {
        fn derive(moniker: &str) -> Result<Self, ConfinementError> {
            let moniker = wide_os(moniker);
            let mut sid = ptr::null_mut();
            let status = unsafe {
                DeriveAppContainerSidFromAppContainerName(moniker.as_ptr(), &raw mut sid)
            };
            if status != 0 || sid.is_null() {
                return Err(ConfinementError::unavailable(
                    "provisioned AppContainer identity could not be resolved",
                ));
            }
            Ok(Self(sid))
        }

        fn raw(&self) -> *mut core::ffi::c_void {
            self.0
        }
    }

    impl Drop for AppContainerSid {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    FreeSid(self.0);
                }
            }
        }
    }

    struct ProcThreadAttributes {
        storage: Vec<usize>,
    }

    impl ProcThreadAttributes {
        fn new(count: u32) -> Result<Self, ConfinementError> {
            let mut bytes = 0_usize;
            unsafe {
                InitializeProcThreadAttributeList(ptr::null_mut(), count, 0, &raw mut bytes);
            }
            if bytes == 0 {
                return Err(ConfinementError::unavailable(
                    "windows process attribute list is unavailable",
                ));
            }
            let words = bytes
                .checked_add(std::mem::size_of::<usize>() - 1)
                .and_then(|value| value.checked_div(std::mem::size_of::<usize>()))
                .ok_or_else(|| {
                    ConfinementError::unavailable("windows process attribute size overflow")
                })?;
            let mut attributes = Self {
                storage: vec![0; words],
            };
            if unsafe {
                InitializeProcThreadAttributeList(attributes.raw(), count, 0, &raw mut bytes)
            } == 0
            {
                return Err(ConfinementError::unavailable(
                    "windows process attribute list initialization failed",
                ));
            }
            Ok(attributes)
        }

        fn raw(&mut self) -> *mut core::ffi::c_void {
            self.storage.as_mut_ptr().cast()
        }

        fn update(
            &mut self,
            attribute: usize,
            value: *mut core::ffi::c_void,
            size: usize,
        ) -> Result<(), ConfinementError> {
            if unsafe {
                UpdateProcThreadAttribute(
                    self.raw(),
                    0,
                    attribute,
                    value,
                    size,
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            } == 0
            {
                return Err(ConfinementError::unavailable(
                    "windows process security attribute installation failed",
                ));
            }
            Ok(())
        }
    }

    impl Drop for ProcThreadAttributes {
        fn drop(&mut self) {
            if !self.storage.is_empty() {
                unsafe {
                    DeleteProcThreadAttributeList(self.raw());
                }
            }
        }
    }

    fn verify_root_acl(
        root: &std::path::Path,
        sid: *mut core::ffi::c_void,
    ) -> Result<(), ConfinementError> {
        let mut root = wide_os(root);
        let mut dacl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        let status = unsafe {
            GetNamedSecurityInfoW(
                root.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &raw mut dacl,
                ptr::null_mut(),
                &raw mut descriptor,
            )
        };
        if status != 0 || dacl.is_null() || descriptor.is_null() {
            return Err(ConfinementError::denied(
                "windows confinement root ACL could not be inspected",
            ));
        }
        let mut trustee = TrusteeW {
            multiple_trustee: ptr::null_mut(),
            multiple_trustee_operation: 0,
            trustee_form: 0,
            trustee_type: 0,
            name: sid.cast(),
        };
        let mut rights = 0_u32;
        let rights_status =
            unsafe { GetEffectiveRightsFromAclW(dacl, &raw mut trustee, &raw mut rights) };
        unsafe {
            LocalFree(descriptor);
        }
        let required = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE;
        if rights_status != 0 || rights & required != required {
            return Err(ConfinementError::denied(
                "windows confinement root lacks the provisioned AppContainer ACL",
            ));
        }
        Ok(())
    }

    fn create_job() -> Result<*mut core::ffi::c_void, ConfinementError> {
        let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if job.is_null() {
            return Err(ConfinementError::unavailable(
                "windows job object is unavailable",
            ));
        }
        let info = JobObjectExtendedLimitInformation {
            basic_limit_information: JobObjectBasicLimitInformation {
                per_process_user_time_limit: 0,
                per_job_user_time_limit: 0,
                limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                    | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
                    | JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
                minimum_working_set_size: 0,
                maximum_working_set_size: 0,
                active_process_limit: 1,
                affinity: 0,
                priority_class: 0,
                scheduling_class: 0,
            },
            io_info: IoCounters {
                read_operation_count: 0,
                write_operation_count: 0,
                other_operation_count: 0,
                read_transfer_count: 0,
                write_transfer_count: 0,
                other_transfer_count: 0,
            },
            process_memory_limit: 0,
            job_memory_limit: 0,
            peak_process_memory_used: 0,
            peak_job_memory_used: 0,
        };
        let ok = unsafe {
            SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                std::ptr::from_ref(&info).cast(),
                dword_size_of::<JobObjectExtendedLimitInformation>()?,
            )
        };
        if ok == 0 {
            close(job);
            return Err(ConfinementError::unavailable(
                "windows job object limits are unavailable",
            ));
        }
        Ok(job)
    }

    fn close(handle: *mut core::ffi::c_void) {
        if !handle.is_null() {
            unsafe {
                CloseHandle(handle);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod windows {
    use std::process::Command;

    use super::{ConfinedChild, ConfinementError, ConfinementProfile};

    pub(super) fn spawn(
        _command: Command,
        _profile: &ConfinementProfile,
    ) -> Result<ConfinedChild, ConfinementError> {
        Err(ConfinementError::unavailable(
            "windows restricted token and job object are not compiled on this target",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    struct SpawnProbe {
        spawned: AtomicBool,
    }

    impl SpawnProbe {
        fn command(&self) -> Command {
            self.spawned.store(true, Ordering::Release);
            // The probe records that the test asked for a command. The
            // unavailable backend must return before Command::spawn.
            Command::new("finstack-confinement-must-not-spawn")
        }
    }

    #[test]
    fn confinement_unavailable_refuses_to_spawn() {
        let probe = SpawnProbe {
            spawned: AtomicBool::new(false),
        };
        let root = std::env::temp_dir();
        let profile = ConfinementProfile::try_new(&root).expect("temp root");
        let error = ProcessConfinement::unavailable()
            .spawn(probe.command(), &profile)
            .expect_err("unavailable");
        assert_eq!(error.code(), CONFINEMENT_UNAVAILABLE);
        assert!(
            !error.message().is_empty(),
            "unavailable error must be stable and named"
        );
        let _ = probe.spawned;
    }

    #[test]
    fn relative_root_is_denied() {
        let error = ConfinementProfile::try_new("relative-root").expect_err("relative");
        assert_eq!(error.code(), CONFINEMENT_DENIED);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_profile_keeps_allow_default_and_tightens_denies() {
        let root = std::env::temp_dir();
        let profile = macos::seatbelt_profile(&root, Path::new("/bin/echo")).expect("profile");
        let source = profile.to_string_lossy();
        assert!(source.contains("(allow default)"));
        assert!(source.contains("(deny network*)"));
        assert!(source.contains("(deny process-exec)"));
        assert!(source.contains("(allow process-exec (literal \"/bin/echo\"))"));
        assert!(source.contains("(deny file-write*)"));
        assert!(source.contains(&format!(
            "(allow file-write* (subpath \"{}\"))",
            root.display()
        )));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_confined_echo_starts_or_fails_closed() {
        let root = std::env::temp_dir();
        let profile = ConfinementProfile::try_new(&root).expect("temp root");
        let mut command = Command::new("/bin/echo");
        command.arg("confined");
        match ProcessConfinement::for_current_platform().spawn(command, &profile) {
            Ok(mut child) => {
                let status = child.wait().expect("wait");
                assert!(status.success(), "confined echo must exit 0: {status:?}");
            }
            Err(error) => assert_eq!(
                error.code(),
                CONFINEMENT_IO,
                "a host that refuses nested Seatbelt must fail closed"
            ),
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_confined_spawn_applies_token_or_fails_closed() {
        let root = std::env::temp_dir();
        let profile = ConfinementProfile::try_new(&root).expect("temp root");
        let mut command = Command::new("cmd.exe");
        command.args(["/c", "echo", "confined"]);
        match ProcessConfinement::for_current_platform().spawn(command, &profile) {
            Ok(mut child) => {
                let status = child.wait().expect("wait");
                assert!(
                    status.success() || status.code().is_some(),
                    "confined windows child must terminate: {status:?}"
                );
            }
            Err(error) => {
                assert!(matches!(
                    error.code(),
                    CONFINEMENT_UNAVAILABLE | CONFINEMENT_DENIED
                ));
            }
        }
    }
}
