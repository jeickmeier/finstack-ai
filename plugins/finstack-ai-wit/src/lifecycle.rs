//! Host-side plugin initialize, optional warmup, health, and shutdown.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use finstack_ai::AgentConstructionContext;
use finstack_ai::registry::{
    ComponentConstructionContext, ComponentHealth, ComponentLifecycle, LifecycleError,
};
use finstack_ai_kernel::Timestamp;
use finstack_ai_runtime::ports::PortFuture;
use thiserror::Error;

/// Stable plugin lifecycle failure. Wrap into [`LifecycleError::failed`] at the SDK boundary.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PluginLifecycleError {
    /// Host-side initialize failed before the first collect or list-tools.
    #[error("plugin_initialize_failed: {0}")]
    InitializeFailed(&'static str),
    /// Optional host-side warmup failed.
    #[error("plugin_warmup_failed: {0}")]
    WarmupFailed(&'static str),
    /// Construction cancellation or deadline fired.
    #[error("plugin_lifecycle_timeout")]
    Timeout,
    /// Health or shutdown failed after the component was ready.
    #[error("plugin_lifecycle_failed: {0}")]
    Failed(&'static str),
}

impl PluginLifecycleError {
    /// Stable diagnostic code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InitializeFailed(_) => "plugin_initialize_failed",
            Self::WarmupFailed(_) => "plugin_warmup_failed",
            Self::Timeout => "plugin_lifecycle_timeout",
            Self::Failed(_) => "plugin_lifecycle_failed",
        }
    }

    /// Wrap the plugin code into the public SDK lifecycle error.
    #[must_use]
    pub fn into_lifecycle_error(self) -> LifecycleError {
        LifecycleError::failed(self.to_string())
    }
}

/// Optional host-side initialize and warmup hooks. Defaults skip warmup work.
pub trait PluginGuestHooks: Send + Sync + 'static {
    /// Run once before the first collect or list-tools.
    ///
    /// # Errors
    ///
    /// Returns [`PluginLifecycleError`] when initialize fails or times out.
    fn initialize(
        &self,
        context: &ComponentConstructionContext,
    ) -> Result<(), PluginLifecycleError> {
        honor_deadline(context)?;
        Ok(())
    }

    /// Optional once-at-construction warmup. The default skips.
    ///
    /// # Errors
    ///
    /// Returns [`PluginLifecycleError`] when warmup fails or times out.
    fn warmup(&self, context: &ComponentConstructionContext) -> Result<(), PluginLifecycleError> {
        honor_deadline(context)?;
        Ok(())
    }
}

/// Host-owned lifecycle state attached through [`finstack_ai::registry::LifecycleBinding`].
pub struct PluginLifecycle {
    hooks: Arc<dyn PluginGuestHooks>,
    initialized: AtomicBool,
    shutdown: AtomicBool,
}

impl PluginLifecycle {
    /// Construct idle lifecycle state.
    #[must_use]
    pub fn new(hooks: Arc<dyn PluginGuestHooks>) -> Self {
        Self {
            hooks,
            initialized: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        }
    }

    /// Run initialize then optional warmup exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`PluginLifecycleError`] when a hook fails or the deadline fires.
    pub fn run_construction(
        &self,
        context: &ComponentConstructionContext,
    ) -> Result<(), PluginLifecycleError> {
        honor_deadline(context)?;
        if self
            .initialized
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let outcome = self
                .hooks
                .initialize(context)
                .and_then(|()| self.hooks.warmup(context));
            if outcome.is_err() {
                // A failed construction must not leave the component
                // reporting healthy; release the claim so a retry can run.
                self.initialized.store(false, Ordering::Release);
            }
            return outcome;
        }
        Ok(())
    }
}

impl ComponentLifecycle for PluginLifecycle {
    fn health(&self) -> PortFuture<Result<ComponentHealth, LifecycleError>> {
        let ready =
            self.initialized.load(Ordering::Acquire) && !self.shutdown.load(Ordering::Acquire);
        Box::pin(async move {
            Ok(ComponentHealth {
                ready,
                detail: Arc::from(if ready {
                    "plugin ready"
                } else {
                    "plugin not ready"
                }),
            })
        })
    }

    fn shutdown(
        &self,
        context: AgentConstructionContext,
    ) -> PortFuture<Result<(), LifecycleError>> {
        if context.cancellation.is_cancelled() {
            return Box::pin(async { Err(PluginLifecycleError::Timeout.into_lifecycle_error()) });
        }
        self.shutdown.store(true, Ordering::Release);
        Box::pin(async { Ok(()) })
    }
}

