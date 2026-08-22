//! Host-supplied media-blob resolution for model leaves. This is not a
//! registered port: hosts hand resolvers to
//! provider configs, mirroring [`super::credentials::CredentialStore`].

use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::BlobRef;

use crate::ContentBlock;
use crate::ports::model::ModelRequestDraft;
use crate::ports::{PortFuture, PortObject};

/// Resolve one committed blob reference to provider-usable media.
pub trait MediaResolver: PortObject {
    /// Resolve `blob` to a URL or bounded bytes.
    fn resolve(&self, blob: &BlobRef) -> PortFuture<Result<ResolvedMedia, MediaResolveError>>;
}

/// One resolved media payload.
#[derive(Debug, Clone)]
pub enum ResolvedMedia {
    /// Provider-fetchable HTTPS URL.
    Url(Arc<str>),
    /// Inline bytes with their media type.
    Bytes {
        /// Media-type label such as `image/png`.
        media_type: Arc<str>,
        /// Raw payload bytes.
        bytes: Arc<[u8]>,
    },
}

/// Protocol-neutral resolution failure; provider leaves map it onto their
/// own stable error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaResolveError {
    /// Failure class.
    pub kind: MediaResolveKind,
    /// Stable non-secret message.
    pub message: &'static str,
}

/// Resolution failure class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaResolveKind {
    /// The blob id is unknown to the host.
    NotFound,
    /// The host store is temporarily unavailable.
    Unavailable,
    /// The payload exceeds a host-side limit.
    Limit,
}

/// Protocol-neutral failure while resolving every distinct media block in a
/// draft. Provider leaves map it onto their own stable error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveDraftMediaError {
    /// The draft contains media but no resolver was configured.
    MissingResolver,
    /// The host resolver failed for one blob.
    Resolve(MediaResolveError),
    /// A [`ResolvedMedia::Bytes`] payload or the draft aggregate exceeded
    /// `max_stream_bytes`.
    Limit,
}

