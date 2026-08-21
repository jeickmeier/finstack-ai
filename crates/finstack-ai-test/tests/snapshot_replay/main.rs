//! snapshot replay contract snapshot discard, mismatch, and replay-equivalence proofs.

use std::sync::Arc;

use finstack_ai_kernel::{Digest, KernelState, SessionTag};
use finstack_ai_protocol::{CanonicalValue, decode_value, encode_snapshot, encode_value};
use finstack_ai_runtime::{CommitCoordinator, JournalStore, LoadRequest};

mod helpers;
use helpers::*;

include!("discard.rs");
include!("mismatch.rs");
include!("equivalence.rs");
