//! Preparation callbacks cannot bypass Rust reservation and session admission.
use super::{error, invalid};
use crate::{agent::PyAgent, callbacks::PythonCallback};
use finstack_ai::{Agent, AgentRunRequest};
use finstack_ai_eval::{
    Cell, EvalError, PreparedAttempt, SharedSubject, Subject, SubjectBinding, SubjectId, TaskSet,
};
use pyo3::prelude::*;
use serde::Deserialize;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

#[pyclass(module = "finstack_ai._finstack_ai", name = "_EvalSubject", frozen)]
pub(crate) struct PyEvalSubject {
    id: Arc<str>,
    agent: Agent,
    template: AgentRunRequest,
    callback: Option<Arc<PythonCallback>>,
}
#[pymethods]
impl PyEvalSubject {
    #[new]
    #[pyo3(signature = (subject_id, agent, *, tenant_scope = "python-local", prepare = None, callback_timeout_seconds = 30.0))]
    fn new(
        py: Python<'_>,
        subject_id: String,
        agent: &Bound<'_, PyAgent>,
        tenant_scope: &str,
        prepare: Option<Py<PyAny>>,
        callback_timeout_seconds: f64,
    ) -> PyResult<Self> {
        let agent = agent.borrow();
        let template = template(&agent, tenant_scope)?;
        SharedSubject::new(
            subject_id.clone(),
            agent.inner.as_ref().clone(),
            template.clone(),
            &vec![],
        )
        .map_err(error)?;
        let callback = prepare
            .map(|callable| {
                PythonCallback::try_new(py, callable, callback_timeout_seconds).map(Arc::new)
            })
            .transpose()?;
        Ok(Self {
            id: Arc::from(subject_id),
            agent: agent.inner.as_ref().clone(),
            template,
            callback,
        })
    }
}
pub(super) fn template(agent: &PyAgent, tenant_scope: &str) -> PyResult<AgentRunRequest> {
    crate::run::run_request(
        &agent.model,
        "eval".to_owned(),
        agent.default_timeout_seconds,
        finstack_ai::DEFAULT_MAX_CYCLES,
        1,
        None,
        agent.settings.clone(),
        tenant_scope,
        vec![],
        agent.compaction_authorization.clone(),
    )
    .map_err(|_| invalid())
}
impl PyEvalSubject {
    pub(super) fn bind(&self, tasks: &TaskSet) -> PyResult<SubjectBinding> {
        let base = SharedSubject::new(
            Arc::clone(&self.id),
            self.agent.clone(),
            self.template.clone(),
            tasks,
        )
        .map_err(error)?;
        match &self.callback {
            None => Ok(base.bind()),
            Some(callback) => Ok(SubjectBinding {
                agent: self.agent.clone(),
                subject: Arc::new(CallbackSubject {
                    id: Arc::clone(&self.id),
                    base,
                    callback: Arc::clone(callback),
                }),
            }),
        }
    }
}
struct CallbackSubject {
    id: Arc<str>,
    base: SharedSubject,
    callback: Arc<PythonCallback>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedRequest {
    input: Option<String>,
    settings: Option<finstack_ai::runtime::ports::model::ModelSettings>,
    timeout_ms: Option<u64>,
    max_cycles: Option<u64>,
}
fn callback_error() -> EvalError {
    EvalError::new(
        finstack_ai_eval::EVAL_SUBJECT_UNBOUND,
        "subject preparation callback failed",
    )
}
impl Subject for CallbackSubject {
    fn id(&self) -> SubjectId {
        Arc::clone(&self.id)
    }
    fn prepare<'a>(
        &'a self,
        cell: &'a Cell,
    ) -> Pin<Box<dyn Future<Output = Result<PreparedAttempt, EvalError>> + Send + 'a>> {
        Box::pin(async move {
            let mut prepared = self.base.prepare(cell).await?;
            let input = serde_json::json!({"cell":cell, "input":prepared.request.input, "settings":prepared.request.settings});
            let value: PreparedRequest = self
                .callback
                .invoke_data(&input)
                .await
                .map_err(|_| callback_error())?;
            if let Some(input) = value.input {
                if input.is_empty() || input.len() > 65_536 || input.contains('\0') {
                    return Err(callback_error());
                }
                prepared.request.input = Arc::from(input);
            }
            if let Some(settings) = value.settings {
                if serde_json::to_vec(&settings)
                    .map_err(|_| callback_error())?
                    .len()
                    > 65_536
                {
                    return Err(callback_error());
                }
                prepared.request.settings = settings;
            }
            if let Some(timeout) = value.timeout_ms {
                if timeout == 0 || timeout > 86_400_000 {
                    return Err(callback_error());
                }
                prepared.request.timeout = Duration::from_millis(timeout);
            }
            if let Some(cycles) = value.max_cycles {
                if cycles == 0 || cycles > 10_000 {
                    return Err(callback_error());
                }
                prepared.request.max_cycles = cycles;
            }
            Ok(prepared)
        })
    }
}
