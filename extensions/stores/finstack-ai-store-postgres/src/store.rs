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
use std::sync::{Arc, Mutex, PoisonError};

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
///
/// Every slot carries a generation counter so a load that started before an
/// invalidation cannot write its (now unproven) head afterwards — see
/// [`remember_head`].
pub(crate) type VerifiedCache = Arc<Mutex<HashMap<SessionId, CacheSlot>>>;

/// One session's cache slot: the proof, plus the generation it belongs to.
///
/// The slot outlives the proof it held: [`invalidate_head`] clears `head` but
/// keeps (and bumps) `generation`, so the invalidation is still visible to a
/// load that read the slot before it happened.
pub(crate) struct CacheSlot {
    /// Bumped by every invalidation of this session.
    generation: u64,
    /// The verified head, or `None` once invalidated.
    head: Option<VerifiedHead>,
}

/// A cache read: the proof (if any) plus the generation it was read at.
///
/// The generation must be handed back to [`remember_head`], which discards
/// the write when an invalidation intervened.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VerifiedRead {
    /// Generation of the slot at the time of the read.
    pub(crate) generation: u64,
    /// The cached proof, or `None` when there is none to use.
    pub(crate) head: Option<VerifiedHead>,
}

/// Take the cache lock, recovering from poisoning rather than propagating it.
///
/// The guarded sections are `HashMap` operations that cannot leave the map
/// half-updated, so a panic elsewhere never makes the contents unsafe to
/// read; and the crate forbids `unwrap`/`panic` in non-test code.
fn lock(cache: &VerifiedCache) -> std::sync::MutexGuard<'_, HashMap<SessionId, CacheSlot>> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Read the cached verified head for `session_id`, with its generation.
///
/// A session with no slot reads as generation 0 and no proof, which is the
/// generation a first [`remember_head`] must present.
pub(crate) fn cached_head(cache: &VerifiedCache, session_id: SessionId) -> VerifiedRead {
    let map = lock(cache);
    map.get(&session_id).map_or(
        VerifiedRead {
            generation: 0,
            head: None,
        },
        |slot| VerifiedRead {
            generation: slot.generation,
            head: slot.head,
        },
    )
}

/// Record `head` as verified for `session_id`, unless an invalidation landed
/// since `read.generation` was observed.
///
/// Without this check a load that read the cache *before* a concurrent load
/// found corruption could write its own head afterwards, resurrecting a proof
/// the invalidation was meant to destroy — and, because that head anchors
/// every later suffix verification, the corrupt prefix would never be re-read
/// for the life of the process.
pub(crate) fn remember_head(
    cache: &VerifiedCache,
    session_id: SessionId,
    read: VerifiedRead,
    head: VerifiedHead,
) {
    let mut map = lock(cache);
    let generation = map.get(&session_id).map_or(0, |slot| slot.generation);
    if generation != read.generation {
        // An invalidation intervened: this proof describes a journal state
        // that has since been called into question. Drop it; the next load
        // verifies in full.
        return;
    }
    map.insert(
        session_id,
        CacheSlot {
            generation,
            head: Some(head),
        },
    );
}

/// Drop any cached proof for `session_id` (spec D9: on an `Integrity`
/// result, or when the observed head fell below the cached sequence).
///
/// The slot is kept as a tombstone with a bumped generation so in-flight
/// loads that already read it cannot write over the invalidation.
pub(crate) fn invalidate_head(cache: &VerifiedCache, session_id: SessionId) {
    let mut map = lock(cache);
    let generation = map.get(&session_id).map_or(0, |slot| slot.generation);
    map.insert(
        session_id,
        CacheSlot {
            generation: generation.saturating_add(1),
            head: None,
        },
    );
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
    use finstack_ai_kernel::{Digest, SessionTag};
    use finstack_ai_test::store_fixtures::id;

    use super::*;

    fn empty_cache() -> VerifiedCache {
        Arc::new(Mutex::new(HashMap::new()))
    }

    fn head(sequence: u64) -> VerifiedHead {
        VerifiedHead {
            sequence,
            checksum: Some(Digest::raw_json(b"{}")),
        }
    }

    /// The ordinary sequence — read, then write back — caches the head.
    #[test]
    fn a_head_read_at_the_current_generation_is_cached() {
        let cache = empty_cache();
        let session = id::<SessionTag>(1);

        let read = cached_head(&cache, session);
        assert!(read.head.is_none());
        remember_head(&cache, session, read, head(10));

        assert_eq!(cached_head(&cache, session).head, Some(head(10)));
    }

    /// A load that read the cache before a concurrent load invalidated it
    /// must not resurrect the proof afterwards: otherwise the corrupt prefix
    /// that caused the invalidation would never be verified again.
    #[test]
    fn a_write_racing_an_invalidation_is_discarded() {
        let cache = empty_cache();
        let session = id::<SessionTag>(1);
        remember_head(&cache, session, cached_head(&cache, session), head(10));

        // Load Y reads the cache…
        let stale = cached_head(&cache, session);
        assert_eq!(stale.head, Some(head(10)));
        // …load X finds corruption and invalidates…
        invalidate_head(&cache, session);
        // …and load Y, which knows nothing about that, tries to write back.
        remember_head(&cache, session, stale, head(10));

        let after = cached_head(&cache, session);
        assert!(
            after.head.is_none(),
            "the invalidation must survive a racing write"
        );
        // The next load reads at the bumped generation and can cache again.
        remember_head(&cache, session, after, head(12));
        assert_eq!(cached_head(&cache, session).head, Some(head(12)));
    }

    /// Invalidation is not lost when it happens before anything was cached
    /// (the tombstone slot carries the bumped generation).
    #[test]
    fn invalidating_an_uncached_session_still_blocks_a_racing_write() {
        let cache = empty_cache();
        let session = id::<SessionTag>(1);

        let stale = cached_head(&cache, session);
        invalidate_head(&cache, session);
        remember_head(&cache, session, stale, head(10));

        assert!(cached_head(&cache, session).head.is_none());
    }

    /// Sessions do not share a generation.
    #[test]
    fn invalidation_is_scoped_to_one_session() {
        let cache = empty_cache();
        let (first, second) = (id::<SessionTag>(1), id::<SessionTag>(2));
        let read = cached_head(&cache, second);
        invalidate_head(&cache, first);
        remember_head(&cache, second, read, head(3));

        assert_eq!(cached_head(&cache, second).head, Some(head(3)));
        assert!(cached_head(&cache, first).head.is_none());
    }

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
