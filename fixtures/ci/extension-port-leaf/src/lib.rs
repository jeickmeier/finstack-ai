//! Compile-only proof that leaf extensions implement PR-018 ports through public runtime APIs.

#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, Digest, InvocationRecovery, Metadata, RunEvent, Stage,
    Version,
};
use finstack_ai_runtime::{
    ContextCallContext, ContextContribution, ContextError, ContextProvider,
    ContextProviderDescriptor, ContextRequest, Middleware, MiddlewareContext, MiddlewareDescriptor,
    MiddlewareError, MiddlewareOrder, MiddlewareRole, Observer, ObserverDescriptor, ObserverError,
    ObserverPayloadMode, OrderTier, PortFuture, StageInput, StageMask, StageOutcome,
};

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

/// Compile-only context provider.
pub struct LeafContextProvider;

impl ContextProvider for LeafContextProvider {
    fn descriptor(&self) -> ContextProviderDescriptor {
        ContextProviderDescriptor {
            invocation: invocation(finstack_ai_kernel::static_key!(
                ComponentId,
                "fixture.context"
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

/// Compile-only middleware.
pub struct LeafMiddleware;

impl Middleware for LeafMiddleware {
    fn descriptor(&self) -> MiddlewareDescriptor {
        MiddlewareDescriptor {
            invocation: invocation(finstack_ai_kernel::static_key!(
                ComponentId,
                "fixture.middleware"
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

/// Compile-only immutable observer.
pub struct LeafObserver;

impl Observer for LeafObserver {
    fn descriptor(&self) -> ObserverDescriptor {
        ObserverDescriptor {
            component: finstack_ai_kernel::ComponentRef::new(
                finstack_ai_kernel::static_key!(ComponentId, "fixture.observer"),
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
