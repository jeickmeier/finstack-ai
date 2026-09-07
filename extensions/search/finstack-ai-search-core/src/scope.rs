use std::sync::Arc;

use finstack_ai_kernel::Digest;
use serde::{Deserialize, Serialize};

use crate::{SearchError, configuration_digest, validate_id};

/// Authenticated application scope. Every populated field narrows authority.
/// A facade binds this at construction; model tool arguments cannot set it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchScope {
    /// Required tenant, always matched exactly.
    pub tenant: Arc<str>,
    /// Optional user boundary.
    pub user: Option<Arc<str>>,
    /// Optional agent boundary.
    pub agent: Option<Arc<str>>,
    /// Optional workspace boundary.
    pub workspace: Option<Arc<str>>,
}

impl SearchScope {
    /// Construct a tenant scope. Add optional dimensions before binding a source.
    ///
    /// # Errors
    /// Rejects invalid tenant identifiers.
    pub fn try_new(tenant: &str) -> Result<Self, SearchError> {
        validate_id(tenant)?;
        Ok(Self {
            tenant: Arc::from(tenant),
            user: None,
            agent: None,
            workspace: None,
        })
    }

    /// Validate all populated dimensions.
    ///
    /// # Errors
    /// Rejects missing/invalid tenant and malformed optional identifiers.
    pub fn validate(&self) -> Result<(), SearchError> {
        validate_id(&self.tenant)?;
        for dimension in [&self.user, &self.agent, &self.workspace]
            .into_iter()
            .flatten()
        {
            validate_id(dimension)?;
        }
        Ok(())
    }

    /// Exact complete-scope fingerprint used in derived indexes.
    ///
    /// # Errors
    /// Rejects malformed scope or encoding failures.
    pub fn digest(&self) -> Result<Digest, SearchError> {
        self.validate()?;
        configuration_digest("search-scope", self)
    }
}

/// Explicit source scope mapping. Both choices always query the complete bound
/// scope, never a less restrictive scope reconstructed from omitted fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeMapping {
    /// Require equality of all four dimensions (default).
    #[default]
    Exact,
    /// Allow a caller to omit dimensions already bound by the source. Supplied
    /// dimensions must match; the source retains every bound restriction. A
    /// caller cannot add a dimension the source is unable to enforce.
    RestrictToBound,
}

impl ScopeMapping {
    /// Validate the mapping and return the complete source-bound scope.
    ///
    /// # Errors
    /// Rejects tenant conflicts, supplied dimension conflicts, and attempts to
    /// narrow a source lacking that dimension.
    pub fn resolve(
        self,
        bound: &SearchScope,
        requested: &SearchScope,
    ) -> Result<SearchScope, SearchError> {
        bound.validate()?;
        requested.validate()?;
        if bound.tenant != requested.tenant {
            return Err(SearchError::SearchScopeDenied);
        }
        for (held, asked) in [
            (&bound.user, &requested.user),
            (&bound.agent, &requested.agent),
            (&bound.workspace, &requested.workspace),
        ] {
            if held != asked && (self == Self::Exact || asked.is_some()) {
                return Err(SearchError::SearchScopeDenied);
            }
        }
        Ok(bound.clone())
    }
}
