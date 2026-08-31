use std::collections::BTreeMap;
use std::time::Duration;

use axum::http::header::{
    ACCEPT, CACHE_CONTROL, CONTENT_TYPE, COOKIE, ORIGIN, REFERER, USER_AGENT,
};
use axum::http::{HeaderValue, StatusCode};
use bytes::{Bytes, BytesMut};
use rand::RngCore;
use serde_json::{json, Map, Value};
use zeroize::Zeroizing;

use crate::domain::providers::credentials::reveal_provider_credential;
use crate::domain::providers::registry::AuthScheme;
use crate::domain::providers::runtime::{RuntimeAuthRef, RuntimeModelPolicy};
use crate::domain::providers::web_session::{
    guard_exact_request, web_session_profile_for_driver, ParsedWebSessionCredential,
    WebSessionMethod, WebSessionProfileSpec, WebSessionScope, GROK_WEB_SESSION_DRIVER_ID,
    PERPLEXITY_WEB_SESSION_DRIVER_ID, WEB_SESSION_CREDENTIAL_SLOT,
};
use crate::state::{ServerState, WebSessionStateError};

use super::provider_ops::ProviderExecution;
use super::router::ProxyRoute;
use super::ProxyError;

const MAX_WEB_SESSION_FRAME_BYTES: usize = 1024 * 1024;
const GROK_WEB_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";
const PERPLEXITY_WEB_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:148.0) Gecko/20100101 Firefox/148.0";
const PERPLEXITY_API_VERSION: &str = "2.18";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebSessionKind {
    Grok,
    Perplexity,
}

impl WebSessionKind {
    fn for_driver(driver_id: &str) -> Option<Self> {
        match driver_id {
            GROK_WEB_SESSION_DRIVER_ID => Some(Self::Grok),
            PERPLEXITY_WEB_SESSION_DRIVER_ID => Some(Self::Perplexity),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReviewedWebSessionModel {
    pub id: &'static str,
    pub display_name: &'static str,
    pub upstream_selector: &'static str,
}

const GROK_MODELS: &[ReviewedWebSessionModel] = &[
    ReviewedWebSessionModel {
        id: "fast",
        display_name: "Grok Web Fast",
        upstream_selector: "fast",
    },
    ReviewedWebSessionModel {
        id: "expert",
        display_name: "Grok Web Expert",
        upstream_selector: "expert",
    },
    ReviewedWebSessionModel {
        id: "heavy",
        display_name: "Grok Web Heavy",
        upstream_selector: "heavy",
    },
];

const PERPLEXITY_MODELS: &[ReviewedWebSessionModel] = &[
    ReviewedWebSessionModel {
        id: "pplx-auto",
        display_name: "Perplexity Web Auto",
        upstream_selector: "pplx_pro",
    },
    ReviewedWebSessionModel {
        id: "pplx-sonar",
        display_name: "Perplexity Web Sonar",
        upstream_selector: "turbo",
    },
    ReviewedWebSessionModel {
        id: "pplx-sonnet",
        display_name: "Perplexity Web Sonnet",
        upstream_selector: "claude50sonnet",
    },
    ReviewedWebSessionModel {
        id: "pplx-opus",
        display_name: "Perplexity Web Opus",
        upstream_selector: "claude50opus",
    },
];

pub(crate) fn reviewed_models_for_driver(
    driver_id: &str,
) -> Option<&'static [ReviewedWebSessionModel]> {
    match WebSessionKind::for_driver(driver_id)? {
        WebSessionKind::Grok => Some(GROK_MODELS),
        WebSessionKind::Perplexity => Some(PERPLEXITY_MODELS),
    }
}

pub(crate) fn is_web_session_driver(driver_id: &str) -> bool {
    WebSessionKind::for_driver(driver_id).is_some()
}

pub(crate) fn preflight_actual_model(
    execution: &ProviderExecution,
    route: ProxyRoute,
    body: &[u8],
) -> Result<String, ProxyError> {
    let kind = WebSessionKind::for_driver(execution.plan.driver_id.as_str())
        .ok_or_else(|| ProxyError::bad_request("Provider is not a Web Session Driver"))?;
    let request = parse_downstream_request(route, body)?;
    resolve_model(execution, kind, &request.requested_model).map(|selection| selection.actual)
}

#[derive(Debug)]
pub(crate) struct WebSessionForwardResult {
    pub downstream_body: Bytes,
    pub downstream_content_type: &'static str,
    pub stream_requested: bool,
    pub requested_model: String,
    pub actual_model: String,
    pub actual_model_source: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub upstream_status: u16,
    pub local_only: bool,
}

#[derive(Debug, Clone)]
struct CanonicalMessage {
    role: String,
    content: String,
}

#[derive(Debug)]
struct CanonicalRequest {
    requested_model: String,
    stream_requested: bool,
    system: Vec<String>,
    messages: Vec<CanonicalMessage>,
    input_tokens: u64,
}

#[derive(Debug)]
struct ModelSelection {
    requested: String,
    actual: String,
    actual_source: Option<String>,
    upstream_selector: &'static str,
}

#[derive(Debug)]
struct ParsedUpstreamCompletion {
    text: String,
    session_id: Option<String>,
}

pub(crate) async fn execute(
    state: &ServerState,
    execution: &ProviderExecution,
    route: ProxyRoute,
    body: Bytes,
) -> Result<WebSessionForwardResult, ProxyError> {
    let driver_id = execution.plan.driver_id.as_str();
    let kind = WebSessionKind::for_driver(driver_id)
        .ok_or_else(|| ProxyError::bad_request("Provider is not a Web Session Driver"))?;
    let profile = web_session_profile_for_driver(driver_id)
        .ok_or_else(|| ProxyError::bad_request("Web Session Driver has no reviewed profile"))?;
    if body.len() > profile.request_body_limit_bytes {
        return Err(ProxyError {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: format!(
                "Web Session request exceeds the {} byte profile limit",
                profile.request_body_limit_bytes
            ),
        });
    }
    let request = parse_downstream_request(route, &body)?;
    let selection = resolve_model(execution, kind, &request.requested_model)?;

    if route == ProxyRoute::ClaudeCountTokens {
        return Ok(WebSessionForwardResult {
            downstream_body: Bytes::from(
                serde_json::to_vec(&json!({"input_tokens": request.input_tokens}))
                    .map_err(|error| ProxyError::bad_gateway(error.to_string()))?,
            ),
            downstream_content_type: "application/json",
            stream_requested: false,
            requested_model: selection.requested,
            actual_model: selection.actual,
            actual_model_source: selection.actual_source,
            input_tokens: request.input_tokens,
            output_tokens: 0,
            upstream_status: StatusCode::OK.as_u16(),
            local_only: true,
        });
    }

    validate_runtime_credential_contract(execution)?;
    let scope = scope_for_execution(execution, profile)?;
    state
        .prepare_web_session_scope(&scope)
        .await
        .map_err(map_state_error)?;

    let raw_cookie = Zeroizing::new(
        reveal_provider_credential(&execution.stored.provider, WEB_SESSION_CREDENTIAL_SLOT)
            .map_err(|_| {
                ProxyError::bad_request("Web Session Provider credential is not configured")
            })?,
    );
    let credential = ParsedWebSessionCredential::parse(profile, raw_cookie.as_str())
        .map_err(|error| ProxyError::bad_request(error.to_string()))?;

    let endpoint = validate_runtime_endpoint(profile, &execution.plan.endpoint)?;
    let request_id = random_uuid_like();
    let upstream_body = match kind {
        WebSessionKind::Grok => build_grok_request(&request, selection.upstream_selector),
        WebSessionKind::Perplexity => {
            build_perplexity_request(&request, selection.upstream_selector, &request_id)?
        }
    };
    let encoded = serde_json::to_vec(&upstream_body)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_gateway(format!("encode Web Session request: {error}")))?;
    if encoded.len() > profile.request_body_limit_bytes {
        return Err(ProxyError {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: "translated Web Session request exceeds the reviewed profile limit"
                .to_string(),
        });
    }

    let client = state.web_session_http_client().await;
    let mut request_builder = client
        .post(&endpoint)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .header(ORIGIN, &profile.fixed_origin)
        .header(REFERER, format!("{}/", profile.fixed_origin))
        .header(COOKIE, credential.cookie_header())
        .header(CACHE_CONTROL, HeaderValue::from_static("no-cache"))
        .body(encoded);
    request_builder = match kind {
        WebSessionKind::Grok => request_builder
            .header(ACCEPT, HeaderValue::from_static("*/*"))
            .header(USER_AGENT, HeaderValue::from_static(GROK_WEB_USER_AGENT))
            .header("x-xai-request-id", &request_id),
        WebSessionKind::Perplexity => request_builder
            .header(ACCEPT, HeaderValue::from_static("text/event-stream"))
            .header(
                USER_AGENT,
                HeaderValue::from_static(PERPLEXITY_WEB_USER_AGENT),
            )
            .header("x-perplexity-request-endpoint", endpoint)
            .header("x-perplexity-request-reason", "ask-query-state-provider")
            .header("x-perplexity-request-try-number", "1")
            .header("x-request-id", &request_id),
    };

    let response = request_builder
        .timeout(execution.request_timeout())
        .send()
        .await
        .map_err(|error| network_error("send", error))?;
    let status = response.status();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        state
            .invalidate_web_session_authentication(&scope)
            .await
            .map_err(map_state_error)?;
        return Err(ProxyError {
            status,
            message: format!(
                "{} authentication failed; explicitly re-import the reviewed session Cookie",
                profile.label
            ),
        });
    }
    if !status.is_success() {
        let mapped = if status == StatusCode::TOO_MANY_REQUESTS {
            StatusCode::TOO_MANY_REQUESTS
        } else if status.is_redirection() {
            StatusCode::BAD_GATEWAY
        } else {
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY)
        };
        let message = if status.is_redirection() {
            format!(
                "{} returned HTTP {}; redirects are disabled for Web Session credentials",
                profile.label,
                status.as_u16()
            )
        } else {
            format!("{} returned HTTP {}", profile.label, status.as_u16())
        };
        return Err(ProxyError {
            status: mapped,
            message,
        });
    }
    validate_response_content_type(kind, response.headers().get(CONTENT_TYPE))?;
    let response_body = read_response_body_strict(
        response,
        profile.response_body_limit_bytes,
        execution
            .stream_first_byte_timeout()
            .unwrap_or_else(|| execution.request_timeout()),
        execution
            .stream_idle_timeout()
            .unwrap_or_else(|| execution.request_timeout()),
        execution.request_timeout(),
    )
    .await?;
    let parsed = match kind {
        WebSessionKind::Grok => parse_grok_ndjson(&response_body)?,
        WebSessionKind::Perplexity => parse_perplexity_sse(&response_body)?,
    };

    state
        .record_web_session_success(&scope, parsed.session_id.as_deref())
        .await
        .map_err(map_state_error)?;

    let output_tokens = estimate_tokens(&parsed.text);
    let (downstream_body, downstream_content_type) = render_downstream(
        route,
        request.stream_requested,
        &selection.requested,
        &parsed.text,
        request.input_tokens,
        output_tokens,
    )?;
    Ok(WebSessionForwardResult {
        downstream_body,
        downstream_content_type,
        stream_requested: request.stream_requested,
        requested_model: selection.requested,
        actual_model: selection.actual,
        actual_model_source: selection.actual_source,
        input_tokens: request.input_tokens,
        output_tokens,
        upstream_status: status.as_u16(),
        local_only: false,
    })
}

