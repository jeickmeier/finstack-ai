//! Trusted JS / native host implementation of the `MemoryStore` port.
//!
//! [`HostMemoryStore`] proxies the six `MemoryStore` operations as JSON
//! envelopes to host callbacks named `memory_put`, `memory_get`,
//! `memory_search`, `memory_forget`, `memory_correct`, and `memory_list`.
//! Each request/response round-trip is `{"ok": <value>} | {"error":
//! {"code","message"}}`; a missing host method reports a stable
//! `Unavailable` error rather than panicking.
//!
//! [`InProcessMemoryStore`] is re-exported for wasm consumers that skip host
//! persistence entirely.

use std::sync::Arc;

use finstack_ai_memory::record::{MemoryId, MemoryRecord, MemoryScope};
use finstack_ai_memory::store::{
    MatchEvidence, MemoryHit, MemoryListing, MemoryPage, MemoryQuery, MemoryStore,
    MemoryStoreError, PutOutcome,
};

use finstack_ai::runtime::ports::PortFuture;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[cfg(not(target_arch = "wasm32"))]
use crate::host::{HostFailure, NativeHostResult};

/// Stable reason code when a host callback for a `MemoryStore` operation is
/// missing.
const MEMORY_HOST_UNAVAILABLE: &str = "memory_host_unavailable";
/// Stable reason code when a host result cannot be parsed as the expected
/// envelope/DTO shape.
const MEMORY_HOST_RESULT_INVALID: &str = "memory_host_result_invalid";

#[cfg(not(target_arch = "wasm32"))]
type NativeMethod = Arc<dyn Fn(&str) -> Result<NativeHostResult, HostFailure> + Send + Sync>;

/// Native host callbacks backing one [`HostMemoryStore`]. Any field left
/// `None` reports [`MemoryStoreError::Unavailable`] for that operation.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default, Clone)]
pub struct HostMemoryCallbacks {
    /// Backing callback for [`MemoryStore::put`].
    pub put: Option<NativeMethod>,
    /// Backing callback for [`MemoryStore::get`].
    pub get: Option<NativeMethod>,
    /// Backing callback for [`MemoryStore::search`].
    pub search: Option<NativeMethod>,
    /// Backing callback for [`MemoryStore::forget`].
    pub forget: Option<NativeMethod>,
    /// Backing callback for [`MemoryStore::correct`].
    pub correct: Option<NativeMethod>,
    /// Backing callback for [`MemoryStore::list`].
    pub list: Option<NativeMethod>,
}

/// Host-backed [`MemoryStore`]. Delegates every operation to a trusted host
/// (native callbacks in tests, a JS adapter on `wasm32`).
pub struct HostMemoryStore {
    #[cfg(not(target_arch = "wasm32"))]
    put: Option<NativeMethod>,
    #[cfg(not(target_arch = "wasm32"))]
    get: Option<NativeMethod>,
    #[cfg(not(target_arch = "wasm32"))]
    search: Option<NativeMethod>,
    #[cfg(not(target_arch = "wasm32"))]
    forget: Option<NativeMethod>,
    #[cfg(not(target_arch = "wasm32"))]
    correct: Option<NativeMethod>,
    #[cfg(not(target_arch = "wasm32"))]
    list: Option<NativeMethod>,
    #[cfg(target_arch = "wasm32")]
    adapter: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    put: Option<js_sys::Function>,
    #[cfg(target_arch = "wasm32")]
    get: Option<js_sys::Function>,
    #[cfg(target_arch = "wasm32")]
    search: Option<js_sys::Function>,
    #[cfg(target_arch = "wasm32")]
    forget: Option<js_sys::Function>,
    #[cfg(target_arch = "wasm32")]
    correct: Option<js_sys::Function>,
    #[cfg(target_arch = "wasm32")]
    list: Option<js_sys::Function>,
}

