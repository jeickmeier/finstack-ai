//! Shared subject/grader driving and cancellation ownership.
use finstack_ai::{Agent, AgentRunRequest, Lane};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{sync::Notify, time::Instant};

#[derive(Clone, Default)]
pub(crate) struct Cancellation(Arc<Control>);
#[derive(Default)]
struct Control {
    cancelled: AtomicBool,
    changed: Notify,
}
impl Cancellation {
    pub(crate) fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.0.changed.notify_waiters();
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }
    pub(crate) async fn wait(&self) {
        loop {
            let changed = self.0.changed.notified();
            if self.is_cancelled() {
                return;
            }
            changed.await;
        }
    }
}
/// The caller has already persisted this lane's identity before entering here.
pub(crate) async fn drive(
    lane: &Lane,
    agent: &Agent,
    mut request: AgentRunRequest,
    deadline: Instant,
    cancellation: &Cancellation,
) {
    request.timeout = request
        .timeout
        .min(deadline.saturating_duration_since(Instant::now()));
    if cancellation.is_cancelled() || request.timeout.is_zero() {
        return;
    }
    let Ok(run) = lane.run(agent, request) else {
        return;
    };
    run.close_events();
    let finished = tokio::select! {
        _ = run.result() => true,
        () = cancellation.wait() => false,
        () = tokio::time::sleep_until(deadline) => false,
    };
    if !finished {
        // A bounded explicit cancellation attempt is followed by journal
        // reconciliation. Failed/uncertain settlement never implies no admission.
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            let _ = run.cancel().await;
            let _ = run.result().await;
        })
        .await;
    }
}
