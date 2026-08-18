//! PR-059 reference-driver proofs (A01, A02, A04, TM-19).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    AuthorizationEvidence, ComponentId, Duration as KernelDuration, EffectId,
    ExternalEffectCompletion, ExternalEffectCompletionCommand, ExternalEffectOutcome,
    ExternalHandleRef, InteractionResolution, InteractionResolutionCommand, Metadata,
    OperationLocator, PrincipalRef, ProviderIds, RawJson, ReconciliationPolicy,
    ReducerStageOutcome, RetryClassification, RetryDirective, RetrySafety, RunPhase, Stage, Usage,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, Clock, CommitCoordinator, EventHubConfig, ExternalClock,
    JournalStore, JsonSchemaToolValidatorCompiler, ManualDriveAction, Model, ModelDeferral,
    ModelResponse, ModelStreamItem, ModelStreamLimits, ModelTaskConfig, ModelToolCall,
    ResolvedToolCatalog, RunTaskConfig, RunTaskOwner, SameIdentityRetryPolicy, SideEffectClass,
    ToolCallDelta, ToolExecutionPolicy, ToolFailurePolicy, ToolPolicyDecision, ToolResult,
    ToolSpec, ToolStreamItem, ToolStreamLimits, ToolTaskConfig, Toolset, ToolsetRegistration,
    WorkflowSession, WorkflowWait,
};
use finstack_ai_store_sqlite::{
    SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits, SqliteSynchronous,
};
use finstack_ai_test::{
    ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction, ScriptedToolPlan,
    ScriptedToolset,
};
use finstack_ai_workflow_local::{
    CronExpression, CronSchedule, CronScheduleStore, LocalWorkflowDriver, SqliteCronStore,
};
use tempfile::TempDir;

mod helpers;
use helpers::*;

include!("driver.rs");
include!("restart.rs");
include!("tenant.rs");
