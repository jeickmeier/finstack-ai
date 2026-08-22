//! Six-port compile fixtures for the wasm package.
//!
//! Native builds keep `Send + Sync` handles. `wasm32` builds store JS promise
//! factories in `Rc<RefCell<_>>` so the handles stay local (`!Send`).

use std::sync::Arc;

use finstack_ai::runtime::ports::PortFuture;
use finstack_ai::runtime::ports::context::{
    ContextCallContext, ContextContribution, ContextError, ContextProvider,
    ContextProviderDescriptor, ContextRequest,
};
use finstack_ai::runtime::ports::journal::{
    JournalStore, JournalStoreDescriptor, LoadRequest, LoadedSession, SnapshotReceipt,
    SnapshotRequest, StoreError, StoreHealth,
};
use finstack_ai::runtime::ports::middleware::{
    Middleware, MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder,
    MiddlewareRole, OrderTier, StageInput, StageMask, StageOutcome,
};
use finstack_ai::runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, InputCapabilities, Model, ModelCapabilities,
    ModelContextProfile, ModelDescriptor, ModelError, ModelEventStream, ModelName, ModelRequest,
    ModelResponse, ModelStreamItem, ModelTokenEstimate, SideEffectClass,
    StructuredOutputCapability, TokenEstimatorRef, TokenEstimatorSource, ToolDeferralSupport,
    ToolSpec,
};
use finstack_ai::runtime::ports::observer::{
    Observer, ObserverDescriptor, ObserverError, ObserverPayloadMode,
};
use finstack_ai::runtime::ports::tool::{
    ToolCallContext, ToolError, ToolEventStream, ToolResult, ToolStreamItem, Toolset,
    ToolsetDescriptor,
};
use finstack_ai_kernel::{AppendRequest, CommittedBatch};
use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ComponentRef, Digest, InvocationRecovery, Metadata,
    ProviderIds, RawJson, RetrySafety, RunEvent, Stage, ToolExecutionMode, ToolId, Usage,
    ValidatedToolCall, Version,
};

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;

#[cfg(target_arch = "wasm32")]
use js_sys::Promise;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::JsFuture;

