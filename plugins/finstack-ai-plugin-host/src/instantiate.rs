//! Host-import linking, instantiation, trap mapping, and cancellation.

use std::collections::BTreeSet;
use std::future::Future;
use std::time::Duration;

use finstack_ai_kernel::Timestamp;
use finstack_ai_runtime::CancellationSignal;
use finstack_ai_wit::{MAX_STRING_BYTES, reject_before_allocation, reject_declared_len};
use wasmtime::component::{Component, HasData, Linker, ResourceTable};
use wasmtime::{Engine, Store, StoreLimits, Trap};
use wasmtime_wasi::clocks::{WasiClocks, WasiClocksView};
use wasmtime_wasi::filesystem::{WasiFilesystem, WasiFilesystemView};
use wasmtime_wasi::random::WasiRandom;
use wasmtime_wasi::sockets::{WasiSockets, WasiSocketsView};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::bindings::toolset::finstack::ai_host::blobs;
use crate::bindings::toolset::finstack::ai_host::logging::{self, Level};
use crate::bindings::toolset::finstack::ai_types::types::{
    self as types, BlobRef, CallContext, PluginError,
};
use crate::bindings::v1::toolset::finstack::ai_host::blobs as blobs_v1;
use crate::bindings::v1::toolset::finstack::ai_host::logging as logging_v1;
use crate::bindings::v1::toolset::finstack::ai_types::types as types_v1;
use crate::error::PluginHostError;
use crate::grants::{GrantResources, can_link};
use crate::limits::{EffectiveLimits, store_limits};

/// Send + Sync host-import state. This is not the in-process `RecordingLogger`
/// (`RefCell`) from `finstack-ai-wit`.
pub struct HostState {
    logs: Vec<(String, String)>,
    blob: Vec<u8>,
    limits: StoreLimits,
    wasi: WasiCtx,
    table: ResourceTable,
}

impl HostState {
    /// Construct deny-by-default host import state with `limits`.
    #[must_use]
    pub fn new(limits: StoreLimits) -> Self {
        Self {
            logs: Vec::new(),
            blob: Vec::new(),
            limits,
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
        }
    }

    /// Construct state with granted, resource-backed preopens only.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError::InstantiateFailed`] when a configured
    /// preopen path cannot be opened.
    pub fn try_with_grants(
        limits: StoreLimits,
        granted: &BTreeSet<String>,
        resources: &GrantResources,
    ) -> Result<Self, PluginHostError> {
        Ok(Self {
            logs: Vec::new(),
            blob: Vec::new(),
            limits,
            wasi: wasi_ctx(granted, resources)?,
            table: ResourceTable::new(),
        })
    }

    /// Construct state that can serve one blob body.
    #[must_use]
    pub fn with_blob(limits: StoreLimits, bytes: Vec<u8>) -> Self {
        Self {
            logs: Vec::new(),
            blob: bytes,
            limits,
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
        }
    }

