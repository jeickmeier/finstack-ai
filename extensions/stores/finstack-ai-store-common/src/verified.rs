//! Process-local verified-head cache shared by all backends.
//!
//! A journal is append-only, so once a process has verified a session's chain
//! through some record, that proof stays valid however many other writers
//! extend the chain afterwards. This module owns both halves of exploiting
//! that:
//!
//! * a *decision layer* — [`verify_head_against_cache`] — which, given the
//!   stored records, the observed head and a cached anchor, verifies only the
//!   suffix after the anchor, and
//! * a *cache container* — [`VerifiedHeadCache`] — which holds one
//!   generation-guarded [`VerifiedHead`] per session.
//!
//! The cache is never allowed to change an outcome. Any failure of the cheap
//! suffix path falls back to [`crate::verify_full_head`], which owns the
//! reason codes, so accept/reject decisions and the codes they carry are
//! exactly those of an uncached load. Backends keep the call-site policy:
//! which loads may write the cache (only full loads prove the whole chain)
//! and when to invalidate it (any `Integrity` result).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use finstack_ai_kernel::{Digest, RecordEnvelope, SessionId};
use finstack_ai_protocol::ChainAnchor;
use finstack_ai_runtime::StoreError;

use crate::window::{FROM_SEQUENCE_WINDOW, verify_full_head, verify_tail_records};

/// Process-local proof that a session journal was verified through this head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedHead {
    /// Sequence of the last verified record (0 for an empty journal).
    pub sequence: u64,
    /// Checksum of that record, `None` for an empty journal.
    pub checksum: Option<Digest>,
}

/// Verify a stored journal's head, using `cached` to skip the verified prefix.
///
/// `records` holds every stored record of the session in sequence order (a
/// pruned journal may legitimately start above sequence 1). `head_sequence`
/// and `stored_head` are the session's own record of where the chain ends.
///
/// The cached proof is used only when it demonstrably anchors on a stored
/// record with the exact sequence *and* checksum it claims; otherwise, and on
/// any failure of the suffix verification, the whole journal is re-verified.
/// A cached sequence above `head_sequence` means the journal was pruned or
/// reset behind this process's back and is likewise ignored.
///
/// # Errors
///
/// Returns exactly what [`crate::verify_full_head`] would return for an
/// uncached load of the same journal: [`StoreError::Integrity`] when the
/// chain is broken or does not land on `stored_head`.
pub fn verify_head_against_cache(
    anchor: ChainAnchor,
    records: &[RecordEnvelope],
    head_sequence: u64,
    stored_head: Option<Digest>,
    cached: Option<VerifiedHead>,
) -> Result<Option<Digest>, StoreError> {
    if let Some(cached) = cached
        && cached.sequence >= 1
        && cached.sequence <= head_sequence
        && let Some(prior) = cached.checksum
        && suffix_verifies(
            anchor.session_id,
            records,
            cached.sequence,
            prior,
            head_sequence,
            stored_head,
        )
    {
        return Ok(stored_head);
    }
    verify_full_head(anchor, records, stored_head)
}

/// Verify the records after `verified_sequence` against the stored head.
///
/// Returns `false` — never an error — when the cached anchor does not sit on
/// a stored record, or when the suffix does not verify. Both outcomes mean
/// only "this shortcut is not usable"; the caller re-verifies in full, so no
/// internal sentinel can ever reach a caller as a reason code.
fn suffix_verifies(
    session_id: SessionId,
    records: &[RecordEnvelope],
    verified_sequence: u64,
    verified_checksum: Digest,
    head_sequence: u64,
    stored_head: Option<Digest>,
) -> bool {
    let split = records
        .iter()
        .position(|record| record.sequence() > verified_sequence)
        .unwrap_or(records.len());
    // The prefix must still end on the exact record this process verified;
    // otherwise the cached proof is not about these bytes.
    let anchored = split
        .checked_sub(1)
        .and_then(|index| records.get(index))
        .is_some_and(|record| {
            record.sequence() == verified_sequence && record.checksum() == verified_checksum
        });
    if !anchored {
        return false;
    }
    let tail = records.get(split..).unwrap_or(&[]);
    verify_tail_records(
        session_id,
        tail,
        verified_sequence.saturating_add(1),
        verified_checksum,
        head_sequence,
        stored_head,
        FROM_SEQUENCE_WINDOW,
    )
    .is_ok()
}

