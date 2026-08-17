//! Native driver utilities used by the SDK facade without exposing Tokio
//! types in its public API.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::PortFuture;

/// No native runtime is active for a requested driver operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriverUnavailable;

impl std::fmt::Display for DriverUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("native driver is unavailable")
    }
}

impl std::error::Error for DriverUnavailable {}

/// The native driver deadline elapsed before the future completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeoutElapsed;

impl std::fmt::Display for TimeoutElapsed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("native driver deadline elapsed")
    }
}

impl std::error::Error for TimeoutElapsed {}

/// Cloneable one-way notification used by native SDK state machines.
#[derive(Clone, Default)]
pub struct Signal {
    inner: Arc<tokio::sync::Notify>,
}

impl Signal {
    /// Create an empty notification signal.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a waiter before checking its guarded state.
    pub fn notified(&self) -> impl Future<Output = ()> + '_ {
        self.inner.notified()
    }

    /// Wake every waiter registered before this call.
    pub fn notify_waiters(&self) {
        self.inner.notify_waiters();
    }
}

/// Await a future until the native driver deadline elapses.
///
/// # Errors
///
/// Returns [`TimeoutElapsed`] when the future does not complete before the
/// requested duration.
pub async fn timeout<F>(duration: Duration, future: F) -> Result<F::Output, TimeoutElapsed>
where
    F: Future,
{
    tokio::time::timeout(duration, future)
        .await
        .map_err(|_| TimeoutElapsed)
}

/// Run one blocking function on the native driver's blocking executor.
///
/// This keeps synchronous extension code off runtime worker threads while
/// avoiding a dependency on a guest-language executor.
///
/// # Errors
///
/// Returns [`DriverUnavailable`] when no native runtime is active or when
/// the blocking task cannot be joined.
pub async fn run_blocking<F, R>(function: F) -> Result<R, DriverUnavailable>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let handle = tokio::runtime::Handle::try_current().map_err(|_| DriverUnavailable)?;
    handle
        .spawn_blocking(function)
        .await
        .map_err(|_| DriverUnavailable)
}

/// Spawn one detached SDK driver future on the active native runtime.
///
/// # Errors
///
/// Returns [`DriverUnavailable`] when the caller is not inside the native
/// runtime context.
pub fn spawn(future: PortFuture<()>) -> Result<(), DriverUnavailable> {
    let handle = tokio::runtime::Handle::try_current().map_err(|_| DriverUnavailable)?;
    handle.spawn(future);
    Ok(())
}

/// Cooperatively yield one turn to the native driver.
pub async fn yield_now() {
    tokio::task::yield_now().await;
}
