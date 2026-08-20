use std::sync::Arc;
use std::time::Duration;

use finstack_ai_runtime::{StoreError, StoreLimits};

/// Default schema (namespace) used to hold the store's tables.
pub const DEFAULT_SCHEMA: &str = "finstack_ai";

/// Default number of pooled connections.
pub const DEFAULT_POOL_SIZE: usize = 8;

/// Default connect timeout applied when establishing a pooled connection.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum length of a Postgres identifier accepted for [`PostgresStoreConfig::schema`].
const MAX_SCHEMA_LEN: usize = 63;

/// Durability policy applied to every pooled connection and advertised by
/// [`finstack_ai_runtime::JournalStore::health`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostgresDurability {
    /// `SET synchronous_commit = on` on every pooled connection.
    ///
    /// This is the only mode that may set `health().durable = true`. It still
    /// depends on the server and storage honoring `synchronous_commit`.
    Durable,
    /// `SET synchronous_commit = off`. Must never advertise NFR-REL-001.
    Relaxed,
}

/// Governs whether this store instance may issue schema-management DDL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaPolicy {
    /// Apply migrations under an advisory lock when the schema is absent or stale.
    Manage,
    /// Never issue DDL; fail closed if the schema is missing or unmigrated.
    Require,
}

/// Open configuration for the postgres journal store.
#[derive(Debug, Clone)]
pub struct PostgresStoreConfig {
    /// `postgres://` connection string.
    pub url: Arc<str>,
    /// Schema (namespace) holding the store's tables. Validated, never
    /// parameterized — it is interpolated directly into DDL/DML text.
    pub schema: Arc<str>,
    /// Durability / health labeling.
    pub durability: PostgresDurability,
    /// Resource ceilings.
    pub limits: StoreLimits,
    /// Maximum number of pooled connections.
    pub pool_size: usize,
    /// Schema-management policy.
    pub schema_policy: SchemaPolicy,
    /// Timeout applied when establishing a new pooled connection (and, in
    /// [`finstack_ai_runtime::JournalStore::health`], to the checkout and
    /// `SELECT 1` probe).
    ///
    /// Must be non-zero: a zero timeout would expire before the connect
    /// future is ever polled, making every open fail as
    /// `Unavailable{postgres_unavailable}` and `health()` report `ready:
    /// false` forever. Rejected by [`PostgresStoreConfig::validate`] with
    /// `zero_postgres_connect_timeout`, mirroring sqlite's
    /// `zero_sqlite_busy_timeout`.
    pub connect_timeout: Duration,
}

impl PostgresStoreConfig {
    /// Construct a config with the documented defaults: schema
    /// [`DEFAULT_SCHEMA`], [`PostgresDurability::Durable`], pool size
    /// [`DEFAULT_POOL_SIZE`], [`SchemaPolicy::Manage`], connect timeout
    /// [`DEFAULT_CONNECT_TIMEOUT`].
    #[must_use]
    pub fn new(url: impl Into<Arc<str>>, limits: StoreLimits) -> Self {
        Self {
            url: url.into(),
            schema: Arc::from(DEFAULT_SCHEMA),
            durability: PostgresDurability::Durable,
            limits,
            pool_size: DEFAULT_POOL_SIZE,
            schema_policy: SchemaPolicy::Manage,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }
    }

    /// Validate the configuration.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidRequest`] when `pool_size` is zero,
    /// `connect_timeout` is zero, any [`StoreLimits`] field is zero, or
    /// `schema` is not a valid lowercase Postgres identifier.
    pub fn validate(&self) -> Result<(), StoreError> {
        if self.pool_size == 0 {
            return Err(StoreError::InvalidRequest {
                reason_code: "zero_pool_size",
            });
        }
        // Twin of sqlite's `zero_sqlite_busy_timeout` guard
        // (`extensions/stores/finstack-ai-store-sqlite/src/store.rs`): a zero
        // timeout is never a caller's intent, and it would silently turn
        // every connect into `Unavailable`.
        if self.connect_timeout.is_zero() {
            return Err(StoreError::InvalidRequest {
                reason_code: "zero_postgres_connect_timeout",
            });
        }
        self.limits.validate()?;
        validate_schema_name(&self.schema)?;
        Ok(())
    }
}