/// One session's cache slot: the proof, plus the generation it belongs to.
///
/// The slot outlives the proof it held: [`VerifiedHeadCache::invalidate`]
/// clears `head` but keeps (and bumps) `generation`, so the invalidation is
/// still visible to a load that read the slot before it happened.
struct CacheSlot {
    /// Bumped by every invalidation of this session.
    generation: u64,
    /// The verified head, or `None` once invalidated.
    head: Option<VerifiedHead>,
}

/// A cache read: the proof (if any) plus the generation it was read at.
///
/// The generation must be handed back to [`VerifiedHeadCache::remember`],
/// which discards the write when an invalidation intervened.
#[derive(Clone, Copy, Debug)]
pub struct VerifiedRead {
    /// Generation of the slot at the time of the read.
    pub generation: u64,
    /// The cached proof, or `None` when there is none to use.
    pub head: Option<VerifiedHead>,
}

/// Per-session [`VerifiedHead`] cache with a generation guard.
///
/// Reads carry the generation they observed; a write presenting a stale
/// generation is discarded. Without that guard a load that read the cache
/// *before* a concurrent load found corruption could write its own head
/// afterwards, resurrecting a proof the invalidation was meant to destroy —
/// and, because that head anchors every later suffix verification, the
/// corrupt prefix would never be re-read for the life of the process.
///
/// Backends with a single serialized worker (sqlite) cannot observe such a
/// race, but use the same container so the semantics cannot drift.
///
/// The guarded sections are `HashMap` operations only; the `std::sync::Mutex`
/// is never held across an `.await`, and callers are expected to keep it that
/// way.
#[derive(Default)]
pub struct VerifiedHeadCache {
    /// One slot per session; absent means "never cached, generation 0".
    slots: Mutex<HashMap<SessionId, CacheSlot>>,
}

