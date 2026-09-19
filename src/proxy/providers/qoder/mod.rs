//! Qoder COSY Provider lifecycle facade.
//!
//! The COSY codec and response lifecycle remain in `proxy::qoder`, while the
//! runtime cache and payload primitives remain in `proxy::qoder_runtime`.
//! This module owns the Provider-specific boundary around the exact managed
//! Account binding, downstream conversation identity, live-catalog model
//! selection, request payload preparation, generation fencing, and Qoder
//! subscription throttles. Share/lease ownership, usage, terminal handling,
//! the shared attempt budget, and the decision to replay a pre-commit 401
//! deliberately remain in the forwarder.

use axum::http::StatusCode;
use serde_json::Value;

use crate::domain::providers::model::ProviderType;
use crate::domain::usage::store::UsageLogContext;
use crate::logging::opaque_ref;
use crate::state::{QoderRuntimeError, ServerState};

use super::super::adapters::AdapterRequest;
use super::super::provider_ops::ProviderExecution;
use super::super::qoder as wire;
use super::super::qoder_runtime::{self as runtime, PreparedQoderRuntime};
use super::super::request_memory::{
    retained_json_bytes, RequestMemoryBudget, RequestMemoryComponent, RequestMemoryReservation,
};
use super::super::{bounded_upstream_rate_limit_until, ProxyError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundAccountIdentity {
    pub(crate) account_id: String,
    pub(crate) auth_identity_generation: u64,
}

pub(crate) fn bound_account_identity(
    execution: &ProviderExecution,
) -> Result<BoundAccountIdentity, ProxyError> {
    let Some((provider_type, account_id, auth_identity_generation)) =
        execution.managed_account_identity_target()
    else {
        return Err(ProxyError::bad_request(
            "Qoder COSY Provider must bind one explicit qoder_cosy managed account",
        ));
    };
    if provider_type != ProviderType::QoderCosy {
        return Err(ProxyError::bad_request(
            "Qoder COSY Provider account type does not match its runtime contract",
        ));
    }
    Ok(BoundAccountIdentity {
        account_id: account_id.to_string(),
        auth_identity_generation,
    })
}

pub(crate) async fn prepare_runtime(
    state: &ServerState,
    execution: &ProviderExecution,
    bound: &BoundAccountIdentity,
) -> Result<PreparedQoderRuntime, QoderRuntimeError> {
    state
        .prepare_qoder_runtime(
            execution.stored.app,
            &execution.stored.provider.id,
            execution.plan.provider_revision,
            &execution.plan.runtime_fingerprint,
            &bound.account_id,
            bound.auth_identity_generation,
            execution.request_timeout(),
        )
        .await
}

#[derive(Debug, Clone)]
pub(crate) struct CanonicalRequest {
    pub(crate) share_id: String,
    pub(crate) user_namespace: String,
    pub(crate) downstream_session_id: String,
    pub(crate) requested_model: String,
    pub(crate) body: Value,
}

impl CanonicalRequest {
    pub(crate) fn retained_bytes(&self) -> usize {
        self.share_id
            .capacity()
            .saturating_add(self.user_namespace.capacity())
            .saturating_add(self.downstream_session_id.capacity())
            .saturating_add(self.requested_model.capacity())
            .saturating_add(retained_json_bytes(&self.body))
    }
}

#[derive(Debug)]
pub(crate) enum CanonicalRequestError {
    Binding(ProxyError),
    Request(ProxyError),
}

pub(crate) fn prepare_canonical_request(
    request: &AdapterRequest,
    request_context: &mut UsageLogContext,
) -> Result<CanonicalRequest, CanonicalRequestError> {
    let share_id = request_context
        .share_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CanonicalRequestError::Binding(ProxyError::bad_request(
                "Qoder COSY inference requires a Router Share",
            ))
        })?
        .to_string();
    let user_namespace = opaque_ref(
        "qoder_user",
        request_context
            .user_email
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("anonymous"),
    );
    let downstream_session_id = request_context
        .session_id
        .clone()
        .or_else(|| request_context.request_id.clone())
        .unwrap_or_else(|| {
            opaque_ref(
                "qoder_request",
                std::str::from_utf8(&request.body).unwrap_or("non_utf8_request"),
            )
        });
    request_context.session_id = Some(downstream_session_id.clone());
    let requested_model = request
        .actual_model
        .clone()
        .or_else(|| request.model.clone())
        .or_else(|| model_from_canonical_chat(&request.body))
        .ok_or_else(|| {
            CanonicalRequestError::Request(ProxyError::bad_request(
                "Qoder COSY request is missing a model",
            ))
        })?;
    let body = serde_json::from_slice::<Value>(&request.body).map_err(|error| {
        CanonicalRequestError::Request(ProxyError::bad_request(format!(
            "invalid Qoder Chat request: {error}"
        )))
    })?;
    Ok(CanonicalRequest {
        share_id,
        user_namespace,
        downstream_session_id,
        requested_model,
        body,
    })
}

