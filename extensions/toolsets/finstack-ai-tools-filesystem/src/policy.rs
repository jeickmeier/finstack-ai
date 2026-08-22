use std::sync::Arc;

use finstack_ai_runtime::ports::tool::ToolError;

use crate::{FileSystemError, invalid_error, policy_error};

const MAX_PATH_BYTES: usize = 4_096;
const MAX_COMPONENTS: usize = 256;
const MAX_COMPONENT_BYTES: usize = 255;
const MAX_PATTERNS: usize = 256;
const MAX_PATTERN_BYTES: usize = 512;

#[derive(Debug, Clone)]
pub(crate) struct ProtectedPaths(Arc<[Arc<str>]>);

impl ProtectedPaths {
    pub(crate) fn defaults() -> Self {
        Self(Arc::from([
            Arc::from(".git"),
            Arc::from(".env"),
            Arc::from(".ssh"),
        ]))
    }

    pub(crate) fn try_new<I, S>(patterns: I) -> Result<Self, FileSystemError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let patterns = patterns
            .into_iter()
            .map(|pattern| Arc::<str>::from(pattern.as_ref()))
            .collect::<Vec<_>>();
        if patterns.len() > MAX_PATTERNS
            || patterns.iter().any(|pattern| {
                pattern.is_empty()
                    || pattern.len() > MAX_PATTERN_BYTES
                    || pattern.starts_with('/')
                    || pattern.ends_with('/')
                    || pattern.contains("//")
                    || pattern.contains('\\')
                    || pattern.as_bytes().contains(&0)
                    || pattern.split('/').any(|component| component == "..")
            })
        {
            return Err(FileSystemError::Configuration {
                reason: "invalid_protected_path_pattern",
            });
        }
        Ok(Self(Arc::from(patterns)))
    }

    pub(crate) fn denies(&self, path: &str) -> bool {
        self.0.iter().any(|pattern| {
            if pattern.contains(['*', '?', '/']) {
                glob_matches(pattern, path)
            } else {
                path.split('/')
                    .any(|component| component == pattern.as_ref())
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedPath {
    components: Arc<[Arc<str>]>,
    normalized: Arc<str>,
}

impl ValidatedPath {
    pub(crate) fn from_validated_parts(components: Arc<[Arc<str>]>, normalized: Arc<str>) -> Self {
        Self {
            components,
            normalized,
        }
    }

    pub(crate) fn try_file(path: &str, protected: &ProtectedPaths) -> Result<Self, ToolError> {
        let value = Self::parse(path, false)?;
        value.enforce(protected)?;
        Ok(value)
    }

    pub(crate) fn try_directory(path: &str, protected: &ProtectedPaths) -> Result<Self, ToolError> {
        let value = Self::parse(path, true)?;
        value.enforce(protected)?;
        Ok(value)
    }

    fn parse(path: &str, allow_empty: bool) -> Result<Self, ToolError> {
        if path.len() > MAX_PATH_BYTES
            || path.starts_with('/')
            || path.ends_with('/')
            || path.contains("//")
            || path.contains('\\')
            || path.as_bytes().contains(&0)
        {
            return Err(invalid_error("filesystem path is invalid"));
        }
        if path.is_empty() {
            return if allow_empty {
                Ok(Self {
                    components: Arc::from([]),
                    normalized: Arc::from(""),
                })
            } else {
                Err(invalid_error("filesystem file path is empty"))
            };
        }
        let raw = path.split('/').collect::<Vec<_>>();
        if raw.len() > MAX_COMPONENTS
            || raw.iter().any(|component| {
                component.is_empty()
                    || *component == "."
                    || *component == ".."
                    || component.len() > MAX_COMPONENT_BYTES
            })
        {
            return Err(invalid_error("filesystem path components are invalid"));
        }
        let components = raw.into_iter().map(Arc::<str>::from).collect::<Vec<_>>();
        Ok(Self {
            normalized: Arc::from(path),
            components: Arc::from(components),
        })
    }

    fn enforce(&self, protected: &ProtectedPaths) -> Result<(), ToolError> {
        if protected.denies(&self.normalized) {
            Err(policy_error("filesystem path is protected by host policy"))
        } else {
            Ok(())
        }
    }

    pub(crate) fn components(&self) -> &[Arc<str>] {
        &self.components
    }

    pub(crate) fn normalized(&self) -> &str {
        &self.normalized
    }
}

pub(crate) fn validate_glob(pattern: &str) -> Result<(), ToolError> {
    if pattern.is_empty()
        || pattern.len() > MAX_PATTERN_BYTES
        || pattern.starts_with('/')
        || pattern.ends_with('/')
        || pattern.contains("//")
        || pattern.contains('\\')
        || pattern.as_bytes().contains(&0)
        || pattern.split('/').any(|component| component == "..")
    {
        return Err(invalid_error("filesystem glob pattern is invalid"));
    }
    Ok(())
}

pub(crate) fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.as_bytes();
    let path = path.as_bytes();
    let mut previous = vec![false; path.len() + 1];
    previous[0] = true;
    let mut index = 0;
    while index < pattern.len() {
        let double_star = pattern[index] == b'*' && pattern.get(index + 1).copied() == Some(b'*');
        let recursive_prefix = double_star && pattern.get(index + 2).copied() == Some(b'/');
        let token = pattern[index];
        if recursive_prefix {
            let mut current = previous.clone();
            let mut reachable = previous[0];
            for position in 1..=path.len() {
                if path[position - 1] == b'/' && reachable {
                    current[position] = true;
                }
                reachable |= previous[position];
            }
            previous = current;
            index += 3;
            continue;
        }
        if double_star {
            while pattern.get(index + 1).copied() == Some(b'*') {
                index += 1;
            }
        }
        let mut current = vec![false; path.len() + 1];
        if token == b'*' {
            current[0] = previous[0];
            for position in 1..=path.len() {
                let can_consume = double_star || path[position - 1] != b'/';
                current[position] = previous[position] || (can_consume && current[position - 1]);
            }
        } else if token == b'?' {
            for position in 1..=path.len() {
                current[position] = previous[position - 1] && path[position - 1] != b'/';
            }
        } else {
            for position in 1..=path.len() {
                current[position] = previous[position - 1] && path[position - 1] == token;
            }
        }
        previous = current;
        index += 1;
    }
    previous[path.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_has_component_and_recursive_semantics() {
        assert!(glob_matches("src/**/*.rs", "src/a/b.rs"));
        assert!(glob_matches("*.rs", "lib.rs"));
        assert!(!glob_matches("*.rs", "src/lib.rs"));
        assert!(glob_matches("src/?.rs", "src/a.rs"));
    }

    #[test]
    fn paths_reject_traversal_and_protected_components() {
        let protected = ProtectedPaths::defaults();
        assert!(ValidatedPath::try_file("../secret", &protected).is_err());
        assert!(ValidatedPath::try_file("a/.git/config", &protected).is_err());
        assert!(ValidatedPath::try_file("src/lib.rs", &protected).is_ok());
    }
}
