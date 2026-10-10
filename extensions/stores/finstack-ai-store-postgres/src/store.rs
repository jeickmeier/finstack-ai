//! Store handle: [`PostgresJournalStore::try_open`] and the shared
//! per-connection setup every pooled connection runs.
//!
//! ## TLS
//!
//! TLS is required by default. The store verifies server hostnames against
//! the bundled `WebPKI` roots plus any caller-provided PEM trust anchors.
//! Plaintext transport is available only through the explicit
//! [`PostgresTlsMode::Disable`] setting.

use std::str::FromStr;
use std::sync::Arc;

use finstack_ai_runtime::ports::journal::StoreError;
use finstack_ai_store_common::VerifiedHeadCache;
use rustls::pki_types::{CertificateDer, pem::PemObject};

use crate::config::{PostgresDurability, PostgresStoreConfig, PostgresTlsMode};
use crate::error::map_postgres_error;
use crate::pool::Pool;
use crate::schema::ensure_schema;

/// Human-readable, stable [`finstack_ai_runtime::ports::journal::StoreHealth::detail`] text
/// for each durability mode (spec D6).
pub(crate) const DURABLE_DETAIL: &str = "postgres synchronous_commit=on";
/// See [`DURABLE_DETAIL`].
pub(crate) const RELAXED_DETAIL: &str = "postgres synchronous_commit=off";

/// Durable, multi-writer `PostgreSQL` [`finstack_ai_runtime::ports::journal::JournalStore`].
///
/// Construct with [`PostgresJournalStore::try_open`]. `Self` is not itself
/// cheaply `Clone` (its config is, but the store owns the pool outright);
/// hand it out behind an `Arc<PostgresJournalStore>` at the application
/// boundary, the same way the sqlite store is shared.
pub struct PostgresJournalStore {
    pub(crate) pool: Pool<tokio_postgres::Client>,
    pub(crate) config: PostgresStoreConfig,
    /// Process-local chain-verification cache (spec D9), keyed by session.
    ///
    /// `Arc` because every port method returns a `'static` future that must
    /// own what it touches; the container's `std::sync::Mutex` (never held
    /// across an `.await`) guards operations that are pure memory work no
    /// async runtime needs to see. Its generation guard is what makes a
    /// concurrent read/write-back pair safe — see
    /// [`finstack_ai_store_common::VerifiedHeadCache`].
    pub(crate) verified: Arc<VerifiedHeadCache>,
}

impl PostgresJournalStore {
    /// Open (and, under [`crate::SchemaPolicy::Manage`], migrate) a
    /// `PostgreSQL` journal store.
    ///
    /// Validates `config`, opens one connection (bounded by
    /// `config.connect_timeout`), runs `ensure_schema` over it, then seeds
    /// the connection pool with that same connection so no extra round trip
    /// is spent opening and discarding a throwaway one.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidRequest`] for an invalid config or an
    /// unparseable connection URL / TLS trust anchor.
    /// Returns [`StoreError::Unavailable`] if the initial connection cannot
    /// be established within `config.connect_timeout`, or the mapped error
    /// from `ensure_schema` otherwise.
    pub async fn try_open(config: PostgresStoreConfig) -> Result<Self, StoreError> {
        config.validate()?;
        // The TLS client configuration (root store, protocol versions) is
        // built once here and cloned per connection: `MakeRustlsConnect`
        // wraps an `Arc<ClientConfig>`, so each pooled connection shares it
        // instead of re-parsing the trust anchors.
        let transport = Transport::from_config(&config)?;
        let client = connect_and_prepare(&config, &transport).await?;
        tokio::time::timeout(
            config.operation_timeout,
            ensure_schema(&client, &config.schema, config.schema_policy),
        )
        .await
        .map_err(|_| StoreError::Unavailable {
            reason_code: "postgres_operation_timeout",
        })??;

        let pool_config = config.clone();
        let pool = Pool::new(
            config.pool_size,
            Box::new(move || {
                let pool_config = pool_config.clone();
                let transport = transport.clone();
                Box::pin(async move { connect_and_prepare(&pool_config, &transport).await })
            }),
            // A connection can die while idle in the pool (server restart,
            // network drop) with nothing else noticing; the pool checks
            // this on every idle connection it pops before handing it out
            // (see the finding fixed in `src/pool.rs::Pool::get`).
            Box::new(|client: &tokio_postgres::Client| !client.is_closed()),
        );
        pool.seed(client);

        Ok(Self {
            pool,
            config,
            verified: Arc::new(VerifiedHeadCache::new()),
        })
    }
}

/// The wire transport every pooled connection is opened with, resolved
/// once per store from [`PostgresStoreConfig::tls_mode`].
#[derive(Clone)]
pub(crate) enum Transport {
    /// [`PostgresTlsMode::Disable`]: plaintext.
    Plain,
    /// [`PostgresTlsMode::Require`]: rustls over the shared client config.
    Rustls(tokio_postgres_rustls::MakeRustlsConnect),
}

