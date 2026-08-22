//! conversation-tree contract immutable conversation tree, main lane, and child-mapping restore.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::ChildPlacement;
use finstack_ai_kernel::{
    AcceptRun, ContextPrepared, ConversationEntry, Digest, KernelInput, LaneCreated, LaneMoved,
    LaneTag, Message, MessageRole, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody,
    RecordEnvelope, RunPhase, SessionTag,
};
use finstack_ai_runtime::child::{ChildRunContext, ChildRunCoordinator, CompositionError};
use finstack_ai_runtime::commit::{CommitCoordinator, CommitCoordinatorError};
use finstack_ai_runtime::ports::journal::{JournalStore, LoadRequest, SnapshotSchedule};

mod helpers;
use helpers::*;

include!("tree.rs");
include!("restore.rs");
include!("child_mapping.rs");
