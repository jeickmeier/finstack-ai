//! Host-driven local executor for browser `PortFuture` values.
//!
//! The runtime `wasm-host` feature only selects local port aliases. Polling
//! and `spawn_local` live in this binding crate so the runtime graph stays
//! free of wasm-bindgen.

#[cfg(any(target_arch = "wasm32", test))]
use finstack_ai::runtime::ports::PortFuture;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::{future_to_promise, spawn_local};

/// Drive a local fallible future onto a JavaScript `Promise`.
#[cfg(target_arch = "wasm32")]
#[must_use]
pub fn drive(
    future: impl core::future::Future<Output = Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>>
    + 'static,
) -> js_sys::Promise {
    future_to_promise(future)
}

/// Spawn a local port future on the browser event loop.
#[cfg(target_arch = "wasm32")]
pub fn spawn_port_future(future: PortFuture<()>) {
    spawn_local(future);
}

/// Poll an immediately ready native port future to completion.
///
/// Compile fixtures return ready futures. This helper does not introduce a
/// runtime or timer source.
///
/// # Panics
///
/// Panics when the future is pending. Compile fixtures must be immediately ready.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[must_use]
pub fn block_on_ready<T>(future: PortFuture<T>) -> T {
    use core::task::{Context, Poll, Waker};

    let mut future = future;
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("native compile fixture future was not ready"),
    }
}