fn validate_runtime_credential_contract(execution: &ProviderExecution) -> Result<(), ProxyError> {
    match &execution.plan.auth_ref {
        RuntimeAuthRef::StaticCredential {
            auth_scheme: AuthScheme::None,
            slots,
            credential_generation,
        } if slots.as_slice() == [WEB_SESSION_CREDENTIAL_SLOT]
            && *credential_generation == execution.stored.resource.credential_generation
            && execution.plan.extra_headers.is_empty() =>
        {
            Ok(())
        }
        _ => Err(ProxyError::bad_request(
            "Web Session Driver requires one Provider-owned Cookie slot, no Account, no API Key, and no extra headers",
        )),
    }
}

fn scope_for_execution(
    execution: &ProviderExecution,
    profile: &WebSessionProfileSpec,
) -> Result<WebSessionScope, ProxyError> {
    let credential_generation = match &execution.plan.auth_ref {
        RuntimeAuthRef::StaticCredential {
            auth_scheme: AuthScheme::None,
            slots,
            credential_generation,
        } if slots.as_slice() == [WEB_SESSION_CREDENTIAL_SLOT] => *credential_generation,
        _ => {
            return Err(ProxyError::bad_request(
                "Web Session runtime credential contract is incomplete",
            ))
        }
    };
    let scope = WebSessionScope {
        provider_key: execution.plan.provider_key.clone(),
        provider_revision: execution.plan.provider_revision,
        runtime_fingerprint: execution.plan.runtime_fingerprint.clone(),
        credential_generation,
        profile_id: profile.profile_id.clone(),
        fixed_origin: profile.fixed_origin.clone(),
    };
    scope
        .validate()
        .map_err(|error| ProxyError::bad_request(error.to_string()))?;
    Ok(scope)
}

