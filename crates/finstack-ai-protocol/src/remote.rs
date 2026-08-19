//! Remote session vocabulary. Distinct from process and WIT enums (ADR-014).

use finstack_ai_kernel::Digest;
use serde::{Deserialize, Serialize};

use crate::encode;
use crate::error::ProtocolError;
use crate::wire::VersionOffer;

/// Domain for authenticated remote-command digests.
const DOMAIN_REMOTE_COMMAND: &str = "remote-command";
/// Schema version embedded in the remote-command digest domain.
const REMOTE_COMMAND_DIGEST_SCHEMA_VERSION: u32 = 1;

/// Bounded pre-authentication remote messages (TDD §28.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemotePreAuth {
    /// Client hello / version offer.
    ClientHello {
        /// Supported versions, downgrade floor, and features.
        offer: VersionOffer,
    },
    /// Server hello / selected version.
    ServerHello {
        /// Selected protocol version.
        selected_version: u16,
        /// Server offer used for the selection.
        offer: VersionOffer,
    },
    /// Authentication request. Bearer tokens are transport-restricted by the server.
    Authenticate {
        /// Authentication method.
        method: RemoteAuthMethod,
    },
    /// Authentication result.
    AuthResult {
        /// Whether authentication succeeded.
        accepted: bool,
        /// Stable reason when rejected.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason_code: Option<String>,
    },
    /// Close the connection.
    Close {
        /// Stable close reason.
        reason_code: String,
    },
}

/// Authentication method carried in [`RemotePreAuth::Authenticate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteAuthMethod {
    /// Bearer secret. Rejected on plaintext TCP.
    Bearer {
        /// Opaque credential. Never written to audit events.
        token: String,
    },
    /// Loopback-only method with no bearer secret.
    Loopback,
}

/// Post-authentication remote session messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemotePostAuth {
    /// Open or resume a session at an optional durable cursor.
    OpenSession {
        /// Optional last-known durable sequence.
        #[serde(skip_serializing_if = "Option::is_none")]
        last_known_durable_sequence: Option<u64>,
        /// Session/lane/run locator.
        locator: RemoteLocator,
        /// Authenticated tenant scope.
        tenant_scope: String,
    },
    /// Authoritative snapshot at sequence `S`.
    Snapshot {
        /// Snapshot sequence.
        sequence: u64,
        /// Public snapshot projection.
        snapshot: RemoteSnapshot,
    },
    /// Explicit absence of a snapshot.
    NoSnapshot {
        /// Cursor the tail will start after.
        sequence: u64,
    },
    /// Verified durable tail records `S+1..=B`.
    DurableTail {
        /// Inclusive start sequence.
        from_sequence: u64,
        /// Inclusive end sequence.
        to_sequence: u64,
        /// Public durable events.
        events: Vec<RemoteEventView>,
    },
    /// Sync barrier at durable sequence `B`. Live events may follow only after this.
    SyncBarrier {
        /// Barrier sequence.
        sequence: u64,
    },
    /// Public `RunEvent` projection batch.
    EventBatch {
        /// Whether these events are live (post-barrier) or durable.
        live: bool,
        /// Public events.
        events: Vec<RemoteEventView>,
    },
    /// State-changing command.
    Command {
        /// Command body.
        command: RemoteCommand,
    },
    /// Command receipt or failure.
    CommandResult {
        /// Result body.
        result: RemoteCommandResult,
    },
    /// Credit grant for flow control.
    Grant {
        /// Item credits.
        items: u32,
        /// Byte credits.
        bytes: u32,
    },
    /// Credit acknowledgement.
    Ack {
        /// Acknowledged durable or batch cursor.
        cursor: u64,
        /// Items consumed.
        items: u32,
        /// Bytes consumed.
        bytes: u32,
    },
    /// Close the session connection.
    Close {
        /// Stable close reason.
        reason_code: String,
    },
}

