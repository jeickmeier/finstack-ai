//! Bound synchronous adapter work without occupying Tokio executor threads.
use crate::WorkerError;

static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(16);

pub(crate) async fn run<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, WorkerError> + Send + 'static,
) -> Result<T, WorkerError> {
    let permit = SLOTS.acquire().await.map_err(|_| unavailable())?;
    tokio::task::spawn_blocking(move || {
        // A dropped await does not cancel a database call or release its capacity.
        let _permit = permit;
        operation()
    })
    .await
    .map_err(|_| unavailable())?
}
fn unavailable() -> WorkerError {
    WorkerError::StoreIntegrity {
        code: "worker_blocking_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future::Future, task::Poll, time::Duration};

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_awaits_keep_blocking_capacity_until_calls_finish() {
        let mut unblock_senders = Vec::new();
        for _ in 0..16 {
            let (entered, entry) = tokio::sync::oneshot::channel();
            let (release, released) = std::sync::mpsc::channel();
            unblock_senders.push(release);
            let task = tokio::spawn(run(move || {
                entered.send(()).expect("entered");
                released
                    .recv_timeout(Duration::from_secs(30))
                    .expect("released");
                Ok(())
            }));
            entry.await.expect("blocking call entered");
            task.abort();
            assert!(task.await.expect_err("aborted await").is_cancelled());
        }
        assert_eq!(SLOTS.available_permits(), 0);
        let mut queued = Box::pin(run::<()>(|| {
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
        assert!(run::<()>(|| panic!("test panic")).await.is_err());
        assert!(run(|| Ok(())).await.is_ok(), "panic releases capacity");
    }
}