fn map_state_error(error: WebSessionStateError) -> ProxyError {
    match error {
        WebSessionStateError::IdentityChanged => ProxyError::conflict(
            "Web Session Provider runtime or credential generation changed during the request",
        ),
        WebSessionStateError::ExplicitReimportRequired => ProxyError {
            status: StatusCode::UNAUTHORIZED,
            message: "Web Session authentication was invalidated; explicitly re-import the reviewed Cookie"
                .to_string(),
        },
        WebSessionStateError::Invalid(message) => ProxyError::bad_request(message),
    }
}

fn validate_runtime_endpoint(
    profile: &WebSessionProfileSpec,
    candidate: &str,
) -> Result<String, ProxyError> {
    if guard_exact_request(profile, WebSessionMethod::Post, candidate).is_ok() {
        return Ok(candidate.to_string());
    }
    #[cfg(test)]
    {
        let parsed = url::Url::parse(candidate)
            .map_err(|_| ProxyError::bad_request("invalid Web Session test endpoint"))?;
        let loopback = parsed.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
        if parsed.scheme() == "http"
            && loopback
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.path() == profile.path
            && parsed.query().is_none()
            && parsed.fragment().is_none()
        {
            return Ok(candidate.to_string());
        }
    }
    Err(ProxyError::bad_request(
        "Web Session Driver endpoint does not match its fixed reviewed origin and path",
    ))
}

fn resolve_model(
    execution: &ProviderExecution,
    kind: WebSessionKind,
    requested_model: &str,
) -> Result<ModelSelection, ProxyError> {
    let (actual, actual_source) = match &execution.plan.model_policy {
        RuntimeModelPolicy::Single { upstream_model } => (
            upstream_model.clone(),
            (upstream_model != requested_model).then(|| "provider_single_model".to_string()),
        ),
        RuntimeModelPolicy::Passthrough => (requested_model.to_string(), None),
    };
    let models = match kind {
        WebSessionKind::Grok => GROK_MODELS,
        WebSessionKind::Perplexity => PERPLEXITY_MODELS,
    };
    let reviewed = models
        .iter()
        .find(|model| model.id == actual)
        .ok_or_else(|| {
            ProxyError::bad_request(format!(
                "Web Session model {actual} is outside the reviewed fixture catalog"
            ))
        })?;
    Ok(ModelSelection {
        requested: requested_model.to_string(),
        actual,
        actual_source,
        upstream_selector: reviewed.upstream_selector,
    })
}

fn parse_downstream_request(
    route: ProxyRoute,
    body: &[u8],
) -> Result<CanonicalRequest, ProxyError> {
    let value: Value = serde_json::from_slice(body).map_err(|error| {
        ProxyError::bad_request(format!("invalid downstream JSON body: {error}"))
    })?;
    let object = value
        .as_object()
        .ok_or_else(|| ProxyError::bad_request("downstream Web Session body must be an object"))?;
    reject_unsupported_request_features(object)?;
    let requested_model = object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| ProxyError::bad_request("Web Session request is missing model"))?
        .to_string();
    let stream_requested = object
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let (system, messages) = match route {
        ProxyRoute::ClaudeMessages | ProxyRoute::ClaudeCountTokens => {
            parse_claude_messages(object)?
        }
        ProxyRoute::CodexChatCompletions => parse_chat_messages(object)?,
        ProxyRoute::CodexResponses => parse_responses_input(object)?,
        _ => {
            return Err(ProxyError::bad_request(
                "Web Session Driver supports Claude Messages/count_tokens, Chat Completions, and Responses text routes only",
            ))
        }
    };
    if messages.is_empty() || !messages.iter().any(|message| message.role == "user") {
        return Err(ProxyError::bad_request(
            "Web Session request requires at least one non-empty user text message",
        ));
    }
    let input_text = system
        .iter()
        .map(String::as_str)
        .chain(messages.iter().map(|message| message.content.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(CanonicalRequest {
        requested_model,
        stream_requested,
        system,
        messages,
        input_tokens: estimate_tokens(&input_text),
    })
}

fn reject_unsupported_request_features(object: &Map<String, Value>) -> Result<(), ProxyError> {
    if object
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty())
        || object.get("tool_choice").is_some_and(|choice| {
            !choice.is_null()
                && choice.as_str().is_none_or(|value| {
                    !matches!(value.trim().to_ascii_lowercase().as_str(), "none" | "auto")
                })
        })
        || object.get("functions").is_some()
    {
        return Err(ProxyError {
            status: StatusCode::NOT_IMPLEMENTED,
            message: "Web Session text Drivers do not support client tools or function calls"
                .to_string(),
        });
    }
    for key in [
        "images",
        "image",
        "attachments",
        "previous_response_id",
        "background",
    ] {
        if object.get(key).is_some_and(|value| !value.is_null()) {
            return Err(ProxyError {
                status: StatusCode::NOT_IMPLEMENTED,
                message: format!("Web Session text Drivers do not support {key}"),
            });
        }
    }
    Ok(())
}

fn parse_claude_messages(
    object: &Map<String, Value>,
) -> Result<(Vec<String>, Vec<CanonicalMessage>), ProxyError> {
    let system = object
        .get("system")
        .map(|value| extract_text_content(value, &["text"], "Claude system"))
        .transpose()?
        .filter(|text| !text.trim().is_empty())
        .into_iter()
        .collect::<Vec<_>>();
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| ProxyError::bad_request("Claude Web Session request needs messages"))?;
    parse_message_array(messages, &["text"], false).map(|messages| (system, messages))
}

fn parse_chat_messages(
    object: &Map<String, Value>,
) -> Result<(Vec<String>, Vec<CanonicalMessage>), ProxyError> {
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| ProxyError::bad_request("Chat Web Session request needs messages"))?;
    let parsed = parse_message_array(messages, &["text"], true)?;
    let mut system = Vec::new();
    let mut conversation = Vec::new();
    for message in parsed {
        if matches!(message.role.as_str(), "system" | "developer") {
            system.push(message.content);
        } else {
            conversation.push(message);
        }
    }
    Ok((system, conversation))
}