impl HostMemoryStore {
    /// Construct a native scripted store from individually optional
    /// callbacks.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn from_callbacks(callbacks: HostMemoryCallbacks) -> Self {
        Self {
            put: callbacks.put,
            get: callbacks.get,
            search: callbacks.search,
            forget: callbacks.forget,
            correct: callbacks.correct,
            list: callbacks.list,
        }
    }

    /// Construct a native scripted store where every operation is backed.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn from_callback_fns(
        put: impl Fn(&str) -> Result<NativeHostResult, HostFailure> + Send + Sync + 'static,
        get: impl Fn(&str) -> Result<NativeHostResult, HostFailure> + Send + Sync + 'static,
        search: impl Fn(&str) -> Result<NativeHostResult, HostFailure> + Send + Sync + 'static,
        forget: impl Fn(&str) -> Result<NativeHostResult, HostFailure> + Send + Sync + 'static,
        correct: impl Fn(&str) -> Result<NativeHostResult, HostFailure> + Send + Sync + 'static,
        list: impl Fn(&str) -> Result<NativeHostResult, HostFailure> + Send + Sync + 'static,
    ) -> Self {
        Self::from_callbacks(HostMemoryCallbacks {
            put: Some(Arc::new(put)),
            get: Some(Arc::new(get)),
            search: Some(Arc::new(search)),
            forget: Some(Arc::new(forget)),
            correct: Some(Arc::new(correct)),
            list: Some(Arc::new(list)),
        })
    }

    /// Construct a wasm32 store around a JS `MemoryStore` adapter. Methods
    /// the adapter does not expose report `Unavailable` when called.
    #[cfg(target_arch = "wasm32")]
    #[must_use]
    pub fn from_js(adapter: wasm_bindgen::JsValue) -> Self {
        let method = |name| crate::host::extract_optional_method(&adapter, name);
        Self {
            put: method("memory_put"),
            get: method("memory_get"),
            search: method("memory_search"),
            forget: method("memory_forget"),
            correct: method("memory_correct"),
            list: method("memory_list"),
            adapter,
        }
    }
}

impl MemoryStore for HostMemoryStore {
    fn put(
        &self,
        idempotency_key: Arc<str>,
        record: MemoryRecord,
    ) -> PortFuture<Result<PutOutcome, MemoryStoreError>> {
        if invalid_new_record(&record) {
            return Box::pin(async {
                Err(MemoryStoreError::InvalidRecord {
                    reason: "memory_host_record_invalid",
                })
            });
        }
        let Ok(encoded) = serde_json::to_string(&PutRequestWire {
            idempotency_key,
            record,
        }) else {
            return Box::pin(async { Err(unavailable(MEMORY_HOST_RESULT_INVALID)) });
        };
        self.call(self.put.clone(), encoded, |body| {
            parse_envelope::<PutOutcomeWire>(body).map(Into::into)
        })
    }

    fn get(
        &self,
        scope: MemoryScope,
        id: MemoryId,
    ) -> PortFuture<Result<Option<MemoryRecord>, MemoryStoreError>> {
        let expected_scope = scope.clone();
        let Ok(encoded) = serde_json::to_string(&GetRequestWire { scope, id }) else {
            return Box::pin(async { Err(unavailable(MEMORY_HOST_RESULT_INVALID)) });
        };
        self.call(self.get.clone(), encoded, move |body| {
            let record = parse_envelope::<Option<MemoryRecord>>(body)?;
            record
                .map(|record| validate_host_record(record, &expected_scope))
                .transpose()
        })
    }

    fn search(
        &self,
        scope: MemoryScope,
        query: MemoryQuery,
        limit: usize,
    ) -> PortFuture<Result<Vec<MemoryHit>, MemoryStoreError>> {
        let query = match QueryWire::try_from_query(&query) {
            Ok(query) => query,
            Err(error) => return Box::pin(async { Err(error) }),
        };
        let expected_scope = scope.clone();
        let Ok(encoded) = serde_json::to_string(&SearchRequestWire {
            scope,
            query,
            limit,
        }) else {
            return Box::pin(async { Err(unavailable(MEMORY_HOST_RESULT_INVALID)) });
        };
        self.call(self.search.clone(), encoded, move |body| {
            parse_envelope::<Vec<MemoryHitWire>>(body)?
                .into_iter()
                .map(|hit| {
                    let hit: MemoryHit = hit.into();
                    validate_host_record(hit.record.clone(), &expected_scope)?;
                    Ok(hit)
                })
                .collect()
        })
    }

