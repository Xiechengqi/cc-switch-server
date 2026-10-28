//! Monotonic observations for one fixed Provider request and its attempts.
//!
//! These clocks never decide retries, usage, deadlines, or provider health.
//! HTTP body handoff and local WebSocket sink completion are different events;
//! neither proves that a remote client consumed the response. The final memory
//! owner reports release independently of the semantic terminal and delivery.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::response::Response;
use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};

use super::response_semantics::SemanticObservation;

#[derive(Clone, Copy, Debug)]
pub(super) enum Milestone {
    Admission,
    SendStarted,
    Headers,
    FirstSemantic,
    HttpBodyHandoff,
    UpstreamWriteCompleted,
    DownstreamWriteCompleted,
    Terminal,
    OwnerReleased,
}

impl Milestone {
    fn as_str(self) -> &'static str {
        match self {
            Self::Admission => "admission",
            Self::SendStarted => "send_started",
            Self::Headers => "headers",
            Self::FirstSemantic => "first_semantic",
            Self::HttpBodyHandoff => "http_body_handoff",
            Self::UpstreamWriteCompleted => "upstream_write_completed",
            Self::DownstreamWriteCompleted => "downstream_write_completed",
            Self::Terminal => "terminal",
            Self::OwnerReleased => "owner_released",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum WaitKind {
    Retry,
    Headers,
    UpstreamRead,
    UpstreamWrite,
    DownstreamWrite,
    DownstreamDemand,
    CancelDrain,
}

impl WaitKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Retry => "retry",
            Self::Headers => "headers",
            Self::UpstreamRead => "upstream_read",
            Self::UpstreamWrite => "upstream_write",
            Self::DownstreamWrite => "downstream_write",
            Self::DownstreamDemand => "downstream_demand",
            Self::CancelDrain => "cancel_drain",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TimingResult {
    Complete,
    Incomplete,
    Failed,
    Cancelled,
    Retried,
    Unfinished,
}

impl TimingResult {
    fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Retried => "retried",
            Self::Unfinished => "unfinished",
        }
    }
}

#[derive(Debug, Default)]
struct TimingState {
    milestones: [Option<Duration>; 9],
    waits: [Duration; 7],
    semantic_result: Option<TimingResult>,
    incomplete_hint: bool,
    semantic_terminal_at: Option<Instant>,
    result: Option<TimingResult>,
    cancelled: Option<Instant>,
}

#[derive(Debug)]
struct Timeline {
    started: Instant,
    provider_type: &'static str,
    surface: &'static str,
    scope: &'static str,
    state: Mutex<TimingState>,
}

impl Timeline {
    fn new(
        started: Instant,
        provider_type: &'static str,
        surface: &'static str,
        scope: &'static str,
    ) -> Self {
        Self {
            started,
            provider_type,
            surface,
            scope,
            state: Mutex::new(TimingState::default()),
        }
    }

    fn mark(&self, phase: Milestone) {
        self.mark_at(phase, Instant::now());
    }

    fn mark_at(&self, phase: Milestone, at: Instant) {
        let elapsed = at.saturating_duration_since(self.started);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let slot = &mut state.milestones[phase as usize];
        if slot.is_none() {
            *slot = Some(elapsed);
            crate::metrics::record_forward_milestone(
                self.provider_type,
                self.surface,
                self.scope,
                phase.as_str(),
                elapsed,
            );
        }
    }

    fn wait(&self, phase: WaitKind, elapsed: Duration) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.waits[phase as usize] = state.waits[phase as usize].saturating_add(elapsed);
        crate::metrics::record_forward_wait(
            self.provider_type,
            self.surface,
            self.scope,
            phase.as_str(),
            elapsed,
        );
    }

    fn finish(&self, result: TimingResult) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.result.is_none() {
            if result == TimingResult::Cancelled {
                state.cancelled = Some(Instant::now());
            }
            state.result = Some(result);
            crate::metrics::record_forward_timeline(
                self.provider_type,
                self.surface,
                self.scope,
                result.as_str(),
                self.started.elapsed(),
            );
        }
    }
}

impl Drop for Timeline {
    fn drop(&mut self) {
        // An uninstrumented exceptional exit stays explicit instead of being
        // promoted to a successful delivery or an inferred cancellation.
        self.finish(TimingResult::Unfinished);
    }
}

