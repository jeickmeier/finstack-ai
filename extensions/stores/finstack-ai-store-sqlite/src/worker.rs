//! Dedicated bounded sqlite worker: one thread, one connection, ordered jobs.

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, JoinHandle};

use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::journal::StoreError;
use finstack_ai_store_common::VerifiedHeadCache;
use rusqlite::Connection;

use crate::config::{SqliteStoreConfig, SqliteStoreLimits, is_memory_path};
use crate::error::map_sqlite_error;
use crate::schema::{apply_durability, apply_schema, quick_check};

/// In-flight jobs waiting for the single writer. Sized for the 64-session
/// concurrent bench plus headroom; overflow fails closed.
const WORKER_QUEUE_BOUND: usize = 128;

enum Work {
    Job(Box<dyn FnOnce(&mut WorkerCtx) + Send>),
    Shutdown,
}

/// Exclusive sqlite connection plus the process-local verification cache.
pub(crate) struct WorkerCtx {
    pub(crate) connection: Connection,
    pub(crate) limits: SqliteStoreLimits,
    /// Process-local chain-verification cache (spec D9), keyed by session.
    ///
    /// Owned outright rather than shared: the worker thread is the only
    /// accessor, so the container's generation guard is never contended. It
    /// is still the shared container so the cache semantics cannot drift
    /// from the other backends'.
    pub(crate) verified: VerifiedHeadCache,
}

/// Handle to the ordered worker. `Drop` sends shutdown and joins the thread.
pub(crate) struct WorkerHandle {
    tx: SyncSender<Work>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl WorkerHandle {
    pub(crate) fn spawn(config: SqliteStoreConfig) -> Result<Self, StoreError> {
        let (tx, rx) = sync_channel(WORKER_QUEUE_BOUND);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread = thread::Builder::new()
            .name("finstack-sqlite".to_owned())
            .spawn(move || match open_context(&config) {
                Ok(mut ctx) => {
                    let _ = ready_tx.send(Ok(()));
                    run(rx, &mut ctx);
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            })
            .map_err(|_| StoreError::Unavailable {
                reason_code: "sqlite_worker_spawn",
            })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                tx,
                thread: Mutex::new(Some(thread)),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(StoreError::Unavailable {
                    reason_code: "sqlite_worker_disconnected",
                })
            }
        }
    }

    pub(crate) fn submit<T, F>(&self, job: F) -> PortFuture<Result<T, StoreError>>
    where
        T: Send + 'static,
        F: FnOnce(&mut WorkerCtx) -> Result<T, StoreError> + Send + 'static,
    {
        let (reply, future) = oneshot();
        match self.enqueue(Box::new(move |ctx| {
            reply.send(run_job(job, ctx));
        })) {
            Ok(()) => Box::pin(future),
            Err(error) => Box::pin(core::future::ready(Err(error))),
        }
    }

    pub(crate) fn call<T, F>(&self, job: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut WorkerCtx) -> Result<T, StoreError> + Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel();
        self.enqueue(Box::new(move |ctx| {
            let _ = tx.send(run_job(job, ctx));
        }))?;
        rx.recv().map_err(|_| StoreError::Unavailable {
            reason_code: "sqlite_worker_disconnected",
        })?
    }

    fn enqueue(&self, job: Box<dyn FnOnce(&mut WorkerCtx) + Send>) -> Result<(), StoreError> {
        self.tx
            .try_send(Work::Job(job))
            .map_err(|error| match error {
                TrySendError::Full(_) => StoreError::Unavailable {
                    reason_code: "sqlite_worker_saturated",
                },
                TrySendError::Disconnected(_) => StoreError::Unavailable {
                    reason_code: "sqlite_worker_disconnected",
                },
            })
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(Work::Shutdown);
        if let Ok(mut thread) = self.thread.lock()
            && let Some(thread) = thread.take()
        {
            let _ = thread.join();
        }
    }
}

fn run_job<T, F>(job: F, ctx: &mut WorkerCtx) -> Result<T, StoreError>
where
    F: FnOnce(&mut WorkerCtx) -> Result<T, StoreError>,
{
    catch_unwind(AssertUnwindSafe(|| job(ctx))).unwrap_or(Err(StoreError::Unavailable {
        reason_code: "sqlite_worker_job_panic",
    }))
}

fn run(rx: Receiver<Work>, ctx: &mut WorkerCtx) {
    for work in rx {
        match work {
            Work::Job(job) => job(ctx),
            Work::Shutdown => break,
        }
    }
}

fn open_context(config: &SqliteStoreConfig) -> Result<WorkerCtx, StoreError> {
    let memory = is_memory_path(&config.path);
    let connection = if memory {
        Connection::open_in_memory().map_err(map_sqlite_error)?
    } else {
        Connection::open(&config.path).map_err(map_sqlite_error)?
    };
    connection
        .busy_timeout(config.busy_timeout)
        .map_err(map_sqlite_error)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(map_sqlite_error)?;
    apply_durability(&connection, config.durability, memory)?;
    apply_schema(&connection)?;
    quick_check(&connection)?;
    Ok(WorkerCtx {
        connection,
        limits: config.limits,
        verified: VerifiedHeadCache::new(),
    })
}

/// The reply slot shared by one job's sender and its awaiting future.
struct OneshotInner<T> {
    slot: Mutex<(Option<T>, Option<Waker>)>,
}

impl<T> OneshotInner<T> {
    fn lock(&self) -> std::sync::MutexGuard<'_, (Option<T>, Option<Waker>)> {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct OneshotSender<T> {
    inner: Arc<OneshotInner<T>>,
}

struct Oneshot<T> {
    inner: Arc<OneshotInner<T>>,
}

impl<T> OneshotSender<T> {
    fn send(self, value: T) {
        let waker = {
            let mut slot = self.inner.lock();
            slot.0 = Some(value);
            slot.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Future for Oneshot<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut slot = self.inner.lock();
        if let Some(value) = slot.0.take() {
            return Poll::Ready(value);
        }
        slot.1 = Some(cx.waker().clone());
        Poll::Pending
    }
}

fn oneshot<T>() -> (OneshotSender<T>, Oneshot<T>) {
    let inner = Arc::new(OneshotInner {
        slot: Mutex::new((None, None)),
    });
    (
        OneshotSender {
            inner: Arc::clone(&inner),
        },
        Oneshot { inner },
    )
}
