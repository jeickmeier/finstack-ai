//! Named provider credentials. The store is host-supplied and never reads
//! environment variables.

use core::fmt;
use std::collections::BTreeMap;
use std::sync::Arc;

use super::secret::SecretString;

/// Why a credential name was rejected.
///
/// Leaf crates map each variant to their own stable adapter code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialRejected {
    /// Empty name.
    Empty,
    /// Name contains a NUL byte.
    ContainsNul,
}

/// Explicit provider authentication configuration.
#[derive(Clone, PartialEq, Eq)]
pub enum Authentication {
    /// No credential, suitable for keyless local loopback.
    None,
    /// Bearer credential (`Authorization: Bearer …`).
    Bearer(SecretString),
    /// API-key credential (for example `x-api-key`).
    ApiKey(SecretString),
}

impl fmt::Debug for Authentication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("None"),
            Self::Bearer(_) => formatter.write_str("Bearer([REDACTED])"),
            Self::ApiKey(_) => formatter.write_str("ApiKey([REDACTED])"),
        }
    }
}

/// Named credential reference stored on a route. Never a secret literal.
#[derive(Clone, PartialEq, Eq)]
pub struct CredentialReference {
    name: Arc<str>,
}

impl CredentialReference {
    /// Construct one non-empty credential reference name.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialRejected`] for an empty or NUL-bearing name.
    pub fn try_new(name: impl AsRef<str>) -> Result<Self, CredentialRejected> {
        let name = name.as_ref();
        if name.is_empty() {
            return Err(CredentialRejected::Empty);
        }
        if name.as_bytes().contains(&0) {
            return Err(CredentialRejected::ContainsNul);
        }
        Ok(Self {
            name: Arc::from(name),
        })
    }

    /// Configured reference name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.name
    }
}

impl fmt::Debug for CredentialReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialReference")
            .field("name", &self.name)
            .finish()
    }
}

/// Explicit map of credential references to redacted authentication values.
///
/// The store is supplied by the host. It never reads environment variables.
#[derive(Clone, Default)]
pub struct CredentialStore {
    entries: BTreeMap<Arc<str>, Authentication>,
}

impl fmt::Debug for CredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialStore")
            .field("names", &self.entries.keys().cloned().collect::<Vec<_>>())
            .finish()
    }
}

impl CredentialStore {
    /// Construct an empty store.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Insert one named credential.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialRejected`] for an empty or NUL-bearing name.
    pub fn insert(
        &mut self,
        name: impl AsRef<str>,
        authentication: Authentication,
    ) -> Result<(), CredentialRejected> {
        let reference = CredentialReference::try_new(name)?;
        self.entries
            .insert(Arc::from(reference.as_str()), authentication);
        Ok(())
    }

    /// Resolve one named credential.
    #[must_use]
    pub fn resolve(&self, reference: &CredentialReference) -> Option<&Authentication> {
        self.entries.get(reference.name.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::{Authentication, CredentialReference, CredentialRejected, CredentialStore};
    use crate::ports::model::provider_util::secret::SecretString;

    #[test]
    fn store_resolves_named_entries_and_redacts_debug() {
        let mut store = CredentialStore::empty();
        let secret = SecretString::try_new("sk-test").expect("secret");
        store
            .insert("prod", Authentication::Bearer(secret))
            .expect("insert");
        let reference = CredentialReference::try_new("prod").expect("reference");
        assert!(matches!(
            store.resolve(&reference),
            Some(Authentication::Bearer(_))
        ));
        let debug = format!("{store:?}");
        assert!(debug.contains("prod"));
        assert!(!debug.contains("sk-test"));
    }

    #[test]
    fn reference_rejects_empty_and_nul() {
        assert_eq!(
            CredentialReference::try_new("").err(),
            Some(CredentialRejected::Empty)
        );
        assert_eq!(
            CredentialReference::try_new("pro\0d").err(),
            Some(CredentialRejected::ContainsNul)
        );
    }
}