    fn forget(
        &self,
        idempotency_key: Arc<str>,
        scope: MemoryScope,
        id: MemoryId,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        let Ok(encoded) = serde_json::to_string(&ForgetRequestWire {
            idempotency_key,
            scope,
            id,
        }) else {
            return Box::pin(async { Err(unavailable(MEMORY_HOST_RESULT_INVALID)) });
        };
        self.call(self.forget.clone(), encoded, |body| {
            parse_envelope::<()>(body)
        })
    }

    fn correct(
        &self,
        idempotency_key: Arc<str>,
        scope: MemoryScope,
        old: MemoryId,
        replacement: MemoryRecord,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        if invalid_new_record(&replacement) || replacement.scope != scope {
            return Box::pin(async {
                Err(MemoryStoreError::InvalidRecord {
                    reason: "memory_host_record_invalid",
                })
            });
        }
        let Ok(encoded) = serde_json::to_string(&CorrectRequestWire {
            idempotency_key,
            scope,
            old,
            replacement,
        }) else {
            return Box::pin(async { Err(unavailable(MEMORY_HOST_RESULT_INVALID)) });
        };
        self.call(self.correct.clone(), encoded, |body| {
            parse_envelope::<()>(body)
        })
    }

    fn list(
        &self,
        scope: MemoryScope,
        page: MemoryPage,
    ) -> PortFuture<Result<MemoryListing, MemoryStoreError>> {
        let expected_scope = scope.clone();
        let Ok(encoded) = serde_json::to_string(&ListRequestWire {
            scope,
            page: PageWire::from(page),
        }) else {
            return Box::pin(async { Err(unavailable(MEMORY_HOST_RESULT_INVALID)) });
        };
        self.call(self.list.clone(), encoded, move |body| {
            let listing: MemoryListing = parse_envelope::<MemoryListingWire>(body)?.into();
            for record in &listing.records {
                validate_host_record(record.clone(), &expected_scope)?;
            }
            Ok(listing)
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
type OptionalMethod = Option<NativeMethod>;
#[cfg(target_arch = "wasm32")]
type OptionalMethod = Option<js_sys::Function>;

impl HostMemoryStore {
    /// Dispatch one host round-trip for `encoded`, decoding the envelope
    /// body with `decode`. Common to every `MemoryStore` operation; a
    /// missing `method` reports `Unavailable` without calling the host.
    // `self` is only read on wasm32 (to clone `self.adapter`); the native
    // arm is a free function in all but name.
    #[allow(clippy::unused_self)]
    fn call<T: Send + 'static>(
        &self,
        method: OptionalMethod,
        encoded: String,
        decode: impl FnOnce(&str) -> Result<T, MemoryStoreError> + Send + 'static,
    ) -> PortFuture<Result<T, MemoryStoreError>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(callback) = method else {
                return Box::pin(async { Err(unavailable(MEMORY_HOST_UNAVAILABLE)) });
            };
            Box::pin(async move {
                let result = callback(&encoded).map_err(native_failure)?;
                let body = native_result_body(result)?;
                decode(&body)
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let Some(method) = method else {
                return Box::pin(async { Err(unavailable(MEMORY_HOST_UNAVAILABLE)) });
            };
            let adapter = self.adapter.clone();
            Box::pin(async move {
                let body = invoke_memory_json(&adapter, &method, &encoded).await?;
                decode(&body)
            })
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn native_failure(failure: HostFailure) -> MemoryStoreError {
    unavailable(failure.message())
}

#[cfg(not(target_arch = "wasm32"))]
fn native_result_body(result: NativeHostResult) -> Result<String, MemoryStoreError> {
    match result {
        NativeHostResult::Object(body) => Ok(body),
        NativeHostResult::Items(_) => Err(unavailable(MEMORY_HOST_RESULT_INVALID)),
    }
}

#[cfg(target_arch = "wasm32")]
async fn invoke_memory_json(
    adapter: &wasm_bindgen::JsValue,
    method: &js_sys::Function,
    encoded: &str,
) -> Result<String, MemoryStoreError> {
    let result = crate::host::invoke_host(
        adapter,
        method,
        &[wasm_bindgen::JsValue::from_str(encoded)],
        None,
    )
    .await
    .map_err(|_| unavailable(MEMORY_HOST_UNAVAILABLE))?;
    let crate::host::HostJsResult::Value(value) = result else {
        return Err(unavailable(MEMORY_HOST_RESULT_INVALID));
    };
    crate::host::stringify_js(&value).map_err(|_| unavailable(MEMORY_HOST_RESULT_INVALID))
}

fn unavailable(message: &str) -> MemoryStoreError {
    MemoryStoreError::Unavailable {
        message: Arc::from(message),
    }
}

fn validate_host_record(
    record: MemoryRecord,
    expected_scope: &MemoryScope,
) -> Result<MemoryRecord, MemoryStoreError> {
    record
        .validate()
        .map_err(|_| MemoryStoreError::InvalidRecord {
            reason: "memory_host_record_invalid",
        })?;
    if &record.scope != expected_scope {
        return Err(MemoryStoreError::ScopeMismatch);
    }
    if record.tombstoned || record.superseded_by.is_some() {
        return Err(MemoryStoreError::InvalidRecord {
            reason: "memory_host_record_not_live",
        });
    }
    Ok(record)
}

fn invalid_new_record(record: &MemoryRecord) -> bool {
    record.validate().is_err()
        || record.tombstoned
        || record.supersedes.is_some()
        || record.superseded_by.is_some()
}

/// Parse a `{"ok": T} | {"error": {"code","message"}}` envelope.
///
/// Looks the `"ok"` key up directly on the parsed JSON object rather than
/// deserializing straight into `Option<Value>`: serde's `Option`
/// deserialization treats a present-but-`null` value the same as an absent
/// key, which would make a legitimate `{"ok": null}` (e.g. `forget`'s unit
/// result, or `get` finding nothing) indistinguishable from a malformed
/// envelope with neither key.
fn parse_envelope<T: DeserializeOwned>(encoded: &str) -> Result<T, MemoryStoreError> {
    let value: serde_json::Value =
        serde_json::from_str(encoded).map_err(|_| unavailable(MEMORY_HOST_RESULT_INVALID))?;
    let object = value
        .as_object()
        .ok_or_else(|| unavailable(MEMORY_HOST_RESULT_INVALID))?;
    if let Some(error) = object.get("error") {
        let error: ErrorWire = serde_json::from_value(error.clone())
            .map_err(|_| unavailable(MEMORY_HOST_RESULT_INVALID))?;
        return Err(error_from_wire(error));
    }
    let ok = object
        .get("ok")
        .ok_or_else(|| unavailable(MEMORY_HOST_RESULT_INVALID))?;
    serde_json::from_value(ok.clone()).map_err(|_| unavailable(MEMORY_HOST_RESULT_INVALID))
}

#[derive(Deserialize)]
struct ErrorWire {
    code: String,
    message: String,
}

fn error_from_wire(error: ErrorWire) -> MemoryStoreError {
    match error.code.as_str() {
        "memory_not_found" => MemoryStoreError::NotFound,
        "memory_scope_mismatch" => MemoryStoreError::ScopeMismatch,
        "memory_id_conflict" => MemoryStoreError::IdConflict,
        _ => MemoryStoreError::Unavailable {
            message: Arc::from(error.message),
        },
    }
}

#[derive(Serialize)]
struct PutRequestWire {
    idempotency_key: Arc<str>,
    record: MemoryRecord,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PutOutcomeWire {
    Inserted,
    AlreadyApplied,
}

impl From<PutOutcomeWire> for PutOutcome {
    fn from(wire: PutOutcomeWire) -> Self {
        match wire {
            PutOutcomeWire::Inserted => Self::Inserted,
            PutOutcomeWire::AlreadyApplied => Self::AlreadyApplied,
        }
    }
}

#[derive(Serialize)]
struct GetRequestWire {
    scope: MemoryScope,
    id: MemoryId,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum QueryWire {
    ExactId(Arc<str>),
    Keywords(Arc<[Arc<str>]>),
    FullText(Arc<str>),
}

impl QueryWire {
    /// Convert a [`MemoryQuery`] to its wire form.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::Unavailable`] for a query variant added
    /// to the `#[non_exhaustive]` [`MemoryQuery`] enum after this bridge was
    /// written.
    fn try_from_query(query: &MemoryQuery) -> Result<Self, MemoryStoreError> {
        match query {
            MemoryQuery::ExactId(id) => Ok(Self::ExactId(Arc::from(id.as_str()))),
            MemoryQuery::Keywords(words) => Ok(Self::Keywords(Arc::clone(words))),
            MemoryQuery::FullText(text) => Ok(Self::FullText(Arc::clone(text))),
            _ => Err(unavailable(MEMORY_HOST_RESULT_INVALID)),
        }
    }
}

#[derive(Serialize)]
struct SearchRequestWire {
    scope: MemoryScope,
    query: QueryWire,
    limit: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MatchEvidenceWire {
    ExactId,
    Keyword(Arc<str>),
    FullText,
}

impl From<MatchEvidenceWire> for MatchEvidence {
    fn from(wire: MatchEvidenceWire) -> Self {
        match wire {
            MatchEvidenceWire::ExactId => Self::ExactId,
            MatchEvidenceWire::Keyword(keyword) => Self::Keyword(keyword),
            MatchEvidenceWire::FullText => Self::FullText,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct MemoryHitWire {
    record: MemoryRecord,
    score: u32,
    matched: MatchEvidenceWire,
}

impl From<MemoryHitWire> for MemoryHit {
    fn from(wire: MemoryHitWire) -> Self {
        Self {
            record: wire.record,
            score: wire.score,
            matched: wire.matched.into(),
        }
    }
}

#[derive(Serialize)]
struct ForgetRequestWire {
    idempotency_key: Arc<str>,
    scope: MemoryScope,
    id: MemoryId,
}

#[derive(Serialize)]
struct CorrectRequestWire {
    idempotency_key: Arc<str>,
    scope: MemoryScope,
    old: MemoryId,
    replacement: MemoryRecord,
}

#[derive(Serialize, Deserialize)]
struct PageWire {
    offset: usize,
    limit: usize,
}

impl From<MemoryPage> for PageWire {
    fn from(page: MemoryPage) -> Self {
        Self {
            offset: page.offset,
            limit: page.limit,
        }
    }
}

#[derive(Serialize)]
struct ListRequestWire {
    scope: MemoryScope,
    page: PageWire,
}

#[derive(Serialize, Deserialize)]
struct MemoryListingWire {
    records: Vec<MemoryRecord>,
    total: usize,
}

impl From<MemoryListingWire> for MemoryListing {
    fn from(wire: MemoryListingWire) -> Self {
        Self {
            records: wire.records,
            total: wire.total,
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{HostMemoryStore, MEMORY_HOST_UNAVAILABLE};
    use crate::executor::block_on_ready;
    use crate::host::NativeHostResult;
    use finstack_ai_kernel::{Sensitivity, Timestamp};
    use finstack_ai_memory::record::{
        ExtractionMethod, MemoryBody, MemoryId, MemoryProvenance, MemoryRecord, MemoryScope,
        RetentionPolicy,
    };
    use finstack_ai_memory::store::{
        MatchEvidence, MemoryPage, MemoryQuery, MemoryStore, MemoryStoreError, PutOutcome,
    };
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn record(id: &str, scope: MemoryScope) -> MemoryRecord {
        MemoryRecord {
            id: MemoryId::parse(id).expect("id"),
            scope,
            keywords: Arc::from([]),
            body: MemoryBody::Inline(Arc::from("hello world")),
            preview: Arc::from("hello world"),
            sensitivity: Sensitivity::Internal,
            provenance: MemoryProvenance {
                source_session: None,
                source_run: None,
                source_ref: None,
                extraction: ExtractionMethod::Explicit,
                confidence: 80,
            },
            created_at: Timestamp::from_unix_ms(0).expect("ts"),
            last_confirmed_at: Timestamp::from_unix_ms(0).expect("ts"),
            supersedes: None,
            superseded_by: None,
            retention: RetentionPolicy::KeepUntilDeleted,
            tombstoned: false,
        }
    }

    fn scope(tenant: &str) -> MemoryScope {
        MemoryScope::try_new(tenant).expect("scope")
    }

    #[derive(Default)]
    struct ScriptedState {
        records: HashMap<(MemoryScope, String), MemoryRecord>,
        applied_keys: std::collections::HashSet<(MemoryScope, String)>,
    }

    /// A `HashMap`-backed scripted host implementing the six `memory_*`
    /// operations, exercised through envelope JSON exactly as a JS host
    /// would return it.
    #[derive(Clone, Default)]
    struct ScriptedHost {
        state: Arc<Mutex<ScriptedState>>,
    }

    fn ok_envelope(value: &serde_json::Value) -> NativeHostResult {
        NativeHostResult::Object(serde_json::json!({ "ok": value }).to_string())
    }

    fn error_envelope(code: &str, message: &str) -> NativeHostResult {
        NativeHostResult::Object(
            serde_json::json!({ "error": { "code": code, "message": message } }).to_string(),
        )
    }

    impl ScriptedHost {
        // One scripted closure per `memory_*` operation, inlined for
        // readability rather than split into six near-identical helpers.
        #[allow(clippy::too_many_lines)]
        fn store(self) -> HostMemoryStore {
            let put_state = self.state.clone();
            let get_state = self.state.clone();
            let search_state = self.state.clone();
            let forget_state = self.state.clone();
            let correct_state = self.state.clone();
            let list_state = self.state.clone();
            HostMemoryStore::from_callback_fns(
                move |encoded| {
                    let request: serde_json::Value =
                        serde_json::from_str(encoded).expect("valid put request");
                    let idempotency_key = request["idempotency_key"]
                        .as_str()
                        .expect("idempotency key")
                        .to_owned();
                    let record: MemoryRecord =
                        serde_json::from_value(request["record"].clone()).expect("record");
                    let mut state = put_state.lock().expect("lock");
                    let scoped_key = (record.scope.clone(), idempotency_key);
                    if state.applied_keys.contains(&scoped_key) {
                        return Ok(ok_envelope(&serde_json::json!("already_applied")));
                    }
                    let record_key = (record.scope.clone(), record.id.as_str().to_owned());
                    if state.records.contains_key(&record_key) {
                        return Ok(error_envelope("memory_id_conflict", "id already taken"));
                    }
                    state.applied_keys.insert(scoped_key);
                    state.records.insert(record_key, record);
                    Ok(ok_envelope(&serde_json::json!("inserted")))
                },
                move |encoded| {
                    let request: serde_json::Value =
                        serde_json::from_str(encoded).expect("valid get request");
                    let requested_scope: MemoryScope =
                        serde_json::from_value(request["scope"].clone()).expect("scope");
                    let id = request["id"].as_str().expect("id").to_owned();
                    let state = get_state.lock().expect("lock");
                    let found = state
                        .records
                        .get(&(requested_scope, id))
                        .filter(|record| !record.tombstoned && record.superseded_by.is_none());
                    Ok(ok_envelope(&serde_json::to_value(found).expect("value")))
                },
                move |encoded| {
                    let request: serde_json::Value =
                        serde_json::from_str(encoded).expect("valid search request");
                    let requested_scope: MemoryScope =
                        serde_json::from_value(request["scope"].clone()).expect("scope");
                    let full_text = request["query"]["full_text"].as_str().map(str::to_owned);
                    let state = search_state.lock().expect("lock");
                    let mut hits = Vec::new();
                    for record in state.records.values() {
                        if record.tombstoned
                            || record.superseded_by.is_some()
                            || !requested_scope.permits(&record.scope)
                        {
                            continue;
                        }
                        if let Some(text) = &full_text
                            && record.preview.contains(text.as_str())
                        {
                            hits.push(serde_json::json!({
                                "record": record,
                                "score": 1,
                                "matched": "full_text",
                            }));
                        }
                    }
                    Ok(ok_envelope(&serde_json::Value::Array(hits)))
                },
                move |encoded| {
                    let request: serde_json::Value =
                        serde_json::from_str(encoded).expect("valid forget request");
                    let requested_scope: MemoryScope =
                        serde_json::from_value(request["scope"].clone()).expect("scope");
                    let id = request["id"].as_str().expect("id").to_owned();
                    let mut state = forget_state.lock().expect("lock");
                    let Some(record) = state.records.get_mut(&(requested_scope, id)) else {
                        return Ok(error_envelope("memory_not_found", "no such record"));
                    };
                    record.tombstoned = true;
                    Ok(ok_envelope(&serde_json::Value::Null))
                },
                move |encoded| {
                    let request: serde_json::Value =
                        serde_json::from_str(encoded).expect("valid correct request");
                    let requested_scope: MemoryScope =
                        serde_json::from_value(request["scope"].clone()).expect("scope");
                    let old = request["old"].as_str().expect("old").to_owned();
                    let replacement: MemoryRecord =
                        serde_json::from_value(request["replacement"].clone()).expect("record");
                    let mut state = correct_state.lock().expect("lock");
                    let old_key = (requested_scope.clone(), old);
                    let Some(old_record) = state.records.get(&old_key).cloned() else {
                        return Ok(error_envelope("memory_not_found", "no such record"));
                    };
                    let mut old_record = old_record;
                    old_record.superseded_by = Some(replacement.id.clone());
                    let mut replacement = replacement;
                    replacement.supersedes = Some(old_record.id.clone());
                    state.records.insert(old_key, old_record);
                    state.records.insert(
                        (requested_scope, replacement.id.as_str().to_owned()),
                        replacement,
                    );
                    Ok(ok_envelope(&serde_json::Value::Null))
                },
                move |encoded| {
                    let request: serde_json::Value =
                        serde_json::from_str(encoded).expect("valid list request");
                    let requested_scope: MemoryScope =
                        serde_json::from_value(request["scope"].clone()).expect("scope");
                    let offset =
                        usize::try_from(request["page"]["offset"].as_u64().expect("offset"))
                            .expect("offset fits usize");
                    let limit = usize::try_from(request["page"]["limit"].as_u64().expect("limit"))
                        .expect("limit fits usize");
                    let state = list_state.lock().expect("lock");
                    let mut matching: Vec<&MemoryRecord> = state
                        .records
                        .values()
                        .filter(|record| {
                            requested_scope.permits(&record.scope)
                                && !record.tombstoned
                                && record.superseded_by.is_none()
                        })
                        .collect();
                    matching.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
                    let total = matching.len();
                    let page: Vec<MemoryRecord> = matching
                        .into_iter()
                        .skip(offset)
                        .take(limit)
                        .cloned()
                        .collect();
                    Ok(ok_envelope(
                        &serde_json::json!({ "records": page, "total": total }),
                    ))
                },
            )
        }
    }

    #[test]
    fn native_host_memory_store_put_is_idempotent() {
        let store = ScriptedHost::default().store();
        let key: Arc<str> = Arc::from("key-1");
        let outcome =
            block_on_ready(store.put(key.clone(), record("m1", scope("tenant-a")))).expect("put");
        assert_eq!(outcome, PutOutcome::Inserted);
        let outcome = block_on_ready(store.put(key, record("m1", scope("tenant-a")))).expect("put");
        assert_eq!(outcome, PutOutcome::AlreadyApplied);
    }

    #[test]
    fn native_host_memory_store_excludes_tombstoned_from_search() {
        let store = ScriptedHost::default().store();
        block_on_ready(store.put(Arc::from("k1"), record("m1", scope("tenant-a")))).expect("put");
        let hits = block_on_ready(store.search(
            scope("tenant-a"),
            MemoryQuery::FullText(Arc::from("hello")),
            10,
        ))
        .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matched, MatchEvidence::FullText);

        block_on_ready(store.forget(
            Arc::from("k2"),
            scope("tenant-a"),
            MemoryId::parse("m1").expect("id"),
        ))
        .expect("forget");

        let hits = block_on_ready(store.search(
            scope("tenant-a"),
            MemoryQuery::FullText(Arc::from("hello")),
            10,
        ))
        .expect("search");
        assert!(hits.is_empty());
    }

    #[test]
    fn native_host_memory_store_filters_by_scope() {
        let store = ScriptedHost::default().store();
        block_on_ready(store.put(Arc::from("k1"), record("m1", scope("tenant-a")))).expect("put");
        block_on_ready(store.put(Arc::from("k2"), record("m2", scope("tenant-b")))).expect("put");

        let listing = block_on_ready(store.list(
            scope("tenant-a"),
            MemoryPage {
                offset: 0,
                limit: 50,
            },
        ))
        .expect("list");
        assert_eq!(listing.total, 1);
        assert_eq!(listing.records[0].id.as_str(), "m1");

        let found =
            block_on_ready(store.get(scope("tenant-b"), MemoryId::parse("m1").expect("id")))
                .expect("get");
        assert!(found.is_none());
    }

    #[test]
    fn native_host_memory_store_correct_supersedes() {
        let store = ScriptedHost::default().store();
        block_on_ready(store.put(Arc::from("k1"), record("m1", scope("tenant-a")))).expect("put");
        block_on_ready(store.correct(
            Arc::from("k2"),
            scope("tenant-a"),
            MemoryId::parse("m1").expect("id"),
            record("m2", scope("tenant-a")),
        ))
        .expect("correct");

        let old = block_on_ready(store.get(scope("tenant-a"), MemoryId::parse("m1").expect("id")))
            .expect("get");
        assert!(old.is_none());
        let replacement =
            block_on_ready(store.get(scope("tenant-a"), MemoryId::parse("m2").expect("id")))
                .expect("get")
                .expect("replacement");
        assert_eq!(
            replacement
                .supersedes
                .as_ref()
                .map(finstack_ai_memory::record::MemoryId::as_str),
            Some("m1")
        );
    }

    #[test]
    fn native_host_memory_store_maps_not_found_without_scope_disclosure() {
        let store = ScriptedHost::default().store();
        let missing = block_on_ready(store.forget(
            Arc::from("k1"),
            scope("tenant-a"),
            MemoryId::parse("missing").expect("id"),
        ));
        assert_eq!(missing, Err(MemoryStoreError::NotFound));

        block_on_ready(store.put(Arc::from("k2"), record("m1", scope("tenant-a")))).expect("put");
        let mismatched = block_on_ready(store.forget(
            Arc::from("k3"),
            scope("tenant-b"),
            MemoryId::parse("m1").expect("id"),
        ));
        assert_eq!(mismatched, Err(MemoryStoreError::NotFound));
    }

    #[test]
    fn native_host_memory_store_isolates_identical_ids_by_scope() {
        let store = ScriptedHost::default().store();
        block_on_ready(store.put(Arc::from("k1"), record("m1", scope("tenant-a")))).expect("put");
        let second = block_on_ready(store.put(Arc::from("k2"), record("m1", scope("tenant-b"))))
            .expect("second scope");
        assert_eq!(second, PutOutcome::Inserted);
    }

    #[test]
    fn native_host_memory_store_missing_method_is_unavailable() {
        let store = HostMemoryStore::from_callbacks(super::HostMemoryCallbacks::default());
        let result =
            block_on_ready(store.get(scope("tenant-a"), MemoryId::parse("m1").expect("id")));
        let Err(MemoryStoreError::Unavailable { message }) = result else {
            panic!("expected unavailable");
        };
        assert_eq!(message.as_ref(), MEMORY_HOST_UNAVAILABLE);
    }

    #[test]
    fn native_host_memory_store_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<HostMemoryStore>();
    }
}