fn parse_responses_input(
    object: &Map<String, Value>,
) -> Result<(Vec<String>, Vec<CanonicalMessage>), ProxyError> {
    let mut system = object
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| vec![value.to_string()])
        .unwrap_or_default();
    let input = object
        .get("input")
        .ok_or_else(|| ProxyError::bad_request("Responses Web Session request needs input"))?;
    let mut messages = Vec::new();
    match input {
        Value::String(text) if !text.trim().is_empty() => messages.push(CanonicalMessage {
            role: "user".to_string(),
            content: text.clone(),
        }),
        Value::Array(items) => {
            for item in items {
                let item = item.as_object().ok_or_else(|| {
                    ProxyError::bad_request("Responses input items must be message objects")
                })?;
                let item_type = item
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("message");
                if item_type != "message" {
                    return Err(ProxyError {
                        status: StatusCode::NOT_IMPLEMENTED,
                        message: format!(
                            "Web Session text Drivers do not support Responses input item {item_type}"
                        ),
                    });
                }
                let role = item
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("user")
                    .trim()
                    .to_ascii_lowercase();
                let content = extract_text_content(
                    item.get("content").unwrap_or(&Value::Null),
                    &["input_text", "output_text", "text"],
                    "Responses input",
                )?;
                if content.trim().is_empty() {
                    continue;
                }
                if matches!(role.as_str(), "system" | "developer") {
                    system.push(content);
                } else if matches!(role.as_str(), "user" | "assistant") {
                    messages.push(CanonicalMessage { role, content });
                } else {
                    return Err(ProxyError::bad_request(format!(
                        "unsupported Responses message role {role}"
                    )));
                }
            }
        }
        _ => {
            return Err(ProxyError::bad_request(
                "Responses Web Session input must be non-empty text or message items",
            ))
        }
    }
    Ok((system, messages))
}

fn parse_message_array(
    messages: &[Value],
    text_types: &[&str],
    allow_system_roles: bool,
) -> Result<Vec<CanonicalMessage>, ProxyError> {
    let mut parsed = Vec::new();
    for message in messages {
        let object = message
            .as_object()
            .ok_or_else(|| ProxyError::bad_request("message must be an object"))?;
        if object.get("tool_calls").is_some() || object.get("tool_call_id").is_some() {
            return Err(ProxyError {
                status: StatusCode::NOT_IMPLEMENTED,
                message: "Web Session text Drivers do not support tool history".to_string(),
            });
        }
        let role = object
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user")
            .trim()
            .to_ascii_lowercase();
        let valid = matches!(role.as_str(), "user" | "assistant")
            || (allow_system_roles && matches!(role.as_str(), "system" | "developer"));
        if !valid {
            return Err(ProxyError::bad_request(format!(
                "unsupported Web Session message role {role}"
            )));
        }
        let content = extract_text_content(
            object.get("content").unwrap_or(&Value::Null),
            text_types,
            "message content",
        )?;
        if !content.trim().is_empty() {
            parsed.push(CanonicalMessage { role, content });
        }
    }
    Ok(parsed)
}

fn extract_text_content(
    value: &Value,
    text_types: &[&str],
    label: &str,
) -> Result<String, ProxyError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Array(items) => {
            let mut parts = Vec::new();
            for item in items {
                if let Value::String(text) = item {
                    parts.push(text.clone());
                    continue;
                }
                let object = item.as_object().ok_or_else(|| {
                    ProxyError::bad_request(format!("{label} contains a non-text item"))
                })?;
                let item_type = object.get("type").and_then(Value::as_str).unwrap_or("text");
                if !text_types.contains(&item_type) {
                    return Err(ProxyError {
                        status: StatusCode::NOT_IMPLEMENTED,
                        message: format!(
                            "Web Session text Drivers do not support {label} item {item_type}"
                        ),
                    });
                }
                let text = object
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ProxyError::bad_request(format!("{label} text is missing")))?;
                parts.push(text.to_string());
            }
            Ok(parts.join("\n"))
        }
        Value::Null => Ok(String::new()),
        _ => Err(ProxyError::bad_request(format!(
            "{label} must contain text only"
        ))),
    }
}

fn build_grok_request(request: &CanonicalRequest, mode: &str) -> Value {
    json!({
        "temporary": true,
        "modeId": mode,
        "message": render_conversation(request),
        "fileAttachments": [],
        "imageAttachments": [],
        "disableSearch": false,
        "enableImageGeneration": false,
        "returnImageBytes": false,
        "returnRawGrokInXaiRequest": false,
        "enableImageStreaming": false,
        "imageGenerationCount": 0,
        "forceConcise": false,
        "toolOverrides": {},
        "enableSideBySide": true,
        "sendFinalMetadata": true,
        "isReasoning": false,
        "disableTextFollowUps": false,
        "disableMemory": true,
        "forceSideBySide": false,
        "isAsyncChat": false,
        "disableSelfHarmShortCircuit": false,
        "deviceEnvInfo": {
            "darkModeEnabled": false,
            "devicePixelRatio": 2,
            "screenWidth": 2056,
            "screenHeight": 1329,
            "viewportWidth": 2056,
            "viewportHeight": 1083
        }
    })
}

fn build_perplexity_request(
    request: &CanonicalRequest,
    model_preference: &str,
    request_id: &str,
) -> Result<Value, ProxyError> {
    let last_user = request
        .messages
        .iter()
        .rposition(|message| message.role == "user")
        .ok_or_else(|| ProxyError::bad_request("Perplexity request needs a final user query"))?;
    if request.messages[last_user + 1..]
        .iter()
        .any(|message| !message.content.trim().is_empty())
    {
        return Err(ProxyError::bad_request(
            "Perplexity Web Session requires the final non-empty message to be from the user",
        ));
    }
    let current = request.messages[last_user].content.clone();
    let history = request.messages[..last_user]
        .iter()
        .map(|message| json!({"role": message.role, "content": message.content}))
        .collect::<Vec<_>>();
    let mut query = Map::new();
    if !request.system.is_empty() {
        query.insert("instructions".to_string(), json!(request.system));
    }
    if !history.is_empty() {
        query.insert("history".to_string(), Value::Array(history));
    }
    query.insert("query".to_string(), json!(current));
    let query = serde_json::to_string(&Value::Object(query))
        .map_err(|error| ProxyError::bad_gateway(error.to_string()))?;
    if query.len() > 96_000 {
        return Err(ProxyError {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: "Perplexity Web Session query exceeds the reviewed 96 KiB protocol limit"
                .to_string(),
        });
    }
    Ok(json!({
        "query_str": query,
        "params": {
            "attachments": [],
            "language": "en-US",
            "timezone": "UTC",
            "search_focus": "internet",
            "sources": ["web"],
            "frontend_uuid": request_id,
            "mode": "copilot",
            "model_preference": model_preference,
            "is_related_query": false,
            "is_sponsored": false,
            "frontend_context_uuid": random_uuid_like(),
            "prompt_source": "user",
            "query_source": "home",
            "is_incognito": true,
            "local_search_enabled": false,
            "use_schematized_api": true,
            "send_back_text_in_streaming_api": false,
            "supported_block_use_cases": [
                "answer_modes", "media_items", "knowledge_cards", "inline_entity_cards",
                "search_result_widgets", "inline_images", "diff_blocks", "workflow_steps",
                "workflow_widgets", "preserve_latex"
            ],
            "client_coordinates": null,
            "mentions": [],
            "dsl_query": request.messages[last_user].content,
            "skip_search_enabled": true,
            "is_nav_suggestions_disabled": false,
            "source": "default",
            "always_search_override": false,
            "override_no_search": false,
            "client_search_results_cache_key": request_id,
            "should_ask_for_mcp_tool_confirmation": true,
            "supports_tool_approval_modal": true,
            "browser_agent_allow_once_from_toggle": false,
            "force_enable_browser_agent": false,
            "supported_features": ["browser_agent_permission_banner_v1.1"],
            "extended_context": false,
            "version": PERPLEXITY_API_VERSION,
            "rum_session_id": random_uuid_like()
        }
    }))
}

