//! Small bounded connection pool: semaphore-limited, lazy-connect,
//! discard-on-error (spec D2).
//!
//! Deliberately hand-rolled rather than a `deadpool` dependency: a
//! `tokio::sync::Semaphore` bounds the number of physical connections ever
//! created to `pool_size`, and a plain `std::sync::Mutex<Vec<C>>` holds idle
//! connections (a `std::sync::Mutex` rather than `tokio::sync::Mutex`
//! because the critical sections here are pure, non-`await`-ing
//! push/pop — using it lets [`PooledClient::drop`] return a connection to
//! the pool synchronously, which an async mutex cannot do from `Drop`).
//!
//! [`Pool`] is generic over the pooled connection type `C` purely so the
//! bookkeeping (semaphore + idle queue + discard-on-drop) can be unit
//! tested without a live server: production code only ever instantiates
//! `Pool<tokio_postgres::Client>` (see `src/store.rs`).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use finstack_ai_runtime::StoreError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// A connection-factory future, boxed so it can be stored as a field.
type ConnectFuture<C> = Pin<Box<dyn Future<Output = Result<C, StoreError>> + Send>>;

/// A connection factory: called whenever the pool needs a new physical
/// connection (idle queue empty, or exhausted by dead connections, on
/// checkout).
type ConnectFn<C> = Box<dyn Fn() -> ConnectFuture<C> + Send + Sync>;

/// A liveness check, called on every idle connection popped during
/// checkout before it is handed out (see [`Pool::get`]).
type IsAliveFn<C> = Box<dyn Fn(&C) -> bool + Send + Sync>;

/// Shared pool state, held behind an `Arc` so [`PooledClient`] can return
/// its connection on drop without borrowing from [`Pool`].
struct PoolInner<C> {
    /// Bounds the number of physical connections ever open at once to the
    /// configured pool size (permits are held only while a connection is
    /// checked out or newly connecting — an idle, un-checked-out connection
    /// does not hold a permit, see [`Pool::get`]).
    semaphore: Arc<Semaphore>,
    /// Idle, ready-to-use connections.
    idle: StdMutex<Vec<C>>,
    /// Lazily creates a new physical connection.
    connect: ConnectFn<C>,
    /// Checks whether an idle connection is still usable. A connection can
    /// die while sitting idle (server restart, network drop) with nothing
    /// else noticing; [`Pool::get`] runs this on every idle connection it
    /// pops and silently drops dead ones instead of handing them out.
    is_alive: IsAliveFn<C>,
}

/// A small bounded pool of connections of type `C`.
///
/// Cheaply [`Clone`] (an `Arc` clone) so it can be captured by `'static`
/// futures returned from [`finstack_ai_runtime::JournalStore`] methods.
pub(crate) struct Pool<C> {
    inner: Arc<PoolInner<C>>,
}

