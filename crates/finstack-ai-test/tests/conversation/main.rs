//! PR-046 immutable conversation tree, main lane, and child-mapping restore.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    AcceptRun, ContextPrepared, ConversationEntry, ConversationError, Digest, KernelInput,
    LaneCreated, LaneMoved, LaneTag, Message, MessageRole, RecordBody, RecordEnvelope, RunPhase,
    SessionTag, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION,
};
use finstack_ai_runtime::{
    ChildPlacement, ChildRunContext, ChildRunCoordinator, CommitCoordinator,
    CommitCoordinatorError, CompositionError, JournalStore, LoadRequest, SnapshotSchedule,
};

mod helpers;
use helpers::*;

include!("tree.rs");
include!("restore.rs");
include!("child_mapping.rs");