fn render_conversation(request: &CanonicalRequest) -> String {
    let mut lines = Vec::new();
    if !request.system.is_empty() {
        lines.push(format!("System:\n{}", request.system.join("\n\n")));
    }
    for message in &request.messages {
        let role = if message.role == "assistant" {
            "Assistant"
        } else {
            "User"
        };
        lines.push(format!("{role}:\n{}", message.content));
    }
    lines.join("\n\n")
}

fn validate_response_content_type(
    kind: WebSessionKind,
    value: Option<&HeaderValue>,
) -> Result<(), ProxyError> {
    let value = value
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let accepted = match kind {
        WebSessionKind::Grok => value.contains("json") || value.starts_with("text/plain"),
        WebSessionKind::Perplexity => value.starts_with("text/event-stream"),
    };
    if accepted {
        Ok(())
    } else {
        Err(ProxyError::bad_gateway(
            "Web Session upstream returned an unexpected response content type",
        ))
    }
}

async fn read_response_body_strict(
    mut response: reqwest::Response,
    limit: usize,
    first_byte_timeout: Duration,
    idle_timeout: Duration,
    total_timeout: Duration,
) -> Result<Bytes, ProxyError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(ProxyError::bad_gateway(format!(
            "Web Session response exceeds the {limit} byte profile limit"
        )));
    }
    let deadline = tokio::time::Instant::now() + total_timeout;
    let mut first = true;
    let mut output = BytesMut::with_capacity(limit.min(16 * 1024));
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(ProxyError {
                status: StatusCode::GATEWAY_TIMEOUT,
                message: "Web Session response exceeded the total timeout".to_string(),
            });
        }
        let phase = if first {
            first_byte_timeout
        } else {
            idle_timeout
        };
        let remaining = deadline.saturating_duration_since(now);
        let wait = phase.min(remaining);
        let chunk = tokio::time::timeout(wait, response.chunk())
            .await
            .map_err(|_| ProxyError {
                status: StatusCode::GATEWAY_TIMEOUT,
                message: if first {
                    "Web Session response timed out before the first body frame".to_string()
                } else {
                    "Web Session response exceeded the inter-frame idle timeout".to_string()
                },
            })?
            .map_err(|error| network_error("read", error))?;
        let Some(chunk) = chunk else {
            break;
        };
        first = false;
        if chunk.len() > limit.saturating_sub(output.len()) {
            return Err(ProxyError::bad_gateway(format!(
                "Web Session response exceeds the {limit} byte profile limit"
            )));
        }
        output.extend_from_slice(&chunk);
    }
    if first {
        return Err(ProxyError::bad_gateway(
            "Web Session upstream returned an empty response body",
        ));
    }
    Ok(output.freeze())
}

fn network_error(operation: &str, error: reqwest::Error) -> ProxyError {
    let status = if error.is_timeout() {
        StatusCode::GATEWAY_TIMEOUT
    } else {
        StatusCode::BAD_GATEWAY
    };
    ProxyError {
        status,
        message: format!("Web Session upstream {operation} failed"),
    }
}

fn parse_grok_ndjson(body: &[u8]) -> Result<ParsedUpstreamCompletion, ProxyError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| ProxyError::bad_gateway("Grok Web NDJSON is not valid UTF-8"))?;
    let mut terminal: Option<String> = None;
    let mut session_id = None;
    for raw_line in text.split('\n') {
        let line = raw_line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_WEB_SESSION_FRAME_BYTES {
            return Err(ProxyError::bad_gateway(
                "Grok Web NDJSON line exceeds the frame limit",
            ));
        }
        if terminal.is_some() {
            return Err(ProxyError::bad_gateway(
                "Grok Web NDJSON contains data after the terminal modelResponse",
            ));
        }
        let event: Value = serde_json::from_str(line)
            .map_err(|_| ProxyError::bad_gateway("Grok Web NDJSON contains malformed JSON"))?;
        if event.get("error").is_some_and(|value| !value.is_null()) {
            return Err(ProxyError::bad_gateway(
                "Grok Web NDJSON reported an upstream error",
            ));
        }
        let response = event.pointer("/result/response");
        if let Some(id) = response
            .and_then(|value| value.get("responseId"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            session_id = Some(id.to_string());
        }
        if let Some(model_response) = response.and_then(|value| value.get("modelResponse")) {
            let object = model_response.as_object().ok_or_else(|| {
                ProxyError::bad_gateway("Grok Web terminal modelResponse is not an object")
            })?;
            let message = object
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    ProxyError::bad_gateway("Grok Web terminal modelResponse has no text message")
                })?;
            terminal = Some(message);
        }
    }
    let text = terminal.ok_or_else(|| {
        ProxyError::bad_gateway("Grok Web NDJSON ended without one terminal modelResponse")
    })?;
    Ok(ParsedUpstreamCompletion { text, session_id })
}

#[derive(Default)]
struct PerplexityAccumulator {
    candidates: BTreeMap<String, String>,
    chunks: BTreeMap<String, Vec<String>>,
    fallback_text: Option<String>,
    session_id: Option<String>,
}

