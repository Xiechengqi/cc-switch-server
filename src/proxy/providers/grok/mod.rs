//! Grok Provider reasoning-replay lifecycle.
//!
//! Wire parsing and replay proof construction remain in `proxy::grok_replay`;
//! this facade owns the Provider/Account/Share-bound cache lifecycle used by
//! the shared forwarder. It deliberately does not select another binding or
//! introduce another retry budget.

use bytes::Bytes;
use serde_json::Value;

use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::runtime::RuntimeAuthRef;
use crate::domain::usage::store::UsageLogContext;
use crate::infra::time::now_ms as current_time_ms;
use crate::state::ServerState;

use super::super::execution::context::CacheSnapshotOwnership;
use super::super::grok_replay::{
    self, GrokReplayProof, GrokReplayScope, GrokReplaySnapshot, GrokReplayStreamAccumulator,
};
use super::super::provider_ops::ProviderExecution;
use super::super::ProxyError;

#[derive(Debug, Clone)]
pub(crate) struct ReplayWriteContext {
    read: Option<(GrokReplayScope, GrokReplaySnapshot, CacheSnapshotOwnership)>,
    write_scope: GrokReplayScope,
    write_snapshot: GrokReplaySnapshot,
    write_ownership: CacheSnapshotOwnership,
    replay_applied: bool,
    app: AppKind,
    provider_id: String,
    provider_revision: u64,
    runtime_fingerprint: String,
    account_id: String,
    auth_identity_generation: u64,
    token_refresh_generation: u64,
    share_id: String,
}

impl ReplayWriteContext {
    pub(crate) fn replay_applied(&self) -> bool {
        self.replay_applied
    }

    fn reset_after_rejection(&mut self) {
        self.replay_applied = false;
        self.read = None;
    }
}

pub(crate) async fn prepare_http_replay(
    state: &ServerState,
    execution: &ProviderExecution,
    request_context: &UsageLogContext,
    headers: &axum::http::HeaderMap,
    upstream_url: &str,
    suppress_replay: bool,
    body: &mut Bytes,
) -> Result<Option<ReplayWriteContext>, ProxyError> {
    prepare_transport_replay(
        state,
        execution,
        request_context,
        super::super::grok::turn_index_from_headers(headers),
        request_context.session_id.as_deref(),
        upstream_url,
        "http",
        suppress_replay,
        body,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare_transport_replay(
    state: &ServerState,
    execution: &ProviderExecution,
    request_context: &UsageLogContext,
    turn_index: Option<u64>,
    session_id: Option<&str>,
    upstream_url: &str,
    rail: &'static str,
    suppress_replay: bool,
    body: &mut Bytes,
) -> Result<Option<ReplayWriteContext>, ProxyError> {
    if !execution.driver_is("oauth.grok_responses") {
        return Ok(None);
    }
    let Some(turn_index) = turn_index else {
        crate::metrics::record_grok_reasoning_replay("turn_absent", 1);
        return Ok(None);
    };
    let Some(share_id) = request_context
        .share_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        crate::metrics::record_grok_reasoning_replay("share_absent", 1);
        return Ok(None);
    };
    let Some(user_namespace) = request_context
        .user_email
        .as_deref()
        .and_then(grok_replay::user_namespace)
    else {
        crate::metrics::record_grok_reasoning_replay("user_absent", 1);
        return Ok(None);
    };
    let session_id = session_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProxyError::bad_request("Grok reasoning replay requires a session"))?;
    let document = serde_json::from_slice::<Value>(body)
        .map_err(|_| ProxyError::bad_request("Grok reasoning replay request is invalid"))?;
    let model_family = grok_replay::model_family(
        document
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| ProxyError::bad_request("Grok reasoning replay requires a model"))?,
    )
    .ok_or_else(|| ProxyError::bad_request("Grok reasoning replay model is invalid"))?;
    let upstream_plane = reqwest::Url::parse(upstream_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .ok_or_else(|| ProxyError::bad_request("Grok reasoning replay plane is invalid"))?;
    let (provider_type, account_id, auth_identity_generation) = execution
        .managed_account_identity_target()
        .filter(|(provider_type, _, _)| *provider_type == ProviderType::GrokOAuth)
        .ok_or_else(|| ProxyError::bad_request("Grok Provider must bind one Account"))?;
    let account = state
        .find_account_for_provider(provider_type, account_id)
        .await
        .filter(|account| account.auth_identity_generation == auth_identity_generation)
        .ok_or_else(|| ProxyError::bad_request("Grok Account generation drifted"))?;
    let token_refresh_generation = account.token_refresh_generation;
    let derive = |turn| {
        GrokReplayScope::derive(
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
            turn,
            &model_family,
            rail,
            &upstream_plane,
        )
    };
    let write_scope = derive(turn_index)
        .ok_or_else(|| ProxyError::bad_request("Grok reasoning replay scope is incomplete"))?;
    let now_ms = replay_now_ms();
    let (_, write_snapshot) = state.grok_reasoning_replays.get(&write_scope, now_ms).await;
    let mut read = None;
    let mut replay_applied = false;
    if turn_index > 0 && !suppress_replay {
        let read_scope = derive(turn_index - 1)
            .ok_or_else(|| ProxyError::bad_request("Grok reasoning replay scope is incomplete"))?;
        let (proof, snapshot) = state.grok_reasoning_replays.get(&read_scope, now_ms).await;
        let ownership = CacheSnapshotOwnership::from_hit(
            "grok_reasoning_read",
            read_scope.ownership_digest(),
            snapshot.generation(),
        );
        read = Some((read_scope.clone(), snapshot, ownership.clone()));
        if let Some(proof) = proof {
            let result = grok_replay::apply(body, &proof);
            if result.context_mismatch {
                crate::metrics::record_grok_reasoning_replay("context_mismatch", 1);
                if ownership.authorize_mutation(
                    "grok_reasoning_read",
                    &read_scope.ownership_digest(),
                    snapshot.generation(),
                ) {
                    state
                        .grok_reasoning_replays
                        .delete_if_unchanged(&read_scope, snapshot, now_ms)
                        .await;
                }
                return Err(ProxyError::bad_request(
                    "Grok reasoning replay context does not match the cached turn",
                ));
            }
            if result.applied {
                *body = result.body;
                replay_applied = true;
                crate::metrics::record_grok_reasoning_replay("hit", 1);
            } else {
                crate::metrics::record_grok_reasoning_replay("present_noop", 1);
            }
        } else {
            crate::metrics::record_grok_reasoning_replay("miss", 1);
        }
    }
    Ok(Some(ReplayWriteContext {
        read,
        write_ownership: CacheSnapshotOwnership::from_hit(
            "grok_reasoning_write",
            write_scope.ownership_digest(),
            write_snapshot.generation(),
        ),
        write_scope,
        write_snapshot,
        replay_applied,
        app: execution.plan.provider_key.app,
        provider_id: execution.stored.provider.id.clone(),
        provider_revision: execution.plan.provider_revision,
        runtime_fingerprint: execution.plan.runtime_fingerprint.clone(),
        account_id: account_id.to_string(),
        auth_identity_generation,
        token_refresh_generation,
        share_id: share_id.to_string(),
    }))
}

