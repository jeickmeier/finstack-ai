use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{ComponentId, Digest, LaneId, SessionId};
use finstack_ai_runtime::CompactionCheckpoint;

use super::types::{AGENT_RUN_INVALID_CONFIGURATION, AgentRunError};

/// Bounded process-local cache policy for validated compaction checkpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryCachePolicy {
    max_entries: usize,
    max_bytes: usize,
}

impl HistoryCachePolicy {
    /// Default entry bound.
    pub const DEFAULT_MAX_ENTRIES: usize = 64;
    /// Default aggregate serialized-byte bound.
    pub const DEFAULT_MAX_BYTES: usize = 16 * 1024 * 1024;
    /// Largest configurable entry bound.
    pub const MAX_ENTRIES: usize = 4_096;
    /// Largest configurable aggregate serialized-byte bound.
    pub const MAX_BYTES: usize = 256 * 1024 * 1024;

    /// Construct a validated cache policy.
    ///
    /// # Errors
    ///
    /// Returns an invalid-configuration error when either bound exceeds its
    /// fixed maximum or only one bound is zero.
    pub fn try_new(max_entries: usize, max_bytes: usize) -> Result<Self, AgentRunError> {
        if max_entries > Self::MAX_ENTRIES
            || max_bytes > Self::MAX_BYTES
            || (max_entries == 0) != (max_bytes == 0)
        {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "history cache limits are invalid",
            ));
        }
        Ok(Self {
            max_entries,
            max_bytes,
        })
    }

    /// Disable process-local checkpoint reuse.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            max_entries: 0,
            max_bytes: 0,
        }
    }

    /// Maximum retained checkpoint entries.
    #[must_use]
    pub const fn max_entries(self) -> usize {
        self.max_entries
    }

    /// Maximum aggregate serialized checkpoint bytes.
    #[must_use]
    pub const fn max_bytes(self) -> usize {
        self.max_bytes
    }
}

