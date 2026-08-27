use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::domain::providers::model::{AppKind, ProviderType};

use super::store::{UsageLog, UsageOutcome, UsageRecordKind, UsageState, UsageStore};

#[derive(Debug, Clone, Default)]
pub struct UsageQuery {
    pub from_ms: Option<u128>,
    pub to_ms: Option<u128>,
    pub app: Option<AppKind>,
    pub bundle_id: Option<String>,
    pub share_id: Option<String>,
    pub user_email: Option<String>,
    pub actual_model: Option<String>,
    pub outcome: Option<UsageOutcome>,
    pub usage_state: Option<UsageState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageCursorError {
    NotFound,
}

impl UsageQuery {
    fn matches(&self, log: &UsageLog) -> bool {
        log.record_kind != UsageRecordKind::HealthProbe
            && self.from_ms.is_none_or(|from| log.started_at_ms >= from)
            && self.to_ms.is_none_or(|to| log.started_at_ms < to)
            && self.app.is_none_or(|app| log.app == app)
            && self
                .bundle_id
                .as_deref()
                .is_none_or(|bundle_id| log.bundle_id == bundle_id)
            && self
                .share_id
                .as_deref()
                .is_none_or(|share_id| log.share_id.as_deref() == Some(share_id))
            && self.user_email.as_deref().is_none_or(|user_email| {
                log.user_email
                    .as_deref()
                    .is_some_and(|email| email.eq_ignore_ascii_case(user_email))
            })
            && self
                .actual_model
                .as_deref()
                .is_none_or(|model| effective_model(log) == model)
            && self.outcome.is_none_or(|outcome| log.outcome == outcome)
            && self
                .usage_state
                .is_none_or(|usage_state| log.usage_state == usage_state)
    }

