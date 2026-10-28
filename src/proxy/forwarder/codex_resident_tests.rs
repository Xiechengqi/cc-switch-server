use super::*;
use crate::infra::resident_memory::ResidentMemoryBudget;

async fn socket_pair() -> (
    RawResponsesUpstreamWebSocket,
    tokio::sync::mpsc::Sender<TungsteniteMessage>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (commands, mut received) = tokio::sync::mpsc::channel::<TungsteniteMessage>(256);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(message) = received.recv().await {
            if socket.send(message).await.is_err() {
                break;
            }
        }
    });
    let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
        .await
        .unwrap();
    (socket, commands, server)
}

async fn wait_for(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("resident owner transition must complete");
}

#[tokio::test]
async fn codex_r4_idle_and_active_frames_keep_one_owner_across_turn_changes() {
    let parent = ResidentMemoryBudget::new(None);
    let (raw, commands, server) = socket_pair().await;
    let mut socket = ResponsesUpstreamWebSocket::with_parent(raw, parent.clone()).unwrap();
    let base = parent.snapshot().used_bytes;
    commands
        .send(TungsteniteMessage::Text("idle".into()))
        .await
        .unwrap();
    wait_for(|| socket.reads.len() == 1).await;
    assert_eq!(parent.snapshot().used_bytes, base + 4);

    let request = RequestMemoryBudget::with_parent(1024, parent.clone());
    socket.set_request_memory(Some(request.clone()));
    let idle = socket.next().await.unwrap();
    assert!(matches!(&idle.result, Ok(TungsteniteMessage::Text(text)) if text == "idle"));
    assert_eq!(request.snapshot().used_bytes, 0);
    assert_eq!(parent.snapshot().used_bytes, base + 4);
    drop(idle);
    assert_eq!(parent.snapshot().used_bytes, base);

    commands
        .send(TungsteniteMessage::Text("active".into()))
        .await
        .unwrap();
    wait_for(|| socket.reads.len() == 1).await;
    assert_eq!(request.snapshot().used_bytes, 6);
    assert_eq!(parent.snapshot().used_bytes, base + 6);
    socket.set_request_memory(None);
    let active = socket.next().await.unwrap();
    assert_eq!(request.snapshot().used_bytes, 6);
    drop(active);
    assert_eq!(request.snapshot().used_bytes, 0);
    assert_eq!(parent.snapshot().used_bytes, base);
    drop(socket);
    wait_for(|| parent.snapshot().used_bytes == 0).await;
    server.abort();
}

#[tokio::test]
async fn codex_r4_idle_capacity_failure_and_constructor_denial_do_not_replay() {
    let parent = ResidentMemoryBudget::new(None);
    let (raw, commands, server) = socket_pair().await;
    let mut socket = ResponsesUpstreamWebSocket::with_parent(raw, parent.clone()).unwrap();
    let base = parent.snapshot().used_bytes;
    parent.configure_limit(Some(base + 4));
    commands
        .send(TungsteniteMessage::Text("large".into()))
        .await
        .unwrap();
    let message = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(&message.result, Err(ResponsesWebSocketReadError::Memory(error))
        if error.is_request_memory_exhausted() && error.status == StatusCode::SERVICE_UNAVAILABLE)
    );
    assert!(socket.peer_terminal_observed());
    assert_eq!(parent.snapshot().used_bytes, base);
    assert_eq!(parent.snapshot().rejections, 1);
    drop(message);
    drop(socket);
    wait_for(|| parent.snapshot().used_bytes == 0).await;
    server.abort();

    parent.configure_limit(Some(0));
    let (raw, _commands, server) = socket_pair().await;
    assert!(
        matches!(ResponsesUpstreamWebSocket::with_parent(raw, parent.clone()),
        Err(error) if error.is_request_memory_exhausted())
    );
    assert_eq!(parent.snapshot().used_bytes, 0);
    server.abort();
}

