//! Shared journal-store semantics for finstack-ai store backends.
//!
//! Pure functions over the kernel, protocol, and runtime port types. Backends
//! (memory, sqlite, future stores) own all storage plumbing and delegate the
//! store-contract decisions — idempotency, admission, chain verification,
//! windowing — to this crate so the semantics cannot drift between them.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]

mod append;
mod error;
mod scan;
mod snapshot;
mod verified;
mod window;

pub use append::{
    AppendIdentity, SessionUsage, admit_append_limits, build_committed_batch,
    check_append_sequence, classify_record_reuse, request_cbor, request_identity,
};
pub use error::protocol_error;
pub use scan::{scan_next_sequence, scan_start, validate_scan_limit};
pub use snapshot::{
    accelerated_from, admit_prune_snapshot, admit_snapshot_sequence, check_snapshot_size,
    encode_state_request, outstanding_count, tombstone_count,
};
pub use verified::{VerifiedHead, VerifiedHeadCache, VerifiedRead, verify_head_against_cache};
pub use window::{
    FROM_SEQUENCE_WINDOW, SNAPSHOT_WINDOW, WindowCodes, check_batch_alignment, select_tail_batches,
    verify_full_head, verify_tail_records,
};
