//! Kiro and Amazon Q Provider lifecycle facade.
//!
//! The AWS EventStream codec and protocol conversion remain in `proxy::kiro`.
//! This module owns the Provider-specific boundary around canonical request
//! preparation, exact managed-account identity, catalog/model preparation,
//! local CountTokens, keepalive/error classification, and subscription
//! throttles. Share/lease ownership, usage, terminal handling, and the shared
//! attempt budget deliberately remain in the forwarder.

use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use bytes::Bytes;
use serde_json::{json, Value};

use crate::domain::accounts::store::AccountStore;
use crate::domain::providers::model::ProviderType;
use crate::domain::providers::runtime::RuntimeAuthRef;
use crate::domain::providers::store::StoredProvider;
use crate::domain::usage::store::UsageLogContext;
use crate::state::ServerState;

use super::super::adapters::{self, AdapterRequest, GenericForwardingAdapter};
use super::super::kiro as driver;
use super::super::provider_ops::ProviderExecution;
use super::super::router::ProxyRoute;
#[cfg(test)]
use super::super::setting;
use super::super::{bounded_upstream_rate_limit_until, ProxyError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProductBoundary {
    pub(crate) provider_type: ProviderType,
    pub(crate) label: &'static str,
}

pub(crate) fn product_boundary(stored: &StoredProvider) -> Result<ProductBoundary, ProxyError> {
    match stored.provider_type {
        ProviderType::KiroOAuth => Ok(ProductBoundary {
            provider_type: ProviderType::KiroOAuth,
            label: "Kiro",
        }),
        ProviderType::AmazonQOAuth => Ok(ProductBoundary {
            provider_type: ProviderType::AmazonQOAuth,
            label: "Amazon Q",
        }),
        _ => Err(ProxyError::bad_request(
            "CodeWhisperer driver requires kiro_oauth or amazon_q_oauth Provider",
        )),
    }
}

pub(crate) struct CanonicalRequest {
    pub(crate) adapter: GenericForwardingAdapter,
    pub(crate) runtime_request: AdapterRequest,
    pub(crate) request_body: Value,
    pub(crate) routed_model: String,
    pub(crate) response_model: String,
    pub(crate) actual_model: String,
    pub(crate) stream_requested: bool,
    pub(crate) claude_code_tools: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_canonical_request(
    execution: &ProviderExecution,
    stored: &StoredProvider,
    route: ProxyRoute,
    gemini_path: Option<&str>,
    headers: &HeaderMap,
    body: Bytes,
    request_context: &mut UsageLogContext,
) -> Result<CanonicalRequest, ProxyError> {
    let product = product_boundary(stored)?;
    let adapter = adapters::adapter_for(route.app(), stored.provider_type);
    let metadata = adapters::CopilotRequestMetadata {
        has_anthropic_beta: headers.contains_key("anthropic-beta"),
        session_id: request_context.session_id.clone(),
    };
    let mut runtime_request = adapter.transform_request_for_route_with_metadata(
        body,
        stored,
        route,
        gemini_path,
        &metadata,
    )?;
    execution.enforce_model_policy(&mut runtime_request)?;
    let request_body: Value = serde_json::from_slice(&runtime_request.body).map_err(|error| {
        ProxyError::bad_request(format!(
            "invalid {} canonical request body: {error}",
            product.label
        ))
    })?;
    if request_context.session_id.is_none() {
        request_context.session_id = Some(driver::request_scoped_session_id());
    }
    let routed_model = request_body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProxyError::bad_request("missing model"))?
        .to_string();
    let response_model = runtime_request
        .requested_model
        .clone()
        .unwrap_or_else(|| routed_model.clone());
    let actual_model = driver::resolve_model(&routed_model).ok_or_else(|| {
        ProxyError::bad_request(format!(
            "{} model identifier is invalid: {routed_model}",
            product.label
        ))
    })?;
    let claude_code_tools = route == ProxyRoute::ClaudeMessages
        && (headers
            .get("x-app")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("cli"))
            || headers.contains_key("x-claude-code-session-id")
            || headers
                .get("anthropic-beta")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("claude-code")));
    let stream_requested = runtime_request.stream_requested;
    Ok(CanonicalRequest {
        adapter,
        runtime_request,
        request_body,
        routed_model,
        response_model,
        actual_model,
        stream_requested,
        claude_code_tools,
    })
}

pub(crate) fn local_count_tokens_body(
    route: ProxyRoute,
    request_body: &Value,
) -> Result<Option<Bytes>, ProxyError> {
    if route != ProxyRoute::ClaudeCountTokens {
        return Ok(None);
    }
    let input_tokens = driver::count_input_tokens(request_body)?;
    serde_json::to_vec(&json!({"input_tokens": input_tokens}))
        .map(Bytes::from)
        .map(Some)
        .map_err(ProxyError::bad_gateway)
}

