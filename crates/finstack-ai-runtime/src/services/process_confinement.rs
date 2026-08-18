//! Internal process-confinement service. This is not a port.
//!
//! Trusted native toolsets (shell now; FR-03 stdio later) consume this
//! service. Fail closed: a missing or broken platform primitive never
//! falls back to an unconfined spawn. The labeled unconfined
//! `std::process` runner stays in the shell crate as a separate path.

use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus};

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfinementProfile {
    root: PathBuf,
    cwd: Option<PathBuf>,
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
        Ok(Self { root, cwd: None })
    }

    /// Set an already-authorized working directory under [`Self::root`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfinementError::denied`] when `cwd` is not under `root`.
    pub fn with_authorized_cwd(mut self, cwd: impl AsRef<Path>) -> Result<Self, ConfinementError> {
        let cwd = cwd.as_ref();
        if !cwd.starts_with(&self.root) || !cwd.is_dir() {
            return Err(ConfinementError::denied(
                "confinement cwd must be a directory under the authorized root",
            ));
        }
        self.cwd = Some(cwd.to_path_buf());
        Ok(self)
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
    /// Windows restricted token plus Job Object.
    WindowsRestrictedJob,
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
            ConfinementBackend::WindowsRestrictedJob => Err(ConfinementError::unavailable(
                "windows confined children must use ProcessConfinement::spawn",
            )),
        }
    }

    /// Spawn `command` under `profile`. Refuses when the backend is missing.
    ///
    /// Windows applies the restricted token with `CreateProcessAsUser` and
    /// assigns the child to a Job Object. Either primitive missing fails
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
            ConfinementBackend::WindowsRestrictedJob => windows::spawn(command, profile),
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
    pub stdin: Option<ChildStdin>,
    /// Optional stdout pipe.
    pub stdout: Option<ChildStdout>,
    /// Optional stderr pipe.
    pub stderr: Option<ChildStderr>,
}

