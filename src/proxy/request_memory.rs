use std::fmt;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use serde_json::Value;

use super::ProxyError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestMemoryComponent {
    InboundRaw,
    DecodedBody,
    NormalizedBody,
    TransportPending,
    SemanticPrelude,
    StreamRetainedState,
    ToolArguments,
    NormalizedEvent,
    WebSocketReadQueue,
    WebSocketWriteQueue,
    ReasoningReplay,
    GroundingCitation,
}

impl RequestMemoryComponent {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::InboundRaw => "inbound_raw",
            Self::DecodedBody => "decoded_body",
            Self::NormalizedBody => "normalized_body",
            Self::TransportPending => "transport_pending",
            Self::SemanticPrelude => "semantic_prelude",
            Self::StreamRetainedState => "stream_retained_state",
            Self::ToolArguments => "tool_arguments",
            Self::NormalizedEvent => "normalized_event",
            Self::WebSocketReadQueue => "websocket_read_queue",
            Self::WebSocketWriteQueue => "websocket_write_queue",
            Self::ReasoningReplay => "reasoning_replay",
            Self::GroundingCitation => "grounding_citation",
        }
    }
}

/// Returns a conservative allocation-sized estimate without serializing the
/// value. This is used for request-scoped JSON state that survives between
/// stream chunks; transient parse/output values are accounted separately.
pub(super) fn retained_json_bytes(value: &Value) -> usize {
    fn dynamic_bytes(value: &Value) -> usize {
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => 0,
            Value::String(value) => value.capacity(),
            Value::Array(values) => values
                .capacity()
                .saturating_mul(std::mem::size_of::<Value>())
                .saturating_add(values.iter().map(dynamic_bytes).sum::<usize>()),
            Value::Object(values) => values.iter().fold(
                values.len().saturating_mul(
                    std::mem::size_of::<String>()
                        .saturating_add(std::mem::size_of::<Value>())
                        .saturating_add(std::mem::size_of::<usize>() * 3),
                ),
                |bytes, (key, value)| {
                    bytes
                        .saturating_add(key.capacity())
                        .saturating_add(dynamic_bytes(value))
                },
            ),
        }
    }

    std::mem::size_of::<Value>().saturating_add(dynamic_bytes(value))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RequestMemorySnapshot {
    pub(super) limit_bytes: usize,
    pub(super) used_bytes: usize,
    pub(super) high_water_bytes: usize,
    pub(super) active_reservations: usize,
    pub(super) exhausted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RequestMemoryError {
    component: RequestMemoryComponent,
    requested_bytes: usize,
    used_bytes: usize,
    limit_bytes: usize,
}

impl RequestMemoryError {
    pub(super) fn into_proxy_error(self) -> ProxyError {
        tracing::warn!(
            component = self.component.as_str(),
            requested_bytes = self.requested_bytes,
            used_bytes = self.used_bytes,
            limit_bytes = self.limit_bytes,
            "request resident-memory budget exhausted"
        );
        ProxyError::request_memory_exhausted()
    }
}

impl fmt::Display for RequestMemoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "request memory budget exhausted for {}: requested {} bytes with {}/{} bytes resident",
            self.component.as_str(),
            self.requested_bytes,
            self.used_bytes,
            self.limit_bytes,
        )
    }
}

impl std::error::Error for RequestMemoryError {}

#[derive(Clone)]
pub(crate) struct RequestMemoryBudget {
    inner: Arc<RequestMemoryBudgetInner>,
}

struct RequestMemoryBudgetInner {
    state: Mutex<RequestMemoryState>,
}

#[derive(Debug)]
struct RequestMemoryState {
    limit_bytes: usize,
    used_bytes: usize,
    high_water_bytes: usize,
    active_reservations: usize,
    exhausted: bool,
}

impl fmt::Debug for RequestMemoryBudget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestMemoryBudget")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

impl RequestMemoryBudget {
    pub(super) fn new(limit_bytes: usize) -> Self {
        Self {
            inner: Arc::new(RequestMemoryBudgetInner {
                state: Mutex::new(RequestMemoryState {
                    limit_bytes: limit_bytes.max(1),
                    used_bytes: 0,
                    high_water_bytes: 0,
                    active_reservations: 0,
                    exhausted: false,
                }),
            }),
        }
    }

    pub(super) fn reserve(
        &self,
        component: RequestMemoryComponent,
        bytes: usize,
    ) -> Result<RequestMemoryReservation, RequestMemoryError> {
        let reservation = RequestMemoryReservation {
            inner: Arc::new(RequestMemoryReservationInner {
                budget: self.clone(),
                component,
                state: Mutex::new(ReservationState {
                    bytes: 0,
                    released: false,
                }),
            }),
        };
        reservation.resize(bytes)?;
        Ok(reservation)
    }

