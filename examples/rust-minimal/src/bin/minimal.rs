//! Minimal offline model-only run.

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

use finstack_ai_native_examples::{BoxError, run_model_only};

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    println!("{}", Box::pin(run_model_only("minimal ready")).await?);
    Ok(())
}
