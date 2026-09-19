//! CodeBuddy OAuth Provider lifecycle facade.
//!
//! The SSE codec and terminal lifecycle remain in `proxy::codebuddy`, while
//! catalog caching and payload primitives remain in `proxy::codebuddy_runtime`.
//! This module owns the Provider-specific boundary around the exact managed
//! Account binding, canonical request, live-catalog model/capability selection,
//! payload preparation, and generation fencing. Share/Account leases, usage,
//! terminal handling, the shared attempt budget, and the decision to replay a
//! pre-commit 401 deliberately remain in the forwarder.

use axum::http::StatusCode;
use serde_json::Value;

use crate::domain::providers::model::ProviderType;
use crate::state::{CodeBuddyRuntimeError, ServerState};

use super::super::adapters::AdapterRequest;
use super::super::codebuddy_runtime::{self as runtime, PreparedCodeBuddyRuntime};
use super::super::provider_ops::ProviderExecution;
use super::super::ProxyError;

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
            "CodeBuddy Provider must bind one explicit codebuddy_oauth managed account",
        ));
    };
    if provider_type != ProviderType::CodeBuddyOAuth {
        return Err(ProxyError::bad_request(
            "CodeBuddy Provider account type does not match its runtime contract",
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
) -> Result<PreparedCodeBuddyRuntime, CodeBuddyRuntimeError> {
    state
        .prepare_codebuddy_runtime(
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
    pub(crate) requested_model: String,
    pub(crate) body: Value,
}

pub(crate) fn prepare_canonical_request(
    request: &AdapterRequest,
) -> Result<CanonicalRequest, ProxyError> {
    let requested_model = request
        .actual_model
        .clone()
        .or_else(|| request.model.clone())
        .or_else(|| model_from_canonical_chat(&request.body))
        .ok_or_else(|| ProxyError::bad_request("CodeBuddy request is missing a model"))?;
    let body = serde_json::from_slice::<Value>(&request.body).map_err(|error| {
        ProxyError::bad_request(format!("invalid CodeBuddy Chat request: {error}"))
    })?;
    Ok(CanonicalRequest {
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

#[derive(Debug)]
pub(crate) enum GenerationPreparationError {
    /// Preserve the pre-facade direct error path without recording a request outcome.
    Direct(ProxyError),
    /// Preserve the explicit forbidden-entitlement failure record.
    RecordFailure(ProxyError),
    /// Preserve capability failures routed through the shared pre-commit recorder.
    FailBeforeCommit(ProxyError),
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedGeneration {
    pub(crate) model_id: String,
    pub(crate) payload: Value,
}

pub(crate) fn prepare_generation(
    prepared_runtime: &PreparedCodeBuddyRuntime,
    canonical: &CanonicalRequest,
) -> Result<PreparedGeneration, GenerationPreparationError> {
    let model_id = runtime::resolve_codebuddy_model_id(
        prepared_runtime.profile.site,
        &canonical.requested_model,
    )
    .map_err(|error| GenerationPreparationError::Direct(ProxyError::bad_request(error)))?;
    if !prepared_runtime
        .catalog
        .enabled_models
        .iter()
        .any(|enabled| enabled == &model_id)
    {
        return Err(GenerationPreparationError::RecordFailure(ProxyError {
            status: StatusCode::FORBIDDEN,
            message: format!(
                "CodeBuddy model {model_id} is not enabled in the bound account's live catalog"
            ),
        }));
    }
    let capability = prepared_runtime
        .catalog
        .capabilities
        .get(&model_id)
        .ok_or_else(|| {
            GenerationPreparationError::Direct(ProxyError::bad_gateway(format!(
                "CodeBuddy live catalog has no capability record for enabled model {model_id}"
            )))
        })?;
    let has_tools = canonical
        .body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty());
    if has_tools && !capability.supports_tools {
        return Err(GenerationPreparationError::FailBeforeCommit(
            ProxyError::bad_request(format!(
                "CodeBuddy model {model_id} does not advertise tool support"
            )),
        ));
    }
    let requests_reasoning = canonical.body.get("reasoning").is_some()
        || canonical.body.get("reasoning_effort").is_some();
    if requests_reasoning && !capability.supports_reasoning {
        return Err(GenerationPreparationError::FailBeforeCommit(
            ProxyError::bad_request(format!(
                "CodeBuddy model {model_id} does not advertise reasoning support"
            )),
        ));
    }
    let payload = runtime::build_codebuddy_payload(&canonical.body, &model_id, capability)
        .map_err(|error| GenerationPreparationError::Direct(ProxyError::bad_request(error)))?;
    Ok(PreparedGeneration { model_id, payload })
}

pub(crate) async fn runtime_is_current(
    state: &ServerState,
    execution: &ProviderExecution,
    prepared_runtime: &PreparedCodeBuddyRuntime,
) -> bool {
    state
        .codebuddy_runtime_generation_matches(
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::domain::codebuddy::{
        CodeBuddyAccountProfile, CodeBuddySite, CODEBUDDY_CLIENT_VERSION, CODEBUDDY_PLATFORM,
    };
    use crate::proxy::codebuddy_runtime::{
        parse_codebuddy_model_catalog, CodeBuddyRuntimeCache, CodeBuddyRuntimeScope,
    };

    use super::*;

    async fn prepared_runtime(
        site: CodeBuddySite,
        model: &str,
        supports_tools: bool,
        supports_reasoning: bool,
    ) -> PreparedCodeBuddyRuntime {
        let domain = site.profile().domain.to_string();
        let scope = CodeBuddyRuntimeScope::derive(
            "codex",
            "codebuddy-provider",
            3,
            "runtime-fingerprint",
            "codebuddy-account",
            site,
            &domain,
            7,
            11,
        )
        .unwrap();
        let fetched = parse_codebuddy_model_catalog(
            &json!({"data":{"models":[{
                "id": model,
                "name": "Fixture",
                "supportsToolCall": supports_tools,
                "supportsReasoning": supports_reasoning,
                "reasoning": {"defaultEffort":"high","supportedEfforts":["low","high"]}
            }]}}),
            site,
        )
        .unwrap();
        let catalog = CodeBuddyRuntimeCache::default()
            .insert_catalog(scope.clone(), fetched, 1_700_000_000_000)
            .await;
        PreparedCodeBuddyRuntime {
            scope,
            profile: CodeBuddyAccountProfile {
                site,
                domain,
                uid: "fixture-user".to_string(),
                enterprise_id: String::new(),
                name: "Fixture".to_string(),
                email: String::new(),
                nickname: String::new(),
                account_type: "personal".to_string(),
                client_version: CODEBUDDY_CLIENT_VERSION.to_string(),
                product_platform: CODEBUDDY_PLATFORM.to_string(),
            },
            access_token: "fixture-access".into(),
            base_url: "https://codebuddy.example".to_string(),
            catalog,
            account_id: "codebuddy-account".to_string(),
            auth_identity_generation: 7,
            token_refresh_generation: 11,
        }
    }

    fn canonical(requested_model: &str, extra: Value) -> CanonicalRequest {
        let mut body = json!({
            "model": requested_model,
            "messages": [{"role":"user","content":"fixture"}]
        });
        for (key, value) in extra.as_object().unwrap() {
            body.as_object_mut()
                .unwrap()
                .insert(key.clone(), value.clone());
        }
        CanonicalRequest {
            requested_model: requested_model.to_string(),
            body,
        }
    }

    #[tokio::test]
    async fn generation_uses_exact_site_bound_live_catalog() {
        let intl = prepared_runtime(CodeBuddySite::Intl, "default-model", true, true).await;
        let intl_generation = prepare_generation(&intl, &canonical("auto", json!({}))).unwrap();
        assert_eq!(intl_generation.model_id, "default-model");
        assert_eq!(intl_generation.payload["model"], "default-model");

        let cn = prepared_runtime(CodeBuddySite::Cn, "default", true, true).await;
        let cn_generation = prepare_generation(&cn, &canonical("auto", json!({}))).unwrap();
        assert_eq!(cn_generation.model_id, "default");
        assert_eq!(cn_generation.payload["model"], "default");

        let missing =
            prepare_generation(&cn, &canonical("deepseek-v4.1-flash", json!({}))).unwrap_err();
        assert!(matches!(
            missing,
            GenerationPreparationError::RecordFailure(ProxyError {
                status: StatusCode::FORBIDDEN,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn generation_fails_closed_on_unadvertised_capabilities() {
        let runtime = prepared_runtime(CodeBuddySite::Intl, "default-model", false, false).await;
        let tools = prepare_generation(
            &runtime,
            &canonical(
                "default-model",
                json!({"tools":[{"type":"function","function":{"name":"lookup"}}]}),
            ),
        )
        .unwrap_err();
        assert!(matches!(
            tools,
            GenerationPreparationError::FailBeforeCommit(_)
        ));
        let reasoning = prepare_generation(
            &runtime,
            &canonical("default-model", json!({"reasoning":{"effort":"high"}})),
        )
        .unwrap_err();
        assert!(matches!(
            reasoning,
            GenerationPreparationError::FailBeforeCommit(_)
        ));
    }
}
