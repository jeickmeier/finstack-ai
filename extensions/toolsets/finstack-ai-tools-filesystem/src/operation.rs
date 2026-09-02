use finstack_ai_kernel::ErrorCategory;
use finstack_ai_runtime::ports::tool::ToolError;

#[cfg(unix)]
use crate::FileSystemLimits;
use crate::policy::ValidatedPath;
use crate::{FILESYSTEM_IO_ERROR, FILESYSTEM_LIMIT_EXCEEDED, fs_tool_error};

/// Resource ceilings threaded from the toolset into one operation: the
/// explicit [`FileSystemLimits`] plus the artifact-store-derived byte
/// ceiling applied to the serialized result.
#[cfg(unix)]
#[derive(Clone, Copy)]
pub(crate) struct FileSystemCeilings {
    pub(crate) limits: FileSystemLimits,
    pub(crate) max_artifact_bytes: usize,
}

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
        max_artifact_bytes: usize,
    ) -> Result<Self, ToolError> {
        let json = serde_json::to_vec(value).map_err(|_| {
            fs_tool_error(
                FILESYSTEM_IO_ERROR,
                ErrorCategory::Internal,
                "filesystem result serialization failed",
            )
        })?;
        if json.len() > max_artifact_bytes {
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
    pub(crate) const fn is_search(&self) -> bool {
        matches!(self, Self::Search { .. })
    }
}
