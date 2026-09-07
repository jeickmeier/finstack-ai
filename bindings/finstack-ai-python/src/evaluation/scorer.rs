//! Scorer construction and data-only callback conversion; all scoring lives in Rust.
use super::{error, invalid, parse};
use crate::{agent::PyAgent, callbacks::PythonCallback};
use finstack_ai_eval::{
    EVAL_SCORER_FAILED, EvalError, ExactMatchScorer, FieldSpec, IncludesScorer, JudgeRubric,
    JudgeScorer, NumericToleranceScorer, ParsedNumber, RecordKindsScorer, RegexScorer, Score,
    ScoreContext, Scorer, ScorerId, StructuredFieldScorer, ToleranceBands,
};
use pyo3::prelude::*;
use serde::Deserialize;
use std::{future::Future, pin::Pin, sync::Arc};

#[pyclass(module = "finstack_ai._finstack_ai", name = "_EvalScorer", frozen)]
pub(crate) struct PyEvalScorer {
    pub(super) inner: Arc<dyn Scorer>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Builtin {
    ExactMatch {
        trim: bool,
        case_sensitive: bool,
    },
    Includes {
        case_sensitive: bool,
    },
    Regex {
        pattern: String,
    },
    NumericTolerance {
        bands: ToleranceBands,
    },
    StructuredField {
        fields: Vec<FieldSpec>,
    },
    RecordKinds {
        required: Vec<Arc<str>>,
        forbidden: Vec<Arc<str>>,
    },
}
#[pymethods]
impl PyEvalScorer {
    #[staticmethod]
    fn builtin(
        py: Python<'_>,
        scorer_id: String,
        version: u32,
        config_json: String,
    ) -> PyResult<Self> {
        py.detach(move || {
            let config: Builtin = parse(&config_json, 65_536)?;
            let inner: Arc<dyn Scorer> = match config {
                Builtin::ExactMatch {
                    trim,
                    case_sensitive,
                } => Arc::new(
                    ExactMatchScorer::new(scorer_id, version, trim, case_sensitive)
                        .map_err(error)?,
                ),
                Builtin::Includes { case_sensitive } => Arc::new(
                    IncludesScorer::new(scorer_id, version, case_sensitive).map_err(error)?,
                ),
                Builtin::Regex { pattern } => {
                    Arc::new(RegexScorer::new(scorer_id, version, &pattern).map_err(error)?)
                }
                Builtin::NumericTolerance { bands } => {
                    Arc::new(NumericToleranceScorer::new(scorer_id, version, bands).map_err(error)?)
                }
                Builtin::StructuredField { fields } => {
                    Arc::new(StructuredFieldScorer::new(scorer_id, version, fields).map_err(error)?)
                }
                Builtin::RecordKinds {
                    required,
                    forbidden,
                } => Arc::new(
                    RecordKindsScorer::new(scorer_id, version, required, forbidden)
                        .map_err(error)?,
                ),
            };
            Ok(Self { inner })
        })
    }
    #[staticmethod]
    #[pyo3(signature = (scorer_id, version, agent, rubric_json, *, tenant_scope = "python-local"))]
    fn judge(
        py: Python<'_>,
        scorer_id: String,
        version: u32,
        agent: &Bound<'_, PyAgent>,
        rubric_json: &str,
        tenant_scope: &str,
    ) -> PyResult<Self> {
        let rubric: JudgeRubric = parse(rubric_json, 65_536)?;
        let agent = agent.borrow();
        let template = super::subject::template(&agent, tenant_scope)?;
        let agent = agent.inner.as_ref().clone();
        py.detach(move || {
            Ok(Self {
                inner: Arc::new(
                    JudgeScorer::new(scorer_id, version, agent, template, rubric).map_err(error)?,
                ),
            })
        })
    }
    #[staticmethod]
    #[pyo3(signature = (scorer_id, version, callback, *, callback_timeout_seconds = 30.0))]
    fn python(
        py: Python<'_>,
        scorer_id: String,
        version: u32,
        callback: Py<PyAny>,
        callback_timeout_seconds: f64,
    ) -> PyResult<Self> {
        let inner = Arc::new(CallbackScorer {
            id: Arc::from(scorer_id),
            version,
            callback: PythonCallback::try_new(py, callback, callback_timeout_seconds)?,
        });
        inner.validate().map_err(error)?;
        Ok(Self { inner })
    }
    #[staticmethod]
    fn parse_number(py: Python<'_>, value: &str) -> PyResult<String> {
        let parsed = ParsedNumber::parse(value).map_err(error)?;
        py.detach(move || serde_json::to_string(&parsed).map_err(|_| invalid()))
    }
}
struct CallbackScorer {
    id: Arc<str>,
    version: u32,
    callback: PythonCallback,
}
impl Scorer for CallbackScorer {
    fn id(&self) -> ScorerId {
        Arc::clone(&self.id)
    }
    fn version(&self) -> u32 {
        self.version
    }
    fn score<'a>(
        &'a self,
        context: &'a ScoreContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Score>, EvalError>> + Send + 'a>> {
        Box::pin(async move {
            let output = context.output.map(|output| serde_json::json!({"text": output.text(), "structured_json": output.structured_json().map(finstack_ai_kernel::RawJson::as_str)}));
            let input = serde_json::json!({"sample":context.sample,"output":output,"record":context.record});
            self.callback
                .invoke_data(&input)
                .await
                .map_err(|_| EvalError::new(EVAL_SCORER_FAILED, "Python scorer callback failed"))
        })
    }
}
