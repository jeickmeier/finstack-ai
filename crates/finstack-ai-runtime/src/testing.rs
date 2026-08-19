//! Hidden test-support surface. Not a production API.

use finstack_ai_kernel::{EffectId, KernelState, Timestamp};

/// Expose committed poll derivation to cross-crate restore tests.
#[cfg(feature = "native-tokio")]
#[doc(hidden)]
#[must_use]
pub fn due_polls(state: &KernelState, now: Timestamp) -> Vec<(EffectId, Timestamp)> {
    crate::settlement::due_polls(state, now)
        .into_iter()
        .map(|poll| (poll.effect_id, poll.at))
        .collect()
}

#[cfg(feature = "native-tokio")]
#[doc(hidden)]
pub use crate::native::manual_drive::{
    ManualDriveAction, ManualDriveController, ManualDriveEffect, ManualDriveError,
    ManualDrivePermit,
};