fn model_from_canonical_chat(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()?
        .get("model")?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedGeneration {
    pub(crate) model_key: String,
    pub(crate) model_source: String,
    pub(crate) payload: runtime::PreparedQoderPayload,
    pub(crate) prepared_memory: Option<RequestMemoryReservation>,
}

impl PreparedGeneration {
    pub(crate) fn retained_bytes(&self) -> usize {
        self.model_key
            .capacity()
            .saturating_add(self.model_source.capacity())
            .saturating_add(self.payload.retained_bytes())
    }
}

pub(crate) fn prepare_generation(
    prepared_runtime: &PreparedQoderRuntime,
    canonical: &CanonicalRequest,
    now_ms: i64,
    request_memory: Option<&RequestMemoryBudget>,
) -> Result<PreparedGeneration, ProxyError> {
    // Model/session derivation and payload construction normalize history and
    // tools before assembling the final COSY document. Reserve their working
    // set before any request-owned strings or JSON clones are allocated.
    let prepared_memory = request_memory
        .map(|budget| {
            budget.reserve(
                RequestMemoryComponent::NormalizedBody,
                canonical.retained_bytes().saturating_mul(3),
            )
        })
        .transpose()
        .map_err(|error| error.into_proxy_error())?;
    let model_key = runtime::resolve_qoder_model_key(
        prepared_runtime.session.session.site,
        &canonical.requested_model,
    )
    .map_err(ProxyError::bad_request)?;
    if !prepared_runtime
        .catalog
        .enabled_models
        .iter()
        .any(|enabled| enabled == &model_key)
    {
        return Err(ProxyError {
            status: StatusCode::FORBIDDEN,
            message: format!(
                "Qoder model {model_key} is not enabled in the bound account's live catalog"
            ),
        });
    }
    let exact_model_config = prepared_runtime
        .exact_model_config(&model_key)
        .ok_or_else(|| ProxyError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: format!(
                "Qoder live catalog has no exact model_config for enabled model {model_key}"
            ),
        })?;
    let conversation_session_id = runtime::derive_qoder_conversation_session_id(
        &prepared_runtime.scope,
        &canonical.share_id,
        &canonical.user_namespace,
        &canonical.downstream_session_id,
        &model_key,
    )
    .map_err(ProxyError::bad_request)?;
    if let Some(memory) = prepared_memory.as_ref() {
        memory
            .resize(
                canonical
                    .retained_bytes()
                    .saturating_mul(3)
                    .saturating_add(retained_json_bytes(exact_model_config).saturating_mul(2)),
            )
            .map_err(|error| error.into_proxy_error())?;
    }
    let payload = runtime::build_qoder_payload(
        &canonical.body,
        exact_model_config,
        prepared_runtime.session.session.site,
        &model_key,
        &conversation_session_id,
        &prepared_runtime.session.session.identity.user_type,
        now_ms,
    )
    .map_err(ProxyError::bad_request)?;
    let model_source = exact_model_config
        .get("source")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("system")
        .to_string();
    let prepared = PreparedGeneration {
        model_key,
        model_source,
        payload,
        prepared_memory,
    };
    if let Some(memory) = prepared.prepared_memory.as_ref() {
        memory
            .resize(prepared.retained_bytes())
            .map_err(|error| error.into_proxy_error())?;
    }
    Ok(prepared)
}

pub(crate) async fn runtime_is_current(
    state: &ServerState,
    execution: &ProviderExecution,
    prepared_runtime: &PreparedQoderRuntime,
) -> bool {
    state
        .qoder_runtime_generation_matches(
            execution.stored.app,
            &execution.stored.provider.id,
            execution.plan.provider_revision,
            &execution.plan.runtime_fingerprint,
            &prepared_runtime.account_id,
            prepared_runtime.auth_identity_generation,
            prepared_runtime.token_refresh_generation,
        )
        .await
}

