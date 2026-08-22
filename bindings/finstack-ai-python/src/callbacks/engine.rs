use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use finstack_ai::runtime::ports::PortFuture;
use finstack_ai_kernel::ErrorCategory;
use futures_util::future::{Either, select};
use futures_util::{FutureExt, pin_mut};
use pyo3::exceptions::{PyRuntimeError as PyBuiltinRuntimeError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::context::PyCallbackContext;

const CALLBACK_TIMEOUT_MAX_SECONDS: f64 = 86_400.0;
const CALLBACK_CANCELLATION_GRACE: Duration = Duration::from_millis(100);
const PYTHON_CALLBACK_CANCELLED: &str = "python_callback_cancelled";
const PYTHON_CALLBACK_FAILED: &str = "python_callback_failed";
const PYTHON_CALLBACK_RESULT_INVALID: &str = "python_callback_result_invalid";
const PYTHON_CALLBACK_TIMEOUT: &str = "python_callback_timeout";

static CALLBACK_TASK_LOCALS: OnceLock<Result<pyo3_async_runtimes::TaskLocals, Arc<str>>> =
    OnceLock::new();

fn callback_task_locals(py: Python<'_>) -> PyResult<pyo3_async_runtimes::TaskLocals> {
    CALLBACK_TASK_LOCALS
        .get_or_init(|| start_callback_loop(py))
        .clone()
        .map_err(|message| PyBuiltinRuntimeError::new_err(message.to_string()))
}

fn start_callback_loop(py: Python<'_>) -> Result<pyo3_async_runtimes::TaskLocals, Arc<str>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("finstack-python-callbacks".to_owned())
        .spawn(move || {
            Python::attach(|py| {
                let started = (|| {
                    let event_loop = py.import("asyncio")?.call_method0("new_event_loop")?;
                    let locals = pyo3_async_runtimes::TaskLocals::new(event_loop.clone());
                    sender.send(Ok(locals)).map_err(|_| {
                        PyBuiltinRuntimeError::new_err("callback loop receiver closed")
                    })?;
                    event_loop.call_method0("run_forever")?;
                    Ok::<(), PyErr>(())
                })();
                if let Err(error) = started {
                    let _ = sender.send(Err(Arc::from("failed to start Python callback loop")));
                    error.print(py);
                }
            });
        })
        .map_err(|_| Arc::from("failed to spawn Python callback loop"))?;
    py.detach(move || {
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| Arc::from("Python callback loop did not start"))?
    })
}

#[derive(Debug, Clone, Copy)]
pub(super) enum CallbackFailure {
    Cancelled,
    Exception,
    InvalidResult,
    Timeout,
}

impl CallbackFailure {
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::Cancelled => PYTHON_CALLBACK_CANCELLED,
            Self::Exception => PYTHON_CALLBACK_FAILED,
            Self::InvalidResult => PYTHON_CALLBACK_RESULT_INVALID,
            Self::Timeout => PYTHON_CALLBACK_TIMEOUT,
        }
    }

    pub(super) const fn category(self, component: ErrorCategory) -> ErrorCategory {
        match self {
            Self::Cancelled => ErrorCategory::Cancellation,
            Self::Timeout => ErrorCategory::Deadline,
            Self::Exception => component,
            Self::InvalidResult => ErrorCategory::Validation,
        }
    }

    pub(super) const fn message(self) -> &'static str {
        match self {
            Self::Cancelled => "Python callback was cancelled",
            Self::Exception => "Python callback failed",
            Self::InvalidResult => "Python callback returned an invalid normalized result",
            Self::Timeout => "Python callback exceeded its configured timeout",
        }
    }
}

#[derive(Clone)]
pub(super) struct CallbackState {
    pub(super) active: Arc<AtomicBool>,
    pub(super) settled: finstack_ai::runtime::native_driver::Signal,
}

impl CallbackState {
    pub(super) fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(true)),
            settled: finstack_ai::runtime::native_driver::Signal::new(),
        }
    }

    fn settle(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            self.settled.notify_waiters();
        }
    }
}

struct CallbackLease(CallbackState);

impl Drop for CallbackLease {
    fn drop(&mut self) {
        self.0.settle();
    }
}

pub(super) struct PythonCallback {
    callable: Py<PyAny>,
    json_loads: Py<PyAny>,
    pub(super) json_dumps: Py<PyAny>,
    is_async: bool,
    task_locals: Option<pyo3_async_runtimes::TaskLocals>,
    timeout: Duration,
}

impl PythonCallback {
    pub(super) fn try_new(
        py: Python<'_>,
        callable: Py<PyAny>,
        timeout_seconds: f64,
    ) -> PyResult<Self> {
        if !callable.bind(py).is_callable() {
            return Err(PyTypeError::new_err("callback must be callable"));
        }
        if !timeout_seconds.is_finite()
            || timeout_seconds <= 0.0
            || timeout_seconds > CALLBACK_TIMEOUT_MAX_SECONDS
        {
            return Err(PyTypeError::new_err(
                "callback_timeout_seconds must be finite and in (0, 86400]",
            ));
        }
        let inspect = py.import("inspect")?;
        let classifier = inspect.getattr("iscoroutinefunction")?;
        let mut is_async = classifier.call1((callable.bind(py),))?.is_truthy()?;
        if !is_async && let Ok(call_method) = callable.bind(py).getattr("__call__") {
            is_async = classifier.call1((call_method,))?.is_truthy()?;
        }
        let json = py.import("json")?;
        Ok(Self {
            callable,
            json_loads: json.getattr("loads")?.unbind(),
            json_dumps: json.getattr("dumps")?.unbind(),
            is_async,
            task_locals: is_async.then(|| callback_task_locals(py)).transpose()?,
            timeout: Duration::from_secs_f64(timeout_seconds),
        })
    }

