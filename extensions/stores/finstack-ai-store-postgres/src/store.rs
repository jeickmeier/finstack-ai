//! Store handle: [`PostgresJournalStore::try_open`] and the shared
//! per-connection setup every pooled connection runs (spec D2/D6/D8).
//!
//! ## TLS
//!
//! This crate depends on `tokio-postgres-rustls` + `rustls`, but this task
//! wires only plaintext (`tokio_postgres::NoTls`) connections. Building a
//! working `rustls::ClientConfig` requires a trust root store; the offline,
//! reproducible option (bundling `webpki-roots`) and the "use the platform
//! roots" option both add real complexity and a dependency decision that
//! belongs to its own change, not this one. The plan's TLS risk note
//! pre-authorizes deferring this: any connection URL whose `sslmode`
//! demands TLS (`require`, `verify-ca`, `verify-full`) is rejected up front
//! in [`PostgresJournalStore::try_open`] with
//! `StoreError::InvalidRequest{reason_code: "postgres_tls_unsupported"}`
//! rather than silently connecting in plaintext. Wiring
//! `tokio-postgres-rustls` is follow-up work.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::SessionId;
use finstack_ai_runtime::StoreError;

use crate::config::{PostgresDurability, PostgresStoreConfig};
use crate::error::map_postgres_error;
use crate::load::VerifiedHead;
use crate::pool::Pool;
use crate::schema::ensure_schema;

/// `sslmode` values that require a TLS connection. Anything else (`disable`,
/// `allow`, `prefer`, or an absent `sslmode`) is compatible with the
/// plaintext-only [`connect_and_prepare`] below.
const TLS_REQUIRED_SSLMODES: [&str; 3] = [
    "sslmode=require",
    "sslmode=verify-ca",
    "sslmode=verify-full",
];

/// Human-readable, stable [`finstack_ai_runtime::StoreHealth::detail`] text
/// for each durability mode (spec D6).
pub(crate) const DURABLE_DETAIL: &str = "postgres synchronous_commit=on";
/// See [`DURABLE_DETAIL`].
pub(crate) const RELAXED_DETAIL: &str = "postgres synchronous_commit=off";

/// Durable, multi-writer `PostgreSQL` [`finstack_ai_runtime::JournalStore`].
///
/// Construct with [`PostgresJournalStore::try_open`]. `Self` is not itself
/// cheaply `Clone` (its config is, but the store owns the pool outright);
/// hand it out behind an `Arc<PostgresJournalStore>` at the application
/// boundary, the same way the sqlite store is shared.
pub struct PostgresJournalStore {
    pub(crate) pool: Pool<tokio_postgres::Client>,
    pub(crate) config: PostgresStoreConfig,
    /// Process-local chain-verification cache (spec D9), keyed by session.
    ///
    /// `Arc` because every port method returns a `'static` future that must
    /// own what it touches; a `std::sync::Mutex` (never held across an
    /// `.await` — see [`VerifiedCache`]) because the guarded map operations
    /// are pure memory work that no async runtime needs to see.
    pub(crate) verified: VerifiedCache,
}

/// Shared handle to the per-session [`VerifiedHead`] cache.
pub(crate) type VerifiedCache = Arc<Mutex<HashMap<SessionId, VerifiedHead>>>;

/// Read the cached verified head for `session_id`, if any.
///
/// A poisoned mutex is recovered from rather than propagated: the guarded
/// section is a `HashMap` lookup that cannot leave the map inconsistent, and
/// the crate forbids `unwrap`/`panic` in non-test code.
pub(crate) fn cached_head(cache: &VerifiedCache, session_id: SessionId) -> Option<VerifiedHead> {
    match cache.lock() {
        Ok(map) => map.get(&session_id).copied(),
        Err(poisoned) => poisoned.into_inner().get(&session_id).copied(),
    }
}

/// Record `head` as verified for `session_id`, replacing any prior entry.
pub(crate) fn remember_head(cache: &VerifiedCache, session_id: SessionId, head: VerifiedHead) {
    match cache.lock() {
        Ok(mut map) => {
            map.insert(session_id, head);
        }
        Err(poisoned) => {
            poisoned.into_inner().insert(session_id, head);
        }
    }
}

/// Drop any cached proof for `session_id` (spec D9: on an `Integrity`
/// result, or when the observed head fell below the cached sequence).
pub(crate) fn invalidate_head(cache: &VerifiedCache, session_id: SessionId) {
    match cache.lock() {
        Ok(mut map) => {
            map.remove(&session_id);
        }
        Err(poisoned) => {
            poisoned.into_inner().remove(&session_id);
        }
    }
}

