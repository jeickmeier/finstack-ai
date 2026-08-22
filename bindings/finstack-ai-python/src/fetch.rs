//! Python constructor for the Rust bounded HTTP fetch toolset.

use std::sync::Arc;

use finstack_ai::runtime::ports::tool::Toolset;
use finstack_ai_kernel::{ComponentId, ComponentRef, Version};
use finstack_ai_tools_fetch::{HttpFetchConfigSnapshot, HttpFetchToolset};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

const FETCH_COMPONENT: &str = "finstack.tools.fetch";

/// Bounded, allowlisted HTTP fetch toolset backed by the Rust implementation.
///
/// Built from a JSON configuration document: `allowlist` (required,
/// non-empty), and optional `max_response_bytes`, `request_timeout_ms`,
/// `max_redirects`, `per_host_headers`, `user_agent`. The JSON document does
/// NOT accept `allow_loopback_http` (an unknown-key error if it does) —
/// that privilege is intentionally not data-configurable. Fixtures that need
/// it pass the keyword-only `insecure_allow_loopback_http=True` argument to
/// this constructor instead, which sets the field on the parsed config after
/// `from_json`, in code, never from the JSON payload. **Fixtures only. Never
/// enable in production** — see `HttpFetchConfig::allow_loopback_http`'s doc
/// on the Rust side for why this is the single largest privilege the crate
/// can grant.
///
/// This constructor never attaches an artifact store: the Python-built
/// toolset always calls `HttpFetchToolset::try_new` alone, with no
/// `with_artifact_store` call available from Python in v1. As a result,
/// `mode: "artifact"` and any binary (or invalid-UTF-8) response body always
/// fail with `fetch_limit_exceeded` for this binding — there is no store to
/// stage them to.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "HttpFetchToolset",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyHttpFetchToolset {
    component: ComponentRef,
    inner: Arc<HttpFetchToolset>,
}

#[pymethods]
impl PyHttpFetchToolset {
    /// Build the fetch toolset from a JSON configuration document.
    ///
    /// `insecure_allow_loopback_http` is keyword-only, defaults to `False`,
    /// and is applied to the parsed config AFTER `from_json` — the JSON
    /// document itself cannot carry this privilege. The name carries the
    /// warning: never set this to `True` outside test fixtures.
    #[new]
    #[pyo3(signature = (config_json, *, insecure_allow_loopback_http = false))]
    fn new(config_json: &str, insecure_allow_loopback_http: bool) -> PyResult<Self> {
        let mut config = HttpFetchConfigSnapshot::from_json(config_json.as_bytes())
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        config.allow_loopback_http = insecure_allow_loopback_http;
        let toolset = HttpFetchToolset::try_new(config)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let component = ComponentId::parse(FETCH_COMPONENT)
            .map(|id| {
                ComponentRef::new(
                    id,
                    Some(Version {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    }),
                )
            })
            .map_err(|_| PyValueError::new_err("fetch component id is invalid"))?;
        Ok(Self {
            component,
            inner: Arc::new(toolset),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }

    /// Number of tools exposed to the model (always 1: `http_fetch`).
    #[getter]
    fn tool_count(&self) -> usize {
        self.inner.tools().len()
    }
}

impl PyHttpFetchToolset {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Toolset>) {
        (self.component.clone(), self.inner.clone())
    }
}
