//! Stateful `Lane` run, suspend, and resume orchestration.

use finstack_ai_runtime::session::SessionError;
use std::sync::atomic::{AtomicU64, Ordering};

use super::handle::Agent;
use super::prepare::session_error;
use super::run::AgentRun;
use super::types::{AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, AgentRunRequest};

#[derive(Clone, Copy, PartialEq, Eq)]
enum LaneState {
    Running,
    Suspending,
    Suspended,
    Resuming,
}

pub(crate) struct LaneLive {
    generation: u64,
    state: LaneState,
    run: Option<AgentRun>,
}

fn next_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn run_is_settled(run: &AgentRun) -> bool {
    run.inner.result.lock().is_ok_and(|result| result.is_some())
}

fn live_is_releasable(live: &LaneLive) -> bool {
    live.state == LaneState::Running && live.run.as_ref().is_some_and(run_is_settled)
}

fn reserve_run(lane: &crate::Lane) -> Result<u64, AgentRunError> {
    let mut lanes = lane
        .session()
        .live_lanes()
        .lock()
        .map_err(|_| AgentRunError::runtime_message("lane driver lock is poisoned"))?;
    if lanes.get(&lane.lane_id()).is_some_and(live_is_releasable) {
        lanes.remove(&lane.lane_id());
    }
    if lanes.contains_key(&lane.lane_id()) {
        return Err(session_error(&SessionError::LaneBusy));
    }
    let generation = next_generation();
    lanes.insert(
        lane.lane_id(),
        LaneLive {
            generation,
            state: LaneState::Running,
            run: None,
        },
    );
    Ok(generation)
}

fn attach_run(lane: &crate::Lane, generation: u64, run: AgentRun) -> Result<(), AgentRunError> {
    let mut lanes = lane
        .session()
        .live_lanes()
        .lock()
        .map_err(|_| AgentRunError::runtime_message("lane driver lock is poisoned"))?;
    let live = lanes
        .get_mut(&lane.lane_id())
        .filter(|live| live.generation == generation)
        .ok_or_else(|| AgentRunError::runtime_message("lane reservation was superseded"))?;
    live.run = Some(run);
    Ok(())
}

pub(crate) fn live_run(lane: &crate::Lane) -> Result<Option<AgentRun>, SessionError> {
    let lanes = lane
        .session()
        .live_lanes()
        .lock()
        .map_err(|_| SessionError::Poisoned)?;
    Ok(lanes
        .get(&lane.lane_id())
        .filter(|live| live.state == LaneState::Running)
        .and_then(|live| live.run.clone()))
}

fn abandon(lane: &crate::Lane, generation: u64) {
    if let Ok(mut lanes) = lane.session().live_lanes().lock()
        && lanes
            .get(&lane.lane_id())
            .is_some_and(|live| live.generation == generation)
    {
        lanes.remove(&lane.lane_id());
    }
}

fn begin_suspend(lane: &crate::Lane) -> Result<Option<(u64, Option<AgentRun>)>, SessionError> {
    let mut lanes = lane
        .session()
        .live_lanes()
        .lock()
        .map_err(|_| SessionError::Poisoned)?;
    let Some(live) = lanes.get_mut(&lane.lane_id()) else {
        return Ok(None);
    };
    match live.state {
        LaneState::Suspended => return Ok(None),
        LaneState::Running => {}
        LaneState::Suspending | LaneState::Resuming => return Err(SessionError::LaneBusy),
    }
    if live.run.is_none() {
        return Err(SessionError::LaneBusy);
    }
    live.state = LaneState::Suspending;
    Ok(Some((live.generation, live.run.clone())))
}

fn finish_suspend(lane: &crate::Lane, generation: u64) -> Result<(), SessionError> {
    let mut lanes = lane
        .session()
        .live_lanes()
        .lock()
        .map_err(|_| SessionError::Poisoned)?;
    if let Some(live) = lanes
        .get_mut(&lane.lane_id())
        .filter(|live| live.generation == generation)
    {
        live.state = LaneState::Suspended;
    }
    Ok(())
}

fn begin_resume(lane: &crate::Lane) -> Result<(u64, Option<AgentRun>), AgentRunError> {
    let mut lanes = lane
        .session()
        .live_lanes()
        .lock()
        .map_err(|_| AgentRunError::runtime_message("lane driver lock is poisoned"))?;
    if let Some(live) = lanes.get_mut(&lane.lane_id()) {
        if live.state != LaneState::Suspended {
            return Err(session_error(&SessionError::LaneBusy));
        }
        live.state = LaneState::Resuming;
        return Ok((live.generation, live.run.clone()));
    }
    Err(AgentRunError::configuration(
        AGENT_RUN_INVALID_CONFIGURATION,
        "lane has no in-process parked controller; use explicit workflow recovery for a journal-only run",
    ))
}

