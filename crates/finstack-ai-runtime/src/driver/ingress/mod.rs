//! Authenticated external-command routing by direct durable locator.

mod completion;
mod interaction;
mod shared;
mod types;

#[cfg(test)]
mod tests;

pub use completion::ExternalCompletionRouter;
pub use interaction::InteractionRouter;
pub use types::{ExternalRouteError, ExternalRouteOutcome};