/// Resolve every distinct media block in `draft`.
///
/// Image, audio, and file blocks share one map keyed by blob id. Each
/// [`ResolvedMedia::Bytes`] payload is bounded by `max_stream_bytes`, and so
/// is the running aggregate across the draft (ADR-049), so a caller cannot
/// smuggle an oversized request past per-blob checks by splitting it across
/// many blobs. URL payloads do not count toward the aggregate.
///
/// # Errors
///
/// Returns [`ResolveDraftMediaError::MissingResolver`] when the draft contains
/// media and `media_resolver` is `None`, [`ResolveDraftMediaError::Resolve`]
/// when the host resolver fails, and [`ResolveDraftMediaError::Limit`] when a
/// byte payload or the aggregate exceeds `max_stream_bytes`.
pub async fn resolve_draft_media(
    media_resolver: Option<&Arc<dyn MediaResolver>>,
    draft: &ModelRequestDraft,
    max_stream_bytes: usize,
) -> Result<BTreeMap<Arc<str>, ResolvedMedia>, ResolveDraftMediaError> {
    let mut resolved_media = BTreeMap::new();
    let mut aggregate_bytes: usize = 0;
    for message in draft.messages.iter() {
        for block in message.content() {
            let (ContentBlock::Image(media)
            | ContentBlock::Audio(media)
            | ContentBlock::File(media)) = block
            else {
                continue;
            };
            let id: Arc<str> = Arc::from(media.blob().id());
            if resolved_media.contains_key(&id) {
                continue;
            }
            let Some(media_resolver) = media_resolver else {
                return Err(ResolveDraftMediaError::MissingResolver);
            };
            let payload = media_resolver
                .resolve(media.blob())
                .await
                .map_err(ResolveDraftMediaError::Resolve)?;
            if let ResolvedMedia::Bytes { bytes, .. } = &payload {
                if bytes.len() > max_stream_bytes {
                    return Err(ResolveDraftMediaError::Limit);
                }
                aggregate_bytes = aggregate_bytes.saturating_add(bytes.len());
                if aggregate_bytes > max_stream_bytes {
                    return Err(ResolveDraftMediaError::Limit);
                }
            }
            resolved_media.insert(id, payload);
        }
    }
    Ok(resolved_media)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use finstack_ai_kernel::{
        MediaRef, Message, MessageId, MessageRole, Metadata, OutputSpec, ProviderIds, RawJson,
        Timestamp,
    };

    use crate::ports::model::{ModelName, ModelRequestLimits, ModelSettings};

    use super::*;

    #[derive(Debug)]
    struct FixtureResolver;

    impl MediaResolver for FixtureResolver {
        fn resolve(&self, blob: &BlobRef) -> PortFuture<Result<ResolvedMedia, MediaResolveError>> {
            let id = blob.id().to_owned();
            Box::pin(async move {
                if id == "missing" {
                    return Err(MediaResolveError {
                        kind: MediaResolveKind::NotFound,
                        message: "unknown blob",
                    });
                }
                Ok(ResolvedMedia::Url(Arc::from("https://example.test/a.png")))
            })
        }
    }

    #[derive(Debug)]
    struct CountingResolver {
        calls: Arc<AtomicUsize>,
    }

    impl MediaResolver for CountingResolver {
        fn resolve(&self, blob: &BlobRef) -> PortFuture<Result<ResolvedMedia, MediaResolveError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let id = blob.id().to_owned();
            Box::pin(async move {
                Ok(ResolvedMedia::Url(Arc::from(format!(
                    "https://example.test/{id}"
                ))))
            })
        }
    }

    #[derive(Debug)]
    struct SizedResolver {
        first_bytes: usize,
        second_bytes: usize,
    }

    impl MediaResolver for SizedResolver {
        fn resolve(&self, blob: &BlobRef) -> PortFuture<Result<ResolvedMedia, MediaResolveError>> {
            let size = if blob.id() == "blob-1" {
                self.first_bytes
            } else {
                self.second_bytes
            };
            Box::pin(async move {
                Ok(ResolvedMedia::Bytes {
                    media_type: Arc::from("image/png"),
                    bytes: Arc::from(vec![0_u8; size]),
                })
            })
        }
    }

    fn blob(id: &str) -> BlobRef {
        BlobRef::try_new(id, "image/png", 8, None, None::<&str>).expect("blob")
    }

    fn draft(blocks: Vec<ContentBlock>) -> ModelRequestDraft {
        ModelRequestDraft {
            model: ModelName::try_new("fixture").expect("model"),
            messages: Arc::from([Message::try_new(
                MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id"),
                MessageRole::User,
                blocks,
                Timestamp::from_unix_ms(1).expect("ts"),
                None,
                ProviderIds::empty(),
                Metadata::empty(),
            )
            .expect("message")]),
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
        }
    }

    #[tokio::test]
    async fn fixture_resolver_round_trips() {
        let resolver: Arc<dyn MediaResolver> = Arc::new(FixtureResolver);
        let blob = BlobRef::try_new("blob-1", "image/png", 8, None, None::<&str>).expect("blob");
        let outcome = resolver.resolve(&blob).await.expect("resolved");
        assert!(matches!(outcome, ResolvedMedia::Url(_)));
        let missing =
            BlobRef::try_new("missing", "image/png", 8, None, None::<&str>).expect("blob");
        let error = resolver.resolve(&missing).await.expect_err("missing");
        assert_eq!(error.kind, MediaResolveKind::NotFound);
    }

    #[tokio::test]
    async fn text_only_draft_does_not_require_a_resolver() {
        let resolved = resolve_draft_media(None, &draft(Vec::new()), 64)
            .await
            .expect("empty draft");
        assert!(resolved.is_empty());
    }

    #[tokio::test]
    async fn missing_resolver_fails_when_the_draft_has_media() {
        let error = resolve_draft_media(
            None,
            &draft(vec![ContentBlock::Image(MediaRef::new(blob("blob-1")))]),
            64,
        )
        .await
        .expect_err("missing resolver");
        assert_eq!(error, ResolveDraftMediaError::MissingResolver);
    }

    #[tokio::test]
    async fn distinct_blob_id_resolves_once_and_image_audio_file_are_included() {
        let calls = Arc::new(AtomicUsize::new(0));
        let resolver: Arc<dyn MediaResolver> = Arc::new(CountingResolver {
            calls: Arc::clone(&calls),
        });
        let repeated = MediaRef::new(blob("blob-1"));
        let media = resolve_draft_media(
            Some(&resolver),
            &draft(vec![
                ContentBlock::Image(repeated.clone()),
                ContentBlock::Image(repeated),
                ContentBlock::Audio(MediaRef::new(blob("blob-audio"))),
                ContentBlock::File(MediaRef::new(blob("blob-file"))),
            ]),
            64,
        )
        .await
        .expect("resolved");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(media.len(), 3);
        assert!(media.contains_key("blob-1"));
        assert!(media.contains_key("blob-audio"));
        assert!(media.contains_key("blob-file"));
    }

    #[tokio::test]
    async fn oversized_byte_payload_fails_closed() {
        let resolver: Arc<dyn MediaResolver> = Arc::new(SizedResolver {
            first_bytes: 9,
            second_bytes: 1,
        });
        let error = resolve_draft_media(
            Some(&resolver),
            &draft(vec![ContentBlock::Image(MediaRef::new(blob("blob-1")))]),
            8,
        )
        .await
        .expect_err("oversize");
        assert_eq!(error, ResolveDraftMediaError::Limit);
    }

    #[tokio::test]
    async fn aggregate_byte_payloads_fail_closed_when_each_blob_is_under() {
        let resolver: Arc<dyn MediaResolver> = Arc::new(SizedResolver {
            first_bytes: 5,
            second_bytes: 5,
        });
        let error = resolve_draft_media(
            Some(&resolver),
            &draft(vec![
                ContentBlock::Image(MediaRef::new(blob("blob-1"))),
                ContentBlock::Image(MediaRef::new(blob("blob-2"))),
            ]),
            8,
        )
        .await
        .expect_err("aggregate");
        assert_eq!(error, ResolveDraftMediaError::Limit);
    }

    #[tokio::test]
    async fn url_payloads_do_not_count_toward_the_stream_cap() {
        let resolver: Arc<dyn MediaResolver> = Arc::new(FixtureResolver);
        let media = resolve_draft_media(
            Some(&resolver),
            &draft(vec![
                ContentBlock::Image(MediaRef::new(blob("blob-1"))),
                ContentBlock::Image(MediaRef::new(blob("blob-2"))),
            ]),
            1,
        )
        .await
        .expect("urls are uncapped");
        assert_eq!(media.len(), 2);
    }

    #[tokio::test]
    async fn resolver_failure_is_surfaced() {
        let resolver: Arc<dyn MediaResolver> = Arc::new(FixtureResolver);
        let error = resolve_draft_media(
            Some(&resolver),
            &draft(vec![ContentBlock::Image(MediaRef::new(blob("missing")))]),
            64,
        )
        .await
        .expect_err("not found");
        assert!(matches!(
            error,
            ResolveDraftMediaError::Resolve(MediaResolveError {
                kind: MediaResolveKind::NotFound,
                ..
            })
        ));
    }
}