/// Clones share the forwarding entry origin across all subsequent retries.
#[derive(Clone, Debug)]
pub(super) struct TimingSlot {
    started: Instant,
    request: Arc<OnceLock<RequestTiming>>,
    current: Arc<Mutex<Option<ForwardTiming>>>,
}

impl Default for TimingSlot {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            request: Arc::new(OnceLock::new()),
            current: Arc::new(Mutex::new(None)),
        }
    }
}

impl TimingSlot {
    pub(super) fn begin(&self, surface: &'static str, started: Instant) -> ForwardTiming {
        self.begin_for_provider(
            crate::domain::providers::model::ProviderType::CodexOAuth,
            surface,
            started,
        )
        .expect("Codex timing is a declared Provider")
    }

    pub(super) fn begin_for_provider(
        &self,
        provider_type: crate::domain::providers::model::ProviderType,
        surface: &'static str,
        started: Instant,
    ) -> Option<ForwardTiming> {
        use crate::domain::providers::model::ProviderType;
        let provider_type = match provider_type {
            ProviderType::CodexOAuth => "codex_oauth",
            ProviderType::ClaudeOAuth => "claude_oauth",
            ProviderType::AntigravityOAuth => "antigravity_oauth",
            ProviderType::AgyOAuth => "agy_oauth",
            ProviderType::CursorOAuth => "cursor_oauth",
            ProviderType::CursorApiKey => "cursor_apikey",
            ProviderType::GrokOAuth => "grok_oauth",
            ProviderType::KiroOAuth => "kiro_oauth",
            ProviderType::AmazonQOAuth => "amazon_q_oauth",
            ProviderType::QoderCosy => "qoder_cosy",
            ProviderType::CodeBuddyOAuth => "codebuddy_oauth",
            _ => return None,
        };
        let request = self
            .request
            .get_or_init(|| RequestTiming::for_provider(self.started, provider_type, surface));
        // A request's original label cannot be rewritten by another binding.
        if request.0.provider_type != provider_type || request.0.surface != surface {
            return None;
        }
        let next = request.attempt(started);
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(previous) = current.replace(next.clone()) {
            previous.finish_attempt(TimingResult::Retried);
        }
        Some(next)
    }

    pub(super) fn run<F: Future>(&self, future: F) -> impl Future<Output = F::Output> {
        TimedForward {
            inner: Box::pin(future),
            slot: self.clone(),
            finished: false,
        }
    }

    pub(super) fn get(&self) -> Option<ForwardTiming> {
        self.current
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// A WS connection carries independent requests; keep only the current
    /// attempt here, without retaining a turn's request-memory budget.
    pub(super) fn adopt_websocket_attempt(&self, timing: ForwardTiming) {
        if timing.request.0.surface == "responses_websocket" {
            *self
                .current
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(timing);
        }
    }

    pub(super) fn response(&self, response: Response) -> Response {
        match self.get() {
            Some(timing) => timing.response_with_slot(response, Some(self.clone())),
            None => response,
        }
    }
}

// Drop runs before the pending forwarding future and its memory owners are
// released, so cancellation-to-release uses the actual cancellation boundary.
struct TimedForward<F: Future> {
    inner: Pin<Box<F>>,
    slot: TimingSlot,
    finished: bool,
}

impl<F: Future> Future for TimedForward<F> {
    type Output = F::Output;
    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let result = this.inner.as_mut().poll(context);
        if result.is_ready() {
            this.finished = true;
        }
        result
    }
}