pub(crate) async fn record_limit_if_needed(
    state: &ServerState,
    execution: &ProviderExecution,
    error: &wire::QoderUpstreamError,
) {
    if !error.is_agent_limited() && error.downstream_status() != StatusCode::TOO_MANY_REQUESTS {
        return;
    }
    let Some((ProviderType::QoderCosy, account_id, auth_identity_generation)) =
        execution.managed_account_identity_target()
    else {
        return;
    };
    let now_ms = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    let until_ms = bounded_upstream_rate_limit_until(
        now_ms,
        error
            .agent_limit_reset_at_ms
            .or_else(|| {
                error
                    .retry_after_ms
                    .map(|retry_after_ms| now_ms.saturating_add(retry_after_ms))
            })
            .unwrap_or_else(|| now_ms.saturating_add(60_000)),
    );
    state
        .mark_account_rate_limited_until_if_current(
            account_id,
            ProviderType::QoderCosy,
            auth_identity_generation,
            until_ms,
            Some(format!("Qoder rate limit is active until {until_ms}")),
        )
        .await;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::domain::qoder::{QoderCosySession, QoderIdentity, QoderMachineIdentity, QoderSite};
    use crate::proxy::qoder_runtime::{
        parse_qoder_model_catalog, QoderCachedSession, QoderRuntimeScope,
    };

    use super::*;

    async fn runtime(site: QoderSite) -> PreparedQoderRuntime {
        let rail = match site {
            QoderSite::Global => crate::domain::qoder::QoderCredentialRail::GlobalOauth,
            QoderSite::Cn => crate::domain::qoder::QoderCredentialRail::CnOauth,
        };
        let scope = QoderRuntimeScope::derive(
            "codex",
            "qoder-provider",
            3,
            "runtime-fingerprint",
            "qoder-account",
            site,
            rail,
            7,
            11,
        )
        .unwrap();
        let identity = QoderIdentity {
            name: "Fixture".to_string(),
            aid: "fixture-account".to_string(),
            uid: "fixture-user".to_string(),
            organization_id: String::new(),
            organization_name: String::new(),
            user_type: "qoder_user".to_string(),
            security_oauth_token: "fixture-access".to_string(),
            refresh_token: "fixture-refresh".to_string(),
        };
        let machine = match site {
            QoderSite::Global => QoderMachineIdentity {
                machine_id: "0123456789abcdef0123456789abcdef0123".to_string(),
                machine_token: String::new(),
                machine_type: "fixture".to_string(),
            },
            QoderSite::Cn => QoderMachineIdentity {
                machine_id: "123e4567-e89b-42d3-a456-426614174000".to_string(),
                machine_token: String::new(),
                machine_type: "fixture".to_string(),
            },
        };
        let session = QoderCosySession::new(site, identity, machine).unwrap();
        let session =
            QoderCachedSession::new(session, "https://qoder.example".to_string(), None, 1).unwrap();
        let catalog_value = json!({
            "chat": [{"key": "gmodel", "enable": true, "source": "system"}]
        });
        let fetched = parse_qoder_model_catalog(&catalog_value, site).unwrap();
        let catalog = runtime::QoderRuntimeCache::default();
        let catalog = catalog.insert_catalog(scope.clone(), fetched, 1).await;
        PreparedQoderRuntime {
            scope,
            session,
            catalog,
            credential_rail: rail,
            account_id: "qoder-account".to_string(),
            auth_identity_generation: 7,
            token_refresh_generation: 11,
        }
    }

    fn canonical(requested_model: &str) -> CanonicalRequest {
        CanonicalRequest {
            share_id: "share-qoder".to_string(),
            user_namespace: "qoder_user_fixture".to_string(),
            downstream_session_id: "session-fixture".to_string(),
            requested_model: requested_model.to_string(),
            body: json!({
                "model": requested_model,
                "messages": [{"role": "user", "content": "fixture"}],
                "stream": true
            }),
        }
    }

    #[tokio::test]
    async fn live_catalog_selection_is_exact_and_site_scoped() {
        let global = runtime(QoderSite::Global).await;
        let prepared = prepare_generation(&global, &canonical("glm-5.3"), 1_700_000_000_000, None)
            .expect("Global alias is entitled");
        assert_eq!(prepared.model_key, "gmodel");
        assert_eq!(prepared.model_source, "system");
        assert_eq!(prepared.payload.model_key, "gmodel");

        let cn = runtime(QoderSite::Cn).await;
        let missing = prepare_generation(&cn, &canonical("qwen3.7-max"), 1_700_000_000_000, None)
            .unwrap_err();
        assert_eq!(missing.status, StatusCode::FORBIDDEN);
        assert!(missing.message.contains("qmodel_latest"));
    }

    #[tokio::test]
    async fn conversation_identity_changes_with_share_and_never_falls_back() {
        let prepared_runtime = runtime(QoderSite::Global).await;
        let first = prepare_generation(
            &prepared_runtime,
            &canonical("glm-5.3"),
            1_700_000_000_000,
            None,
        )
        .unwrap();
        let mut second_request = canonical("glm-5.3");
        second_request.share_id = "share-qoder-decoy".to_string();
        let second =
            prepare_generation(&prepared_runtime, &second_request, 1_700_000_000_000, None)
                .unwrap();
        assert_ne!(first.payload.session_id, second.payload.session_id);
    }
}