    fn matches_user_request(&self, log: &UsageLog) -> bool {
        log.record_kind == UsageRecordKind::UserInference && self.matches(log)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageMetrics {
    pub request_count: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub pending_count: u64,
    pub processed_tokens: u64,
    pub fresh_input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub supplemental_tokens: u64,
    pub observed_usage_count: u64,
    pub missing_usage_count: u64,
    pub parse_error_usage_count: u64,
    pub interrupted_usage_count: u64,
    pub success_rate: f64,
    pub usage_coverage: f64,
    pub average_end_to_end_ms: Option<f64>,
    pub average_upstream_ms: Option<f64>,
    pub average_first_token_ms: Option<f64>,
    pub last_request_at_ms: Option<u128>,
}

#[derive(Debug, Clone, Default)]
struct MetricsAccumulator {
    metrics: UsageMetrics,
    end_to_end_sum_ms: u128,
    upstream_sum_ms: u128,
    completed_request_count: u64,
    usage_terminal_count: u64,
    first_token_sum_ms: u128,
    first_token_count: u64,
}

impl MetricsAccumulator {
    fn push(&mut self, log: &UsageLog) {
        let processed_tokens = log.processed_tokens();
        self.metrics.processed_tokens = self
            .metrics
            .processed_tokens
            .saturating_add(processed_tokens);
        self.metrics.fresh_input_tokens = self
            .metrics
            .fresh_input_tokens
            .saturating_add(log.input_tokens.unwrap_or(0));
        self.metrics.output_tokens = self
            .metrics
            .output_tokens
            .saturating_add(log.output_tokens.unwrap_or(0));
        self.metrics.cache_read_tokens = self
            .metrics
            .cache_read_tokens
            .saturating_add(log.cache_read_tokens.unwrap_or(0));
        self.metrics.cache_creation_tokens = self
            .metrics
            .cache_creation_tokens
            .saturating_add(log.cache_creation_tokens.unwrap_or(0));

        if log.record_kind == UsageRecordKind::InternalSupplemental {
            self.metrics.supplemental_tokens = self
                .metrics
                .supplemental_tokens
                .saturating_add(processed_tokens);
            return;
        }
        if log.record_kind != UsageRecordKind::UserInference {
            return;
        }

        self.metrics.request_count = self.metrics.request_count.saturating_add(1);
        match log.outcome {
            UsageOutcome::Success => {
                self.metrics.success_count = self.metrics.success_count.saturating_add(1)
            }
            UsageOutcome::Pending => {
                self.metrics.pending_count = self.metrics.pending_count.saturating_add(1)
            }
            _ => self.metrics.failure_count = self.metrics.failure_count.saturating_add(1),
        }
        match log.usage_state {
            UsageState::Observed => {
                self.metrics.observed_usage_count =
                    self.metrics.observed_usage_count.saturating_add(1);
                self.usage_terminal_count = self.usage_terminal_count.saturating_add(1);
            }
            UsageState::Missing => {
                self.metrics.missing_usage_count =
                    self.metrics.missing_usage_count.saturating_add(1);
                self.usage_terminal_count = self.usage_terminal_count.saturating_add(1);
            }
            UsageState::ParseError => {
                self.metrics.parse_error_usage_count =
                    self.metrics.parse_error_usage_count.saturating_add(1);
                self.usage_terminal_count = self.usage_terminal_count.saturating_add(1);
            }
            UsageState::Interrupted => {
                self.metrics.interrupted_usage_count =
                    self.metrics.interrupted_usage_count.saturating_add(1);
                self.usage_terminal_count = self.usage_terminal_count.saturating_add(1);
            }
            UsageState::NotApplicable => {
                self.usage_terminal_count = self.usage_terminal_count.saturating_add(1);
            }
            UsageState::Pending => {}
        }
        if log.completed_at_ms > 0 && log.outcome != UsageOutcome::Pending {
            self.end_to_end_sum_ms = self
                .end_to_end_sum_ms
                .saturating_add(log.end_to_end_duration_ms);
            self.upstream_sum_ms = self
                .upstream_sum_ms
                .saturating_add(log.upstream_duration_ms);
            self.completed_request_count = self.completed_request_count.saturating_add(1);
        }
        if let Some(first_token_ms) = log.first_token_ms {
            self.first_token_sum_ms = self.first_token_sum_ms.saturating_add(first_token_ms);
            self.first_token_count = self.first_token_count.saturating_add(1);
        }
        self.metrics.last_request_at_ms = Some(
            self.metrics
                .last_request_at_ms
                .map_or(log.started_at_ms, |last| last.max(log.started_at_ms)),
        );
    }

    fn finish(mut self) -> UsageMetrics {
        let terminal_outcome_count = self
            .metrics
            .success_count
            .saturating_add(self.metrics.failure_count);
        if terminal_outcome_count > 0 {
            self.metrics.success_rate =
                self.metrics.success_count as f64 / terminal_outcome_count as f64 * 100.0;
        }
        if self.usage_terminal_count > 0 {
            self.metrics.usage_coverage =
                self.metrics.observed_usage_count as f64 / self.usage_terminal_count as f64 * 100.0;
        }
        if self.completed_request_count > 0 {
            self.metrics.average_end_to_end_ms =
                Some(self.end_to_end_sum_ms as f64 / self.completed_request_count as f64);
            self.metrics.average_upstream_ms =
                Some(self.upstream_sum_ms as f64 / self.completed_request_count as f64);
        }
        if self.first_token_count > 0 {
            self.metrics.average_first_token_ms =
                Some(self.first_token_sum_ms as f64 / self.first_token_count as f64);
        }
        self.metrics
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceUsage {
    pub app: AppKind,
    pub metrics: UsageMetrics,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageOverview {
    pub metrics: UsageMetrics,
    pub surfaces: Vec<SurfaceUsage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTrendPoint {
    pub start_ms: u128,
    pub end_ms: u128,
    pub metrics: UsageMetrics,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleSurfaceUsage {
    pub app: AppKind,
    pub provider_id: String,
    pub provider_type: ProviderType,
    pub metrics: UsageMetrics,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBundleUsage {
    pub bundle_id: String,
    pub provider_name: String,
    pub family_id: Option<String>,
    pub supported_apps: Vec<AppKind>,
    pub metrics: UsageMetrics,
    pub surfaces: Vec<BundleSurfaceUsage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub app: AppKind,
    pub actual_model: String,
    pub requested_models: Vec<String>,
    pub metrics: UsageMetrics,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserUsage {
    pub user_email: String,
    pub metrics: UsageMetrics,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUsage {
    pub share_id: String,
    pub share_name: Option<String>,
    pub share_slug: Option<String>,
    pub metrics: UsageMetrics,
    pub users: Vec<ShareUserUsage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleFacet {
    pub bundle_id: String,
    pub provider_name: String,
    pub supported_apps: Vec<AppKind>,
    pub request_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareFacet {
    pub share_id: String,
    pub share_name: Option<String>,
    pub share_slug: Option<String>,
    pub request_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserFacet {
    pub user_email: String,
    pub request_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFacet {
    pub app: AppKind,
    pub actual_model: String,
    pub request_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueFacet {
    pub value: String,
    pub request_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageFacets {
    pub surfaces: Vec<ValueFacet>,
    pub bundles: Vec<BundleFacet>,
    pub shares: Vec<ShareFacet>,
    pub users: Vec<UserFacet>,
    pub models: Vec<ModelFacet>,
    pub outcomes: Vec<ValueFacet>,
    pub usage_states: Vec<ValueFacet>,
}

#[derive(Debug, Clone)]
struct BundleAccumulator {
    provider_name: String,
    family_id: Option<String>,
    metadata_at_ms: u128,
    supported_apps: BTreeSet<AppKind>,
    metrics: MetricsAccumulator,
    surfaces: BTreeMap<AppKind, (String, ProviderType, u128, MetricsAccumulator)>,
}

#[derive(Debug, Clone)]
struct ShareAccumulator {
    share_name: Option<String>,
    share_slug: Option<String>,
    metadata_at_ms: u128,
    metrics: MetricsAccumulator,
    users: BTreeMap<String, MetricsAccumulator>,
}

impl UsageStore {
    pub fn query_overview(&self, query: &UsageQuery) -> UsageOverview {
        let mut total = MetricsAccumulator::default();
        let mut surfaces = BTreeMap::<AppKind, MetricsAccumulator>::new();
        for log in self.logs.iter().filter(|log| query.matches(log)) {
            total.push(log);
            surfaces.entry(log.app).or_default().push(log);
        }
        UsageOverview {
            metrics: total.finish(),
            surfaces: surfaces
                .into_iter()
                .map(|(app, metrics)| SurfaceUsage {
                    app,
                    metrics: metrics.finish(),
                })
                .collect(),
        }
    }

    pub fn query_trends(&self, query: &UsageQuery, window_ms: u128) -> Vec<UsageTrendPoint> {
        let window_ms = window_ms.max(1);
        let mut buckets = BTreeMap::<u128, MetricsAccumulator>::new();
        for log in self.logs.iter().filter(|log| query.matches(log)) {
            let start_ms = log.started_at_ms - (log.started_at_ms % window_ms);
            buckets.entry(start_ms).or_default().push(log);
        }
        if !buckets.is_empty() {
            if let (Some(from_ms), Some(to_ms)) = (query.from_ms, query.to_ms) {
                let mut start_ms = from_ms - (from_ms % window_ms);
                while start_ms < to_ms {
                    buckets.entry(start_ms).or_default();
                    let next_ms = start_ms.saturating_add(window_ms);
                    if next_ms <= start_ms {
                        break;
                    }
                    start_ms = next_ms;
                }
            }
        }
        buckets
            .into_iter()
            .map(|(start_ms, metrics)| UsageTrendPoint {
                start_ms,
                end_ms: start_ms.saturating_add(window_ms),
                metrics: metrics.finish(),
            })
            .collect()
    }

    pub fn query_provider_bundles(&self, query: &UsageQuery) -> Vec<ProviderBundleUsage> {
        let mut groups = BTreeMap::<String, BundleAccumulator>::new();
        for log in self.logs.iter().filter(|log| query.matches(log)) {
            let group = groups
                .entry(log.bundle_id.clone())
                .or_insert_with(|| BundleAccumulator {
                    provider_name: log.provider_name.clone(),
                    family_id: log.family_id.clone(),
                    metadata_at_ms: log.started_at_ms,
                    supported_apps: log.supported_apps.iter().copied().collect(),
                    metrics: MetricsAccumulator::default(),
                    surfaces: BTreeMap::new(),
                });
            if log.started_at_ms >= group.metadata_at_ms {
                group.provider_name = log.provider_name.clone();
                group.family_id = log.family_id.clone();
                group.supported_apps = log.supported_apps.iter().copied().collect();
                group.metadata_at_ms = log.started_at_ms;
            }
            group.metrics.push(log);
            group
                .surfaces
                .entry(log.app)
                .or_insert_with(|| {
                    (
                        log.provider_id.clone(),
                        log.provider_type,
                        log.started_at_ms,
                        MetricsAccumulator::default(),
                    )
                })
                .3
                .push(log);
            if let Some(surface) = group.surfaces.get_mut(&log.app) {
                if log.started_at_ms >= surface.2 {
                    surface.0 = log.provider_id.clone();
                    surface.1 = log.provider_type;
                    surface.2 = log.started_at_ms;
                }
            }
        }
        let mut result = groups
            .into_iter()
            .map(|(bundle_id, group)| ProviderBundleUsage {
                bundle_id,
                provider_name: group.provider_name,
                family_id: group.family_id,
                supported_apps: group.supported_apps.into_iter().collect(),
                metrics: group.metrics.finish(),
                surfaces: group
                    .surfaces
                    .into_iter()
                    .map(
                        |(app, (provider_id, provider_type, _, metrics))| BundleSurfaceUsage {
                            app,
                            provider_id,
                            provider_type,
                            metrics: metrics.finish(),
                        },
                    )
                    .collect(),
            })
            .collect::<Vec<_>>();
        result.sort_by(|left, right| {
            right
                .metrics
                .processed_tokens
                .cmp(&left.metrics.processed_tokens)
                .then(right.metrics.request_count.cmp(&left.metrics.request_count))
                .then(left.bundle_id.cmp(&right.bundle_id))
        });
        result
    }

    pub fn query_models(&self, query: &UsageQuery) -> Vec<ModelUsage> {
        let mut groups =
            BTreeMap::<(AppKind, String), (BTreeSet<String>, MetricsAccumulator)>::new();
        for log in self.logs.iter().filter(|log| query.matches(log)) {
            let key = (log.app, effective_model(log).to_string());
            let group = groups.entry(key).or_default();
            if let Some(requested_model) = log
                .requested_model
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                group.0.insert(requested_model.to_string());
            }
            group.1.push(log);
        }
        let mut result = groups
            .into_iter()
            .map(
                |((app, actual_model), (requested_models, metrics))| ModelUsage {
                    app,
                    actual_model,
                    requested_models: requested_models.into_iter().collect(),
                    metrics: metrics.finish(),
                },
            )
            .collect::<Vec<_>>();
        result.sort_by(|left, right| {
            right
                .metrics
                .processed_tokens
                .cmp(&left.metrics.processed_tokens)
                .then(right.metrics.request_count.cmp(&left.metrics.request_count))
                .then(left.app.cmp(&right.app))
                .then(left.actual_model.cmp(&right.actual_model))
        });
        result
    }

    pub fn query_shares(&self, query: &UsageQuery) -> Vec<ShareUsage> {
        let mut groups = BTreeMap::<String, ShareAccumulator>::new();
        for log in self.logs.iter().filter(|log| query.matches(log)) {
            let Some(share_id) = log.share_id.as_deref() else {
                continue;
            };
            let group = groups
                .entry(share_id.to_string())
                .or_insert_with(|| ShareAccumulator {
                    share_name: log.share_name.clone(),
                    share_slug: log.share_slug.clone(),
                    metadata_at_ms: log.started_at_ms,
                    metrics: MetricsAccumulator::default(),
                    users: BTreeMap::new(),
                });
            if log.started_at_ms >= group.metadata_at_ms {
                group.share_name = log.share_name.clone();
                group.share_slug = log.share_slug.clone();
                group.metadata_at_ms = log.started_at_ms;
            }
            group.metrics.push(log);
            if let Some(email) = log.user_email.as_deref() {
                group.users.entry(email.to_string()).or_default().push(log);
            }
        }
        let mut result = groups
            .into_iter()
            .map(|(share_id, group)| {
                let mut users = group
                    .users
                    .into_iter()
                    .map(|(user_email, metrics)| ShareUserUsage {
                        user_email,
                        metrics: metrics.finish(),
                    })
                    .collect::<Vec<_>>();
                users.sort_by(|left, right| {
                    right
                        .metrics
                        .processed_tokens
                        .cmp(&left.metrics.processed_tokens)
                        .then(right.metrics.request_count.cmp(&left.metrics.request_count))
                        .then(left.user_email.cmp(&right.user_email))
                });
                ShareUsage {
                    share_id,
                    share_name: group.share_name,
                    share_slug: group.share_slug,
                    metrics: group.metrics.finish(),
                    users,
                }
            })
            .collect::<Vec<_>>();
        result.sort_by(|left, right| {
            right
                .metrics
                .processed_tokens
                .cmp(&left.metrics.processed_tokens)
                .then(right.metrics.request_count.cmp(&left.metrics.request_count))
                .then(left.share_id.cmp(&right.share_id))
        });
        result
    }

    pub fn query_facets(&self, query: &UsageQuery) -> UsageFacets {
        let mut surfaces = BTreeMap::<String, u64>::new();
        let mut bundles = BTreeMap::<String, (String, BTreeSet<AppKind>, u64, u128)>::new();
        let mut shares = BTreeMap::<String, (Option<String>, Option<String>, u64, u128)>::new();
        let mut users = BTreeMap::<String, u64>::new();
        let mut models = BTreeMap::<(AppKind, String), u64>::new();
        let mut outcomes = BTreeMap::<String, u64>::new();
        let mut usage_states = BTreeMap::<String, u64>::new();
        for log in self
            .logs
            .iter()
            .filter(|log| query.matches_user_request(log))
        {
            *surfaces.entry(log.app.as_str().to_string()).or_default() += 1;
            let bundle = bundles.entry(log.bundle_id.clone()).or_insert_with(|| {
                (
                    log.provider_name.clone(),
                    log.supported_apps.iter().copied().collect(),
                    0,
                    log.started_at_ms,
                )
            });
            bundle.2 = bundle.2.saturating_add(1);
            if log.started_at_ms >= bundle.3 {
                bundle.0 = log.provider_name.clone();
                bundle.1 = log.supported_apps.iter().copied().collect();
                bundle.3 = log.started_at_ms;
            }
            if let Some(share_id) = log.share_id.as_deref() {
                let share = shares.entry(share_id.to_string()).or_insert_with(|| {
                    (
                        log.share_name.clone(),
                        log.share_slug.clone(),
                        0,
                        log.started_at_ms,
                    )
                });
                share.2 = share.2.saturating_add(1);
                if log.started_at_ms >= share.3 {
                    share.0 = log.share_name.clone();
                    share.1 = log.share_slug.clone();
                    share.3 = log.started_at_ms;
                }
            }
            if let Some(email) = log.user_email.as_deref() {
                *users.entry(email.to_string()).or_default() += 1;
            }
            *models
                .entry((log.app, effective_model(log).to_string()))
                .or_default() += 1;
            *outcomes
                .entry(log.outcome.as_str().to_string())
                .or_default() += 1;
            *usage_states
                .entry(log.usage_state.as_str().to_string())
                .or_default() += 1;
        }
        UsageFacets {
            surfaces: value_facets(surfaces),
            bundles: bundles
                .into_iter()
                .map(
                    |(bundle_id, (provider_name, supported_apps, request_count, _))| BundleFacet {
                        bundle_id,
                        provider_name,
                        supported_apps: supported_apps.into_iter().collect(),
                        request_count,
                    },
                )
                .collect(),
            shares: shares
                .into_iter()
                .map(
                    |(share_id, (share_name, share_slug, request_count, _))| ShareFacet {
                        share_id,
                        share_name,
                        share_slug,
                        request_count,
                    },
                )
                .collect(),
            users: users
                .into_iter()
                .map(|(user_email, request_count)| UserFacet {
                    user_email,
                    request_count,
                })
                .collect(),
            models: models
                .into_iter()
                .map(|((app, actual_model), request_count)| ModelFacet {
                    app,
                    actual_model,
                    request_count,
                })
                .collect(),
            outcomes: value_facets(outcomes),
            usage_states: value_facets(usage_states),
        }
    }

    pub fn query_requests(
        &self,
        query: &UsageQuery,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<UsageLog>, Option<String>, usize), UsageCursorError> {
        let mut logs = self
            .logs
            .iter()
            .filter(|log| query.matches_user_request(log))
            .collect::<Vec<_>>();
        logs.sort_by(|left, right| {
            right
                .started_at_ms
                .cmp(&left.started_at_ms)
                .then(right.request_id.cmp(&left.request_id))
        });
        let total = logs.len();
        let offset = match cursor {
            Some(cursor) => logs
                .iter()
                .position(|log| log.request_id == cursor)
                .map(|index| index.saturating_add(1))
                .ok_or(UsageCursorError::NotFound)?,
            None => 0,
        };
        let selected = logs
            .into_iter()
            .skip(offset)
            .take(limit.saturating_add(1))
            .collect::<Vec<_>>();
        let has_more = selected.len() > limit;
        let items = selected
            .into_iter()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let next_cursor = has_more
            .then(|| items.last().map(|log| log.request_id.clone()))
            .flatten();
        Ok((items, next_cursor, total))
    }
}

fn effective_model(log: &UsageLog) -> &str {
    log.actual_model
        .as_deref()
        .filter(|value| !value.is_empty())
        .or(log
            .requested_model
            .as_deref()
            .filter(|value| !value.is_empty()))
        .or(log.model.as_deref().filter(|value| !value.is_empty()))
        .unwrap_or("unknown")
}

fn value_facets(values: BTreeMap<String, u64>) -> Vec<ValueFacet> {
    values
        .into_iter()
        .map(|(value, request_count)| ValueFacet {
            value,
            request_count,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::usage::store::{TokenUsage, UsageLogContext, UsageModelMetadata};

    fn usage_log(app: AppKind, request_id: &str, started_at_ms: u128) -> UsageLog {
        let mut log = UsageLog::new(
            app,
            "surface-provider".to_string(),
            "Bundle".to_string(),
            ProviderType::OpenRouter,
            200,
            20,
            UsageModelMetadata {
                actual_model: Some("actual-model".to_string()),
                ..UsageModelMetadata::default()
            },
            TokenUsage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                cache_read_tokens: Some(3),
                cache_creation_tokens: Some(2),
                ..TokenUsage::default()
            },
        );
        log.bundle_id = "bundle".to_string();
        log.supported_apps = vec![AppKind::Claude, AppKind::Codex];
        log.apply_context(UsageLogContext {
            request_id: Some(request_id.to_string()),
            started_at_ms: Some(started_at_ms),
            completed_at_ms: Some(started_at_ms + 20),
            ..UsageLogContext::default()
        });
        log
    }

    #[test]
    fn bundle_groups_surfaces_without_duplicating_requests() {
        let mut older = usage_log(AppKind::Claude, "claude-request", 100);
        older.provider_name = "Old Bundle".to_string();
        older.share_name = Some("Old Share".to_string());
        older.share_slug = Some("old-share".to_string());
        older.share_id = Some("share".to_string());
        let mut newer = usage_log(AppKind::Codex, "codex-request", 200);
        newer.provider_name = "Current Bundle".to_string();
        newer.supported_apps = vec![AppKind::Codex];
        newer.share_name = Some("Current Share".to_string());
        newer.share_slug = Some("current-share".to_string());
        newer.share_id = Some("share".to_string());
        let store = UsageStore {
            logs: vec![older, newer],
            ..UsageStore::default()
        };

        let bundles = store.query_provider_bundles(&UsageQuery::default());
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].provider_name, "Current Bundle");
        assert_eq!(bundles[0].supported_apps, vec![AppKind::Codex]);
        assert_eq!(bundles[0].metrics.request_count, 2);
        assert_eq!(bundles[0].surfaces.len(), 2);
        assert_eq!(bundles[0].metrics.processed_tokens, 40);
        let shares = store.query_shares(&UsageQuery::default());
        assert_eq!(shares[0].share_name.as_deref(), Some("Current Share"));
        assert_eq!(shares[0].share_slug.as_deref(), Some("current-share"));
        let facets = store.query_facets(&UsageQuery::default());
        assert_eq!(facets.bundles[0].provider_name, "Current Bundle");
        assert_eq!(facets.bundles[0].supported_apps, vec![AppKind::Codex]);
        assert_eq!(
            facets.shares[0].share_name.as_deref(),
            Some("Current Share")
        );
    }

    #[test]
    fn supplemental_usage_adds_tokens_without_adding_requests() {
        let request = usage_log(AppKind::Codex, "request", 100);
        let mut supplemental = usage_log(AppKind::Codex, "compact", 110);
        supplemental.record_kind = UsageRecordKind::InternalSupplemental;
        supplemental.parent_request_id = Some(request.request_id.clone());
        let store = UsageStore {
            logs: vec![request, supplemental],
            ..UsageStore::default()
        };

        let overview = store.query_overview(&UsageQuery::default());
        assert_eq!(overview.metrics.request_count, 1);
        assert_eq!(overview.metrics.processed_tokens, 40);
        assert_eq!(overview.metrics.supplemental_tokens, 20);
    }

    #[test]
    fn time_range_is_half_open() {
        let store = UsageStore {
            logs: vec![
                usage_log(AppKind::Claude, "inside", 100),
                usage_log(AppKind::Claude, "outside", 200),
            ],
            ..UsageStore::default()
        };
        let overview = store.query_overview(&UsageQuery {
            from_ms: Some(100),
            to_ms: Some(200),
            ..UsageQuery::default()
        });
        assert_eq!(overview.metrics.request_count, 1);
    }

    #[test]
    fn trends_preserve_empty_windows_between_requests() {
        let store = UsageStore {
            logs: vec![
                usage_log(AppKind::Claude, "first", 100),
                usage_log(AppKind::Claude, "last", 250),
            ],
            ..UsageStore::default()
        };
        let query = UsageQuery {
            from_ms: Some(100),
            to_ms: Some(300),
            ..UsageQuery::default()
        };

        let trends = store.query_trends(&query, 50);

        assert_eq!(
            trends
                .iter()
                .map(|point| (point.start_ms, point.metrics.request_count))
                .collect::<Vec<_>>(),
            vec![(100, 1), (150, 0), (200, 0), (250, 1)]
        );
    }

    #[test]
    fn pending_requests_do_not_dilute_terminal_latency_or_usage_coverage() {
        let completed = usage_log(AppKind::Codex, "completed", 100);
        let mut pending = usage_log(AppKind::Codex, "pending", 200);
        pending.completed_at_ms = 0;
        pending.end_to_end_duration_ms = 0;
        pending.upstream_duration_ms = 0;
        pending.outcome = UsageOutcome::Pending;
        pending.usage_state = UsageState::Pending;
        pending.input_tokens = None;
        pending.output_tokens = None;
        pending.cache_read_tokens = None;
        pending.cache_creation_tokens = None;
        let store = UsageStore {
            logs: vec![completed, pending],
            ..UsageStore::default()
        };

        let overview = store.query_overview(&UsageQuery::default());

        assert_eq!(overview.metrics.request_count, 2);
        assert_eq!(overview.metrics.pending_count, 1);
        assert_eq!(overview.metrics.success_rate, 100.0);
        assert_eq!(overview.metrics.average_end_to_end_ms, Some(20.0));
        assert_eq!(overview.metrics.average_upstream_ms, Some(20.0));
        assert_eq!(overview.metrics.usage_coverage, 100.0);
    }

    #[test]
    fn request_pagination_rejects_a_cursor_outside_the_filtered_range() {
        let store = UsageStore {
            logs: vec![usage_log(AppKind::Codex, "request", 100)],
            ..UsageStore::default()
        };

        assert!(matches!(
            store.query_requests(&UsageQuery::default(), Some("missing"), 50),
            Err(UsageCursorError::NotFound)
        ));
    }
}
