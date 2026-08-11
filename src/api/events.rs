use std::time::Duration;

use super::*;

pub(in crate::api) async fn events(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    require_event_session(&state, &headers).await?;
    let receiver = state.subscribe_events();
    let shutdown = state.subscribe_shutdown();
    Ok(Sse::new(event_stream(receiver, shutdown)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

fn event_stream(
    receiver: tokio::sync::broadcast::Receiver<crate::state::ServerEvent>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    futures_util::stream::unfold(
        (receiver, shutdown),
        |(mut receiver, mut shutdown)| async move {
            loop {
                if *shutdown.borrow() {
                    return None;
                }
                tokio::select! {
                    biased;
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            return None;
                        }
                    }
                    received = receiver.recv() => match received {
                        Ok(event) => {
                            let event_name = event.event_type.clone();
                            let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
                            return Some((Ok(Event::default().event(event_name).data(data)), (receiver, shutdown)));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    }
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;

    use super::*;

    #[tokio::test]
    async fn event_stream_ends_when_shutdown_begins() {
        let (events, _) = tokio::sync::broadcast::channel(4);
        let (shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
        let stream = event_stream(events.subscribe(), shutdown_rx);
        tokio::pin!(stream);

        shutdown.send(true).unwrap();

        let next = tokio::time::timeout(Duration::from_millis(100), stream.next())
            .await
            .expect("shutdown stream should finish promptly");
        assert!(next.is_none());
    }
}