    pub(super) fn snapshot(&self) -> RequestMemorySnapshot {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        RequestMemorySnapshot {
            limit_bytes: state.limit_bytes,
            used_bytes: state.used_bytes,
            high_water_bytes: state.high_water_bytes,
            active_reservations: state.active_reservations,
            exhausted: state.exhausted,
        }
    }

    pub(super) fn is_exhausted(&self) -> bool {
        self.snapshot().exhausted
    }

    pub(super) fn remaining_bytes(&self) -> usize {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.exhausted {
            0
        } else {
            state.limit_bytes.saturating_sub(state.used_bytes)
        }
    }

    pub(super) fn reject(
        &self,
        component: RequestMemoryComponent,
        requested_bytes: usize,
    ) -> RequestMemoryError {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.exhausted = true;
        let requested_bytes = requested_bytes.max(1);
        crate::metrics::record_request_memory_reservation(
            component.as_str(),
            requested_bytes,
            "rejected",
        );
        RequestMemoryError {
            component,
            requested_bytes,
            used_bytes: state.used_bytes,
            limit_bytes: state.limit_bytes,
        }
    }

    /// Binds a reservation to the allocation backing `bytes`. Every clone or
    /// slice of the returned `Bytes` keeps the reservation alive until the
    /// final view is dropped, which matches downstream HTTP body ownership.
    pub(super) fn retain_bytes(
        &self,
        component: RequestMemoryComponent,
        bytes: Bytes,
    ) -> Result<Bytes, RequestMemoryError> {
        if bytes.is_empty() {
            return Ok(bytes);
        }
        let reservation = self.reserve(component, bytes.len())?;
        Ok(Bytes::from_owner(RequestMemoryBytes {
            bytes,
            _reservation: reservation,
        }))
    }

    fn resize(
        &self,
        component: RequestMemoryComponent,
        current: usize,
        next: usize,
    ) -> Result<(), RequestMemoryError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if next <= current {
            let released = current - next;
            state.used_bytes = state.used_bytes.saturating_sub(released);
            if current > 0 && next == 0 {
                state.active_reservations = state.active_reservations.saturating_sub(1);
            }
            return Ok(());
        }

        let additional = next - current;
        if state.exhausted || additional > state.limit_bytes.saturating_sub(state.used_bytes) {
            state.exhausted = true;
            crate::metrics::record_request_memory_reservation(
                component.as_str(),
                additional,
                "rejected",
            );
            return Err(RequestMemoryError {
                component,
                requested_bytes: additional,
                used_bytes: state.used_bytes,
                limit_bytes: state.limit_bytes,
            });
        }

        if current == 0 {
            state.active_reservations = state.active_reservations.saturating_add(1);
        }
        state.used_bytes = state.used_bytes.saturating_add(additional);
        state.high_water_bytes = state.high_water_bytes.max(state.used_bytes);
        crate::metrics::record_request_memory_reservation(
            component.as_str(),
            additional,
            "reserved",
        );
        Ok(())
    }
}

struct RequestMemoryBytes {
    bytes: Bytes,
    _reservation: RequestMemoryReservation,
}

impl AsRef<[u8]> for RequestMemoryBytes {
    fn as_ref(&self) -> &[u8] {
        self.bytes.as_ref()
    }
}

impl Drop for RequestMemoryBudgetInner {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::metrics::record_request_memory_high_water(
            state.high_water_bytes,
            state.limit_bytes,
            state.exhausted,
        );
    }
}

#[derive(Clone)]
pub(crate) struct RequestMemoryReservation {
    inner: Arc<RequestMemoryReservationInner>,
}

struct RequestMemoryReservationInner {
    budget: RequestMemoryBudget,
    component: RequestMemoryComponent,
    state: Mutex<ReservationState>,
}

#[derive(Debug)]
struct ReservationState {
    bytes: usize,
    released: bool,
}

impl fmt::Debug for RequestMemoryReservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        formatter
            .debug_struct("RequestMemoryReservation")
            .field("component", &self.inner.component)
            .field("bytes", &state.bytes)
            .field("released", &state.released)
            .finish()
    }
}

impl RequestMemoryReservation {
    pub(super) fn resize(&self, bytes: usize) -> Result<(), RequestMemoryError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.released {
            return Ok(());
        }
        self.inner
            .budget
            .resize(self.inner.component, state.bytes, bytes)?;
        state.bytes = bytes;
        Ok(())
    }

    pub(super) fn bytes(&self) -> usize {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .bytes
    }

    pub(crate) fn budget(&self) -> RequestMemoryBudget {
        self.inner.budget.clone()
    }

    /// Transfers this reservation to the allocation backing `bytes`. This is
    /// used when an already-accounted transport buffer becomes the response
    /// body without allocating a decoded copy.
    pub(super) fn retain_bytes(self, bytes: Bytes) -> Bytes {
        if bytes.is_empty() {
            return bytes;
        }
        Bytes::from_owner(RequestMemoryBytes {
            bytes,
            _reservation: self,
        })
    }

    pub(super) fn release(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.released {
            return;
        }
        let _ = self
            .inner
            .budget
            .resize(self.inner.component, state.bytes, 0);
        state.bytes = 0;
        state.released = true;
    }
}

