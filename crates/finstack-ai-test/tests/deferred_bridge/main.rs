//! Deferred child-run bridge integration coverage.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai::{ChildRunBridge, ChildSettleOutcome, outstanding_deferrals};
use finstack_ai_kernel::PrincipalRef;
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::ingress::ExternalRouteOutcome;

mod helpers;
use helpers::*;

include!("completion.rs");
include!("sink.rs");
