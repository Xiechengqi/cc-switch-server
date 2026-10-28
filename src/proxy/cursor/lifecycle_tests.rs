//! Loopback-only h2 lifecycle probes. No vendor address, TLS override, or credential.
use super::*;
use crate::infra::resident_memory::ResidentMemoryBudget;
use crate::proxy::cursor::profile::CursorProtocolRail;
use crate::proxy::cursor::session::{
    CursorSessionKey, CursorSessionManager, CursorSessionScope, CursorSessionScopeInput,
    SessionState,
};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::oneshot;

struct ResponseOwner(Option<oneshot::Sender<()>>);
impl Drop for ResponseOwner {
    fn drop(&mut self) {
        if let Some(closed) = self.0.take() {
            let _ = closed.send(());
        }
    }
}

pub(crate) struct LoopbackPeer {
    stream: Option<CursorH2Stream>,
    response_closed: oneshot::Receiver<()>,
    request_closed: oneshot::Receiver<()>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    response_writer: UnboundedSender<Bytes>,
}
impl LoopbackPeer {
    pub(crate) async fn open(budget: RequestMemoryBudget, first_timeout: Duration) -> Self {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (response_tx, response_closed) = oneshot::channel();
        let (request_tx, request_closed) = oneshot::channel();
        let response_owner = Arc::new(std::sync::Mutex::new(Some(ResponseOwner(Some(
            response_tx,
        )))));
        let request_owner = Arc::new(std::sync::Mutex::new(Some(request_tx)));
        let (response_writer, response_rx) = unbounded_channel::<Bytes>();
        let response_rx = Arc::new(std::sync::Mutex::new(Some(response_rx)));
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let service = service_fn(move |request: http::Request<Incoming>| {
                let response_owner = response_owner.lock().unwrap().take().unwrap();
                let request_owner = request_owner.lock().unwrap().take().unwrap();
                let mut response_rx = response_rx.lock().unwrap().take().unwrap();
                async move {
                    // Read until END_STREAM/RST_STREAM so the test observes
                    // cancellation from the client's live request half.
                    tokio::spawn(async move {
                        let mut body = request.into_body();
                        while let Some(frame) = body.frame().await {
                            if frame.is_err() {
                                break;
                            }
                        }
                        let _ = request_owner.send(());
                    });
                    let body = stream! {
                        let _owner = response_owner;
                        while let Some(bytes) = response_rx.recv().await {
                            yield Ok::<_, Infallible>(Frame::data(bytes));
                        }
                    };
                    Ok::<_, Infallible>(http::Response::new(StreamBody::new(Box::pin(body))))
                }
            });
            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(socket), service)
                .await;
        });
        let socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let (mut sender, connection) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(socket))
                .await
                .unwrap();
        let client = tokio::spawn(async move {
            let _ = connection.await;
        });
        let (writer, mut receiver) = unbounded_channel::<Bytes>();
        let body = stream! {
            while let Some(bytes) = receiver.recv().await {
                yield Ok::<_, Infallible>(Frame::data(bytes));
            }
        };
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("http://{address}/agent.v1.AgentService/Run"))
            .body(StreamBody::new(Box::pin(body)))
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        drop(sender);
        let timeouts = CursorH2Timeouts {
            request: Duration::from_secs(2),
            first_frame: Some(first_timeout),
            inter_frame: Some(Duration::from_secs(2)),
        };
        let stream = CursorH2Stream {
            writer: Some(writer),
            response,
            parser: ConnectFrameParser::new(),
            trailers: None,
            pending: VecDeque::new(),
            closed: false,
            received_any_frame: false,
            output_phase: CursorOutputPhase::new(timeouts, TokioInstant::now()),
            connect_end_stream: None,
            parser_memory: Some(
                budget
                    .reserve(RequestMemoryComponent::TransportPending, 0)
                    .unwrap(),
            ),
            pending_memory: Some(
                budget
                    .reserve(RequestMemoryComponent::DecodedBody, 0)
                    .unwrap(),
            ),
            request_memory: Some(budget),
        };
        Self {
            stream: Some(stream),
            response_closed,
            request_closed,
            tasks: vec![server, client],
            response_writer,
        }
    }

    pub(crate) fn take_stream(&mut self) -> CursorH2Stream {
        self.stream.take().unwrap()
    }

    pub(crate) fn send(&self, bytes: Bytes) {
        self.response_writer.send(bytes).unwrap();
    }

    pub(crate) async fn assert_closed(&mut self) {
        tokio::time::timeout(Duration::from_secs(2), &mut self.request_closed)
            .await
            .expect("upstream request must close")
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), &mut self.response_closed)
            .await
            .expect("upstream response owner must drop")
            .unwrap();
    }
}
impl Drop for LoopbackPeer {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[tokio::test]
async fn cursor_r4_loopback_cancel_and_first_output_timeout_close_both_h2_halves() {
    for cancel in [false, true] {
        let parent = ResidentMemoryBudget::new(None);
        let budget = RequestMemoryBudget::with_parent(64 * 1024, parent.clone());
        let held = budget
            .reserve(RequestMemoryComponent::StreamRetainedState, 4096)
            .unwrap();
        let mut peer = LoopbackPeer::open(budget, Duration::from_millis(20)).await;
        let mut stream = peer.take_stream();
        if cancel {
            let task = tokio::spawn(async move {
                let _held = held;
                stream.next_frame().await
            });
            tokio::task::yield_now().await;
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            let error = stream.next_frame().await.unwrap_err();
            assert!(error.message.contains("timed out"));
            drop((stream, held));
        }
        peer.assert_closed().await;
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
}

#[tokio::test]
async fn cursor_r4_loopback_parked_expiry_and_capacity_eviction_close_h2_on_both_rails() {
    for rail in [CursorProtocolRail::OAuthCli, CursorProtocolRail::ApiKeySdk] {
        for expiry in [false, true] {
            let parent = ResidentMemoryBudget::new(None);
            let budget = RequestMemoryBudget::with_parent(128 * 1024, parent.clone());
            let ttl = if expiry {
                Duration::from_millis(20)
            } else {
                Duration::from_secs(30)
            };
            let manager = CursorSessionManager::new(ttl, 1);
            let key = CursorSessionKey::new(
                CursorSessionScope::fixture("loopback-parked"),
                "conversation",
            );
            let mut peer = LoopbackPeer::open(budget.clone(), Duration::from_secs(2)).await;
            let entry = manager
                .reserve(
                    key.clone(),
                    rail,
                    HashMap::new(),
                    Vec::new(),
                    vec![serde_json::json!({"content":"x".repeat(4096)})],
                    String::new(),
                )
                .await
                .unwrap();
            manager
                .attach_stream(&entry, peer.take_stream())
                .await
                .unwrap();
            entry.lock().await.attach_request_memory(&budget).unwrap();
            manager
                .release(entry, SessionState::AwaitingToolResult)
                .await;
            assert!(parent.snapshot().used_bytes > 4096);
            if expiry {
                tokio::time::timeout(Duration::from_secs(2), async {
                    while manager.has(&key).await {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
            } else {
                let new_key = CursorSessionKey::new(
                    CursorSessionScope::fixture("loopback-new"),
                    "new-conversation",
                );
                let replacement = manager
                    .reserve(
                        new_key,
                        rail,
                        HashMap::new(),
                        Vec::new(),
                        Vec::new(),
                        String::new(),
                    )
                    .await
                    .unwrap();
                manager.release(replacement, SessionState::Closed).await;
            }
            peer.assert_closed().await;
            assert!(!manager.has(&key).await);
            assert_eq!(parent.snapshot().used_bytes, 0);
        }
    }
}

#[tokio::test]
async fn cursor_r4_loopback_late_tool_result_cannot_acquire_another_identity() {
    let parent = ResidentMemoryBudget::new(None);
    let budget = RequestMemoryBudget::with_parent(128 * 1024, parent.clone());
    let manager = CursorSessionManager::default();
    let identity_scope = |principal| {
        CursorSessionScope::derive(CursorSessionScopeInput {
            app: "codex",
            provider_id: "provider",
            provider_revision: 1,
            runtime_fingerprint: "runtime",
            rail: CursorProtocolRail::OAuthCli,
            protocol_revision: CursorProtocolRail::OAuthCli.protocol_revision(),
            principal,
            share_id: Some("share"),
            user_email: Some("user@example.com"),
        })
    };
    let old_scope = identity_scope("account:1:token-digest-a");
    let new_scope = identity_scope("account:2:token-digest-b");
    let key = CursorSessionKey::new(old_scope, "conversation");
    let mut peer = LoopbackPeer::open(budget.clone(), Duration::from_secs(2)).await;
    let entry = manager
        .reserve(
            key.clone(),
            CursorProtocolRail::OAuthCli,
            HashMap::new(),
            Vec::new(),
            Vec::new(),
            String::new(),
        )
        .await
        .unwrap();
    manager
        .attach_stream(&entry, peer.take_stream())
        .await
        .unwrap();
    entry.lock().await.attach_request_memory(&budget).unwrap();
    manager.bind_tool_call_id(&key, &entry, "call-a").await;
    manager.bind_response_id(&key, &entry, "resp-a").await;
    manager
        .release(entry.clone(), SessionState::AwaitingToolResult)
        .await;
    assert!(manager
        .resolve_tool_call_id(&new_scope, "call-a")
        .await
        .is_none());
    assert!(manager
        .resolve_response_id(&new_scope, "resp-a")
        .await
        .is_none());
    assert!(manager
        .acquire(&CursorSessionKey::new(new_scope, "conversation"))
        .await
        .is_none());
    assert!(manager.has(&key).await);
    manager.release(entry, SessionState::Closed).await;
    peer.assert_closed().await;
    assert_eq!(parent.snapshot().used_bytes, 0);
}
