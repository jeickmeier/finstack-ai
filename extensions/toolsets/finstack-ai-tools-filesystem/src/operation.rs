use finstack_ai_runtime::{CancellationSignal, ErrorCategory, ToolError};

use crate::policy::{ProtectedPaths, ValidatedPath};
use crate::{FILESYSTEM_LIMIT_EXCEEDED, FileSystemLimits, fs_tool_error};

pub(crate) enum FileOperation {
    Read(ValidatedPath),
    Write {
        path: ValidatedPath,
        content: Vec<u8>,
    },
    Edit {
        path: ValidatedPath,
        old: String,
        new: String,
        replace_all: bool,
    },
    List(ValidatedPath),
    Glob {
        pattern: String,
    },
    Search {
        path: ValidatedPath,
        query: String,
        glob: Option<String>,
    },
}

pub(crate) struct OperationOutput {
    pub(crate) json: Vec<u8>,
    pub(crate) artifact_name: &'static str,
}

impl OperationOutput {
    pub(crate) fn try_new<T: serde::Serialize>(
        value: &T,
        artifact_name: &'static str,
    ) -> Result<Self, ToolError> {
        let json = serde_json::to_vec(value).map_err(|_| {
            fs_tool_error(
                "filesystem_io_error",
                ErrorCategory::Internal,
                "filesystem result serialization failed",
            )
        })?;
        if json.len() > finstack_ai_runtime::MAX_ARTIFACT_BYTES {
            return Err(fs_tool_error(
                FILESYSTEM_LIMIT_EXCEEDED,
                ErrorCategory::Limit,
                "filesystem result exceeds the artifact byte ceiling",
            ));
        }
        Ok(Self {
            json,
            artifact_name,
        })
    }
}

#[cfg(unix)]
impl FileOperation {
    pub(crate) fn execute(
        self,
        root: &crate::unix::Root,
        limits: FileSystemLimits,
        protected: &ProtectedPaths,
        cancellation: &CancellationSignal,
    ) -> Result<OperationOutput, ToolError> {
        root.execute(self, limits, protected, cancellation)
    }
}
