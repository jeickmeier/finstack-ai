//! Server-gated proof of the spec D5 ambiguous-acknowledgement contract.
//!
//! An append whose connection dies around `COMMIT` cannot know whether the
//! transaction landed. The store reports that as
//! [`StoreError::AmbiguousAcknowledgement`] and the caller recovers by
//! retrying the identical [`AppendRequest`] — which the idempotency contract
//! makes safe on *both* sides of the sever. This suite proves it rather than
//! asserting it, by putting a byte-level TCP proxy between the store and a
//! real server and cutting the stream at a chosen point in the frontend
//! protocol.
//!
//! Every test skips with a notice (exit 0) when `FINSTACK_PG_TEST_URL` is
//! unset, per the crate's env-gated convention (spec D10).

#[allow(
    dead_code,
    reason = "this suite uses only part of the crate-wide postgres test harness"
)]
mod helpers;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use finstack_ai_kernel::SessionTag;
use finstack_ai_runtime::{JournalStore, LoadRequest, StoreError, StoreLimits};
use finstack_ai_store_postgres::{PostgresJournalStore, PostgresStoreConfig};
use finstack_ai_test::store_fixtures::{draft, id, request};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use helpers::{SchemaGuard, connect, fresh_schema_name, pg_test_url};

// ---------------------------------------------------------------------------
// Protocol markers
// ---------------------------------------------------------------------------

/// Frontend simple-query message carrying `COMMIT`.
///
/// Postgres frontend messages are `type byte + i32 length (including the
/// length field) + body`; `tokio_postgres::Transaction::commit` issues the
/// statement through the simple-query path, so the bytes on the wire are
/// exactly `'Q'`, length `11`, and the NUL-terminated string `COMMIT`.
/// Matching the full framed message (rather than sniffing for the substring
/// `COMMIT` anywhere in the stream) is what makes the sever point
/// deterministic.
const COMMIT_QUERY: &[u8] = b"Q\x00\x00\x00\x0bCOMMIT\x00";

/// Backend `CommandComplete` for that statement: same framing, tag `'C'`.
///
/// Postgres only writes this after the transaction is durably committed, so
/// observing it proves the commit landed server-side — the precondition for
/// the "committed but unacknowledged" half of the ambiguity contract.
const COMMIT_COMPLETE: &[u8] = b"C\x00\x00\x00\x0bCOMMIT\x00";

// ---------------------------------------------------------------------------
// The severing proxy
// ---------------------------------------------------------------------------

/// Where the proxy cuts the stream once armed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sever {
    /// Never cut: control run proving the proxy itself is byte-transparent.
    Never,
    /// Drop the connection on the frontend `COMMIT`, before it reaches the
    /// server. The server sees EOF mid-transaction and rolls back, so the
    /// retry must commit the batch fresh.
    BeforeCommit,
    /// Forward the `COMMIT`, wait for the backend's `CommandComplete`, then
    /// drop the connection without delivering it. The server committed and
    /// the client never learns, so the retry must replay the committed batch.
    AfterCommitAck,
}

/// A running loopback proxy in front of the real server.
struct Proxy {
    /// Loopback address the store connects to.
    address: SocketAddr,
    /// Set by the test once the store is open, so the schema-migration
    /// traffic that `try_open` issues is never a sever candidate.
    armed: Arc<AtomicBool>,
    /// Connections cut by the sever rule.
    severed: Arc<AtomicU32>,
    /// Frontend `COMMIT` messages observed while armed.
    commits: Arc<AtomicU32>,
}

/// Bind a proxy on `127.0.0.1:0` forwarding to `upstream`.
async fn start_proxy(upstream: String, rule: Sever) -> Proxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
    let address = listener.local_addr().expect("proxy address");
    let armed = Arc::new(AtomicBool::new(false));
    let severed = Arc::new(AtomicU32::new(0));
    let commits = Arc::new(AtomicU32::new(0));

    let task_armed = Arc::clone(&armed);
    let task_severed = Arc::clone(&severed);
    let task_commits = Arc::clone(&commits);
    tokio::spawn(async move {
        loop {
            let Ok((client, _peer)) = listener.accept().await else {
                return;
            };
            let Ok(upstream_socket) = TcpStream::connect(&upstream).await else {
                return;
            };
            tokio::spawn(pump(
                client,
                upstream_socket,
                rule,
                Arc::clone(&task_armed),
                Arc::clone(&task_severed),
                Arc::clone(&task_commits),
            ));
        }
    });

    Proxy {
        address,
        armed,
        severed,
        commits,
    }
}