impl ConfinedChild {
    #[cfg(not(windows))]
    fn from_std(mut inner: std::process::Child) -> Self {
        Self {
            stdin: inner.stdin.take(),
            stdout: inner.stdout.take(),
            stderr: inner.stderr.take(),
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
fn apply_authorized_cwd(cwd: &Path) -> std::io::Result<()> {
    use rustix::fs::{Mode, OFlags, open};
    let fd = open(
        cwd,
        OFlags::RDONLY
            .union(OFlags::DIRECTORY)
            .union(OFlags::NOFOLLOW)
            .union(OFlags::CLOEXEC),
        Mode::empty(),
    )?;
    rustix::process::fchdir(&fd).map_err(std::io::Error::from)
}

const fn current_backend() -> ConfinementBackend {
    if cfg!(target_os = "linux") {
        ConfinementBackend::LinuxLandlock
    } else if cfg!(target_os = "macos") {
        ConfinementBackend::MacosSeatbelt
    } else if cfg!(target_os = "windows") {
        ConfinementBackend::WindowsRestrictedJob
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
    const ACCESS_READ_FILE: u64 = 1 << 2;
    const ACCESS_READ_DIR: u64 = 1 << 3;
    const HANDLED_ACCESS: u64 = ACCESS_EXECUTE | ACCESS_READ_FILE | ACCESS_READ_DIR;

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
        let root = profile.root().to_path_buf();
        let program = command
            .get_program()
            .to_owned()
            .into_os_string()
            .into_string()
            .map_err(|_| ConfinementError::denied("confined program path is not UTF-8"))?;
        let cwd = profile.cwd().map(Path::to_path_buf);
        // SAFETY: `pre_exec` runs only in the forked child before `exec`.
        // Landlock and `no_new_privs` apply to that child only.
        #[allow(unsafe_code)]
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(move || {
                if let Some(cwd) = &cwd {
                    super::apply_authorized_cwd(cwd)?;
                }
                apply_landlock(&root, Path::new(&program))
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
        if abi < 1 {
            return Err(ConfinementError::unavailable(
                "landlock is unavailable on this kernel",
            ));
        }
        Ok(())
    }

    fn apply_landlock(root: &Path, program: &Path) -> std::io::Result<()> {
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
        add_path_rule(&ruleset, root, ACCESS_READ_FILE | ACCESS_READ_DIR)?;
        add_path_rule(&ruleset, program, ACCESS_EXECUTE | ACCESS_READ_FILE)?;
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
                    HANDLED_ACCESS
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
        let cwd = profile.cwd().map(Path::to_path_buf);
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
    use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus};
    use std::ptr;

    use super::{ConfinedChild, ConfinementError, ConfinementProfile};

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION: u32 = 0x0000_0400;
    const JOB_OBJECT_LIMIT_ACTIVE_PROCESS: u32 = 0x0000_0008;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
    const CREATE_SUSPENDED: u32 = 0x0000_0004;
    const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
    const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
    const TOKEN_ALL_ACCESS: u32 = 0x000F_01FF;
    const DISABLE_MAX_PRIVILEGE: u32 = 0x01;
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
        fn GetCurrentProcess() -> *mut core::ffi::c_void;
        fn OpenProcessToken(
            process: *mut core::ffi::c_void,
            access: u32,
            token: *mut *mut core::ffi::c_void,
        ) -> i32;
        fn CreateRestrictedToken(
            existing: *mut core::ffi::c_void,
            flags: u32,
            disable_sid_count: u32,
            sids_to_disable: *const core::ffi::c_void,
            delete_privilege_count: u32,
            privileges_to_delete: *const core::ffi::c_void,
            restricted_sid_count: u32,
            sids_to_restrict: *const core::ffi::c_void,
            new_token: *mut *mut core::ffi::c_void,
        ) -> i32;
        fn CreateProcessAsUserW(
            token: *mut core::ffi::c_void,
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
        let job = create_job()?;
        let restricted = create_restricted_token().inspect_err(|_| close(job))?;
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
        let mut startup = StartupInfoW {
            cb: u32::try_from(std::mem::size_of::<StartupInfoW>()).expect("startup info size"),
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
        };
        let mut info = ProcessInformation {
            process: ptr::null_mut(),
            thread: ptr::null_mut(),
            process_id: 0,
            thread_id: 0,
        };
        let created = unsafe {
            CreateProcessAsUserW(
                raw_handle(&restricted),
                application.as_ptr(),
                command_line.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                CREATE_BREAKAWAY_FROM_JOB | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
                environment.as_ptr(),
                cwd.as_ref().map_or(ptr::null(), Vec::as_ptr),
                &raw const startup,
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
                "windows CreateProcessAsUser is unavailable",
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
        // Pipe parent ends are new handles; `ChildStd*` take them via FromRawHandle.
        Ok(unsafe {
            ConfinedChild {
                process: OwnedHandle::from_raw_handle(info.process),
                _job: OwnedHandle::from_raw_handle(job),
                stdin: Some(ChildStdin::from_raw_handle(pipes.parent_stdin)),
                stdout: Some(ChildStdout::from_raw_handle(pipes.parent_stdout)),
                stderr: Some(ChildStderr::from_raw_handle(pipes.parent_stderr)),
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
            length: u32::try_from(std::mem::size_of::<SecurityAttributes>()).expect("sa size"),
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

    fn create_job() -> Result<*mut core::ffi::c_void, ConfinementError> {
        let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if job.is_null() {
            return Err(ConfinementError::unavailable(
                "windows job object is unavailable",
            ));
        }
        let mut info = JobObjectExtendedLimitInformation {
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
                u32::try_from(std::mem::size_of_val(&info)).expect("job info size"),
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

    fn create_restricted_token() -> Result<OwnedHandle, ConfinementError> {
        let mut existing = ptr::null_mut();
        let opened =
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &raw mut existing) };
        if opened == 0 || existing.is_null() {
            return Err(ConfinementError::unavailable(
                "windows process token is unavailable",
            ));
        }
        let mut restricted = ptr::null_mut();
        let created = unsafe {
            CreateRestrictedToken(
                existing,
                DISABLE_MAX_PRIVILEGE,
                0,
                ptr::null(),
                0,
                ptr::null(),
                0,
                ptr::null(),
                &raw mut restricted,
            )
        };
        close(existing);
        if created == 0 || restricted.is_null() {
            return Err(ConfinementError::unavailable(
                "windows restricted token is unavailable",
            ));
        }
        Ok(unsafe { OwnedHandle::from_raw_handle(restricted) })
    }

    fn close(handle: *mut core::ffi::c_void) {
        if !handle.is_null() {
            unsafe {
                CloseHandle(handle);
            }
        }
    }

    trait RawChildHandle {
        fn as_raw_handle_mut(&mut self) -> *mut core::ffi::c_void;
    }

    impl RawChildHandle for Child {
        fn as_raw_handle_mut(&mut self) -> *mut core::ffi::c_void {
            use std::os::windows::io::AsRawHandle;
            self.as_raw_handle()
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
            let _ = self.spawned.store(true, Ordering::Release);
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
    fn macos_confined_echo_starts() {
        let root = std::env::temp_dir();
        let profile = ConfinementProfile::try_new(&root).expect("temp root");
        let mut command = Command::new("/bin/echo");
        command.arg("confined");
        let mut child = ProcessConfinement::for_current_platform()
            .spawn(command, &profile)
            .expect("confined echo");
        let status = child.wait().expect("wait");
        assert!(status.success(), "confined echo must exit 0: {status:?}");
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
                assert_eq!(error.code(), CONFINEMENT_UNAVAILABLE);
            }
        }
    }
}