/// Validate a schema identifier: `[a-z_][a-z0-9_]{0,62}`.
///
/// The schema name is interpolated into DDL/DML text (Postgres does not
/// support parameterizing identifiers), so it must be validated up front
/// rather than escaped at use.
fn validate_schema_name(schema: &str) -> Result<(), StoreError> {
    let invalid = || StoreError::InvalidRequest {
        reason_code: "invalid_schema_name",
    };

    if schema.is_empty() || schema.len() > MAX_SCHEMA_LEN {
        return Err(invalid());
    }

    let mut chars = schema.chars();
    let Some(first) = chars.next() else {
        return Err(invalid());
    };
    if !(first.is_ascii_lowercase() || first == '_') {
        return Err(invalid());
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return Err(invalid());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> StoreLimits {
        StoreLimits {
            sessions: 1,
            batches_per_session: 1,
            records_per_session: 1,
            snapshot_bytes: 1,
        }
    }

    fn config() -> PostgresStoreConfig {
        PostgresStoreConfig::new("postgres://localhost/db", limits())
    }

    #[test]
    fn defaults_are_documented() {
        let config = config();
        assert_eq!(&*config.schema, DEFAULT_SCHEMA);
        assert_eq!(config.durability, PostgresDurability::Durable);
        assert_eq!(config.pool_size, DEFAULT_POOL_SIZE);
        assert_eq!(config.schema_policy, SchemaPolicy::Manage);
        assert_eq!(config.connect_timeout, DEFAULT_CONNECT_TIMEOUT);
    }

    #[test]
    fn zero_pool_size_is_rejected() {
        let mut config = config();
        config.pool_size = 0;
        let error = config.validate().unwrap_err();
        assert!(matches!(
            error,
            StoreError::InvalidRequest {
                reason_code: "zero_pool_size"
            }
        ));
    }

    #[test]
    fn zero_connect_timeout_is_rejected() {
        let mut config = config();
        config.connect_timeout = Duration::ZERO;
        let error = config.validate().unwrap_err();
        assert!(matches!(
            error,
            StoreError::InvalidRequest {
                reason_code: "zero_postgres_connect_timeout"
            }
        ));
    }

    #[test]
    fn zero_limit_is_rejected() {
        let mut config = config();
        config.limits.sessions = 0;
        let error = config.validate().unwrap_err();
        assert!(matches!(
            error,
            StoreError::InvalidRequest {
                reason_code: "zero_store_limit"
            }
        ));
    }

    #[test]
    fn valid_schema_names_are_accepted() {
        for name in ["finstack_ai", "_leading_underscore", "a", "fa_test_1"] {
            assert!(validate_schema_name(name).is_ok(), "{name} should be valid");
        }
    }

    #[test]
    fn invalid_schema_names_are_rejected() {
        for name in ["", "Finstack", "1leading_digit", "has-dash", "has space"] {
            let error = validate_schema_name(name).unwrap_err();
            assert!(
                matches!(
                    error,
                    StoreError::InvalidRequest {
                        reason_code: "invalid_schema_name"
                    }
                ),
                "{name} should be rejected"
            );
        }
    }

    #[test]
    fn schema_name_over_max_length_is_rejected() {
        let name = "a".repeat(MAX_SCHEMA_LEN + 1);
        let error = validate_schema_name(&name).unwrap_err();
        assert!(matches!(
            error,
            StoreError::InvalidRequest {
                reason_code: "invalid_schema_name"
            }
        ));
    }

    #[test]
    fn schema_name_at_max_length_is_accepted() {
        let name = "a".repeat(MAX_SCHEMA_LEN);
        assert!(validate_schema_name(&name).is_ok());
    }
}