impl Transport {
    /// Build the transport, validating any caller-provided trust anchors.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidRequest`] when the configured CA PEM is
    /// unparseable (`postgres_tls_ca_invalid`) or no TLS configuration can
    /// be assembled (`postgres_tls_config_invalid`).
    pub(crate) fn from_config(config: &PostgresStoreConfig) -> Result<Self, StoreError> {
        match config.tls_mode {
            PostgresTlsMode::Disable => Ok(Self::Plain),
            PostgresTlsMode::Require => postgres_tls(config).map(Self::Rustls),
        }
    }
}

/// Open one physical connection, apply its session-scoped setup (spec D6),
/// and spawn its driving task.
///
/// # Errors
///
/// Returns [`StoreError::Unavailable`] if the connection cannot be
/// established within `config.connect_timeout`, or a mapped
/// [`StoreError`] if the connection succeeds but the session-setup
/// statements fail.
pub(crate) async fn connect_and_prepare(
    config: &PostgresStoreConfig,
    transport: &Transport,
) -> Result<tokio_postgres::Client, StoreError> {
    let mut postgres =
        tokio_postgres::Config::from_str(&config.url).map_err(|_| StoreError::InvalidRequest {
            reason_code: "postgres_url_invalid",
        })?;
    let client = match transport {
        Transport::Plain => {
            postgres.ssl_mode(tokio_postgres::config::SslMode::Disable);
            connect(&postgres, tokio_postgres::NoTls, config.connect_timeout).await?
        }
        Transport::Rustls(tls) => {
            postgres.ssl_mode(tokio_postgres::config::SslMode::Require);
            connect(&postgres, tls.clone(), config.connect_timeout).await?
        }
    };

    let setup = async {
        let synchronous_commit = match config.durability {
            PostgresDurability::Durable => "on",
            PostgresDurability::Relaxed => "off",
        };
        let statement_timeout = config.operation_timeout.as_millis().max(1);
        client
            .batch_execute(&format!(
                "SET synchronous_commit = {synchronous_commit}; SET statement_timeout = {statement_timeout}"
            ))
            .await
            .map_err(|error| map_postgres_error(&error))?;
        client
            .batch_execute(&format!("SET search_path = {}", config.schema))
            .await
            .map_err(|error| map_postgres_error(&error))?;
        Ok::<(), StoreError>(())
    };
    tokio::time::timeout(config.operation_timeout, setup)
        .await
        .map_err(|_| StoreError::Unavailable {
            reason_code: "postgres_operation_timeout",
        })??;

    Ok(client)
}

/// Open the connection within `timeout` and spawn its driving task.
async fn connect<T>(
    postgres: &tokio_postgres::Config,
    tls: T,
    timeout: std::time::Duration,
) -> Result<tokio_postgres::Client, StoreError>
where
    T: tokio_postgres::tls::MakeTlsConnect<tokio_postgres::Socket>,
    T::Stream: Send + 'static,
{
    let (client, connection) = tokio::time::timeout(timeout, postgres.connect(tls))
        .await
        .map_err(|_| StoreError::Unavailable {
            reason_code: "postgres_unavailable",
        })?
        .map_err(|error| map_postgres_error(&error))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}

fn postgres_tls(
    config: &PostgresStoreConfig,
) -> Result<tokio_postgres_rustls::MakeRustlsConnect, StoreError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(pem) = &config.tls_ca_pem {
        let mut certificate_count = 0_usize;
        for certificate in CertificateDer::pem_slice_iter(pem.as_ref()) {
            let certificate = certificate.map_err(|_| StoreError::InvalidRequest {
                reason_code: "postgres_tls_ca_invalid",
            })?;
            certificate_count += 1;
            roots
                .add(certificate)
                .map_err(|_| StoreError::InvalidRequest {
                    reason_code: "postgres_tls_ca_invalid",
                })?;
        }
        if certificate_count == 0 {
            return Err(StoreError::InvalidRequest {
                reason_code: "postgres_tls_ca_invalid",
            });
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let client = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| StoreError::InvalidRequest {
            reason_code: "postgres_tls_config_invalid",
        })?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(client))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_is_built_once_from_the_configured_mode() {
        let mut config = PostgresStoreConfig::new(
            "postgres://localhost/db",
            finstack_ai_runtime::ports::journal::StoreLimits {
                sessions: 1,
                batches_per_session: 1,
                records_per_session: 1,
                snapshot_bytes: 1,
            },
        );
        assert!(matches!(
            Transport::from_config(&config),
            Ok(Transport::Rustls(_))
        ));
        config.tls_mode = PostgresTlsMode::Disable;
        assert!(matches!(
            Transport::from_config(&config),
            Ok(Transport::Plain)
        ));
        config.tls_mode = PostgresTlsMode::Require;
        config.tls_ca_pem = Some(Arc::from(b"not a certificate".as_slice()));
        assert!(matches!(
            Transport::from_config(&config),
            Err(StoreError::InvalidRequest {
                reason_code: "postgres_tls_ca_invalid"
            })
        ));
    }

    #[test]
    fn tls_builder_accepts_public_roots() {
        let config = PostgresStoreConfig::new(
            "postgres://localhost/db",
            finstack_ai_runtime::ports::journal::StoreLimits {
                sessions: 1,
                batches_per_session: 1,
                records_per_session: 1,
                snapshot_bytes: 1,
            },
        );
        assert!(postgres_tls(&config).is_ok());
    }
}
