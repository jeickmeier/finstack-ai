#![no_main]

use finstack_ai_protocol::{
    POST_AUTH_FRAME_MAX_BYTES, PRE_AUTH_FRAME_MAX_BYTES, PayloadFamily, ProcessPreAuth,
    RemotePostAuth, RemotePreAuth, decode_envelope, decode_frame, decode_frame_len,
    decode_remote_post_auth, decode_remote_pre_auth, select_version,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() >= 4 {
        let header: [u8; 4] = data[..4].try_into().expect("header");
        if let Ok(declared) = decode_frame_len(header, PRE_AUTH_FRAME_MAX_BYTES) {
            assert!(
                declared <= PRE_AUTH_FRAME_MAX_BYTES,
                "pre-auth length check allocated past the ceiling"
            );
        }
        if let Ok(declared) = decode_frame_len(header, POST_AUTH_FRAME_MAX_BYTES) {
            assert!(
                declared <= POST_AUTH_FRAME_MAX_BYTES,
                "post-auth length check allocated past the ceiling"
            );
        }
    }

    let Ok(payload) = decode_frame(data, PRE_AUTH_FRAME_MAX_BYTES)
        .or_else(|_| decode_frame(data, POST_AUTH_FRAME_MAX_BYTES))
    else {
        return;
    };

    let remote = decode_envelope::<RemotePreAuth>(payload, PayloadFamily::Remote);
    let process = decode_envelope::<ProcessPreAuth>(payload, PayloadFamily::Process);
    if remote.is_ok() && process.is_ok() {
        panic!("remote and process families accepted the same envelope");
    }
    if let (Ok(remote), Ok(_)) = (
        remote.as_ref(),
        decode_envelope::<RemotePreAuth>(payload, PayloadFamily::Process),
    ) {
        panic!(
            "process family accepted a remote envelope {:?}",
            remote.payload_family()
        );
    }

    let _ = decode_remote_pre_auth(payload);
    let _ = decode_remote_post_auth(payload);
    if let Ok(post) = decode_envelope::<RemotePostAuth>(payload, PayloadFamily::Remote) {
        assert_eq!(post.payload_family(), PayloadFamily::Remote);
        if post.protocol_version() == 0 {
            panic!("protocol version 0 is a downgrade and must not decode as v1");
        }
    }
    if let (Ok(client), Ok(server)) = (
        decode_envelope::<RemotePreAuth>(payload, PayloadFamily::Remote),
        decode_envelope::<RemotePreAuth>(payload, PayloadFamily::Remote),
    ) && let (
        RemotePreAuth::ClientHello { offer: client_offer },
        RemotePreAuth::ServerHello {
            offer: server_offer,
            selected_version,
        },
    ) = (client.body(), server.body())
    {
        if let Ok(chosen) = select_version(client_offer, server_offer) {
            assert!(
                chosen >= client_offer.downgrade_floor()
                    && chosen >= server_offer.downgrade_floor(),
                "select_version returned a version below a downgrade floor"
            );
            let _ = selected_version;
        }
    }
});
