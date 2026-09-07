//! Only reconstruction inputs absent from the authoritative accepted history.

use finstack_ai_kernel::{ArtifactRef, Digest, EntryId, OperationLocator};
use finstack_ai_runtime::ports::model::{ModelName, ModelSettings};
use finstack_ai_runtime::session::LaneRunContext;
use finstack_ai_workflow_worker::{RecoveryRegistration, RecoveryStore};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::DurableHostError;

/// Version one recovery payload. Credentials live in registered host handles.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Descriptor {
    pub(super) version: u32,
    pub(super) locator: OperationLocator,
    pub(super) workflow_kind: Arc<str>,
    pub(super) lock_fingerprint: Digest,
    pub(super) output_schema: Option<finstack_ai_kernel::SchemaRef>,
    pub(super) model: ModelName,
    pub(super) settings: ModelSettings,
    pub(super) model_profile_digest: Digest,
    pub(super) source_leaf_id: EntryId,
    pub(super) context_sequence: u64,
    pub(super) context_checksum: Digest,
    pub(super) required_artifacts: Arc<[ArtifactRef]>,
}

impl Descriptor {
    pub(super) fn save(&self, store: &dyn RecoveryStore) -> Result<(), DurableHostError> {
        let bytes = serde_json_canonicalizer::to_vec(self)
            .map_err(|_| DurableHostError::new("durable_descriptor_encode"))?;
        store.insert_recovery(&RecoveryRegistration {
            locator: self.locator.clone(),
            descriptor: bytes.into(),
        })?;
        Ok(())
    }

    pub(super) fn load(
        store: &dyn RecoveryStore,
        locator: &OperationLocator,
    ) -> Result<Self, DurableHostError> {
        let row = store
            .load_recovery(locator)?
            .ok_or_else(|| DurableHostError::new("durable_descriptor_missing"))?;
        let descriptor: Self = serde_json::from_slice(&row.descriptor)
            .map_err(|_| DurableHostError::new("durable_descriptor_invalid"))?;
        if descriptor.version != 1 {
            return Err(DurableHostError::new("durable_descriptor_version"));
        }
        if descriptor.locator != *locator
            || descriptor.workflow_kind.is_empty()
            || descriptor.context_sequence == 0
        {
            return Err(DurableHostError::new("durable_descriptor_binding"));
        }
        Ok(descriptor)
    }

    pub(super) fn context(&self, messages: Arc<[finstack_ai_kernel::Message]>) -> LaneRunContext {
        LaneRunContext {
            messages,
            source_leaf_id: self.source_leaf_id,
            journal_sequence: self.context_sequence,
            head_checksum: Some(self.context_checksum),
        }
    }
}
