//! Private durable semantic-timer execution.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{EffectId, PostCommitAction, TimerFiredInput, Timestamp};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::coordinator::{DispatchError, PostCommitDispatcher, RuntimeDispatch, TimerDispatchSeed};
use crate::{CancellationSignal, Clock, DeadlineDiagnostic, MonotonicDeadline, PortFuture};

pub(crate) struct TimerJob {
    seed: TimerDispatchSeed,
    wait: MonotonicDeadline,
    cancellation: CancellationSignal,
}

pub(crate) struct TimerDriverResult {
    pub(crate) input: TimerFiredInput,
    pub(crate) diagnostic: DeadlineDiagnostic,
}

pub(crate) enum TimerDriverMessage {
    Fired(TimerDriverResult),
    Failed,
}

type ActiveTimers = Arc<Mutex<BTreeMap<EffectId, CancellationSignal>>>;

pub(crate) struct TimerDispatcher<C> {
    clock: Arc<C>,
    jobs: mpsc::Sender<TimerJob>,
    active: ActiveTimers,
    parent: CancellationSignal,
}

impl<C: Clock> TimerDispatcher<C> {
    pub(crate) fn new(
        clock: Arc<C>,
        jobs: mpsc::Sender<TimerJob>,
        parent: CancellationSignal,
    ) -> Self {
        Self {
            clock,
            jobs,
            active: Arc::new(Mutex::new(BTreeMap::new())),
            parent,
        }
    }

    pub(crate) async fn resume(&self, seed: TimerDispatchSeed) -> Result<(), DispatchError> {
        self.enqueue(seed).await
    }

    pub(crate) fn active(&self) -> ActiveTimers {
        Arc::clone(&self.active)
    }

    async fn enqueue(&self, seed: TimerDispatchSeed) -> Result<(), DispatchError> {
        let effect_id = seed.scheduled.timer_effect_id;
        let wait = MonotonicDeadline::from_persisted(
            self.clock.as_ref(),
            seed.scheduled_at,
            seed.scheduled.due_at,
        )
        .map_err(|_| DispatchError {
            code: "timer_deadline_invalid",
        })?;
        let cancellation = self.parent.child();
        {
            let mut active = self.active.lock().map_err(|_| DispatchError {
                code: "timer_registry_unavailable",
            })?;
            if active.insert(effect_id, cancellation.clone()).is_some() {
                return Err(DispatchError {
                    code: "timer_already_active",
                });
            }
        }
        if self
            .jobs
            .send(TimerJob {
                seed,
                wait,
                cancellation,
            })
            .await
            .is_err()
        {
            if let Ok(mut active) = self.active.lock() {
                active.remove(&effect_id);
            }
            return Err(DispatchError {
                code: "timer_job_queue_closed",
            });
        }
        Ok(())
    }
}

impl<C> PostCommitDispatcher for TimerDispatcher<C>
where
    C: Clock + Send + Sync + 'static,
{
    fn dispatch(&self, dispatch: RuntimeDispatch) -> PortFuture<Result<(), DispatchError>> {
        match dispatch.action {
            PostCommitAction::CancelEffect { effect_id } => {
                let cancellation = self
                    .active
                    .lock()
                    .ok()
                    .and_then(|active| active.get(&effect_id).cloned());
                Box::pin(async move {
                    if let Some(cancellation) = cancellation {
                        cancellation.cancel();
                    }
                    Ok(())
                })
            }
            PostCommitAction::ExecuteEffect { .. } => {
                let Some(seed) = dispatch.timer else {
                    return Box::pin(async {
                        Err(DispatchError {
                            code: "unsupported_effect_driver",
                        })
                    });
                };
                let clock = Arc::clone(&self.clock);
                let jobs = self.jobs.clone();
                let active = Arc::clone(&self.active);
                let parent = self.parent.clone();
                Box::pin(async move {
                    let dispatcher = Self {
                        clock,
                        jobs,
                        active,
                        parent,
                    };
                    dispatcher.enqueue(seed).await
                })
            }
        }
    }
}

