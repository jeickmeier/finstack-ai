//! Signed opaque callback tokens binding one deferred effect to one locator.

use core::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use finstack_ai_kernel::{
    AuthorizationEvidence, EffectId, OperationLocator, PrincipalRef, Timestamp,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::config::ResolvedKeys;

type HmacSha256 = Hmac<Sha256>;

/// Maximum accepted token length in bytes.
///
/// Bounds what [`crate::CompletionIngress::deliver`] accepts as a token; hosts
/// should cap transport payloads to align with this limit.
pub const MAX_TOKEN_BYTES: usize = 4_096;
const TOKEN_PREFIX: &str = "fcit1.";
/// Current claims schema version.
pub(crate) const CLAIMS_VERSION: u32 = 1;
/// Claims `kind` for a deferred external effect completion.
pub(crate) const KIND_EFFECT_COMPLETION: &str = "effect_completion";

/// Opaque signed callback token. Treat as a bearer capability.
#[derive(Clone, PartialEq, Eq)]
pub struct CallbackToken(String);

impl CallbackToken {
    /// Borrow the encoded token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CallbackToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CallbackToken([REDACTED])")
    }
}

/// Signed claims binding one deferred effect to one authenticated locator.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Claims {
    /// Claims schema version.
    pub(crate) v: u32,
    /// Id of the key that signed this token.
    pub(crate) kid: String,
    /// Claims kind discriminator (currently only [`KIND_EFFECT_COMPLETION`]).
    pub(crate) kind: String,
    /// Durable operation locator this token authorizes.
    pub(crate) locator: OperationLocator,
    /// Authenticated principal the token was minted for.
    pub(crate) principal: PrincipalRef,
    /// Authorization evidence backing the mint decision.
    pub(crate) authorization: AuthorizationEvidence,
    /// Target deferred effect.
    pub(crate) effect_id: EffectId,
    /// Expiry instant; verification rejects at or after this instant.
    pub(crate) expires_at: Timestamp,
}

/// Why [`mint_token`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MintErrorKind {
    /// Claims could not be canonically encoded, or no active key was available.
    Encoding,
}

/// Why [`verify_token`] failed. Drives the audit reason code; never shown to callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerifyFailure {
    /// Token shape, size, or claims structure was invalid.
    Malformed,
    /// The claimed `kid` is not among the accepted verification keys.
    UnknownKey,
    /// The HMAC tag did not verify against the claims.
    BadSignature,
    /// `submitted_at` is at or after `expires_at`.
    Expired,
}

fn mac_for(key: &str, message: &[u8]) -> Result<HmacSha256, ()> {
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).map_err(|_| ())?;
    mac.update(message);
    Ok(mac)
}

/// Read only the `kid` before authentication; full strict decode happens
/// after the signature is proven.
#[derive(Deserialize)]
struct KidOnly {
    kid: String,
    #[serde(flatten)]
    _rest: serde_json::Map<String, serde_json::Value>,
}

/// Mint a signed callback token for `claims`, signed with the active key.
///
/// # Errors
///
/// Returns [`MintErrorKind::Encoding`] when the claims cannot be canonically
/// encoded or no active signing key is available.
pub(crate) fn mint_token(
    keys: &ResolvedKeys,
    claims: &Claims,
) -> Result<CallbackToken, MintErrorKind> {
    let claims_bytes =
        serde_json_canonicalizer::to_vec(claims).map_err(|_| MintErrorKind::Encoding)?;
    let claims_b64 = URL_SAFE_NO_PAD.encode(claims_bytes);
    let message = format!("{TOKEN_PREFIX}{claims_b64}");
    let (_, key) = keys.keys.first().ok_or(MintErrorKind::Encoding)?;
    let mac = mac_for(key.expose(), message.as_bytes()).map_err(|()| MintErrorKind::Encoding)?;
    let tag = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(CallbackToken(format!("{message}.{tag}")))
}