impl<C> Clone for Pool<C> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<C: Send + 'static> Pool<C> {
    /// Construct a pool bounded to `size` concurrent connections, using
    /// `connect` to lazily create new ones and `is_alive` to check an idle
    /// connection's liveness before handing it out.
    pub(crate) fn new(size: usize, connect: ConnectFn<C>, is_alive: IsAliveFn<C>) -> Self {
        Self {
            inner: Arc::new(PoolInner {
                semaphore: Arc::new(Semaphore::new(size)),
                idle: StdMutex::new(Vec::new()),
                connect,
                is_alive,
            }),
        }
    }

    /// Seed the pool with an already-open connection (used by
    /// [`crate::store::PostgresJournalStore::try_open`] to hand the client
    /// it used for schema setup straight into the pool rather than opening
    /// and discarding an extra connection).
    ///
    /// Deliberately does not consume a semaphore permit: the first
    /// [`Pool::get`] call will find this connection in the idle queue and
    /// reuse it before ever calling `connect`, so at most `size - 1`
    /// additional physical connections are ever opened.
    pub(crate) fn seed(&self, client: C) {
        let mut idle = self
            .inner
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        idle.push(client);
    }

    /// Check out a connection, waiting if `size` connections are already
    /// checked out.
    ///
    /// Reuses an idle connection when one is available *and* still passes
    /// the pool's liveness check; dead idle connections (the server bounced,
    /// the network dropped) are silently discarded and the next idle
    /// connection is tried, falling through to opening a fresh connection
    /// once the idle queue is exhausted. This makes the pool self-healing
    /// even when a caller propagates an error with `?` instead of calling
    /// [`PooledClient::discard`] — a stale idle connection is never handed
    /// out a second time.
    ///
    /// # Errors
    ///
    /// Returns the error from the connect function if a new connection must
    /// be opened and fails.
    pub(crate) async fn get(&self) -> Result<PooledClient<C>, StoreError> {
        // `acquire_owned` requires an `Arc<Semaphore>` and never fails
        // unless the semaphore has been explicitly closed, which this pool
        // never does, so the only realistic outcome is waiting, not an
        // error; still map defensively rather than unwrap.
        let permit = Arc::clone(&self.inner.semaphore)
            .acquire_owned()
            .await
            .map_err(|_| StoreError::Unavailable {
                reason_code: "postgres_pool_closed",
            })?;

        let client = loop {
            let idle_client = {
                let mut idle = self
                    .inner
                    .idle
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                idle.pop()
            };

            match idle_client {
                Some(client) if (self.inner.is_alive)(&client) => break client,
                // Dead idle connection: drop `client` here (its Drop, if
                // any, is the driver's normal close path) and loop back to
                // try the next idle one instead of handing it out.
                Some(_dead_client) => {}
                None => break (self.inner.connect)().await?,
            }
        };

        Ok(PooledClient {
            inner: Arc::clone(&self.inner),
            client: Some(client),
            permit: Some(permit),
            discarded: false,
        })
    }
}

/// A checked-out connection.
///
/// Returned to the pool's idle queue on drop unless [`PooledClient::discard`]
/// was called first (used after a connection returns a protocol/IO error and
/// must never be reused, per spec D2).
pub(crate) struct PooledClient<C> {
    inner: Arc<PoolInner<C>>,
    client: Option<C>,
    permit: Option<OwnedSemaphorePermit>,
    discarded: bool,
}

impl<C> PooledClient<C> {
    /// Mark this connection as poisoned *in place*, without consuming the
    /// handle: it is dropped rather than returned to the idle queue when the
    /// handle itself is eventually dropped.
    ///
    /// [`PooledClient::discard`] is the preferred spelling when the caller
    /// owns the handle. `poison` exists for callers that only hold
    /// `&mut PooledClient` — notably [`crate::append::append`], whose
    /// signature (spec D4) borrows the checkout rather than consuming it, yet
    /// must still guarantee that a connection which hit an IO/protocol error
    /// (or an ambiguous `COMMIT`, spec D5) is never handed out again.
    pub(crate) fn poison(&mut self) {
        self.discarded = true;
    }

    /// Mark this connection as poisoned: it is dropped rather than returned
    /// to the pool, and the checkout slot it held is released so a fresh
    /// connection can be opened by a later [`Pool::get`].
    pub(crate) fn discard(mut self) {
        self.discarded = true;
        // Explicit drop makes the intent visible at the call site; the
        // `Drop` impl below is what actually skips returning the client to
        // the idle queue once `discarded` is set.
        drop(self.client.take());
        drop(self.permit.take());
    }
}

impl<C> std::ops::Deref for PooledClient<C> {
    type Target = C;

    fn deref(&self) -> &C {
        #[allow(
            clippy::unwrap_used,
            reason = "client is only ever None after discard()/drop(), \
                      neither of which leaves the PooledClient reachable"
        )]
        self.client.as_ref().unwrap()
    }
}

impl<C> std::ops::DerefMut for PooledClient<C> {
    fn deref_mut(&mut self) -> &mut C {
        #[allow(
            clippy::unwrap_used,
            reason = "client is only ever None after discard()/drop(), \
                      neither of which leaves the PooledClient reachable"
        )]
        self.client.as_mut().unwrap()
    }
}

