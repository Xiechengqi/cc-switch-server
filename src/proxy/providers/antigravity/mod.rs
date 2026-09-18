use axum::http::header::RETRY_AFTER;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use bytes::Bytes;
use serde_json::Value;

use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::runtime::RuntimeAuthRef;
use crate::domain::usage::store::UsageLogContext;
use crate::infra::time::now_ms as current_time_ms;
use crate::state::ServerState;

use super::super::antigravity_replay::{
    self, AntigravityReplayChain, AntigravityReplayScope, AntigravityReplaySnapshot,
    AntigravityReplayStreamAccumulator,
};
use super::super::antigravity_retry::{self, AntigravityRetryInfo};
use super::super::antigravity_transport::{AntigravityTransportKey, AntigravityTransportPolicy};
use super::super::execution::context::CacheSnapshotOwnership;
use super::super::provider_ops::ProviderExecution;
use super::super::router::ProxyRoute;
use super::super::{bounded_upstream_rate_limit_until, ProxyError};

pub(crate) fn is_provider(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
    )
}

pub(crate) fn limit_info(
    execution: &ProviderExecution,
    status: StatusCode,
    body: &[u8],
) -> Option<AntigravityRetryInfo> {
    is_provider(execution.stored.provider_type)
        .then(|| antigravity_retry::parse_google_rpc_retry(status.as_u16(), body))
        .flatten()
}

pub(crate) fn limit_retry_source(
    retry_attempted: bool,
    retry_allowed: bool,
    execution: &ProviderExecution,
    limit: &AntigravityRetryInfo,
) -> Option<&'static str> {
    if retry_attempted
        || !retry_allowed
        || !limit.is_short_delay()
        || execution.managed_account_identity_target().is_none()
    {
        return None;
    }
    Some(limit.kind.reason())
}

pub(crate) fn session_rollover_source(
    execution: &ProviderExecution,
    status: StatusCode,
    body: &[u8],
    rollover_attempted: bool,
    retry_allowed: bool,
    has_session: bool,
) -> Option<&'static str> {
    (is_provider(execution.stored.provider_type)
        && antigravity_replay::is_session_accumulation_error(status.as_u16(), body)
        && !rollover_attempted
        && retry_allowed
        && execution.managed_account_identity_target().is_some()
        && has_session)
        .then_some("session_accumulation_exceeded")
}

pub(crate) async fn record_limit_evidence(
    state: &ServerState,
    execution: &ProviderExecution,
    limit: &AntigravityRetryInfo,
) {
    use crate::domain::accounts::capability_evidence::AccountCapabilityObservationState::{
        Unknown, Unsupported,
    };

    let Some((provider_type, account_id, auth_identity_generation)) =
        execution.managed_account_identity_target()
    else {
        return;
    };
    let observation_state = match limit.kind {
        antigravity_retry::AntigravityLimitKind::RateLimit => Unknown,
        antigravity_retry::AntigravityLimitKind::ModelCapacity => Unsupported,
    };
    let now_ms = current_time_ms().min(i64::MAX as u128) as i64;
    let expires_at_ms =
        now_ms.saturating_add(i64::try_from(limit.retry_delay_ms.max(1_000)).unwrap_or(i64::MAX));
    if let Err(error) = state
        .record_antigravity_capability_observation_if_current(
            account_id,
            crate::domain::accounts::capability_evidence::MODEL_CAPACITY_DIMENSION,
            crate::state::CurrentAccountCapabilityObservation {
                provider_type,
                auth_identity_generation,
                state: observation_state,
                source: "google_rpc_error",
                reason: Some(limit.kind.reason()),
                expires_at_ms: Some(expires_at_ms),
            },
        )
        .await
    {
        tracing::warn!(
            account_id,
            provider_type = provider_type.as_str(),
            error = %error,
            "failed to persist Antigravity capacity evidence"
        );
    }
}

pub(crate) fn mark_limit_cooldown(
    state: &ServerState,
    execution: &ProviderExecution,
    share_id: Option<&str>,
    limit: &AntigravityRetryInfo,
) {
    let Some(share_id) = share_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let now_ms = current_time_ms().min(i64::MAX as u128) as i64;
    let delay_ms = i64::try_from(limit.retry_delay_ms.max(1_000)).unwrap_or(i64::MAX);
    let until_ms = bounded_upstream_rate_limit_until(now_ms, now_ms.saturating_add(delay_ms));
    state.mark_share_model_cooldown(
        share_id,
        &execution.plan.runtime_fingerprint,
        &limit.model,
        until_ms,
        limit.kind.reason(),
        now_ms,
    );
}

