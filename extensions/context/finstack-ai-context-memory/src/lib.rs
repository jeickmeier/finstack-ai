//! Reference in-process memory/retrieval `ContextProvider`.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    ArtifactId, ComponentId, ComponentInvocation, ContentBlock, Digest, InvocationRecovery,
    Metadata, Sensitivity, TextBlock, Version,
};
use finstack_ai_runtime::{
    ArtifactMetadata, ArtifactScope, ArtifactStore, Bytes, ContextAuthority, ContextCallContext,
    ContextContribution, ContextError, ContextItem, ContextItemKind, ContextOverflowPolicy,
    ContextProvenance, ContextProvider, ContextProviderDescriptor, ContextRequest, PortFuture,
    stage_required_artifact,
};
use thiserror::Error;

/// Memory construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MemoryError {
    /// Configuration is malformed.
    #[error("memory_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

#[derive(Debug, Clone)]
struct MemoryRecord {
    id: Arc<str>,
    keywords: Arc<[Arc<str>]>,
    artifact_name: Arc<str>,
    preview: Arc<str>,
    sensitivity: Sensitivity,
}

/// Tiny in-process index used only by this crate. Bodies live in [`ArtifactStore`].
#[derive(Debug, Default)]
pub struct MemoryIndex {
    records: Mutex<Vec<MemoryRecord>>,
}

/// In-process [`ArtifactStore`] used only by this reference provider.
#[derive(Debug, Default)]
pub struct InProcessArtifactStore {
    bodies: Mutex<BTreeMap<ArtifactId, Bytes>>,
}

impl ArtifactStore for InProcessArtifactStore {
    fn stage_put(
        &self,
        scope: ArtifactScope,
        content: Bytes,
        metadata: ArtifactMetadata,
    ) -> PortFuture<Result<finstack_ai_kernel::ArtifactRef, finstack_ai_runtime::ArtifactError>>
    {
        let digest = Digest::blob_content(&content);
        let mut artifact_id = [0_u8; 16];
        artifact_id.copy_from_slice(&digest.as_bytes()[..16]);
        let artifact_id = ArtifactId::from_bytes(artifact_id);
        let stored = self
            .bodies
            .lock()
            .map(|mut guard| {
                guard.insert(artifact_id, content.clone());
            })
            .map_err(|_| ());
        Box::pin(async move {
            stored.map_err(|()| finstack_ai_runtime::ArtifactError::Unavailable {
                message: Arc::from("memory artifact lock failed"),
            })?;
            let blob = finstack_ai_kernel::BlobRef::try_new(
                digest.to_hex(),
                metadata.media_type.as_ref(),
                u64::try_from(content.len()).unwrap_or(0),
                Some(digest),
                metadata.name.as_deref(),
            )
            .map_err(
                |error| finstack_ai_runtime::ArtifactError::InvalidMetadata {
                    message: Arc::from(error.to_string()),
                },
            )?;
            finstack_ai_kernel::ArtifactRef::try_new(
                artifact_id,
                metadata.kind.as_ref(),
                blob,
                digest,
                scope.digest()?,
                metadata.attributes,
            )
            .map_err(
                |error| finstack_ai_runtime::ArtifactError::InvalidMetadata {
                    message: Arc::from(error.to_string()),
                },
            )
        })
    }

    fn get(
        &self,
        _scope: ArtifactScope,
        artifact: finstack_ai_kernel::ArtifactRef,
    ) -> PortFuture<Result<Bytes, finstack_ai_runtime::ArtifactError>> {
        let bodies = self
            .bodies
            .lock()
            .map(|guard| guard.get(&artifact.id()).cloned())
            .map_err(|_| ());
        Box::pin(async move {
            let stored = bodies.map_err(|()| finstack_ai_runtime::ArtifactError::Unavailable {
                message: Arc::from("memory artifact lock failed"),
            })?;
            stored.ok_or(finstack_ai_runtime::ArtifactError::NotFound)
        })
    }
}

/// Reference memory/retrieval provider.
pub struct MemoryContextProvider {
    descriptor: ContextProviderDescriptor,
    store: Arc<dyn ArtifactStore>,
    index: Arc<MemoryIndex>,
    tenant_scope: Arc<str>,
}

impl MemoryContextProvider {
    /// Construct a provider with the crate-local in-process artifact store.
    ///
    /// # Errors
    ///
    /// Rejects an empty tenant scope or an invalid checked-in identity.
    pub fn try_new(tenant_scope: impl Into<Arc<str>>) -> Result<Self, MemoryError> {
        Self::try_with_store(Arc::new(InProcessArtifactStore::default()), tenant_scope)
    }

