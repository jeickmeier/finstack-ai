//! Python constructors for native Rust middleware extensions.

use std::sync::Arc;

use finstack_ai::runtime::ports::middleware::Middleware;
use finstack_ai_kernel::{ComponentId, ComponentRef, Version};
use finstack_ai_middleware_compaction::{CompactionConfig, CompactionMiddleware};
use finstack_ai_middleware_instructions::{
    InstructionsMiddleware, PolicyEntry, PolicyInstructionsConfig,
};
use finstack_ai_middleware_verify::{
    EvidenceFinding, EvidenceFindings, EvidenceKind, EvidenceVerifier, Verdict, VerifyMiddleware,
    VerifyPolicy,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

const INSTRUCTIONS_COMPONENT: &str = "finstack.middleware.instructions";

/// `InstructionsMiddleware`'s declared invocation version
/// (`INSTRUCTIONS_VERSION` in `finstack-ai-middleware-instructions::lib`).
/// `validate_middleware_descriptor` requires the registered `ComponentRef`
/// to match the handle's own reported `(component id, version)` exactly.
const INSTRUCTIONS_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Frozen policy-instruction middleware backed by the Rust implementation.
///
/// Each `(label, text)` entry becomes one protected System context item
/// injected at `prepare_context`, with provenance source id
/// `policy:{label}`. Entries are validated and frozen at construction; at
/// most 16 entries, labels at most 249 bytes.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "InstructionsMiddleware",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyInstructionsMiddleware {
    component: ComponentRef,
    inner: Arc<InstructionsMiddleware>,
}

#[pymethods]
impl PyInstructionsMiddleware {
    /// Build the middleware from ordered `(label, text)` policy entries.
    #[new]
    #[pyo3(signature = (entries))]
    fn new(entries: Vec<(String, String)>) -> PyResult<Self> {
        let config = PolicyInstructionsConfig {
            entries: entries
                .into_iter()
                .map(|(label, text)| PolicyEntry { label, text })
                .collect(),
        };
        let middleware = InstructionsMiddleware::try_new(config)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let component = ComponentId::parse(INSTRUCTIONS_COMPONENT)
            .map(|id| ComponentRef::new(id, Some(INSTRUCTIONS_VERSION)))
            .map_err(|_| PyValueError::new_err("instructions component id is invalid"))?;
        Ok(Self {
            component,
            inner: Arc::new(middleware),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }
}

impl PyInstructionsMiddleware {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Middleware>) {
        (self.component.clone(), self.inner.clone())
    }
}

const COMPACTION_COMPONENT: &str = "finstack.middleware.compaction";

/// `CompactionMiddleware`'s declared invocation version (see the crate's
/// checked-in `MiddlewareDescriptor`); the registered `ComponentRef` must
/// match it exactly.
const COMPACTION_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 4,
};

/// Deterministic context-compaction middleware backed by the Rust
/// implementation.
///
/// One instance owns one strategy; construct via the strategy factories.
/// The `summarize` strategy (model-assisted) is not exposed from Python
/// yet — it needs an authorized model reference and budget scope.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "CompactionMiddleware",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyCompactionMiddleware {
    component: ComponentRef,
    inner: Arc<CompactionMiddleware>,
}

impl PyCompactionMiddleware {
    fn from_config(config: CompactionConfig) -> PyResult<Self> {
        let middleware = CompactionMiddleware::try_new(config)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let component = ComponentId::parse(COMPACTION_COMPONENT)
            .map(|id| ComponentRef::new(id, Some(COMPACTION_VERSION)))
            .map_err(|_| PyValueError::new_err("compaction component id is invalid"))?;
        Ok(Self {
            component,
            inner: Arc::new(middleware),
        })
    }

    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Middleware>) {
        (self.component.clone(), self.inner.clone())
    }
}

#[pymethods]
impl PyCompactionMiddleware {
    /// Drop the oldest unprotected context above the token threshold.
    #[staticmethod]
    #[pyo3(signature = (threshold_tokens, hysteresis_tokens))]
    fn sliding_window(threshold_tokens: u64, hysteresis_tokens: u64) -> PyResult<Self> {
        Self::from_config(CompactionConfig::sliding_window(
            threshold_tokens,
            hysteresis_tokens,
        ))
    }

    /// Truncate large tool-result bodies while keeping call/result pairs.
    #[staticmethod]
    #[pyo3(signature = (threshold_tokens, hysteresis_tokens, max_body_bytes))]
    fn large_tool_output(
        threshold_tokens: u64,
        hysteresis_tokens: u64,
        max_body_bytes: usize,
    ) -> PyResult<Self> {
        Self::from_config(CompactionConfig::large_tool_output(
            threshold_tokens,
            hysteresis_tokens,
            max_body_bytes,
        ))
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }
}

const VERIFY_COMPONENT: &str = "finstack.middleware.verify";

/// `VerifyMiddleware`'s declared invocation version (`VERIFY_VERSION` in
/// `finstack-ai-middleware-verify::lib`).
const VERIFY_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Bridge from a synchronous Python callable to the crate's pure
/// [`EvidenceVerifier`] trait.
///
/// The callable receives the candidate assistant message as a JSON string
/// and must be pure and deterministic (same input, same verdict): the
/// engine re-runs verification wholesale on recovery and never journals
/// it. Any callback error or malformed verdict fails closed as `Reject`.
struct PythonEvidenceVerifier {
    id: String,
    callback: Py<PyAny>,
}

