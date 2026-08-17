//! Shared offline support for the native example binaries.

use std::error::Error;
use std::sync::Arc;

use finstack_ai::runtime::{
    AgentId, BundleId, ComponentId, ComponentRef, JournalStore, Model, ModelName, Toolset, Version,
};
use finstack_ai::{Agent, AgentRunRequest, PrincipalRef, RunSecurityContext};
use finstack_ai_provider_ollama::{OllamaConfig, OllamaModelConfig, OllamaProvider};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_tools_calculator::CalculatorToolset;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Boxed error used by the small standalone binaries.
pub type BoxError = Box<dyn Error + Send + Sync>;

/// Exact developer-preview component version.
pub const PREVIEW_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

/// Build an offline native Ollama agent and its loopback server task.
///
/// # Errors
///
/// Returns provider, component, composition, store, or listener setup errors.
pub async fn build_agent(
    responses: Vec<String>,
    calculator: bool,
) -> Result<
    (
        Agent,
        ModelName,
        tokio::task::JoinHandle<Result<(), String>>,
    ),
    BoxError,
> {
    let (base_url, server) = serve_ndjson(responses).await?;
    let model_name = ModelName::try_new("preview-model")?;
    let provider: Arc<dyn Model> = Arc::new(OllamaProvider::try_new(
        OllamaConfig::try_new(base_url)?,
        vec![OllamaModelConfig::try_new(
            model_name.as_ref(),
            1_048_576,
            1_048_576,
            512,
            512,
            64,
        )?],
    )?);
    let store: Arc<dyn JournalStore> = Arc::new(MemoryJournalStore::try_new(MemoryStoreLimits {
        sessions: 8,
        batches_per_session: 128,
        records_per_session: 1_024,
        snapshot_bytes: 16_384,
    })?);
    let mut builder = Agent::builder(
        AgentId::parse("preview.agent.native")?,
        BundleId::parse("preview.bundle.native")?,
        (component("preview.model.ollama")?, provider),
        (component("preview.store.memory")?, store),
    )
    .try_instruction("Answer directly and use registered tools when helpful.")?;
    if calculator {
        let toolset: Arc<dyn Toolset> = Arc::new(CalculatorToolset::try_new()?);
        builder = builder.toolset(component("preview.tools.calculator")?, toolset);
    }
    Ok((builder.build().await?, model_name, server))
}

/// Construct the explicit local security projection used by every example.
///
/// # Errors
///
/// Returns an identity validation error if a frozen example label is invalid.
pub fn security() -> Result<RunSecurityContext, BoxError> {
    Ok(RunSecurityContext::try_new(
        "preview-local",
        PrincipalRef::try_new("local-example", "developer", Some("preview-local"))?,
        "local",
        "developer-preview",
        "preview-policy-v1",
        "preview-decision-v1",
        None,
    )?)
}

/// Run one model-only offline request.
///
/// # Errors
///
/// Returns composition, provider, runtime, or loopback transport errors.
pub async fn run_model_only(text: &str) -> Result<String, BoxError> {
    let (agent, model, server) =
        build_agent(vec![text_response(text, "model-only")], false).await?;
    let output = agent
        .run(AgentRunRequest::try_new(model, "Reply once.", security()?)?)
        .await?;
    server.await.map_err(|error| error.to_string())??;
    Ok(output.text())
}

/// Run one real calculator tool loop through two provider responses.
///
/// # Errors
///
/// Returns composition, provider, tool, runtime, or loopback transport errors.
pub async fn run_tool_loop() -> Result<String, BoxError> {
    let (agent, model, server) = build_agent(
        vec![calculator_response(), text_response("five", "tool-final")],
        true,
    )
    .await?;
    let output = agent
        .run(AgentRunRequest::try_new(
            model,
            "What is two plus three?",
            security()?,
        )?)
        .await?;
    server.await.map_err(|error| error.to_string())??;
    Ok(output.text())
}

fn component(id: &str) -> Result<ComponentRef, BoxError> {
    Ok(ComponentRef::new(
        ComponentId::parse(id)?,
        Some(PREVIEW_VERSION),
    ))
}

/// One NDJSON text completion used by the offline loopback server.
#[must_use]
pub fn text_response(text: &str, id: &str) -> String {
    let _ = id;
    format!(
        "{{\"message\":{{\"role\":\"assistant\",\"content\":\"{text}\"}},\"done\":false}}\n{{\"message\":{{\"role\":\"assistant\",\"content\":\"\"}},\"done\":true,\"prompt_eval_count\":1,\"eval_count\":1}}\n"
    )
}

/// One NDJSON calculator tool-call used by the offline loopback server.
#[must_use]
pub fn calculator_response() -> String {
    concat!(
        "{\"message\":{\"role\":\"assistant\",\"content\":\"\",\"tool_calls\":[{\"function\":{\"name\":\"calculator\",\"arguments\":{\"operation\":\"add\",\"operands\":[2,3]}}}]},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"prompt_eval_count\":1,\"eval_count\":1}\n",
    )
    .to_owned()
}

/// Serve a finite sequence of NDJSON bodies on a loopback listener.
///
/// # Errors
///
/// Returns a bind or address error before the accept loop starts.
pub async fn serve_ndjson(
    responses: Vec<String>,
) -> Result<(String, tokio::task::JoinHandle<Result<(), String>>), BoxError> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        for body in responses {
            let (mut socket, _) = listener.accept().await.map_err(|error| error.to_string())?;
            read_request(&mut socket).await?;
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket
                .write_all(headers.as_bytes())
                .await
                .map_err(|error| error.to_string())?;
            socket
                .write_all(body.as_bytes())
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    });
    Ok((format!("http://{address}"), task))
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Result<(), String> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let header_end = loop {
        let count = socket
            .read(&mut buffer)
            .await
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("request closed before headers".to_owned());
        }
        request.extend_from_slice(&buffer[..count]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .and_then(|value| value.parse::<usize>().ok())
        })
        .ok_or_else(|| "request omitted content-length".to_owned())?;
    while request.len() < header_end + content_length {
        let count = socket
            .read(&mut buffer)
            .await
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("request closed before body".to_owned());
        }
        request.extend_from_slice(&buffer[..count]);
    }
    Ok(())
}
