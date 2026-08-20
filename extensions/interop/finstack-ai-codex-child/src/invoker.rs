//! Codex exec child invoker.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::RunId;

use crate::config::CodexExecConfig;
use crate::identity::configuration;
use crate::CodexChildError;

/// Placeholder for one accepted child's live process state. A later task
/// replaces this with the real run type (request digest, handle, run
/// state); until then the map that holds it is intentionally unread.
#[derive(Debug)]
#[allow(clippy::zero_sized_map_values)]
pub(crate) struct CodexRun;

/// `AgentInvoker` leaf that spawns `codex exec --json` with frozen flags.
#[derive(Debug)]
#[allow(dead_code, clippy::zero_sized_map_values)]
pub struct CodexChildInvoker {
    pub(crate) config: CodexExecConfig,
    pub(crate) runs: Arc<Mutex<BTreeMap<RunId, CodexRun>>>,
}

impl CodexChildInvoker {
    /// Construct the invoker; fails closed on a missing binary or workspace.
    ///
    /// # Errors
    ///
    /// Returns [`CodexChildError::Configuration`] with reason
    /// `binary_missing`, `workspace_root_missing`, or `extra_args_invalid`.
    #[allow(clippy::zero_sized_map_values)]
    pub fn try_new(config: CodexExecConfig) -> Result<Self, CodexChildError> {
        if !config.binary.is_file() {
            return Err(configuration("binary_missing"));
        }
        if !config.workspace_root.is_dir() {
            return Err(configuration("workspace_root_missing"));
        }
        if config.extra_args.iter().any(|arg| arg.as_bytes().contains(&0)) {
            return Err(configuration("extra_args_invalid"));
        }
        Ok(Self {
            config,
            runs: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }
}
