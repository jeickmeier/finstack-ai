//! Isolated proof that `wasm-host`-only linked constructors fail closed.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]

use std::future::Future;
use std::task::{Context, Poll, Waker};

use finstack_ai::{
    AGENT_RUN_UNSUPPORTED_PLAN, Agent, AgentRunError, AnthropicAgentSpec, GatewayAgentSpec,
    GeminiAgentSpec, LinkedCommon, LinkedProviderSpec, OllamaAgentSpec, OpenAiAgentSpec,
    OpenRouterAgentSpec,
};

fn require_unsupported<T>(
    name: &str,
    future: impl Future<Output = Result<T, AgentRunError>>,
) -> Result<(), String> {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(Err(error)) if error.code() == AGENT_RUN_UNSUPPORTED_PLAN => Ok(()),
        Poll::Ready(Err(error)) => Err(format!(
            "{name} returned {}, expected {AGENT_RUN_UNSUPPORTED_PLAN}",
            error.code()
        )),
        Poll::Ready(Ok(_)) => Err(format!("{name} unexpectedly constructed on wasm-host")),
        Poll::Pending => Err(format!("{name} did not fail synchronously on wasm-host")),
    }
}

fn main() -> Result<(), String> {
    require_unsupported(
        "openai",
        Agent::linked(LinkedProviderSpec::OpenAi(OpenAiAgentSpec {
            model: "fixture-model".into(),
            api_key: "sk-unused".into(),
            reasoning_effort: None,
            reasoning_summary: None,
            common: LinkedCommon::default(),
        })),
    )?;
    require_unsupported(
        "openrouter",
        Agent::linked(LinkedProviderSpec::OpenRouter(OpenRouterAgentSpec {
            model: "fixture-model".into(),
            api_key: "sk-unused".into(),
            referer: None,
            title: None,
            reasoning_effort: None,
            reasoning_summary: None,
            common: LinkedCommon::default(),
        })),
    )?;
    require_unsupported(
        "anthropic",
        Agent::linked(LinkedProviderSpec::Anthropic(AnthropicAgentSpec {
            base_url: "https://api.anthropic.com".into(),
            model: "fixture-model".into(),
            api_key: None,
            common: LinkedCommon::default(),
        })),
    )?;
    require_unsupported(
        "gemini",
        Agent::linked(LinkedProviderSpec::Gemini(GeminiAgentSpec {
            endpoint: "https://generativelanguage.googleapis.com".into(),
            model: "fixture-model".into(),
            api_key: None,
            common: LinkedCommon::default(),
        })),
    )?;
    require_unsupported(
        "ollama",
        Agent::linked(LinkedProviderSpec::Ollama(OllamaAgentSpec {
            base_url: "http://127.0.0.1:11434".into(),
            model: "fixture-model".into(),
            common: LinkedCommon::default(),
        })),
    )?;
    require_unsupported(
        "gateway",
        Agent::linked(LinkedProviderSpec::Gateway(GatewayAgentSpec {
            endpoint: "https://api.example.test/v1/responses".into(),
            model: "fixture-model".into(),
            wire_protocol: "openai_responses".into(),
            credential_name: "prod".into(),
            hard_input_bytes: Some(1_000_000),
            auth_kind: Some("bearer".into()),
            api_key: Some("sk-unused".into()),
            common: LinkedCommon::default(),
        })),
    )
}
