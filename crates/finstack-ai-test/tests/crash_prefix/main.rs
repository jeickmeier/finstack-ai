//! PR-048 crash-prefix matrix: drop the owner, recover, assert a legal class.
#![allow(
    clippy::too_many_lines,
    reason = "each prefix cluster is one legal-class matrix"
)]
#![allow(
    clippy::large_futures,
    reason = "coordinator fixtures are large; boxing would hide the matrix"
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    ActiveCapability, ActiveToolCallStatus, AppendRequest, AuthorizationEvidence, CancelRequested,
    CancellationInitiator, CapabilitiesActivated, CapabilityActivationSource, ChildPlacement,
    ComponentId, ContentBlock, Digest, Duration as KernelDuration, EffectDeferred, EffectFailed,
    EffectKind, ErrorCategory, ErrorDescriptor, ExternalEffectCompletedInput,
    ExternalEffectCompletion, ExternalEffectOutcome, ExternalHandleRef, InteractionExpired,
    InteractionKind, InteractionResolution, InteractionSettled, InteractionTag, KernelInput,
    LaneCreated, LaneTag, Message, MessageRole, Metadata, ModelSettled, ModelSettlement,
    OperationLocator, PrincipalRef, ProviderIds, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION,
    RawJson, ReconciliationPolicy, RecordBody, RecordDraft, RecordTag, ReducerStageOutcome,
    RequestInteraction, RetryClassification, RetryDirective, RunPhase, RunTag, SessionTag, Stage,
    TextBlock, ToolBatchSettled, ToolSettlement,
};
use finstack_ai_kernel::{ArtifactRef, BlobRef, ExternalEffectCompletionCommand, Sensitivity};
use finstack_ai_runtime::{
    ARTIFACT_INTEGRITY_FAILURE, ArtifactMetadata, ArtifactScope, ArtifactStoreLimits,
    BudgetCoordinator, BudgetOperationIds, ChildRunCoordinator, CompositionError,
    ExternalCompletionRouter, ExternalRouteError, IdempotencyHorizon, JournalStore, LaneAppendIds,
    LaneCreateIds, LoadRequest, OpaqueSnapshot, PruneRequest, SecurityAuditCategory,
    SecurityAuditGate, SessionCreateIds, SessionError, SessionRuntime, SnapshotRequest,
    WriteMetadataRequest, validate_staged_artifact,
};
use finstack_ai_test::{LegalRestore, all_activated_record_bodies, classify_phase};

mod helpers;
use helpers::*;

include!("catalog.rs");
include!("prefix_w.rs");
include!("prefix_d.rs");
include!("prefix_i.rs");
include!("prefix_f.rs");
include!("prefix_a.rs");
include!("prefix_l.rs");
include!("prefix_b.rs");
include!("prefix_n.rs");
include!("prefix_c.rs");
include!("sqlite.rs");