impl Drop for RequestMemoryReservationInner {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.released || state.bytes == 0 {
            return;
        }
        let _ = self.budget.resize(self.component, state.bytes, 0);
        state.bytes = 0;
        state.released = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_budget_accounts_resizes_and_releases_once_across_clones() {
        let budget = RequestMemoryBudget::new(100);
        let raw = budget
            .reserve(RequestMemoryComponent::InboundRaw, 40)
            .unwrap();
        let raw_clone = raw.clone();
        let normalized = budget
            .reserve(RequestMemoryComponent::NormalizedBody, 30)
            .unwrap();
        assert_eq!(budget.snapshot().used_bytes, 70);
        assert_eq!(budget.snapshot().active_reservations, 2);

        normalized.resize(10).unwrap();
        assert_eq!(budget.snapshot().used_bytes, 50);
        drop(raw);
        assert_eq!(budget.snapshot().used_bytes, 50);
        drop(raw_clone);
        assert_eq!(budget.snapshot().used_bytes, 10);
        normalized.release();
        normalized.release();
        assert_eq!(budget.snapshot().used_bytes, 0);
        assert_eq!(budget.snapshot().active_reservations, 0);
        assert_eq!(budget.snapshot().high_water_bytes, 70);
    }

    #[test]
    fn exhaustion_is_sticky_but_release_always_returns_capacity() {
        let budget = RequestMemoryBudget::new(64);
        let raw = budget
            .reserve(RequestMemoryComponent::InboundRaw, 48)
            .unwrap();
        let error = budget
            .reserve(RequestMemoryComponent::DecodedBody, 17)
            .unwrap_err();
        assert_eq!(error.component, RequestMemoryComponent::DecodedBody);
        assert!(budget.is_exhausted());
        assert!(budget
            .reserve(RequestMemoryComponent::NormalizedBody, 1)
            .is_err());
        drop(raw);
        assert_eq!(budget.snapshot().used_bytes, 0);
        assert!(budget.is_exhausted());
    }

    #[test]
    fn exhaustion_maps_to_a_stable_capacity_error_without_content() {
        let budget = RequestMemoryBudget::new(8);
        let error = budget
            .reserve(RequestMemoryComponent::ToolArguments, 9)
            .unwrap_err()
            .into_proxy_error();
        assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.error_code(), "cc_switch_request_memory_exhausted");
        assert_eq!(error.retry_after_seconds(), Some(1));
        assert_eq!(
            error.client_message(),
            "Request resident-memory capacity exhausted; retry later"
        );
    }

    #[test]
    fn retained_bytes_release_only_after_the_last_view_is_dropped() {
        let budget = RequestMemoryBudget::new(32);
        let retained = budget
            .retain_bytes(
                RequestMemoryComponent::NormalizedEvent,
                Bytes::from_static(b"response"),
            )
            .unwrap();
        let view = retained.slice(0..4);
        assert_eq!(budget.snapshot().used_bytes, 8);

        drop(retained);
        assert_eq!(budget.snapshot().used_bytes, 8);
        drop(view);
        assert_eq!(budget.snapshot().used_bytes, 0);
    }

    #[test]
    fn existing_reservation_can_follow_the_accounted_bytes() {
        let budget = RequestMemoryBudget::new(32);
        let reservation = budget
            .reserve(RequestMemoryComponent::TransportPending, 8)
            .unwrap();
        let retained = reservation.retain_bytes(Bytes::from_static(b"response"));
        let retained_clone = retained.clone();
        assert_eq!(budget.snapshot().used_bytes, 8);
        drop(retained);
        assert_eq!(budget.snapshot().used_bytes, 8);
        drop(retained_clone);
        assert_eq!(budget.snapshot().used_bytes, 0);
    }

    #[test]
    fn explicit_rejection_is_sticky_and_reports_only_sizes() {
        let budget = RequestMemoryBudget::new(16);
        let error = budget.reject(RequestMemoryComponent::DecodedBody, 17);
        assert_eq!(error.component, RequestMemoryComponent::DecodedBody);
        assert_eq!(error.requested_bytes, 17);
        assert!(budget.is_exhausted());
        assert_eq!(budget.remaining_bytes(), 0);
        assert!(budget
            .reserve(RequestMemoryComponent::NormalizedBody, 1)
            .is_err());
    }
}
