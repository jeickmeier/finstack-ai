//! `OpenRouter` Responses (`/api/v1/responses`) implementation of the public [`Model`](finstack_ai_runtime::Model) port.

#![warn(missing_docs)]

mod catalog;
mod config;
mod error;
mod provider;
mod request;
mod sse;
mod stream;

pub use catalog::model_configs_from_catalog_json;
pub use config::{OpenRouterConfig, OpenRouterModelConfig, SecretHeader};
pub use finstack_ai_runtime::{
    Authentication, CredentialReference, CredentialRejected, CredentialStore, SecretRejected,
    SecretString,
};
pub use provider::OpenRouterProvider;

/// Benchmark-only access to crate-private request/stream internals.
///
/// Gated behind the `bench` feature (dev/bench profile only, never default)
/// so `criterion` benches can exercise `ResponsesRequest::try_from_draft`,
/// SSE framing, and stream assembly without widening the crate's public API.
#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench_support {
    use finstack_ai_runtime::{ModelError, ModelRequestDraft, ModelStreamItem, ResolvedMedia};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// Benchmark-facing wrapper around the crate-private `ResponsesRequest`.
    pub struct ResponsesRequest(crate::request::ResponsesRequest);

    impl ResponsesRequest {
        /// Translate one canonical draft into the wire request shape.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the wrapped translation.
        pub fn try_from_draft(
            draft: &ModelRequestDraft,
            model: &crate::OpenRouterModelConfig,
            continuation_state: Option<&finstack_ai_kernel::RawJson>,
            resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
        ) -> Result<Self, ModelError> {
            crate::request::ResponsesRequest::try_from_draft(
                draft,
                model,
                continuation_state,
                resolved,
            )
            .map(Self)
        }
    }

    /// Serialize a translated request to its wire JSON bytes.
    ///
    /// # Errors
    ///
    /// Returns the same errors as the wrapped serializer.
    pub fn serialize_request(request: &ResponsesRequest) -> Result<Vec<u8>, ModelError> {
        crate::request::serialize_request(&request.0)
    }

    /// Benchmark-facing wrapper around the crate-private `CompletionAssembly`.
    pub struct CompletionAssembly(crate::stream::CompletionAssembly);

    impl CompletionAssembly {
        /// Construct a fresh assembly for one request/response cycle.
        #[must_use]
        pub fn new(request_id: String, structured: bool) -> Self {
            Self(crate::stream::CompletionAssembly::new(
                request_id, structured,
            ))
        }

        /// Consume one decoded SSE event payload, returning stream items.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the wrapped assembly.
        pub fn consume(&mut self, data: &str) -> Result<Vec<ModelStreamItem>, ModelError> {
            self.0.consume(data)
        }
    }

    /// Benchmark-facing wrapper around the crate-private `SseParser`.
    ///
    /// Surfaces parsed event payloads as owned `String`s (rather than the
    /// crate-private `SseEventData`) so a bench crate outside this crate's
    /// privacy boundary can drive the exact same parsing path.
    pub struct SseParser(crate::sse::SseParser);

    impl SseParser {
        /// Construct a parser with the given per-event/stream byte ceilings.
        #[must_use]
        pub fn new(max_event_bytes: usize, max_stream_bytes: usize) -> Self {
            Self(crate::sse::SseParser::new(
                max_event_bytes,
                max_stream_bytes,
            ))
        }

        /// Feed one chunk of bytes, returning newly completed event payloads.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the wrapped parser.
        pub fn push(
            &mut self,
            bytes: &[u8],
        ) -> Result<Vec<String>, finstack_ai_runtime::ModelError> {
            Ok(self
                .0
                .push(bytes)?
                .into_iter()
                .map(|event| event.data)
                .collect())
        }

        /// Finish the stream, asserting it did not end mid-event.
        ///
        /// # Errors
        ///
        /// Returns the same errors as the wrapped parser.
        pub fn finish(self) -> Result<(), finstack_ai_runtime::ModelError> {
            self.0.finish()
        }
    }
}
