use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendRequest, BudgetChargeReceipt, BudgetChargeRequest,
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
use crate::{
    AgentInvokeError, AgentInvoker, AgentRef, AuthorizationContext, BudgetCoordinator, BudgetError,
    BudgetLedger, BudgetOperationIds, BudgetRequest, BudgetReservationReceipt,
    BudgetReservationState, BudgetReserveRequest, ChildCoordinationIds, ChildPlacement,
    ChildRunContext, ChildRunCoordinator, ChildRunHandle, ChildRunLocator, ChildRunRequest,
    CompositionError, LoadRequest, LoadedSession, OperationLocator, PortFuture, SnapshotReceipt,
    SnapshotRequest, StateSnapshotRequest, StoreHealth, child_relation_digest,
};

include!("fixtures.rs");
include!("commit_order.rs");
include!("composition.rs");
include!("budget.rs");
include!("store_integrity.rs");
include!("cancel_dispatch.rs");
include!("middleware_chain.rs");
