//! multi-lane session contract multi-lane session APIs, concurrency, and lineage fan-out.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai::ChildRunPolicy;
use finstack_ai_kernel::{
    AcceptRun, CancellationInitiator, CancellationPropagation, ChildPlacement, KernelInput,
    LaneCreated, LaneMoved, LaneTag, Metadata, RunPhase, SessionTag,
};
use finstack_ai_runtime::{
    AGENT_INVOKE_INVALID_ACCEPTANCE, ChildCoordinationIds, ChildRunContext, ChildRunCoordinator,
    ExternalIdentityKey, ExternalIdentityMap, JournalStore, MemoryExternalIdentityMap,
    SessionError, SessionRuntime, Toolset,
};
use finstack_ai_store_sqlite::{
    SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits, SqliteSynchronous,
};
use finstack_ai_test::{LegalRestore, classify_phase};
use proptest::test_runner::{Config as ProptestConfig, RngSeed};

mod helpers;
use helpers::*;

include!("session.rs");
include!("subagent.rs");
include!("property.rs");
include!("document_ingest.rs");
include!("tool_policy.rs");