impl RemotePostAuth {
    /// Stable `snake_case` kind matching the serde tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::OpenSession { .. } => "open_session",
            Self::Snapshot { .. } => "snapshot",
            Self::NoSnapshot { .. } => "no_snapshot",
            Self::DurableTail { .. } => "durable_tail",
            Self::SyncBarrier { .. } => "sync_barrier",
            Self::EventBatch { .. } => "event_batch",
            Self::Command { .. } => "command",
            Self::CommandResult { .. } => "command_result",
            Self::Grant { .. } => "grant",
            Self::Ack { .. } => "ack",
            Self::Close { .. } => "close",
        }
    }
}

/// Public locator. Never a store-private handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_field_names,
    reason = "session_id/lane_id/run_id are the shared semantic field names"
)]
pub struct RemoteLocator {
    session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    lane_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
}

impl RemoteLocator {
    /// Construct a locator from public identifiers.
    #[must_use]
    pub fn new(
        session_id: impl Into<String>,
        lane_id: Option<String>,
        run_id: Option<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            lane_id,
            run_id,
        }
    }

    /// Public session identifier.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Public lane identifier when present.
    #[must_use]
    pub fn lane_id(&self) -> Option<&str> {
        self.lane_id.as_deref()
    }

    /// Public run identifier when present.
    #[must_use]
    pub fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }
}

/// Public snapshot projection. No journal page layout or rusqlite types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteSnapshot {
    session_id: String,
    sequence: u64,
}

impl RemoteSnapshot {
    /// Construct a public snapshot view.
    #[must_use]
    pub fn new(session_id: impl Into<String>, sequence: u64) -> Self {
        Self {
            session_id: session_id.into(),
            sequence,
        }
    }

    /// Public session identifier.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Authoritative durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
}

/// Public runtime-event projection used on the socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteEventView {
    event_id: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    durable_sequence: Option<u64>,
    transient_sequence: u64,
}

impl RemoteEventView {
    /// Construct a public event view.
    #[must_use]
    pub fn new(
        event_id: impl Into<String>,
        kind: impl Into<String>,
        durable_sequence: Option<u64>,
        transient_sequence: u64,
    ) -> Self {
        Self {
            event_id: event_id.into(),
            kind: kind.into(),
            durable_sequence,
            transient_sequence,
        }
    }

    /// Public event identity.
    #[must_use]
    pub fn event_id(&self) -> &str {
        &self.event_id
    }

    /// Public event kind name.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Durable sequence when the event is durable-derived.
    #[must_use]
    pub const fn durable_sequence(&self) -> Option<u64> {
        self.durable_sequence
    }

    /// Transient sequence.
    #[must_use]
    pub const fn transient_sequence(&self) -> u64 {
        self.transient_sequence
    }
}

/// Public SDK operations that may cross the remote boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteCommandOp {
    /// Start a run.
    Start,
    /// Cancel a run or interaction.
    Cancel,
    /// Resolve an interaction.
    Resolve,
    /// Complete an externally finished effect.
    Complete,
}

/// Authenticated remote command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteCommand {
    command_id: String,
    locator: RemoteLocator,
    tenant_scope: String,
    digest: Digest,
    op: RemoteCommandOp,
}

impl RemoteCommand {
    /// Construct a command and compute its digest from the normalized fields.
    ///
    /// # Errors
    ///
    /// Returns codec failures while hashing the normalized command.
    pub fn try_new(
        command_id: impl Into<String>,
        locator: RemoteLocator,
        tenant_scope: impl Into<String>,
        op: RemoteCommandOp,
    ) -> Result<Self, ProtocolError> {
        let command_id = command_id.into();
        let tenant_scope = tenant_scope.into();
        let digest = command_digest(&command_id, &locator, &tenant_scope, op)?;
        Ok(Self {
            command_id,
            locator,
            tenant_scope,
            digest,
            op,
        })
    }

    /// `UUIDv7` command identity.
    #[must_use]
    pub fn command_id(&self) -> &str {
        &self.command_id
    }

    /// Public locator.
    #[must_use]
    pub const fn locator(&self) -> &RemoteLocator {
        &self.locator
    }

    /// Authenticated tenant scope.
    #[must_use]
    pub fn tenant_scope(&self) -> &str {
        &self.tenant_scope
    }

    /// Normalized command digest.
    #[must_use]
    pub const fn digest(&self) -> Digest {
        self.digest
    }

