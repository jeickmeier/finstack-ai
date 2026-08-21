//! Explicit-route remote child invoker over PR-058 framing.
//!
//! Native-only. `wasm-host` callers use the SDK inherent, which returns
//! `agent_run_unsupported_plan` without linking this crate.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

mod invoker;
mod route;
mod transport;

pub use invoker::RemoteChildInvoker;
pub use route::RemoteChildRoute;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_kernel::BudgetRequest;
    use finstack_ai_kernel::{
        AgentId, BundleId, ChildPlacement, ChildRunLocator, ComponentId, ComponentRef,
        ContentBlock, Digest, EffectId, ExternalHandleRef, LaneId, Metadata, OperationLocator,
        RawJson, RemoteRouteRef, RunId, SessionId, TextBlock, Version,
    };
    use finstack_ai_protocol::{
        FRAME_LENGTH_BYTES, POST_AUTH_FRAME_MAX_BYTES, PRE_AUTH_FRAME_MAX_BYTES,
        PROTOCOL_VERSION_V1, PayloadFamily, ProtocolEnvelope, RemoteCommandResult, RemotePostAuth,
        RemotePreAuth, VersionOffer, decode_envelope, decode_frame_len, encode_envelope,
        encode_frame,
    };
    use finstack_ai_runtime::{
        AgentInvoker, AuthorizationContext, ChildRunContext, ChildRunRequest,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::{RemoteChildInvoker, RemoteChildRoute};

    #[test]
    fn construction_rejects_non_loopback_plaintext() {
        let error = RemoteChildInvoker::try_new(RemoteChildRoute {
            endpoint: "8.8.8.8:9".into(),
            service: "finstack.remote.worker".into(),
            route: "route-1".into(),
            token: None,
        })
        .err()
        .expect("non-loopback");
        assert!(error.to_string().contains("loopback"));
    }

    #[tokio::test]
    async fn start_or_attach_is_idempotent_and_cancel_is_not_a_noop() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let endpoint = listener.local_addr().expect("addr").to_string();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept start");
            serve_one_command(stream).await;
            let (stream, _) = listener.accept().await.expect("accept cancel");
            serve_one_command(stream).await;
        });

        let invoker = RemoteChildInvoker::try_new(RemoteChildRoute {
            endpoint,
            service: "finstack.remote.worker".into(),
            route: "route-1".into(),
            token: None,
        })
        .expect("invoker");
        let request = child_request();
        let context = child_context();
        let first = invoker
            .start_or_attach(context.clone(), request.clone())
            .await
            .expect("start");
        let second = invoker
            .start_or_attach(context, request.clone())
            .await
            .expect("attach");
        assert_eq!(first, second);
        invoker.cancel(&request.locator).await.expect("cancel");
        server.await.expect("server");
    }

    #[tokio::test]
    async fn cancel_rejects_a_locator_that_was_never_accepted() {
        let invoker = RemoteChildInvoker::try_new(RemoteChildRoute {
            endpoint: "127.0.0.1:9".into(),
            service: "finstack.remote.worker".into(),
            route: "route-1".into(),
            token: None,
        })
        .expect("invoker");
        let error = invoker
            .cancel(&child_request().locator)
            .await
            .expect_err("never accepted");
        assert!(error.to_string().contains("never accepted"));
    }

    #[tokio::test]
    async fn peer_that_never_sends_command_result_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let endpoint = listener.local_addr().expect("addr").to_string();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            handshake_stream(&mut stream).await;
            let _command = read_post(&mut stream).await;
            // Keep the socket open so the client hits the read timeout, not EOF.
            tokio::time::sleep(std::time::Duration::from_secs(8)).await;
        });
        let invoker = RemoteChildInvoker::try_new(RemoteChildRoute {
            endpoint,
            service: "finstack.remote.worker".into(),
            route: "route-1".into(),
            token: None,
        })
        .expect("invoker");
        let error = invoker
            .start_or_attach(child_context(), child_request())
            .await
            .expect_err("timeout");
        assert!(error.to_string().contains("timed out") || error.to_string().contains("bound"));
        server.abort();
    }

    #[tokio::test]
    async fn ten_thousand_event_batches_fail_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let endpoint = listener.local_addr().expect("addr").to_string();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            handshake_stream(&mut stream).await;
            let _command = read_post(&mut stream).await;
            for _ in 0..10_000 {
                write_post(
                    &mut stream,
                    &RemotePostAuth::EventBatch { events: Vec::new() },
                )
                .await;
            }
        });
        let invoker = RemoteChildInvoker::try_new(RemoteChildRoute {
            endpoint,
            service: "finstack.remote.worker".into(),
            route: "route-1".into(),
            token: None,
        })
        .expect("invoker");
        let error = invoker
            .start_or_attach(child_context(), child_request())
            .await
            .expect_err("bounded");
        assert!(error.to_string().contains("bound"));
        server.abort();
    }

    fn child_request() -> ChildRunRequest {
        let locator = ChildRunLocator {
            operation: OperationLocator::try_new(
                "tenant-a",
                SessionId::from_bytes([1; 16]),
                LaneId::from_bytes([2; 16]),
                RunId::from_bytes([3; 16]),
            )
            .expect("locator"),
            remote: Some(RemoteRouteRef {
                service: ComponentRef::new(
                    ComponentId::parse("finstack.remote.worker").expect("service"),
                    Some(Version {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    }),
                ),
                route: ExternalHandleRef::try_new(
                    ComponentId::parse("finstack.remote.worker").expect("provider"),
                    "route-1",
                    RawJson::parse(b"{}").expect("json"),
                )
                .expect("route"),
            }),
        };
        let agent = finstack_ai_runtime::AgentRef {
            id: AgentId::parse("python.agent.remote").expect("agent"),
            bundle: Some(BundleId::parse("python.bundle.remote").expect("bundle")),
            spec_digest: Digest::raw_json(b"{}"),
        };
        let input = Arc::from([ContentBlock::Text(
            TextBlock::try_new("hello").expect("text"),
        )]);
        ChildRunRequest {
            agent,
            input,
            placement: ChildPlacement::RemoteChildSession,
            locator,
            requested_deadline: None,
            requested_budget: BudgetRequest::default(),
            delegation_id: None,
            metadata: Metadata::empty(),
            request_digest: Digest::raw_json(b"{\"n\":1}"),
        }
    }

    fn child_context() -> ChildRunContext {
        ChildRunContext {
            parent: OperationLocator::try_new(
                "tenant-a",
                SessionId::from_bytes([4; 16]),
                LaneId::from_bytes([5; 16]),
                RunId::from_bytes([6; 16]),
            )
            .expect("parent"),
            parent_effect_id: EffectId::from_bytes([7; 16]),
            authorization: AuthorizationContext {
                principal: finstack_ai_kernel::PrincipalRef::try_new(
                    "loopback",
                    "tester",
                    Some("tenant-a"),
                )
                .expect("principal"),
                authentication_method: Arc::from("loopback"),
                assurance_level: Arc::from("low"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("1"),
                decision_id: Arc::from("d1"),
            },
        }
    }

    async fn handshake_stream(stream: &mut tokio::net::TcpStream) {
        let offer = VersionOffer::try_new(
            vec![PROTOCOL_VERSION_V1],
            PROTOCOL_VERSION_V1,
            vec!["auth".into()],
        )
        .expect("offer");
        let _hello = read_pre(stream).await;
        write_pre(
            stream,
            &RemotePreAuth::ServerHello {
                selected_version: PROTOCOL_VERSION_V1,
                offer,
            },
        )
        .await;
        let _auth = read_pre(stream).await;
        write_pre(
            stream,
            &RemotePreAuth::AuthResult {
                accepted: true,
                reason_code: None,
            },
        )
        .await;
        let _open = read_post(stream).await;
        write_post(stream, &RemotePostAuth::NoSnapshot { sequence: 0 }).await;
        write_post(stream, &RemotePostAuth::SyncBarrier { sequence: 0 }).await;
    }

    async fn serve_one_command(mut stream: tokio::net::TcpStream) {
        handshake_stream(&mut stream).await;
        let RemotePostAuth::Command { command } = read_post(&mut stream).await else {
            panic!("expected command");
        };
        write_post(
            &mut stream,
            &RemotePostAuth::CommandResult {
                result: RemoteCommandResult::try_new(
                    command.command_id(),
                    command.digest(),
                    command.expected_durable_sequence().saturating_add(1),
                    true,
                    None,
                )
                .expect("result"),
            },
        )
        .await;
    }

    async fn read_pre(stream: &mut tokio::net::TcpStream) -> RemotePreAuth {
        let payload = read_frame(stream, PRE_AUTH_FRAME_MAX_BYTES).await;
        let envelope: ProtocolEnvelope<RemotePreAuth> =
            decode_envelope(&payload, PayloadFamily::Remote, PROTOCOL_VERSION_V1).expect("pre");
        envelope.into_body()
    }

    async fn write_pre(stream: &mut tokio::net::TcpStream, body: &RemotePreAuth) {
        let payload =
            encode_envelope(PayloadFamily::Remote, PROTOCOL_VERSION_V1, body).expect("encode");
        write_frame(stream, &payload, PRE_AUTH_FRAME_MAX_BYTES).await;
    }

    async fn read_post(stream: &mut tokio::net::TcpStream) -> RemotePostAuth {
        let payload = read_frame(stream, POST_AUTH_FRAME_MAX_BYTES).await;
        let envelope: ProtocolEnvelope<RemotePostAuth> =
            decode_envelope(&payload, PayloadFamily::Remote, PROTOCOL_VERSION_V1).expect("post");
        envelope.into_body()
    }

    async fn write_post(stream: &mut tokio::net::TcpStream, body: &RemotePostAuth) {
        let payload =
            encode_envelope(PayloadFamily::Remote, PROTOCOL_VERSION_V1, body).expect("encode");
        write_frame(stream, &payload, POST_AUTH_FRAME_MAX_BYTES).await;
    }

    async fn read_frame(stream: &mut tokio::net::TcpStream, ceiling: usize) -> Vec<u8> {
        let mut header = [0_u8; FRAME_LENGTH_BYTES];
        stream.read_exact(&mut header).await.expect("hdr");
        let declared = decode_frame_len(header, ceiling).expect("len");
        let mut payload = vec![0_u8; declared];
        if declared > 0 {
            stream.read_exact(&mut payload).await.expect("payload");
        }
        payload
    }

    async fn write_frame(stream: &mut tokio::net::TcpStream, payload: &[u8], ceiling: usize) {
        let frame = encode_frame(payload, ceiling).expect("frame");
        stream.write_all(&frame).await.expect("write");
        stream.flush().await.expect("flush");
    }
}
