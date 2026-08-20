mod provider;
mod record;
mod store;

use crate::record::*;
use finstack_ai_kernel::{Sensitivity, UNIX_EPOCH};
use std::sync::Arc;

pub(crate) fn sample_record(id: &str, tenant: &str) -> MemoryRecord {
    MemoryRecord {
        id: MemoryId::parse(id).unwrap(),
        scope: MemoryScope::try_new(tenant).unwrap(),
        keywords: Arc::from([Arc::<str>::from("alpha")]),
        body: MemoryBody::Inline(Arc::from("body text")),
        preview: Arc::from("body text"),
        sensitivity: Sensitivity::Internal,
        provenance: MemoryProvenance {
            source_session: None,
            source_run: None,
            source_ref: None,
            extraction: ExtractionMethod::Explicit,
            confidence: 80,
        },
        created_at: UNIX_EPOCH,
        last_confirmed_at: UNIX_EPOCH,
        supersedes: None,
        superseded_by: None,
        retention: RetentionPolicy::KeepUntilDeleted,
        tombstoned: false,
    }
}
