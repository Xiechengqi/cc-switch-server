//! Per-bridge Codex interrupt state. An interrupt can only use the current
//! physical socket and response. It never acquires/reconnects/replays a lease.
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::proxy::ProxyError;

pub(in crate::proxy) const MAX_RESPONSE_ID_BYTES: usize = 512;
pub(in crate::proxy) const INTERRUPT_ACK_TIMEOUT: Duration = Duration::from_secs(30);

fn valid_response_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|id| !id.trim().is_empty() && id.len() <= MAX_RESPONSE_ID_BYTES)
}

pub(in crate::proxy) fn interrupt_id(value: &Value) -> Result<Option<&str>, ProxyError> {
    if value.get("type").and_then(Value::as_str) != Some("response.interrupt") {
        return Ok(None);
    }
    let id = value
        .get("response_id")
        .and_then(valid_response_id)
        .ok_or_else(|| {
            ProxyError::bad_request(
                "response.interrupt requires response_id (nonempty, at most 512 bytes)",
            )
        })?;
    if value.get("mode").and_then(Value::as_str) != Some("discard_partial_items") {
        return Err(ProxyError::bad_request(
            "response.interrupt requires mode=discard_partial_items",
        ));
    }
    Ok(Some(id))
}

/// Only the reviewed control envelope may bypass generation error handling.
/// A response/delta/item or future envelope field cannot be hidden in an ACK.
pub(in crate::proxy) fn is_interrupt_ack(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let accepted = match object.get("type").and_then(Value::as_str) {
        Some("response.interrupt.accepted") => true,
        Some("response.interrupt.failed") => false,
        _ => return false,
    };
    object
        .get("response_id")
        .and_then(valid_response_id)
        .is_some()
        && object
            .get("sequence_number")
            .is_none_or(|value| value.as_u64().is_some())
        && if accepted {
            object.get("error").is_none_or(Value::is_null)
        } else {
            object.get("error").is_some_and(Value::is_object)
        }
        && object.keys().all(|key| {
            matches!(
                key.as_str(),
                "type" | "response_id" | "sequence_number" | "error"
            )
        })
}

#[derive(Default)]
pub(in crate::proxy) struct InterruptState {
    response_id: Option<String>,
    connection_created_at: Option<Instant>,
    requested: bool,
    acknowledgement_deadline: Option<tokio::time::Instant>,
    terminal_seen: bool,
}

impl InterruptState {
    pub(in crate::proxy) fn begin_response(&mut self) {
        debug_assert!(!self.awaiting_ack());
        *self = Self::default();
    }

    pub(in crate::proxy) fn observe_response_id(
        &mut self,
        connection: Instant,
        id: &str,
    ) -> Result<(), ProxyError> {
        if id.trim().is_empty() || id.len() > MAX_RESPONSE_ID_BYTES {
            return Err(ProxyError::bad_gateway(
                "Codex WebSocket response ID is invalid",
            ));
        }
        if self
            .response_id
            .as_deref()
            .is_some_and(|existing| existing != id)
            || self
                .connection_created_at
                .is_some_and(|existing| existing != connection)
        {
            return Err(ProxyError::bad_gateway(
                "Codex WebSocket response identity changed during a turn",
            ));
        }
        self.response_id.get_or_insert_with(|| id.to_string());
        self.connection_created_at = Some(connection);
        Ok(())
    }

    pub(in crate::proxy) fn response_id(&self) -> Option<&str> {
        self.response_id.as_deref()
    }

    /// `false` means a duplicate request; its original ACK remains authoritative.
    pub(in crate::proxy) fn request(
        &mut self,
        connection: Instant,
        id: &str,
        now: tokio::time::Instant,
        timeout: Duration,
    ) -> Result<bool, ProxyError> {
        if self.terminal_seen
            || self.connection_created_at != Some(connection)
            || self.response_id.as_deref() != Some(id)
        {
            return Err(ProxyError::bad_request(
                "response.interrupt does not match the active upstream response",
            ));
        }
        if self.requested {
            return Ok(false);
        }
        self.requested = true;
        self.acknowledgement_deadline = Some(now + timeout);
        Ok(true)
    }

