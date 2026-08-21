//! Non-default synthetic fixture for binding performance and concurrency proof.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use finstack_ai::runtime::{
    JournalStore, Model, ModelContextProfile, ModelName, ModelResponse, ModelStreamItem, TextDelta,
    TokenEstimatorRef, TokenEstimatorSource,
};
use finstack_ai::{Agent, AgentRunError};
use finstack_ai_kernel::{AgentId, BundleId, ContentBlock, ProviderIds, TextBlock, Usage};
use finstack_ai_memory::InProcessArtifactStore;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    ScriptedModel, ScriptedModelAction, ScriptedModelControl, ScriptedModelPlan,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use super::agent::{PyAgent, component, empty_model_settings};
use super::errors::{agent_error, configuration_error};
use super::run::run_request;

const BENCHMARK_MODEL: &str = "python-fast-path-v1";
const MAX_DELTAS: usize = 4_096;
const MAX_RUNS: usize = 512;
const MAX_TOTAL_DELTAS: usize = 65_536;

/// Control plane for deterministic benchmark-only model gates and counters.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "_BenchmarkControl",
    frozen
)]
pub(super) struct PyBenchmarkControl {
    model: Arc<ScriptedModel>,
    control: ScriptedModelControl,
}

#[pymethods]
impl PyBenchmarkControl {
    /// Number of Rust model streams currently parked at a named gate.
    fn gate_entries(&self, name: &str) -> usize {
        self.control.entries(name)
    }

    /// Release every current and future stream parked at a named gate.
    fn release(&self, name: &str) {
        self.control.release(name);
    }

    /// Number of model requests issued through the Rust SDK path.
    #[getter]
    fn request_count(&self) -> usize {
        self.model.request_count()
    }
}

#[pyfunction(name = "_benchmark_agent")]
#[pyo3(signature = (deltas, runs, gate = None))]
fn benchmark_agent(
    py: Python<'_>,
    deltas: usize,
    runs: usize,
    gate: Option<String>,
) -> PyResult<Bound<'_, PyAny>> {
    validate_workload(deltas, runs)?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let built = build_agent(deltas, runs, gate).await;
        Python::attach(|py| match built {
            Ok((agent, control)) => Ok((Py::new(py, agent)?, Py::new(py, control)?)),
            Err(error) => Err(agent_error(py, &error, None)),
        })
    })
}

#[pyfunction(name = "_benchmark_native")]
#[pyo3(signature = (deltas, runs))]
fn benchmark_native(py: Python<'_>, deltas: usize, runs: usize) -> PyResult<Bound<'_, PyAny>> {
    validate_workload(deltas, runs)?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let result = run_native_workload(deltas, runs).await;
        match result {
            Ok(elapsed_ns) => Ok((elapsed_ns, deltas.saturating_mul(runs))),
            Err(error) => Python::attach(|py| Err(agent_error(py, &error, None))),
        }
    })
}

pub(super) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyBenchmarkControl>()?;
    module.add_function(wrap_pyfunction!(benchmark_agent, module)?)?;
    module.add_function(wrap_pyfunction!(benchmark_native, module)?)?;
    Ok(())
}

async fn run_native_workload(deltas: usize, runs: usize) -> Result<u64, AgentRunError> {
    let (agent, _) = build_agent(deltas, runs, None).await?;
    let started = Instant::now();
    for index in 0..runs {
        let request = run_request(
            &agent.model,
            format!("native synthetic run {index}"),
            30.0,
            1,
            1,
            None,
            empty_model_settings()?,
            "python-local",
            Vec::new(),
        )?;
        let output = agent.inner.start(request)?.result().await?;
        black_box(output);
    }
    Ok(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX))
}

async fn build_agent(
    deltas: usize,
    runs: usize,
    gate: Option<String>,
) -> Result<(PyAgent, PyBenchmarkControl), AgentRunError> {
    let model_name = ModelName::try_new(BENCHMARK_MODEL)
        .map_err(|error| configuration_error(error.to_string()))?;
    let profile = ModelContextProfile {
        provider: Arc::from("scripted-python-benchmark"),
        model: model_name.clone(),
        hard_input_bytes: 1_048_576,
        context_window_tokens: 1_048_576,
        max_output_tokens: 65_536,
        reserved_output_tokens: 65_536,
        provider_overhead_tokens: 32,
        estimator: TokenEstimatorRef {
            id: Arc::from("bytes-upper-bound"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    };
    let plan = scripted_plan(deltas, gate.as_deref())?;
    let model = Arc::new(ScriptedModel::from_plans(profile, vec![plan; runs]));
    let control = model.control();
    let model_port: Arc<dyn Model> = model.clone();
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: runs.saturating_mul(2).clamp(64, 1_024),
            batches_per_session: 256,
            records_per_session: 16_384,
            snapshot_bytes: 64 * 1_024,
        })
        .map_err(|error| configuration_error(error.to_string()))?,
    );
    let agent = Agent::builder(
        AgentId::parse("python.agent.fast-path-benchmark")
            .map_err(|error| configuration_error(error.to_string()))?,
        BundleId::parse("python.bundle.fast-path-benchmark")
            .map_err(|error| configuration_error(error.to_string()))?,
        (component("python.model.fast-path-benchmark")?, model_port),
        (component("python.store.fast-path-benchmark")?, store),
    )
    .build()
    .await?;
    Ok((
        PyAgent {
            inner: Arc::new(agent),
            model: model_name,
            output_adapter: None,
            settings: empty_model_settings()?,
            default_timeout_seconds: 30.0,
            // This fast-path benchmark agent registers no toolsets or
            // middleware, so these are unused, dedicated instances rather
            // than the shared ones a real agent factory wires up.
            artifact_store: Arc::new(InProcessArtifactStore::default()),
        },
        PyBenchmarkControl { model, control },
    ))
}

fn scripted_plan(deltas: usize, gate: Option<&str>) -> Result<ScriptedModelPlan, AgentRunError> {
    let mut actions = Vec::with_capacity(deltas.saturating_add(2));
    if let Some(gate) = gate {
        actions.push(ScriptedModelAction::Block(Arc::from(gate)));
    }
    actions.extend((0..deltas).map(|_| {
        ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
            text: Arc::from("abcdefgh"),
        })))
    }));
    let text = "abcdefgh".repeat(deltas);
    actions.push(ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(
        ModelResponse {
            assistant_content: Arc::from([ContentBlock::Text(
                TextBlock::try_new(text).map_err(|error| configuration_error(error.to_string()))?,
            )]),
            tool_calls: Arc::from([]),
            usage: Usage::empty(),
            provider_ids: ProviderIds::empty(),
            completion_id: Arc::from("python-fast-path-benchmark-completion"),
            continuation_state: None,
        },
    ))));
    Ok(ScriptedModelPlan { actions })
}

fn validate_workload(deltas: usize, runs: usize) -> PyResult<()> {
    if !(1..=MAX_DELTAS).contains(&deltas) {
        return Err(PyValueError::new_err(format!(
            "deltas must be in 1..={MAX_DELTAS}"
        )));
    }
    if !(1..=MAX_RUNS).contains(&runs) {
        return Err(PyValueError::new_err(format!(
            "runs must be in 1..={MAX_RUNS}"
        )));
    }
    if deltas.saturating_mul(runs) > MAX_TOTAL_DELTAS {
        return Err(PyValueError::new_err(format!(
            "deltas multiplied by runs must not exceed {MAX_TOTAL_DELTAS}"
        )));
    }
    Ok(())
}
