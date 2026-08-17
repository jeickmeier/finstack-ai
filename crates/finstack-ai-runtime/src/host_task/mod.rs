//! Sequential host-driven run owner for `wasm-host` builds.
//!
//! One local task owns commit intake, inline model/tool dispatch, and the
//! local event hub. This is not a Tokio `JoinSet` port.

mod dispatcher;
mod fault;
mod handle;
mod oneshot;
mod owner;
mod shared;
mod worker;

#[cfg(test)]
mod tests;

pub use handle::RunHandle;
pub use owner::RunTaskOwner;