    /// Public operation.
    #[must_use]
    pub const fn op(&self) -> RemoteCommandOp {
        self.op
    }
}

/// Command receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteCommandResult {
    command_id: String,
    digest: Digest,
    accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_code: Option<String>,
}

impl RemoteCommandResult {
    /// Construct a command result.
    #[must_use]
    pub fn new(
        command_id: impl Into<String>,
        digest: Digest,
        accepted: bool,
        reason_code: Option<String>,
    ) -> Self {
        Self {
            command_id: command_id.into(),
            digest,
            accepted,
            reason_code,
        }
    }

    /// Command identity.
    #[must_use]
    pub fn command_id(&self) -> &str {
        &self.command_id
    }

    /// Command digest echoed on the receipt.
    #[must_use]
    pub const fn digest(&self) -> Digest {
        self.digest
    }

    /// Whether the command was accepted or replayed.
    #[must_use]
    pub const fn accepted(&self) -> bool {
        self.accepted
    }

    /// Stable failure reason.
    #[must_use]
    pub fn reason_code(&self) -> Option<&str> {
        self.reason_code.as_deref()
    }
}

/// Domain-separated digest of the normalized command excluding the digest field.
///
/// # Errors
///
/// Returns codec or digest-domain failures.
fn command_digest(
    command_id: &str,
    locator: &RemoteLocator,
    tenant_scope: &str,
    op: RemoteCommandOp,
) -> Result<Digest, ProtocolError> {
    #[derive(Serialize)]
    struct View<'a> {
        command_id: &'a str,
        locator: &'a RemoteLocator,
        tenant_scope: &'a str,
        op: RemoteCommandOp,
    }
    let bytes = encode(&View {
        command_id,
        locator,
        tenant_scope,
        op,
    })?;
    Digest::domain_separated(
        DOMAIN_REMOTE_COMMAND,
        REMOTE_COMMAND_DIGEST_SCHEMA_VERSION,
        &bytes,
    )
    .map_err(|err| ProtocolError::codec(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        RemoteAuthMethod, RemoteCommand, RemoteCommandOp, RemoteLocator, RemotePostAuth,
        RemotePreAuth,
    };
    use crate::wire::{PayloadFamily, VersionOffer, encode_envelope};
    use crate::{decode, encode};

    #[test]
    fn unknown_post_auth_variant_is_rejected() {
        #[derive(serde::Serialize)]
        struct Unknown {
            kind: &'static str,
        }
        let bytes = encode(&Unknown { kind: "store_page" }).expect("encode");
        assert!(decode::<RemotePostAuth>(&bytes).is_err());
    }

    #[test]
    fn command_digest_is_stable() {
        let first = RemoteCommand::try_new(
            "0192e0f6-7c3a-7c11-8a4d-2b6e9c1d0a11",
            RemoteLocator::new("sess", None, None),
            "tenant-a",
            RemoteCommandOp::Start,
        )
        .expect("first");
        let second = RemoteCommand::try_new(
            "0192e0f6-7c3a-7c11-8a4d-2b6e9c1d0a11",
            RemoteLocator::new("sess", None, None),
            "tenant-a",
            RemoteCommandOp::Start,
        )
        .expect("second");
        assert_eq!(first.digest(), second.digest());
    }

    #[test]
    fn process_hello_body_is_not_a_remote_command() {
        let offer = VersionOffer::try_new(vec![1], 1, vec!["auth".into()]).expect("offer");
        let process = crate::process::ProcessPreAuth::ProcessClientHello { offer };
        let body = encode(&process).expect("body");
        assert!(decode::<RemotePostAuth>(&body).is_err());
        assert!(decode::<RemotePreAuth>(&body).is_err());
        let _ = RemoteAuthMethod::Loopback;
        let _ = encode_envelope(PayloadFamily::Process, 1, &process).expect("env");
    }

    #[test]
    fn post_auth_kind_matches_serde_tag() {
        assert_eq!(
            RemotePostAuth::NoSnapshot { sequence: 0 }.kind(),
            "no_snapshot"
        );
        assert_eq!(
            RemotePostAuth::Close {
                reason_code: "x".into()
            }
            .kind(),
            "close"
        );
    }
}
