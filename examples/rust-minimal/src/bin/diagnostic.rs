//! Credential-free specification and resolution-lock diagnostics.

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

use finstack_ai_native_examples::{BoxError, build_agent};

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let (agent, _model, server) = build_agent(Vec::new(), true).await?;
    let spec = agent.resolved().spec().ok_or("missing AgentSpec")?;
    let lock = agent.resolved().lock().ok_or("missing resolved lock")?;
    println!("agent_spec={}", String::from_utf8(spec.canonical_json()?)?);
    println!("lock_fingerprint={}", lock.fingerprint()?);
    println!("lock={}", String::from_utf8(lock.to_json()?)?);
    server.await.map_err(|error| error.to_string())??;
    Ok(())
}