impl<F: Future> Drop for TimedForward<F> {
    fn drop(&mut self) {
        if !self.finished {
            if let Some(timing) = self.slot.get() {
                timing.finish(TimingResult::Cancelled);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct RequestTiming(Arc<Timeline>);

impl RequestTiming {
    pub(super) fn new(started: Instant, surface: &'static str) -> Self {
        Self::for_provider(started, "codex_oauth", surface)
    }

    pub(super) fn responses_websocket(
        started: Instant,
        provider_type: crate::domain::providers::model::ProviderType,
    ) -> Option<Self> {
        use crate::domain::providers::model::ProviderType;
        let label = match provider_type {
            ProviderType::CodexOAuth => "codex_oauth",
            ProviderType::GrokOAuth => "grok_oauth",
            _ => return None,
        };
        Some(Self::for_provider(started, label, "responses_websocket"))
    }

    fn for_provider(started: Instant, provider_type: &'static str, surface: &'static str) -> Self {
        Self(Arc::new(Timeline::new(
            started,
            provider_type,
            surface,
            "request",
        )))
    }

    pub(super) fn attempt(&self, started: Instant) -> ForwardTiming {
        ForwardTiming {
            request: self.clone(),
            attempt: Arc::new(Timeline::new(
                started,
                self.0.provider_type,
                self.0.surface,
                "attempt",
            )),
        }
    }

    pub(super) fn finish(&self, result: TimingResult) {
        self.0.finish(result);
    }

    pub(super) fn owner_released(&self) {
        self.0.mark(Milestone::OwnerReleased);
        let cancelled = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .cancelled;
        if let Some(cancelled) = cancelled {
            self.0.wait(WaitKind::CancelDrain, cancelled.elapsed());
        }
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> serde_json::Value {
        let state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let phases = [
            Milestone::Admission,
            Milestone::SendStarted,
            Milestone::Headers,
            Milestone::FirstSemantic,
            Milestone::HttpBodyHandoff,
            Milestone::UpstreamWriteCompleted,
            Milestone::DownstreamWriteCompleted,
            Milestone::Terminal,
            Milestone::OwnerReleased,
        ];
        let waits = [
            WaitKind::Retry,
            WaitKind::Headers,
            WaitKind::UpstreamRead,
            WaitKind::UpstreamWrite,
            WaitKind::DownstreamWrite,
            WaitKind::DownstreamDemand,
            WaitKind::CancelDrain,
        ];
        serde_json::json!({
            "providerType": self.0.provider_type,
            "surface": self.0.surface,
            "milestonesMs": phases.into_iter().filter_map(|phase| state.milestones[phase as usize]
                .map(|duration| (phase.as_str(), duration.as_secs_f64() * 1000.0))).collect::<std::collections::BTreeMap<_, _>>(),
            "waitsMs": waits.into_iter().map(|phase| (phase.as_str(), state.waits[phase as usize].as_secs_f64() * 1000.0))
                .collect::<std::collections::BTreeMap<_, _>>(),
            "result": state.result.map(TimingResult::as_str),
        })
    }
}

/// An operation holds its original attempt even if a later retry replaces it.
#[derive(Clone, Debug)]
pub(super) struct ForwardTiming {
    request: RequestTiming,
    attempt: Arc<Timeline>,
}

impl ForwardTiming {
    pub(super) fn surface(&self) -> &'static str {
        self.request.0.surface
    }

    pub(super) fn is_grok(&self) -> bool {
        self.request.0.provider_type == "grok_oauth"
    }

    /// Observe the already decoded Responses value. Grok's private control
    /// and billing frames are filtered later in the existing wire pipeline;
    /// they must not become first content here. This does not change commit,
    /// replay, visibility, usage, or any protocol classifier.
    pub(super) fn observe_responses_value(
        &self,
        value: &serde_json::Value,
        declared_event: Option<&str>,
        observation: &SemanticObservation,
    ) {
        use serde_json::Value;
        if !self.is_grok() {
            self.observe(observation);
            return;
        }
        let kind = value.get("type").and_then(Value::as_str);
        let content = match observation {
            SemanticObservation::Business => {
                let control = matches!(
                    kind,
                    Some("ping" | "keepalive" | "response.doom_loop_check")
                ) || (matches!(declared_event, Some("ping" | "keepalive"))
                    && kind.is_none());
                if control {
                    false
                } else if kind.is_some_and(|kind| kind.ends_with(".delta")) {
                    value.get("delta").is_some_and(|delta| {
                        delta.as_str().is_some_and(|text| !text.is_empty())
                            || delta.as_object().is_some_and(|object| !object.is_empty())
                    })
                } else if kind.is_some() {
                    // Unknown business events retain the existing conservative
                    // activity contract; only reviewed control is excluded.
                    true
                } else {
                    responses_document_has_content(value)
                }
            }
            SemanticObservation::SuccessTerminal | SemanticObservation::IncompleteTerminal => {
                responses_document_has_content(value.get("response").unwrap_or(value))
            }
            _ => false,
        };
        if content {
            self.mark(Milestone::FirstSemantic);
        }
        if !matches!(observation, SemanticObservation::Business) {
            self.observe(observation);
        }
    }

    pub(super) fn observe_responses_document(
        &self,
        value: &serde_json::Value,
        observation: &SemanticObservation,
    ) {
        self.observe_responses_value(value, None, observation);
        if self.is_grok() && matches!(observation, SemanticObservation::ErrorFrame(_)) {
            // A full JSON document has no later frame that can resolve an
            // error. This remains observation-only when the guard is off.
            self.observe_terminal(TimingResult::Failed);
        }
    }

    pub(super) fn mark(&self, phase: Milestone) {
        self.request.0.mark(phase);
        self.attempt.mark(phase);
    }

    pub(super) fn observe(&self, observation: &SemanticObservation) {
        let result = match observation {
            SemanticObservation::Business => {
                self.mark(Milestone::FirstSemantic);
                None
            }
            SemanticObservation::SuccessTerminal => Some(TimingResult::Complete),
            SemanticObservation::IncompleteTerminal => Some(TimingResult::Incomplete),
            SemanticObservation::Failure(_) => Some(TimingResult::Failed),
            _ => None,
        };
        if let Some(result) = result {
            self.observe_terminal(result);
        }
    }

    pub(super) fn observe_terminal(&self, result: TimingResult) {
        let incomplete = self
            .attempt
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .incomplete_hint;
        let result = if result == TimingResult::Complete && incomplete {
            TimingResult::Incomplete
        } else {
            result
        };
        self.attempt.mark(Milestone::Terminal);
        for timeline in [&self.request.0, &self.attempt] {
            let mut state = timeline
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.semantic_result = Some(result);
            state.semantic_terminal_at.get_or_insert_with(Instant::now);
        }
    }

    /// EOF can complete a media HTTP response without replacing an observed
    /// provider failure. It says nothing about completion of a video job.
    pub(super) fn observe_media_eof(&self) {
        let terminal = self
            .attempt
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .semantic_result;
        if terminal.is_none() {
            self.observe_terminal(TimingResult::Complete);
        }
    }

    /// This is narrower than the existing Anthropic commit classifier: empty
    /// block boundaries, stop reasons, usage, signatures and ping are not first
    /// semantic content. It does not change commit, timeout or retry decisions.
    pub(super) fn observe_anthropic_value(&self, value: &serde_json::Value) {
        use serde_json::Value;
        let stop_reason = value
            .get("stop_reason")
            .or_else(|| value.pointer("/delta/stop_reason"))
            .or_else(|| value.pointer("/message/stop_reason"))
            .and_then(Value::as_str);
        if matches!(
            stop_reason,
            Some("max_tokens" | "pause_turn" | "model_context_window_exceeded")
        ) {
            self.attempt
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .incomplete_hint = true;
        }
        let nonempty = |value: Option<&Value>| {
            value
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty())
        };
        let identifier = |value: Option<&Value>| {
            value
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty())
        };
        let block_has_content = |block: &Value| match block.get("type").and_then(Value::as_str) {
            Some("text") => nonempty(block.get("text")),
            Some("thinking") => nonempty(block.get("thinking")),
            Some("redacted_thinking") => identifier(block.get("data")),
            Some("tool_use" | "server_tool_use") => {
                identifier(block.get("id"))
                    && identifier(block.get("name"))
                    && block.get("input").is_some_and(Value::is_object)
            }
            _ => false,
        };
        let content = match value.get("type").and_then(Value::as_str) {
            Some("message") => value
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|blocks| blocks.iter().any(block_has_content)),
            Some("message_start") => value
                .pointer("/message/content")
                .and_then(Value::as_array)
                .is_some_and(|blocks| blocks.iter().any(block_has_content)),
            Some("content_block_start") => {
                value.get("content_block").is_some_and(block_has_content)
            }
            Some("content_block_delta") => {
                match value.pointer("/delta/type").and_then(Value::as_str) {
                    Some("text_delta") => nonempty(value.pointer("/delta/text")),
                    Some("thinking_delta") => nonempty(value.pointer("/delta/thinking")),
                    Some("input_json_delta") => nonempty(value.pointer("/delta/partial_json")),
                    _ => false,
                }
            }
            _ => false,
        };
        if content {
            self.mark(Milestone::FirstSemantic);
        }
    }

    /// Observe an existing canonical Chat value without treating usage, roles,
    /// empty tool placeholders, or finish reasons as first content. A length
    /// hint affects timing only; the protocol owner still validates completion.
    pub(super) fn observe_chat_value(&self, value: &serde_json::Value) {
        use serde_json::Value;
        let Some(choices) = value.get("choices").and_then(Value::as_array) else {
            return;
        };
        for choice in choices {
            if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
                self.attempt
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .incomplete_hint = true;
            }
            let Some(delta) = choice.get("delta").or_else(|| choice.get("message")) else {
                continue;
            };
            let nonempty = |value: Option<&Value>| {
                value
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
            };
            let content = [
                "content",
                "refusal",
                "reasoning_content",
                "reasoning_text",
                "reasoning",
            ]
            .iter()
            .any(|field| nonempty(delta.get(*field)))
                || delta
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .is_some_and(|calls| {
                        calls.iter().any(|call| {
                            nonempty(call.get("id"))
                                || nonempty(call.pointer("/function/name"))
                                || nonempty(call.pointer("/function/arguments"))
                        })
                    });
            if content {
                self.mark(Milestone::FirstSemantic);
            }
        }
    }