fn model_profile() -> Result<ModelContextProfile, ModelError> {
    Ok(ModelContextProfile {
        provider: Arc::from("wasm-proxy"),
        model: ModelName::try_new("wasm-proxy-model")?,
        hard_input_bytes: 1_024,
        context_window_tokens: 1_024,
        max_output_tokens: 128,
        reserved_output_tokens: 128,
        provider_overhead_tokens: 0,
        estimator: TokenEstimatorRef {
            id: Arc::from("wasm-proxy-bytes"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    })
}

fn model_response() -> ModelResponse {
    ModelResponse {
        assistant_content: Arc::from([]),
        tool_calls: Arc::from([]),
        usage: Usage::empty(),
        provider_ids: ProviderIds::empty(),
        completion_id: Arc::from("wasm-proxy-completion"),
        continuation_state: None,
    }
}

fn tool_spec() -> ToolSpec {
    ToolSpec {
        id: finstack_ai_kernel::static_key!(ToolId, "finstack.tools.wasm_proxy"),
        model_name: Arc::from("wasm_proxy"),
        title: Arc::from("WASM proxy"),
        description: Arc::from("Compile-only JS promise proxy"),
        input_schema: RawJson::parse(
            br#"{"additionalProperties":false,"properties":{},"type":"object"}"#,
        )
        .unwrap_or_else(|_| Metadata::empty().as_raw_json().clone()),
        output_schema: None,
        execution: ToolExecutionMode::Sequential,
        side_effect: SideEffectClass::ReadOnly,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 1_024,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    }
}

fn tool_result() -> ToolResult {
    ToolResult {
        output: RawJson::parse(br#"{"ok":true}"#)
            .unwrap_or_else(|_| Metadata::empty().as_raw_json().clone()),
        is_error: false,
    }
}

fn invocation(component: ComponentId) -> ComponentInvocation {
    ComponentInvocation {
        component,
        version: Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        configuration_digest: Digest::raw_json(b"{}"),
        recovery: InvocationRecovery::RecomputeSafe,
    }
}

struct SingleItemStream<T> {
    item: Option<T>,
}

impl<T: Unpin> futures_util::Stream for SingleItemStream<T> {
    type Item = T;

    fn poll_next(
        self: core::pin::Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        core::task::Poll::Ready(self.get_mut().item.take())
    }
}

fn model_capabilities(profile: &ModelContextProfile) -> ModelCapabilities {
    ModelCapabilities {
        input: InputCapabilities {
            text: true,
            json: false,
            images: false,
            audio: false,
            files: false,
        },
        context_profile: profile.clone(),
        native_tool_calls: false,
        parallel_tool_calls: false,
        structured_output: StructuredOutputCapability::Unsupported,
        reasoning: false,
        prompt_cache: false,
        resumable_stream: false,
        idempotent_requests: false,
        native_capabilities: std::collections::BTreeSet::default(),
    }
}

/// Native `Send + Sync` stand-in for the Model port.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct NativeModelProxy {
    profile: ModelContextProfile,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeModelProxy {
    /// Construct the native compile fixture.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] when the fixture model name is invalid.
    pub fn new() -> Result<Self, ModelError> {
        Ok(Self {
            profile: model_profile()?,
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Model for NativeModelProxy {
    fn descriptor(&self) -> ModelDescriptor {
        ModelDescriptor {
            provider: Arc::clone(&self.profile.provider),
            models: Arc::from([self.profile.model.clone()]),
            metadata: Metadata::empty(),
        }
    }

    fn capabilities(&self, _model: &ModelName) -> ModelCapabilities {
        model_capabilities(&self.profile)
    }

    fn estimate_input_tokens(
        &self,
        _model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        Ok(ModelTokenEstimate {
            input_tokens: u64::try_from(canonical_request.len()).unwrap_or(u64::MAX),
            estimator: self.profile.estimator.clone(),
        })
    }

    fn request(&self, _request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        Box::pin(async {
            Ok(Box::pin(SingleItemStream {
                item: Some(Ok(ModelStreamItem::Completed(model_response()))),
            }) as ModelEventStream)
        })
    }
}

/// Native `Send + Sync` stand-in for the Toolset port.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Default)]
pub struct NativeToolsetProxy;

#[cfg(not(target_arch = "wasm32"))]
impl Toolset for NativeToolsetProxy {
    fn descriptor(&self) -> ToolsetDescriptor {
        ToolsetDescriptor {
            name: Arc::from("wasm-native-toolset"),
            metadata: Metadata::empty(),
        }
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::from([tool_spec()])
    }

    fn call(
        &self,
        _ctx: ToolCallContext,
        _call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        Box::pin(async {
            Ok(Box::pin(SingleItemStream {
                item: Some(Ok(ToolStreamItem::Completed(tool_result()))),
            }) as ToolEventStream)
        })
    }
}

/// Native `Send + Sync` stand-in for the context-provider port.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Default)]
pub struct NativeContextProviderProxy;

#[cfg(not(target_arch = "wasm32"))]
impl ContextProvider for NativeContextProviderProxy {
    fn descriptor(&self) -> ContextProviderDescriptor {
        ContextProviderDescriptor {
            invocation: invocation(finstack_ai_kernel::static_key!(
                ComponentId,
                "wasm.native.context"
            )),
            trusted_application_instructions: false,
            metadata: Metadata::empty(),
        }
    }

    fn collect(
        &self,
        _ctx: ContextCallContext,
        _request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        Box::pin(async { ContextContribution::try_new(Vec::new(), None::<&str>) })
    }
}

/// Native `Send + Sync` stand-in for the middleware port.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Default)]
pub struct NativeMiddlewareProxy;

#[cfg(not(target_arch = "wasm32"))]
impl Middleware for NativeMiddlewareProxy {
    fn descriptor(&self) -> MiddlewareDescriptor {
        MiddlewareDescriptor {
            invocation: invocation(finstack_ai_kernel::static_key!(
                ComponentId,
                "wasm.native.middleware"
            )),
            stages: StageMask::from_stages([Stage::BeforeRun]),
            order: MiddlewareOrder {
                tier: OrderTier::Standard,
                priority: 0,
                before: Arc::from([]),
                after: Arc::from([]),
            },
            role: MiddlewareRole::Standard,
            metadata: Metadata::empty(),
        }
    }

    fn invoke(
        &self,
        _ctx: MiddlewareContext,
        _input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        Box::pin(async { Ok(StageOutcome::Continue) })
    }
}

/// Native `Send + Sync` stand-in for the journal-store port.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub struct NativeJournalStoreProxy(std::sync::Mutex<()>);

#[cfg(not(target_arch = "wasm32"))]
impl JournalStore for NativeJournalStoreProxy {
    fn descriptor(&self) -> JournalStoreDescriptor {
        JournalStoreDescriptor::unspecified()
    }

    fn append(&self, _request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        drop(self.0.lock());
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "native_compile_fixture",
            })
        })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        Box::pin(async move { Ok(LoadedSession::empty(request.session_id)) })
    }

    fn write_snapshot(
        &self,
        _request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "native_compile_fixture",
            })
        })
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        Box::pin(async {
            Ok(StoreHealth {
                ready: true,
                durable: false,
                detail: Arc::from("native_compile_fixture"),
            })
        })
    }
}

