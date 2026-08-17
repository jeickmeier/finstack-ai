//! Shared length-prefixed framing and family-tagged handshake envelopes.

mod frame;
mod handshake;

pub use frame::{
    FRAME_LENGTH_BYTES, POST_AUTH_FRAME_MAX_BYTES, PRE_AUTH_FRAME_MAX_BYTES, decode_frame,
    decode_frame_len, encode_frame,
};
pub use handshake::{
    PROTOCOL_VERSION_V1, PayloadFamily, ProtocolEnvelope, VersionOffer, decode_envelope,
    encode_envelope, require_features, select_version,
};
