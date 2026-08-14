//! Host clock and random-source adapters.

use finstack_ai::runtime::{Clock, IdGenerationError, RandomSource, Timestamp};

use crate::host::HostFailure;

#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

/// Host-backed clock. `now()` returns Unix milliseconds.
pub struct HostClock {
    #[cfg(not(target_arch = "wasm32"))]
    now: Arc<dyn Fn() -> Result<i64, HostFailure> + Send + Sync>,
    #[cfg(target_arch = "wasm32")]
    adapter: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    now: std::rc::Rc<std::cell::RefCell<js_sys::Function>>,
}

impl HostClock {
    /// Construct a native clock.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_callback(
        now: impl Fn() -> Result<i64, HostFailure> + Send + Sync + 'static,
    ) -> Self {
        Self { now: Arc::new(now) }
    }

    /// Construct a wasm32 clock around a JS `HostClock`.
    ///
    /// # Errors
    ///
    /// Returns [`HostFailure`] when `now` is missing.
    #[cfg(target_arch = "wasm32")]
    pub fn from_js(adapter: wasm_bindgen::JsValue) -> Result<Self, HostFailure> {
        let now = crate::host::extract_method(&adapter, "now")?;
        Ok(Self {
            adapter,
            now: std::rc::Rc::new(std::cell::RefCell::new(now)),
        })
    }
}

impl Clock for HostClock {
    fn now(&self) -> Result<Timestamp, IdGenerationError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let millis = (self.now)()
                .map_err(|failure| IdGenerationError::Source(failure.message().to_owned()))?;
            Timestamp::from_unix_ms(millis).map_err(IdGenerationError::from)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let method = self.now.borrow().clone();
            let value = method
                .call0(&self.adapter)
                .map_err(|_| IdGenerationError::Source("JavaScript host failed".into()))?;
            let millis = value
                .as_f64()
                .and_then(|millis| i64::try_from(millis as i128).ok())
                .ok_or_else(|| {
                    IdGenerationError::Source(
                        "JavaScript host returned an invalid normalized result".into(),
                    )
                })?;
            Timestamp::from_unix_ms(millis).map_err(IdGenerationError::from)
        }
    }
}

/// Host-backed random source. `fillBytes(length)` returns a `Uint8Array`.
pub struct HostRandomSource {
    #[cfg(not(target_arch = "wasm32"))]
    fill: Arc<dyn Fn(usize) -> Result<Vec<u8>, HostFailure> + Send + Sync>,
    #[cfg(target_arch = "wasm32")]
    adapter: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    fill: std::rc::Rc<std::cell::RefCell<js_sys::Function>>,
}

impl HostRandomSource {
    /// Construct a native random source.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_callback(
        fill: impl Fn(usize) -> Result<Vec<u8>, HostFailure> + Send + Sync + 'static,
    ) -> Self {
        Self {
            fill: Arc::new(fill),
        }
    }

    /// Construct a wasm32 random source around a JS `HostRandomSource`.
    ///
    /// # Errors
    ///
    /// Returns [`HostFailure`] when `fillBytes` is missing.
    #[cfg(target_arch = "wasm32")]
    pub fn from_js(adapter: wasm_bindgen::JsValue) -> Result<Self, HostFailure> {
        let fill = crate::host::extract_method(&adapter, "fillBytes")?;
        Ok(Self {
            adapter,
            fill: std::rc::Rc::new(std::cell::RefCell::new(fill)),
        })
    }
}

impl RandomSource for HostRandomSource {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let bytes = (self.fill)(buf.len())
                .map_err(|failure| IdGenerationError::Source(failure.message().to_owned()))?;
            if bytes.len() != buf.len() {
                return Err(IdGenerationError::Source(
                    "JavaScript host returned an invalid normalized result".into(),
                ));
            }
            buf.copy_from_slice(&bytes);
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let method = self.fill.borrow().clone();
            let value = method
                .call1(
                    &self.adapter,
                    &wasm_bindgen::JsValue::from_f64(buf.len() as f64),
                )
                .map_err(|_| IdGenerationError::Source("JavaScript host failed".into()))?;
            let bytes = crate::host::bytes_from_uint8_array(&value).map_err(|_| {
                IdGenerationError::Source(
                    "JavaScript host returned an invalid normalized result".into(),
                )
            })?;
            if bytes.len() != buf.len() {
                return Err(IdGenerationError::Source(
                    "JavaScript host returned an invalid normalized result".into(),
                ));
            }
            buf.copy_from_slice(&bytes);
            Ok(())
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::HostClock;
    use finstack_ai::runtime::Clock;

    #[test]
    fn native_clock_converts_unix_ms() {
        let clock = HostClock::from_callback(|| Ok(1_704_067_200_000));
        let now = clock.now().expect("now");
        assert_eq!(now.as_unix_ms(), 1_704_067_200_000);
    }
}
