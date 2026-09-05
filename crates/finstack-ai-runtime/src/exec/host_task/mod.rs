//! Host-driven run owner for `wasm-host` builds.
//!
//! One local task owns commit intake and deterministic settlement. Parallel
//! tool groups use bounded owned host-driver child tasks; sequential and
//! barrier groups remain exclusive.

mod dispatcher;
mod fault;
mod handle;
pub(crate) mod oneshot;
mod owner;
mod shared;
mod worker;

#[cfg(test)]
mod tests;

pub use handle::RunHandle;
pub use owner::RunTaskOwner;
