//! Leaf-crate compile proof for the public target-correct `Model` ABI.

use core::pin::Pin;
use core::task::{Context, Poll};
use std::sync::Arc;

use finstack_ai_kernel::{ProviderIds, Usage};
use finstack_ai_runtime::{
    InputCapabilities, Model, ModelCapabilities, ModelContextProfile, ModelDescriptor, ModelError,
    ModelEventStream, ModelName, ModelRequest, ModelResponse, ModelStreamItem, ModelTokenEstimate,
    PortFuture, StructuredOutputCapability, TokenEstimatorRef, TokenEstimatorSource,
};
use futures_core::Stream;

/// Minimal provider leaf that uses no kernel edits and no runtime driver feature.
pub struct LeafModel {
    profile: ModelContextProfile,
}

impl LeafModel {
    /// Construct the compile fixture.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] if the fixture's model name violates the public
    /// runtime contract.
    pub fn new() -> Result<Self, ModelError> {
        Ok(Self {
            profile: ModelContextProfile {
                provider: Arc::from("leaf"),
                model: ModelName::try_new("leaf-model")?,
                hard_input_bytes: 1_024,
                context_window_tokens: 1_024,
                max_output_tokens: 128,
                reserved_output_tokens: 128,
                provider_overhead_tokens: 0,
                estimator: TokenEstimatorRef {
                    id: Arc::from("leaf-bytes"),
                    version: Arc::from("1"),
                    source: TokenEstimatorSource::ConservativeUpperBound,
                },
            },
        })
    }

    /// Deterministic normalized completion returned by this sample provider.
    #[must_use]
    pub fn response() -> ModelResponse {
        ModelResponse {
            assistant_content: Arc::from([]),
            tool_calls: Arc::from([]),
            usage: Usage::empty(),
            provider_ids: ProviderIds::empty(),
            completion_id: Arc::from("leaf-completion"),
            continuation_state: None,
        }
    }
}

impl Model for LeafModel {
    fn descriptor(&self) -> ModelDescriptor {
        ModelDescriptor {
            provider: Arc::clone(&self.profile.provider),
            models: Arc::from([self.profile.model.clone()]),
            metadata: finstack_ai_kernel::Metadata::empty(),
        }
    }

    fn capabilities(&self, _model: &ModelName) -> ModelCapabilities {
        ModelCapabilities {
            input: InputCapabilities {
                text: true,
                json: false,
                images: false,
                audio: false,
                files: false,
            },
            context_profile: self.profile.clone(),
            native_tool_calls: false,
            parallel_tool_calls: false,
            structured_output: StructuredOutputCapability::Unsupported,
            reasoning: false,
            prompt_cache: false,
            resumable_stream: false,
            idempotent_requests: false,
            native_capabilities: std::collections::BTreeSet::default(),
        }
    }

    fn estimate_input_tokens(
        &self,
        _model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        Ok(ModelTokenEstimate {
            input_tokens: u64::try_from(canonical_request.len()).unwrap_or(u64::MAX),
            estimator: self.profile.estimator.clone(),
        })
    }

    fn request(&self, _request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        Box::pin(async {
            Ok(Box::pin(SingleItemStream {
                item: Some(Ok(ModelStreamItem::Completed(Self::response()))),
            }) as ModelEventStream)
        })
    }
}

struct SingleItemStream {
    item: Option<Result<ModelStreamItem, ModelError>>,
}

impl Stream for SingleItemStream {
    type Item = Result<ModelStreamItem, ModelError>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.item.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::{
        EffectId, LaneId, Metadata, ModelRequestId, OperationLocator, OutputSpec, PrincipalRef,
        RawJson, RunId, SessionId,
    };
    use finstack_ai_runtime::{
        AuthorizationContext, CancellationSignal, ModelCallContext, ModelRequestDraft,
        ModelRequestLimits, ModelSettings, ModelStreamLimits, ModelTerminal, RunCallContext,
    };
    use finstack_ai_test::{ModelConformanceCase, check_model_conformance};

    fn session_id(value: u64) -> SessionId {
        SessionId::parse(&format!("00000000-0000-7000-8000-{value:012x}")).expect("session")
    }

    fn lane_id(value: u64) -> LaneId {
        LaneId::parse(&format!("00000000-0000-7000-8000-{value:012x}")).expect("lane")
    }

    fn run_id(value: u64) -> RunId {
        RunId::parse(&format!("00000000-0000-7000-8000-{value:012x}")).expect("run")
    }

    #[tokio::test]
    async fn public_leaf_provider_passes_test_kit_conformance() {
        let model = LeafModel::new().expect("leaf model");
        let selected = model.descriptor().models[0].clone();
        let principal =
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
        let request = ModelRequest {
            call: ModelCallContext {
                run: RunCallContext {
                    locator: OperationLocator::try_new(
                        "tenant-a",
                        session_id(1),
                        lane_id(2),
                        run_id(3),
                    )
                    .expect("locator"),
                    authorization: AuthorizationContext {
                        principal,
                        authentication_method: Arc::from("test"),
                        assurance_level: Arc::from("test"),
                        roles: Arc::from([]),
                        permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                        safe_claims: Metadata::empty(),
                        policy_version: Arc::from("policy-v1"),
                        decision_id: Arc::from("decision-v1"),
                    },
                    effect_id: EffectId::parse("00000000-0000-7000-8000-000000000004")
                        .expect("effect"),
                    attempt: 1,
                    deadline: None,
                    budget_scope_id: None,
                    cancellation: CancellationSignal::new(),
                },
                request_id: ModelRequestId::parse("00000000-0000-7000-8000-000000000005")
                    .expect("request"),
            },
            draft: ModelRequestDraft {
                model: selected.clone(),
                messages: Arc::from([]),
                tools: Arc::from([]),
                output: OutputSpec::PlainText,
                settings: ModelSettings {
                    values: RawJson::parse(b"{}").expect("settings"),
                },
                limits: ModelRequestLimits {
                    max_input_bytes: 1_024,
                    max_input_tokens: 1_024,
                    max_output_tokens: 128,
                },
            },
            continuation_state: None,
        };
        let assembled = check_model_conformance(
            &model,
            ModelConformanceCase {
                model: selected,
                request,
                expected_terminal: ModelTerminal::Completed(LeafModel::response()),
                stream_limits: ModelStreamLimits::default(),
            },
        )
        .await
        .expect("public Model contract");
        assert_eq!(
            assembled.terminal,
            ModelTerminal::Completed(LeafModel::response())
        );
    }
}