/// Native `Send + Sync` stand-in for the observer port.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Default)]
pub struct NativeObserverProxy;

#[cfg(not(target_arch = "wasm32"))]
impl Observer for NativeObserverProxy {
    fn descriptor(&self) -> ObserverDescriptor {
        ObserverDescriptor {
            component: ComponentRef::new(
                finstack_ai_kernel::static_key!(ComponentId, "wasm.native.observer"),
                Some(Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                }),
            ),
            payload_mode: ObserverPayloadMode::MetadataOnly,
            metadata: Metadata::empty(),
        }
    }

    fn observe(&self, _batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Construct and type-check the six native port fixtures.
#[cfg(not(target_arch = "wasm32"))]
pub fn compile_native_port_proxies() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<NativeModelProxy>();
    assert_send_sync::<NativeToolsetProxy>();
    assert_send_sync::<NativeContextProviderProxy>();
    assert_send_sync::<NativeMiddlewareProxy>();
    assert_send_sync::<NativeJournalStoreProxy>();
    assert_send_sync::<NativeObserverProxy>();
    if let Ok(model) = NativeModelProxy::new() {
        let _: Arc<dyn Model> = Arc::new(model);
    }
    let _: Arc<dyn Toolset> = Arc::new(NativeToolsetProxy);
    let _: Arc<dyn ContextProvider> = Arc::new(NativeContextProviderProxy);
    let _: Arc<dyn Middleware> = Arc::new(NativeMiddlewareProxy);
    let _: Arc<dyn JournalStore> = Arc::new(NativeJournalStoreProxy::default());
    let _: Arc<dyn Observer> = Arc::new(NativeObserverProxy);
    crate::compile_native_host_adapters();
}

#[cfg(target_arch = "wasm32")]
fn promise_factory(source: &str) -> js_sys::Function {
    js_sys::Function::new_no_args(source)
}

#[cfg(target_arch = "wasm32")]
async fn await_promise(function: js_sys::Function) -> Result<(), ()> {
    let value = function
        .call0(&wasm_bindgen::JsValue::NULL)
        .map_err(|_| ())?;
    let promise = value.dyn_into::<Promise>().map_err(|_| ())?;
    JsFuture::from(promise).await.map(|_| ()).map_err(|_| ())
}

/// Local JS promise proxy for the Model port.
#[cfg(target_arch = "wasm32")]
pub struct JsModelProxy {
    profile: ModelContextProfile,
    request: Rc<RefCell<js_sys::Function>>,
}

#[cfg(target_arch = "wasm32")]
impl JsModelProxy {
    /// Construct a local model proxy around a JS promise factory.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] when the fixture model name is invalid.
    pub fn new(request: js_sys::Function) -> Result<Self, ModelError> {
        Ok(Self {
            profile: model_profile()?,
            request: Rc::new(RefCell::new(request)),
        })
    }
}

#[cfg(target_arch = "wasm32")]
impl Model for JsModelProxy {
    fn descriptor(&self) -> ModelDescriptor {
        ModelDescriptor {
            provider: Arc::clone(&self.profile.provider),
            models: Arc::from([self.profile.model.clone()]),
            metadata: Metadata::empty(),
        }
    }

    fn capabilities(&self, _model: &ModelName) -> ModelCapabilities {
        model_capabilities(&self.profile)
    }

    fn estimate_input_tokens(
        &self,
        _model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        Ok(ModelTokenEstimate {
            input_tokens: u64::try_from(canonical_request.len()).unwrap_or(u64::MAX),
            estimator: self.profile.estimator.clone(),
        })
    }

