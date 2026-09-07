//! Native executor boundary for synchronous experiment stores.
use crate::{EvalError, EvalStore};
use std::sync::Arc;

// Bound admitted blocking work across runners, including calls whose await was dropped.
static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(16);

#[derive(Clone)]
pub(crate) struct AsyncEvalStore(Arc<dyn EvalStore>);
impl AsyncEvalStore {
    pub(crate) fn new(store: Arc<dyn EvalStore>) -> Self {
        Self(store)
    }
    async fn call<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&dyn EvalStore) -> Result<T, EvalError> + Send + 'static,
    ) -> Result<T, EvalError> {
        let permit = SLOTS
            .acquire()
            .await
            .map_err(|_| crate::error::unavailable())?;
        let store = Arc::clone(&self.0);
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation(store.as_ref())
        })
        .await
        .map_err(|_| crate::error::unavailable())?
    }
    pub(crate) async fn acquire_runner(
        &self,
    ) -> Result<Box<dyn crate::store::RunnerLease>, EvalError> {
        self.call(move |store| store.acquire_runner()).await
    }
    pub(crate) async fn snapshot(&self) -> Result<crate::StoreSnapshot, EvalError> {
        self.call(move |store| store.snapshot()).await
    }
    pub(crate) async fn freeze(
        &self,
        spec: &crate::EvalSpec,
    ) -> Result<crate::FrozenExperiment, EvalError> {
        let spec = spec.to_owned();
        self.call(move |store| store.freeze(&spec)).await
    }
    pub(crate) async fn bind_subject(
        &self,
        subject: &str,
        digest: finstack_ai_kernel::Digest,
    ) -> Result<(), EvalError> {
        let subject = subject.to_owned();
        self.call(move |store| store.bind_subject(&subject, digest))
            .await
    }
    pub(crate) async fn reserve(
        &self,
        cell: &crate::Cell,
        sequence: u32,
        started_at_ms: u64,
    ) -> Result<crate::AttemptReservation, EvalError> {
        let cell = cell.to_owned();
        self.call(move |store| store.reserve(&cell, sequence, started_at_ms))
            .await
    }
    pub(crate) async fn bind_execution(
        &self,
        cell: &str,
        sequence: u32,
        identity: crate::ExecutionIdentity,
    ) -> Result<(), EvalError> {
        let cell = cell.to_owned();
        self.call(move |store| store.bind_execution(&cell, sequence, identity))
            .await
    }
    pub(crate) async fn settle(&self, record: &crate::AttemptRecord) -> Result<(), EvalError> {
        let record = record.to_owned();
        self.call(move |store| store.settle(&record)).await
    }
    pub(crate) async fn append_scores(
        &self,
        cell: &str,
        sequence: u32,
        scores: &crate::ScoreSet,
    ) -> Result<(), EvalError> {
        let cell = cell.to_owned();
        let scores = scores.to_owned();
        self.call(move |store| store.append_scores(&cell, sequence, &scores))
            .await
    }
    pub(crate) async fn reserve_grader(
        &self,
        reservation: &crate::GraderReservation,
    ) -> Result<(), EvalError> {
        let reservation = reservation.to_owned();
        self.call(move |store| store.reserve_grader(&reservation))
            .await
    }
    pub(crate) async fn bind_grader_execution(
        &self,
        key: &str,
        identity: crate::ExecutionIdentity,
    ) -> Result<(), EvalError> {
        let key = key.to_owned();
        self.call(move |store| store.bind_grader_execution(&key, identity))
            .await
    }
    pub(crate) async fn settle_grader(
        &self,
        key: &str,
        outcome: &crate::GraderOutcome,
    ) -> Result<(), EvalError> {
        let key = key.to_owned();
        let outcome = outcome.to_owned();
        self.call(move |store| store.settle_grader(&key, &outcome))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future::Future, task::Poll, time::Duration};

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_awaits_keep_store_capacity_until_calls_finish() {
        let store = AsyncEvalStore::new(Arc::new(crate::MemoryEvalStore::new()));
        let mut unblock_senders = Vec::new();
        for _ in 0..16 {
            let (entered, entry) = tokio::sync::oneshot::channel();
            let (release, released) = std::sync::mpsc::channel();
            unblock_senders.push(release);
            let owned = store.clone();
            let task = tokio::spawn(async move {
                owned
                    .call(move |_| {
                        entered.send(()).expect("entered");
                        released
                            .recv_timeout(Duration::from_secs(30))
                            .expect("released");
                        Ok(())
                    })
                    .await
            });
            entry.await.expect("blocking call entered");
            task.abort();
            assert!(task.await.expect_err("aborted await").is_cancelled());
        }
        assert_eq!(SLOTS.available_permits(), 0);
        let mut queued = Box::pin(store.call(|_| -> Result<(), EvalError> {
            panic!("cancelled queued call must never execute")
        }));
        std::future::poll_fn(|cx| {
            assert!(queued.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(queued);
        for release in unblock_senders {
            release.send(()).expect("release blocking call");
        }
        let permits = SLOTS.acquire_many(16).await.expect("all calls finished");
        drop(permits);
        assert!(store.call::<()>(|_| panic!("test panic")).await.is_err());
        assert!(store.snapshot().await.is_ok(), "panic releases capacity");
    }
}
