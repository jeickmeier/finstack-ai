//! Shared test harness for the server-gated integration suites.
//!
//! Every suite that needs a real server reads [`pg_test_url`] first and
//! skips (returning `Ok(())`/printing a notice) when it is unset, so `cargo
//! test -p finstack-ai-store-postgres` is green both with and without a
//! live Postgres reachable.

use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use finstack_ai_runtime::StoreLimits;
use finstack_ai_store_postgres::{PostgresJournalStore, PostgresStoreConfig};

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
/// flag (via a stderr warning) a test that forgot to call it.
///
/// The `Drop` impl must never panic: it also runs while unwinding from a
/// failed assertion inside a test (the guard is typically constructed
/// before the assertions it covers), and a panic during unwind is a double
/// panic that aborts the whole test process, taking down every
/// concurrently-running test with it. A leaked `fa_test_*` schema after a
/// failed test is an acceptable, visible (via the eprintln) trade-off for
/// keeping the original assertion failure intact.
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

/// Open a disposable [`PostgresJournalStore`] against a fresh schema, for
/// tests that exercise the store through its public API (`try_open`,
/// `health()`, and — once landed — `append`/`load`/`write_snapshot`) rather
/// than driving `ensure_schema` directly.
///
/// Uses generous [`StoreLimits`] since these tests care about the store's
/// wiring, not its admission-limit behavior. `config.schema` is overridden
/// to a fresh, disposable name so concurrent test runs never collide.
///
/// # Panics
///
/// Panics (via `expect`) if `try_open` fails. Callers should check
/// [`pg_test_url`] themselves first and skip-with-notice when it is unset,
/// matching the crate's other env-gated suites — this helper assumes the
/// caller already confirmed a server is configured.
pub async fn disposable_store(url: &str) -> (PostgresJournalStore, SchemaGuard) {
    let schema = fresh_schema_name();
    let limits = StoreLimits {
        sessions: 1_000,
        batches_per_session: 1_000,
        records_per_session: 1_000,
        snapshot_bytes: 1_000_000,
    };
    let mut config = PostgresStoreConfig::new(url, limits);
    config.tls_mode = finstack_ai_store_postgres::PostgresTlsMode::Disable;
    config.schema = Arc::from(schema.as_str());

    let store = PostgresJournalStore::try_open(config)
        .await
        .expect("try_open disposable store");
    (store, SchemaGuard::new(schema))
}

impl Drop for SchemaGuard {
    fn drop(&mut self) {
        // Must not panic here: this can run during unwind from a failed
        // test assertion, and panicking while already panicking aborts the
        // process (see the struct doc comment). Warn instead so the leaked
        // schema is still visible for manual cleanup.
        if !self.cleaned_up {
            eprintln!(
                "warning: SchemaGuard for {} dropped without calling cleanup(); the disposable \
                 schema was leaked in the test database",
                self.schema
            );
        }
    }
}
