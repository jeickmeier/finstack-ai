//! Target-neutral conformance runner with placeholder binding adapters.
//!
//! Python and WASM adapters are explicitly deferred. They never report a
//! passing parity result (PR-005 exclusion).

use std::fmt;

use serde_json::Value;

use crate::trace_fixture::{
    compare_normalized_bytes, load_golden_trace, GoldenTrace, TraceError,
};

/// Supported conformance target kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// Native Rust harness path.
    Rust,
    /// Python binding placeholder.
    Python,
    /// Browser WASM binding placeholder.
    Wasm,
}

impl fmt::Display for TargetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rust => write!(f, "rust"),
            Self::Python => write!(f, "python"),
            Self::Wasm => write!(f, "wasm"),
        }
    }
}

/// Adapter capability status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterCapability {
    /// Adapter can execute the fixture path under test.
    Available,
    /// Adapter is intentionally deferred / unavailable.
    Unavailable,
}

/// Outcome of one adapter execution.
#[derive(Debug, Clone, PartialEq)]
pub enum AdapterOutcome {
    /// Observed expected-trace JSON for comparison.
    Observed(Value),
    /// Explicit deferred/unavailable result.
    Deferred {
        /// Human-readable reason.
        reason: String,
    },
    /// Adapter failure.
    Failed {
        /// Diagnostic message.
        message: String,
    },
}

/// Target adapter contract for the conformance runner.
pub trait ConformanceAdapter {
    /// Target identity.
    fn target(&self) -> TargetKind;

    /// Capability for the current build.
    fn capability(&self) -> AdapterCapability;

    /// Execute or project a loaded golden trace.
    ///
    /// # Errors
    ///
    /// Returns fixture or adapter failures.
    fn execute(&self, trace: &GoldenTrace) -> Result<AdapterOutcome, TraceError>;
}

/// Report for one runner invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct ConformanceReport {
    /// Target that ran.
    pub target: TargetKind,
    /// Trace identifier.
    pub trace_id: String,
    /// Whether the run is a passing comparison.
    pub passed: bool,
    /// Whether the run is an explicit deferral (not parity evidence).
    pub deferred: bool,
    /// Diagnostic message.
    pub message: String,
}

/// Target-neutral conformance runner.
#[derive(Debug, Default)]
pub struct ConformanceRunner;

impl ConformanceRunner {
    /// Create a runner.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Load a fixture path and run it through `adapter`.
    ///
    /// # Errors
    ///
    /// Returns load/compare failures for available adapters. Deferred adapters
    /// return a non-passing report rather than an error.
    pub fn run_path(
        &self,
        path: impl AsRef<std::path::Path>,
        adapter: &dyn ConformanceAdapter,
    ) -> Result<ConformanceReport, TraceError> {
        let trace = load_golden_trace(path)?;
        self.run_trace(&trace, adapter)
    }

    /// Run an already-loaded golden trace through `adapter`.
    ///
    /// # Errors
    ///
    /// Returns compare failures for available adapters.
    pub fn run_trace(
        &self,
        trace: &GoldenTrace,
        adapter: &dyn ConformanceAdapter,
    ) -> Result<ConformanceReport, TraceError> {
        let target = adapter.target();
        if adapter.capability() == AdapterCapability::Unavailable {
            let reason = format!(
                "{target} adapter unavailable; cross-language parity deferred until real bindings exist"
            );
            return Ok(ConformanceReport {
                target,
                trace_id: trace.trace_id.clone(),
                passed: false,
                deferred: true,
                message: reason,
            });
        }

        match adapter.execute(trace)? {
            AdapterOutcome::Observed(observed) => {
                let expected = serde_json::to_value(&trace.expected)
                    .map_err(|error| TraceError::Parse(error.to_string()))?;
                compare_normalized_bytes(&expected, &observed)?;
                Ok(ConformanceReport {
                    target,
                    trace_id: trace.trace_id.clone(),
                    passed: true,
                    deferred: false,
                    message: "normalized expected and observed bytes match".to_owned(),
                })
            }
            AdapterOutcome::Deferred { reason } => Ok(ConformanceReport {
                target,
                trace_id: trace.trace_id.clone(),
                passed: false,
                deferred: true,
                message: reason,
            }),
            AdapterOutcome::Failed { message } => Ok(ConformanceReport {
                target,
                trace_id: trace.trace_id.clone(),
                passed: false,
                deferred: false,
                message,
            }),
        }
    }
}

/// Rust no-op adapter that echoes fixture expectations.
///
/// Exercises load/normalize/compare without a reducer.
#[derive(Debug, Default)]
pub struct NoOpRustAdapter;

impl ConformanceAdapter for NoOpRustAdapter {
    fn target(&self) -> TargetKind {
        TargetKind::Rust
    }

    fn capability(&self) -> AdapterCapability {
        AdapterCapability::Available
    }

    fn execute(&self, trace: &GoldenTrace) -> Result<AdapterOutcome, TraceError> {
        let observed = serde_json::to_value(&trace.expected)
            .map_err(|error| TraceError::Parse(error.to_string()))?;
        Ok(AdapterOutcome::Observed(observed))
    }
}

/// Placeholder binding adapter that can never claim parity.
#[derive(Debug, Clone, Copy)]
pub struct DeferredBindingAdapter {
    target: TargetKind,
}

impl DeferredBindingAdapter {
    /// Python placeholder adapter.
    #[must_use]
    pub const fn python() -> Self {
        Self {
            target: TargetKind::Python,
        }
    }

    /// WASM placeholder adapter.
    #[must_use]
    pub const fn wasm() -> Self {
        Self {
            target: TargetKind::Wasm,
        }
    }
}

impl ConformanceAdapter for DeferredBindingAdapter {
    fn target(&self) -> TargetKind {
        self.target
    }

    fn capability(&self) -> AdapterCapability {
        AdapterCapability::Unavailable
    }

    fn execute(&self, _trace: &GoldenTrace) -> Result<AdapterOutcome, TraceError> {
        Ok(AdapterOutcome::Deferred {
            reason: format!(
                "{} binding adapter is a Phase 0 placeholder and cannot claim parity",
                self.target
            ),
        })
    }
}
