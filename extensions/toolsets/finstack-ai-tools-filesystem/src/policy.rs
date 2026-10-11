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
    /// Rebuild a path from components that were already validated as part of
    /// a longer `ValidatedPath` (the parent directory of a leaf).
    pub(crate) fn from_components(components: &[Arc<str>]) -> Self {
        let normalized = components
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<&str>>()
            .join("/");
        Self {
            components: Arc::from(components),
            normalized: Arc::from(normalized),
        }
    }

    pub(crate) fn try_file(path: &str, protected: &ProtectedPaths) -> Result<Self, ToolError> {
        Self::parse(path, false, protected)
    }

    pub(crate) fn try_directory(path: &str, protected: &ProtectedPaths) -> Result<Self, ToolError> {
        Self::parse(path, true, protected)
    }

    fn parse(path: &str, allow_empty: bool, protected: &ProtectedPaths) -> Result<Self, ToolError> {
        if path.len() > MAX_PATH_BYTES
            || path.starts_with('/')
            || path.ends_with('/')
            || path.contains("//")
            || path.contains('\\')
            || path.as_bytes().contains(&0)
        {
            return Err(invalid_error("filesystem path is invalid"));
        }
        if path.is_empty() && !allow_empty {
            return Err(invalid_error("filesystem file path is empty"));
        }
        let raw = if path.is_empty() {
            Vec::new()
        } else {
            path.split('/').collect::<Vec<_>>()
        };
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
        if protected.denies(path) {
            return Err(policy_error("filesystem path is protected by host policy"));
        }
        Ok(Self {
            components: raw.into_iter().map(Arc::<str>::from).collect(),
            normalized: Arc::from(path),
        })
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

/// Match slash-separated relative paths against one glob pattern, reusing
/// two dynamic-programming rows across calls.
///
/// `*` and `?` never cross `/`; `**` matches across separators and a
/// leading `**/` also matches zero directories. Any other byte is literal.
pub(crate) struct GlobMatcher<'p> {
    pattern: &'p [u8],
    previous: Vec<bool>,
    current: Vec<bool>,
}

impl<'p> GlobMatcher<'p> {
    pub(crate) fn new(pattern: &'p str) -> Self {
        Self {
            pattern: pattern.as_bytes(),
            previous: Vec::new(),
            current: Vec::new(),
        }
    }

    pub(crate) fn matches(&mut self, path: &str) -> bool {
        let pattern = self.pattern;
        let path = path.as_bytes();
        let width = path.len() + 1;
        self.previous.clear();
        self.previous.resize(width, false);
        self.current.clear();
        self.current.resize(width, false);
        self.previous[0] = true;
        let mut index = 0;
        while index < pattern.len() {
            let double_star =
                pattern[index] == b'*' && pattern.get(index + 1).copied() == Some(b'*');
            let recursive_prefix = double_star && pattern.get(index + 2).copied() == Some(b'/');
            let token = pattern[index];
            if recursive_prefix {
                self.step_recursive_prefix(path);
                index += 3;
                continue;
            }
            if double_star {
                while pattern.get(index + 1).copied() == Some(b'*') {
                    index += 1;
                }
            }
            self.step_token(path, token, double_star);
            index += 1;
        }
        self.previous[path.len()]
    }

    fn step_recursive_prefix(&mut self, path: &[u8]) {
        self.current.copy_from_slice(&self.previous);
        let mut reachable = self.previous[0];
        for position in 1..=path.len() {
            if path[position - 1] == b'/' && reachable {
                self.current[position] = true;
            }
            reachable |= self.previous[position];
        }
        core::mem::swap(&mut self.previous, &mut self.current);
    }

    fn step_token(&mut self, path: &[u8], token: u8, double_star: bool) {
        self.current.fill(false);
        if token == b'*' {
            self.current[0] = self.previous[0];
            for position in 1..=path.len() {
                let can_consume = double_star || path[position - 1] != b'/';
                self.current[position] =
                    self.previous[position] || (can_consume && self.current[position - 1]);
            }
        } else if token == b'?' {
            for position in 1..=path.len() {
                self.current[position] = self.previous[position - 1] && path[position - 1] != b'/';
            }
        } else {
            for position in 1..=path.len() {
                self.current[position] = self.previous[position - 1] && path[position - 1] == token;
            }
        }
        core::mem::swap(&mut self.previous, &mut self.current);
    }
}

/// One-shot convenience over [`GlobMatcher`] for callers without a hot loop.
pub(crate) fn glob_matches(pattern: &str, path: &str) -> bool {
    GlobMatcher::new(pattern).matches(path)
}