impl VerifiedHeadCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// Take the lock, recovering from poisoning rather than propagating it.
    ///
    /// The guarded sections cannot leave the map half-updated, so a panic
    /// elsewhere never makes the contents unsafe to read; and this crate
    /// forbids `unwrap`/`panic` in non-test code.
    fn lock(&self) -> MutexGuard<'_, HashMap<SessionId, CacheSlot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Read the cached verified head for `session_id`, with its generation.
    ///
    /// A session with no slot reads as generation 0 and no proof, which is
    /// the generation a first [`VerifiedHeadCache::remember`] must present.
    #[must_use]
    pub fn read(&self, session_id: SessionId) -> VerifiedRead {
        let map = self.lock();
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

    /// Record `head` as verified for `session_id`, unless an invalidation
    /// landed since `read.generation` was observed.
    pub fn remember(&self, session_id: SessionId, read: VerifiedRead, head: VerifiedHead) {
        let mut map = self.lock();
        let generation = map.get(&session_id).map_or(0, |slot| slot.generation);
        if generation != read.generation {
            // An invalidation intervened: this proof describes a journal
            // state that has since been called into question. Drop it; the
            // next load verifies in full.
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

    /// Drop any cached proof for `session_id`.
    ///
    /// The slot is kept as a tombstone with a bumped generation so in-flight
    /// loads that already read it cannot write over the invalidation.
    pub fn invalidate(&self, session_id: SessionId) {
        let mut map = self.lock();
        let generation = map.get(&session_id).map_or(0, |slot| slot.generation);
        map.insert(
            session_id,
            CacheSlot {
                generation: generation.saturating_add(1),
                head: None,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::append::build_committed_batch;
    use finstack_ai_kernel::{Digest, RecordEnvelope, SessionTag};
    use finstack_ai_runtime::StoreError;
    use finstack_ai_test::store_fixtures::{draft, id, request};

    fn chained_records() -> Vec<RecordEnvelope> {
        let first = build_committed_batch(&request(1, 1, 1, vec![draft(1, 1), draft(2, 1)]), None)
            .expect("first batch");
        let prior = first.records[1].checksum();
        let second = build_committed_batch(
            &request(2, 1, 3, vec![draft(3, 1), draft(4, 1)]),
            Some(prior),
        )
        .expect("second batch");
        first
            .records
            .iter()
            .chain(second.records.iter())
            .cloned()
            .collect()
    }

    fn head_of(records: &[RecordEnvelope]) -> (u64, Option<Digest>) {
        (
            records.last().map_or(0, RecordEnvelope::sequence),
            records.last().map(RecordEnvelope::checksum),
        )
    }

    fn head(sequence: u64) -> VerifiedHead {
        VerifiedHead {
            sequence,
            checksum: Some(Digest::raw_json(b"{}")),
        }
    }

    #[test]
    fn a_cached_head_verifies_only_the_appended_tail() {
        let records = chained_records();
        let (head_sequence, stored_head) = head_of(&records);
        let cached = VerifiedHead {
            sequence: 2,
            checksum: Some(records[1].checksum()),
        };
        assert_eq!(
            verify_head_against_cache(
                ChainAnchor::root(records[0].session_id()),
                &records,
                head_sequence,
                stored_head,
                Some(cached),
            )
            .expect("cached load"),
            stored_head
        );
    }

    #[test]
    fn a_stale_cached_head_falls_back_to_full_verification() {
        let records = chained_records();
        let (head_sequence, stored_head) = head_of(&records);
        let stale = VerifiedHead {
            sequence: 2,
            checksum: Some(Digest::raw_json(b"{}")),
        };
        // A garbage anchor is rejected, a real one accepted. (On this valid
        // journal both routes return the same head, so these two assertions
        // pin the predicate rather than the outcome.)
        assert!(
            !suffix_verifies(
                records[0].session_id(),
                &records,
                2,
                Digest::raw_json(b"{}"),
                head_sequence,
                stored_head
            ),
            "a garbage anchor must not be trusted"
        );
        assert!(
            suffix_verifies(
                records[0].session_id(),
                &records,
                2,
                records[1].checksum(),
                head_sequence,
                stored_head
            ),
            "the real record-2 checksum must anchor"
        );
        assert_eq!(
            verify_head_against_cache(
                ChainAnchor::root(records[0].session_id()),
                &records,
                head_sequence,
                stored_head,
                Some(stale),
            )
            .expect("full fallback"),
            stored_head
        );
    }

    /// The anchor predicate is what makes the cache fail *closed* when the
    /// record it was proved against has been replaced.
    ///
    /// The stored journal here is `[2', 3, 4]`: some other record now holds
    /// sequence 2, while records 3-4 still chain from the *original* record
    /// 2 — the checksum this process cached. The suffix after the cached
    /// sequence therefore verifies perfectly on its own, and only the
    /// requirement that the cached proof anchor on a stored record with that
    /// exact checksum catches the broken join. Drop that requirement and this
    /// corrupt journal loads successfully.
    #[test]
    fn a_cached_head_whose_anchor_record_was_replaced_is_not_trusted() {
        let records = chained_records();
        let anchor = records[1].checksum();
        // A different record occupying sequence 2, chained from record 1.
        let replacement = build_committed_batch(
            &request(3, 1, 2, vec![draft(99, 1)]),
            Some(records[0].checksum()),
        )
        .expect("replacement batch");
        let divergent: Vec<RecordEnvelope> = core::iter::once(replacement.records[0].clone())
            .chain(records[2..].iter().cloned())
            .collect();
        assert_ne!(divergent[0].checksum(), anchor, "sequence 2 really changed");
        let (head_sequence, stored_head) = head_of(&divergent);

        // The suffix on its own is intact: records 3-4 chain from the cached
        // checksum and land on the stored head.
        verify_tail_records(
            divergent[0].session_id(),
            &divergent[1..],
            3,
            anchor,
            head_sequence,
            stored_head,
            FROM_SEQUENCE_WINDOW,
        )
        .expect("the suffix alone verifies");
        // The anchor check is the only thing that rejects it…
        assert!(
            !suffix_verifies(
                divergent[0].session_id(),
                &divergent,
                2,
                anchor,
                head_sequence,
                stored_head,
            ),
            "a proof about a record that is no longer stored must be rejected"
        );
        // …and the load then fails closed on the full verification.
        assert!(matches!(
            verify_head_against_cache(
                ChainAnchor::root(divergent[0].session_id()),
                &divergent,
                head_sequence,
                stored_head,
                Some(VerifiedHead {
                    sequence: 2,
                    checksum: Some(anchor),
                })
            ),
            Err(StoreError::Integrity { .. })
        ));
    }

    #[test]
    fn a_cached_head_above_the_observed_head_is_ignored() {
        let records = chained_records();
        let truncated = records.get(..2).expect("prefix").to_vec();
        let (head_sequence, stored_head) = head_of(&truncated);
        let cached = VerifiedHead {
            sequence: 4,
            checksum: records.last().map(RecordEnvelope::checksum),
        };
        assert_eq!(
            verify_head_against_cache(
                ChainAnchor::root(truncated[0].session_id()),
                &truncated,
                head_sequence,
                stored_head,
                Some(cached),
            )
            .expect("full re-verify"),
            stored_head
        );
    }

    #[test]
    fn a_cached_head_cannot_mask_a_broken_head() {
        let records = chained_records();
        let (head_sequence, _) = head_of(&records);
        let broken = Some(Digest::raw_json(b"{}"));
        let cached = VerifiedHead {
            sequence: 2,
            checksum: Some(records[1].checksum()),
        };
        assert!(matches!(
            verify_head_against_cache(
                ChainAnchor::root(records[0].session_id()),
                &records,
                head_sequence,
                broken,
                Some(cached),
            ),
            Err(StoreError::Integrity {
                reason_code: "head_checksum_mismatch"
            })
        ));
    }

    #[test]
    fn a_head_read_at_the_current_generation_is_cached() {
        let cache = VerifiedHeadCache::new();
        let session = id::<SessionTag>(1);

        let read = cache.read(session);
        assert!(read.head.is_none());
        cache.remember(session, read, head(10));

        assert_eq!(cache.read(session).head, Some(head(10)));
    }

    #[test]
    fn a_write_racing_an_invalidation_is_discarded() {
        let cache = VerifiedHeadCache::new();
        let session = id::<SessionTag>(1);
        cache.remember(session, cache.read(session), head(10));

        let stale = cache.read(session);
        assert_eq!(stale.head, Some(head(10)));
        cache.invalidate(session);
        cache.remember(session, stale, head(10));

        let after = cache.read(session);
        assert!(
            after.head.is_none(),
            "the invalidation must survive a racing write"
        );
        cache.remember(session, after, head(12));
        assert_eq!(cache.read(session).head, Some(head(12)));
    }

    #[test]
    fn invalidating_an_uncached_session_still_blocks_a_racing_write() {
        let cache = VerifiedHeadCache::new();
        let session = id::<SessionTag>(1);

        let stale = cache.read(session);
        cache.invalidate(session);
        cache.remember(session, stale, head(10));

        assert!(cache.read(session).head.is_none());
    }

    #[test]
    fn invalidation_is_scoped_to_one_session() {
        let cache = VerifiedHeadCache::new();
        let (first, second) = (id::<SessionTag>(1), id::<SessionTag>(2));
        let read = cache.read(second);
        cache.invalidate(first);
        cache.remember(second, read, head(3));

        assert_eq!(cache.read(second).head, Some(head(3)));
        assert!(cache.read(first).head.is_none());
    }
}