/// Reject a fired construction deadline or cancellation.
///
/// # Errors
///
/// Returns [`PluginLifecycleError::Timeout`] when the host cancelled the signal.
pub fn honor_deadline(context: &ComponentConstructionContext) -> Result<(), PluginLifecycleError> {
    if context.cancellation.is_cancelled() {
        return Err(PluginLifecycleError::Timeout);
    }
    if let Some(deadline) = context.deadline
        && unix_now_ms() >= deadline.as_unix_ms()
    {
        return Err(PluginLifecycleError::Timeout);
    }
    Ok(())
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

/// Build an absolute deadline from `now + timeout_ms`, keeping the earlier of
/// that instant and `incoming` when both exist.
#[must_use]
pub fn merge_call_deadline(incoming: Option<Timestamp>, timeout_ms: u64) -> Option<Timestamp> {
    let from_timeout = unix_now_ms()
        .checked_add(i64::try_from(timeout_ms).unwrap_or(i64::MAX))
        .and_then(|ms| Timestamp::from_unix_ms(ms).ok());
    match (incoming, from_timeout) {
        (Some(left), Some(right)) if left.as_unix_ms() <= right.as_unix_ms() => Some(left),
        (incoming, from_timeout) => from_timeout.or(incoming),
    }
}

/// No-op hooks used by the in-process reference guests.
#[derive(Debug, Default)]
pub struct NoopPluginHooks;

impl PluginGuestHooks for NoopPluginHooks {}

#[cfg(test)]
mod tests {
    use super::{
        NoopPluginHooks, PluginGuestHooks, PluginLifecycle, PluginLifecycleError, honor_deadline,
    };
    use finstack_ai::AgentConstructionContext;
    use finstack_ai::registry::{ComponentConstructionContext, ComponentLifecycle};
    use finstack_ai_kernel::{ComponentId, ComponentRef, Metadata, Timestamp, Version};
    use finstack_ai_runtime::ports::model::CancellationSignal;
    use std::sync::Arc;

    fn construction(cancelled: bool) -> ComponentConstructionContext {
        let cancellation = CancellationSignal::new();
        if cancelled {
            cancellation.cancel();
        }
        ComponentConstructionContext {
            component: ComponentRef::new(
                ComponentId::parse("finstack.plugin.reference.context").expect("id"),
                Some(Version {
                    major: 0,
                    minor: 0,
                    patch: 4,
                }),
            ),
            configuration: None,
            cancellation,
            deadline: None,
            metadata: Metadata::empty(),
        }
    }

    struct FailingInit;

    impl PluginGuestHooks for FailingInit {
        fn initialize(
            &self,
            context: &ComponentConstructionContext,
        ) -> Result<(), PluginLifecycleError> {
            honor_deadline(context)?;
            Err(PluginLifecycleError::InitializeFailed("boom"))
        }
    }

    struct FailingWarmup;

    impl PluginGuestHooks for FailingWarmup {
        fn warmup(
            &self,
            context: &ComponentConstructionContext,
        ) -> Result<(), PluginLifecycleError> {
            honor_deadline(context)?;
            Err(PluginLifecycleError::WarmupFailed("warm"))
        }
    }

    #[test]
    fn timeout_and_failure_codes_are_stable() {
        assert_eq!(
            honor_deadline(&construction(true))
                .expect_err("timeout")
                .code(),
            "plugin_lifecycle_timeout"
        );
        let mut expired = construction(false);
        expired.deadline = Some(Timestamp::from_unix_ms(1).expect("past"));
        assert_eq!(
            honor_deadline(&expired).expect_err("deadline").code(),
            "plugin_lifecycle_timeout"
        );
        let init = PluginLifecycle::new(Arc::new(FailingInit));
        assert_eq!(
            init.run_construction(&construction(false))
                .expect_err("init")
                .code(),
            "plugin_initialize_failed"
        );
        let warmup = PluginLifecycle::new(Arc::new(FailingWarmup));
        assert_eq!(
            warmup
                .run_construction(&construction(false))
                .expect_err("warmup")
                .code(),
            "plugin_warmup_failed"
        );
        let wrapped = PluginLifecycleError::Failed("late").into_lifecycle_error();
        assert_eq!(wrapped.code(), "component_lifecycle_failed");
        assert!(wrapped.message().contains("plugin_lifecycle_failed"));
    }

    #[tokio::test]
    async fn a_failed_construction_does_not_report_ready() {
        let failing = PluginLifecycle::new(Arc::new(FailingInit));
        assert!(
            failing.run_construction(&construction(false)).is_err(),
            "construction fails"
        );
        let health = failing.health().await.expect("health");
        assert!(!health.ready, "failed construction must not report ready");

        // A retry after the failure may run hooks again.
        let retried = PluginLifecycle::new(Arc::new(NoopPluginHooks));
        assert!(retried.run_construction(&construction(false)).is_ok());
        let health = retried.health().await.expect("health");
        assert!(health.ready);
    }

    #[tokio::test]
    async fn shutdown_is_idempotent() {
        let lifecycle = PluginLifecycle::new(Arc::new(NoopPluginHooks));
        lifecycle
            .run_construction(&construction(false))
            .expect("init");
        let health = lifecycle.health().await.expect("health");
        assert!(health.ready);
        lifecycle
            .shutdown(AgentConstructionContext::new())
            .await
            .expect("first");
        lifecycle
            .shutdown(AgentConstructionContext::new())
            .await
            .expect("second");
        let cancelled = AgentConstructionContext {
            cancellation: {
                let signal = CancellationSignal::new();
                signal.cancel();
                signal
            },
            deadline: None,
            metadata: Metadata::empty(),
        };
        let error = lifecycle.shutdown(cancelled).await.expect_err("timeout");
        assert!(error.message().contains("plugin_lifecycle_timeout"));
    }
}