    /// Observe decoded Gemini content without changing the existing commit or
    /// terminal contract. Empty signature carriers and metadata are excluded.
    pub(super) fn observe_gemini_value(&self, value: &serde_json::Value) {
        use serde_json::Value;
        if gemini_has_error(value) {
            return;
        }
        let response = value.get("response").unwrap_or(value);
        let Some(candidates) = response.get("candidates").and_then(Value::as_array) else {
            return;
        };
        if candidates.iter().any(|candidate| {
            candidate
                .get("finishReason")
                .or_else(|| candidate.get("finish_reason"))
                .and_then(Value::as_str)
                == Some("MAX_TOKENS")
        }) {
            self.attempt
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .incomplete_hint = true;
        }
        let text = |value: Option<&Value>| {
            value
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty())
        };
        let identifier = |value: Option<&Value>| {
            value
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty())
        };
        let content = candidates.iter().any(|candidate| {
            candidate
                .pointer("/content/parts")
                .and_then(Value::as_array)
                .is_some_and(|parts| {
                    parts.iter().any(|part| {
                        text(part.get("text"))
                            || part.get("functionCall").is_some_and(|call| {
                                identifier(call.get("name"))
                                    && call.get("args").is_none_or(Value::is_object)
                            })
                            || (identifier(part.pointer("/inlineData/mimeType"))
                                && text(part.pointer("/inlineData/data")))
                            || identifier(part.pointer("/fileData/fileUri"))
                            || text(part.pointer("/executableCode/code"))
                            || text(part.pointer("/codeExecutionResult/output"))
                    })
                })
        });
        if content {
            self.mark(Milestone::FirstSemantic);
        }
    }

    /// Called on the document already parsed by the existing error check, or
    /// after successful SSE aggregation. It never rejects or rewrites a body.
    pub(super) fn observe_gemini_document(&self, value: &serde_json::Value, count_tokens: bool) {
        if gemini_has_error(value) {
            self.observe_terminal(TimingResult::Failed);
            return;
        }
        if count_tokens {
            if value
                .get("totalTokens")
                .and_then(serde_json::Value::as_u64)
                .is_some()
            {
                self.mark(Milestone::FirstSemantic);
                self.observe_terminal(TimingResult::Complete);
            }
            return;
        }
        self.observe_gemini_value(value);
        if super::terminal_detector::gemini_terminal(value) {
            self.observe_terminal(TimingResult::Complete);
        }
    }

    pub(super) fn wait(&self, phase: WaitKind) -> TimingWait {
        TimingWait {
            timing: self.clone(),
            phase,
            started: Instant::now(),
        }
    }

    fn record_wait(&self, phase: WaitKind, elapsed: Duration) {
        self.request.0.wait(phase, elapsed);
        self.attempt.wait(phase, elapsed);
    }

    pub(super) fn finish_attempt(&self, result: TimingResult) {
        self.attempt.finish(result);
    }

    pub(super) fn finish(&self, result: TimingResult) {
        let terminal = self
            .attempt
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .semantic_terminal_at;
        if let Some(terminal) = terminal {
            self.request.0.mark_at(Milestone::Terminal, terminal);
        }
        self.finish_attempt(result);
        self.request.finish(result);
    }

    pub(super) fn finish_delivery(&self, status_success: bool) {
        let result = if status_success {
            self.attempt
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .semantic_result
                .unwrap_or(TimingResult::Unfinished)
        } else {
            TimingResult::Failed
        };
        self.finish(result);
    }

    pub(super) fn finish_websocket_exit(&self, failed: bool) {
        if self
            .request
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .result
            .is_some()
        {
            return;
        }
        if failed {
            self.observe_terminal(TimingResult::Failed);
            self.finish(TimingResult::Failed);
        } else {
            self.finish(TimingResult::Cancelled);
        }
    }

    pub(super) fn request(&self) -> RequestTiming {
        self.request.clone()
    }

    #[cfg(test)]
    pub(super) fn attempt_snapshot(&self) -> serde_json::Value {
        RequestTiming(self.attempt.clone()).snapshot()
    }

    #[cfg(test)]
    pub(super) fn response(&self, response: Response) -> Response {
        self.response_with_slot(response, None)
    }

    fn response_with_slot(&self, response: Response, slot: Option<TimingSlot>) -> Response {
        let (parts, body) = response.into_parts();
        let success = parts.status.is_success();
        let ended = body.is_end_stream();
        if ended {
            self.finish_delivery(success);
        }
        Response::from_parts(
            parts,
            Body::new(TimedBody {
                inner: Box::pin(body),
                timing: self.clone(),
                slot,
                success,
                ended,
                handed_off: None,
            }),
        )
    }
}

