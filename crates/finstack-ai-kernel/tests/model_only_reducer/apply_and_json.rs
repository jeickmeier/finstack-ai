use finstack_ai_kernel::{ArtifactRef, ArtifactTag, BlobRef, EffectOutputKind, KernelState};

use super::tool_batches::{CALL_A, call, execute};
use super::*;
use finstack_ai_kernel::{ToolBatchContinuation, ToolExecutionMode, ToolFailurePolicy};

include!("apply_and_json/decide_apply.rs");
include!("apply_and_json/stage_digests.rs");
include!("apply_and_json/identity_terminal.rs");
include!("apply_and_json/decode_bounds.rs");
include!("apply_and_json/json_fixtures.rs");
