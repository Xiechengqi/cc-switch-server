use bytes::Bytes;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::responses_transport::{
    encode_canonical_done_sse, encode_canonical_json_sse, ResponsesLivenessKind,
    ResponsesTransportDecoder, ResponsesTransportItem,
};

const MAX_SEMANTIC_PENDING_BYTES: usize = 2 * 1024 * 1024;
const MAX_SEMANTIC_EVENT_BYTES: usize = 128 * 1024 * 1024;
const RESPONSES_CONTENT_REPEAT_LIMIT: usize = 128;
const RESPONSES_REASONING_REPEAT_LIMIT: usize = 256;
const REPORTED_RESPONSE_MODEL_MAX_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReportedResponseModel {
    pub(super) model: String,
    pub(super) conflict: bool,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ReportedResponseModelObserver {
    first: Option<String>,
    terminal: Option<String>,
    conflict: bool,
}

impl ReportedResponseModelObserver {
    pub(super) fn observe_json_bytes(&mut self, bytes: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            return;
        };
        if value.get("type").and_then(Value::as_str).is_some() {
            self.observe_event_value(&value);
        } else {
            self.observe_document_value(&value);
        }
    }

    pub(super) fn observe_document_value(&mut self, value: &Value) {
        let model = value
            .get("model")
            .or_else(|| value.pointer("/response/model"));
        self.observe_model_value(model, true);
    }

    pub(super) fn observe_event_value(&mut self, value: &Value) {
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        let terminal = matches!(
            event_type,
            "response.completed"
                | "response.incomplete"
                | "response.failed"
                | "response.cancelled"
                | "response.canceled"
                | "response.done"
        );
        if !terminal
            && !matches!(
                event_type,
                "response.created" | "response.in_progress" | "response.queued"
            )
        {
            return;
        }
        self.observe_model_value(value.pointer("/response/model"), terminal);
    }

    pub(super) fn observation(&self) -> Option<ReportedResponseModel> {
        self.terminal
            .as_ref()
            .or(self.first.as_ref())
            .map(|model| ReportedResponseModel {
                model: model.clone(),
                conflict: self.conflict,
            })
    }

    pub(super) fn terminal_observation(&self) -> Option<ReportedResponseModel> {
        self.terminal.as_ref().map(|model| ReportedResponseModel {
            model: model.clone(),
            conflict: self.conflict,
        })
    }

    fn observe_model_value(&mut self, value: Option<&Value>, terminal: bool) {
        let Some(model) = value.and_then(valid_reported_response_model) else {
            return;
        };
        if self
            .terminal
            .as_ref()
            .or(self.first.as_ref())
            .is_some_and(|existing| !existing.eq_ignore_ascii_case(&model))
        {
            self.conflict = true;
        }
        if terminal {
            if self.terminal.is_none() {
                self.terminal = Some(model);
            }
        } else if self.first.is_none() {
            self.first = Some(model);
        }
    }
}

fn valid_reported_response_model(value: &Value) -> Option<String> {
    let model = value.as_str()?.trim();
    if model.is_empty()
        || model.len() > REPORTED_RESPONSE_MODEL_MAX_BYTES
        || !model.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
        })
    {
        return None;
    }
    Some(model.to_string())
}