impl PerplexityAccumulator {
    fn observe(&mut self, event: &Value) -> Result<(), ProxyError> {
        if event
            .get("error_code")
            .or_else(|| event.get("error_message"))
            .is_some_and(|value| !value.is_null())
        {
            return Err(ProxyError::bad_gateway(
                "Perplexity Web SSE reported an upstream error",
            ));
        }
        if event.get("status").and_then(Value::as_str) == Some("FAILED") {
            return Err(ProxyError::bad_gateway(
                "Perplexity Web SSE reported FAILED status",
            ));
        }
        if let Some(id) = event
            .get("backend_uuid")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.session_id = Some(id.to_string());
        }
        if let Some(blocks) = event.get("blocks").and_then(Value::as_array) {
            for (index, block) in blocks.iter().enumerate() {
                self.observe_block(block, index)?;
            }
        }
        if let Some(text) = event.get("text").and_then(Value::as_str) {
            if let Some(answer) = extract_perplexity_final_text(text) {
                self.fallback_text = Some(answer);
            }
        }
        Ok(())
    }

    fn observe_block(&mut self, block: &Value, index: usize) -> Result<(), ProxyError> {
        let usage = block
            .get("intended_usage")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let key = format!("{usage}:{index}");
        if let Some(markdown) = block.get("markdown_block") {
            if let Some(answer) = materialized_markdown(markdown) {
                self.candidates.insert(key.clone(), answer);
            }
        }
        if let Some(diff) = block.get("diff_block") {
            let field = diff
                .get("field")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let patches = diff
                .get("patches")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if field == "markdown_block" || field.is_empty() {
                apply_markdown_patches(self.chunks.entry(key.clone()).or_default(), &patches)?;
                let joined = self
                    .chunks
                    .get(&key)
                    .map(|chunks| chunks.join(""))
                    .unwrap_or_default();
                if !joined.is_empty() {
                    self.candidates.insert(key.clone(), joined);
                }
            }
            if field == "workflow_block" {
                for patch in &patches {
                    if let Some(answer) = extract_workflow_answer(patch.get("value")) {
                        self.candidates.insert(format!("workflow:{index}"), answer);
                    }
                }
            }
        }
        if let Some(answer) = extract_workflow_answer(block.get("workflow_block")) {
            self.candidates.insert(format!("workflow:{index}"), answer);
        }
        Ok(())
    }

    fn finish(self) -> Result<ParsedUpstreamCompletion, ProxyError> {
        let text = self
            .candidates
            .into_values()
            .chain(self.fallback_text)
            .filter(|value| !value.trim().is_empty())
            .max_by_key(|value| value.len())
            .ok_or_else(|| {
                ProxyError::bad_gateway(
                    "Perplexity Web completed without a non-empty answer text block",
                )
            })?;
        Ok(ParsedUpstreamCompletion {
            text,
            session_id: self.session_id,
        })
    }
}

fn parse_perplexity_sse(body: &[u8]) -> Result<ParsedUpstreamCompletion, ProxyError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| ProxyError::bad_gateway("Perplexity Web SSE is not valid UTF-8"))?;
    let mut accumulator = PerplexityAccumulator::default();
    let mut data_lines = Vec::<String>::new();
    let mut data_bytes = 0usize;
    let mut completed = false;
    let mut end_of_stream = false;

    let flush_data = |data_lines: &mut Vec<String>,
                      data_bytes: &mut usize,
                      accumulator: &mut PerplexityAccumulator,
                      completed: &mut bool|
     -> Result<(), ProxyError> {
        if data_lines.is_empty() {
            return Ok(());
        }
        if *completed {
            return Err(ProxyError::bad_gateway(
                "Perplexity Web SSE contains data after COMPLETED",
            ));
        }
        let payload = data_lines.join("\n");
        data_lines.clear();
        *data_bytes = 0;
        if payload.trim() == "[DONE]" {
            return Err(ProxyError::bad_gateway(
                "Perplexity Web SSE used [DONE] instead of end_of_stream",
            ));
        }
        let event: Value = serde_json::from_str(&payload).map_err(|_| {
            ProxyError::bad_gateway("Perplexity Web SSE data contains malformed JSON")
        })?;
        accumulator.observe(&event)?;
        if event.get("status").and_then(Value::as_str) == Some("COMPLETED") {
            *completed = true;
        }
        Ok(())
    };

    for raw_line in text.split_inclusive('\n') {
        let line = raw_line
            .strip_suffix('\n')
            .unwrap_or(raw_line)
            .strip_suffix('\r')
            .unwrap_or_else(|| raw_line.strip_suffix('\n').unwrap_or(raw_line));
        if end_of_stream {
            if !line.trim().is_empty() {
                return Err(ProxyError::bad_gateway(
                    "Perplexity Web SSE contains data after end_of_stream",
                ));
            }
            continue;
        }
        if line.is_empty() {
            flush_data(
                &mut data_lines,
                &mut data_bytes,
                &mut accumulator,
                &mut completed,
            )?;
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            data_bytes = data_bytes.saturating_add(data.len());
            if data_bytes > MAX_WEB_SESSION_FRAME_BYTES {
                return Err(ProxyError::bad_gateway(
                    "Perplexity Web SSE frame exceeds the frame limit",
                ));
            }
            data_lines.push(data.to_string());
            continue;
        }
        if let Some(event) = line.strip_prefix("event:") {
            let event = event.trim();
            if event == "end_of_stream" {
                flush_data(
                    &mut data_lines,
                    &mut data_bytes,
                    &mut accumulator,
                    &mut completed,
                )?;
                if !completed {
                    return Err(ProxyError::bad_gateway(
                        "Perplexity Web SSE ended before COMPLETED",
                    ));
                }
                end_of_stream = true;
            } else if completed {
                return Err(ProxyError::bad_gateway(
                    "Perplexity Web SSE did not place end_of_stream immediately after COMPLETED",
                ));
            }
            continue;
        }
        if line.starts_with(':') {
            if completed {
                return Err(ProxyError::bad_gateway(
                    "Perplexity Web SSE contains a comment after COMPLETED",
                ));
            }
            continue;
        }
        return Err(ProxyError::bad_gateway(
            "Perplexity Web SSE contains an unsupported field",
        ));
    }
    if !text.ends_with('\n') {
        flush_data(
            &mut data_lines,
            &mut data_bytes,
            &mut accumulator,
            &mut completed,
        )?;
    }
    if !completed || !end_of_stream {
        return Err(ProxyError::bad_gateway(
            "Perplexity Web SSE ended without COMPLETED followed by end_of_stream",
        ));
    }
    accumulator.finish()
}

fn materialized_markdown(markdown: &Value) -> Option<String> {
    markdown
        .get("chunks")
        .and_then(Value::as_array)
        .map(|chunks| chunks.iter().filter_map(Value::as_str).collect::<String>())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            markdown
                .get("answer")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.is_empty())
        })
}

fn apply_markdown_patches(chunks: &mut Vec<String>, patches: &[Value]) -> Result<(), ProxyError> {
    for patch in patches {
        let path = patch
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if path.is_empty() {
            let value = patch.get("value").unwrap_or(&Value::Null);
            if let Some(values) = value.get("chunks").and_then(Value::as_array) {
                chunks.clear();
                for value in values {
                    chunks.push(value.as_str().unwrap_or_default().to_string());
                }
            } else if let Some(answer) = value.get("answer").and_then(Value::as_str) {
                *chunks = vec![answer.to_string()];
            }
            continue;
        }
        let Some(index) = path
            .strip_prefix("/chunks/")
            .and_then(|value| value.parse::<usize>().ok())
        else {
            continue;
        };
        if index > 4096 {
            return Err(ProxyError::bad_gateway(
                "Perplexity Web markdown patch index exceeds the bound",
            ));
        }
        let value = patch.get("value").and_then(Value::as_str).ok_or_else(|| {
            ProxyError::bad_gateway("Perplexity Web markdown patch has a non-text value")
        })?;
        if chunks.len() <= index {
            chunks.resize(index + 1, String::new());
        }
        chunks[index] = value.to_string();
    }
    Ok(())
}

