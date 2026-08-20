//! Shared test harness for the server-gated integration suites.
//!
//! Every suite that needs a real server reads [`pg_test_url`] first and
//! skips (returning `Ok(())`/printing a notice) when it is unset, so `cargo
//! test -p finstack-ai-store-postgres` is green both with and without a
//! live Postgres reachable.

use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Environment variable naming a live Postgres server for the integration
/// suites, e.g. `postgres://postgres:postgres@localhost:5432/postgres`.
const PG_TEST_URL_VAR: &str = "FINSTACK_PG_TEST_URL";

/// Read the server-gated test URL.
///
/// Returns `None` when [`PG_TEST_URL_VAR`] is unset. Callers should print a
/// skip notice and return early with success in that case, matching the
/// crate's other env-gated suites.
#[must_use]
pub fn pg_test_url() -> Option<String> {
    env::var(PG_TEST_URL_VAR).ok()
}

/// Connect to `url` and spawn its connection-driving task.
///
/// `tokio_postgres::connect` returns a `Client` plus a `Connection` future
/// that must be polled for the client to make progress; this helper spawns
/// that future onto the current runtime (dropping any connection error,
/// which only matters once the client itself starts failing) and returns
/// the ready-to-use client.
///
/// # Panics
///
/// Panics (via `expect`) if the connection cannot be established. Test-only
/// code — the workspace's `unwrap`/`expect` ban is relaxed for `tests/`.
pub async fn connect(url: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
        .await
        .expect("connect to FINSTACK_PG_TEST_URL");
    tokio::spawn(async move {
        // Best-effort: a connection error here only matters to callers
        // through subsequent client errors, which they already handle.
        let _ = connection.await;
    });
    client
}

/// Generate a disposable schema name of the form `fa_test_<hex>`, unique
/// within this process.
///
/// Uses a monotonic counter plus the process id and current time rather
/// than pulling in `uuid`/`getrandom` — collisions would require two calls
/// in the same process at the same nanosecond with the same counter value,
/// which cannot happen since the counter is strictly increasing.
#[must_use]
pub fn fresh_schema_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    format!("fa_test_{pid:x}_{nanos:x}_{counter:x}")
}

/// Drops a disposable test schema when the test is done with it.
///
/// Cleanup is an explicit async call ([`SchemaGuard::cleanup`]) rather than
/// a `Drop` impl: dropping the schema requires an async `DROP SCHEMA`
/// round-trip, and `Drop` cannot run async code. A `Drop`-based best-effort
/// cleanup would need to block the runtime or detach an unsupervised
/// `tokio::spawn` with no guarantee it completes before the process exits
/// (particularly under `#[tokio::test]`'s single-threaded runtime, which
/// stops driving spawned tasks once the test function returns). Calling
/// `cleanup()` explicitly at the end of each test is simpler and its
/// completion is actually observable. `Drop` still exists below purely to
/// flag (via `debug_assert`) a test that forgot to call it.
pub struct SchemaGuard {
    schema: String,
    cleaned_up: bool,
}

impl SchemaGuard {
    /// Wrap a schema name for later cleanup.
    #[must_use]
    pub fn new(schema: String) -> Self {
        Self {
            schema,
            cleaned_up: false,
        }
    }

    /// Drop the schema (`CASCADE`) using `client`. Idempotent-ish: safe to
    /// call once per guard; the underlying `DROP SCHEMA IF EXISTS` is
    /// itself idempotent if called more than once.
    ///
    /// # Panics
    ///
    /// Panics (via `expect`) if the drop fails. Test-only code.
    pub async fn cleanup(mut self, client: &tokio_postgres::Client) {
        let schema = self.schema.clone();
        client
            .batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
            .await
            .expect("drop disposable test schema");
        self.cleaned_up = true;
    }
}

impl Drop for SchemaGuard {
    fn drop(&mut self) {
        debug_assert!(
            self.cleaned_up,
            "SchemaGuard for {} dropped without calling cleanup(); the disposable schema was \
             leaked in the test database",
            self.schema
        );
    }
}
