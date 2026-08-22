use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendRequest, BoundedMap, BudgetChargeReceipt, BudgetChargeRequest,
    BudgetPropagation, BudgetReleaseReceipt, BudgetReleaseRequest, CancellationPropagation,
    ContentBlock, DeadlinePropagation, Digest, EffectCompleted, EffectOutputContract,
    EffectOutputKind, Id, IdTag, LaneCreated, LaneTag, Message, MessageRole, Metadata,
    ModelSettled, ModelSettlement, PostCommitAction, PrincipalPropagation, PrincipalRef,
    ProviderIds, RawJson, RecordBody, RecordEnvelope, ReducerStageOutcome, RetrySafety,
    RunAccepted, RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext, SessionTag,
    Stage, StageCursor, TextBlock, Timestamp, Usage,
};

use super::session_commit::session_draft;
use super::*;
use crate::budget::{BudgetError, BudgetLedger, BudgetReservationState};
use crate::child::{
    AgentInvokeError, AgentInvoker, AgentRef, BudgetCoordinator, BudgetOperationIds,
    ChildCoordinationIds, ChildRunContext, ChildRunCoordinator, ChildRunHandle, ChildRunRequest,
    CompositionError, child_relation_digest,
};
use crate::ports::PortFuture;
use crate::ports::journal::{
    JournalStore, JournalStoreDescriptor, LoadRequest, LoadedSession, SnapshotReceipt,
    SnapshotRequest, StateSnapshotRequest, StoreError, StoreHealth,
};
use crate::ports::model::AuthorizationContext;
use crate::{
    BudgetRequest, BudgetReservationReceipt, BudgetReserveRequest, ChildPlacement, ChildRunLocator,
    OperationLocator,
};

include!("fixtures.rs");
include!("commit_order.rs");
include!("composition.rs");
include!("budget.rs");
include!("store_integrity.rs");
include!("cancel_dispatch.rs");
include!("middleware_chain.rs");