/// Fill only missing terminal output IDs from the matching completed item.
/// A non-empty terminal output remains authoritative for every other field.
pub(super) fn hydrate_response_output_item_ids<'a>(
    output: &mut [Value],
    mut indexed_item: impl FnMut(usize) -> Option<&'a Value>,
) -> bool {
    let mut changed = false;
    for (index, item) in output.iter_mut().enumerate() {
        let Some(item_object) = item.as_object_mut() else {
            continue;
        };
        let missing_id = item_object
            .get("id")
            .is_none_or(|id| id.is_null() || id.as_str().is_some_and(|id| id.trim().is_empty()));
        if !missing_id {
            continue;
        }
        let Some(id) = indexed_item(index)
            .and_then(|item| item.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
        else {
            continue;
        };
        item_object.insert("id".to_string(), Value::String(id.to_string()));
        changed = true;
    }
    changed
}

#[derive(Debug, Default)]
pub(super) struct ResponsesRepeatTracker {
    content: RepeatLane,
    reasoning: RepeatLane,
}

#[derive(Debug, Default)]
struct RepeatLane {
    last: Option<(usize, [u8; 32])>,
    repeats: usize,
}

impl ResponsesRepeatTracker {
    pub(super) fn observe_value(&mut self, value: &Value) -> Result<(), SemanticProtocolError> {
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(delta) = value.get("delta").and_then(Value::as_str) else {
            return Ok(());
        };
        let (lane, limit, kind) = match event_type {
            "response.output_text.delta" => {
                (&mut self.content, RESPONSES_CONTENT_REPEAT_LIMIT, "content")
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => (
                &mut self.reasoning,
                RESPONSES_REASONING_REPEAT_LIMIT,
                "reasoning",
            ),
            _ => return Ok(()),
        };
        let fingerprint = (delta.len(), Sha256::digest(delta.as_bytes()).into());
        if lane.last == Some(fingerprint) {
            lane.repeats = lane.repeats.saturating_add(1);
        } else {
            lane.last = Some(fingerprint);
            lane.repeats = 1;
        }
        if lane.repeats > limit {
            return Err(SemanticProtocolError::new(format!(
                "Responses {kind} output repeat limit exceeded"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FailureOrigin {
    Client,
    Provider,
}

impl FailureOrigin {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Provider => "provider",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SemanticFailure {
    pub(super) origin: FailureOrigin,
    pub(super) code: String,
    pub(super) message: String,
}

impl SemanticFailure {
    pub(super) fn display_message(&self) -> String {
        format!("{}: {}", self.code, self.message)
    }

    pub(super) fn retryable_error_frame(&self) -> bool {
        if self.origin == FailureOrigin::Client {
            return false;
        }
        let code = normalize_failure_token(&self.code);
        matches!(
            code.as_str(),
            "server_is_overloaded"
                | "slow_down"
                | "server_error"
                | "service_unavailable"
                | "service_unavailable_error"
                | "upstream_error"
                | "authentication_error"
                | "unauthorized"
                | "invalid_api_key"
        ) || code.contains("rate_limit")
            || code.contains("usage_limit")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SemanticObservation {
    Lifecycle,
    /// Reviewed out-of-band information, independent of generation progress.
    /// Visibility and transport commit remain the caller's responsibility.
    Metadata,
    /// A reviewed interrupt ACK is not a generation error or terminal. Its
    /// delivery can still commit bytes on the transport.
    Control,
    Business,
    ErrorFrame(SemanticFailure),
    SuccessTerminal,
    IncompleteTerminal,
    Failure(SemanticFailure),
}

impl SemanticObservation {
    pub(super) fn metric_kind(&self) -> &'static str {
        match self {
            Self::Lifecycle => "lifecycle",
            Self::Metadata => "metadata",
            Self::Control => "control",
            Self::Business => "business",
            Self::ErrorFrame(_) => "error_frame",
            Self::SuccessTerminal => "success_terminal",
            Self::IncompleteTerminal => "incomplete_terminal",
            Self::Failure(failure) => match failure.origin {
                FailureOrigin::Client => "client_failure",
                FailureOrigin::Provider => "provider_failure",
            },
        }
    }

    pub(super) fn commits_downstream(&self) -> bool {
        match self {
            Self::Lifecycle | Self::Metadata | Self::Control => false,
            Self::ErrorFrame(failure) => !failure.retryable_error_frame(),
            _ => true,
        }
    }

    pub(super) fn counts_as_business_output(&self) -> bool {
        matches!(self, Self::Business)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SemanticTerminal {
    Success,
    Incomplete,
    Failure(SemanticFailure),
}

impl SemanticTerminal {
    pub(super) fn stream_status(&self) -> &'static str {
        match self {
            Self::Success => "completed",
            Self::Incomplete => "incomplete",
            Self::Failure(failure) => match failure.origin {
                FailureOrigin::Client => "client_error",
                FailureOrigin::Provider => "provider_failed",
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SemanticProtocolErrorKind {
    Protocol,
    Capacity,
    MissingTerminal,
}

#[derive(Debug)]
pub(super) struct SemanticProtocolError {
    kind: SemanticProtocolErrorKind,
    message: String,
}

impl SemanticProtocolError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            kind: SemanticProtocolErrorKind::Protocol,
            message: message.into(),
        }
    }

    fn from_transport(error: super::responses_transport::ResponsesTransportError) -> Self {
        Self {
            kind: if error.is_capacity() {
                SemanticProtocolErrorKind::Capacity
            } else {
                SemanticProtocolErrorKind::Protocol
            },
            message: error.to_string(),
        }
    }

    fn missing_terminal(message: impl Into<String>) -> Self {
        Self {
            kind: SemanticProtocolErrorKind::MissingTerminal,
            message: message.into(),
        }
    }

    pub(super) fn is_capacity(&self) -> bool {
        self.kind == SemanticProtocolErrorKind::Capacity
    }

    pub(super) fn is_missing_terminal(&self) -> bool {
        self.kind == SemanticProtocolErrorKind::MissingTerminal
    }
}

impl std::fmt::Display for SemanticProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SemanticProtocolError {}

pub(super) fn semantic_guard_enabled() -> bool {
    std::env::var("CC_SWITCH_PROXY_SEMANTIC_GUARD_ENABLED")
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no" | "disabled"
            )
        })
        .unwrap_or(true)
}

pub(super) fn responses_sse_normalizer_enabled() -> bool {
    std::env::var("CC_SWITCH_CODEX_RESPONSES_SSE_NORMALIZER_ENABLED")
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no" | "disabled"
            )
        })
        .unwrap_or(true)
}

pub(super) fn classify_json_document(
    body: &[u8],
) -> Result<SemanticObservation, SemanticProtocolError> {
    classify_json_document_with_timing(body, None)
}

pub(super) fn classify_json_document_with_timing(
    body: &[u8],
    timing: Option<&super::request_timing::ForwardTiming>,
) -> Result<SemanticObservation, SemanticProtocolError> {
    let value = serde_json::from_slice::<Value>(body).map_err(|error| {
        SemanticProtocolError::new(format!("Responses body is not valid JSON: {error}"))
    })?;
    if !value.is_object() {
        return Err(SemanticProtocolError::new(
            "Responses body must be a JSON object",
        ));
    }
    reject_conflicting_completed_status(&value)?;
    let observation = classify_value(&value);
    if let Some(timing) = timing {
        timing.observe_responses_document(&value, &observation);
    }
    Ok(observation)
}

fn reject_conflicting_completed_status(value: &Value) -> Result<(), SemanticProtocolError> {
    let response = value
        .get("response")
        .filter(|response| response.is_object())
        .unwrap_or(value);
    if value.get("type").and_then(Value::as_str) == Some("response.completed")
        && response
            .get("status")
            .is_some_and(|status| status.as_str() != Some("completed"))
    {
        return Err(SemanticProtocolError::new(
            "Responses completed event has a non-completed response status",
        ));
    }
    Ok(())
}

pub(super) fn classify_value(value: &Value) -> SemanticObservation {
    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    let response = value
        .get("response")
        .filter(|response| response.is_object())
        .unwrap_or(value);
    let status = response.get("status").and_then(Value::as_str);

    if super::providers::codex::control::is_interrupt_ack(value) {
        return SemanticObservation::Control;
    }

    if event_type == "error" {
        return SemanticObservation::ErrorFrame(failure_from_value(value, response, status));
    }
    if matches!(status, Some("failed" | "cancelled"))
        || nested_non_null_error(response).is_some()
        || nested_non_null_error(value).is_some()
        || event_type == "response.failed"
        || matches!(event_type, "response.cancelled" | "response.canceled")
    {
        return SemanticObservation::Failure(failure_from_value(value, response, status));
    }

    if is_reviewed_responses_metadata(value) {
        return SemanticObservation::Metadata;
    }

    if event_type == "response.incomplete"
        || (event_type.is_empty() && status == Some("incomplete"))
    {
        return SemanticObservation::IncompleteTerminal;
    }
    if event_type == "response.completed" && status == Some("incomplete") {
        return SemanticObservation::IncompleteTerminal;
    }
    if event_type == "response.completed"
        && response
            .get("status")
            .is_some_and(|value| value.as_str() != Some("completed"))
    {
        return SemanticObservation::Business;
    }
    if event_type == "response.completed" || (event_type.is_empty() && status == Some("completed"))
    {
        return SemanticObservation::SuccessTerminal;
    }
    if matches!(
        event_type,
        "response.created" | "response.in_progress" | "response.queued"
    ) || (event_type.is_empty() && matches!(status, Some("queued" | "in_progress")))
    {
        return SemanticObservation::Lifecycle;
    }
    if is_empty_startup_announcement(event_type, value) {
        return SemanticObservation::Lifecycle;
    }
    SemanticObservation::Business
}

/// A closed envelope list, deliberately unrelated to the public SSE allowlist.
/// Unknown events/fields may carry observable work and must remain business.
fn is_reviewed_responses_metadata(value: &Value) -> bool {
    reviewed_metadata_envelope(value.get("type").and_then(Value::as_str), value)
}

fn reviewed_metadata_envelope(kind: Option<&str>, value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let Some(kind) = kind else {
        return false;
    };
    let fields: &[&str] = match kind {
        "response.metadata" | "codex.response.metadata" => &[
            "type",
            "response_id",
            "sequence_number",
            "metadata",
            "headers",
        ],
        "codex.rate_limits" | "rate_limits.updated" => &[
            "type",
            "response_id",
            "sequence_number",
            "rate_limits",
            "rate_limit",
            "plan_type",
            "credits",
            "metered_limit_name",
            "limit_id",
            "limit_name",
        ],
        _ => return false,
    };
    object.iter().all(|(key, value)| {
        fields.contains(&key.as_str())
            && match key.as_str() {
                "type" => true,
                "sequence_number" => value.as_u64().is_some(),
                "response_id" => value
                    .as_str()
                    .is_some_and(|id| !id.trim().is_empty() && id.len() <= 512),
                "metadata" | "headers" | "rate_limits" | "rate_limit" | "credits" => {
                    value.is_object() || value.is_null()
                }
                _ => value.is_string() || value.is_null(),
            }
    })
}

pub(super) fn is_responses_terminal_event(declared_event: Option<&str>, value: &Value) -> bool {
    matches!(
        declared_event,
        Some(
            "response.completed"
                | "response.incomplete"
                | "response.failed"
                | "response.cancelled"
                | "response.canceled"
                | "error"
        )
    ) || matches!(
        classify_value(value),
        SemanticObservation::SuccessTerminal
            | SemanticObservation::IncompleteTerminal
            | SemanticObservation::Failure(_)
            | SemanticObservation::ErrorFrame(_)
    )
}

/// Returns whether a Responses `*.added` event is only an empty startup shell.
///
/// This deliberately uses closed item and part lists. Unknown shapes, server-side
/// operations, and any known content field that is already non-empty must commit
/// the response so a later failure cannot replay observable work.
fn is_empty_startup_announcement(event_type: &str, value: &Value) -> bool {
    match event_type {
        "response.output_item.added" => value.get("item").is_some_and(is_empty_startup_output_item),
        "response.content_part.added" | "response.reasoning_summary_part.added" => {
            value.get("part").is_some_and(is_empty_startup_part)
        }
        _ => false,
    }
}

fn is_empty_startup_output_item(item: &Value) -> bool {
    let Some(item) = item.as_object() else {
        return false;
    };
    match item.get("type").and_then(Value::as_str) {
        Some("message") => {
            has_only_startup_item_fields(item, &["id", "type", "status", "role", "content"])
                && item
                    .get("content")
                    .is_none_or(is_empty_startup_content_list)
        }
        Some("reasoning") => {
            has_only_startup_item_fields(
                item,
                &[
                    "id",
                    "type",
                    "status",
                    "summary",
                    "content",
                    "encrypted_content",
                ],
            ) && item
                .get("encrypted_content")
                .is_none_or(|value| matches!(value, Value::Null) || value.as_str() == Some(""))
                && item
                    .get("summary")
                    .is_none_or(is_empty_startup_content_list)
                && item
                    .get("content")
                    .is_none_or(is_empty_startup_content_list)
        }
        Some("function_call") => {
            has_only_startup_item_fields(
                item,
                &["id", "type", "status", "call_id", "name", "arguments"],
            ) && item
                .get("arguments")
                .is_none_or(|arguments| arguments.as_str() == Some(""))
        }
        Some("custom_tool_call") => {
            has_only_startup_item_fields(
                item,
                &["id", "type", "status", "call_id", "name", "input"],
            ) && item
                .get("input")
                .is_none_or(|input| input.as_str() == Some(""))
        }
        _ => false,
    }
}

fn has_only_startup_item_fields(item: &serde_json::Map<String, Value>, allowed: &[&str]) -> bool {
    item.iter().all(|(field, value)| {
        allowed.contains(&field.as_str())
            && (!matches!(
                field.as_str(),
                "id" | "status" | "role" | "call_id" | "name"
            ) || value.is_string())
    })
}

fn is_empty_startup_content_list(content: &Value) -> bool {
    let Some(content) = content.as_array() else {
        return false;
    };
    content.iter().all(is_empty_startup_part)
}

fn is_empty_startup_part(part: &Value) -> bool {
    let Some(part) = part.as_object() else {
        return false;
    };
    match part.get("type").and_then(Value::as_str) {
        Some("output_text" | "summary_text" | "text" | "reasoning_text") => {
            part.get("text").and_then(Value::as_str) == Some("")
                && part.iter().all(|(field, value)| match field.as_str() {
                    "type" | "text" => true,
                    "annotations" | "logprobs" => value.as_array().is_some_and(Vec::is_empty),
                    _ => false,
                })
        }
        Some("refusal") => {
            part.get("refusal").and_then(Value::as_str) == Some("")
                && part
                    .keys()
                    .all(|field| matches!(field.as_str(), "type" | "refusal"))
        }
        _ => false,
    }
}

fn failure_from_value(value: &Value, response: &Value, status: Option<&str>) -> SemanticFailure {
    let error = nested_non_null_error(response)
        .or_else(|| nested_non_null_error(value))
        .unwrap_or(response);
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| error.get("type").and_then(Value::as_str))
        .or(status)
        .or_else(|| value.get("type").and_then(Value::as_str))
        .filter(|code| !code.trim().is_empty())
        .unwrap_or("upstream_error")
        .to_string();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .filter(|message| !message.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| match status {
            Some("cancelled") => "response generation was cancelled".to_string(),
            _ => "response generation failed".to_string(),
        });
    let client_failure = [
        error.get("code").and_then(Value::as_str),
        error.get("type").and_then(Value::as_str),
        status,
        value.get("type").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .any(|candidate| classify_failure_origin(candidate) == FailureOrigin::Client);
    let origin = if client_failure {
        FailureOrigin::Client
    } else {
        FailureOrigin::Provider
    };
    SemanticFailure {
        origin,
        code,
        message,
    }
}

fn nested_non_null_error(value: &Value) -> Option<&Value> {
    value
        .get("error")
        .filter(|error| error_value_is_substantive(error))
        .or_else(|| {
            value
                .pointer("/status_details/error")
                .filter(|error| error_value_is_substantive(error))
        })
        .or_else(|| {
            value
                .pointer("/body/error")
                .filter(|error| error_value_is_substantive(error))
        })
}

pub(super) fn error_value_is_substantive(error: &Value) -> bool {
    match error {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        Value::Number(_) => true,
    }
}

fn classify_failure_origin(code: &str) -> FailureOrigin {
    let code = normalize_failure_token(code);
    if matches!(
        code.as_str(),
        "bad_request"
            | "bad_request_error"
            | "content_filter"
            | "content_filter_error"
            | "content_policy_violation"
            | "context_length_exceeded"
            | "cancelled"
            | "canceled"
            | "client_cancelled"
            | "invalid_argument"
            | "invalid_argument_error"
            | "invalid_request"
            | "invalid_request_error"
            | "missing_required_parameter"
            | "moderation_blocked"
            | "prompt_blocked"
            | "request_too_large"
            | "request_cancelled"
            | "response_cancelled"
            | "response_canceled"
            | "safety_violation"
            | "unsupported_parameter"
            | "unprocessable_entity"
            | "validation_error"
            | "validation_failed"
    ) || code.starts_with("invalid_request_")
        || code.starts_with("invalid_argument_")
        || code.starts_with("validation_")
    {
        FailureOrigin::Client
    } else {
        FailureOrigin::Provider
    }
}

fn normalize_failure_token(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['-', '.'], "_")
}

#[derive(Debug)]
pub(super) struct ResponsesSseInspector {
    transport: ResponsesTransportDecoder,
    rejected: bool,
    saw_business: bool,
    terminal: Option<SemanticTerminal>,
    pending_error: Option<SemanticFailure>,
    terminal_from_error_frame: bool,
    done_seen: bool,
    repeats: ResponsesRepeatTracker,
    repeat_guard_enabled: bool,
    event_visibility: ResponsesEventVisibility,
    reported_model: ReportedResponseModelObserver,
    started: bool,
    timing: Option<super::request_timing::ForwardTiming>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ResponsesEventVisibility {
    #[default]
    Unfiltered,
    StandardClient,
    NativeCodexClient,
}

#[derive(Debug, Default)]
pub(super) struct ResponsesInspectionBatch {
    pub(super) observations: Vec<SemanticObservation>,
    pub(super) normalized: Bytes,
    pub(super) liveness: Vec<ResponsesLivenessKind>,
}

impl ResponsesInspectionBatch {
    pub(super) fn record_transport_metrics(&self, surface: &'static str) {
        for kind in &self.liveness {
            crate::metrics::record_responses_sse_transport(surface, kind.metric_kind());
        }
        if !self.normalized.is_empty() {
            crate::metrics::record_responses_sse_transport(surface, "normalized_event");
        }
    }
}

impl Default for ResponsesSseInspector {
    fn default() -> Self {
        Self {
            transport: ResponsesTransportDecoder::new(
                MAX_SEMANTIC_PENDING_BYTES,
                MAX_SEMANTIC_EVENT_BYTES,
            ),
            rejected: false,
            saw_business: false,
            terminal: None,
            pending_error: None,
            terminal_from_error_frame: false,
            done_seen: false,
            repeats: ResponsesRepeatTracker::default(),
            repeat_guard_enabled: false,
            event_visibility: ResponsesEventVisibility::Unfiltered,
            reported_model: ReportedResponseModelObserver::default(),
            started: false,
            timing: None,
        }
    }
}

impl ResponsesSseInspector {
    pub(super) fn set_timing(&mut self, timing: Option<super::request_timing::ForwardTiming>) {
        self.timing = timing;
    }
    pub(super) fn with_repeat_guard(enabled: bool) -> Self {
        Self {
            repeat_guard_enabled: enabled,
            ..Self::default()
        }
    }

    pub(super) fn with_repeat_guard_and_visibility(
        enabled: bool,
        event_visibility: ResponsesEventVisibility,
    ) -> Self {
        Self {
            repeat_guard_enabled: enabled,
            event_visibility,
            ..Self::default()
        }
    }

    fn observe_repeat(&mut self, value: &Value) -> Result<(), SemanticProtocolError> {
        if self.repeat_guard_enabled {
            self.repeats.observe_value(value)?;
        }
        Ok(())
    }

    pub(super) fn push(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<SemanticObservation>, SemanticProtocolError> {
        self.push_normalized(chunk).map(|batch| batch.observations)
    }

    pub(super) fn push_normalized(
        &mut self,
        chunk: &[u8],
    ) -> Result<ResponsesInspectionBatch, SemanticProtocolError> {
        if self.rejected {
            return Err(Self::rejected_error());
        }
        self.started = true;
        let result = self
            .transport
            .push(chunk)
            .map_err(SemanticProtocolError::from_transport)
            .and_then(|items| self.process_transport_items(items));
        self.rejected = result.is_err();
        result
    }

    fn push_with_limits(
        &mut self,
        chunk: &[u8],
        max_pending_bytes: usize,
        max_event_bytes: usize,
    ) -> Result<Vec<SemanticObservation>, SemanticProtocolError> {
        if self.started {
            return Err(SemanticProtocolError::new(
                "Responses inspector limits cannot change after decoding starts",
            ));
        }
        self.transport = ResponsesTransportDecoder::new(max_pending_bytes, max_event_bytes);
        self.push(chunk)
    }

    pub(super) fn finish(&mut self) -> Result<Vec<SemanticObservation>, SemanticProtocolError> {
        self.finish_normalized().map(|batch| batch.observations)
    }

    pub(super) fn finish_normalized(
        &mut self,
    ) -> Result<ResponsesInspectionBatch, SemanticProtocolError> {
        if self.rejected {
            return Err(Self::rejected_error());
        }
        self.started = true;
        let result = self.finish_normalized_inner();
        self.rejected = result.is_err();
        result
    }

    fn finish_normalized_inner(
        &mut self,
    ) -> Result<ResponsesInspectionBatch, SemanticProtocolError> {
        let items = self
            .transport
            .finish()
            .map_err(SemanticProtocolError::from_transport)?;
        let mut batch = self.process_transport_items(items)?;
        if self.terminal.is_none() {
            if let Some(failure) = self.promote_pending_error() {
                batch
                    .observations
                    .push(SemanticObservation::Failure(failure));
            } else {
                return Err(if self.done_seen {
                    SemanticProtocolError::new(
                        "Responses stream emitted [DONE] before a terminal response event",
                    )
                } else {
                    SemanticProtocolError::missing_terminal(
                        "Responses stream ended before a terminal response event",
                    )
                });
            }
        }
        Ok(batch)
    }

    pub(super) fn saw_business(&self) -> bool {
        self.saw_business
    }

    pub(super) fn terminal(&self) -> Option<&SemanticTerminal> {
        if self.rejected {
            None
        } else {
            self.terminal.as_ref()
        }
    }

    fn rejected_error() -> SemanticProtocolError {
        SemanticProtocolError::new("Responses inspector was already rejected")
    }

    pub(super) fn retained_transport_bytes(&self) -> usize {
        self.transport.retained_bytes()
    }

    pub(super) fn reported_model(&self) -> Option<ReportedResponseModel> {
        self.reported_model.observation()
    }

    pub(super) fn synthesized_failure_from_error_frame(&self) -> Option<&SemanticFailure> {
        match self.terminal.as_ref() {
            Some(SemanticTerminal::Failure(failure)) if self.terminal_from_error_frame => {
                Some(failure)
            }
            None => self.pending_error.as_ref(),
            _ => None,
        }
    }

    fn process_transport_items(
        &mut self,
        items: Vec<ResponsesTransportItem>,
    ) -> Result<ResponsesInspectionBatch, SemanticProtocolError> {
        let mut observations = Vec::new();
        let mut normalized = Vec::new();
        let mut liveness = Vec::new();
        for item in items {
            match item {
                ResponsesTransportItem::Liveness(kind) => liveness.push(kind),
                ResponsesTransportItem::Done => {
                    let had_terminal = self.terminal.is_some();
                    if let Some(failure) = self.observe_done()? {
                        observations.push(SemanticObservation::Failure(failure));
                    }
                    if had_terminal && !self.terminal_from_error_frame {
                        normalized.extend_from_slice(&encode_canonical_done_sse());
                    }
                }
                ResponsesTransportItem::Json {
                    declared_event,
                    value,
                } => {
                    if self.terminal.is_some() {
                        if is_responses_terminal_event(declared_event.as_deref(), &value) {
                            return Err(SemanticProtocolError::new(
                                "Responses stream emitted another terminal event after a terminal response",
                            ));
                        }
                        continue;
                    }
                    reject_conflicting_completed_status(&value)?;
                    self.reported_model.observe_event_value(&value);
                    if responses_event_is_filtered(
                        self.event_visibility,
                        declared_event.as_deref(),
                        &value,
                    ) {
                        crate::metrics::record_responses_sse_transport(
                            "event_visibility",
                            "filtered_private_event",
                        );
                        continue;
                    }
                    self.observe_repeat(&value)?;
                    let mut observation = classify_value(&value);
                    // SSE may supply the event name without a payload type.
                    // The decoder already rejects conflicting explicit names.
                    // Do not clone the metadata document or change its wire.
                    if observation == SemanticObservation::Business
                        && value.get("type").is_none()
                        && reviewed_metadata_envelope(declared_event.as_deref(), &value)
                    {
                        observation = SemanticObservation::Metadata;
                    }
                    if matches!(
                        observation,
                        SemanticObservation::SuccessTerminal
                            | SemanticObservation::IncompleteTerminal
                    ) && self.pending_error.is_some()
                    {
                        if let Some(failure) = self.promote_pending_error() {
                            observations.push(SemanticObservation::Failure(failure));
                        }
                        continue;
                    }
                    if let Some(timing) = &self.timing {
                        timing.observe_responses_value(
                            &value,
                            declared_event.as_deref(),
                            &observation,
                        );
                    }
                    self.record(&observation);
                    normalized.extend_from_slice(&encode_canonical_json_sse(
                        declared_event.as_deref(),
                        &value,
                    ));
                    observations.push(observation);
                }
            }
        }
        Ok(ResponsesInspectionBatch {
            observations,
            normalized: Bytes::from(normalized),
            liveness,
        })
    }

    fn observe_done(&mut self) -> Result<Option<SemanticFailure>, SemanticProtocolError> {
        if self.done_seen {
            return Err(SemanticProtocolError::new(
                "Responses stream emitted duplicate [DONE] sentinel",
            ));
        }
        self.done_seen = true;
        if self.terminal.is_none() {
            if let Some(failure) = self.promote_pending_error() {
                return Ok(Some(failure));
            }
            return Err(SemanticProtocolError::new(
                "Responses stream emitted [DONE] before a terminal response event",
            ));
        }
        Ok(None)
    }

    fn record(&mut self, observation: &SemanticObservation) {
        match observation {
            SemanticObservation::Lifecycle
            | SemanticObservation::Metadata
            | SemanticObservation::Control => {}
            SemanticObservation::ErrorFrame(failure) => {
                self.pending_error = Some(failure.clone());
            }
            SemanticObservation::Business => self.saw_business = true,
            SemanticObservation::SuccessTerminal => {
                self.terminal.get_or_insert(SemanticTerminal::Success);
            }
            SemanticObservation::IncompleteTerminal => {
                self.terminal.get_or_insert(SemanticTerminal::Incomplete);
            }
            SemanticObservation::Failure(failure) => {
                self.terminal
                    .get_or_insert_with(|| SemanticTerminal::Failure(failure.clone()));
            }
        }
    }

    fn promote_pending_error(&mut self) -> Option<SemanticFailure> {
        let failure = self.pending_error.take()?;
        if self.terminal.is_none() {
            self.terminal = Some(SemanticTerminal::Failure(failure.clone()));
            self.terminal_from_error_frame = true;
            if let Some(timing) = &self.timing {
                timing.observe(&SemanticObservation::Failure(failure.clone()));
            }
        }
        Some(failure)
    }
}

fn responses_event_is_filtered(
    visibility: ResponsesEventVisibility,
    declared_event: Option<&str>,
    value: &Value,
) -> bool {
    if visibility == ResponsesEventVisibility::Unfiltered {
        return false;
    }
    let payload_type = value.get("type").and_then(Value::as_str);
    let mut saw_event_name = false;
    for name in [declared_event, payload_type]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        saw_event_name = true;
        if !responses_event_is_visible(visibility, name) {
            return true;
        }
    }
    // A data-bearing JSON frame without either an SSE event name or a payload
    // type is not part of the reviewed public Responses event set. `[DONE]`
    // and comment/keepalive frames are handled by the transport decoder before
    // this visibility boundary.
    !saw_event_name
}

fn responses_event_is_visible(visibility: ResponsesEventVisibility, name: &str) -> bool {
    if name.starts_with("responsesapi.") || name == "codex.rate_limits" {
        return false;
    }
    if name.starts_with("codex.") {
        return visibility == ResponsesEventVisibility::NativeCodexClient
            && name == "codex.response.metadata";
    }
    public_responses_stream_event(name)
}

/// Closed public event set from the official Responses streaming reference.
/// Unknown future events remain hidden until their downstream contract is
/// reviewed; protocol errors and terminal events are explicitly retained.
fn public_responses_stream_event(name: &str) -> bool {
    matches!(
        name,
        "error"
            | "response.audio.delta"
            | "response.audio.done"
            | "response.audio.transcript.delta"
            | "response.audio.transcript.done"
            | "response.cancelled"
            | "response.canceled"
            | "response.code_interpreter_call.completed"
            | "response.code_interpreter_call.in_progress"
            | "response.code_interpreter_call.interpreting"
            | "response.code_interpreter_call_code.delta"
            | "response.code_interpreter_call_code.done"
            | "response.compaction"
            | "response.compaction.compacting"
            | "response.completed"
            | "response.content_part.added"
            | "response.content_part.done"
            | "response.created"
            | "response.custom_tool_call_input.delta"
            | "response.custom_tool_call_input.done"
            | "response.done"
            | "response.failed"
            | "response.file_search_call.completed"
            | "response.file_search_call.in_progress"
            | "response.file_search_call.searching"
            | "response.function_call_arguments.delta"
            | "response.function_call_arguments.done"
            | "response.image_generation_call.completed"
            | "response.image_generation_call.generating"
            | "response.image_generation_call.in_progress"
            | "response.image_generation_call.partial_image"
            | "response.in_progress"
            | "response.incomplete"
            | "response.mcp_approval_request"
            | "response.mcp_call.completed"
            | "response.mcp_call.failed"
            | "response.mcp_call.in_progress"
            | "response.mcp_call_arguments.delta"
            | "response.mcp_call_arguments.done"
            | "response.mcp_list_tools.completed"
            | "response.mcp_list_tools.failed"
            | "response.mcp_list_tools.in_progress"
            | "response.output_item.added"
            | "response.output_item.done"
            | "response.output_text.annotation.added"
            | "response.output_text.delta"
            | "response.output_text.done"
            | "response.queued"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.delta"
            | "response.reasoning_text.done"
            | "response.refusal.delta"
            | "response.refusal.done"
            | "response.shell_call_command.added"
            | "response.shell_call_command.delta"
            | "response.shell_call_command.done"
            | "response.shell_call_output_content.delta"
            | "response.shell_call_output_content.done"
            | "response.steer.accepted"
            | "response.steer.failed"
            | "response.steer.pending"
            | "response.web_search_call.completed"
            | "response.web_search_call.in_progress"
            | "response.web_search_call.searching"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_lifecycle_business_and_terminals() {
        assert_eq!(
            classify_value(&json!({"type": "response.created"})),
            SemanticObservation::Lifecycle
        );
        assert_eq!(
            classify_value(&json!({"type": "response.output_text.delta", "delta": "hi"})),
            SemanticObservation::Business
        );
        assert_eq!(
            classify_value(
                &json!({"type": "response.completed", "response": {"status": "completed"}})
            ),
            SemanticObservation::SuccessTerminal
        );
        assert_eq!(
            classify_value(&json!({"status": "incomplete", "output": []})),
            SemanticObservation::IncompleteTerminal
        );
        assert_eq!(
            classify_value(&json!({
                "type": "response.created",
                "response": {"status": "completed"}
            })),
            SemanticObservation::Lifecycle
        );
        assert_eq!(
            classify_value(&json!({
                "type": "response.done",
                "response": {"status": "completed"}
            })),
            SemanticObservation::Business
        );
    }

    #[test]
    fn completed_event_cannot_override_an_explicit_non_completed_status() {
        for status in [
            json!("incomplete"),
            json!("in_progress"),
            json!("queued"),
            json!(null),
            json!(42),
        ] {
            let event = json!({
                "type": "response.completed",
                "response": {"status": status, "incomplete_details": {"reason": "max_output_tokens"}}
            });
            assert_ne!(classify_value(&event), SemanticObservation::SuccessTerminal);
            let body = serde_json::to_vec(&event).unwrap();
            assert!(classify_json_document(&body).is_err(), "status={status}");
            let mut inspector = ResponsesSseInspector::default();
            let frame = format!("event: response.completed\ndata: {event}\n\n");
            assert!(
                inspector.push_normalized(frame.as_bytes()).is_err(),
                "status={status}"
            );
        }
        for response in [json!({}), json!({"status": "completed"})] {
            let event = json!({"type": "response.completed", "response": response});
            assert_eq!(classify_value(&event), SemanticObservation::SuccessTerminal);
            assert!(classify_json_document(event.to_string().as_bytes()).is_ok());
        }
    }

    #[test]
    fn codex_empty_startup_announcements_are_lifecycle_on_the_shared_classifier() {
        let lifecycle = [
            json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
            }),
            json!({
                "type": "response.output_item.added",
                "item": {"id": "msg_status", "type": "message", "status": "in_progress", "role": "assistant", "content": []}
            }),
            json!({
                "type": "response.output_item.added",
                "item": {"id": "msg_2", "type": "message", "content": [
                    {"type": "output_text", "text": ""},
                    {"type": "refusal", "refusal": ""}
                ]}
            }),
            json!({
                "type": "response.output_item.added",
                "item": {"id": "rs_1", "type": "reasoning", "summary": [
                    {"type": "summary_text", "text": ""}
                ], "content": [{"type": "reasoning_text", "text": ""}]}
            }),
            json!({
                "type": "response.output_item.added",
                "item": {"id": "fc_1", "type": "function_call", "name": "lookup", "arguments": ""}
            }),
            json!({
                "type": "response.output_item.added",
                "item": {"id": "fc_call", "type": "function_call", "call_id": "call_1", "name": "lookup", "status": "in_progress", "arguments": ""}
            }),
            json!({
                "type": "response.output_item.added",
                "item": {"id": "ct_1", "type": "custom_tool_call", "name": "exec", "input": ""}
            }),
            json!({
                "type": "response.content_part.added",
                "part": {"type": "output_text", "text": ""}
            }),
            json!({
                "type": "response.reasoning_summary_part.added",
                "part": {"type": "summary_text", "text": ""},
                "response_lite": true
            }),
        ];

        for event in lifecycle {
            assert_eq!(
                classify_value(&event),
                SemanticObservation::Lifecycle,
                "event must remain pre-commit: {event}"
            );
            let encoded = serde_json::to_vec(&event).unwrap();
            assert_eq!(
                classify_json_document(&encoded).unwrap(),
                SemanticObservation::Lifecycle
            );
        }

        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(
                concat!(
                    "event: response.output_item.added\n",
                    "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\",\"content\":[]}}\n",
                    "\n",
                    "event: response.content_part.added\n",
                    "data: {\"type\":\"response.content_part.added\",\"part\":{\"type\":\"output_text\",\"text\":\"\"}}\n",
                    "\n"
                )
                .as_bytes(),
            )
            .unwrap();
        assert_eq!(
            observations,
            vec![
                SemanticObservation::Lifecycle,
                SemanticObservation::Lifecycle
            ]
        );
    }

    #[test]
    fn codex_visible_or_unknown_startup_announcements_commit_fail_closed() {
        let business = [
            json!({"type": "response.output_item.added"}),
            json!({"type": "response.output_item.added", "item": {"type": "some_future_item"}}),
            json!({"type": "response.output_item.added", "item": {"type": "web_search_call", "status": "in_progress"}}),
            json!({"type": "response.output_item.added", "item": {"type": "file_search_call"}}),
            json!({"type": "response.output_item.added", "item": {"type": "message", "content": [{"type": "output_text", "text": "visible"}]}}),
            json!({"type": "response.output_item.added", "item": {"type": "message", "content": [{"type": "output_audio", "audio": "AAAA"}]}}),
            json!({"type": "response.output_item.added", "item": {"type": "reasoning", "encrypted_content": "opaque"}}),
            json!({"type": "response.output_item.added", "item": {"type": "function_call", "arguments": "{}"}}),
            json!({"type": "response.output_item.added", "item": {"type": "custom_tool_call", "input": "ls"}}),
            json!({"type": "response.content_part.added", "part": {"type": "output_text", "text": "visible"}}),
            json!({"type": "response.content_part.added", "part": {"type": "output_audio", "audio": "AAAA"}}),
            json!({"type": "response.reasoning_summary_part.added", "part": {"type": "future_part"}}),
            json!({"type": "response.some_future_event"}),
        ];

        for event in business {
            let observation = classify_value(&event);
            assert_eq!(
                observation,
                SemanticObservation::Business,
                "event must commit fail closed: {event}"
            );
            assert!(observation.commits_downstream());
        }
    }

    #[test]
    fn codex_empty_text_announcement_with_annotations_or_logprobs_commits() {
        let substantive_parts = [
            json!({"type": "output_text", "text": "", "annotations": [{"type": "url_citation", "url": "https://example.com"}]}),
            json!({"type": "output_text", "text": "", "logprobs": [{"token": "x", "logprob": -0.1}]}),
            json!({"type": "output_text", "text": "", "future_content": "opaque"}),
        ];
        for part in substantive_parts {
            for event in [
                json!({"type": "response.content_part.added", "part": part}),
                json!({"type": "response.output_item.added", "item": {
                    "type": "message", "role": "assistant", "content": [part]
                }}),
            ] {
                assert_eq!(
                    classify_value(&event),
                    SemanticObservation::Business,
                    "content-bearing startup event must commit: {event}"
                );
                assert_eq!(
                    classify_json_document(event.to_string().as_bytes()).unwrap(),
                    SemanticObservation::Business
                );
                let mut inspector = ResponsesSseInspector::default();
                let frame = format!("data: {event}\n\n");
                assert_eq!(
                    inspector.push(frame.as_bytes()).unwrap(),
                    vec![SemanticObservation::Business]
                );
                assert!(inspector.saw_business());
            }
        }

        for part in [
            json!({"type": "output_text", "text": "", "annotations": [], "logprobs": []}),
            json!({"type": "summary_text", "text": ""}),
        ] {
            for event in [
                json!({"type": "response.content_part.added", "part": part}),
                json!({"type": "response.output_item.added", "item": {
                    "type": "message", "role": "assistant", "content": [part]
                }}),
            ] {
                assert_eq!(classify_value(&event), SemanticObservation::Lifecycle);
            }
        }
    }

    #[test]
    fn codex_empty_output_item_with_unknown_business_field_commits() {
        let items = [
            json!({"type": "message", "role": "assistant", "content": [], "future_content": "visible"}),
            json!({"type": "reasoning", "summary": [], "future_content": {"token": "visible"}}),
            json!({"type": "function_call", "name": "lookup", "arguments": "", "future_content": "visible"}),
            json!({"type": "custom_tool_call", "name": "exec", "input": "", "future_content": "visible"}),
            json!({"type": "message", "content": [], "role": {"future_content": "visible"}}),
        ];
        for item in items {
            let event = json!({"type": "response.output_item.added", "item": item});
            assert_eq!(
                classify_value(&event),
                SemanticObservation::Business,
                "{event}"
            );
            assert_eq!(
                classify_json_document(event.to_string().as_bytes()).unwrap(),
                SemanticObservation::Business,
                "{event}"
            );
            let mut inspector = ResponsesSseInspector::default();
            let frame = format!("data: {event}\n\n");
            assert_eq!(
                inspector.push(frame.as_bytes()).unwrap(),
                vec![SemanticObservation::Business],
                "{event}"
            );
            assert!(inspector.saw_business(), "{event}");
        }
    }

    #[test]
    fn json_document_rejects_scalar_and_array_payloads() {
        for body in [b"[]".as_slice(), b"\"not a response object\"".as_slice()] {
            assert!(classify_json_document(body)
                .unwrap_err()
                .to_string()
                .contains("JSON object"));
        }
    }

    #[test]
    fn repeat_tracker_separates_content_and_reasoning_and_resets_on_change() {
        let mut tracker = ResponsesRepeatTracker::default();
        for _ in 0..RESPONSES_CONTENT_REPEAT_LIMIT {
            tracker
                .observe_value(&json!({"type":"response.output_text.delta","delta":"x"}))
                .unwrap();
        }
        tracker
            .observe_value(&json!({"type":"response.output_text.delta","delta":"y"}))
            .unwrap();
        for _ in 1..RESPONSES_CONTENT_REPEAT_LIMIT {
            tracker
                .observe_value(&json!({"type":"response.output_text.delta","delta":"y"}))
                .unwrap();
        }
        assert!(tracker
            .observe_value(&json!({"type":"response.output_text.delta","delta":"y"}))
            .is_err());

        let mut reasoning = ResponsesRepeatTracker::default();
        for _ in 0..RESPONSES_REASONING_REPEAT_LIMIT {
            reasoning
                .observe_value(&json!({
                    "type":"response.reasoning_summary_text.delta",
                    "delta":"."
                }))
                .unwrap();
        }
        assert!(reasoning
            .observe_value(&json!({
                "type":"response.reasoning_text.delta",
                "delta":"."
            }))
            .is_err());
    }

    #[test]
    fn separates_client_validation_from_provider_failure() {
        let client = classify_value(&json!({
            "type": "response.failed",
            "response": {"error": {"type": "invalid_request_error", "message": "bad tool"}}
        }));
        assert!(matches!(
            client,
            SemanticObservation::Failure(SemanticFailure {
                origin: FailureOrigin::Client,
                ..
            })
        ));

        let specific_client = classify_value(&json!({
            "type": "response.failed",
            "response": {
                "error": {
                    "type": "invalid_request_error",
                    "code": "unknown_specific_validation_code",
                    "message": "bad tool schema"
                }
            }
        }));
        assert!(matches!(
            specific_client,
            SemanticObservation::Failure(SemanticFailure {
                origin: FailureOrigin::Client,
                ..
            })
        ));

        let provider = classify_value(&json!({
            "status": "failed",
            "error": {"code": "server_error", "message": "busy"}
        }));
        assert!(matches!(
            provider,
            SemanticObservation::Failure(SemanticFailure {
                origin: FailureOrigin::Provider,
                ..
            })
        ));

        let cancelled = classify_value(&json!({
            "type": "response.cancelled",
            "response": {"status": "cancelled"}
        }));
        assert!(matches!(
            cancelled,
            SemanticObservation::Failure(SemanticFailure {
                origin: FailureOrigin::Client,
                ..
            })
        ));
        let error_frame = classify_value(&json!({
            "type": "error",
            "error": {
                "type": "service_unavailable_error",
                "code": "server_is_overloaded",
                "message": "Our servers are currently overloaded. Please try again later."
            }
        }));
        assert!(matches!(
            error_frame,
            SemanticObservation::ErrorFrame(SemanticFailure {
                origin: FailureOrigin::Provider,
                ref code,
                ..
            }) if code == "server_is_overloaded"
        ));
        assert!(!error_frame.commits_downstream());

        let status_details = classify_value(&json!({
            "type": "response.failed",
            "response": {
                "status": "failed",
                "error": null,
                "status_details": {
                    "error": {
                        "code": "server_is_overloaded",
                        "message": "Our servers are currently overloaded. Please try again later."
                    }
                }
            }
        }));
        assert!(matches!(
            status_details,
            SemanticObservation::Failure(SemanticFailure {
                origin: FailureOrigin::Provider,
                ref code,
                ..
            }) if code == "server_is_overloaded"
        ));

        let non_retryable_error_frame = classify_value(&json!({
            "type": "error",
            "error": {
                "code": "unexpected_protocol_failure",
                "message": "cannot continue"
            }
        }));
        assert!(non_retryable_error_frame.commits_downstream());
    }

    #[test]
    fn empty_error_placeholders_do_not_override_response_status() {
        for error in [Value::Null, json!({}), json!([]), json!(""), json!(false)] {
            assert_eq!(
                classify_value(&json!({
                    "type": "response.completed",
                    "response": {"status": "completed", "error": error}
                })),
                SemanticObservation::SuccessTerminal
            );
        }
    }

    #[test]
    fn sse_inspector_handles_split_lifecycle_and_failure() {
        let mut inspector = ResponsesSseInspector::default();
        assert!(inspector
            .push(b"event: response.created\ndata: {\"type\":\"response.created\"}\n")
            .unwrap()
            .is_empty());
        let lifecycle = inspector.push(b"\n").unwrap();
        assert_eq!(lifecycle, vec![SemanticObservation::Lifecycle]);
        let failure = inspector
            .push(
                b"event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"server_error\",\"message\":\"busy\"}}}\n\n",
            )
            .unwrap();
        assert!(matches!(
            failure.as_slice(),
            [SemanticObservation::Failure(SemanticFailure {
                origin: FailureOrigin::Provider,
                ..
            })]
        ));
        assert!(matches!(
            inspector.terminal(),
            Some(SemanticTerminal::Failure(_))
        ));
        inspector.finish().unwrap();
    }

    #[test]
    fn sse_inspector_does_not_treat_error_frame_as_terminal() {
        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(
                concat!(
                    "event: response.created\n",
                    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n",
                    "\n",
                    "event: error\n",
                    "data: {\"type\":\"error\",\"error\":{\"code\":\"server_is_overloaded\",\"message\":\"overloaded\"}}\n",
                    "\n"
                )
                .as_bytes(),
            )
            .unwrap();
        assert!(matches!(
            observations.as_slice(),
            [
                SemanticObservation::Lifecycle,
                SemanticObservation::ErrorFrame(SemanticFailure {
                    origin: FailureOrigin::Provider,
                    code,
                    ..
                })
            ] if code == "server_is_overloaded"
        ));
        assert!(inspector.terminal().is_none());
        let finished = inspector.finish().unwrap();
        assert!(matches!(
            finished.as_slice(),
            [SemanticObservation::Failure(SemanticFailure {
                origin: FailureOrigin::Provider,
                code,
                ..
            })] if code == "server_is_overloaded"
        ));
        assert!(matches!(
            inspector.terminal(),
            Some(SemanticTerminal::Failure(SemanticFailure {
                code,
                ..
            })) if code == "server_is_overloaded"
        ));
    }

    #[test]
    fn sse_inspector_promotes_error_frame_when_done_arrives_first() {
        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(
                concat!(
                    "event: error\n",
                    "data: {\"type\":\"error\",\"error\":{\"code\":\"rate_limit_exceeded\",\"message\":\"try again in 3s\"}}\n",
                    "\n",
                    "data: [DONE]\n",
                    "\n"
                )
                .as_bytes(),
            )
            .unwrap();
        assert!(matches!(
            observations.as_slice(),
            [
                SemanticObservation::ErrorFrame(SemanticFailure {
                    origin: FailureOrigin::Provider,
                    code: error_code,
                    ..
                }),
                SemanticObservation::Failure(SemanticFailure {
                    origin: FailureOrigin::Provider,
                    code: failure_code,
                    ..
                })
            ] if error_code == "rate_limit_exceeded" && failure_code == error_code
        ));
        assert!(matches!(
            inspector.terminal(),
            Some(SemanticTerminal::Failure(SemanticFailure {
                code,
                ..
            })) if code == "rate_limit_exceeded"
        ));
        assert!(inspector.synthesized_failure_from_error_frame().is_some());
        inspector.finish().unwrap();
    }

    #[test]
    fn sse_inspector_rejects_success_after_error_frame() {
        let mut inspector = ResponsesSseInspector::default();
        inspector
            .push(b"event: error\ndata: {\"type\":\"error\",\"error\":{\"code\":\"server_is_overloaded\",\"message\":\"overloaded\"}}\n\n")
            .unwrap();
        let observations = inspector
            .push(b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n")
            .unwrap();
        assert!(matches!(
            observations.as_slice(),
            [SemanticObservation::Failure(SemanticFailure { code, .. })]
                if code == "server_is_overloaded"
        ));
        assert!(matches!(
            inspector.terminal(),
            Some(SemanticTerminal::Failure(_))
        ));
        inspector.finish().unwrap();
    }

    #[test]
    fn sse_inspector_rejects_incomplete_after_error_frame() {
        let mut inspector = ResponsesSseInspector::default();
        inspector
            .push(b"event: error\ndata: {\"type\":\"error\",\"error\":{\"code\":\"server_is_overloaded\",\"message\":\"overloaded\"}}\n\n")
            .unwrap();
        let observations = inspector
            .push(b"event: response.incomplete\ndata: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\"}}\n\n")
            .unwrap();
        assert!(matches!(
            observations.as_slice(),
            [SemanticObservation::Failure(SemanticFailure { code, .. })]
                if code == "server_is_overloaded"
        ));
        assert!(matches!(
            inspector.terminal(),
            Some(SemanticTerminal::Failure(SemanticFailure { code, .. }))
                if code == "server_is_overloaded"
        ));
        inspector.finish().unwrap();
    }

    #[test]
    fn sse_inspector_requires_a_semantic_terminal() {
        let mut inspector = ResponsesSseInspector::default();
        let error = inspector
            .push(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\ndata: [DONE]\n\n")
            .unwrap_err();
        assert!(inspector.saw_business());
        assert!(error.to_string().contains("[DONE]"));
    }

    #[test]
    fn sse_inspector_accepts_done_only_after_a_semantic_terminal() {
        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(
                concat!(
                    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
                    "data: [DONE]\n\n"
                )
                .as_bytes(),
            )
            .unwrap();
        assert_eq!(observations, vec![SemanticObservation::SuccessTerminal]);
        inspector.finish().unwrap();
    }

    #[test]
    fn responses_done_marker_is_unique_after_completion() {
        for (completed, done) in [
            (
                b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n"
                    .as_slice(),
                b"data: [DONE]\n\n".as_slice(),
            ),
            (
                b"{\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n"
                    .as_slice(),
                b"[DONE]\n".as_slice(),
            ),
        ] {
            let mut inspector = ResponsesSseInspector::default();
            inspector.push_normalized(completed).unwrap();
            let first = inspector.push_normalized(done).unwrap();
            assert_eq!(
                String::from_utf8_lossy(&first.normalized).matches("[DONE]").count(),
                1
            );
            let error = inspector.push_normalized(done).unwrap_err();
            assert!(error.to_string().contains("duplicate [DONE]"));

            let mut same_chunk = ResponsesSseInspector::default();
            let mut wire = completed.to_vec();
            wire.extend_from_slice(done);
            wire.extend_from_slice(done);
            let error = same_chunk.push_normalized(&wire).unwrap_err();
            assert!(error.to_string().contains("duplicate [DONE]"));
        }
    }

    #[test]
    fn responses_inspector_rejects_conflicting_terminal_after_completion() {
        let completed =
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n";
        for trailing in [
            "{\"type\":\"response.failed\",\"response\":{\"status\":\"failed\"}}",
            "{\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\"}}",
            "{\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}",
            "{\"type\":\"error\",\"error\":{\"message\":\"late\"}}",
        ] {
            let mut inspector = ResponsesSseInspector::default();
            let stream = format!("{completed}data: {trailing}\n\n");
            assert!(
                inspector.push_normalized(stream.as_bytes()).is_err(),
                "{trailing}"
            );
        }
    }

    #[test]
    fn responses_inspector_cannot_finish_after_a_rejected_push() {
        let completed =
            b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n";
        for trailing in [
            b"data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\"}}\n\n"
                .as_slice(),
            b"data: not-json\n\n".as_slice(),
        ] {
            let mut inspector = ResponsesSseInspector::default();
            inspector.push(completed).unwrap();
            inspector.push(trailing).unwrap_err();
            assert!(inspector.terminal().is_none());
            assert!(inspector.finish().is_err());
            assert!(inspector.push(completed).is_err());
        }
    }

    #[test]
    fn responses_inspector_cannot_accept_a_terminal_after_rejected_finish() {
        let mut inspector = ResponsesSseInspector::default();
        inspector
            .push(b"data: {\"type\":\"response.created\"}\n\n")
            .unwrap();
        assert!(inspector.finish().is_err());
        assert!(inspector
            .push(b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n")
            .is_err());
        assert!(inspector.terminal().is_none());
    }

    #[test]
    fn inspector_accepts_whole_json_stream_documents() {
        let mut inspector = ResponsesSseInspector::default();
        assert!(inspector.push(b"{\"status\":").unwrap().is_empty());
        assert_eq!(
            inspector.push(b"\"completed\",\"output\":[]}").unwrap(),
            vec![SemanticObservation::SuccessTerminal]
        );
        inspector.finish().unwrap();
    }

    #[test]
    fn inspector_accepts_line_delimited_json_events() {
        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(
                concat!(
                    "{\"type\":\"response.created\"}\n",
                    "{\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n",
                    "{\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n",
                    "[DONE]\n"
                )
                .as_bytes(),
            )
            .unwrap();
        assert_eq!(
            observations,
            vec![
                SemanticObservation::Lifecycle,
                SemanticObservation::Business,
                SemanticObservation::SuccessTerminal
            ]
        );
        inspector.finish().unwrap();
    }

    #[test]
    fn inspector_accepts_line_delimited_terminal_without_trailing_newline() {
        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(
                concat!(
                    "{\"type\":\"response.created\"}\n",
                    "{\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}"
                )
                .as_bytes(),
            )
            .unwrap();
        assert_eq!(
            observations,
            vec![
                SemanticObservation::Lifecycle,
                SemanticObservation::SuccessTerminal
            ]
        );
        assert!(inspector.finish().unwrap().is_empty());
    }

    #[test]
    fn line_delimited_json_rejects_failure_after_terminal() {
        let mut inspector = ResponsesSseInspector::default();
        let error = inspector
            .push(
                concat!(
                    "{\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n",
                    "{\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"type\":\"server_error\"}}}\n"
                )
                .as_bytes(),
            )
            .unwrap_err();
        assert!(error.to_string().contains("another terminal event"));
    }

    #[test]
    fn inspector_allows_complete_events_beyond_pending_bound() {
        let max_pending_bytes = 96;
        let max_event_bytes = 4 * 1024;

        let document = serde_json::to_vec(&json!({
            "status": "completed",
            "padding": "x".repeat(max_pending_bytes)
        }))
        .unwrap();
        assert_eq!(
            ResponsesSseInspector::default()
                .push_with_limits(&document, max_pending_bytes, max_event_bytes)
                .unwrap(),
            vec![SemanticObservation::SuccessTerminal]
        );

        let ndjson = format!(
            "{{\"type\":\"response.completed\",\"response\":{{\"status\":\"completed\",\"padding\":\"{}\"}}}}\n",
            "x".repeat(max_pending_bytes)
        );
        assert_eq!(
            ResponsesSseInspector::default()
                .push_with_limits(ndjson.as_bytes(), max_pending_bytes, max_event_bytes)
                .unwrap(),
            vec![SemanticObservation::SuccessTerminal]
        );

        let sse = format!(
            "data: {{\"type\":\"response.completed\",\"response\":{{\"status\":\"completed\",\"padding\":\"{}\"}}}}\n\n",
            "x".repeat(max_pending_bytes)
        );
        assert_eq!(
            ResponsesSseInspector::default()
                .push_with_limits(sse.as_bytes(), max_pending_bytes, max_event_bytes)
                .unwrap(),
            vec![SemanticObservation::SuccessTerminal]
        );
    }

    #[test]
    fn inspector_enforces_pending_and_complete_event_bounds() {
        let max_pending_bytes = 96;
        let max_event_bytes = 192;
        let pending = format!("{{\"status\":\"{}", "x".repeat(max_pending_bytes));
        assert!(ResponsesSseInspector::default()
            .push_with_limits(pending.as_bytes(), max_pending_bytes, max_event_bytes)
            .unwrap_err()
            .to_string()
            .contains("exceeded 96 bytes"));

        let complete = serde_json::to_vec(&json!({
            "status": "completed",
            "padding": "x".repeat(max_event_bytes)
        }))
        .unwrap();
        assert!(ResponsesSseInspector::default()
            .push_with_limits(&complete, max_pending_bytes, max_event_bytes)
            .unwrap_err()
            .to_string()
            .contains("exceeded 192 bytes"));
    }

    #[test]
    fn incomplete_is_a_valid_partial_terminal() {
        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(b"data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\"}}\n\n")
            .unwrap();
        assert_eq!(observations, vec![SemanticObservation::IncompleteTerminal]);
        assert_eq!(inspector.terminal(), Some(&SemanticTerminal::Incomplete));
        inspector.finish().unwrap();
    }

    #[test]
    fn standard_responses_stream_filters_private_and_unknown_events_by_both_names() {
        let stream = concat!(
            "event: responsesapi.websocket_timing\n",
            "data: {\"type\":\"responsesapi.websocket_timing\",\"latency_ms\":1}\n\n",
            "data: {\"type\":\"codex.response.metadata\",\"trace\":\"private\"}\n\n",
            "data: {\"anonymous_private_payload\":true}\n\n",
            "event: codex.rate_limits\n",
            "data: {\"remaining\":1}\n\n",
            "event: response.future.delta\n",
            "data: {\"type\":\"response.future.delta\",\"delta\":\"private until reviewed\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"public\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
            "data: [DONE]\n\n"
        );
        let mut inspector = ResponsesSseInspector::with_repeat_guard_and_visibility(
            false,
            ResponsesEventVisibility::StandardClient,
        );
        let mut normalized = inspector
            .push_normalized(stream.as_bytes())
            .unwrap()
            .normalized;
        let tail = inspector.finish_normalized().unwrap().normalized;
        normalized = [normalized.as_ref(), tail.as_ref()].concat().into();
        let normalized = String::from_utf8(normalized.to_vec()).unwrap();

        assert!(normalized.contains("response.output_text.delta"));
        assert!(normalized.contains("response.completed"));
        assert!(!normalized.contains("responsesapi."));
        assert!(!normalized.contains("codex."));
        assert!(!normalized.contains("anonymous_private_payload"));
        assert!(!normalized.contains("response.future.delta"));
    }

    #[test]
    fn codex_r4_metadata_has_no_business_or_terminal_semantics() {
        for event in [
            json!({"type":"response.metadata","response_id":"r","sequence_number":2,
                "metadata":{"openai_chatgpt_moderation_metadata":{"prompt":{}}}}),
            json!({"type":"codex.response.metadata","headers":{"x-codex-primary-used-percent":"0"}}),
            json!({"type":"codex.rate_limits","rate_limits":{"primary":{"used_percent":0}}}),
            json!({"type":"rate_limits.updated","rate_limit":{"primary_window":{"used_percent":0}}}),
        ] {
            let observation = classify_value(&event);
            assert!(!observation.counts_as_business_output(), "{event}");
            assert!(!observation.commits_downstream(), "{event}");
            assert!(!is_responses_terminal_event(None, &event), "{event}");
            assert_eq!(observation.metric_kind(), "metadata");
        }
    }

    #[test]
    fn codex_r4_metadata_never_masks_generation_errors_or_unknown_content() {
        for event in [
            json!({"type":"response.metadata","error":{"code":"server_error","message":"failed"}}),
            json!({"type":"codex.response.metadata","response":{"status":"failed","error":{"code":"server_error"}}}),
        ] {
            assert!(matches!(
                classify_value(&event),
                SemanticObservation::Failure(_)
            ));
        }
        for event in [
            json!({"type":"response.future.delta","delta":"unknown content"}),
            json!({"type":"codex.future.metadata","metadata":{}}),
            json!({"type":"response.metadata","delta":"real output"}),
            json!({"type":"response.metadata","item":{"type":"web_search_call"}}),
            json!({"type":"response.metadata","new_field":"unreviewed"}),
        ] {
            assert_eq!(
                classify_value(&event),
                SemanticObservation::Business,
                "{event}"
            );
        }
    }

    #[test]
    fn codex_r4_native_sse_metadata_does_not_satisfy_content_or_eof() {
        let mut inspector = ResponsesSseInspector::with_repeat_guard_and_visibility(
            false,
            ResponsesEventVisibility::NativeCodexClient,
        );
        let batch = inspector
            .push_normalized(
                b"data: {\"type\":\"codex.response.metadata\",\"response_id\":\"r\"}\n\n",
            )
            .unwrap();
        assert!(!batch.normalized.is_empty());
        assert!(!inspector.saw_business());
        assert!(batch.observations.iter().all(|o| !o.commits_downstream()));
        assert!(inspector.finish().is_err());
    }

    #[test]
    fn codex_r4_sse_metadata_visibility_and_overload_are_separate() {
        for visibility in [
            ResponsesEventVisibility::Unfiltered,
            ResponsesEventVisibility::StandardClient,
            ResponsesEventVisibility::NativeCodexClient,
        ] {
            let mut inspector =
                ResponsesSseInspector::with_repeat_guard_and_visibility(false, visibility);
            let metadata = inspector.push_normalized(concat!(
                "event: codex.response.metadata\ndata: {\"headers\":{\"x-codex-primary-used-percent\":\"0\"}}\n\n",
                "data: {\"type\":\"response.metadata\",\"metadata\":{}}\n\n",
                "data: {\"type\":\"codex.rate_limits\",\"rate_limits\":{}}\n\n"
            ).as_bytes()).unwrap();
            assert!(!inspector.saw_business());
            assert!(metadata
                .observations
                .iter()
                .all(|observation| observation.metric_kind() == "metadata"));
            if visibility == ResponsesEventVisibility::StandardClient {
                assert!(metadata.normalized.is_empty());
            }
            let failed = inspector.push_normalized(b"data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_is_overloaded\",\"message\":\"overloaded\"}}}\n\n").unwrap();
            assert!(matches!(
                failed.observations.as_slice(),
                [SemanticObservation::Failure(_)]
            ));
            inspector.finish().unwrap();
        }
    }

    #[test]
    fn codex_r4_sse_names_cannot_relabel_content_as_metadata() {
        for stream in [
            "event: codex.response.metadata\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"visible\"}\n\n",
            "event: response.output_text.delta\ndata: {\"type\":\"codex.response.metadata\",\"metadata\":{}}\n\n",
        ] {
            let mut inspector = ResponsesSseInspector::with_repeat_guard_and_visibility(
                false, ResponsesEventVisibility::NativeCodexClient,
            );
            assert!(inspector.push_normalized(stream.as_bytes()).is_err());
        }
    }

    #[test]
    fn native_codex_stream_allows_only_reviewed_metadata_beyond_public_events() {
        let stream = concat!(
            "data: {\"type\":\"codex.response.metadata\",\"response_id\":\"resp_1\"}\n\n",
            "event: codex.rate_limits\n",
            "data: {\"type\":\"codex.rate_limits\",\"remaining\":1}\n\n",
            "data: {\"type\":\"codex.secret.future\",\"value\":true}\n\n",
            "event: responsesapi.telemetry\n",
            "data: {\"type\":\"responsesapi.telemetry\"}\n\n",
            "event: response.failed\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"message\":\"visible failure\"}}}\n\n"
        );
        let mut inspector = ResponsesSseInspector::with_repeat_guard_and_visibility(
            false,
            ResponsesEventVisibility::NativeCodexClient,
        );
        let batch = inspector.push_normalized(stream.as_bytes()).unwrap();
        let normalized = String::from_utf8(batch.normalized.to_vec()).unwrap();

        assert!(normalized.contains("codex.response.metadata"));
        assert!(normalized.contains("response.failed"));
        assert!(normalized.contains("visible failure"));
        assert!(!normalized.contains("codex.rate_limits"));
        assert!(!normalized.contains("codex.secret.future"));
        assert!(!normalized.contains("responsesapi.telemetry"));
        assert!(matches!(
            inspector.terminal(),
            Some(SemanticTerminal::Failure(_))
        ));
        inspector.finish().unwrap();
    }

    #[test]
    fn reported_response_model_is_bounded_and_terminal_authoritative() {
        let mut observer = ReportedResponseModelObserver::default();
        observer.observe_event_value(&json!({
            "type": "response.created",
            "response": {"model": "gpt-5.5"}
        }));
        assert_eq!(observer.terminal_observation(), None);
        observer.observe_event_value(&json!({
            "type": "response.output_text.delta",
            "response": {"model": "ignored-business-frame"}
        }));
        observer.observe_event_value(&json!({
            "type": "response.completed",
            "response": {"model": "gpt-5.5-2026-09-01"}
        }));
        assert_eq!(
            observer.observation(),
            Some(ReportedResponseModel {
                model: "gpt-5.5-2026-09-01".to_string(),
                conflict: true,
            })
        );
        assert_eq!(observer.terminal_observation(), observer.observation());

        let mut invalid = ReportedResponseModelObserver::default();
        invalid.observe_document_value(&json!({"model": "gpt-5.5\nsecret"}));
        invalid.observe_document_value(&json!({
            "model": "x".repeat(REPORTED_RESPONSE_MODEL_MAX_BYTES + 1)
        }));
        assert_eq!(invalid.observation(), None);
    }

    #[test]
    fn responses_inspector_observes_reported_model_without_changing_wire() {
        let mut inspector = ResponsesSseInspector::with_repeat_guard_and_visibility(
            false,
            ResponsesEventVisibility::StandardClient,
        );
        inspector
            .push_normalized(
                concat!(
                    "event: response.created\n",
                    "data: {\"type\":\"response.created\",\"response\":{\"model\":\"gpt-5.5\"}}\n\n",
                    "event: response.completed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"model\":\"gpt-5.5\"}}\n\n"
                )
                .as_bytes(),
            )
            .unwrap();
        assert_eq!(
            inspector.reported_model(),
            Some(ReportedResponseModel {
                model: "gpt-5.5".to_string(),
                conflict: false,
            })
        );
    }

    #[test]
    fn proxy_bridge_contract_fixture_distinguishes_failure_origins() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/proxy_bridge/failure_origin.json"
        ))
        .unwrap();
        assert_eq!(fixture["id"], "semantic-failure-origin");
        assert_eq!(fixture["category"], "failure_origin");

        for case in fixture["cases"].as_array().unwrap() {
            let observation = classify_value(&case["input"]);
            assert_eq!(
                observation.metric_kind(),
                case["expected"]["metricKind"].as_str().unwrap(),
                "fixture case {}",
                case["name"]
            );
            assert_eq!(
                observation.commits_downstream(),
                case["expected"]["commitsDownstream"].as_bool().unwrap()
            );
            let SemanticObservation::Failure(failure) = observation else {
                panic!("fixture case {} must classify as failure", case["name"]);
            };
            assert_eq!(failure.origin.as_str(), case["expected"]["origin"]);
            assert_eq!(failure.code, case["expected"]["code"]);
            assert_eq!(
                failure.origin == FailureOrigin::Provider,
                case["expected"]["eligibleForPreCommitFailover"]
                    .as_bool()
                    .unwrap()
            );
        }
    }

    #[test]
    fn proxy_bridge_contract_fixture_accepts_incomplete_terminals() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/proxy_bridge/incomplete_terminal.json"
        ))
        .unwrap();
        assert_eq!(fixture["id"], "incomplete-valid-terminal");
        assert_eq!(fixture["category"], "incomplete_terminal");

        for document in fixture["documents"].as_array().unwrap() {
            let observation = classify_value(document);
            assert_eq!(
                observation.metric_kind(),
                fixture["expected"]["metricKind"].as_str().unwrap()
            );
            assert_eq!(observation, SemanticObservation::IncompleteTerminal);
        }

        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(fixture["sse"].as_str().unwrap().as_bytes())
            .unwrap();
        assert!(observations.contains(&SemanticObservation::IncompleteTerminal));
        inspector.finish().unwrap();
        let terminal = inspector.terminal().unwrap();
        assert_eq!(
            terminal.stream_status(),
            fixture["expected"]["streamStatus"].as_str().unwrap()
        );
        assert_eq!(terminal, &SemanticTerminal::Incomplete);
    }
}
