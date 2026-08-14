//! Store, session, lane, and snapshot journal bodies (TDD §12.2 / PR-039).

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::content::LABEL_MAX_BYTES;
use crate::digest::Digest;
use crate::ids::EntryId;
use crate::raw_json::Metadata;

/// Session-creation body. Labels live in [`Metadata`] (Architecture §9.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionCreated {
    metadata: Metadata,
}

impl SessionCreated {
    /// Construct a session-creation body.
    #[must_use]
    pub const fn new(metadata: Metadata) -> Self {
        Self { metadata }
    }

    /// Session metadata. Never grants authority.
    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

impl<'de> Deserialize<'de> for SessionCreated {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            metadata: Metadata,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self::new(wire.metadata))
    }
}

/// Lane-creation body. `main` is the first documented application key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LaneCreated {
    name: Arc<str>,
}

impl LaneCreated {
    /// Construct a lane-creation body.
    ///
    /// # Errors
    ///
    /// Returns [`SessionRecordError::InvalidLaneName`] when `name` is empty or
    /// longer than [`LABEL_MAX_BYTES`].
    pub fn try_new(name: impl Into<Arc<str>>) -> Result<Self, SessionRecordError> {
        let name = name.into();
        if name.is_empty() || name.len() > LABEL_MAX_BYTES {
            return Err(SessionRecordError::InvalidLaneName { len: name.len() });
        }
        Ok(Self { name })
    }

    /// Stable application lane key.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl<'de> Deserialize<'de> for LaneCreated {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            name: Arc<str>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.name).map_err(de::Error::custom)
    }
}

/// Lane leaf-pointer body. The envelope carries `lane_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LaneMoved {
    leaf_id: EntryId,
}

impl LaneMoved {
    /// Construct a lane-move body.
    #[must_use]
    pub const fn new(leaf_id: EntryId) -> Self {
        Self { leaf_id }
    }

    /// New leaf pointed to by the lane.
    #[must_use]
    pub fn leaf_id(&self) -> EntryId {
        self.leaf_id
    }
}

impl<'de> Deserialize<'de> for LaneMoved {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            leaf_id: EntryId,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self::new(wire.leaf_id))
    }
}

/// Disposable snapshot-written body. Bytes stay on `write_snapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SnapshotWritten {
    sequence: u64,
    digest: Digest,
}

impl SnapshotWritten {
    /// Construct a snapshot-written body.
    #[must_use]
    pub const fn new(sequence: u64, digest: Digest) -> Self {
        Self { sequence, digest }
    }

    /// Journal sequence covered by the snapshot.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Snapshot-state digest.
    #[must_use]
    pub const fn digest(&self) -> Digest {
        self.digest
    }
}

impl<'de> Deserialize<'de> for SnapshotWritten {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            sequence: u64,
            digest: Digest,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self::new(wire.sequence, wire.digest))
    }
}

/// Session/lane/snapshot journal construction errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionRecordError {
    /// Lane name is empty or exceeds [`LABEL_MAX_BYTES`].
    #[error("lane name length {len} is empty or exceeds {LABEL_MAX_BYTES}")]
    InvalidLaneName {
        /// Rejected name length.
        len: usize,
    },
    /// Structural session/lane/snapshot records must omit `run_id`.
    #[error("session, lane, and snapshot records must omit run_id")]
    StructuralRunIdPresent,
}

impl SessionRecordError {
    /// Stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidLaneName { .. } => "invalid_lane_name",
            Self::StructuralRunIdPresent => "structural_run_id_present",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::LABEL_MAX_BYTES;
    use crate::ids::{EntryId, Id};

    fn entry(ordinal: u64) -> EntryId {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    #[test]
    fn lane_name_rejects_empty_and_oversize() {
        assert!(LaneCreated::try_new("main").is_ok());
        assert!(matches!(
            LaneCreated::try_new(""),
            Err(SessionRecordError::InvalidLaneName { len: 0 })
        ));
        let over = "x".repeat(LABEL_MAX_BYTES + 1);
        assert!(matches!(
            LaneCreated::try_new(over),
            Err(SessionRecordError::InvalidLaneName { .. })
        ));
    }

    #[test]
    fn structural_bodies_round_trip_json() {
        let session = SessionCreated::new(Metadata::empty());
        let lane = LaneCreated::try_new("main").expect("lane");
        let moved = LaneMoved::new(entry(1));
        let snapshot = SnapshotWritten::new(1, Digest::raw_json(b"snap"));
        for value in [
            serde_json::to_string(&session).expect("session"),
            serde_json::to_string(&lane).expect("lane"),
            serde_json::to_string(&moved).expect("moved"),
            serde_json::to_string(&snapshot).expect("snapshot"),
        ] {
            assert!(value.contains('{'));
        }
        let decoded: LaneCreated = serde_json::from_str(r#"{"name":"main"}"#).expect("decode");
        assert_eq!(decoded.name(), "main");
    }
}
