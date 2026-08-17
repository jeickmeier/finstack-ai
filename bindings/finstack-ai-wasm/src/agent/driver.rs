use std::sync::Arc;

use finstack_ai::runtime::{Clock, RandomSource};
use js_sys::Uint8Array;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// Install spawn, sleep, clock, and random hooks for the host driver.
pub fn install_host_driver() {
    finstack_ai::runtime::host_driver::install_spawner(crate::executor::spawn_port_future);
    finstack_ai::runtime::host_driver::install_sleeper(|duration, done| {
        let millis =
            u32::try_from(duration.as_millis().min(u128::from(u32::MAX))).unwrap_or(u32::MAX);
        let global = js_sys::global();
        if let Ok(set_timeout) = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout"))
            && let Ok(set_timeout) = set_timeout.dyn_into::<js_sys::Function>()
        {
            let callback = Closure::once_into_js(move || done());
            let _ = set_timeout.call2(&global, &callback, &JsValue::from(millis));
            return;
        }
        done();
    });
    finstack_ai::runtime::host_driver::install_clock(Arc::new(BrowserClock));
    finstack_ai::runtime::host_driver::install_random(Arc::new(BrowserRandom));
}

struct BrowserClock;

impl Clock for BrowserClock {
    fn now(
        &self,
    ) -> Result<finstack_ai::runtime::Timestamp, finstack_ai::runtime::IdGenerationError> {
        let millis = js_sys::Date::now();
        if !millis.is_finite() || millis < 0.0 || millis > 9_007_199_254_740_991.0 {
            return Err(finstack_ai::runtime::IdGenerationError::Source(
                "host clock overflow".into(),
            ));
        }
        #[allow(
            clippy::cast_possible_truncation,
            reason = "finite millis were range-checked against i64::MAX"
        )]
        let millis = millis as i64;
        Ok(finstack_ai::runtime::Timestamp::from_unix_ms(millis)?)
    }
}

struct BrowserRandom;

impl RandomSource for BrowserRandom {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), finstack_ai::runtime::IdGenerationError> {
        let len = u32::try_from(buf.len()).map_err(|_| {
            finstack_ai::runtime::IdGenerationError::Source(
                "host entropy request is too large".into(),
            )
        })?;
        let array = Uint8Array::new_with_length(len);
        let global = js_sys::global();
        let crypto = js_sys::Reflect::get(&global, &JsValue::from_str("crypto")).map_err(|_| {
            finstack_ai::runtime::IdGenerationError::Source("host crypto is unavailable".into())
        })?;
        let fill =
            js_sys::Reflect::get(&crypto, &JsValue::from_str("getRandomValues")).map_err(|_| {
                finstack_ai::runtime::IdGenerationError::Source("host crypto is unavailable".into())
            })?;
        let fill = fill.dyn_into::<js_sys::Function>().map_err(|_| {
            finstack_ai::runtime::IdGenerationError::Source("host crypto is unavailable".into())
        })?;
        fill.call1(&crypto, &array).map_err(|_| {
            finstack_ai::runtime::IdGenerationError::Source("host entropy fill failed".into())
        })?;
        array.copy_to(buf);
        Ok(())
    }
}