pub(crate) async fn clear_rejected(state: &ServerState, context: Option<&ReplayWriteContext>) {
    let Some((scope, snapshot, ownership)) = context
        .filter(|context| context.replay_applied)
        .and_then(|context| context.read.as_ref())
    else {
        return;
    };
    if !ownership.authorize_mutation(
        "grok_reasoning_read",
        &scope.ownership_digest(),
        snapshot.generation(),
    ) {
        crate::metrics::record_grok_reasoning_replay("rejection_ownership_denied", 1);
        return;
    }
    let deleted = state
        .grok_reasoning_replays
        .delete_if_unchanged(scope, *snapshot, replay_now_ms())
        .await;
    crate::metrics::record_grok_reasoning_replay(
        if deleted {
            "rejection_deleted"
        } else {
            "rejection_cas_conflict"
        },
        1,
    );
}

pub(crate) async fn commit(
    state: &ServerState,
    context: Option<&ReplayWriteContext>,
    proof: Option<GrokReplayProof>,
) {
    let Some(context) = context else {
        return;
    };
    if !binding_is_current(state, context).await {
        crate::metrics::record_grok_reasoning_replay("binding_drift", 1);
        return;
    }
    if !context.write_ownership.authorize_mutation(
        "grok_reasoning_write",
        &context.write_scope.ownership_digest(),
        context.write_snapshot.generation(),
    ) {
        crate::metrics::record_grok_reasoning_replay("commit_ownership_denied", 1);
        return;
    }
    let now_ms = replay_now_ms();
    let (outcome, committed) = if let Some(proof) = proof {
        let committed = state
            .grok_reasoning_replays
            .replace_if_unchanged(
                context.write_scope.clone(),
                context.write_snapshot,
                proof,
                now_ms,
            )
            .await;
        ("commit", committed)
    } else {
        let committed = state
            .grok_reasoning_replays
            .delete_if_unchanged(&context.write_scope, context.write_snapshot, now_ms)
            .await;
        ("non_replayable", committed)
    };
    crate::metrics::record_grok_reasoning_replay(
        if committed {
            outcome
        } else {
            "commit_cas_conflict"
        },
        1,
    );
}

pub(crate) async fn binding_is_current(state: &ServerState, context: &ReplayWriteContext) -> bool {
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
                expected_provider_type: ProviderType::GrokOAuth,
                auth_identity_generation,
            } if account_id == &context.account_id
                && *auth_identity_generation == context.auth_identity_generation
        )
    {
        return false;
    }
    let Some(account) = state
        .find_account_for_provider(ProviderType::GrokOAuth, &context.account_id)
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
            && ((share.app == context.app
                && share.provider_id == context.provider_id
                && share.provider_type == ProviderType::GrokOAuth)
                || share.bindings.iter().any(|binding| {
                    binding.app == context.app
                        && binding.provider_id == context.provider_id
                        && binding.provider_type == ProviderType::GrokOAuth
                }))
    })
}

#[derive(Debug)]
pub(crate) struct ReplayStreamWrite {
    context: ReplayWriteContext,
    accumulator: GrokReplayStreamAccumulator,
}

impl ReplayStreamWrite {
    pub(crate) fn new(context: ReplayWriteContext) -> Self {
        Self {
            context,
            accumulator: GrokReplayStreamAccumulator::default(),
        }
    }

    pub(crate) fn inspect(&mut self, chunk: &[u8]) {
        self.accumulator.push(chunk);
    }

    pub(crate) fn replay_applied(&self) -> bool {
        self.context.replay_applied()
    }

    pub(crate) async fn binding_is_current(&self, state: &ServerState) -> bool {
        binding_is_current(state, &self.context).await
    }

    pub(crate) async fn clear_rejected_and_reset(&mut self, state: &ServerState) {
        clear_rejected(state, Some(&self.context)).await;
        self.context.reset_after_rejection();
        self.accumulator = GrokReplayStreamAccumulator::default();
    }

    pub(crate) async fn commit(self, state: &ServerState) {
        let proof = self.accumulator.finish();
        commit(state, Some(&self.context), proof).await;
    }
}

fn replay_now_ms() -> i64 {
    current_time_ms().min(i64::MAX as u128) as i64
}
