//! Resolve-once service-shaped request handling example.

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

use finstack_ai::AgentRunRequest;
use finstack_ai_native_examples::{BoxError, build_agent, security};

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let (agent, model, server) = build_agent(
        vec!["data: {\"id\":\"service\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"service ready\"},\"finish_reason\":\"stop\"}],\"usage\":null}\n\ndata: [DONE]\n\n".to_owned()],
        false,
    )
    .await?;
    let health = agent.resolved().health().await;
    println!("resolved components with lifecycle hooks: {}", health.len());
    let output = agent
        .run(AgentRunRequest::try_new(
            model,
            "Handle this request.",
            security()?,
        )?)
        .await?;
    server.await.map_err(|error| error.to_string())??;
    println!("{}", output.text());
    Ok(())
}
