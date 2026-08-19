//! Family-tagged envelope and version negotiation shared by remote and process.
//!
//! Handshake *code* is generic. Remote and process keep distinct message enums.

use serde::{Deserialize, Serialize};

use crate::error::ProtocolError;
use crate::{decode, encode};

/// Current negotiated protocol version for both families.
pub const PROTOCOL_VERSION_V1: u16 = 1;

/// Envelope payload-schema family (TDD §28.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadFamily {
    /// Remote client/session family.
    Remote,
    /// External-process family. Shares framing only.
    Process,
}

/// Canonical-CBOR envelope around a family-specific body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolEnvelope<T> {
    payload_family: PayloadFamily,
    protocol_version: u16,
    body: T,
}

impl<T> ProtocolEnvelope<T> {
    /// Construct an envelope for `family` at `protocol_version`.
    #[must_use]
    pub const fn new(payload_family: PayloadFamily, protocol_version: u16, body: T) -> Self {
        Self {
            payload_family,
            protocol_version,
            body,
        }
    }

    /// Envelope family.
    #[must_use]
    pub const fn payload_family(&self) -> PayloadFamily {
        self.payload_family
    }

    /// Negotiated or offered protocol version.
    #[must_use]
    pub const fn protocol_version(&self) -> u16 {
        self.protocol_version
    }

    /// Family-specific body.
    #[must_use]
    pub const fn body(&self) -> &T {
        &self.body
    }

    /// Consume the envelope and return the body.
    #[must_use]
    pub fn into_body(self) -> T {
        self.body
    }
}

/// Encode a family-tagged envelope to canonical-CBOR.
///
/// # Errors
///
/// Returns codec or limit failures.
pub fn encode_envelope<T: Serialize>(
    family: PayloadFamily,
    protocol_version: u16,
    body: &T,
) -> Result<Vec<u8>, ProtocolError> {
    encode(&ProtocolEnvelope::new(family, protocol_version, body))
}

/// Decode a family-tagged envelope and require `expected` family.
///
/// # Errors
///
/// Unknown families fail at typed decode. A mismatched known family returns
/// [`ProtocolError::InvalidCbor`] with `unknown_payload_family` so the
/// connection can close before session acquisition.
pub fn decode_envelope<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    expected: PayloadFamily,
) -> Result<ProtocolEnvelope<T>, ProtocolError> {
    let envelope: ProtocolEnvelope<T> = decode(bytes)?;
    if envelope.payload_family() != expected {
        return Err(ProtocolError::invalid("unknown_payload_family"));
    }
    Ok(envelope)
}

/// Version offer used by hello messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionOffer {
    supported_versions: Vec<u16>,
    downgrade_floor: u16,
    features: Vec<String>,
}

impl VersionOffer {
    /// Construct a version offer.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::InvalidCbor`] when the offer is empty or the
    /// floor is above every supported version.
    pub fn try_new(
        supported_versions: Vec<u16>,
        downgrade_floor: u16,
        features: Vec<String>,
    ) -> Result<Self, ProtocolError> {
        if supported_versions.is_empty() {
            return Err(ProtocolError::invalid("empty_version_offer"));
        }
        if !supported_versions
            .iter()
            .any(|version| *version >= downgrade_floor)
        {
            return Err(ProtocolError::invalid("version_below_floor"));
        }
        Ok(Self {
            supported_versions,
            downgrade_floor,
            features,
        })
    }

    /// Supported protocol versions, highest preferred.
    #[must_use]
    pub fn supported_versions(&self) -> &[u16] {
        &self.supported_versions
    }

    /// Configured minimum / downgrade floor.
    #[must_use]
    pub const fn downgrade_floor(&self) -> u16 {
        self.downgrade_floor
    }

    /// Advertised security or capability features.
    #[must_use]
    pub fn features(&self) -> &[String] {
        &self.features
    }
}

/// Select the highest mutually supported version at or above both floors.
///
/// # Errors
///
/// Unknown or below-floor intersections fail closed with no fallback.
///
/// # Examples
///
/// ```
/// use finstack_ai_protocol::{VersionOffer, select_version};
///
/// let client = VersionOffer::try_new(vec![1], 1, vec!["auth".into()]).expect("client");
/// let server = VersionOffer::try_new(vec![1], 1, vec!["auth".into()]).expect("server");
/// assert_eq!(select_version(&client, &server).expect("v1"), 1);
/// ```
pub fn select_version(client: &VersionOffer, server: &VersionOffer) -> Result<u16, ProtocolError> {
    let mut chosen = None;
    for version in &client.supported_versions {
        if *version < client.downgrade_floor || *version < server.downgrade_floor {
            continue;
        }
        if server.supported_versions.contains(version) {
            chosen = Some(chosen.map_or(*version, |current: u16| current.max(*version)));
        }
    }
    chosen.ok_or_else(|| ProtocolError::invalid("unknown_protocol_version"))
}

/// Require every mandatory server feature to be present on the client offer.
///
/// # Errors
///
/// Missing features fail closed with no fallback.
pub fn require_features(offered: &VersionOffer, mandatory: &[&str]) -> Result<(), ProtocolError> {
    for feature in mandatory {
        if !offered.features.iter().any(|item| item == feature) {
            return Err(ProtocolError::invalid("missing_mandatory_feature"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        PayloadFamily, VersionOffer, decode_envelope, encode_envelope, require_features,
        select_version,
    };
    use crate::ProtocolError;

    #[test]
    fn unknown_version_fails_before_session() {
        let client = VersionOffer::try_new(vec![99], 99, vec!["auth".into()]).expect("client");
        let server = VersionOffer::try_new(vec![1], 1, vec!["auth".into()]).expect("server");
        assert!(matches!(
            select_version(&client, &server),
            Err(ProtocolError::InvalidCbor {
                reason_code: "unknown_protocol_version"
            })
        ));
    }

    #[test]
    fn missing_mandatory_feature_fails_closed() {
        let client = VersionOffer::try_new(vec![1], 1, vec!["other".into()]).expect("client");
        assert!(matches!(
            require_features(&client, &["auth"]),
            Err(ProtocolError::InvalidCbor {
                reason_code: "missing_mandatory_feature"
            })
        ));
    }

    #[test]
    fn unknown_family_string_fails_decode() {
        let bytes = encode_envelope(PayloadFamily::Process, 1, &"hello").expect("encode");
        let err = decode_envelope::<String>(&bytes, PayloadFamily::Remote).expect_err("family");
        assert_eq!(err.code(), "unknown_payload_family");
    }
}