impl<C> Drop for PooledClient<C> {
    fn drop(&mut self) {
        if !self.discarded
            && let Some(client) = self.client.take()
        {
            let mut idle = self
                .inner
                .idle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            idle.push(client);
        }
        // The permit (if still held) drops here, releasing the semaphore
        // slot regardless of whether the connection was returned or
        // discarded — either way the pool may now open/reuse one more
        // connection.
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use super::*;

    /// A dedicated dead-marker value the test connectors below never
    /// produce themselves, so `always_alive`/`dead_marker_is_dead` can tell
    /// it apart from any real connected value.
    const DEAD_MARKER: u32 = u32::MAX;

    fn counting_connector() -> (ConnectFn<u32>, Arc<AtomicU32>) {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_for_closure = Arc::clone(&counter);
        let connect: ConnectFn<u32> = Box::new(move || {
            let counter = Arc::clone(&counter_for_closure);
            Box::pin(async move { Ok(counter.fetch_add(1, Ordering::SeqCst)) })
        });
        (connect, counter)
    }

    fn always_alive() -> IsAliveFn<u32> {
        Box::new(|_client| true)
    }

    fn dead_marker_is_dead() -> IsAliveFn<u32> {
        Box::new(|client| *client != DEAD_MARKER)
    }

    #[tokio::test]
    async fn reuses_idle_connections_instead_of_reconnecting() {
        let (connect, counter) = counting_connector();
        let pool = Pool::new(4, connect, always_alive());

        let first = pool.get().await.expect("first checkout");
        drop(first);
        let _second = pool.get().await.expect("second checkout");

        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "second checkout should reuse the returned connection, not open a new one"
        );
    }