fn extract_workflow_answer(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| extract_workflow_answer(Some(item)))
            .max_by_key(|answer| answer.len()),
        Value::Object(object) => {
            let variant = object
                .get("variant")
                .or_else(|| value.pointer("/payload/text_payload/variant"))
                .and_then(Value::as_str);
            if variant == Some("answer") {
                let payload = value
                    .pointer("/payload/text_payload")
                    .or_else(|| object.get("text_payload"))
                    .unwrap_or(value);
                if let Some(chunks) = payload.get("chunks").and_then(Value::as_array) {
                    let answer = chunks.iter().filter_map(Value::as_str).collect::<String>();
                    if !answer.is_empty() {
                        return Some(answer);
                    }
                }
                if let Some(text) = payload.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        return Some(text.to_string());
                    }
                }
            }
            object
                .values()
                .filter_map(|item| extract_workflow_answer(Some(item)))
                .max_by_key(|answer| answer.len())
        }
        _ => None,
    }
}

fn extract_perplexity_final_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !matches!(trimmed.as_bytes().first(), Some(b'{') | Some(b'[')) {
        return Some(trimmed.to_string());
    }
    let value: Value = serde_json::from_str(trimmed).ok()?;
    let values = value.as_array().cloned().unwrap_or_else(|| vec![value]);
    for step in values {
        let object = step.as_object()?;
        if object
            .get("step_type")
            .or_else(|| object.get("stepType"))
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "FINAL")
        {
            continue;
        }
        let raw = step
            .pointer("/content/answer")
            .or_else(|| object.get("answer"))
            .and_then(Value::as_str)?;
        if let Ok(inner) = serde_json::from_str::<Value>(raw) {
            if let Some(answer) = inner.get("answer").and_then(Value::as_str) {
                if !answer.trim().is_empty() {
                    return Some(answer.to_string());
                }
            }
            if let Some(chunks) = inner.get("chunks").and_then(Value::as_array) {
                let answer = chunks.iter().filter_map(Value::as_str).collect::<String>();
                if !answer.trim().is_empty() {
                    return Some(answer);
                }
            }
        }
        if !raw.trim().is_empty() {
            return Some(raw.to_string());
        }
    }
    None
}