impl std::fmt::Debug for PythonEvidenceVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PythonEvidenceVerifier")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

fn parse_findings(value: &serde_json::Value) -> Result<EvidenceFindings, ()> {
    let entries = value
        .get("findings")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut findings = Vec::with_capacity(entries.len());
    for entry in entries {
        let kind = match entry.get("kind").and_then(serde_json::Value::as_str) {
            Some("citation") => EvidenceKind::Citation,
            Some("test") => EvidenceKind::Test,
            Some("artifact") => EvidenceKind::Artifact,
            _ => return Err(()),
        };
        let note = entry
            .get("note")
            .and_then(serde_json::Value::as_str)
            .ok_or(())?;
        findings.push(EvidenceFinding::try_new(kind, note).map_err(|_| ())?);
    }
    EvidenceFindings::try_new(findings).map_err(|_| ())
}

fn reject_fallback(note: &str) -> Verdict {
    let findings = EvidenceFinding::try_new(EvidenceKind::Artifact, note)
        .ok()
        .and_then(|finding| EvidenceFindings::try_new(vec![finding]).ok())
        .or_else(|| EvidenceFindings::try_new(Vec::new()).ok());
    match findings {
        Some(findings) => Verdict::Reject(findings),
        // Unreachable: empty findings always construct. Failing open here
        // is impossible in practice; the arm exists only for totality.
        None => Verdict::Accept,
    }
}

impl EvidenceVerifier for PythonEvidenceVerifier {
    fn verifier_id(&self) -> &str {
        &self.id
    }

    fn verify(&self, message: &finstack_ai_kernel::RawJson) -> Verdict {
        Python::attach(|py| {
            let message = String::from_utf8_lossy(message.as_bytes()).into_owned();
            let outcome = self.callback.bind(py).call1((message,)).and_then(|value| {
                value
                    .py()
                    .import("json")
                    .and_then(|json| json.call_method1("dumps", (value,)))
                    .and_then(|encoded| encoded.extract::<String>())
            });
            let Ok(encoded) = outcome else {
                return reject_fallback("python verifier raised");
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&encoded) else {
                return reject_fallback("python verifier returned non-JSON");
            };
            let verdict = if let Some(text) = value.as_str() {
                text.to_owned()
            } else {
                match value.get("verdict").and_then(serde_json::Value::as_str) {
                    Some(text) => text.to_owned(),
                    None => return reject_fallback("python verifier verdict missing"),
                }
            };
            match verdict.as_str() {
                "accept" => Verdict::Accept,
                "bounce" => match parse_findings(&value) {
                    Ok(findings) => Verdict::Bounce(findings),
                    Err(()) => reject_fallback("python verifier findings invalid"),
                },
                "reject" => match parse_findings(&value) {
                    Ok(findings) => Verdict::Reject(findings),
                    Err(()) => reject_fallback("python verifier findings invalid"),
                },
                _ => reject_fallback("python verifier verdict unknown"),
            }
        })
    }
}

/// Deterministic content-verification middleware backed by the Rust
/// implementation.
///
/// Wraps a synchronous, pure Python verifier callable: it receives the
/// candidate assistant message as a JSON string and returns `"accept"`,
/// or a mapping `{"verdict": "accept" | "bounce" | "reject", "findings":
/// [{"kind": "citation" | "test" | "artifact", "note": str}, ...]}`.
/// `bounce` retries the model with feedback; `reject` fails the run with
/// the stable `verify_rejected` code. Errors fail closed as reject.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "VerifyMiddleware",
    frozen,
    skip_from_py_object
)]
pub(crate) struct PyVerifyMiddleware {
    component: ComponentRef,
    inner: Arc<VerifyMiddleware>,
}

#[pymethods]
impl PyVerifyMiddleware {
    /// Build the middleware from a pure verifier callable and bounce policy.
    #[new]
    #[pyo3(signature = (verifier, *, verifier_id, backoff_seconds = 0.0, policy_version = "verify-policy-v1"))]
    fn new(
        verifier: Py<PyAny>,
        verifier_id: String,
        backoff_seconds: f64,
        policy_version: &str,
    ) -> PyResult<Self> {
        if !backoff_seconds.is_finite() || backoff_seconds < 0.0 {
            return Err(PyValueError::new_err("backoff_seconds must be >= 0"));
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "validated non-negative finite seconds, bounded to whole milliseconds"
        )]
        let backoff_ms = (backoff_seconds * 1_000.0) as u64;
        let policy = VerifyPolicy::try_new(
            finstack_ai_kernel::Duration::from_millis(backoff_ms),
            policy_version,
        )
        .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let middleware = VerifyMiddleware::try_new(
            Arc::new(PythonEvidenceVerifier {
                id: verifier_id,
                callback: verifier,
            }),
            policy,
        )
        .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let component = ComponentId::parse(VERIFY_COMPONENT)
            .map(|id| ComponentRef::new(id, Some(VERIFY_VERSION)))
            .map_err(|_| PyValueError::new_err("verify component id is invalid"))?;
        Ok(Self {
            component,
            inner: Arc::new(middleware),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }
}

impl PyVerifyMiddleware {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Middleware>) {
        (self.component.clone(), self.inner.clone())
    }
}
