use finstack_ai_kernel::{
    ActiveToolCallStatus, EffectDeferred, EffectId, KernelState, ReconciliationPolicy, Timestamp,
};

/// A committed deferred effect's next poll deadline.
pub(crate) struct DuePoll {
    /// Deferred effect identity.
    pub effect_id: EffectId,
    /// Semantic time at which the effect becomes due for polling.
    pub at: Timestamp,
}

/// Return all committed pollable deferrals that have a next poll deadline.
///
/// Membership ignores `now`; the driver is responsible for filtering deadlines
/// whose `at` is less than or equal to `now`.
pub(crate) fn due_polls(state: &KernelState, now: Timestamp) -> Vec<DuePoll> {
    let _ = now;
    state
        .active_tool_batch
        .iter()
        .flat_map(|batch| batch.calls.iter())
        .filter_map(|call| {
            let ActiveToolCallStatus::Requested {
                deferred: Some(deferred),
                ..
            } = &call.status
            else {
                return None;
            };
            if !matches!(
                deferred.reconciliation,
                ReconciliationPolicy::Poll | ReconciliationPolicy::CallbackOrPoll
            ) {
                return None;
            }
            deferred.next_poll_at.map(|at| DuePoll {
                effect_id: deferred.effect_id,
                at,
            })
        })
        .collect()
}

/// Return whether a deferred effect has reached its inclusive expiry.
pub(crate) fn expired(deferred: &EffectDeferred, now: Timestamp) -> bool {
    deferred
        .expires_at
        .is_some_and(|expires_at| expires_at <= now)
}
