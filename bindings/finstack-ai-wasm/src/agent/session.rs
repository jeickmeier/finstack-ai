use finstack_ai::OperationLocator;
use wasm_bindgen::prelude::*;

use crate::executor;

use super::errors::{locator_object, session_error};

/// Read-only operation locator.
#[wasm_bindgen(js_name = Locator)]
pub struct Locator {
    pub(super) locator: OperationLocator,
}

#[wasm_bindgen(js_class = Locator)]
impl Locator {
    /// Tenant scope captured at acceptance.
    #[wasm_bindgen(getter, js_name = tenantScope)]
    pub fn tenant_scope(&self) -> String {
        self.locator.tenant_scope.to_string()
    }

    /// Session identity.
    #[wasm_bindgen(getter, js_name = sessionId)]
    pub fn session_id(&self) -> String {
        self.locator.session_id.to_string()
    }

    /// Lane identity.
    #[wasm_bindgen(getter, js_name = laneId)]
    pub fn lane_id(&self) -> String {
        self.locator.lane_id.to_string()
    }

    /// Run identity.
    #[wasm_bindgen(getter, js_name = runId)]
    pub fn run_id(&self) -> String {
        self.locator.run_id.to_string()
    }

    /// Explicit locator snapshot.
    ///
    /// # Errors
    ///
    /// Returns a JavaScript exception when the snapshot object cannot be constructed.
    #[wasm_bindgen(js_name = toDict)]
    pub fn to_dict(&self) -> Result<JsValue, JsValue> {
        locator_object(&self.locator)
    }
}

/// Live session handle.
#[wasm_bindgen(js_name = Session)]
pub struct Session {
    pub(super) inner: finstack_ai::Session,
}

#[wasm_bindgen(js_class = Session)]
impl Session {
    /// Tenant scope captured by the host.
    #[wasm_bindgen(getter, js_name = tenantScope)]
    pub fn tenant_scope(&self) -> String {
        self.inner.tenant_scope().to_string()
    }

    /// Session identity.
    #[wasm_bindgen(getter, js_name = sessionId)]
    pub fn session_id(&self) -> String {
        self.inner.session_id().to_string()
    }

    /// Create a named lane, optionally forking from an existing entry.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the lane cannot be created.
    #[wasm_bindgen(js_name = createLane)]
    pub fn create_lane(&self, name: String, fork: Option<String>) -> js_sys::Promise {
        let session = self.inner.clone();
        executor::drive(async move {
            let fork = fork
                .map(|value| finstack_ai::runtime::EntryId::parse(&value))
                .transpose()
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            session
                .create_lane(name, fork)
                .await
                .map(|inner| JsValue::from(Lane { inner }))
                .map_err(|error| session_error(&error))
        })
    }

    /// List restored lanes.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the session cannot be loaded.
    #[wasm_bindgen(js_name = listLanes)]
    pub fn list_lanes(&self) -> js_sys::Promise {
        let session = self.inner.clone();
        executor::drive(async move {
            session
                .list_lanes()
                .await
                .map(|lanes| {
                    lanes
                        .into_iter()
                        .map(|inner| JsValue::from(Lane { inner }))
                        .collect::<js_sys::Array>()
                        .into()
                })
                .map_err(|error| session_error(&error))
        })
    }

    /// Look up one lane by application name.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the lane does not exist.
    pub fn lane(&self, name: String) -> js_sys::Promise {
        let session = self.inner.clone();
        executor::drive(async move {
            session
                .lane(&name)
                .await
                .map(|inner| JsValue::from(Lane { inner }))
                .map_err(|error| session_error(&error))
        })
    }

    /// Bind a host-owned external identity to one lane.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the key is invalid, the lane is
    /// unknown, or the key is already bound to a different session lane.
    #[wasm_bindgen(js_name = bindExternalIdentity)]
    pub fn bind_external_identity(
        &self,
        map: &MemoryExternalIdentityMap,
        channel: String,
        account: String,
        thread: String,
        lane_id: String,
    ) -> Result<(), JsValue> {
        let key = finstack_ai::ExternalIdentityKey::try_new(channel, account, thread)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let lane_id = finstack_ai::runtime::LaneId::parse(&lane_id)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        self.inner
            .bind_external_identity(&map.inner, key, lane_id)
            .map_err(|error| session_error(&error))
    }
}

/// Live lane handle.
#[wasm_bindgen(js_name = Lane)]
pub struct Lane {
    pub(super) inner: finstack_ai::Lane,
}

#[wasm_bindgen(js_class = Lane)]
impl Lane {
    /// Durable lane identity.
    #[wasm_bindgen(getter, js_name = laneId)]
    pub fn lane_id(&self) -> String {
        self.inner.lane_id().to_string()
    }

    /// Session that owns this lane.
    #[wasm_bindgen(getter)]
    pub fn session(&self) -> Session {
        Session {
            inner: self.inner.session().clone(),
        }
    }

    /// Point this idle lane at an existing entry without copying.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the entry is unknown or the lane
    /// is busy.
    pub fn navigate(&self, entry_id: String) -> js_sys::Promise {
        let lane = self.inner.clone();
        executor::drive(async move {
            let entry_id = finstack_ai::runtime::EntryId::parse(&entry_id)
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            lane.navigate(entry_id)
                .await
                .map(|()| JsValue::UNDEFINED)
                .map_err(|error| session_error(&error))
        })
    }

    /// Inspect name, leaf, and history length.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the lane cannot be inspected.
    pub fn inspect(&self) -> js_sys::Promise {
        let lane = self.inner.clone();
        executor::drive(async move {
            lane.inspect()
                .await
                .map(|inspect| {
                    let object = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(
                        &object,
                        &JsValue::from_str("laneId"),
                        &JsValue::from_str(&inspect.lane_id.to_string()),
                    );
                    let _ = js_sys::Reflect::set(
                        &object,
                        &JsValue::from_str("name"),
                        &JsValue::from_str(&inspect.name),
                    );
                    let _ = js_sys::Reflect::set(
                        &object,
                        &JsValue::from_str("historyLen"),
                        &JsValue::from_f64(inspect.history.len() as f64),
                    );
                    JsValue::from(object)
                })
                .map_err(|error| session_error(&error))
        })
    }
}

/// In-process external identity map.
#[wasm_bindgen(js_name = MemoryExternalIdentityMap)]
pub struct MemoryExternalIdentityMap {
    pub(super) inner: finstack_ai::MemoryExternalIdentityMap,
}

#[wasm_bindgen(js_class = MemoryExternalIdentityMap)]
impl MemoryExternalIdentityMap {
    /// Empty map.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: finstack_ai::MemoryExternalIdentityMap::new(),
        }
    }

    /// Resolve one previously bound key.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the key is invalid.
    pub fn resolve(
        &self,
        channel: String,
        account: String,
        thread: String,
    ) -> Result<JsValue, JsValue> {
        let key = finstack_ai::ExternalIdentityKey::try_new(channel, account, thread)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        Ok(
            finstack_ai::ExternalIdentityMap::resolve(&self.inner, &key).map_or(
                JsValue::UNDEFINED,
                |(session_id, lane_id)| {
                    let object = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(
                        &object,
                        &JsValue::from_str("sessionId"),
                        &JsValue::from_str(&session_id.to_string()),
                    );
                    let _ = js_sys::Reflect::set(
                        &object,
                        &JsValue::from_str("laneId"),
                        &JsValue::from_str(&lane_id.to_string()),
                    );
                    object.into()
                },
            ),
        )
    }
}
