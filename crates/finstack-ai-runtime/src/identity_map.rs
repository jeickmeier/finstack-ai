//! Host-owned external identity mapping hooks.
//!
//! The map is not a port, not a journal family, and never grants authority.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{LaneId, SessionId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Channel/account/thread key that resolves to one session lane.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ExternalIdentityKey {
    /// Channel or adapter name.
    pub channel: Arc<str>,
    /// Account identity on that channel.
    pub account: Arc<str>,
    /// Conversation or thread identity.
    pub thread: Arc<str>,
}

impl<'de> Deserialize<'de> for ExternalIdentityKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            channel: Arc<str>,
            account: Arc<str>,
            thread: Arc<str>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.channel, wire.account, wire.thread).map_err(serde::de::Error::custom)
    }
}

impl ExternalIdentityKey {
    /// Construct a key after rejecting empty, oversized, or NUL labels.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityMapError::InvalidKey`] when any field is empty, longer
    /// than the kernel label byte limit, or contains a NUL byte.
    pub fn try_new(
        channel: impl Into<Arc<str>>,
        account: impl Into<Arc<str>>,
        thread: impl Into<Arc<str>>,
    ) -> Result<Self, IdentityMapError> {
        let key = Self {
            channel: channel.into(),
            account: account.into(),
            thread: thread.into(),
        };
        key.validate()?;
        Ok(key)
    }

    fn validate(&self) -> Result<(), IdentityMapError> {
        for (field, value) in [
            ("channel", self.channel.as_ref()),
            ("account", self.account.as_ref()),
            ("thread", self.thread.as_ref()),
        ] {
            if !finstack_ai_kernel::label_is_valid(value) {
                return Err(IdentityMapError::InvalidKey { field });
            }
        }
        Ok(())
    }
}

/// Fail-closed identity-map errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdentityMapError {
    /// A key field is empty, oversized, or contains a NUL.
    #[error("external identity key field {field} is invalid")]
    InvalidKey {
        /// Invalid field name.
        field: &'static str,
    },
    /// The key is already bound to a different session or lane.
    #[error("external identity is already bound to a different session lane")]
    Conflict,
}

/// Host-owned `(channel, account, thread) → (session_id, lane_id)` map.
pub trait ExternalIdentityMap: Send + Sync {
    /// Resolve one previously bound key.
    fn resolve(&self, key: &ExternalIdentityKey) -> Option<(SessionId, LaneId)>;

    /// Bind a key. Equal bind is idempotent; a different target fails closed.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityMapError::Conflict`] when the key is already bound to
    /// another pair, or [`IdentityMapError::InvalidKey`] when the key is invalid.
    fn bind(
        &self,
        key: ExternalIdentityKey,
        session_id: SessionId,
        lane_id: LaneId,
    ) -> Result<(), IdentityMapError>;
}

/// In-process identity map for tests and single-process hosts.
#[derive(Debug, Default)]
pub struct MemoryExternalIdentityMap {
    inner: Mutex<BTreeMap<ExternalIdentityKey, (SessionId, LaneId)>>,
}

impl MemoryExternalIdentityMap {
    /// Empty map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ExternalIdentityMap for MemoryExternalIdentityMap {
    fn resolve(&self, key: &ExternalIdentityKey) -> Option<(SessionId, LaneId)> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.get(key).copied())
    }

    fn bind(
        &self,
        key: ExternalIdentityKey,
        session_id: SessionId,
        lane_id: LaneId,
    ) -> Result<(), IdentityMapError> {
        key.validate()?;
        let mut inner = self.inner.lock().map_err(|_| IdentityMapError::Conflict)?;
        match inner.get(&key) {
            Some(existing) if *existing == (session_id, lane_id) => Ok(()),
            Some(_) => Err(IdentityMapError::Conflict),
            None => {
                inner.insert(key, (session_id, lane_id));
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::{Id, IdTag};

    fn id<T: IdTag>(ordinal: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    #[test]
    fn session_identity_bind_is_idempotent_and_conflicts_fail_closed() {
        let map = MemoryExternalIdentityMap::new();
        let key = ExternalIdentityKey::try_new("slack", "acct", "thread-1").expect("key");
        map.bind(key.clone(), id(1), id(2)).expect("bind");
        map.bind(key.clone(), id(1), id(2)).expect("equal");
        assert_eq!(map.resolve(&key), Some((id(1), id(2))));
        assert_eq!(map.bind(key, id(1), id(3)), Err(IdentityMapError::Conflict));
    }

    #[test]
    fn session_identity_rejects_empty_keys() {
        assert!(matches!(
            ExternalIdentityKey::try_new("", "acct", "thread"),
            Err(IdentityMapError::InvalidKey { field: "channel" })
        ));
    }
}