/// Forward one connection in both directions until the sever rule fires.
///
/// Returning from this function drops both socket halves, which closes both
/// TCP connections — that abrupt close is the "crash" the store observes.
async fn pump(
    client: TcpStream,
    upstream: TcpStream,
    rule: Sever,
    armed: Arc<AtomicBool>,
    severed: Arc<AtomicU32>,
    commits: Arc<AtomicU32>,
) {
    let (mut client_read, mut client_write) = client.into_split();
    let (mut server_read, mut server_write) = upstream.into_split();
    let mut client_buffer = vec![0_u8; 16 * 1024];
    let mut server_buffer = vec![0_u8; 16 * 1024];
    let mut client_tail = Vec::new();
    let mut server_tail = Vec::new();
    // Set once the armed connection's `COMMIT` has been handed to the server,
    // arming the `AfterCommitAck` cut on the backend's acknowledgement.
    let mut commit_forwarded = false;

    loop {
        tokio::select! {
            read = client_read.read(&mut client_buffer) => {
                let Ok(count) = read else { return };
                if count == 0 {
                    return;
                }
                let chunk = &client_buffer[..count];
                let commit = scan(&mut client_tail, chunk, COMMIT_QUERY)
                    && armed.load(Ordering::SeqCst);
                if commit {
                    commits.fetch_add(1, Ordering::SeqCst);
                    if rule == Sever::BeforeCommit {
                        // Cut without forwarding: the server sees EOF with the
                        // transaction still open and rolls it back.
                        severed.fetch_add(1, Ordering::SeqCst);
                        return;
                    }
                }
                if server_write.write_all(chunk).await.is_err() {
                    return;
                }
                if commit && rule == Sever::AfterCommitAck {
                    commit_forwarded = true;
                }
            }
            read = server_read.read(&mut server_buffer) => {
                let Ok(count) = read else { return };
                if count == 0 {
                    return;
                }
                let chunk = &server_buffer[..count];
                let acknowledged = scan(&mut server_tail, chunk, COMMIT_COMPLETE);
                if commit_forwarded && acknowledged {
                    // The commit is durable; cut before the client can learn.
                    severed.fetch_add(1, Ordering::SeqCst);
                    return;
                }
                if client_write.write_all(chunk).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Does `tail + chunk` contain `marker`?
///
/// `tail` carries the last `marker.len() - 1` bytes of the previous chunk
/// forward, so a marker split across two reads is still matched.
fn scan(tail: &mut Vec<u8>, chunk: &[u8], marker: &[u8]) -> bool {
    let mut window = std::mem::take(tail);
    window.extend_from_slice(chunk);
    let found = window.windows(marker.len()).any(|slice| slice == marker);
    let keep = window.len().saturating_sub(marker.len() - 1);
    *tail = window[keep..].to_vec();
    found
}

// ---------------------------------------------------------------------------
// URL plumbing
// ---------------------------------------------------------------------------

/// `host:port` the connection URL points at (defaulting to Postgres' 5432).
fn upstream_authority(url: &str) -> String {
    let rest = url.split_once("://").expect("postgres:// url").1;
    let after_credentials = rest.rsplit_once('@').map_or(rest, |(_user, host)| host);
    let authority = after_credentials
        .split(['/', '?'])
        .next()
        .expect("authority");
    if authority.contains(':') {
        authority.to_owned()
    } else {
        format!("{authority}:5432")
    }
}

/// The same URL with its authority replaced by the proxy's address.
fn proxied_url(url: &str, address: SocketAddr) -> String {
    let (scheme, rest) = url.split_once("://").expect("postgres:// url");
    let (credentials, tail) = rest
        .rsplit_once('@')
        .map_or((String::new(), rest), |(user, host)| {
            (format!("{user}@"), host)
        });
    let path_start = tail.find(['/', '?']).unwrap_or(tail.len());
    format!("{scheme}://{credentials}{address}{}", &tail[path_start..])
}

// ---------------------------------------------------------------------------
// Store plumbing
// ---------------------------------------------------------------------------

fn wide_limits() -> StoreLimits {
    StoreLimits {
        sessions: 64,
        batches_per_session: 512,
        records_per_session: 2_048,
        snapshot_bytes: 1_000_000,
    }
}

/// Open a store against `schema`, pinned to a single pooled connection so the
/// append the test severs is unambiguously the connection the proxy watches.
async fn open_store(url: &str, schema: &str) -> PostgresJournalStore {
    let mut config = PostgresStoreConfig::new(url, wide_limits());
    config.schema = Arc::from(schema);
    config.pool_size = 1;
    PostgresJournalStore::try_open(config)
        .await
        .expect("open postgres journal store")
}

/// Rows in `schema.batches` — the ground truth for "how many copies of the
/// batch are committed", read over a direct (unproxied) connection.
async fn committed_batch_count(client: &tokio_postgres::Client, schema: &str) -> i64 {
    client
        .query_one(&format!("SELECT count(*) FROM {schema}.batches"), &[])
        .await
        .expect("count committed batches")
        .get(0)
}

/// Rows in `schema.records`.
async fn committed_record_count(client: &tokio_postgres::Client, schema: &str) -> i64 {
    client
        .query_one(&format!("SELECT count(*) FROM {schema}.records"), &[])
        .await
        .expect("count committed records")
        .get(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Control: with the proxy forwarding every byte, the append behaves exactly
/// as it does on a direct connection. Without this row, a failure in the two
/// sever tests below could not be attributed to the sever itself.
#[tokio::test]
async fn transparent_proxy_commits_normally() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let schema = fresh_schema_name();
    let guard = SchemaGuard::new(schema.clone());
    let proxy = start_proxy(upstream_authority(&url), Sever::Never).await;
    let store = open_store(&proxied_url(&url, proxy.address), &schema).await;
    proxy.armed.store(true, Ordering::SeqCst);

    let committed = store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("append through a transparent proxy");
    assert_eq!(committed.first_sequence, 1);
    assert_eq!(committed.last_sequence, 1);
    assert!(
        proxy.commits.load(Ordering::SeqCst) >= 1,
        "the proxy must recognise the framed COMMIT message"
    );
    assert_eq!(proxy.severed.load(Ordering::SeqCst), 0);

    drop(store);
    let client = connect(&url).await;
    assert_eq!(committed_batch_count(&client, &schema).await, 1);
    guard.cleanup(&client).await;
}

/// Sever *after* the server acknowledges the commit: the batch is durable but
/// the client never hears it. The retry must replay the committed batch, and
/// the journal must still hold exactly one copy.
#[tokio::test]
async fn commit_acknowledged_then_severed_is_ambiguous_and_retry_replays() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (committed_before_retry, retried) =
        run_severed_append(&url, Sever::AfterCommitAck, 1).await;
    assert_eq!(
        committed_before_retry, 1,
        "AfterCommitAck must leave the batch committed server-side"
    );
    assert_eq!(retried.first_sequence, 1);
    assert_eq!(retried.last_sequence, 1);
}

/// Sever *before* the server sees the commit: the transaction rolls back. The
/// retry must commit the batch fresh, and the journal must still hold exactly
/// one copy.
#[tokio::test]
async fn commit_severed_before_delivery_is_ambiguous_and_retry_commits_fresh() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (committed_before_retry, retried) = run_severed_append(&url, Sever::BeforeCommit, 0).await;
    assert_eq!(
        committed_before_retry, 0,
        "BeforeCommit must leave the transaction rolled back server-side"
    );
    assert_eq!(retried.first_sequence, 1);
    assert_eq!(retried.last_sequence, 1);
}

/// Shared body of both ambiguity rows.
///
/// Severs one append per `rule`, asserts the reported error is
/// [`StoreError::AmbiguousAcknowledgement`], then retries the *identical*
/// request through a fresh store on a direct connection and asserts the final
/// journal holds exactly one copy of the batch and verifies as a chain.
///
/// Returns the committed-batch count observed *between* the sever and the
/// retry (which distinguishes the two ambiguity outcomes) and the retry's
/// committed batch.
async fn run_severed_append(
    url: &str,
    rule: Sever,
    expected_batches_after_sever: i64,
) -> (i64, finstack_ai_kernel::CommittedBatch) {
    let schema = fresh_schema_name();
    let guard = SchemaGuard::new(schema.clone());
    let proxy = start_proxy(upstream_authority(url), rule).await;
    let store = open_store(&proxied_url(url, proxy.address), &schema).await;

    // Arm only now: `try_open`'s migration transaction commits too, and it is
    // not the transaction under test.
    proxy.armed.store(true, Ordering::SeqCst);

    let append = request(1, 1, 1, vec![draft(1, 1)]);
    let error = store
        .append(append.clone())
        .await
        .expect_err("a severed commit must not report success");
    assert!(
        matches!(error, StoreError::AmbiguousAcknowledgement),
        "expected AmbiguousAcknowledgement, got {error:?}"
    );
    assert_eq!(
        proxy.severed.load(Ordering::SeqCst),
        1,
        "the proxy must have cut exactly the append's connection"
    );
    drop(store);

    let client = connect(url).await;
    let committed_before_retry = committed_batch_count(&client, &schema).await;
    assert_eq!(committed_before_retry, expected_batches_after_sever);

    // Retry the identical request on a fresh, unproxied store.
    let retry_store = open_store(url, &schema).await;
    let retried = retry_store
        .append(append)
        .await
        .expect("identical retry after an ambiguous acknowledgement");

    // `load` performs full-chain verification, so a successful load is itself
    // the chain assertion.
    let loaded = retry_store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("chain-verified load after the retry");
    assert_eq!(loaded.head_sequence, 1);
    assert_eq!(loaded.committed_batches.len(), 1);
    assert_eq!(
        loaded.committed_batches.first().expect("one batch"),
        &retried
    );

    assert_eq!(committed_batch_count(&client, &schema).await, 1);
    assert_eq!(committed_record_count(&client, &schema).await, 1);

    drop(retry_store);
    guard.cleanup(&client).await;
    (committed_before_retry, retried)
}