pub(super) struct TimingWait {
    timing: ForwardTiming,
    phase: WaitKind,
    started: Instant,
}

fn gemini_has_error(value: &serde_json::Value) -> bool {
    [value.get("error"), value.pointer("/response/error")]
        .into_iter()
        .flatten()
        .any(super::response_semantics::error_value_is_substantive)
}

fn responses_document_has_content(value: &serde_json::Value) -> bool {
    use serde_json::Value;
    fn nonempty(value: Option<&Value>) -> bool {
        value
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    }
    value
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                nonempty(item.get("encrypted_content"))
                    || nonempty(item.get("result"))
                    || (nonempty(item.get("name"))
                        && (nonempty(item.get("arguments")) || nonempty(item.get("input"))))
                    || ["content", "summary"].iter().any(|field| {
                        item.get(field)
                            .and_then(Value::as_array)
                            .is_some_and(|parts| {
                                parts.iter().any(|part| {
                                    nonempty(part.get("text")) || nonempty(part.get("refusal"))
                                })
                            })
                    })
            })
        })
}

impl Drop for TimingWait {
    fn drop(&mut self) {
        self.timing.record_wait(self.phase, self.started.elapsed());
    }
}

struct TimedBody {
    inner: Pin<Box<Body>>,
    timing: ForwardTiming,
    slot: Option<TimingSlot>,
    success: bool,
    ended: bool,
    handed_off: Option<(Instant, ForwardTiming)>,
}

