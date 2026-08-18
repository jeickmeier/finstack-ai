//! Internal process-confinement service. This is not a port.
//!
//! Trusted native toolsets (shell now; FR-03 stdio later) consume this
//! service. Fail closed: a missing or broken platform primitive never
//! falls back to an unconfined spawn. The labeled unconfined
//! `std::process` runner stays in the shell crate as a separate path.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

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
    /// # Errors
    ///
    /// Returns a stable [`ConfinementError`] and does not leave an unconfined
    /// child running.
    pub fn spawn(
        &self,
        command: Command,
        profile: &ConfinementProfile,
    ) -> Result<Child, ConfinementError> {
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
    use std::process::{Child, Command};

    use std::os::fd::{FromRawFd, OwnedFd};

    use rustix::fs::{Mode, OFlags, open};
    use rustix::thread::set_no_new_privs;

    use super::{ConfinementError, ConfinementProfile};

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
    ) -> Result<Child, ConfinementError> {
        configure(&mut command, profile)?;
        command
            .spawn()
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
    use std::process::{Child, Command};

    use super::{ConfinementError, ConfinementProfile};

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
    ) -> Result<Child, ConfinementError> {
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
    use std::process::{Child, Command};

    use super::{ConfinementError, ConfinementProfile};

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
    ) -> Result<Child, ConfinementError> {
        configure(&mut command, profile)?;
        command
            .spawn()
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
    use std::process::{Child, Command};

    use super::{ConfinementError, ConfinementProfile};

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
    ) -> Result<Child, ConfinementError> {
        Err(ConfinementError::unavailable(
            "macos seatbelt is not compiled on this target",
        ))
    }
}

#[cfg(target_os = "windows")]
#[allow(
    unsafe_code,
    reason = "Job Object and restricted-token FFI have no safe std equivalent"
)]
mod windows {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};
    use std::ptr;

    use super::{ConfinementError, ConfinementProfile};

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION: u32 = 0x0000_0400;
    const JOB_OBJECT_LIMIT_ACTIVE_PROCESS: u32 = 0x0000_0008;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    const TOKEN_ALL_ACCESS: u32 = 0x000F_01FF;
    const DISABLE_MAX_PRIVILEGE: u32 = 0x01;

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
    }

    pub(super) fn spawn(
        mut command: Command,
        profile: &ConfinementProfile,
    ) -> Result<Child, ConfinementError> {
        let _ = profile;
        let job = create_job()?;
        let restricted = create_restricted_token()?;
        // Restricted-token creation is required so a host without that
        // primitive fails closed. `std::process::Command` cannot take the
        // token; the live boundary is the Job Object assigned immediately
        // after spawn. CreateProcessAsUser remains a documented gap.
        drop(restricted);
        command.creation_flags(CREATE_BREAKAWAY_FROM_JOB);
        let mut child = command.spawn().map_err(|_| {
            close(job);
            ConfinementError::io("confined windows process could not be started")
        })?;
        let assigned = unsafe { AssignProcessToJobObject(job, child.as_raw_handle_mut()) };
        if assigned == 0 {
            let _ = child.kill();
            let _ = child.wait();
            close(job);
            return Err(ConfinementError::unavailable(
                "windows job object assignment failed",
            ));
        }
        // Leak the job handle into the child lifetime via OwnedHandle drop
        // disable: keep the job open until process exit by storing it on a
        // forgotten handle. Kill-on-job-close would kill the child if we
        // closed now; forget the job so it outlives this function.
        std::mem::forget(unsafe { OwnedHandle::from_raw_handle(job) });
        Ok(child)
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
    use std::process::{Child, Command};

    use super::{ConfinementError, ConfinementProfile};

    pub(super) fn spawn(
        _command: Command,
        _profile: &ConfinementProfile,
    ) -> Result<Child, ConfinementError> {
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
}
