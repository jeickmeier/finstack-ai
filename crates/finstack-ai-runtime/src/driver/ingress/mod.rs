//! Authenticated external-command routing by direct durable locator.

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
use crate::driver::host_driver::{
    InstalledClock as IngressClock, InstalledRandom as IngressRandom,
};
use crate::ids::UuidV7Generator;
#[cfg(feature = "native-tokio")]
use crate::ids::{OsRandomSource as IngressRandom, SystemClock as IngressClock};

fn ingress_ids() -> UuidV7Generator<IngressClock, IngressRandom> {
    UuidV7Generator::new(IngressClock, IngressRandom)
}

mod completion;
mod interaction;
mod shared;
mod types;

#[cfg(all(test, feature = "native-tokio"))]
mod tests;

pub use completion::ExternalCompletionRouter;
pub use interaction::InteractionRouter;
pub use types::{ExternalRouteError, ExternalRouteOutcome};