    #[tokio::test]
    async fn discarded_connections_are_not_reused() {
        let (connect, counter) = counting_connector();
        let pool = Pool::new(4, connect, always_alive());

        let first = pool.get().await.expect("first checkout");
        first.discard();
        let _second = pool.get().await.expect("second checkout");

        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "a discarded connection must never be handed out again"
        );
    }

    /// `poison` is `discard`'s in-place form (used by `append`, which only
    /// holds `&mut PooledClient`): a poisoned connection must not return to
    /// the idle queue when the handle is later dropped.
    #[tokio::test]
    async fn poisoned_connections_are_not_reused() {
        let (connect, counter) = counting_connector();
        let pool = Pool::new(4, connect, always_alive());

        let mut first = pool.get().await.expect("first checkout");
        first.poison();
        drop(first);
        let _second = pool.get().await.expect("second checkout");

        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "a poisoned connection must never be handed out again"
        );
    }

    #[tokio::test]
    async fn seeded_connection_is_used_before_connecting() {
        let (connect, counter) = counting_connector();
        let pool = Pool::new(4, connect, always_alive());
        pool.seed(9999);

        let checked_out = pool.get().await.expect("checkout");

        assert_eq!(
            *checked_out, 9999,
            "should have handed out the seeded connection"
        );
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "connect() must not be called while a seeded connection is idle"
        );
    }

    #[tokio::test]
    async fn pool_size_is_honored_third_get_waits_for_a_return() {
        let (connect, _counter) = counting_connector();
        let pool = Pool::new(2, connect, always_alive());

        let first = pool.get().await.expect("first checkout");
        let second = pool.get().await.expect("second checkout");

        let waiting_pool = pool.clone();
        let mut third = Box::pin(waiting_pool.get());

        // The third checkout must not resolve while both permits are held.
        let not_ready = tokio::time::timeout(Duration::from_millis(50), third.as_mut()).await;
        assert!(
            not_ready.is_err(),
            "third get() should still be waiting with 0 free permits"
        );

        drop(first);

        let third = tokio::time::timeout(Duration::from_secs(1), third)
            .await
            .expect("third get() should complete once a connection is returned")
            .expect("third checkout");

        drop(second);
        drop(third);
    }

    /// Reviewer finding: a dead idle connection must be dropped and skipped
    /// at checkout, not handed back out. Seeds two "dead" connections
    /// (never popped by `is_alive`) directly into the idle queue and
    /// asserts `get()` skips both and falls through to `connect()` exactly
    /// once, rather than returning a dead connection or looping forever.
    #[tokio::test]
    async fn dead_idle_connections_are_skipped_at_checkout() {
        let (connect, counter) = counting_connector();
        let pool = Pool::new(4, connect, dead_marker_is_dead());
        pool.seed(DEAD_MARKER);
        pool.seed(DEAD_MARKER);

        let checked_out = pool.get().await.expect("checkout should self-heal");

        assert_ne!(
            *checked_out, DEAD_MARKER,
            "a dead idle connection must never be handed out"
        );
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "checkout should skip both dead idle connections and connect exactly once"
        );
    }

    /// Reviewer finding: [`crate::journal_store`]'s `health()` wraps its
    /// `Pool::get` checkout in `tokio::time::timeout` so an exhausted pool
    /// reports `ready: false` instead of hanging forever. This exercises the
    /// exact scenario that fix guards against, one level down: with the
    /// pool's only permit checked out and never returned, a `get()` racing
    /// against a short timeout must lose the race (time out) rather than
    /// resolve — i.e. `get()` alone genuinely blocks on an exhausted pool,
    /// which is why `health()` needs the timeout wrapper at all.
    #[tokio::test]
    async fn get_times_out_on_an_exhausted_pool() {
        let (connect, _counter) = counting_connector();
        let pool = Pool::new(1, connect, always_alive());

        // Hold the pool's only permit for the lifetime of the test.
        let _held = pool.get().await.expect("first checkout");

        let waiting_pool = pool.clone();
        let outcome = tokio::time::timeout(Duration::from_millis(50), waiting_pool.get()).await;

        assert!(
            outcome.is_err(),
            "get() must not resolve while the pool's only permit is held"
        );
    }

    /// Same finding, against a real server: an idle pooled
    /// `tokio_postgres::Client` whose backend gets killed out from under it
    /// (server restart / network drop, simulated here with
    /// `pg_terminate_backend`) must be dropped at the next checkout rather
    /// than handed back out, and the pool must transparently reconnect.
    #[tokio::test]
    async fn dead_idle_postgres_connection_is_dropped_and_pool_reconnects() {
        let Some(url) = std::env::var("FINSTACK_PG_TEST_URL").ok() else {
            eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
            return;
        };

        let connect_url = url.clone();
        let connect: ConnectFn<tokio_postgres::Client> = Box::new(move || {
            let url = connect_url.clone();
            Box::pin(async move {
                let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
                    .await
                    .map_err(|_error| StoreError::Unavailable {
                        reason_code: "postgres_unavailable",
                    })?;
                tokio::spawn(async move {
                    let _ = connection.await;
                });
                Ok(client)
            })
        });
        let is_alive: IsAliveFn<tokio_postgres::Client> =
            Box::new(|client: &tokio_postgres::Client| !client.is_closed());

        let pool = Pool::new(4, connect, is_alive);

        let first = pool.get().await.expect("first checkout");
        let pid: i32 = first
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .expect("read backend pid")
            .get(0);
        drop(first); // returns the connection to the idle queue

        let (admin, admin_connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
            .await
            .expect("admin connect");
        tokio::spawn(async move {
            let _ = admin_connection.await;
        });
        admin
            .execute("SELECT pg_terminate_backend($1)", &[&pid])
            .await
            .expect("terminate the idle backend");

        // Give the client's background connection task a moment to observe
        // the closed socket and flip `is_closed()`.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let second = pool
            .get()
            .await
            .expect("checkout must self-heal despite the dead idle connection");
        let same_backend: bool = second
            .query_one("SELECT pg_backend_pid() = $1", &[&pid])
            .await
            .expect("read backend pid")
            .get(0);
        assert!(
            !same_backend,
            "the pool must not hand back the terminated connection"
        );
    }
}
