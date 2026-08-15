//! Host-import linking, instantiation, trap mapping, and cancellation.

use std::future::Future;

use finstack_ai_runtime::CancellationSignal;
use finstack_ai_wit::{MAX_STRING_BYTES, reject_before_allocation, reject_declared_len};
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};

use crate::bindings::toolset::finstack::ai_host::blobs;
use crate::bindings::toolset::finstack::ai_host::logging::{self, Level};
use crate::bindings::toolset::finstack::ai_types::types::{
    self as types, BlobRef, CallContext, PluginError,
};
use crate::error::PluginHostError;

/// Send + Sync host-import state. This is not the in-process `RecordingLogger`
/// (`RefCell`) from `finstack-ai-wit`.
#[derive(Debug, Default)]
pub struct HostState {
    logs: Vec<(String, String)>,
    blob: Vec<u8>,
}

impl HostState {
    /// Construct empty host import state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct state that can serve one blob body.
    #[must_use]
    pub fn with_blob(bytes: Vec<u8>) -> Self {
        Self {
            logs: Vec::new(),
            blob: bytes,
        }
    }

    /// Recorded log lines for diagnostics.
    #[must_use]
    pub fn logs(&self) -> &[(String, String)] {
        &self.logs
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

/// Map a Wasmtime failure onto a stable plugin code.
#[must_use]
pub fn map_wasmtime_error(error: &wasmtime::Error, cancelled: bool) -> PluginHostError {
    if cancelled {
        return PluginHostError::Timeout;
    }
    if let Some(trap) = trap_code(error) {
        return if matches!(trap, wasmtime::Trap::Interrupt) {
            PluginHostError::Timeout
        } else {
            PluginHostError::Trap(error.to_string())
        };
    }
    let message = error.to_string();
    if is_interrupt(&message) {
        return PluginHostError::Timeout;
    }
    if is_trap(&message) {
        return PluginHostError::Trap(message);
    }
    PluginHostError::InstantiateFailed(message)
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

/// Prepare a store that honors epoch interruption as the cancellation channel.
#[must_use]
pub fn new_store(engine: &Engine, state: HostState) -> Store<HostState> {
    let mut store = Store::new(engine, state);
    store.set_epoch_deadline(1);
    store
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
    work: F,
) -> Result<T, PluginHostError>
where
    F: Future<Output = Result<T, PluginHostError>>,
{
    if cancel.is_cancelled() {
        return Err(PluginHostError::Timeout);
    }
    tokio::select! {
        () = cancel.cancelled() => {
            engine.increment_epoch();
            Err(PluginHostError::Timeout)
        }
        result = work => match result {
            Err(_) if cancel.is_cancelled() => Err(PluginHostError::Timeout),
            other => other,
        },
    }
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
    let mut store = new_store(engine, HostState::new());
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
    async fn unresolved_wasi_import_fails_closed() {
        let host = host();
        let component = compile(
            &host,
            r#"
            (component
              (import "wasi:cli/exit@0.2.0" (instance
                (export "exit" (func (param "status" (option u32))))
              ))
            )
            "#,
        );
        let error = instantiate_with_host_imports(host.engine(), host.linker(), &component)
            .await
            .expect_err("wasi");
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
        let mut store = new_store(host.engine(), HostState::new());
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
        let _other = new_store(host.engine(), HostState::new());
    }
}
