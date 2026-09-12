use axum::extract::ws::{Message, WebSocket};
use futures_util::StreamExt;

use super::super::ProxyError;

pub(in crate::proxy) enum DownstreamWait<T> {
    Ready(T),
    DownstreamClosed,
}

/// Await a network/backoff future without losing downstream cancellation or
/// WebSocket ping/pong liveness. A second request on the same in-flight turn
/// is rejected explicitly instead of being queued behind an opaque operation.
pub(in crate::proxy) async fn wait_with_downstream_cancellation<T>(
    downstream: &mut WebSocket,
    future: impl std::future::Future<Output = T>,
) -> Result<DownstreamWait<T>, ProxyError> {
    tokio::pin!(future);
    loop {
        tokio::select! {
            value = &mut future => return Ok(DownstreamWait::Ready(value)),
            downstream_message = downstream.next() => {
                let Some(message) = downstream_message else {
                    return Ok(DownstreamWait::DownstreamClosed);
                };
                let Ok(message) = message else {
                    return Ok(DownstreamWait::DownstreamClosed);
                };
                match message {
                    Message::Close(_) => return Ok(DownstreamWait::DownstreamClosed),
                    Message::Ping(bytes) => {
                        if downstream.send(Message::Pong(bytes)).await.is_err() {
                            return Ok(DownstreamWait::DownstreamClosed);
                        }
                    }
                    Message::Pong(_) => {}
                    Message::Text(_) | Message::Binary(_) => {
                        return Err(ProxyError::bad_request(
                            "responses websocket received a request while HTTP fallback was in flight",
                        ));
                    }
                }
            }
        }
    }
}