pub(crate) struct BoundPreparedRequest {
    pub(crate) account_id: String,
    pub(crate) replay_allowed: bool,
    pub(crate) prepared: driver::KiroPreparedRequest,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare_bound_request(
    state: &ServerState,
    execution: &ProviderExecution,
    stored: &StoredProvider,
    accounts: &AccountStore,
    route: ProxyRoute,
    request_context: &UsageLogContext,
    request_body: &Value,
    actual_model: &str,
    ide_version: &str,
    claude_code_tools: bool,
) -> Result<BoundPreparedRequest, ProxyError> {
    let product = product_boundary(stored)?;
    execution.materialize_auth(accounts)?;
    let account_id = execution
        .managed_account_target()
        .filter(|(provider_type, _)| *provider_type == product.provider_type)
        .map(|(_, account_id)| account_id)
        .ok_or_else(|| {
            ProxyError::bad_request(format!(
                "{} managed account binding is required",
                product.provider_type.as_str()
            ))
        })?;
    let expected_auth_identity_generation = match &execution.plan.auth_ref {
        RuntimeAuthRef::ManagedAccount {
            account_id: bound_account_id,
            expected_provider_type,
            auth_identity_generation,
        } if bound_account_id == account_id && *expected_provider_type == product.provider_type => {
            *auth_identity_generation
        }
        _ => {
            return Err(ProxyError::conflict(format!(
                "{} runtime account binding changed before model discovery",
                product.label
            )))
        }
    };
    let account = accounts
        .find_for_provider(product.provider_type, Some(account_id))
        .cloned()
        .ok_or_else(|| {
            ProxyError::not_found(format!(
                "{} managed account not found",
                product.provider_type.as_str()
            ))
        })?;
    let replay_allowed = account
        .refresh_token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty());

    #[cfg(test)]
    let models_endpoint_override = execution
        .plan
        .driver_options
        .get(if product.provider_type == ProviderType::AmazonQOAuth {
            "testAmazonQModelsUrl"
        } else {
            "testKiroModelsUrl"
        })
        .and_then(Value::as_str);
    #[cfg(not(test))]
    let models_endpoint_override: Option<&str> = None;

    #[cfg(test)]
    let catalog_model = if models_endpoint_override.is_none() {
        Some((actual_model.to_string(), None))
    } else if product.provider_type == ProviderType::AmazonQOAuth {
        state
            .amazon_q_catalog_model(
                stored.app,
                &stored.provider.id,
                execution.plan.provider_revision,
                &execution.plan.runtime_fingerprint,
                account_id,
                expected_auth_identity_generation,
                actual_model,
                models_endpoint_override,
                execution.request_timeout(),
            )
            .await
    } else {
        state
            .kiro_catalog_model(
                stored.app,
                &stored.provider.id,
                execution.plan.provider_revision,
                &execution.plan.runtime_fingerprint,
                account_id,
                expected_auth_identity_generation,
                actual_model,
                models_endpoint_override,
                execution.request_timeout(),
            )
            .await
    };
    #[cfg(not(test))]
    let catalog_model = if product.provider_type == ProviderType::AmazonQOAuth {
        state
            .amazon_q_catalog_model(
                stored.app,
                &stored.provider.id,
                execution.plan.provider_revision,
                &execution.plan.runtime_fingerprint,
                account_id,
                expected_auth_identity_generation,
                actual_model,
                models_endpoint_override,
                execution.request_timeout(),
            )
            .await
    } else {
        state
            .kiro_catalog_model(
                stored.app,
                &stored.provider.id,
                execution.plan.provider_revision,
                &execution.plan.runtime_fingerprint,
                account_id,
                expected_auth_identity_generation,
                actual_model,
                models_endpoint_override,
                execution.request_timeout(),
            )
            .await
    };
    let (catalog_model_id, catalog_max_input_tokens) = catalog_model.ok_or_else(|| {
        ProxyError::bad_request(format!(
            "{} model is not available to the bound account: {actual_model}",
            product.label
        ))
    })?;
    let cache_session = request_context
        .session_id
        .as_deref()
        .expect("Kiro request session identity is always resolved");
    let cache_runtime_region = driver::KiroAccountData::from_account(&account)?.api_region;
    let cache_route = format!("{route:?}");
    let cache_namespace = super::super::kiro_prompt_cache::PromptCacheScope {
        app: stored.app.as_str(),
        provider_id: &stored.provider.id,
        provider_revision: execution.plan.provider_revision,
        runtime_fingerprint: &execution.plan.runtime_fingerprint,
        account_id: &account.id,
        auth_identity_generation: account.auth_identity_generation,
        token_refresh_generation: account.token_refresh_generation,
        share_id: request_context.share_id.as_deref().unwrap_or("direct"),
        signed_user: request_context.user_email.as_deref().unwrap_or("anonymous"),
        route: &cache_route,
        runtime_region: &cache_runtime_region,
        session: cache_session,
    }
    .namespace();
    let call_context = driver::KiroCallContext {
        ide_version: ide_version.to_string(),
        claude_code_tools,
        cache_namespace,
        catalog_model_id: Some(catalog_model_id),
        catalog_max_input_tokens,
        session_id: Some(cache_session.to_string()),
    };
    let mut prepared =
        driver::prepare_kiro_request_with_context(&account, request_body, &call_context)?;
    if let Some(base_url) = api_base_override(stored) {
        prepared.url = url_with_base_override(&base_url, &prepared.url)?;
    }
    Ok(BoundPreparedRequest {
        account_id: account_id.to_string(),
        replay_allowed,
        prepared,
    })
}

