//! local-workflow contract reference-driver proofs (A01, A02, A04, TM-19).

#![allow(
    clippy::large_futures,
    reason = "contract tests keep driver setup inline for readable state-machine scenarios"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration as StdDuration;

use finstack_ai_kernel::ToolFailurePolicy;
use finstack_ai_kernel::{
    AuthorizationEvidence, ComponentId, Duration as KernelDuration, EffectId,
    ExternalEffectCompletion, ExternalEffectCompletionCommand, ExternalEffectOutcome,
    ExternalHandleRef, InteractionResolution, InteractionResolutionCommand, Metadata,
    OperationLocator, PrincipalRef, ProviderIds, RawJson, ReconciliationPolicy,
    ReducerStageOutcome, RetryClassification, RetryDirective, RetrySafety, RunPhase, Stage, Usage,
};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::events::EventHubConfig;
use finstack_ai_runtime::ids::{Clock, ExternalClock};
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::ports::model::{
    ApprovalGrantMode, ApprovalMetadata, ApprovalRequirement, Model, ModelDeferral, ModelResponse,
    ModelStreamItem, ModelStreamLimits, ModelToolCall, SideEffectClass, ToolCallDelta, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{
    JsonSchemaToolValidatorCompiler, ResolvedToolCatalog, ToolExecutionPolicy, ToolPolicyDecision,
    ToolResult, ToolStreamItem, ToolStreamLimits, Toolset, ToolsetRegistration,
};
use finstack_ai_runtime::run::{
    ModelTaskConfig, RunTaskConfig, RunTaskOwner, SameIdentityRetryPolicy, ToolTaskConfig,
};
use finstack_ai_runtime::testing::ManualDriveAction;
use finstack_ai_runtime::workflow::{WorkflowSession, WorkflowWait};
use finstack_ai_store_sqlite::{
    SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits, SqliteSynchronous,
};
use finstack_ai_test::{
    ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction, ScriptedToolPlan,
    ScriptedToolset,
};
use finstack_ai_workflow_local::{
    CronSchedule, CronScheduleStore, IntervalSchedule, LocalWorkflowDriver, SqliteCronStore,
};
use tempfile::TempDir;

mod helpers;
use helpers::*;

include!("driver.rs");
include!("loop_parity.rs");
include!("restart.rs");
include!("tenant.rs");
