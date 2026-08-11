//! Credential-free specification and resolution-lock diagnostics.

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