    pub(super) async fn invoke<Request, Response>(
        &self,
        context: PyCallbackContext,
        request: &Request,
    ) -> Result<Response, CallbackFailure>
    where
        Request: Serialize,
        Response: DeserializeOwned,
    {
        let request_bytes =
            serde_json::to_vec(request).map_err(|_| CallbackFailure::InvalidResult)?;
        let cancellation = context.cancellation.clone();
        let lease = CallbackLease(context.state.clone());
        let callback: PortFuture<Result<Py<PyAny>, CallbackFailure>> = if self.is_async {
            let Some(task_locals) = self.task_locals.as_ref() else {
                return Err(CallbackFailure::Exception);
            };
            let callback = Python::attach(|py| {
                let request_bytes = PyBytes::new(py, &request_bytes);
                let request = self.json_loads.call1(py, (request_bytes,))?;
                let context = Py::new(py, context)?;
                let awaitable = self.callable.call1(py, (context, request))?;
                pyo3_async_runtimes::into_future_with_locals(task_locals, awaitable.into_bound(py))
            })
            .map_err(|_| CallbackFailure::Exception)?;
            Box::pin(async move { callback.await.map_err(|_| CallbackFailure::Exception) })
        } else {
            let (callable, json_loads) =
                Python::attach(|py| (self.callable.clone_ref(py), self.json_loads.clone_ref(py)));
            Box::pin(async move {
                finstack_ai::runtime::native_driver::run_blocking(move || {
                    Python::attach(|py| {
                        let request_bytes = PyBytes::new(py, &request_bytes);
                        let request = json_loads.call1(py, (request_bytes,))?;
                        let context = Py::new(py, context)?;
                        callable.call1(py, (context, request))
                    })
                })
                .await
                .map_err(|_| CallbackFailure::Exception)?
                .map_err(|_| CallbackFailure::Exception)
            })
        };

        let bounded = finstack_ai::runtime::native_driver::timeout(self.timeout, callback).fuse();
        let cancelled = cancellation.cancelled().fuse();
        pin_mut!(bounded, cancelled);
        let result = match select(bounded, cancelled).await {
            Either::Left((Ok(Ok(value)), _)) => value,
            Either::Left((Ok(Err(failure)), _)) => return Err(failure),
            Either::Left((Err(_), _)) => return Err(CallbackFailure::Timeout),
            Either::Right(((), callback)) => {
                // Give a cooperative callback one bounded turn to observe the
                // signal before invalidating its invocation context. A callback
                // that ignores cancellation cannot delay the Rust run beyond
                // this fixed grace period.
                let _ = finstack_ai::runtime::native_driver::timeout(
                    CALLBACK_CANCELLATION_GRACE,
                    callback,
                )
                .await;
                return Err(CallbackFailure::Cancelled);
            }
        };
        drop(lease);
        Python::attach(|py| {
            let encoded = self
                .json_dumps
                .call1(py, (result,))?
                .extract::<String>(py)?;
            serde_json::from_str(&encoded).map_err(|error| PyTypeError::new_err(error.to_string()))
        })
        .map_err(|_| CallbackFailure::InvalidResult)
    }

    pub(super) async fn invoke_batch<Request>(
        &self,
        request: &Request,
    ) -> Result<(), CallbackFailure>
    where
        Request: Serialize,
    {
        let request_bytes =
            serde_json::to_vec(request).map_err(|_| CallbackFailure::InvalidResult)?;
        let callback: PortFuture<Result<Py<PyAny>, CallbackFailure>> = if self.is_async {
            let Some(task_locals) = self.task_locals.as_ref() else {
                return Err(CallbackFailure::Exception);
            };
            let callback = Python::attach(|py| {
                let request_bytes = PyBytes::new(py, &request_bytes);
                let request = self.json_loads.call1(py, (request_bytes,))?;
                let awaitable = self.callable.call1(py, (request,))?;
                pyo3_async_runtimes::into_future_with_locals(task_locals, awaitable.into_bound(py))
            })
            .map_err(|_| CallbackFailure::Exception)?;
            Box::pin(async move { callback.await.map_err(|_| CallbackFailure::Exception) })
        } else {
            let (callable, json_loads) =
                Python::attach(|py| (self.callable.clone_ref(py), self.json_loads.clone_ref(py)));
            Box::pin(async move {
                finstack_ai::runtime::native_driver::run_blocking(move || {
                    Python::attach(|py| {
                        let request_bytes = PyBytes::new(py, &request_bytes);
                        let request = json_loads.call1(py, (request_bytes,))?;
                        callable.call1(py, (request,))
                    })
                })
                .await
                .map_err(|_| CallbackFailure::Exception)?
                .map_err(|_| CallbackFailure::Exception)
            })
        };
        match finstack_ai::runtime::native_driver::timeout(self.timeout, callback).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(failure)) => Err(failure),
            Err(_) => Err(CallbackFailure::Timeout),
        }
    }
}
