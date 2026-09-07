//! Executable spec for the leased workflow worker.

#![allow(
    clippy::large_futures,
    reason = "contract tests keep driver setup inline for readable state-machine scenarios"
)]

mod execution;
mod expiry;
mod hardening;
mod helpers;
mod inbox_resume;
mod park;
mod tick;