fn finish_resume(lane: &crate::Lane, generation: u64) -> Result<(), AgentRunError> {
    let mut lanes = lane
        .session()
        .live_lanes()
        .lock()
        .map_err(|_| AgentRunError::runtime_message("lane driver lock is poisoned"))?;
    let live = lanes
        .get_mut(&lane.lane_id())
        .filter(|live| live.generation == generation)
        .ok_or_else(|| AgentRunError::runtime_message("lane resume was superseded"))?;
    live.state = LaneState::Running;
    Ok(())
}

fn rollback_resume(lane: &crate::Lane, generation: u64) {
    if let Ok(mut lanes) = lane.session().live_lanes().lock()
        && let Some(live) = lanes
            .get_mut(&lane.lane_id())
            .filter(|live| live.generation == generation)
    {
        live.state = LaneState::Suspended;
    }
}

impl crate::Lane {
    /// Start a new root run on this idle lane.
    ///
    /// `request.input` is the user text. The call uses `Agent::start_on_lane`
    /// so `AcceptRun` lands on this lane rather than bootstrapping a session.
    ///
    /// # Errors
    ///
    /// Returns a tenant mismatch, busy-lane, or agent configuration/runtime
    /// failure.
    pub fn run(&self, agent: &Agent, request: AgentRunRequest) -> Result<AgentRun, AgentRunError> {
        if request.security.tenant_scope() != self.session().tenant_scope() {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "run security tenant does not match the session tenant",
            ));
        }
        let generation = reserve_run(self)?;
        match agent.start_on_lane(self, request) {
            Ok(run) => {
                if let Err(error) = attach_run(self, generation, run.clone()) {
                    abandon(self, generation);
                    Err(error)
                } else {
                    Ok(run)
                }
            }
            Err(error) => {
                abandon(self, generation);
                Err(error)
            }
        }
    }

    /// Park the in-process driver without dropping the journal.
    ///
    /// The lane's active or suspended run remains durable. [`Self::resume`]
    /// respawns [`finstack_ai_runtime::run::RunTaskOwner`].
    ///
    /// # Errors
    ///
    /// Returns a recover or lock failure.
    pub async fn suspend(&self) -> Result<(), SessionError> {
        let Some((generation, run)) = begin_suspend(self)? else {
            return Ok(());
        };
        if let Some(run) = run
            && let Ok(handle) = run.runtime_handle().await
            && run.inner.lifecycle.request_park()
        {
            run.park_events().map_err(|_| SessionError::Poisoned)?;
            handle.shutdown();
            run.inner.lifecycle.wait_parked().await;
        }
        finish_suspend(self, generation)
    }

    /// Rebuild the parked owner and continue the original SDK run controller.
    ///
    /// The in-process controller retains the accepted request, context seed and
    /// deadline. Resuming does not append a second user message or accept a new run.
    ///
    /// # Errors
    ///
    /// Returns an error when no in-process parked controller exists, the agent
    /// lock differs, or rebuilding the owner fails. Journal-only recovery uses
    /// the explicit runtime workflow API.
    pub async fn resume(&self, agent: &Agent) -> Result<(), AgentRunError> {
        Box::pin(self.resume_inner(agent)).await
    }

    async fn resume_inner(&self, agent: &Agent) -> Result<(), AgentRunError> {
        let (generation, run) = begin_resume(self)?;
        let result = Box::pin(async {
            let run = run.as_ref().ok_or_else(|| AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "lane has no in-process parked controller; use explicit workflow recovery for a journal-only run",
            ))?;
            let digest = agent.resolved.lock().ok_or_else(|| AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION, "resume requires a resolved agent lock",
            ))?.fingerprint().map_err(|error| AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string()))?;
            if run.inner.lifecycle.lock_digest != Some(digest) {
                return Err(AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, "resume agent differs from the accepted agent lock"));
            }
            if !run.inner.lifecycle.resume() {
                return Err(AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, "lane run is no longer parked"));
            }
            run.inner.lifecycle.wait_resumed().await;
            // Startup failures replace the retained runtime handle.
            run.runtime_handle().await?;
            run.recover_children().await?;
            finish_resume(self, generation)
        })
        .await;
        if result.is_err() {
            rollback_resume(self, generation);
        }
        result
    }

    #[cfg(all(test, feature = "native-tokio"))]
    pub(crate) fn workflow_owner_is_live(&self) -> bool {
        let Ok(guard) = self.session().live_lanes().lock() else {
            return false;
        };
        guard
            .get(&self.lane_id())
            .filter(|live| live.state == LaneState::Running)
            .and_then(|live| live.run.as_ref())
            .is_some_and(|run| run.inner.result.lock().is_ok_and(|result| result.is_none()))
    }
}
