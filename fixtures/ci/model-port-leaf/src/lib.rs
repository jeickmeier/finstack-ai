//! Leaf-crate compile proof for the public target-correct `Model` ABI.

use core::pin::Pin;
use core::task::{Context, Poll};
use std::sync::Arc;

use finstack_ai_runtime::{
    InputCapabilities, Model, ModelCapabilities, ModelContextProfile, ModelDescriptor, ModelError,
    ModelEventStream, ModelName, ModelRequest, ModelStreamItem, ModelTokenEstimate, PortFuture,
    StructuredOutputCapability, TokenEstimatorRef, TokenEstimatorSource,
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
}

impl Model for LeafModel {
    fn descriptor(&self) -> ModelDescriptor {
        ModelDescriptor {
            provider: Arc::clone(&self.profile.provider),
            models: Arc::from([self.profile.model.clone()]),
            metadata: finstack_ai_runtime::Metadata::empty(),
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
        Box::pin(async { Ok(Box::pin(EmptyStream) as ModelEventStream) })
    }
}

struct EmptyStream;

impl Stream for EmptyStream {
    type Item = Result<ModelStreamItem, ModelError>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}
