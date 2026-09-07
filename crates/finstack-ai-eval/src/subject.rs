//! Subject composition over existing SDK agents, including bounded host callbacks.
use crate::{
    Cell, EVAL_SUBJECT_LOCK_MISMATCH, EVAL_SUBJECT_UNBOUND, EvalError, SubjectId, TaskSet,
};
use finstack_ai::{Agent, AgentRunRequest, AttachmentInput};
use finstack_ai_kernel::Digest;
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

/// Prepared execution; the runner validates its bound lock and journal before dispatch.
pub struct PreparedAttempt {
    /// Existing resolved agent, with host-owned credentials and journal.
    pub agent: Agent,
    /// Model, input, settings, security and bounds for this one execution.
    pub request: AgentRunRequest,
}

/// Subject preparation is bounded by the runner's attempt timeout.
/// Preparation must not dispatch subject work: only the runner admits execution,
/// after persisting its actual session identity. Custom hosts may transform input
/// or choose settings here, and must honor cancellation when their future drops.
pub trait Subject: Send + Sync {
    /// Stable declared arm identity.
    fn id(&self) -> SubjectId;
    /// Prepare an attempt without dispatching it.
    fn prepare<'a>(
        &'a self,
        cell: &'a Cell,
    ) -> Pin<Box<dyn Future<Output = Result<PreparedAttempt, EvalError>> + Send + 'a>>;
}

/// Preflight identity and journal anchor for a subject implementation.
#[derive(Clone)]
pub struct SubjectBinding {
    /// Preparation implementation, potentially a host callback.
    pub subject: Arc<dyn Subject>,
    /// Resolved agent whose exact lock and journal every preparation must retain.
    pub agent: Agent,
}
impl SubjectBinding {
    /// Read the exact credential-free resolved lock.
    /// # Errors
    /// Returns `eval_subject_lock_mismatch` if no frozen SDK lock is available.
    pub fn lock_digest(&self) -> Result<Digest, EvalError> {
        agent_digest(&self.agent)
    }
    pub(crate) fn check(&self, prepared: &PreparedAttempt) -> Result<(), EvalError> {
        if self.lock_digest()? != agent_digest(&prepared.agent)?
            || !Arc::ptr_eq(&self.agent.journal_store(), &prepared.agent.journal_store())
        {
            return Err(lock_error());
        }
        Ok(())
    }
}
fn lock_error() -> EvalError {
    EvalError::new(
        EVAL_SUBJECT_LOCK_MISMATCH,
        "subject must retain its resolved lock and journal",
    )
}
pub(crate) fn agent_digest(agent: &Agent) -> Result<Digest, EvalError> {
    agent
        .resolved()
        .lock()
        .ok_or_else(lock_error)?
        .fingerprint()
        .map_err(|_| lock_error())
}

/// Reuse an immutable agent and request template across dataset cells.
/// Each attempt still gets its own actual session and lane.
pub struct SharedSubject {
    id: SubjectId,
    agent: Agent,
    template: AgentRunRequest,
    tasks: BTreeMap<Arc<str>, crate::TaskSample>,
}
impl SharedSubject {
    /// Bind an agent/template to a validated task set.
    /// # Errors
    /// Returns invalid configuration for duplicate IDs, invalid ID or oversized tasks.
    pub fn new(
        id: impl Into<SubjectId>,
        agent: Agent,
        template: AgentRunRequest,
        tasks: &TaskSet,
    ) -> Result<Self, EvalError> {
        let id = id.into();
        if !crate::config::valid_id(&id) || tasks.len() > crate::MAX_CELLS {
            return Err(crate::error::invalid());
        }
        let mut indexed = BTreeMap::new();
        for task in tasks {
            if task.input.len() > 65_536
                || task.attachments.len() > 8
                || indexed
                    .insert(Arc::clone(&task.task_id), task.clone())
                    .is_some()
            {
                return Err(crate::error::invalid());
            }
        }
        Ok(Self {
            id,
            agent,
            template,
            tasks: indexed,
        })
    }
    /// Convert this implementation into its preflight binding.
    #[must_use]
    pub fn bind(self) -> SubjectBinding {
        let agent = self.agent.clone();
        SubjectBinding {
            subject: Arc::new(self),
            agent,
        }
    }
}
impl Subject for SharedSubject {
    fn id(&self) -> SubjectId {
        Arc::clone(&self.id)
    }
    fn prepare<'a>(
        &'a self,
        cell: &'a Cell,
    ) -> Pin<Box<dyn Future<Output = Result<PreparedAttempt, EvalError>> + Send + 'a>> {
        Box::pin(async move {
            let sample = self
                .tasks
                .get(&cell.task_id)
                .ok_or_else(|| EvalError::new(EVAL_SUBJECT_UNBOUND, "subject task is missing"))?;
            let mut request = self.template.clone();
            request.input = Arc::clone(&sample.input);
            request.attachments = sample
                .attachments
                .iter()
                .cloned()
                .map(|artifact| AttachmentInput { artifact })
                .collect();
            Ok(PreparedAttempt {
                agent: self.agent.clone(),
                request,
            })
        })
    }
}