impl Default for HistoryCachePolicy {
    fn default() -> Self {
        Self {
            max_entries: Self::DEFAULT_MAX_ENTRIES,
            max_bytes: Self::DEFAULT_MAX_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HistoryCacheKey {
    session_id: SessionId,
    lane_id: LaneId,
    model_context_profile_digest: Digest,
    component_id: ComponentId,
    configuration_digest: Digest,
    strategy_id: Arc<str>,
    strategy_version: u32,
}

struct HistoryCacheEntry {
    key: HistoryCacheKey,
    checkpoint: CompactionCheckpoint,
    serialized_bytes: usize,
    last_used: u64,
}

pub(super) struct HistoryCache {
    policy: HistoryCachePolicy,
    entries: Vec<HistoryCacheEntry>,
    total_bytes: usize,
    clock: u64,
}

impl HistoryCache {
    pub(super) fn shared(policy: HistoryCachePolicy) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            policy,
            entries: Vec::new(),
            total_bytes: 0,
            clock: 0,
        }))
    }

    pub(super) fn candidate(
        &mut self,
        session_id: SessionId,
        lane_id: LaneId,
        profile_digest: Digest,
    ) -> Option<CompactionCheckpoint> {
        self.clock = self.clock.saturating_add(1);
        let entry = self
            .entries
            .iter_mut()
            .filter(|entry| {
                entry.key.session_id == session_id
                    && entry.key.lane_id == lane_id
                    && entry.key.model_context_profile_digest == profile_digest
            })
            .max_by_key(|entry| entry.last_used)?;
        entry.last_used = self.clock;
        Some(entry.checkpoint.clone())
    }

    pub(super) fn insert(
        &mut self,
        session_id: SessionId,
        lane_id: LaneId,
        checkpoint: CompactionCheckpoint,
    ) {
        if self.policy == HistoryCachePolicy::disabled() {
            return;
        }
        let Ok(serialized) = serde_json_canonicalizer::to_vec(&checkpoint) else {
            return;
        };
        let serialized_bytes = serialized.len();
        if serialized_bytes > self.policy.max_bytes {
            return;
        }
        self.clock = self.clock.saturating_add(1);
        let key = HistoryCacheKey {
            session_id,
            lane_id,
            model_context_profile_digest: checkpoint.model_context_profile_digest,
            component_id: checkpoint.component_id.clone(),
            configuration_digest: checkpoint.configuration_digest,
            strategy_id: Arc::clone(&checkpoint.strategy_id),
            strategy_version: checkpoint.strategy_version,
        };
        if let Some(index) = self.entries.iter().position(|entry| entry.key == key) {
            self.total_bytes = self
                .total_bytes
                .saturating_sub(self.entries[index].serialized_bytes);
            self.entries.remove(index);
        }
        self.total_bytes = self.total_bytes.saturating_add(serialized_bytes);
        self.entries.push(HistoryCacheEntry {
            key,
            checkpoint,
            serialized_bytes,
            last_used: self.clock,
        });
        while self.entries.len() > self.policy.max_entries
            || self.total_bytes > self.policy.max_bytes
        {
            let Some((index, _)) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.last_used)
            else {
                break;
            };
            let removed = self.entries.remove(index);
            self.total_bytes = self.total_bytes.saturating_sub(removed.serialized_bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::{EntryId, Sensitivity};
    use finstack_ai_runtime::CompactedSummary;

    fn session(byte: u8) -> SessionId {
        SessionId::from_bytes([byte; 16])
    }

    fn lane(byte: u8) -> LaneId {
        LaneId::from_bytes([byte; 16])
    }

    fn checkpoint(byte: u8, profile: Digest) -> CompactionCheckpoint {
        CompactionCheckpoint {
            component_id: ComponentId::parse(format!("test.compactor-{byte}")).expect("component"),
            strategy_id: Arc::from("test.strategy"),
            strategy_version: 1,
            configuration_digest: Digest::raw_json(&[byte]),
            model_context_profile_digest: profile,
            covered_through_entry_id: EntryId::from_bytes([byte; 16]),
            source_digest: Digest::raw_json(&[byte, 1]),
            summary: CompactedSummary::Inline(Arc::from([])),
            summary_digest: Digest::raw_json(&[byte, 2]),
            sensitivity: Sensitivity::Internal,
        }
    }

    #[test]
    fn policy_enforces_fixed_bounds_and_disabled_pairing() {
        assert_eq!(HistoryCachePolicy::default().max_entries(), 64);
        assert_eq!(HistoryCachePolicy::default().max_bytes(), 16 * 1024 * 1024);
        assert!(HistoryCachePolicy::try_new(1, 0).is_err());
        assert!(HistoryCachePolicy::try_new(0, 1).is_err());
        assert!(HistoryCachePolicy::try_new(4_097, 1).is_err());
        assert!(HistoryCachePolicy::try_new(1, 256 * 1024 * 1024 + 1).is_err());
        assert_eq!(HistoryCachePolicy::disabled().max_entries(), 0);
    }

    #[test]
    fn cache_is_lru_and_honors_entry_byte_and_disabled_bounds() {
        let profile = Digest::raw_json(b"profile");
        let mut cache = HistoryCache {
            policy: HistoryCachePolicy::try_new(2, 16_384).expect("policy"),
            entries: Vec::new(),
            total_bytes: 0,
            clock: 0,
        };
        cache.insert(session(1), lane(1), checkpoint(1, profile));
        cache.insert(session(2), lane(2), checkpoint(2, profile));
        assert!(cache.candidate(session(1), lane(1), profile).is_some());
        cache.insert(session(3), lane(3), checkpoint(3, profile));
        assert!(cache.candidate(session(2), lane(2), profile).is_none());
        assert!(cache.candidate(session(1), lane(1), profile).is_some());
        assert!(cache.candidate(session(3), lane(3), profile).is_some());

        let mut tiny = HistoryCache {
            policy: HistoryCachePolicy::try_new(1, 1).expect("tiny policy"),
            entries: Vec::new(),
            total_bytes: 0,
            clock: 0,
        };
        tiny.insert(session(1), lane(1), checkpoint(1, profile));
        assert!(tiny.entries.is_empty());

        let mut disabled = HistoryCache {
            policy: HistoryCachePolicy::disabled(),
            entries: Vec::new(),
            total_bytes: 0,
            clock: 0,
        };
        disabled.insert(session(1), lane(1), checkpoint(1, profile));
        assert!(disabled.entries.is_empty());
    }
}
