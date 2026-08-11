//! Deterministic post-commit dispatch control for native runtime tests.

use finstack_ai_kernel::{EffectId, PostCommitAction};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

/// Kind of committed post-commit action paused by manual drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualDriveAction {
    /// Begin executing one committed effect request.
    Execute,
    /// Signal cancellation for one committed effect.
    Cancel,
}

/// Stable description of one committed action waiting immediately before dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualDriveEffect {
    /// Committed effect identity.
    pub effect_id: EffectId,
    /// Dispatch operation waiting for an explicit permit.
    pub action: ManualDriveAction,
}

impl From<PostCommitAction> for ManualDriveEffect {
    fn from(action: PostCommitAction) -> Self {
        match action {
            PostCommitAction::ExecuteEffect { effect_id } => Self {
                effect_id,
                action: ManualDriveAction::Execute,
            },
            PostCommitAction::CancelEffect { effect_id } => Self {
                effect_id,
                action: ManualDriveAction::Cancel,
            },
        }
    }
}

/// Manual-drive configuration error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ManualDriveError {
    /// The bounded pending-action queue must have at least one slot.
    #[error("manual-drive capacity must be non-zero")]
    ZeroCapacity,
}

/// Exclusive receiver for committed actions paused before external dispatch.
pub struct ManualDriveController {
    receiver: mpsc::Receiver<PausedDispatch>,
}

impl ManualDriveController {
    /// Wait for the next committed action that is ready for external dispatch.
    ///
    /// Returns `None` after the coordinator-side gate has closed.
    pub async fn next_effect(&mut self) -> Option<ManualDrivePermit> {
        let paused = self.receiver.recv().await?;
        Some(ManualDrivePermit {
            effect: paused.effect,
            release: Some(paused.release),
        })
    }
}

/// One paused action whose dispatch proceeds only after explicit release.
pub struct ManualDrivePermit {
    effect: ManualDriveEffect,
    release: Option<oneshot::Sender<()>>,
}

impl ManualDrivePermit {
    /// Inspect the committed action without releasing it.
    #[must_use]
    pub const fn effect(&self) -> ManualDriveEffect {
        self.effect
    }

    /// Allow this one action to cross the external-dispatch boundary.
    pub fn continue_dispatch(mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

pub(crate) struct ManualDriveGate {
    sender: mpsc::Sender<PausedDispatch>,
}

impl ManualDriveGate {
    pub(crate) async fn pause(&self, action: PostCommitAction) -> Result<(), &'static str> {
        let (release, waiting) = oneshot::channel();
        self.sender
            .send(PausedDispatch {
                effect: action.into(),
                release,
            })
            .await
            .map_err(|_| "manual_drive_closed")?;
        waiting.await.map_err(|_| "manual_drive_permit_dropped")
    }
}

struct PausedDispatch {
    effect: ManualDriveEffect,
    release: oneshot::Sender<()>,
}

pub(crate) fn manual_drive(
    capacity: usize,
) -> Result<(ManualDriveGate, ManualDriveController), ManualDriveError> {
    if capacity == 0 {
        return Err(ManualDriveError::ZeroCapacity);
    }
    let (sender, receiver) = mpsc::channel(capacity);
    Ok((
        ManualDriveGate { sender },
        ManualDriveController { receiver },
    ))
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::Id;

    use super::*;

    #[tokio::test]
    async fn one_explicit_permit_releases_exactly_one_action() {
        let effect_id = Id::from_bytes([7; 16]);
        let (gate, mut controller) = manual_drive(1).expect("manual drive");
        let pause = tokio::spawn(async move {
            gate.pause(PostCommitAction::ExecuteEffect { effect_id })
                .await
        });

        let permit = controller.next_effect().await.expect("paused effect");
        assert_eq!(
            permit.effect(),
            ManualDriveEffect {
                effect_id,
                action: ManualDriveAction::Execute,
            }
        );
        assert!(!pause.is_finished());
        permit.continue_dispatch();
        assert_eq!(pause.await.expect("join"), Ok(()));
    }

    #[tokio::test]
    async fn dropped_permit_fails_closed() {
        let effect_id = Id::from_bytes([8; 16]);
        let (gate, mut controller) = manual_drive(1).expect("manual drive");
        let pause = tokio::spawn(async move {
            gate.pause(PostCommitAction::CancelEffect { effect_id })
                .await
        });

        drop(controller.next_effect().await.expect("paused effect"));
        assert_eq!(
            pause.await.expect("join"),
            Err("manual_drive_permit_dropped")
        );
    }

    #[test]
    fn zero_capacity_is_rejected() {
        assert!(matches!(
            manual_drive(0),
            Err(ManualDriveError::ZeroCapacity)
        ));
    }
}