    /// Recorded log lines for diagnostics.
    #[must_use]
    pub fn logs(&self) -> &[(String, String)] {
        &self.logs
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl types::Host for HostState {}

impl logging::Host for HostState {
    fn log(
        &mut self,
        _context: CallContext,
        level: Level,
        message: String,
    ) -> Result<(), PluginError> {
        reject_before_allocation(message.as_bytes(), MAX_STRING_BYTES, "log.message").map_err(
            |error| PluginError {
                code: error.code().to_owned(),
                message: error.to_string(),
                retryable: false,
            },
        )?;
        self.logs.push((format!("{level:?}"), message));
        Ok(())
    }
}

impl blobs::Host for HostState {
    fn read(
        &mut self,
        _context: CallContext,
        _reference: BlobRef,
        offset: u64,
        max_bytes: u64,
    ) -> Result<Vec<u8>, PluginError> {
        reject_declared_len(max_bytes, MAX_STRING_BYTES, "blobs.read").map_err(|error| {
            PluginError {
                code: error.code().to_owned(),
                message: error.to_string(),
                retryable: false,
            }
        })?;
        let start = usize::try_from(offset).unwrap_or(self.blob.len());
        let take = usize::try_from(max_bytes).unwrap_or(0);
        if start >= self.blob.len() {
            return Ok(Vec::new());
        }
        let end = start.saturating_add(take).min(self.blob.len());
        Ok(self.blob[start..end].to_vec())
    }
}

impl types_v1::Host for HostState {}

impl logging_v1::Host for HostState {
    fn log(
        &mut self,
        _context: types_v1::CallContext,
        level: logging_v1::Level,
        message: String,
    ) -> Result<(), types_v1::PluginError> {
        reject_before_allocation(message.as_bytes(), MAX_STRING_BYTES, "log.message").map_err(
            |error| types_v1::PluginError {
                code: error.code().to_owned(),
                message: error.to_string(),
                retryable: false,
            },
        )?;
        self.logs.push((format!("{level:?}"), message));
        Ok(())
    }
}

impl blobs_v1::Host for HostState {
    fn read(
        &mut self,
        _context: types_v1::CallContext,
        _reference: types_v1::BlobRef,
        offset: u64,
        max_bytes: u64,
    ) -> Result<Vec<u8>, types_v1::PluginError> {
        reject_declared_len(max_bytes, MAX_STRING_BYTES, "blobs.read").map_err(|error| {
            types_v1::PluginError {
                code: error.code().to_owned(),
                message: error.to_string(),
                retryable: false,
            }
        })?;
        let start = usize::try_from(offset).unwrap_or(self.blob.len());
        let take = usize::try_from(max_bytes).unwrap_or(0);
        if start >= self.blob.len() {
            return Ok(Vec::new());
        }
        let end = start.saturating_add(take).min(self.blob.len());
        Ok(self.blob[start..end].to_vec())
    }
}

/// Map a Wasmtime failure onto a stable plugin code.
#[must_use]
pub fn map_wasmtime_error(error: &wasmtime::Error, cancelled: bool) -> PluginHostError {
    if cancelled {
        return PluginHostError::Timeout;
    }
    if let Some(trap) = trap_code(error) {
        return match trap {
            Trap::Interrupt => PluginHostError::Timeout,
            Trap::OutOfFuel => PluginHostError::ResourceLimit(error.to_string()),
            _ => PluginHostError::Trap(error.to_string()),
        };
    }
    let message = error_chain(error);
    if is_interrupt(&message) {
        return PluginHostError::Timeout;
    }
    if is_resource_limit(&message) {
        return PluginHostError::ResourceLimit(message);
    }
    if is_trap(&message) {
        return PluginHostError::Trap(message);
    }
    PluginHostError::InstantiateFailed(error.to_string())
}

fn error_chain(error: &wasmtime::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut current = error.source();
    while let Some(next) = current {
        parts.push(next.to_string());
        current = next.source();
    }
    parts.join(" | ")
}

fn trap_code(error: &wasmtime::Error) -> Option<wasmtime::Trap> {
    error
        .downcast_ref::<wasmtime::Trap>()
        .copied()
        .or_else(|| error.root_cause().downcast_ref::<wasmtime::Trap>().copied())
}

fn is_interrupt(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("interrupt") || lower.contains("epoch deadline")
}

fn is_trap(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("unreachable")
        || lower.contains("wasm trap")
        || lower.contains("unconditional trap")
        || lower.contains("wasmtime::trap")
}

fn is_resource_limit(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("all fuel")
        || lower.contains("out of fuel")
        || lower.contains("resource limiter")
        || lower.contains("insufficient fuel")
        || lower.contains("forcing trap when growing")
        || lower.contains("failed to grow memory")
        || lower.contains("failed to grow table")
        || lower.contains("growing memory")
        || lower.contains("growing table")
}

/// Prepare a store with fuel, limiter, and epoch interruption.
///
/// # Errors
///
/// Returns [`PluginHostError::ResourceLimit`] when fuel cannot be installed.
pub fn new_store(
    engine: &Engine,
    state: HostState,
    fuel: u64,
) -> Result<Store<HostState>, PluginHostError> {
    let mut store = Store::new(engine, state);
    store.limiter(|state| &mut state.limits);
    store
        .set_fuel(fuel)
        .map_err(|error| PluginHostError::ResourceLimit(format!("fuel: {error}")))?;
    store.set_epoch_deadline(1);
    Ok(store)
}

/// Add only granted, resource-backed WASI interfaces. Never blanket-links
/// filesystem, sockets, HTTP, or CLI.
///
/// # Errors
///
/// Returns [`PluginHostError::InstantiateFailed`] when a selected interface
/// cannot be linked.
pub fn link_granted_wasi(
    linker: &mut Linker<HostState>,
    granted: &BTreeSet<String>,
    resources: &GrantResources,
) -> Result<(), PluginHostError> {
    if can_link("clock", granted, resources) {
        wasmtime_wasi::p2::bindings::clocks::monotonic_clock::add_to_linker::<_, WasiClocks>(
            linker,
            HostState::clocks,
        )
        .map_err(|error| PluginHostError::InstantiateFailed(format!("link clock: {error}")))?;
        wasmtime_wasi::p2::bindings::clocks::wall_clock::add_to_linker::<_, WasiClocks>(
            linker,
            HostState::clocks,
        )
        .map_err(|error| PluginHostError::InstantiateFailed(format!("link clock: {error}")))?;
    } else if can_link("filesystem", granted, resources) {
        // `wasi:filesystem/types@0.2.12` uses `wasi:clocks/wall-clock` datetime.
        // This is an ABI dependency of a granted preopen, not a clock grant.
        wasmtime_wasi::p2::bindings::clocks::wall_clock::add_to_linker::<_, WasiClocks>(
            linker,
            HostState::clocks,
        )
        .map_err(|error| PluginHostError::InstantiateFailed(format!("link fs clock: {error}")))?;
    }
    if can_link("random", granted, resources) {
        wasmtime_wasi::p2::bindings::random::random::add_to_linker::<_, WasiRandom>(
            linker,
            |state| state.ctx().ctx.random(),
        )
        .map_err(|error| PluginHostError::InstantiateFailed(format!("link random: {error}")))?;
        wasmtime_wasi::p2::bindings::random::insecure::add_to_linker::<_, WasiRandom>(
            linker,
            |state| state.ctx().ctx.random(),
        )
        .map_err(|error| PluginHostError::InstantiateFailed(format!("link random: {error}")))?;
        wasmtime_wasi::p2::bindings::random::insecure_seed::add_to_linker::<_, WasiRandom>(
            linker,
            |state| state.ctx().ctx.random(),
        )
        .map_err(|error| PluginHostError::InstantiateFailed(format!("link random: {error}")))?;
    }
    if can_link("filesystem", granted, resources) {
        link_wasi_io(linker)?;
        wasmtime_wasi::p2::bindings::filesystem::preopens::add_to_linker::<_, WasiFilesystem>(
            linker,
            HostState::filesystem,
        )
        .map_err(|error| PluginHostError::InstantiateFailed(format!("link fs: {error}")))?;
        wasmtime_wasi::p2::bindings::filesystem::types::add_to_linker::<_, WasiFilesystem>(
            linker,
            HostState::filesystem,
        )
        .map_err(|error| PluginHostError::InstantiateFailed(format!("link fs: {error}")))?;
    }
    if can_link("network", granted, resources) {
        let options = wasmtime_wasi::p2::bindings::LinkOptions::default();
        wasmtime_wasi::p2::bindings::sockets::instance_network::add_to_linker::<_, WasiSockets>(
            linker,
            HostState::sockets,
        )
        .map_err(|error| PluginHostError::InstantiateFailed(format!("link net: {error}")))?;
        wasmtime_wasi::p2::bindings::sockets::network::add_to_linker::<_, WasiSockets>(
            linker,
            &options.into(),
            HostState::sockets,
        )
        .map_err(|error| PluginHostError::InstantiateFailed(format!("link net: {error}")))?;
        wasmtime_wasi::p2::bindings::sockets::tcp_create_socket::add_to_linker::<_, WasiSockets>(
            linker,
            HostState::sockets,
        )
        .map_err(|error| PluginHostError::InstantiateFailed(format!("link net: {error}")))?;
        wasmtime_wasi::p2::bindings::sockets::udp_create_socket::add_to_linker::<_, WasiSockets>(
            linker,
            HostState::sockets,
        )
        .map_err(|error| PluginHostError::InstantiateFailed(format!("link net: {error}")))?;
        wasmtime_wasi::p2::bindings::sockets::ip_name_lookup::add_to_linker::<_, WasiSockets>(
            linker,
            HostState::sockets,
        )
        .map_err(|error| PluginHostError::InstantiateFailed(format!("link net: {error}")))?;
    }
    Ok(())
}

struct HasIo;

impl HasData for HasIo {
    type Data<'a> = &'a mut ResourceTable;
}

fn link_wasi_io(linker: &mut Linker<HostState>) -> Result<(), PluginHostError> {
    wasmtime_wasi::p2::bindings::io::error::add_to_linker::<HostState, HasIo>(linker, |state| {
        state.ctx().table
    })
    .map_err(|error| PluginHostError::InstantiateFailed(format!("link io: {error}")))?;
    wasmtime_wasi::p2::bindings::io::poll::add_to_linker::<HostState, HasIo>(linker, |state| {
        state.ctx().table
    })
    .map_err(|error| PluginHostError::InstantiateFailed(format!("link io: {error}")))?;
    wasmtime_wasi::p2::bindings::io::streams::add_to_linker::<HostState, HasIo>(linker, |state| {
        state.ctx().table
    })
    .map_err(|error| PluginHostError::InstantiateFailed(format!("link io: {error}")))?;
    Ok(())
}

fn wasi_ctx(
    granted: &BTreeSet<String>,
    resources: &GrantResources,
) -> Result<WasiCtx, PluginHostError> {
    let mut builder = WasiCtxBuilder::new();
    if can_link("filesystem", granted, resources) {
        for preopen in &resources.filesystem {
            let dir_perms = match (preopen.read, preopen.write) {
                (true, true) => DirPerms::READ | DirPerms::MUTATE,
                (true, false) => DirPerms::READ,
                (false, true) => DirPerms::MUTATE,
                (false, false) => DirPerms::empty(),
            };
            let file_perms = match (preopen.read, preopen.write) {
                (true, true) => FilePerms::READ | FilePerms::WRITE,
                (true, false) => FilePerms::READ,
                (false, true) => FilePerms::WRITE,
                (false, false) => FilePerms::empty(),
            };
            builder
                .preopened_dir(
                    &preopen.host_path,
                    &preopen.guest_path,
                    dir_perms,
                    file_perms,
                )
                .map_err(|error| PluginHostError::InstantiateFailed(format!("preopen: {error}")))?;
        }
    }
    Ok(builder.build())
}

/// Host state plus fuel for one instantiate.
///
/// # Errors
///
/// Returns [`PluginHostError::InstantiateFailed`] when a granted preopen
/// cannot be opened.
pub fn host_state_for(
    limits: EffectiveLimits,
    granted: &BTreeSet<String>,
    resources: &GrantResources,
) -> Result<HostState, PluginHostError> {
    HostState::try_with_grants(store_limits(limits), granted, resources)
}

/// Run `work` until it completes or `cancel` fires. Firing increments the
/// engine epoch so guest code can leave.
///
/// # Errors
///
/// Returns [`PluginHostError::Timeout`] when the signal is already cancelled or
/// fires during `work`.
pub async fn with_cancellation<T, F>(
    engine: &Engine,
    cancel: &CancellationSignal,
    deadline: Option<Timestamp>,
    work: F,
) -> Result<T, PluginHostError>
where
    F: Future<Output = Result<T, PluginHostError>>,
{
    if cancel.is_cancelled() || deadline_fired(deadline) {
        return Err(PluginHostError::Timeout);
    }
    tokio::select! {
        () = cancel.cancelled() => {
            engine.increment_epoch();
            Err(PluginHostError::Timeout)
        }
        () = sleep_until_deadline(deadline) => {
            engine.increment_epoch();
            Err(PluginHostError::Timeout)
        }
        result = work => match result {
            Err(_) if cancel.is_cancelled() || deadline_fired(deadline) => {
                Err(PluginHostError::Timeout)
            }
            other => other,
        },
    }
}

fn deadline_fired(deadline: Option<Timestamp>) -> bool {
    deadline.is_some_and(|deadline| unix_now_ms() >= deadline.as_unix_ms())
}

async fn sleep_until_deadline(deadline: Option<Timestamp>) {
    let Some(deadline) = deadline else {
        std::future::pending::<()>().await;
        return;
    };
    let remaining = deadline.as_unix_ms().saturating_sub(unix_now_ms());
    if remaining == 0 {
        return;
    }
    tokio::time::sleep(Duration::from_millis(remaining.cast_unsigned())).await;
}

fn unix_now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or_default(),
    )
    .unwrap_or(i64::MAX)
}