pub(crate) async fn runtime_client_version(
    state: &ServerState,
    provider_type: ProviderType,
) -> String {
    if provider_type == ProviderType::KiroOAuth {
        state.kiro_ide_version().await
    } else {
        "amazon-q-cli".to_string()
    }
}

pub(crate) fn text_keepalive_interval(execution: &ProviderExecution) -> Option<Duration> {
    let configured = execution
        .plan
        .driver_options
        .get("kiroKeepaliveIntervalMs")
        .and_then(Value::as_u64)
        .or_else(|| {
            std::env::var("CC_SWITCH_KIRO_KEEPALIVE_MS")
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
        })
        .unwrap_or(25_000);
    if configured == 0 {
        None
    } else {
        #[cfg(test)]
        let interval = configured;
        #[cfg(not(test))]
        let interval = configured.clamp(5_000, 60_000);
        Some(Duration::from_millis(interval))
    }
}

pub(crate) fn downstream_keepalive_frame(route: ProxyRoute) -> Bytes {
    match route {
        ProxyRoute::ClaudeMessages => {
            Bytes::from_static(b"event: ping\ndata: {\"type\":\"ping\"}\n\n")
        }
        ProxyRoute::CodexChatCompletions
        | ProxyRoute::CodexResponses
        | ProxyRoute::CodexResponsesCompact => Bytes::from_static(b": keepalive\n\n"),
        ProxyRoute::ClaudeCountTokens | ProxyRoute::Gemini => Bytes::new(),
    }
}

pub(crate) fn stream_failure_status(error: &std::io::Error) -> StatusCode {
    if error
        .to_string()
        .starts_with("[KIRO_EVENT_STREAM_TIMEOUT] ")
    {
        StatusCode::GATEWAY_TIMEOUT
    } else {
        StatusCode::BAD_GATEWAY
    }
}

pub(crate) fn is_client_validation_error(body: &[u8]) -> bool {
    driver::is_client_validation_error(body)
}

pub(crate) fn enforce_disabled_compaction_contract(
    provider_type: ProviderType,
    route: ProxyRoute,
    body: &Bytes,
) -> Result<(), ProxyError> {
    driver::enforce_disabled_compaction_contract(provider_type, route, body)
}