    /// Construct a provider bound to one artifact store and tenant scope.
    ///
    /// # Errors
    ///
    /// Rejects an empty tenant scope or an invalid checked-in identity.
    pub fn try_with_store(
        store: Arc<dyn ArtifactStore>,
        tenant_scope: impl Into<Arc<str>>,
    ) -> Result<Self, MemoryError> {
        let tenant_scope = tenant_scope.into();
        if tenant_scope.is_empty() || tenant_scope.as_bytes().contains(&0) {
            return Err(MemoryError::Configuration {
                reason: "invalid_tenant_scope",
            });
        }
        Ok(Self {
            descriptor: ContextProviderDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse("finstack.context.memory").map_err(|_| {
                        MemoryError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    version: Version {
                        major: 0,
                        minor: 0,
                        patch: 4,
                    },
                    configuration_digest: Digest::raw_json(tenant_scope.as_bytes()),
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                trusted_application_instructions: false,
                metadata: Metadata::empty(),
            },
            store,
            index: Arc::new(MemoryIndex::default()),
            tenant_scope,
        })
    }

    /// Stage one memory body through [`ArtifactStore`] and index keywords / exact id.
    ///
    /// # Errors
    ///
    /// Returns a context error when staging or indexing fails.
    pub async fn stage(
        &self,
        ctx: &ContextCallContext,
        id: impl Into<Arc<str>>,
        keywords: impl IntoIterator<Item = impl AsRef<str>>,
        body: impl AsRef<[u8]>,
        sensitivity: Sensitivity,
    ) -> Result<(), ContextError> {
        let id = id.into();
        if id.is_empty() || id.as_bytes().contains(&0) {
            return Err(contribution_invalid("memory id is invalid"));
        }
        if ctx.run.locator.tenant_scope.as_ref() != self.tenant_scope.as_ref() {
            return Err(contribution_invalid(
                "memory tenant scope does not match the committed effect",
            ));
        }
        let body = Bytes::copy_from_slice(body.as_ref());
        let preview = String::from_utf8_lossy(&body)
            .chars()
            .take(256)
            .collect::<String>();
        let artifact = stage_required_artifact(
            self.store.as_ref(),
            ArtifactScope {
                tenant_scope: Arc::clone(&self.tenant_scope),
                session_id: ctx.run.locator.session_id,
                run_id: Some(ctx.run.locator.run_id),
                sensitivity,
            },
            body,
            ArtifactMetadata {
                kind: Arc::from("memory-record"),
                media_type: Arc::from("text/plain"),
                name: Some(Arc::clone(&id)),
                attributes: Metadata::empty(),
            },
        )
        .await
        .map_err(|_| contribution_invalid("memory artifact staging failed"))?;
        let keywords = keywords
            .into_iter()
            .map(|value| Arc::<str>::from(value.as_ref()))
            .collect::<Vec<_>>();
        self.index
            .records
            .lock()
            .map_err(|_| contribution_invalid("memory index lock failed"))?
            .push(MemoryRecord {
                id,
                keywords: keywords.into(),
                artifact_name: artifact
                    .blob()
                    .name()
                    .map_or_else(|| Arc::from("memory"), Arc::from),
                preview: preview.into(),
                sensitivity,
            });
        Ok(())
    }
}

impl ContextProvider for MemoryContextProvider {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        let index = Arc::clone(&self.index);
        let tenant_scope = Arc::clone(&self.tenant_scope);
        Box::pin(async move {
            if ctx.run.locator.tenant_scope.as_ref() != tenant_scope.as_ref() {
                return Err(contribution_invalid(
                    "memory tenant scope does not match the committed effect",
                ));
            }
            let query = query_text(&request);
            let records = index
                .records
                .lock()
                .map_err(|_| contribution_invalid("memory index lock failed"))?
                .clone();
            let mut items = Vec::new();
            for record in records {
                if !matches_query(&record, &query) {
                    continue;
                }
                items.push(ContextItem::try_new(
                    ContextItemKind::Reference,
                    vec![ContentBlock::Text(
                        TextBlock::try_new(format!(
                            "{} [{}]",
                            record.preview, record.artifact_name
                        ))
                        .map_err(|_| contribution_invalid("memory preview is invalid"))?,
                    )],
                    ContextProvenance {
                        source_id: Arc::from("finstack.context.memory"),
                        source_ref: Some(Arc::clone(&record.id)),
                        external: true,
                    },
                    ContextAuthority::Untrusted,
                    0,
                    estimate_tokens(&record.preview),
                    record.sensitivity,
                    false,
                )?);
            }
            apply_budget(items, &request)
        })
    }
}

fn matches_query(record: &MemoryRecord, query: &str) -> bool {
    if query.is_empty() {
        return false;
    }
    query.split_whitespace().any(|token| {
        record.id.as_ref() == token
            || record
                .keywords
                .iter()
                .any(|keyword| keyword.eq_ignore_ascii_case(token))
    })
}

fn query_text(request: &ContextRequest) -> String {
    request
        .user_input
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text().to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn apply_budget(
    items: Vec<ContextItem>,
    request: &ContextRequest,
) -> Result<ContextContribution, ContextError> {
    let mut accepted = Vec::new();
    let mut tokens = 0_u64;
    let mut bytes = 0_u64;
    for item in items {
        let next_tokens = tokens.saturating_add(item.estimated_tokens);
        let next_bytes = bytes.saturating_add(item.bytes);
        if accepted.len() >= request.budget.max_items
            || next_tokens > request.budget.max_tokens
            || next_bytes > request.budget.max_bytes
        {
            return match request.budget.overflow {
                ContextOverflowPolicy::Reject => Err(ContextError::try_new(
                    finstack_ai_runtime::CONTEXT_BUDGET_EXCEEDED,
                    finstack_ai_kernel::ErrorCategory::Limit,
                    "memory contribution exceeds the committed budget",
                    Metadata::empty(),
                )
                .unwrap_or_else(Into::into)),
                ContextOverflowPolicy::TruncateWithDiagnostic => break,
            };
        }
        tokens = next_tokens;
        bytes = next_bytes;
        accepted.push(item);
    }
    let _ = (tokens, bytes);
    ContextContribution::try_new(accepted, Some("finstack.context.memory"))
}

fn estimate_tokens(text: &str) -> u64 {
    u64::try_from(text.len().div_ceil(4)).unwrap_or(1).max(1)
}

fn contribution_invalid(message: &'static str) -> ContextError {
    ContextError::try_new(
        finstack_ai_runtime::CONTEXT_CONTRIBUTION_INVALID,
        finstack_ai_kernel::ErrorCategory::Validation,
        message,
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

#[cfg(test)]
mod tests;