    fn request(&self, _request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        let function = self.request.borrow().clone();
        Box::pin(async move {
            let _ = await_promise(function).await;
            Ok(Box::pin(SingleItemStream {
                item: Some(Ok(ModelStreamItem::Completed(model_response()))),
            }) as ModelEventStream)
        })
    }
}

/// Local JS promise proxy for the Toolset port.
#[cfg(target_arch = "wasm32")]
pub struct JsToolsetProxy {
    call: Rc<RefCell<js_sys::Function>>,
}

#[cfg(target_arch = "wasm32")]
impl JsToolsetProxy {
    /// Construct a local toolset proxy around a JS promise factory.
    #[must_use]
    pub fn new(call: js_sys::Function) -> Self {
        Self {
            call: Rc::new(RefCell::new(call)),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Toolset for JsToolsetProxy {
    fn descriptor(&self) -> ToolsetDescriptor {
        ToolsetDescriptor {
            name: Arc::from("wasm-js-toolset"),
            metadata: Metadata::empty(),
        }
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::from([tool_spec()])
    }

    fn call(
        &self,
        _ctx: ToolCallContext,
        _call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let function = self.call.borrow().clone();
        Box::pin(async move {
            let _ = await_promise(function).await;
            Ok(Box::pin(SingleItemStream {
                item: Some(Ok(ToolStreamItem::Completed(tool_result()))),
            }) as ToolEventStream)
        })
    }
}

/// Local JS promise proxy for the context-provider port.
#[cfg(target_arch = "wasm32")]
pub struct JsContextProviderProxy {
    collect: Rc<RefCell<js_sys::Function>>,
}

#[cfg(target_arch = "wasm32")]
impl JsContextProviderProxy {
    /// Construct a local context-provider proxy around a JS promise factory.
    #[must_use]
    pub fn new(collect: js_sys::Function) -> Self {
        Self {
            collect: Rc::new(RefCell::new(collect)),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl ContextProvider for JsContextProviderProxy {
    fn descriptor(&self) -> ContextProviderDescriptor {
        ContextProviderDescriptor {
            invocation: invocation(finstack_ai_kernel::static_key!(
                ComponentId,
                "wasm.js.context"
            )),
            trusted_application_instructions: false,
            metadata: Metadata::empty(),
        }
    }

    fn collect(
        &self,
        _ctx: ContextCallContext,
        _request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        let function = self.collect.borrow().clone();
        Box::pin(async move {
            let _ = await_promise(function).await;
            ContextContribution::try_new(Vec::new(), None::<&str>)
        })
    }
}

/// Local JS promise proxy for the middleware port.
#[cfg(target_arch = "wasm32")]
pub struct JsMiddlewareProxy {
    invoke: Rc<RefCell<js_sys::Function>>,
}

#[cfg(target_arch = "wasm32")]
impl JsMiddlewareProxy {
    /// Construct a local middleware proxy around a JS promise factory.
    #[must_use]
    pub fn new(invoke: js_sys::Function) -> Self {
        Self {
            invoke: Rc::new(RefCell::new(invoke)),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Middleware for JsMiddlewareProxy {
    fn descriptor(&self) -> MiddlewareDescriptor {
        MiddlewareDescriptor {
            invocation: invocation(finstack_ai_kernel::static_key!(
                ComponentId,
                "wasm.js.middleware"
            )),
            stages: StageMask::from_stages([Stage::BeforeRun]),
            order: MiddlewareOrder {
                tier: OrderTier::Standard,
                priority: 0,
                before: Arc::from([]),
                after: Arc::from([]),
            },
            role: MiddlewareRole::Standard,
            metadata: Metadata::empty(),
        }
    }

    fn invoke(
        &self,
        _ctx: MiddlewareContext,
        _input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let function = self.invoke.borrow().clone();
        Box::pin(async move {
            let _ = await_promise(function).await;
            Ok(StageOutcome::Continue)
        })
    }
}

/// Local JS promise proxy for the journal-store port.
#[cfg(target_arch = "wasm32")]
pub struct JsJournalStoreProxy {
    health: Rc<RefCell<js_sys::Function>>,
}

#[cfg(target_arch = "wasm32")]
impl JsJournalStoreProxy {
    /// Construct a local journal-store proxy around a JS promise factory.
    #[must_use]
    pub fn new(health: js_sys::Function) -> Self {
        Self {
            health: Rc::new(RefCell::new(health)),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl JournalStore for JsJournalStoreProxy {
    fn descriptor(&self) -> JournalStoreDescriptor {
        JournalStoreDescriptor::unspecified()
    }

    fn append(&self, _request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "js_compile_fixture",
            })
        })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        Box::pin(async move { Ok(LoadedSession::empty(request.session_id)) })
    }

    fn write_snapshot(
        &self,
        _request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "js_compile_fixture",
            })
        })
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        let function = self.health.borrow().clone();
        Box::pin(async move {
            let _ = await_promise(function).await;
            Ok(StoreHealth {
                ready: true,
                durable: false,
                detail: Arc::from("js_compile_fixture"),
            })
        })
    }
}

/// Local JS promise proxy for the observer port.
#[cfg(target_arch = "wasm32")]
pub struct JsObserverProxy {
    observe: Rc<RefCell<js_sys::Function>>,
}

#[cfg(target_arch = "wasm32")]
impl JsObserverProxy {
    /// Construct a local observer proxy around a JS promise factory.
    #[must_use]
    pub fn new(observe: js_sys::Function) -> Self {
        Self {
            observe: Rc::new(RefCell::new(observe)),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Observer for JsObserverProxy {
    fn descriptor(&self) -> ObserverDescriptor {
        ObserverDescriptor {
            component: ComponentRef::new(
                finstack_ai_kernel::static_key!(ComponentId, "wasm.js.observer"),
                Some(Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                }),
            ),
            payload_mode: ObserverPayloadMode::MetadataOnly,
            metadata: Metadata::empty(),
        }
    }

    fn observe(&self, _batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        let function = self.observe.borrow().clone();
        Box::pin(async move {
            let _ = await_promise(function).await;
            Ok(())
        })
    }
}

/// Construct the six local JS promise/stream port fixtures.
///
/// The handles store `Rc<RefCell<_>>` factories, so they are `!Send`.
#[cfg(target_arch = "wasm32")]
pub fn compile_js_port_proxies() {
    let ready = promise_factory("return Promise.resolve(undefined);");
    if let Ok(model) = JsModelProxy::new(ready.clone()) {
        let _: Arc<dyn Model> = Arc::new(model);
    }
    let _: Arc<dyn Toolset> = Arc::new(JsToolsetProxy::new(ready.clone()));
    let _: Arc<dyn ContextProvider> = Arc::new(JsContextProviderProxy::new(ready.clone()));
    let _: Arc<dyn Middleware> = Arc::new(JsMiddlewareProxy::new(ready.clone()));
    let _: Arc<dyn JournalStore> = Arc::new(JsJournalStoreProxy::new(ready.clone()));
    let _: Arc<dyn Observer> = Arc::new(JsObserverProxy::new(ready));
    let _ = crate::executor::drive(async { Ok(wasm_bindgen::JsValue::UNDEFINED) });
    crate::executor::spawn_port_future(Box::pin(async {}));
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{
        NativeContextProviderProxy, NativeJournalStoreProxy, NativeMiddlewareProxy,
        NativeModelProxy, NativeObserverProxy, NativeToolsetProxy, compile_native_port_proxies,
    };
    use crate::executor::block_on_ready;
    use finstack_ai::runtime::ports::journal::{JournalStore, LoadRequest};
    use finstack_ai_kernel::SessionId;

    #[test]
    fn native_port_proxies_are_send_sync() {
        compile_native_port_proxies();
    }

    #[test]
    fn native_store_health_is_ready() {
        let store = NativeJournalStoreProxy::default();
        let health = block_on_ready(store.health()).expect("health");
        assert!(health.ready);
        let session = SessionId::parse("00000000-0000-7000-8000-000000000001").expect("session");
        let loaded = block_on_ready(store.load(LoadRequest {
            session_id: session,
        }))
        .expect("load");
        assert_eq!(loaded.head_sequence, 0);
        let _ = NativeModelProxy::new().expect("model");
        let _ = NativeToolsetProxy;
        let _ = NativeContextProviderProxy;
        let _ = NativeMiddlewareProxy;
        let _ = NativeObserverProxy;
    }
}
