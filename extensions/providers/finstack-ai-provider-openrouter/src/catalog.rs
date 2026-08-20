//! `OpenRouter` model-catalog (`GET /api/v1/models`) response mapping.
//!
//! Parsing is pure and deterministic; fetching lives on
//! [`crate::OpenRouterProvider::fetch_model_catalog`]. Applying a fetched
//! catalog stays an explicit host decision via `replace_model_catalog`.

use finstack_ai_runtime::ModelError;
use serde::Deserialize;

use crate::OpenRouterModelConfig;
use crate::error::response_error;

const PROVIDER_OVERHEAD_TOKENS: u64 = 64;

#[derive(Deserialize)]
struct CatalogResponse {
    data: Vec<CatalogModel>,
}

#[derive(Deserialize)]
struct CatalogModel {
    id: String,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    top_provider: Option<TopProvider>,
    #[serde(default)]
    supported_parameters: Vec<String>,
    #[serde(default)]
    architecture: Option<Architecture>,
}

#[derive(Deserialize)]
struct TopProvider {
    #[serde(default)]
    max_completion_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct Architecture {
    #[serde(default)]
    input_modalities: Vec<String>,
}

/// Map one `GET /api/v1/models` response body onto conservative model configs.
///
/// Models without a usable `context_length` are skipped. `max_output_tokens`
/// prefers `top_provider.max_completion_tokens`, falling back to one quarter
/// of the context window, and is always clamped inside the window.
/// Image and file input follow `architecture.input_modalities`. Audio does
/// not: hosts opt in via [`OpenRouterModelConfig::with_input_audio`].
///
/// # Errors
///
/// Returns `openrouter_response_invalid` when the body is not the catalog
/// shape or no model survives the filters.
pub fn model_configs_from_catalog_json(
    bytes: &[u8],
    hard_input_bytes: u64,
) -> Result<Vec<OpenRouterModelConfig>, ModelError> {
    let catalog: CatalogResponse = serde_json::from_slice(bytes)
        .map_err(|_| response_error("OpenRouter model catalog could not be decoded"))?;
    let mut configs = Vec::with_capacity(catalog.data.len());
    for model in catalog.data {
        let Some(context_window_tokens) = model
            .context_length
            .filter(|window| *window > PROVIDER_OVERHEAD_TOKENS.saturating_add(1))
        else {
            continue;
        };
        let max_output_tokens = model
            .top_provider
            .as_ref()
            .and_then(|top| top.max_completion_tokens)
            .unwrap_or(context_window_tokens / 4)
            .clamp(1, context_window_tokens);
        let reserved_output_tokens =
            max_output_tokens.min(context_window_tokens - PROVIDER_OVERHEAD_TOKENS);
        let supports = |name: &str| model.supported_parameters.iter().any(|p| p == name);
        let modality = |name: &str| {
            model
                .architecture
                .as_ref()
                .is_some_and(|arch| arch.input_modalities.iter().any(|m| m == name))
        };
        let Ok(config) = OpenRouterModelConfig::try_new(
            &model.id,
            hard_input_bytes,
            context_window_tokens,
            max_output_tokens,
            reserved_output_tokens,
            PROVIDER_OVERHEAD_TOKENS,
        ) else {
            // A malformed model id (or other per-entry construction failure)
            // only invalidates that one catalog entry; skip it rather than
            // aborting the whole parse. The empty-result check below still
            // fails closed if every entry turns out unusable.
            continue;
        };
        // Images and files follow catalog architecture. Audio stays off:
        // OpenRouter's Responses `input_audio` support is unverified, so hosts
        // opt in explicitly via [`OpenRouterModelConfig::with_input_audio`].
        let config = config
            .with_parallel_tool_calls(supports("tools"))
            .with_reasoning(supports("reasoning") || supports("include_reasoning"))
            .with_input_images(modality("image"))
            .with_input_files(modality("file"));
        configs.push(config);
    }
    if configs.is_empty() {
        return Err(response_error(
            "OpenRouter model catalog contained no usable models",
        ));
    }
    Ok(configs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &[u8] = br#"{
      "data": [
        {
          "id": "openai/gpt-5",
          "name": "OpenAI: GPT-5",
          "context_length": 400000,
          "pricing": {"prompt": "0.00000125", "completion": "0.00001"},
          "top_provider": {"max_completion_tokens": 128000},
          "supported_parameters": ["tools", "reasoning", "structured_outputs"],
          "architecture": {"input_modalities": ["text", "image"]}
        },
        {
          "id": "tiny/no-window",
          "context_length": 10
        },
        {
          "id": "mistral/basic",
          "context_length": 32768,
          "supported_parameters": ["temperature"]
        },
        {
          "id": "",
          "context_length": 32768
        }
      ]
    }"#;

    #[test]
    fn maps_catalog_entries_onto_conservative_configs() {
        let configs = model_configs_from_catalog_json(CATALOG, 1_000_000).expect("configs");
        assert_eq!(
            configs.len(),
            2,
            "the malformed (empty-id) entry must be skipped, not abort the parse"
        );
        assert_eq!(configs[0].name().as_str(), "openai/gpt-5");
        assert_eq!(configs[0].context_window_tokens(), 400_000);
        assert_eq!(configs[0].max_output_tokens(), 128_000);
        assert!(configs[0].parallel_tool_calls());
        assert!(configs[0].reasoning());
        assert!(configs[0].input_images() && !configs[0].input_audio());
        assert_eq!(configs[1].name().as_str(), "mistral/basic");
        assert_eq!(configs[1].max_output_tokens(), 32_768 / 4);
        assert!(!configs[1].parallel_tool_calls());
        assert!(!configs[1].reasoning());
        for config in &configs {
            assert!(
                config.reserved_output_tokens() + config.provider_overhead_tokens()
                    <= config.context_window_tokens()
            );
        }
    }

    #[test]
    fn catalog_does_not_enable_audio_from_architecture() {
        let body = br#"{
          "data": [{
            "id": "google/gemini-audio",
            "context_length": 32768,
            "architecture": {"input_modalities": ["text", "image", "audio", "file"]}
          }]
        }"#;
        let configs = model_configs_from_catalog_json(body, 1_000_000).expect("configs");
        assert_eq!(configs.len(), 1);
        assert!(configs[0].input_images());
        assert!(configs[0].input_files());
        assert!(
            !configs[0].input_audio(),
            "catalog audio must stay off; hosts opt in via with_input_audio"
        );
    }

    #[test]
    fn invalid_or_empty_catalogs_fail_closed() {
        for body in [
            b"not json".as_slice(),
            br#"{"data": []}"#.as_slice(),
            br#"{"data": [{"id": "tiny/no-window", "context_length": 10}]}"#.as_slice(),
        ] {
            assert_eq!(
                model_configs_from_catalog_json(body, 1_000_000)
                    .expect_err("bad catalog")
                    .code(),
                crate::error::RESPONSE_INVALID
            );
        }
    }
}