    /// ACK errors are control results. A mismatched/duplicate/unsolicited ACK
    /// must not finish, retime, or acknowledge another generation.
    pub(in crate::proxy) fn observe_ack(&mut self, connection: Instant, value: &Value) -> bool {
        if !is_interrupt_ack(value)
            || !self.awaiting_ack()
            || self.connection_created_at != Some(connection)
            || value.get("response_id").and_then(Value::as_str) != self.response_id.as_deref()
        {
            return false;
        }
        self.acknowledgement_deadline = None;
        true
    }

    pub(in crate::proxy) fn finish_generation(&mut self) {
        self.terminal_seen = true;
    }

    pub(in crate::proxy) fn awaiting_ack(&self) -> bool {
        self.acknowledgement_deadline.is_some()
    }

    pub(in crate::proxy) fn read_deadline(
        &self,
        generation_deadline: Option<tokio::time::Instant>,
    ) -> Option<tokio::time::Instant> {
        match (self.acknowledgement_deadline, generation_deadline) {
            (Some(ack), Some(generation)) => Some(ack.min(generation)),
            (ack, generation) => ack.or(generation),
        }
    }

    pub(in crate::proxy) fn ack_expired(&self, now: tokio::time::Instant) -> bool {
        self.acknowledgement_deadline
            .is_some_and(|deadline| now >= deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_r4_interrupt_state_fences_identity_duplicates_ack_and_terminal() {
        let connection = Instant::now();
        let other_connection = connection + Duration::from_secs(1);
        let now = tokio::time::Instant::now();
        let mut state = InterruptState::default();
        assert!(state
            .request(connection, "r", now, INTERRUPT_ACK_TIMEOUT)
            .is_err());
        state.observe_response_id(connection, " r ").unwrap();
        assert!(state
            .request(connection, "r", now, INTERRUPT_ACK_TIMEOUT)
            .is_err());
        assert!(state
            .request(other_connection, " r ", now, INTERRUPT_ACK_TIMEOUT)
            .is_err());
        assert!(state
            .request(connection, " r ", now, INTERRUPT_ACK_TIMEOUT)
            .unwrap());
        let deadline = state.read_deadline(None).unwrap();
        assert!(!state
            .request(
                connection,
                " r ",
                now + Duration::from_secs(1),
                INTERRUPT_ACK_TIMEOUT
            )
            .unwrap());
        assert_eq!(state.read_deadline(None), Some(deadline));
        assert!(!state.observe_ack(
            connection,
            &json!({"type":"response.interrupt.accepted","response_id":"old"})
        ));
        assert!(!state.observe_ack(
            other_connection,
            &json!({"type":"response.interrupt.accepted","response_id":" r "})
        ));
        assert!(state.awaiting_ack());
        assert!(!state.ack_expired(deadline - Duration::from_millis(1)));
        assert!(state.ack_expired(deadline));
        state.finish_generation();
        assert!(
            state.awaiting_ack(),
            "a terminal cannot manufacture an upstream ACK"
        );
        assert!(state.observe_ack(
            connection,
            &json!({"type":"response.interrupt.accepted","response_id":" r "})
        ));
        assert!(!state.awaiting_ack());
        assert!(!state.observe_ack(
            connection,
            &json!({"type":"response.interrupt.failed","response_id":" r ","error":{}})
        ));
        assert!(state
            .request(connection, " r ", now, INTERRUPT_ACK_TIMEOUT)
            .is_err());
        state.begin_response();
        state.observe_response_id(other_connection, "next").unwrap();
        assert!(state
            .request(other_connection, "next", now, INTERRUPT_ACK_TIMEOUT)
            .unwrap());
        assert!(state.observe_ack(other_connection, &json!({"type":"response.interrupt.failed","response_id":"next","error":{"code":"rejected"}})));
        assert!(
            !state.terminal_seen,
            "control failure must leave the generation active"
        );
        assert!(!state
            .request(other_connection, "next", now, INTERRUPT_ACK_TIMEOUT)
            .unwrap());
    }
}