/// Verify a callback token's shape, signature, key acceptance, and expiry.
///
/// # Errors
///
/// Returns the specific [`VerifyFailure`] reason; see variant docs.
pub(crate) fn verify_token(
    keys: &ResolvedKeys,
    token: &str,
    submitted_at: Timestamp,
) -> Result<Claims, VerifyFailure> {
    if token.len() > MAX_TOKEN_BYTES || !token.is_ascii() {
        return Err(VerifyFailure::Malformed);
    }
    let rest = token
        .strip_prefix(TOKEN_PREFIX)
        .ok_or(VerifyFailure::Malformed)?;
    let (claims_b64, tag_b64) = rest.split_once('.').ok_or(VerifyFailure::Malformed)?;
    if claims_b64.is_empty() || tag_b64.is_empty() {
        return Err(VerifyFailure::Malformed);
    }
    let claims_bytes = URL_SAFE_NO_PAD
        .decode(claims_b64)
        .map_err(|_| VerifyFailure::Malformed)?;
    let tag = URL_SAFE_NO_PAD
        .decode(tag_b64)
        .map_err(|_| VerifyFailure::Malformed)?;
    let kid_probe: KidOnly =
        serde_json::from_slice(&claims_bytes).map_err(|_| VerifyFailure::Malformed)?;
    let key = keys
        .keys
        .iter()
        .find(|(id, _)| id.as_ref() == kid_probe.kid)
        .map(|(_, key)| key)
        .ok_or(VerifyFailure::UnknownKey)?;
    let message = format!("{TOKEN_PREFIX}{claims_b64}");
    let mac =
        mac_for(key.expose(), message.as_bytes()).map_err(|()| VerifyFailure::BadSignature)?;
    mac.verify_slice(&tag)
        .map_err(|_| VerifyFailure::BadSignature)?;
    let claims: Claims =
        serde_json::from_slice(&claims_bytes).map_err(|_| VerifyFailure::Malformed)?;
    if claims.v != CLAIMS_VERSION
        || claims.kind != KIND_EFFECT_COMPLETION
        || claims.kid != kid_probe.kid
    {
        return Err(VerifyFailure::Malformed);
    }
    if submitted_at >= claims.expires_at {
        return Err(VerifyFailure::Expired);
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CompletionIngressConfig, validated_keys};
    use finstack_ai_kernel::{
        AuthorizationEvidence, EffectId, LaneId, OperationLocator, PrincipalRef, RunId, SessionId,
        Timestamp,
    };
    use finstack_ai_runtime::SecretString;

    fn keys() -> crate::config::ResolvedKeys {
        validated_keys(&CompletionIngressConfig {
            key_id: "k-active".to_owned(),
            key: SecretString::try_new("a".repeat(32)).expect("secret"),
            additional_verification_keys: vec![(
                "k-old".to_owned(),
                SecretString::try_new("b".repeat(32)).expect("secret"),
            )],
        })
        .expect("keys")
    }

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    fn claims(kid: &str, expires_at: Timestamp) -> Claims {
        Claims {
            v: CLAIMS_VERSION,
            kid: kid.to_owned(),
            kind: KIND_EFFECT_COMPLETION.to_owned(),
            locator: OperationLocator::try_new(
                "tenant-a",
                SessionId::parse("01234567-89ab-7cde-89ab-0123456789a1").expect("session"),
                LaneId::parse("01234567-89ab-7cde-89ab-0123456789a2").expect("lane"),
                RunId::parse("01234567-89ab-7cde-89ab-0123456789a3").expect("run"),
            )
            .expect("locator"),
            principal: PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                .expect("principal"),
            authorization: AuthorizationEvidence::try_new("policy-v1", "decision-v1")
                .expect("auth"),
            effect_id: EffectId::parse("01234567-89ab-7cde-89ab-0123456789a4").expect("effect"),
            expires_at,
        }
    }

    #[test]
    fn mint_then_verify_round_trips() {
        let keys = keys();
        let minted = mint_token(&keys, &claims("k-active", ts(10_000))).expect("mint");
        let verified = verify_token(&keys, minted.as_str(), ts(9_999)).expect("verify");
        assert_eq!(verified.kid, "k-active");
        assert_eq!(verified.effect_id, claims("k-active", ts(10_000)).effect_id);
    }

    #[test]
    fn mint_signs_with_active_key_not_rotation_key() {
        // `keys()` resolves an active key ("k-active") plus a rotation
        // verification key ("k-old"). Minting must sign with the active key:
        // the token verifies against a key set containing only the active
        // key, which would fail if the rotation key had signed instead.
        let keys = keys();
        let minted = mint_token(&keys, &claims("k-active", ts(10_000))).expect("mint");
        let active_only = validated_keys(&CompletionIngressConfig {
            key_id: "k-active".to_owned(),
            key: SecretString::try_new("a".repeat(32)).expect("secret"),
            additional_verification_keys: vec![],
        })
        .expect("active only");
        let verified = verify_token(&active_only, minted.as_str(), ts(9_999)).expect("verify");
        assert_eq!(verified.kid, "k-active");
    }

    #[test]
    fn expiry_boundary_rejects_at_and_after_expiry() {
        let keys = keys();
        let minted = mint_token(&keys, &claims("k-active", ts(10_000))).expect("mint");
        assert_eq!(
            verify_token(&keys, minted.as_str(), ts(10_000)).expect_err("at expiry"),
            VerifyFailure::Expired
        );
        assert_eq!(
            verify_token(&keys, minted.as_str(), ts(10_001)).expect_err("after expiry"),
            VerifyFailure::Expired
        );
    }

    #[test]
    fn tampered_claims_and_tag_fail_signature() {
        let keys = keys();
        let minted = mint_token(&keys, &claims("k-active", ts(10_000))).expect("mint");
        let token = minted.as_str();
        // Flip one character inside the claims segment.
        let mut chars: Vec<char> = token.chars().collect();
        let dot = token.rfind('.').expect("dot");
        chars[dot - 1] = if chars[dot - 1] == 'A' { 'B' } else { 'A' };
        let tampered_claims: String = chars.iter().collect();
        assert!(matches!(
            verify_token(&keys, &tampered_claims, ts(0)).expect_err("claims tamper"),
            VerifyFailure::BadSignature | VerifyFailure::Malformed
        ));
        // Flip the final tag character.
        let mut tag_chars: Vec<char> = token.chars().collect();
        let last = tag_chars.len() - 1;
        tag_chars[last] = if tag_chars[last] == 'A' { 'B' } else { 'A' };
        let tampered_tag: String = tag_chars.iter().collect();
        assert_eq!(
            verify_token(&keys, &tampered_tag, ts(0)).expect_err("tag tamper"),
            VerifyFailure::BadSignature
        );
    }

    #[test]
    fn shape_failures_are_malformed() {
        let keys = keys();
        for bad in [
            "",
            "fcit1.",
            "fcit1.onlyone",
            "not-a-token",
            "fcit2.AA.BB",
            "fcit1.!!.!!",
        ] {
            assert_eq!(
                verify_token(&keys, bad, ts(0)).expect_err("malformed"),
                VerifyFailure::Malformed,
                "input: {bad}"
            );
        }
        let oversize = format!("fcit1.{}.AA", "A".repeat(MAX_TOKEN_BYTES));
        assert_eq!(
            verify_token(&keys, &oversize, ts(0)).expect_err("oversize"),
            VerifyFailure::Malformed
        );
    }

    #[test]
    fn unknown_kid_rejects_and_rotation_key_verifies() {
        let keys = keys();
        let old_signed = {
            // Sign with the rotation key by making it active in a second key set.
            let alt = validated_keys(&CompletionIngressConfig {
                key_id: "k-old".to_owned(),
                key: SecretString::try_new("b".repeat(32)).expect("secret"),
                additional_verification_keys: vec![],
            })
            .expect("alt");
            mint_token(&alt, &claims("k-old", ts(10_000))).expect("mint")
        };
        assert!(verify_token(&keys, old_signed.as_str(), ts(0)).is_ok());
        let stranger = {
            let alt = validated_keys(&CompletionIngressConfig {
                key_id: "k-stranger".to_owned(),
                key: SecretString::try_new("c".repeat(32)).expect("secret"),
                additional_verification_keys: vec![],
            })
            .expect("alt");
            mint_token(&alt, &claims("k-stranger", ts(10_000))).expect("mint")
        };
        assert_eq!(
            verify_token(&keys, stranger.as_str(), ts(0)).expect_err("unknown kid"),
            VerifyFailure::UnknownKey
        );
    }

    #[test]
    fn wrong_kind_or_version_is_malformed() {
        let keys = keys();
        let mut wrong_kind = claims("k-active", ts(10_000));
        wrong_kind.kind = "interaction_resolution".to_owned();
        let minted = mint_token(&keys, &wrong_kind).expect("mint");
        assert_eq!(
            verify_token(&keys, minted.as_str(), ts(0)).expect_err("kind"),
            VerifyFailure::Malformed
        );
        let mut wrong_version = claims("k-active", ts(10_000));
        wrong_version.v = 2;
        let minted = mint_token(&keys, &wrong_version).expect("mint");
        assert_eq!(
            verify_token(&keys, minted.as_str(), ts(0)).expect_err("version"),
            VerifyFailure::Malformed
        );
    }
}