impl PostgresJournalStore {
    /// Open (and, under [`crate::SchemaPolicy::Manage`], migrate) a
    /// `PostgreSQL` journal store.
    ///
    /// Validates `config`, opens one connection (bounded by
    /// `config.connect_timeout`), runs [`ensure_schema`] over it, then seeds
    /// the connection pool with that same connection so no extra round trip
    /// is spent opening and discarding a throwaway one.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidRequest`] for an invalid config or a
    /// connection URL that demands TLS (see the module doc comment).
    /// Returns [`StoreError::Unavailable`] if the initial connection cannot
    /// be established within `config.connect_timeout`, or the mapped error
    /// from [`ensure_schema`] otherwise.
    pub async fn try_open(config: PostgresStoreConfig) -> Result<Self, StoreError> {
        config.validate()?;
        if url_requires_tls(&config.url) {
            return Err(StoreError::InvalidRequest {
                reason_code: "postgres_tls_unsupported",
            });
        }

        let client = connect_and_prepare(&config).await?;
        ensure_schema(&client, &config.schema, config.schema_policy).await?;

        let pool_config = config.clone();
        let pool = Pool::new(
            config.pool_size,
            Box::new(move || {
                let pool_config = pool_config.clone();
                Box::pin(async move { connect_and_prepare(&pool_config).await })
            }),
            // A connection can die while idle in the pool (server restart,
            // network drop) with nothing else noticing; the pool checks
            // this on every idle connection it pops before handing it out
            // (see the finding fixed in `src/pool.rs::Pool::get`).
            Box::new(|client: &tokio_postgres::Client| !client.is_closed()),
        );
        pool.seed(client);

        Ok(Self {
            pool,
            config,
            verified: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

/// Returns `true` if `url` sets an `sslmode` that requires TLS.
///
/// A plain substring check is sufficient here (rather than parsing the URL
/// as a full connection string): `sslmode` only ever appears as a
/// `key=value` query/keyword pair, and every value that requires TLS is
/// checked verbatim.
fn url_requires_tls(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    TLS_REQUIRED_SSLMODES
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Open one physical connection, apply its session-scoped setup (spec D6),
/// and spawn its driving task.
///
/// # Errors
///
/// Returns [`StoreError::Unavailable`] if the connection cannot be
/// established within `config.connect_timeout`, or a mapped
/// [`StoreError`] if the connection succeeds but the session-setup
/// statements fail.
pub(crate) async fn connect_and_prepare(
    config: &PostgresStoreConfig,
) -> Result<tokio_postgres::Client, StoreError> {
    let connect = tokio_postgres::connect(&config.url, tokio_postgres::NoTls);
    let (client, connection) = tokio::time::timeout(config.connect_timeout, connect)
        .await
        .map_err(|_elapsed| StoreError::Unavailable {
            reason_code: "postgres_unavailable",
        })?
        .map_err(|error| map_postgres_error(&error))?;

    // `tokio_postgres::connect` returns a `Connection` future that must be
    // polled for the client to make any progress; drive it on its own task
    // for the lifetime of the connection (mirrors the crate's own test
    // helper and the driver's documented usage pattern).
    tokio::spawn(async move {
        // Best-effort: once this errors, subsequent uses of `client` will
        // themselves start failing, which existing callers already handle
        // through `map_postgres_error`.
        let _ = connection.await;
    });

    let synchronous_commit = match config.durability {
        PostgresDurability::Durable => "on",
        PostgresDurability::Relaxed => "off",
    };
    client
        .batch_execute(&format!("SET synchronous_commit = {synchronous_commit}"))
        .await
        .map_err(|error| map_postgres_error(&error))?;
    // `config.validate()` (called before any connection is opened) already
    // enforced the identifier grammar on `schema`, so interpolating it here
    // is safe for the same reason `ensure_schema`'s DDL interpolation is.
    client
        .batch_execute(&format!("SET search_path = {}", config.schema))
        .await
        .map_err(|error| map_postgres_error(&error))?;

    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sslmode_require() {
        assert!(url_requires_tls(
            "postgres://user:pass@host/db?sslmode=require"
        ));
        assert!(url_requires_tls(
            "postgres://user:pass@host/db?sslmode=verify-ca"
        ));
        assert!(url_requires_tls(
            "postgres://user:pass@host/db?sslmode=verify-full"
        ));
        assert!(url_requires_tls(
            "postgres://user:pass@host/db?sslmode=REQUIRE"
        ));
    }

    #[test]
    fn tolerates_non_tls_sslmodes() {
        for url in [
            "postgres://user:pass@host/db",
            "postgres://user:pass@host/db?sslmode=disable",
            "postgres://user:pass@host/db?sslmode=allow",
            "postgres://user:pass@host/db?sslmode=prefer",
        ] {
            assert!(!url_requires_tls(url), "{url} should not require TLS");
        }
    }
}