/// The deepest directory named literally by `pattern` before its first glob
/// metacharacter (`*`, `?`, or `[`): the only subtree that can hold matches.
///
/// Returns `""` (the root) when the first path component already contains a
/// metacharacter. A pattern without metacharacters names its parent
/// directory.
pub(crate) fn literal_prefix_directory(pattern: &str) -> &str {
    let literal_end = pattern.find(['*', '?', '[']).unwrap_or(pattern.len());
    let literal = &pattern[..literal_end];
    literal.rfind('/').map_or("", |slash| &literal[..slash])
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

    /// The pre-`GlobMatcher` allocating implementation, kept as the parity oracle.
    fn legacy_glob_matches(pattern: &str, path: &str) -> bool {
        let pattern = pattern.as_bytes();
        let path = path.as_bytes();
        let mut previous = vec![false; path.len() + 1];
        previous[0] = true;
        let mut index = 0;
        while index < pattern.len() {
            let double_star =
                pattern[index] == b'*' && pattern.get(index + 1).copied() == Some(b'*');
            let recursive_prefix = double_star && pattern.get(index + 2).copied() == Some(b'/');
            let token = pattern[index];
            if recursive_prefix {
                previous = legacy_recursive_prefix_row(&previous, path);
                index += 3;
                continue;
            }
            if double_star {
                while pattern.get(index + 1).copied() == Some(b'*') {
                    index += 1;
                }
            }
            previous = legacy_token_row(&previous, path, token, double_star);
            index += 1;
        }
        previous[path.len()]
    }

    fn legacy_recursive_prefix_row(previous: &[bool], path: &[u8]) -> Vec<bool> {
        let mut current = previous.to_vec();
        let mut reachable = previous[0];
        for position in 1..=path.len() {
            if path[position - 1] == b'/' && reachable {
                current[position] = true;
            }
            reachable |= previous[position];
        }
        current
    }

    fn legacy_token_row(previous: &[bool], path: &[u8], token: u8, double_star: bool) -> Vec<bool> {
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
        current
    }

    #[test]
    fn glob_matcher_parity() {
        let patterns = [
            "*.rs",
            "**/*.rs",
            "src/**/*.rs",
            "src/*/*.rs",
            "src/?.rs",
            "**",
            "***/x",
            "a**b",
            "**/**/c",
            "src/lib.rs",
            "*",
            "?",
            "",
            "src/**",
            "**/src/**/mod.rs",
        ];
        // Deliberately mixed lengths so one matcher's rows shrink and grow.
        let paths = [
            "src/a/b.rs",
            "lib.rs",
            "src/lib.rs",
            "src/a.rs",
            "",
            "a/b/c",
            "x",
            "src/deep/er/mod.rs",
            "axb",
            "a/b",
            "src/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/t/u/v/w/x/y/z.rs",
            "c",
            "src",
            "src/",
            "b.rs",
        ];
        for pattern in patterns {
            let mut matcher = GlobMatcher::new(pattern);
            for path in paths {
                assert_eq!(
                    matcher.matches(path),
                    legacy_glob_matches(pattern, path),
                    "pattern {pattern:?} path {path:?}"
                );
                assert_eq!(
                    glob_matches(pattern, path),
                    legacy_glob_matches(pattern, path)
                );
            }
            // Reuse after a long path must not leak stale row bits.
            for path in paths.iter().rev() {
                assert_eq!(
                    matcher.matches(path),
                    legacy_glob_matches(pattern, path),
                    "pattern {pattern:?} path {path:?} (reverse)"
                );
            }
        }
    }

    #[test]
    fn literal_prefix_stops_at_the_first_metacharacter() {
        assert_eq!(literal_prefix_directory("src/**/*.rs"), "src");
        assert_eq!(literal_prefix_directory("src/a/b?.rs"), "src/a");
        assert_eq!(literal_prefix_directory("*.rs"), "");
        assert_eq!(literal_prefix_directory("**/*.rs"), "");
        assert_eq!(literal_prefix_directory("src[0]/x"), "");
        assert_eq!(literal_prefix_directory("src/lib.rs"), "src");
        assert_eq!(literal_prefix_directory("lib.rs"), "");
        assert_eq!(literal_prefix_directory("a/b/c/*"), "a/b/c");
    }

    #[test]
    fn paths_reject_traversal_and_protected_components() {
        let protected = ProtectedPaths::defaults();
        assert!(ValidatedPath::try_file("../secret", &protected).is_err());
        assert!(ValidatedPath::try_file("a/.git/config", &protected).is_err());
        assert!(ValidatedPath::try_file("src/lib.rs", &protected).is_ok());
    }
}