pub(crate) async fn run_timer_jobs<C>(
    clock: Arc<C>,
    active: ActiveTimers,
    mut jobs: mpsc::Receiver<TimerJob>,
    results: mpsc::Sender<TimerDriverMessage>,
) where
    C: Clock + Send + Sync + 'static,
{
    let mut tasks = JoinSet::new();
    let mut intake_open = true;
    while intake_open || !tasks.is_empty() {
        tokio::select! {
            job = jobs.recv(), if intake_open => {
                if let Some(job) = job {
                    let clock = Arc::clone(&clock);
                    let active = Arc::clone(&active);
                    let results = results.clone();
                    tasks.spawn(async move {
                        let effect_id = job.seed.scheduled.timer_effect_id;
                        tokio::select! {
                            biased;
                            () = job.cancellation.cancelled() => {}
                            () = job.wait.wait() => {
                                let message = match clock.now() {
                                    Ok(observed) => {
                                        let due_at = job.seed.scheduled.due_at;
                                        let fired_at = maximum_timestamp(observed, due_at);
                                        TimerDriverMessage::Fired(TimerDriverResult {
                                            input: TimerFiredInput {
                                                effect_id,
                                                due_at,
                                                fired_at,
                                            },
                                            diagnostic: job.wait.diagnostic(),
                                        })
                                    }
                                    Err(_) => TimerDriverMessage::Failed,
                                };
                                let _ = results.send(message).await;
                            }
                        }
                        if let Ok(mut values) = active.lock() {
                            values.remove(&effect_id);
                        }
                    });
                } else {
                    intake_open = false;
                    if let Ok(values) = active.lock() {
                        for cancellation in values.values() {
                            cancellation.cancel();
                        }
                    }
                }
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}

fn maximum_timestamp(left: Timestamp, right: Timestamp) -> Timestamp {
    if left < right { right } else { left }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::time::Duration as StdDuration;

    use finstack_ai_kernel::{
        ErrorCategory, ErrorDescriptor, Id, RetryClassification, RetryScheduled,
    };

    use super::*;
    use crate::IdGenerationError;

    struct MutableClock(AtomicI64);

    impl Clock for MutableClock {
        fn now(&self) -> Result<Timestamp, IdGenerationError> {
            Timestamp::from_unix_ms(self.0.load(Ordering::Acquire)).map_err(IdGenerationError::Time)
        }
    }

    fn effect_id(ordinal: u64) -> EffectId {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    fn retry(attempt: u32) -> RetryScheduled {
        RetryScheduled::try_new(
            0,
            attempt,
            RetryClassification::Model,
            "retry-v1",
            effect_id(1),
            Timestamp::from_unix_ms(1_500).expect("due"),
            ErrorDescriptor::new("temporary", "temporary failure", ErrorCategory::Model, true)
                .expect("error"),
        )
        .expect("retry")
    }

    #[tokio::test(start_paused = true)]
    async fn restart_recomputes_overdue_timer_once_from_persisted_attempt() {
        let clock = Arc::new(MutableClock(AtomicI64::new(2_000)));
        let parent = CancellationSignal::new();
        let (jobs, job_receiver) = mpsc::channel(2);
        let (results, mut result_receiver) = mpsc::channel(2);
        let dispatcher = TimerDispatcher::new(Arc::clone(&clock), jobs, parent.clone());
        let active = dispatcher.active();
        let worker = tokio::spawn(run_timer_jobs(
            Arc::clone(&clock),
            active,
            job_receiver,
            results,
        ));
        let scheduled = retry(2);
        assert_eq!(scheduled.attempt, 2);
        dispatcher
            .resume(TimerDispatchSeed {
                scheduled: scheduled.clone(),
                scheduled_at: Timestamp::from_unix_ms(1_000).expect("scheduled"),
            })
            .await
            .expect("resume");
        tokio::task::yield_now().await;
        let TimerDriverMessage::Fired(fired) = result_receiver.recv().await.expect("fired") else {
            panic!("timer failed");
        };
        assert_eq!(fired.input.effect_id, scheduled.timer_effect_id);
        assert_eq!(fired.input.due_at, scheduled.due_at);
        assert_eq!(
            fired.input.fired_at,
            Timestamp::from_unix_ms(2_000).expect("now")
        );
        assert_eq!(fired.diagnostic, DeadlineDiagnostic::AlreadyDue);
        assert!(
            tokio::time::timeout(StdDuration::ZERO, result_receiver.recv())
                .await
                .is_err()
        );

        parent.cancel();
        drop(dispatcher);
        worker.await.expect("worker");
    }
}