impl TimedBody {
    fn current_timing(&self) -> ForwardTiming {
        self.slot
            .as_ref()
            .and_then(TimingSlot::get)
            .unwrap_or_else(|| self.timing.clone())
    }
}

impl HttpBody for TimedBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        let this = self.get_mut();
        if let Some((handed_off, timing)) = this.handed_off.take() {
            // Time until the next consumer poll includes transport scheduling;
            // it is demand latency, never a successful socket-write duration.
            timing.record_wait(WaitKind::DownstreamDemand, handed_off.elapsed());
        }
        let result = this.inner.as_mut().poll_frame(context);
        // Buffered drivers can start another upstream attempt while this body
        // is being polled. Delivery belongs to the attempt that produced it.
        let timing = this.current_timing();
        match &result {
            Poll::Ready(Some(Ok(frame))) => {
                if frame.data_ref().is_some_and(|bytes| !bytes.is_empty()) {
                    timing.mark(Milestone::HttpBodyHandoff);
                    this.handed_off = Some((Instant::now(), timing.clone()));
                }
                if this.inner.is_end_stream() {
                    this.ended = true;
                    timing.finish_delivery(this.success);
                }
            }
            Poll::Ready(None) => {
                this.ended = true;
                timing.finish_delivery(this.success);
            }
            Poll::Ready(Some(Err(_))) => {
                this.ended = true;
                timing.finish(TimingResult::Failed);
            }
            Poll::Pending => {}
        }
        result
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for TimedBody {
    fn drop(&mut self) {
        if !self.ended {
            self.current_timing().finish(TimingResult::Cancelled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::request_memory::{RequestMemoryBudget, RequestMemoryComponent};
    use super::super::response_semantics::ResponsesSseInspector;
    use super::*;
    use futures_util::StreamExt;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn cursor_core03_buffered_retry_delivery_and_cancellation_follow_current_attempt() {
        use crate::domain::providers::model::ProviderType;
        for cancel in [false, true] {
            let slot = TimingSlot::default();
            let first = slot
                .begin_for_provider(ProviderType::CursorApiKey, "responses", Instant::now())
                .unwrap();
            let inner_slot = slot.clone();
            let stream = async_stream::stream! {
                yield Ok::<_, std::io::Error>(Bytes::from_static(b": keepalive\n\n"));
                inner_slot.get().unwrap().observe_terminal(TimingResult::Complete);
                let next = inner_slot.begin_for_provider(
                    ProviderType::CursorApiKey, "responses", Instant::now(),
                ).unwrap();
                next.mark(Milestone::FirstSemantic);
                if cancel { std::future::pending::<()>().await; }
                next.observe_terminal(TimingResult::Complete);
                yield Ok::<_, std::io::Error>(Bytes::from_static(b"accepted result"));
            };
            let mut body = slot
                .response(Response::new(Body::from_stream(stream)))
                .into_body()
                .into_data_stream();
            body.next().await.unwrap().unwrap();
            if cancel {
                assert!(tokio::time::timeout(Duration::from_millis(10), body.next())
                    .await
                    .is_err());
            } else {
                assert_eq!(body.next().await.unwrap().unwrap(), "accepted result");
                assert!(body.next().await.is_none());
            }
            let current = slot.get().unwrap();
            assert_eq!(first.attempt_snapshot()["result"], "retried");
            assert!(first.attempt_snapshot()["milestonesMs"]["first_semantic"].is_null());
            drop(body);
            assert_eq!(
                current.request().snapshot()["result"],
                if cancel { "cancelled" } else { "complete" }
            );
            assert_eq!(
                current.attempt_snapshot()["result"],
                if cancel { "cancelled" } else { "complete" }
            );
            assert_eq!(
                current.attempt_snapshot()["milestonesMs"]["http_body_handoff"].is_number(),
                !cancel
            );
            assert_eq!(
                current.attempt_snapshot()["waitsMs"]["downstream_demand"]
                    .as_f64()
                    .unwrap()
                    > 0.0,
                !cancel
            );
        }
    }

    #[test]
    fn codex_r4_timing_excludes_metadata_control_and_liveness_from_first_semantic() {
        let request = RequestTiming::new(Instant::now(), "responses");
        let timing = request.attempt(Instant::now());
        let mut inspector = ResponsesSseInspector::default();
        inspector.set_timing(Some(timing.clone()));
        inspector.push(b": keepalive\n\nevent: response.metadata\ndata: {\"type\":\"response.metadata\",\"headers\":{}}\n\n").unwrap();
        timing.observe(&SemanticObservation::Control);
        timing.observe(&SemanticObservation::Lifecycle);
        assert!(request.snapshot()["milestonesMs"]["first_semantic"].is_null());
        inspector
            .push(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n")
            .unwrap();
        let first = request.snapshot()["milestonesMs"]["first_semantic"].clone();
        timing.observe(&SemanticObservation::Business);
        assert_eq!(request.snapshot()["milestonesMs"]["first_semantic"], first);
        assert!(request.snapshot()["milestonesMs"]["terminal"].is_null());
        timing.observe(&SemanticObservation::SuccessTerminal);
        timing.finish_delivery(true);
        assert_eq!(request.snapshot()["result"], "complete");
    }

    #[test]
    fn codex_r4_timing_retries_keep_request_origin_and_independent_attempt_results() {
        let slot = TimingSlot::default();
        let first = slot.begin("responses", Instant::now());
        first.observe(&SemanticObservation::IncompleteTerminal);
        let second = slot.begin("responses", Instant::now());
        assert!(Arc::ptr_eq(&first.request.0, &second.request.0));
        assert!(!Arc::ptr_eq(&first.attempt, &second.attempt));
        assert_eq!(
            first.attempt.state.lock().unwrap().result,
            Some(TimingResult::Retried)
        );
        assert!(first.request.snapshot()["milestonesMs"]["terminal"].is_null());
        second.observe(&SemanticObservation::SuccessTerminal);
        second.finish_delivery(true);
        assert_eq!(first.request.snapshot()["result"], "complete");
        assert!(first.request.snapshot()["milestonesMs"]["first_semantic"].is_null());
    }

    #[tokio::test]
    async fn codex_r4_timing_http_handoff_and_last_buffer_release_are_distinct() {
        let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(None);
        let budget = RequestMemoryBudget::with_parent(1024, parent.clone());
        let request = RequestTiming::new(Instant::now(), "responses");
        let timing = request.attempt(Instant::now());
        budget.set_timing(timing.clone());
        timing.observe(&SemanticObservation::SuccessTerminal);
        let bytes = budget
            .reserve(RequestMemoryComponent::NormalizedEvent, 5)
            .unwrap()
            .retain_bytes(Bytes::from_static(b"hello"));
        let retained = bytes.slice(..1);
        let mut body = timing
            .response(Response::new(Body::from(bytes)))
            .into_body();
        let frame = body.frame().await.unwrap().unwrap();
        assert_eq!(frame.data_ref().unwrap().as_ref(), b"hello");
        drop(frame);
        drop(body);
        drop(budget);
        let observed = request.snapshot();
        assert_eq!(observed["result"], "complete");
        assert!(observed["milestonesMs"]["http_body_handoff"].is_number());
        assert!(observed["milestonesMs"]["downstream_write_completed"].is_null());
        assert!(observed["milestonesMs"]["owner_released"].is_null());
        assert_eq!(parent.snapshot().used_bytes, 5);
        drop(retained);
        assert!(request.snapshot()["milestonesMs"]["owner_released"].is_number());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn codex_r4_timing_cancel_drain_waits_for_remaining_payload_owners() {
        let budget = RequestMemoryBudget::new(1024);
        let request = RequestTiming::new(Instant::now(), "responses");
        let timing = request.attempt(Instant::now());
        budget.set_timing(timing.clone());
        let payload = budget
            .reserve(RequestMemoryComponent::NormalizedEvent, 1)
            .unwrap()
            .retain_bytes(Bytes::from_static(b"x"));
        let held = payload.clone();
        let stream = futures_util::stream::once(async { Ok::<_, std::io::Error>(payload) })
            .chain(futures_util::stream::pending());
        let mut body = timing
            .response(Response::new(Body::from_stream(stream)))
            .into_body();
        drop(body.frame().await.unwrap().unwrap());
        drop(body);
        drop(budget);
        assert_eq!(request.snapshot()["result"], "cancelled");
        assert!(request.snapshot()["milestonesMs"]["owner_released"].is_null());
        tokio::time::sleep(Duration::from_millis(2)).await;
        drop(held);
        assert!(
            request.snapshot()["waitsMs"]["cancel_drain"]
                .as_f64()
                .unwrap()
                > 0.0
        );
    }

    #[test]
    fn codex_r4_timing_empty_terminal_never_fabricates_content_or_writer_completion() {
        let request = RequestTiming::new(Instant::now(), "responses_websocket");
        let timing = request.attempt(Instant::now());
        timing.mark(Milestone::SendStarted);
        timing.observe(&SemanticObservation::SuccessTerminal);
        timing.finish_delivery(true);
        timing.finish_delivery(true);
        let state = request.snapshot();
        assert_eq!(state["result"], "complete");
        for phase in [
            "first_semantic",
            "upstream_write_completed",
            "downstream_write_completed",
            "owner_released",
        ] {
            assert!(state["milestonesMs"][phase].is_null());
        }
    }

    #[test]
    fn codex_r4_timing_late_close_cannot_relabel_completed_delivery_as_cancellation() {
        let request = RequestTiming::new(Instant::now(), "responses_websocket");
        let timing = request.attempt(Instant::now());
        timing.observe(&SemanticObservation::SuccessTerminal);
        timing.finish_delivery(true);
        timing.finish(TimingResult::Cancelled);
        request.owner_released();
        assert_eq!(request.snapshot()["result"], "complete");
        assert!(request.0.state.lock().unwrap().cancelled.is_none());
        assert_eq!(request.snapshot()["waitsMs"]["cancel_drain"], 0.0);
    }
}
