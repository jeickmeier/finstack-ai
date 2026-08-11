//! Minimal offline model-only run.

use finstack_ai_native_examples::{BoxError, run_model_only};

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    println!("{}", Box::pin(run_model_only("minimal ready")).await?);
    Ok(())
}