/// Instantiate `component` with host imports only. Unresolved `wasi:*` fails
/// closed as [`PluginHostError::InstantiateFailed`].
///
/// # Errors
///
/// Returns [`PluginHostError::InstantiateFailed`] when linking fails.
#[allow(dead_code)]
pub async fn instantiate_with_host_imports(
    engine: &Engine,
    linker: &Linker<HostState>,
    component: &Component,
) -> Result<Store<HostState>, PluginHostError> {
    let limits = EffectiveLimits::default();
    let mut store = new_store(engine, HostState::new(store_limits(limits)), limits.fuel)?;
    linker
        .instantiate_async(&mut store, component)
        .await
        .map_err(|error| map_wasmtime_error(&error, false))?;
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::{HostState, instantiate_with_host_imports, map_wasmtime_error, new_store};
    use crate::host::{InstancePolicy, PluginHost, PluginHostConfig};
    use wasmtime::component::Component;

    fn host() -> PluginHost {
        PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2).expect("cfg"),
        )
        .expect("host")
    }

    fn compile(host: &PluginHost, wat: &str) -> Component {
        let bytes = wat::parse_str(wat).expect("wat");
        Component::from_binary(host.engine(), &bytes).expect("component")
    }

    #[test]
    fn interrupt_maps_to_timeout_and_unreachable_to_trap() {
        let trap = wasmtime::Error::msg("wasm trap: wasm `unreachable` instruction executed");
        assert_eq!(map_wasmtime_error(&trap, false).code(), "plugin_trap");
        let interrupt = wasmtime::Error::msg("wasm trap: interrupt");
        assert_eq!(
            map_wasmtime_error(&interrupt, false).code(),
            "plugin_lifecycle_timeout"
        );
        assert_eq!(
            map_wasmtime_error(&wasmtime::Error::msg("other"), true).code(),
            "plugin_lifecycle_timeout"
        );
    }

    #[tokio::test]
    #[rustfmt::skip]
    async fn unresolved_wasi_import_fails_closed() {
        let host = host();
        let component = compile(
            &host,
            r#"
            (component
              (import "wasi:cli/exit@0.2.12" (instance
                (export "exit" (func (param "status" (option u32))))
              ))
            )
            "#,
        );
        let Err(error) =
            instantiate_with_host_imports(host.engine(), host.linker(), &component).await
        else {
            panic!("ungranted wasi:cli import must fail");
        };
        assert_eq!(error.code(), "plugin_instantiate_failed");
    }

    #[tokio::test]
    async fn guest_unreachable_is_contained() {
        let host = host();
        let component = compile(
            &host,
            r#"
            (component
              (core module $m
                (func (export "boom") unreachable)
              )
              (core instance $i (instantiate $m))
              (func (export "boom") (canon lift (core func $i "boom")))
            )
            "#,
        );
        let limits = crate::limits::EffectiveLimits::default();
        let mut store = new_store(
            host.engine(),
            HostState::new(crate::limits::store_limits(limits)),
            limits.fuel,
        )
        .expect("store");
        let instance = host
            .linker()
            .instantiate_async(&mut store, &component)
            .await
            .expect("instantiate");
        let func = instance
            .get_typed_func::<(), ()>(&mut store, "boom")
            .expect("export");
        let error = func.call_async(&mut store, ()).await.expect_err("trap");
        let mapped = map_wasmtime_error(&error, false);
        assert_eq!(mapped.code(), "plugin_trap", "trap mapping for {error}");
        let _other = new_store(
            host.engine(),
            HostState::new(crate::limits::store_limits(limits)),
            limits.fuel,
        )
        .expect("fresh store");
    }

    #[tokio::test]
    #[rustfmt::skip]
    async fn ungranted_filesystem_and_sockets_fail_closed() {
        let host = host();
        for wat in [
            r#"
            (component
              (import "wasi:filesystem/preopens@0.2.12" (instance
                (export "get-directories" (func (result u32)))
              ))
            )
            "#,
            r#"
            (component
              (import "wasi:sockets/instance-network@0.2.12" (instance
                (export "instance-network" (func (result u32)))
              ))
            )
            "#,
        ] {
            let component = compile(&host, wat);
            let Err(error) =
                instantiate_with_host_imports(host.engine(), host.linker(), &component).await
            else {
                panic!("ungranted wasi import must fail");
            };
            assert_eq!(error.code(), "plugin_instantiate_failed");
        }
    }

    #[tokio::test]
    async fn fuel_exhaustion_is_contained_and_host_continues() {
        let host = host();
        let component = compile(
            &host,
            r#"
            (component
              (core module $m
                (func (export "burn")
                  (loop $l (br $l)))
              )
              (core instance $i (instantiate $m))
              (func (export "burn") (canon lift (core func $i "burn")))
            )
            "#,
        );
        let limits = crate::limits::EffectiveLimits {
            fuel: 8_000,
            ..crate::limits::EffectiveLimits::default()
        };
        let mut store = new_store(
            host.engine(),
            HostState::new(crate::limits::store_limits(limits)),
            limits.fuel,
        )
        .expect("store");
        let instance = host
            .linker()
            .instantiate_async(&mut store, &component)
            .await
            .expect("instantiate");
        let func = instance
            .get_typed_func::<(), ()>(&mut store, "burn")
            .expect("export");
        let error = func.call_async(&mut store, ()).await.expect_err("fuel");
        let mapped = map_wasmtime_error(&error, false);
        assert_eq!(
            mapped.code(),
            "plugin_resource_limit",
            "fuel mapping for {error}"
        );
        let _fresh = new_store(
            host.engine(),
            HostState::new(crate::limits::store_limits(limits)),
            limits.fuel,
        )
        .expect("host continues");
    }

    #[tokio::test]
    async fn fixture_wats_fail_closed_without_grants() {
        let host = host();
        for name in ["wasi-fs-ungranted", "wasi-sockets-ungranted"] {
            let path = format!(
                "{}/fixtures/guests/{name}/component.wat",
                env!("CARGO_MANIFEST_DIR")
            );
            let wat =
                std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
            let component = compile(&host, &wat);
            let Err(error) =
                instantiate_with_host_imports(host.engine(), host.linker(), &component).await
            else {
                panic!("{name} must fail without a grant");
            };
            assert_eq!(error.code(), "plugin_instantiate_failed");
        }
    }

    #[tokio::test]
    #[rustfmt::skip]
    async fn filesystem_links_only_with_preopen() {
        use crate::grants::FilesystemPreopen;
        use std::collections::BTreeSet;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut grants = BTreeSet::new();
        grants.insert("filesystem".into());
        let host = crate::host::PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2)
                .expect("cfg")
                .with_application_grants(grants.clone())
                .expect("grants")
                .with_filesystem_preopens(vec![FilesystemPreopen {
                    guest_path: "/data".into(),
                    host_path: dir.path().to_path_buf(),
                    read: true,
                    write: false,
                }]),
        )
        .expect("host");
        let wat = r#"
            (component
              (import "wasi:filesystem/types@0.2.12" (instance $types
                (export "descriptor" (type (sub resource)))
              ))
              (alias export $types "descriptor" (type $descriptor))
              (import "wasi:filesystem/preopens@0.2.12" (instance
                (export "get-directories"
                  (func (result (list (tuple (own $descriptor) string)))))
              ))
            )
            "#;
        let component = compile(&host, wat);
        let linker = host
            .linker_for_world(crate::host::PluginWorld::Toolset, "0.0.4", &grants)
            .expect("link");
        instantiate_with_host_imports(host.engine(), &linker, &component)
            .await
            .expect("granted preopen links filesystem");

        let offered_only = crate::host::PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2)
                .expect("cfg")
                .with_application_grants(grants.clone())
                .expect("grants"),
        )
        .expect("host");
        let unlinked = offered_only
            .linker_for_world(crate::host::PluginWorld::Toolset, "0.0.4", &grants)
            .expect("link");
        let Err(error) =
            instantiate_with_host_imports(offered_only.engine(), &unlinked, &component).await
        else {
            panic!("filesystem name without preopen must stay unlinked");
        };
        assert_eq!(error.code(), "plugin_instantiate_failed");
    }

    #[tokio::test]
    async fn memory_grow_past_ceiling_is_contained() {
        let host = host();
        let component = compile(
            &host,
            r#"
            (component
              (core module $m
                (memory (export "mem") 1)
                (func (export "grow") (result i32)
                  (memory.grow (i32.const 4096)))
              )
              (core instance $i (instantiate $m))
              (func (export "grow") (result u32)
                (canon lift (core func $i "grow")))
            )
            "#,
        );
        let limits = crate::limits::EffectiveLimits {
            max_memory_bytes: 64 * 1024,
            ..crate::limits::EffectiveLimits::default()
        };
        let mut store = new_store(
            host.engine(),
            HostState::new(crate::limits::store_limits(limits)),
            limits.fuel,
        )
        .expect("store");
        let instance = host
            .linker()
            .instantiate_async(&mut store, &component)
            .await
            .expect("instantiate");
        let func = instance
            .get_typed_func::<(), (u32,)>(&mut store, "grow")
            .expect("export");
        match func.call_async(&mut store, ()).await {
            Ok((u32::MAX,)) => {}
            Err(error) => {
                assert_eq!(
                    map_wasmtime_error(&error, false).code(),
                    "plugin_resource_limit",
                    "memory mapping for {error}"
                );
            }
            Ok((grown,)) => panic!("grow past ceiling returned {grown}"),
        }
        let _fresh = new_store(
            host.engine(),
            HostState::new(crate::limits::store_limits(limits)),
            limits.fuel,
        )
        .expect("host continues");
    }
}
