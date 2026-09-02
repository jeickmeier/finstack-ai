//! The session-row `FOR UPDATE` lock shared by [`crate::append`] and
//! [`crate::snapshot`].
//!
//! Both modules serialize their writers of one session on the same row
//! lock: an append CASes `current_sequence`/`head_checksum` forward, a
//! snapshot write CASes `snapshot_sequence`, and a metadata write CASes
//! `metadata` against `head_checksum`. Lifting the lock query into one place
//! (rather than duplicating it per caller) keeps the column list and lock
//! semantics from drifting between the two.

use finstack_ai_kernel::{Digest, SessionId};
use tokio_postgres::{Statement, Transaction};

use crate::error::{Failure, u64_from_i64, usize_from_i64};
use crate::load::digest_from_bytes;

/// The session row's committed footprint, read under `FOR UPDATE`.
pub(crate) struct LockedSession {
    /// Sequence of the journal head (0 for a session with no records).
    pub(crate) current_sequence: u64,
    /// Checksum of the head record, `None` before the first append.
    pub(crate) head_checksum: Option<Digest>,
    /// Sequence covered by the stored snapshot, when one exists.
    pub(crate) snapshot_sequence: Option<u64>,
    /// Committed batches in this session.
    pub(crate) batch_count: usize,
    /// Committed records in this session.
    pub(crate) record_count: usize,
}

/// The session-row `FOR UPDATE` lock statement.
///
/// Exposed as a `&'static str` so each write op can hand it to
/// [`crate::pool::PooledClient::prepared`] before opening its transaction
/// and pass the cached [`Statement`] in below.
pub(crate) const LOCK_SESSION_SQL: &str = "SELECT current_sequence, head_checksum, \
     snapshot_sequence, batch_count, record_count FROM sessions WHERE session_id = $1 FOR UPDATE";

/// Take the per-session write lock, returning the row when it exists.
///
/// This is the serialization point for all writers of one session: two
/// writers of the same session (whether both appending, both writing a
/// snapshot, or one of each) queue here, while writers of distinct sessions
/// never contend.
pub(crate) async fn lock_session(
    transaction: &Transaction<'_>,
    statement: &Statement,
    session_id: SessionId,
) -> Result<Option<LockedSession>, Failure> {
    let row = transaction
        .query_opt(statement, &[&session_id.as_bytes().as_slice()])
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let head_checksum: Option<Vec<u8>> = row.get(1);
    let snapshot_sequence: Option<i64> = row.get(2);
    Ok(Some(LockedSession {
        current_sequence: u64_from_i64(row.get(0), "current_sequence")?,
        head_checksum: head_checksum
            .as_deref()
            .map(digest_from_bytes)
            .transpose()?,
        snapshot_sequence: snapshot_sequence
            .map(|value| u64_from_i64(value, "snapshot_sequence"))
            .transpose()?,
        batch_count: usize_from_i64(row.get(3), "batch_count")?,
        record_count: usize_from_i64(row.get(4), "record_count")?,
    }))
}
