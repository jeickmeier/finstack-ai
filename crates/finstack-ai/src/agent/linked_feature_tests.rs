//! Constructor coverage executed with one provider feature at a time.
use super::*;

#[tokio::test]
async fn individual_provider_constructs_without_aggregate_feature() {
    let specs = [
        #[cfg(feature = "provider-openai")]
        LinkedProviderSpec::OpenAi(OpenAiAgentSpec {
            model: "fixture".into(),
            api_key: "fixture-secret".into(),
            reasoning_effort: None,
            reasoning_summary: None,
            common: LinkedCommon::default(),
        }),
        #[cfg(feature = "provider-openrouter")]
        LinkedProviderSpec::OpenRouter(OpenRouterAgentSpec {
            model: "fixture/model".into(),
            api_key: "fixture-secret".into(),
            reasoning_effort: None,
            reasoning_summary: None,
            referer: None,
            title: None,
            common: LinkedCommon::default(),
        }),
        #[cfg(feature = "provider-anthropic")]
        LinkedProviderSpec::Anthropic(AnthropicAgentSpec {
            model: "fixture".into(),
            base_url: "http://127.0.0.1:1".into(),
            api_key: None,
            common: LinkedCommon::default(),
        }),
        #[cfg(feature = "provider-gemini")]
        LinkedProviderSpec::Gemini(GeminiAgentSpec {
            model: "fixture".into(),
            endpoint: "http://127.0.0.1:1".into(),
            api_key: None,
            common: LinkedCommon::default(),
        }),
        #[cfg(feature = "provider-ollama")]
        LinkedProviderSpec::Ollama(OllamaAgentSpec {
            model: "fixture".into(),
            base_url: "http://127.0.0.1:1".into(),
            common: LinkedCommon::default(),
        }),
    ];
    for spec in specs {
        let linked = Agent::linked(spec)
            .await
            .expect("individual provider construction");
        assert!(linked.agent.resolved().lock().is_some());
    }
}
