//! wasm-bindgen Agent / Run handles over Rust-owned state.

#[allow(
    clippy::module_inception,
    reason = "Agent handle stays at crate::agent::Agent after the directory split"
)]
mod agent;
mod build;
mod capabilities;
mod driver;
mod errors;
mod events;
mod request;
mod results;
mod run;
mod session;

pub use driver::install_host_driver;