pub(crate) fn install_retry_after(headers: &mut HeaderMap, limit: &AntigravityRetryInfo) {
    if headers.contains_key(RETRY_AFTER) {
        return;
    }
    let seconds = limit.retry_after_seconds().max(1);
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        headers.insert(RETRY_AFTER, value);
    }
}

pub(crate) async fn http_client(
    state: &ServerState,
    execution: &ProviderExecution,
) -> Result<Option<reqwest::Client>, ProxyError> {
    if !is_provider(execution.stored.provider_type) {
        return Ok(None);
    }
    let policy = AntigravityTransportPolicy::resolve(&execution.plan.transport_policy);
    let key = execution.managed_account_identity_target().and_then(
        |(provider_type, account_id, auth_identity_generation)| {
            AntigravityTransportKey::new(
                execution.plan.provider_key.app,
                provider_type,
                &execution.stored.provider.id,
                execution.plan.provider_revision,
                &execution.plan.runtime_fingerprint,
                account_id,
                auth_identity_generation,
                policy,
            )
        },
    );
    let client = state
        .antigravity_transports
        .client(key, policy)
        .await
        .map_err(ProxyError::bad_gateway)?;
    Ok(Some(client))
}

pub(crate) fn apply_session_contract(
    execution: &ProviderExecution,
    route: ProxyRoute,
    request_context: &UsageLogContext,
    session_generation: u64,
    body: &mut Bytes,
) {
    if route == ProxyRoute::ClaudeCountTokens || !is_provider(execution.stored.provider_type) {
        return;
    }
    let Ok(document) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    if document.get("requestType").and_then(Value::as_str) == Some("web_search") {
        return;
    }
    let Some((_, account_id, _)) = execution.managed_account_identity_target() else {
        return;
    };
    let Some(conversation_scope) = request_context
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let Some(session_id) =
        antigravity_replay::derive_session_id(account_id, conversation_scope, session_generation)
    else {
        return;
    };
    if antigravity_replay::apply_session_id(body, &session_id) {
        crate::metrics::record_antigravity_reasoning_replay("session_id_applied", 1);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ReplayWriteContext {
    scope: AntigravityReplayScope,
    snapshot: AntigravityReplaySnapshot,
    ownership: CacheSnapshotOwnership,
    previous_chain: Option<AntigravityReplayChain>,
    replay_applied: bool,
    app: AppKind,
    provider_id: String,
    provider_revision: u64,
    runtime_fingerprint: String,
    provider_type: ProviderType,
    account_id: String,
    auth_identity_generation: u64,
    token_refresh_generation: u64,
    share_id: String,
}

pub(crate) async fn prepare_reasoning_replay(
    state: &ServerState,
    execution: &ProviderExecution,
    route: ProxyRoute,
    request_context: &UsageLogContext,
    upstream_url: &str,
    body: &mut Bytes,
) -> Option<ReplayWriteContext> {
    if route == ProxyRoute::ClaudeCountTokens || !is_provider(execution.stored.provider_type) {
        return None;
    }
    let share_id = request_context
        .share_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let user_namespace = antigravity_replay::user_namespace(
        request_context
            .user_email
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())?,
    )?;
    let session_id = request_context
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let document = serde_json::from_slice::<Value>(body).ok()?;
    if document.get("requestType").and_then(Value::as_str) == Some("web_search") {
        return None;
    }
    let model = document.get("model")?.as_str()?;
    let model_family = antigravity_replay::model_family(model)?;
    let upstream_plane = reqwest::Url::parse(upstream_url)
        .ok()?
        .host_str()?
        .to_ascii_lowercase();
    let (provider_type, account_id, auth_identity_generation) =
        execution.managed_account_identity_target()?;
    if !is_provider(provider_type) {
        return None;
    }
    let account = state
        .find_account_for_provider(provider_type, account_id)
        .await
        .filter(|account| account.auth_identity_generation == auth_identity_generation)?;
    let token_refresh_generation = account.token_refresh_generation;
    let scope = AntigravityReplayScope::derive(
        execution.plan.provider_key.app.as_str(),
        &execution.stored.provider.id,
        execution.plan.provider_revision,
        &execution.plan.runtime_fingerprint,
        account_id,
        auth_identity_generation,
        token_refresh_generation,
        share_id,
        &user_namespace,
        session_id,
        &model_family,
        &upstream_plane,
    )?;
    let now_ms = replay_now_ms();
    let (mut previous_chain, mut snapshot) = state
        .antigravity_reasoning_replays
        .get(&scope, now_ms)
        .await;
    let mut ownership = CacheSnapshotOwnership::from_hit(
        "antigravity_reasoning",
        scope.ownership_digest(),
        snapshot.generation(),
    );
    let mut replay_applied = false;
    if let Some(chain) = previous_chain.as_ref() {
        let result = antigravity_replay::apply_replay(body, chain);
        if result.applied {
            *body = result.body;
            replay_applied = true;
            crate::metrics::record_antigravity_reasoning_replay("hit", 1);
        } else if result.context_mismatch {
            crate::metrics::record_antigravity_reasoning_replay("context_mismatch", 1);
            if ownership.authorize_mutation(
                "antigravity_reasoning",
                &scope.ownership_digest(),
                snapshot.generation(),
            ) && state
                .antigravity_reasoning_replays
                .delete_if_unchanged(&scope, snapshot, now_ms)
                .await
            {
                let refreshed = state
                    .antigravity_reasoning_replays
                    .get(&scope, now_ms)
                    .await;
                previous_chain = refreshed.0;
                snapshot = refreshed.1;
                ownership = CacheSnapshotOwnership::from_hit(
                    "antigravity_reasoning",
                    scope.ownership_digest(),
                    snapshot.generation(),
                );
            }
        } else {
            crate::metrics::record_antigravity_reasoning_replay("present_noop", 1);
        }
    } else {
        crate::metrics::record_antigravity_reasoning_replay("miss", 1);
    }
    Some(ReplayWriteContext {
        scope,
        snapshot,
        ownership,
        previous_chain,
        replay_applied,
        app: execution.plan.provider_key.app,
        provider_id: execution.stored.provider.id.clone(),
        provider_revision: execution.plan.provider_revision,
        runtime_fingerprint: execution.plan.runtime_fingerprint.clone(),
        provider_type,
        account_id: account_id.to_string(),
        auth_identity_generation,
        token_refresh_generation,
        share_id: share_id.to_string(),
    })
}

pub(crate) async fn clear_rejected_reasoning_replay(
    state: &ServerState,
    context: Option<&ReplayWriteContext>,
) {
    let Some(context) = context.filter(|context| context.replay_applied) else {
        return;
    };
    if !context.ownership.authorize_mutation(
        "antigravity_reasoning",
        &context.scope.ownership_digest(),
        context.snapshot.generation(),
    ) {
        crate::metrics::record_antigravity_reasoning_replay("delete_ownership_denied", 1);
        return;
    }
    let deleted = state
        .antigravity_reasoning_replays
        .delete_if_unchanged(&context.scope, context.snapshot, replay_now_ms())
        .await;
    crate::metrics::record_antigravity_reasoning_replay(
        if deleted {
            "upstream_rejected"
        } else {
            "delete_conflict"
        },
        1,
    );
}

pub(crate) fn is_signature_rejection(status: StatusCode, body: &[u8]) -> bool {
    antigravity_replay::is_signature_rejection(status.as_u16(), body)
}

#[derive(Debug)]
pub(crate) struct ReplayCapture {
    terminal: bool,
    chain: Option<AntigravityReplayChain>,
}

pub(crate) fn capture_response(
    context: Option<&ReplayWriteContext>,
    request_body: &[u8],
    response_body: &[u8],
    successful: bool,
) -> ReplayCapture {
    let terminal = successful && antigravity_replay::response_has_terminal(response_body);
    let chain = terminal
        .then(|| {
            context.and_then(|context| {
                antigravity_replay::capture_response(
                    request_body,
                    response_body,
                    context.previous_chain.as_ref(),
                )
            })
        })
        .flatten();
    ReplayCapture { terminal, chain }
}

pub(crate) async fn commit_captured_response(
    state: &ServerState,
    context: Option<&ReplayWriteContext>,
    capture: ReplayCapture,
) {
    commit_reasoning_replay(state, context, capture.terminal, capture.chain).await;
}

#[derive(Debug)]
pub(crate) struct ReplayStreamWrite {
    context: ReplayWriteContext,
    request_body: Bytes,
    accumulator: AntigravityReplayStreamAccumulator,
}

impl ReplayStreamWrite {
    pub(crate) fn new(context: ReplayWriteContext, request_body: Bytes) -> Self {
        Self {
            context,
            request_body,
            accumulator: AntigravityReplayStreamAccumulator::default(),
        }
    }

    pub(crate) fn inspect(&mut self, chunk: &[u8]) {
        self.accumulator.push(chunk);
    }

    fn is_complete(&self) -> bool {
        self.accumulator.is_complete()
    }
}

pub(crate) async fn commit_complete_stream(
    state: &ServerState,
    replay: &mut Option<ReplayStreamWrite>,
) {
    if !replay.as_ref().is_some_and(ReplayStreamWrite::is_complete) {
        return;
    }
    let Some(replay) = replay.take() else {
        return;
    };
    let previous_chain = replay.context.previous_chain.clone();
    let chain = replay
        .accumulator
        .finish(&replay.request_body, previous_chain.as_ref());
    commit_reasoning_replay(state, Some(&replay.context), true, chain).await;
}

async fn commit_reasoning_replay(
    state: &ServerState,
    context: Option<&ReplayWriteContext>,
    terminal: bool,
    chain: Option<AntigravityReplayChain>,
) {
    let Some(context) = context.filter(|_| terminal) else {
        return;
    };
    if !reasoning_replay_binding_is_current(state, context).await {
        crate::metrics::record_antigravity_reasoning_replay("binding_drift", 1);
        return;
    }
    if !context.ownership.authorize_mutation(
        "antigravity_reasoning",
        &context.scope.ownership_digest(),
        context.snapshot.generation(),
    ) {
        crate::metrics::record_antigravity_reasoning_replay("write_ownership_denied", 1);
        return;
    }
    let now_ms = replay_now_ms();
    let stored = if let Some(chain) = chain {
        state
            .antigravity_reasoning_replays
            .replace_if_unchanged(context.scope.clone(), context.snapshot, chain, now_ms)
            .await
    } else {
        state
            .antigravity_reasoning_replays
            .delete_if_unchanged(&context.scope, context.snapshot, now_ms)
            .await
    };
    crate::metrics::record_antigravity_reasoning_replay(
        if stored {
            if context.previous_chain.is_some() {
                "updated"
            } else {
                "stored"
            }
        } else {
            "write_conflict"
        },
        1,
    );
}

async fn reasoning_replay_binding_is_current(
    state: &ServerState,
    context: &ReplayWriteContext,
) -> bool {
    if state.credential_persistence_degraded() {
        return false;
    }
    let Some(plan) = state
        .provider_runtime_plan(context.app, &context.provider_id)
        .await
    else {
        return false;
    };
    if plan.provider_revision != context.provider_revision
        || plan.runtime_fingerprint != context.runtime_fingerprint
        || !matches!(
            &plan.auth_ref,
            RuntimeAuthRef::ManagedAccount {
                account_id,
                expected_provider_type,
                auth_identity_generation,
            } if account_id == &context.account_id
                && *expected_provider_type == context.provider_type
                && *auth_identity_generation == context.auth_identity_generation
        )
    {
        return false;
    }
    let Some(account) = state
        .find_account_for_provider(context.provider_type, &context.account_id)
        .await
    else {
        return false;
    };
    if account.auth_identity_generation != context.auth_identity_generation
        || account.token_refresh_generation != context.token_refresh_generation
    {
        return false;
    }
    let shares = state.shares.read().await;
    shares.get(&context.share_id).is_some_and(|share| {
        share.enabled
            && share.status == "active"
            && share.bindings.iter().any(|binding| {
                binding.app == context.app
                    && binding.provider_id == context.provider_id
                    && binding.provider_type == context.provider_type
            })
    })
}

fn replay_now_ms() -> i64 {
    current_time_ms().min(i64::MAX as u128) as i64
}
