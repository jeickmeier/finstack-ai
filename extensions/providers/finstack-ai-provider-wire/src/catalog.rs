//! Model-catalog helpers identical across the first-party provider leaves.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::Metadata;
use finstack_ai_runtime::ports::model::{
    InputCapabilities, ModelCapabilities, ModelContextProfile, ModelDescriptor, ModelError,
    ModelName, StructuredOutputCapability, TokenEstimatorRef,
};

use crate::adapter_error::{ProviderErrorCodes, config_error};

/// Validate a local model table and index it by name.
///
/// `name` reads each entry's [`ModelName`]; `validate` re-checks an entry's
/// public profile before it enters the catalog (pass a no-op for leaves whose
/// builders already reject every invalid value). The resulting
/// [`ModelDescriptor`] for `provider` is validated as well.
///
/// # Errors
///
/// Returns the entry's own validation failure, or the leaf's
/// `config_invalid` error for an empty or duplicate catalog.
pub fn catalog_from_models<M>(
    codes: &ProviderErrorCodes,
    provider: &'static str,
    models: Vec<M>,
    name: impl Fn(&M) -> &ModelName,
    validate: impl Fn(&M) -> Result<(), ModelError>,
) -> Result<BTreeMap<ModelName, M>, ModelError> {
    let mut by_name = BTreeMap::new();
    for model in models {
        validate(&model)?;
        if by_name.insert(name(&model).clone(), model).is_some() {
            return Err(config_error(
                codes,
                "provider contains a duplicate model name",
            ));
        }
    }
    if by_name.is_empty() {
        return Err(config_error(codes, "provider requires at least one model"));
    }
    let descriptor = ModelDescriptor {
        provider: Arc::from(provider),
        models: by_name.keys().cloned().collect::<Vec<_>>().into(),
        metadata: Metadata::empty(),
    };
    descriptor.validate()?;
    Ok(by_name)
}

/// Capabilities advertised for a model the provider has not configured.
///
/// Every input kind, flag, and context-profile counter is zero or `false`;
/// only the provider name, the requested model, the leaf's structured-output
/// mode, and its token estimator are filled in.
#[must_use]
pub fn unknown_model_capabilities(
    provider: &'static str,
    model: &ModelName,
    structured_output: StructuredOutputCapability,
    estimator: TokenEstimatorRef,
) -> ModelCapabilities {
    ModelCapabilities {
        input: InputCapabilities {
            text: false,
            json: false,
            images: false,
            audio: false,
            files: false,
        },
        context_profile: ModelContextProfile {
            provider: Arc::from(provider),
            model: model.clone(),
            hard_input_bytes: 0,
            context_window_tokens: 0,
            max_output_tokens: 0,
            reserved_output_tokens: 0,
            provider_overhead_tokens: 0,
            estimator,
        },
        native_tool_calls: false,
        parallel_tool_calls: false,
        structured_output,
        reasoning: false,
        prompt_cache: false,
        resumable_stream: false,
        idempotent_requests: false,
        native_capabilities: BTreeSet::new(),
    }
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::ErrorCategory;
    use finstack_ai_runtime::ports::model::TokenEstimatorSource;

    use super::*;

    const CODES: ProviderErrorCodes = ProviderErrorCodes {
        config_invalid: "test_config_invalid",
        request_invalid: "test_request_invalid",
        http_error: "test_http_error",
        transport_error: "test_transport_error",
        timeout: "test_timeout",
        cancelled: "test_cancelled",
        stream_invalid: "test_stream_invalid",
        stream_limit_exceeded: "test_stream_limit_exceeded",
        response_invalid: "test_response_invalid",
    };

    #[derive(Debug)]
    struct Entry(ModelName, bool);

    fn catalog(entries: Vec<Entry>) -> Result<BTreeMap<ModelName, Entry>, ModelError> {
        catalog_from_models(
            &CODES,
            "test",
            entries,
            |entry| &entry.0,
            |entry| {
                if entry.1 {
                    Ok(())
                } else {
                    Err(config_error(&CODES, "entry invalid"))
                }
            },
        )
    }

    #[test]
    fn rejects_empty_duplicate_and_invalid_entries() {
        let name = ModelName::try_new("m").unwrap();
        assert_eq!(
            catalog(Vec::new()).unwrap_err().message(),
            "provider requires at least one model"
        );
        let error =
            catalog(vec![Entry(name.clone(), true), Entry(name.clone(), true)]).unwrap_err();
        assert_eq!(error.code().as_str(), "test_config_invalid");
        assert_eq!(error.category(), ErrorCategory::Configuration);
        assert_eq!(error.message(), "provider contains a duplicate model name");
        assert_eq!(
            catalog(vec![Entry(name.clone(), false)])
                .unwrap_err()
                .message(),
            "entry invalid"
        );
        let by_name = catalog(vec![Entry(name.clone(), true)]).unwrap();
        assert!(by_name.contains_key(&name));
    }

    #[test]
    fn unknown_model_profile_is_zeroed() {
        let model = ModelName::try_new("unknown").unwrap();
        let capabilities = unknown_model_capabilities(
            "test",
            &model,
            StructuredOutputCapability::Native,
            TokenEstimatorRef {
                id: Arc::from("test.estimator"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        );
        assert_eq!(capabilities.context_profile.model, model);
        assert_eq!(capabilities.context_profile.provider.as_ref(), "test");
        assert_eq!(capabilities.context_profile.context_window_tokens, 0);
        assert_eq!(
            capabilities.structured_output,
            StructuredOutputCapability::Native
        );
        assert!(!capabilities.input.text);
        assert!(capabilities.native_capabilities.is_empty());
    }
}