pub(crate) async fn handle_subscription_throttle(
    state: &ServerState,
    execution: &ProviderExecution,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> bool {
    if !matches!(
        execution.stored.provider_type,
        ProviderType::KiroOAuth | ProviderType::AmazonQOAuth
    ) {
        return false;
    }
    let Some(class) = driver::classify_throttle(status, body) else {
        return false;
    };
    if class == driver::KiroThrottleClass::OrdinaryOverload {
        return false;
    }
    let Some((provider_type, account_id, auth_identity_generation)) =
        execution.managed_account_identity_target()
    else {
        return true;
    };
    let now = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    let fallback = now.saturating_add(
        i64::try_from(driver::throttle_default_cooldown(class).as_millis()).unwrap_or(i64::MAX),
    );
    let until = super::super::grok::retry_after_until_ms(headers, now).unwrap_or(fallback);
    let until = bounded_upstream_rate_limit_until(now, until);
    state
        .mark_account_rate_limited_until_if_current(
            account_id,
            provider_type,
            auth_identity_generation,
            until,
            Some(format!(
                "CodeWhisperer subscription throttle classified as {class:?}"
            )),
        )
        .await;
    true
}

fn api_base_override(stored: &StoredProvider) -> Option<String> {
    #[cfg(test)]
    {
        if stored.provider_type == ProviderType::AmazonQOAuth {
            setting(
                &stored.provider,
                &["AMAZON_Q_API_BASE_URL", "AMAZON_Q_BASE_URL"],
            )
        } else {
            setting(
                &stored.provider,
                &[
                    "KIRO_API_BASE_URL",
                    "KIRO_BASE_URL",
                    "CODEWHISPERER_BASE_URL",
                ],
            )
        }
    }
    #[cfg(not(test))]
    {
        let _ = stored;
        None
    }
}

pub(crate) fn url_with_base_override(
    base_url: &str,
    prepared_url: &str,
) -> Result<String, ProxyError> {
    let prepared = url::Url::parse(prepared_url)
        .map_err(|error| ProxyError::bad_gateway(format!("invalid prepared Kiro URL: {error}")))?;
    let mut base = url::Url::parse(base_url)
        .map_err(|error| ProxyError::bad_request(format!("invalid Kiro API base URL: {error}")))?;
    let base_path = base.path().trim_end_matches('/');
    let prepared_path = prepared.path().trim_start_matches('/');
    let path = if base_path.is_empty() {
        format!("/{prepared_path}")
    } else if prepared_path.is_empty() {
        format!("{base_path}/")
    } else {
        format!("{base_path}/{prepared_path}")
    };
    base.set_path(&path);
    base.set_query(prepared.query());
    base.set_fragment(None);
    Ok(base.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::domain::providers::model::{AppKind, Provider, ProviderMeta};
    use crate::domain::providers::store::StoredProvider;

    use super::*;

    fn stored(provider_type: ProviderType) -> StoredProvider {
        StoredProvider {
            app: AppKind::Claude,
            provider: Provider {
                id: format!("{}-provider", provider_type.as_str()),
                name: "CodeWhisperer fixture".to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(ProviderMeta {
                    provider_type: Some(provider_type.as_str().to_string()),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
            resource: Default::default(),
        }
    }

    #[test]
    fn provider_facade_keeps_kiro_and_amazon_q_product_labels_distinct() {
        let kiro = product_boundary(&stored(ProviderType::KiroOAuth)).unwrap();
        let amazon_q = product_boundary(&stored(ProviderType::AmazonQOAuth)).unwrap();
        assert_eq!(kiro.provider_type, ProviderType::KiroOAuth);
        assert_eq!(kiro.label, "Kiro");
        assert_eq!(amazon_q.provider_type, ProviderType::AmazonQOAuth);
        assert_eq!(amazon_q.label, "Amazon Q");

        let error = product_boundary(&stored(ProviderType::GrokOAuth)).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("kiro_oauth or amazon_q_oauth"));
    }

    #[test]
    fn count_tokens_and_keepalive_decisions_are_surface_scoped() {
        let body = json!({
            "model": "claude-sonnet-4-5",
            "messages": [{"role": "user", "content": "hello"}]
        });
        let count = local_count_tokens_body(ProxyRoute::ClaudeCountTokens, &body)
            .unwrap()
            .expect("CountTokens is local");
        let count: Value = serde_json::from_slice(&count).unwrap();
        assert!(count["input_tokens"]
            .as_i64()
            .is_some_and(|value| value > 0));
        assert!(local_count_tokens_body(ProxyRoute::ClaudeMessages, &body)
            .unwrap()
            .is_none());

        assert_eq!(
            downstream_keepalive_frame(ProxyRoute::ClaudeMessages),
            Bytes::from_static(b"event: ping\ndata: {\"type\":\"ping\"}\n\n")
        );
        assert_eq!(
            downstream_keepalive_frame(ProxyRoute::CodexResponses),
            Bytes::from_static(b": keepalive\n\n")
        );
        assert!(downstream_keepalive_frame(ProxyRoute::ClaudeCountTokens).is_empty());
    }

    #[test]
    fn compact_gate_and_stream_error_mapping_remain_fail_closed() {
        let body =
            Bytes::from_static(br#"{"model":"claude-sonnet-4-5","input":[],"compact":true}"#);
        let error = enforce_disabled_compaction_contract(
            ProviderType::KiroOAuth,
            ProxyRoute::CodexResponsesCompact,
            &body,
        )
        .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("disabled pending"));

        assert_eq!(
            stream_failure_status(&std::io::Error::other(
                "[KIRO_EVENT_STREAM_TIMEOUT] first frame"
            )),
            StatusCode::GATEWAY_TIMEOUT
        );
        assert_eq!(
            stream_failure_status(&std::io::Error::other("invalid CRC")),
            StatusCode::BAD_GATEWAY
        );
    }
}
