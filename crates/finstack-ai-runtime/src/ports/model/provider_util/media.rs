//! Host-supplied media-blob resolution for provider leaves (see the
//! media-resolver ADR). Not a registered port: hosts hand resolvers to
//! provider configs, mirroring [`super::credentials::CredentialStore`].

use std::sync::Arc;

use finstack_ai_kernel::BlobRef;

use crate::{PortFuture, PortObject};

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

#[cfg(test)]
mod tests {
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

    #[tokio::test]
    async fn fixture_resolver_round_trips() {
        let resolver: Arc<dyn MediaResolver> = Arc::new(FixtureResolver);
        let blob = BlobRef::try_new("blob-1", "image/png", 8, None, None::<&str>).expect("blob");
        let resolved = resolver.resolve(&blob).await.expect("resolved");
        assert!(matches!(resolved, ResolvedMedia::Url(_)));
        let missing =
            BlobRef::try_new("missing", "image/png", 8, None, None::<&str>).expect("blob");
        let error = resolver.resolve(&missing).await.expect_err("missing");
        assert_eq!(error.kind, MediaResolveKind::NotFound);
    }
}