#[tokio::test]
async fn codex_r4_aborting_full_idle_read_queue_releases_pending_frame_owner() {
    let parent = ResidentMemoryBudget::new(None);
    let (raw, commands, server) = socket_pair().await;
    let socket = ResponsesUpstreamWebSocket::with_parent(raw, parent.clone()).unwrap();
    let base = parent.snapshot().used_bytes;
    let count = RESPONSES_WEBSOCKET_READ_QUEUE_CAPACITY + 1;
    for _ in 0..count {
        commands
            .send(TungsteniteMessage::Text("x".repeat(32)))
            .await
            .unwrap();
    }
    wait_for(|| parent.snapshot().used_bytes == base + count * 32).await;
    assert_eq!(socket.reads.len(), RESPONSES_WEBSOCKET_READ_QUEUE_CAPACITY);
    drop(socket);
    wait_for(|| parent.snapshot().used_bytes == 0).await;
    assert_eq!(parent.snapshot().active_reservations, 0);
    server.abort();
}

#[tokio::test]
async fn codex_r4_pool_capacity_ttl_acquire_and_peer_close_release_owners() {
    let parent = ResidentMemoryBudget::new(None);
    let policy = ResponsesWebSocketPoolPolicy {
        max_connections: 1,
        idle_timeout: Duration::from_secs(1),
        max_age: Duration::from_secs(10),
    };
    let mut pool = ResponsesWebSocketPool::default();
    let (raw, commands_a, server_a) = socket_pair().await;
    let socket_a = ResponsesUpstreamWebSocket::with_parent(raw, parent.clone()).unwrap();
    let base = parent.snapshot().used_bytes;
    pool.release_with_policy(
        "a".into(),
        CachedResponsesWebSocket {
            socket: socket_a,
            created_at: Instant::now(),
            last_used_at: Instant::now(),
        },
        policy,
    );
    assert!(parent.snapshot().used_bytes > base);
    let acquired = pool.acquire_with_policy("a", policy).unwrap();
    assert_eq!(parent.snapshot().used_bytes, base);
    pool.release_with_policy("a".into(), acquired, policy);
    let (raw, commands_b, server_b) = socket_pair().await;
    let socket_b = ResponsesUpstreamWebSocket::with_parent(raw, parent.clone()).unwrap();
    pool.release_with_policy(
        "b".into(),
        CachedResponsesWebSocket {
            socket: socket_b,
            created_at: Instant::now(),
            last_used_at: Instant::now(),
        },
        policy,
    );
    assert!(pool.acquire_with_policy("a", policy).is_none());
    pool.entries.get_mut("b").unwrap()[0].last_used_at = Instant::now() - Duration::from_secs(2);
    pool.prune_expired_with_policy(policy);
    assert_eq!(pool.total, 0);
    wait_for(|| parent.snapshot().used_bytes == 0).await;
    drop((commands_a, commands_b));
    server_a.abort();
    server_b.abort();

    let (raw, commands, server) = socket_pair().await;
    let mut socket = ResponsesUpstreamWebSocket::with_parent(raw, parent.clone()).unwrap();
    commands
        .send(TungsteniteMessage::Close(None))
        .await
        .unwrap();
    let close = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(&close.result, Ok(TungsteniteMessage::Close(_))));
    drop(close);
    drop(socket);
    wait_for(|| parent.snapshot().used_bytes == 0).await;
    server.abort();

    let (raw, _commands, server) = socket_pair().await;
    let socket = ResponsesUpstreamWebSocket::with_parent(raw, parent.clone()).unwrap();
    parent.configure_limit(Some(parent.snapshot().used_bytes));
    pool.release_with_policy(
        "denied-index".into(),
        CachedResponsesWebSocket {
            socket,
            created_at: Instant::now(),
            last_used_at: Instant::now(),
        },
        policy,
    );
    assert_eq!(pool.total, 0);
    wait_for(|| parent.snapshot().used_bytes == 0).await;
    server.abort();
}
