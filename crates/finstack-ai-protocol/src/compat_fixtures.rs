//! Shared frame/handshake fixture checks parameterized by family.

use std::fs;
use std::path::PathBuf;

use crate::handshake::{
    PayloadFamily, VersionOffer, decode_envelope, encode_envelope, select_version,
};
use crate::process::ProcessPreAuth;
use crate::remote::{RemotePostAuth, RemotePreAuth};
use crate::{PRE_AUTH_FRAME_MAX_BYTES, decode, decode_frame_len, encode, encode_frame};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn remote_and_process_share_frame_limits() {
    for family in [PayloadFamily::Remote, PayloadFamily::Process] {
        let offer = VersionOffer::try_new(vec![1], 1, vec!["auth".into()]).expect("offer");
        let body = match family {
            PayloadFamily::Remote => encode(&RemotePreAuth::ClientHello {
                offer: offer.clone(),
            })
            .expect("remote"),
            PayloadFamily::Process => {
                encode(&ProcessPreAuth::ProcessClientHello { offer }).expect("process")
            }
        };
        let frame = encode_frame(&body, PRE_AUTH_FRAME_MAX_BYTES).expect("frame");
        let header: [u8; 4] = frame[..4].try_into().expect("hdr");
        assert_eq!(
            decode_frame_len(header, PRE_AUTH_FRAME_MAX_BYTES).expect("len"),
            body.len()
        );
    }
}

#[test]
fn fixture_vocabularies_stay_family_separate() {
    let remote = fs::read_to_string(
        repo_root().join("fixtures/compatibility/remote/v1/vocab/valid--open-session.json"),
    )
    .expect("remote fixture");
    let process = fs::read_to_string(
        repo_root()
            .join("fixtures/compatibility/process/v1/handshake/valid--process-client-hello.json"),
    )
    .expect("process fixture");
    assert!(remote.contains("open_session"));
    assert!(process.contains("process_client_hello"));
    assert!(!process.contains("open_session"));

    let offer = VersionOffer::try_new(vec![1], 1, vec!["auth".into()]).expect("offer");
    let process_hello = ProcessPreAuth::ProcessClientHello { offer };
    let env = encode_envelope(PayloadFamily::Process, 1, &process_hello).expect("env");
    assert!(decode_envelope::<RemotePostAuth>(&env, PayloadFamily::Remote).is_err());
    assert!(decode::<RemotePostAuth>(&encode(&process_hello).expect("body")).is_err());
}

#[test]
fn shared_version_select_rejects_unknown() {
    let client = VersionOffer::try_new(vec![2], 2, vec!["auth".into()]).expect("client");
    let server = VersionOffer::try_new(vec![1], 1, vec!["auth".into()]).expect("server");
    assert!(select_version(&client, &server).is_err());
}
