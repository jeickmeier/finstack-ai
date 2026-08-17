//! Process payload family: handshake kinds only (ADR-021 / PRD §18).
//!
//! No process session-command enum is defined in this crate.

use serde::{Deserialize, Serialize};

use crate::wire::VersionOffer;

/// Process-family pre-auth handshake. Kind names are distinct from remote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessPreAuth {
    /// Process client hello.
    ProcessClientHello {
        /// Version offer.
        offer: VersionOffer,
    },
    /// Process server hello.
    ProcessServerHello {
        /// Selected protocol version.
        selected_version: u16,
        /// Server offer.
        offer: VersionOffer,
    },
    /// Process authenticate.
    ProcessAuthenticate {
        /// Opaque method label. No remote session command fields.
        method: String,
    },
    /// Process auth result.
    ProcessAuthResult {
        /// Whether authentication succeeded.
        accepted: bool,
        /// Stable reason when rejected.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason_code: Option<String>,
    },
    /// Process close.
    ProcessClose {
        /// Stable close reason.
        reason_code: String,
    },
}

#[cfg(test)]
mod tests {
    use super::ProcessPreAuth;
    use crate::remote::{RemotePostAuth, decode_remote_post_auth};
    use crate::wire::{PayloadFamily, VersionOffer, decode_envelope, encode_envelope};
    use crate::{decode, encode};

    #[test]
    fn process_hello_is_not_a_remote_session_command() {
        let offer = VersionOffer::try_new(vec![1], 1, vec!["auth".into()]).expect("offer");
        let hello = ProcessPreAuth::ProcessClientHello { offer };
        let body = encode(&hello).expect("body");
        assert!(decode_remote_post_auth(&body).is_err());
        assert!(decode::<RemotePostAuth>(&body).is_err());
        let env = encode_envelope(PayloadFamily::Process, 1, &hello).expect("env");
        assert!(decode_envelope::<RemotePostAuth>(&env, PayloadFamily::Remote).is_err());
        let decoded = decode_envelope::<ProcessPreAuth>(&env, PayloadFamily::Process).expect("ok");
        assert_eq!(decoded.body(), &hello);
    }

    #[test]
    fn remote_open_session_is_not_a_process_hello() {
        let open = RemotePostAuth::OpenSession {
            last_known_durable_sequence: Some(3),
            locator: crate::remote::RemoteLocator::new("sess", None, None),
            tenant_scope: "tenant-a".into(),
        };
        let body = encode(&open).expect("body");
        assert!(decode::<ProcessPreAuth>(&body).is_err());
    }
}
