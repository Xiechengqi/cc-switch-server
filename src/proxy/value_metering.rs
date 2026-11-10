//! Provider-independent admission and response lifetime for USD quotas.
//! Invoke after model/payload preparation, immediately before generation IO.
//! Usage logs settle the same ledger; the guard closes every other exit.
use axum::{body::Body, response::Response};
use futures_util::StreamExt;

use super::{provider_ops::ProviderExecution, ProxyError};
use crate::{
    domain::usage::{
        store::UsageLogContext,
        value_pricing::{canonical_service_tier, supports_value_text_runtime, value_request_facts},
    },
    state::{ServerState, ValueAttemptGuard},
};

/// A series is shared only by the supervisor and response-lifetime guard;
/// Driver generation operations remain sequential.
#[derive(Default)]
pub(crate) struct ValueAttemptSeries {
    attempt: Option<ValueAttemptGuard>,
    pub(crate) number: u32,
}
impl std::fmt::Debug for ValueAttemptSeries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValueAttemptSeries")
            .field("number", &self.number)
            .finish()
    }
}
impl ValueAttemptSeries {
    pub(crate) async fn begin(
        &mut self,
        state: &ServerState,
        context: &UsageLogContext,
        model: &str,
        payload: &[u8],
    ) -> Result<(), ProxyError> {
        if let Some(previous) = self.attempt.take() {
            previous.settle_now().await;
        }
        self.number = self
            .number
            .checked_add(1)
            .ok_or_else(|| ProxyError::bad_gateway("monetary attempt identity exhausted"))?;
        let mut context = context.clone();
        context.attempt_count = Some(self.number);
        self.attempt = admit_prepared(state, &context, model, payload, true).await?;
        Ok(())
    }
    pub(crate) fn sent(&self) {
        mark_sent(&self.attempt);
    }
    pub(crate) async fn auth_rejected(&mut self) {
        authentication_rejected(&mut self.attempt).await;
    }
    pub(crate) async fn observe(
        &self,
        usage: crate::domain::usage::store::TokenUsage,
        estimated: bool,
    ) -> Result<(), ProxyError> {
        if let Some(attempt) = &self.attempt {
            attempt
                .observe_tokens(usage, estimated)
                .await
                .map_err(super::forwarder::value_quota_proxy_error)?;
        }
        Ok(())
    }
}

pub(crate) fn series_response(
    response: Response,
    series: std::sync::Arc<tokio::sync::Mutex<ValueAttemptSeries>>,
) -> Response {
    let (parts, body) = response.into_parts();
    let stream = async_stream::stream! {
        let _series = series;
        let mut stream = body.into_data_stream();
        while let Some(frame) = stream.next().await { yield frame; }
    };
    Response::from_parts(parts, Body::from_stream(stream))
}

pub(crate) async fn admit(
    state: &ServerState,
    execution: &ProviderExecution,
    context: &UsageLogContext,
    model: &str,
    payload: &[u8],
) -> Result<Option<ValueAttemptGuard>, ProxyError> {
    admit_prepared(
        state,
        context,
        model,
        payload,
        supports_value_text_runtime(
            execution.plan.profile_id.as_str(),
            execution.plan.driver_id.as_str(),
            execution.plan.coding_plan.is_some(),
        ),
    )
    .await
}

pub(crate) async fn admit_prepared(
    state: &ServerState,
    context: &UsageLogContext,
    model: &str,
    payload: &[u8],
    verified_runtime: bool,
) -> Result<Option<ValueAttemptGuard>, ProxyError> {
    admit_prepared_with_plan(state, context, model, payload, verified_runtime, None).await
}

pub(crate) async fn admit_prepared_with_plan(
    state: &ServerState,
    context: &UsageLogContext,
    model: &str,
    payload: &[u8],
    verified_runtime: bool,
    resource_plan: Option<crate::domain::usage::value_pricing::ResourcePlan>,
) -> Result<Option<ValueAttemptGuard>, ProxyError> {
    if context.is_health_check {
        return Ok(None);
    }
    let (Some(share), Some(email)) = (context.share_id.as_deref(), context.user_email.as_deref())
    else {
        return Ok(None);
    };
    if !state.share_uses_value_quota(share, Some(email)).await {
        return Ok(None);
    }
    let facts = value_request_facts(payload);
    let plan = resource_plan.or_else(|| {
        serde_json::from_slice(payload).ok().map(|body| {
            crate::domain::usage::value_pricing::resource_request_plan(&body, context.operation)
        })
    });
    state
        .admit_share_value_request(
            context,
            model,
            canonical_service_tier(
                facts
                    .service_tier
                    .as_deref()
                    .or(context.effective_service_tier.as_deref()),
            ),
            facts.estimate,
            verified_runtime,
            plan,
        )
        .await
        .map_err(super::forwarder::value_quota_proxy_error)
}

/// An authentication rejection before any semantic output proves that this
/// attempt can be closed before replaying the same binding with fresh auth.
pub(crate) async fn authentication_rejected(attempt: &mut Option<ValueAttemptGuard>) {
    if let Some(attempt) = attempt.take() {
        attempt.mark_upstream_error();
        attempt.settle_now().await;
    }
}

pub(crate) fn mark_sent(attempt: &Option<ValueAttemptGuard>) {
    if let Some(attempt) = attempt {
        attempt.mark_sent();
    }
}

/// Retain the terminal guard even if the response body is never polled. A
/// streamed response may outlive its forwarding future by many minutes.
pub(crate) fn response(response: Response, attempt: Option<ValueAttemptGuard>) -> Response {
    let Some(attempt) = attempt else {
        return response;
    };
    let (parts, body) = response.into_parts();
    let stream = async_stream::stream! {
        let _attempt = attempt;
        let mut stream = body.into_data_stream();
        while let Some(frame) = stream.next().await { yield frame; }
    };
    Response::from_parts(parts, Body::from_stream(stream))
}