fn render_downstream(
    route: ProxyRoute,
    stream: bool,
    model: &str,
    text: &str,
    input_tokens: u64,
    output_tokens: u64,
) -> Result<(Bytes, &'static str), ProxyError> {
    let id = random_id(match route {
        ProxyRoute::ClaudeMessages => "msg_web_",
        ProxyRoute::CodexChatCompletions => "chatcmpl_web_",
        ProxyRoute::CodexResponses => "resp_web_",
        _ => "web_",
    });
    let created = crate::infra::time::now_ms() as u64 / 1000;
    let encoded = match (route, stream) {
        (ProxyRoute::ClaudeMessages, false) => serde_json::to_vec(&json!({
            "id": id,
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": [{"type":"text","text":text}],
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {"input_tokens":input_tokens,"output_tokens":output_tokens}
        }))?,
        (ProxyRoute::ClaudeMessages, true) => {
            let events = [
                (
                    "message_start",
                    json!({"type":"message_start","message":{"id":id,"type":"message","role":"assistant","model":model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":input_tokens,"output_tokens":0}}}),
                ),
                (
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
                ),
                (
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":0}),
                ),
                (
                    "message_delta",
                    json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":output_tokens}}),
                ),
                ("message_stop", json!({"type":"message_stop"})),
            ];
            events
                .into_iter()
                .map(|(event, value)| format!("event: {event}\ndata: {value}\n\n"))
                .collect::<String>()
                .into_bytes()
        }
        (ProxyRoute::CodexChatCompletions, false) => serde_json::to_vec(&json!({
            "id": id,
            "object": "chat.completion",
            "created": created,
            "model": model,
            "choices": [{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop","logprobs":null}],
            "usage": {"prompt_tokens":input_tokens,"completion_tokens":output_tokens,"total_tokens":input_tokens.saturating_add(output_tokens)}
        }))?,
        (ProxyRoute::CodexChatCompletions, true) => {
            let chunks = [
                json!({"id":id,"object":"chat.completion.chunk","created":created,"model":model,"choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null,"logprobs":null}]}),
                json!({"id":id,"object":"chat.completion.chunk","created":created,"model":model,"choices":[{"index":0,"delta":{"content":text},"finish_reason":null,"logprobs":null}]}),
                json!({"id":id,"object":"chat.completion.chunk","created":created,"model":model,"choices":[{"index":0,"delta":{},"finish_reason":"stop","logprobs":null}],"usage":{"prompt_tokens":input_tokens,"completion_tokens":output_tokens,"total_tokens":input_tokens.saturating_add(output_tokens)}}),
            ];
            let mut output = chunks
                .iter()
                .map(|value| format!("data: {value}\n\n"))
                .collect::<String>();
            output.push_str("data: [DONE]\n\n");
            output.into_bytes()
        }
        (ProxyRoute::CodexResponses, false) => {
            let response =
                completed_response(&id, created, model, text, input_tokens, output_tokens);
            serde_json::to_vec(&response)?
        }
        (ProxyRoute::CodexResponses, true) => {
            let item_id = random_id("msg_web_");
            let in_progress = json!({
                "id": id,
                "object": "response",
                "created_at": created,
                "status": "in_progress",
                "model": model,
                "output": []
            });
            let completed =
                completed_response(&id, created, model, text, input_tokens, output_tokens);
            let events = [
                (
                    "response.created",
                    json!({"type":"response.created","response":in_progress}),
                ),
                (
                    "response.output_item.added",
                    json!({"type":"response.output_item.added","output_index":0,"item":{"id":item_id,"type":"message","status":"in_progress","role":"assistant","content":[]}}),
                ),
                (
                    "response.content_part.added",
                    json!({"type":"response.content_part.added","item_id":item_id,"output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
                ),
                (
                    "response.output_text.delta",
                    json!({"type":"response.output_text.delta","item_id":item_id,"output_index":0,"content_index":0,"delta":text}),
                ),
                (
                    "response.output_text.done",
                    json!({"type":"response.output_text.done","item_id":item_id,"output_index":0,"content_index":0,"text":text}),
                ),
                (
                    "response.content_part.done",
                    json!({"type":"response.content_part.done","item_id":item_id,"output_index":0,"content_index":0,"part":{"type":"output_text","text":text,"annotations":[]}}),
                ),
                (
                    "response.output_item.done",
                    json!({"type":"response.output_item.done","output_index":0,"item":{"id":item_id,"type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":text,"annotations":[]}]}}),
                ),
                (
                    "response.completed",
                    json!({"type":"response.completed","response":completed}),
                ),
            ];
            let mut output = events
                .into_iter()
                .map(|(event, value)| format!("event: {event}\ndata: {value}\n\n"))
                .collect::<String>();
            output.push_str("data: [DONE]\n\n");
            output.into_bytes()
        }
        _ => {
            return Err(ProxyError::bad_request(
                "Web Session response route is unsupported",
            ))
        }
    };
    let content_type = if stream {
        "text/event-stream"
    } else {
        "application/json"
    };
    Ok((Bytes::from(encoded), content_type))
}

fn completed_response(
    id: &str,
    created: u64,
    model: &str,
    text: &str,
    input_tokens: u64,
    output_tokens: u64,
) -> Value {
    json!({
        "id": id,
        "object": "response",
        "created_at": created,
        "status": "completed",
        "model": model,
        "output": [{
            "id": random_id("msg_web_"),
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{"type":"output_text","text":text,"annotations":[]}]
        }],
        "output_text": text,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "total_tokens": input_tokens.saturating_add(output_tokens)
        }
    })
}

fn estimate_tokens(text: &str) -> u64 {
    if text.is_empty() {
        0
    } else {
        (text.chars().count() as u64).saturating_add(3) / 4
    }
}

fn random_id(prefix: &str) -> String {
    let mut bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}

fn random_uuid_like() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

impl From<serde_json::Error> for ProxyError {
    fn from(error: serde_json::Error) -> Self {
        ProxyError::bad_gateway(format!("encode Web Session downstream response: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_ndjson_requires_one_terminal_and_rejects_malformed_or_trailing_data() {
        let valid = br#"{"result":{"response":{"token":"hel","responseId":"grok-1"}}}
{"result":{"response":{"modelResponse":{"message":"hello"},"responseId":"grok-1"}}}
"#;
        let parsed = parse_grok_ndjson(valid).unwrap();
        assert_eq!(parsed.text, "hello");
        assert_eq!(parsed.session_id.as_deref(), Some("grok-1"));

        for invalid in [
            br#"{"result":{"response":{"token":"unterminated"}}}
"#
            .as_slice(),
            br#"not-json
"#
            .as_slice(),
            br#"{"result":{"response":{"modelResponse":{"message":"one"}}}}
{"result":{"response":{"token":"after"}}}
"#
            .as_slice(),
            br#"{"result":{"response":{"modelResponse":{"message":"one"}}}}
{"result":{"response":{"modelResponse":{"message":"two"}}}}
"#
            .as_slice(),
        ] {
            assert!(parse_grok_ndjson(invalid).is_err());
        }
    }

    #[test]
    fn perplexity_sse_requires_completed_then_end_of_stream_and_strict_json() {
        let valid = br#"data: {"backend_uuid":"pplx-1","blocks":[{"intended_usage":"ask_text","markdown_block":{"chunks":["hel"]}}]}

data: {"status":"COMPLETED","backend_uuid":"pplx-1","blocks":[{"intended_usage":"ask_text","markdown_block":{"chunks":["hello"]}}]}

event: end_of_stream

"#;
        let parsed = parse_perplexity_sse(valid).unwrap();
        assert_eq!(parsed.text, "hello");
        assert_eq!(parsed.session_id.as_deref(), Some("pplx-1"));

        let invalid = [
            br#"data: {"status":"COMPLETED","blocks":[{"markdown_block":{"chunks":["x"]}}]}

"#
            .as_slice(),
            br#"event: end_of_stream

"#
            .as_slice(),
            br#"data: not-json

"#
            .as_slice(),
            br#"data: {"status":"COMPLETED","blocks":[{"markdown_block":{"chunks":["x"]}}]}

data: {"status":"PENDING"}

event: end_of_stream

"#
            .as_slice(),
            br#"data: {"status":"COMPLETED","blocks":[{"markdown_block":{"chunks":["x"]}}]}

event: end_of_stream

data: {"status":"COMPLETED"}

"#
            .as_slice(),
        ];
        for body in invalid {
            assert!(parse_perplexity_sse(body).is_err());
        }
    }

    #[test]
    fn parsers_are_independent_of_transport_chunk_boundaries_after_bounded_collection() {
        let grok = br#"{"result":{"response":{"modelResponse":{"message":"split-safe"}}}}
"#;
        let pplx = br#"data: {"status":"COMPLETED","blocks":[{"intended_usage":"ask_text","markdown_block":{"chunks":["split-safe"]}}]}

event: end_of_stream

"#;
        for body in [grok.as_slice(), pplx.as_slice()] {
            for split in 0..=body.len() {
                let mut joined = Vec::new();
                joined.extend_from_slice(&body[..split]);
                joined.extend_from_slice(&body[split..]);
                if std::ptr::eq(body, grok.as_slice()) {
                    assert_eq!(parse_grok_ndjson(&joined).unwrap().text, "split-safe");
                } else {
                    assert_eq!(parse_perplexity_sse(&joined).unwrap().text, "split-safe");
                }
            }
        }
    }

    #[test]
    fn downstream_parsers_reject_tools_images_and_non_text_responses_items() {
        let tools = br#"{"model":"fast","messages":[{"role":"user","content":"ping"}],"tools":[{"type":"function","function":{"name":"x"}}]}"#;
        assert!(parse_downstream_request(ProxyRoute::CodexChatCompletions, tools).is_err());
        let image = br#"{"model":"fast","messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"https://example.test/x.png"}}]}]}"#;
        assert!(parse_downstream_request(ProxyRoute::CodexChatCompletions, image).is_err());
        let function = br#"{"model":"pplx-auto","input":[{"type":"function_call","name":"x"}]}"#;
        assert!(parse_downstream_request(ProxyRoute::CodexResponses, function).is_err());
    }

    #[test]
    fn perplexity_diff_and_final_text_shapes_are_reconstructed_without_citations_rewrite() {
        let body = br#"data: {"blocks":[{"intended_usage":"ask_text","diff_block":{"field":"markdown_block","patches":[{"op":"replace","path":"","value":{"chunks":["hello "]}}]}}]}

data: {"blocks":[{"intended_usage":"ask_text","diff_block":{"field":"markdown_block","patches":[{"op":"add","path":"/chunks/1","value":"world [1]"}]}}]}

data: {"status":"COMPLETED","backend_uuid":"pplx-diff"}

event: end_of_stream

"#;
        let parsed = parse_perplexity_sse(body).unwrap();
        assert_eq!(parsed.text, "hello world [1]");
    }
}
