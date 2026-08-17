//! PR-041 snapshot discard, mismatch, and replay-equivalence proofs.

use std::sync::Arc;

use finstack_ai_kernel::{Digest, KernelState, SessionTag};
use finstack_ai_protocol::{decode_value, encode_snapshot, encode_value, CanonicalValue};
use finstack_ai_runtime::{CommitCoordinator, JournalStore, LoadRequest};

mod helpers;
use helpers::*;

include!("discard.rs");
include!("mismatch.rs");
include!("equivalence.rs");
