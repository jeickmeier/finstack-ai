//! JSON conversion at the Python callback and protocol boundary.

use pyo3::IntoPyObjectExt;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PySequence, PyString};

pub(crate) fn py_to_json(value: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    if value.is_none() {
        return Ok(serde_json::Value::Null);
    }
    if let Ok(b) = value.cast::<PyBool>() {
        return Ok(serde_json::Value::Bool(b.is_true()));
    }
    if let Ok(i) = value.cast::<PyInt>() {
        if let Ok(v) = i.extract::<i64>() {
            return Ok(serde_json::Value::from(v));
        }
        if let Ok(v) = i.extract::<u64>() {
            return Ok(serde_json::Value::from(v));
        }
    }
    if let Ok(f) = value.cast::<PyFloat>() {
        let val = f.value();
        return serde_json::Number::from_f64(val)
            .map(serde_json::Value::Number)
            .ok_or_else(|| PyTypeError::new_err("JSON number is not finite"));
    }
    if let Ok(s) = value.cast::<PyString>() {
        return Ok(serde_json::Value::String(s.to_str()?.to_owned()));
    }
    if let Ok(dict) = value.cast::<PyDict>() {
        let mut object = serde_json::Map::with_capacity(dict.len());
        for (k, v) in dict.iter() {
            let key = k.extract::<String>()?;
            object.insert(key, py_to_json(&v)?);
        }
        return Ok(serde_json::Value::Object(object));
    }
    if let Ok(list) = value.cast::<PyList>() {
        let mut array = Vec::with_capacity(list.len());
        for item in list.iter() {
            array.push(py_to_json(&item)?);
        }
        return Ok(serde_json::Value::Array(array));
    }
    if let Ok(seq) = value.cast::<PySequence>() {
        let len = seq.len()?;
        let mut array = Vec::with_capacity(len);
        for i in 0..len {
            array.push(py_to_json(&seq.get_item(i)?)?);
        }
        return Ok(serde_json::Value::Array(array));
    }
    Err(PyTypeError::new_err("value is not JSON serializable"))
}

pub(crate) fn json_to_py(py: Python<'_>, value: &serde_json::Value) -> PyResult<Py<PyAny>> {
    match value {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(value) => value.into_bound_py_any(py).map(Bound::unbind),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                value.into_bound_py_any(py).map(Bound::unbind)
            } else if let Some(value) = value.as_u64() {
                value.into_bound_py_any(py).map(Bound::unbind)
            } else if let Some(value) = value.as_f64() {
                value.into_bound_py_any(py).map(Bound::unbind)
            } else {
                Err(PyTypeError::new_err("unsupported JSON number"))
            }
        }
        serde_json::Value::String(value) => value.into_bound_py_any(py).map(Bound::unbind),
        serde_json::Value::Array(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(json_to_py(py, item)?)?;
            }
            Ok(list.into_any().unbind())
        }
        serde_json::Value::Object(map) => {
            let dict = PyDict::new(py);
            for (key, item) in map {
                dict.set_item(key, json_to_py(py, item)?)?;
            }
            Ok(dict.into_any().unbind())
        }
    }
}
