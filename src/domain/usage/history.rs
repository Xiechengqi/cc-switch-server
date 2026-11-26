use anyhow::Result;

use super::store::UsageLog;
use crate::domain::providers::model::AppKind;

/// A bounded selection of durable request details. Accounting and reports
/// visit the durable history independently of the recent in-memory cache.
#[derive(Debug, Clone, Default)]
pub(crate) struct UsageSelection {
    pub from_ms: Option<u128>,
    pub to_ms: Option<u128>,
    pub app: Option<AppKind>,
    pub provider_id: Option<String>,
    pub share_id: Option<String>,
    pub user_email: Option<String>,
    pub is_health_check: Option<bool>,
    pub data_source: Option<String>,
    pub limit: Option<usize>,
    pub include_expired: bool,
    pub health_fingerprint: Option<String>,
    pub requested_model: Option<String>,
    pub data_source_prefix: Option<String>,
    pub pending_router_sync: bool,
    pub retry_before_ms: Option<u128>,
    pub exact_probe_identity: bool,
}

impl UsageSelection {
    pub(crate) fn matches(&self, log: &UsageLog) -> bool {
        self.from_ms.is_none_or(|from| log.created_at_ms >= from)
            && self.to_ms.is_none_or(|to| log.created_at_ms < to)
            && self.app.is_none_or(|app| log.app == app)
            && self
                .provider_id
                .as_deref()
                .is_none_or(|id| log.provider_id == id)
            && self
                .share_id
                .as_deref()
                .is_none_or(|id| log.share_id.as_deref() == Some(id))
            && self.user_email.as_deref().is_none_or(|email| {
                log.user_email
                    .as_deref()
                    .is_some_and(|user| user.eq_ignore_ascii_case(email))
            })
            && self
                .is_health_check
                .is_none_or(|health| log.is_health_check == health)
            && self
                .data_source
                .as_deref()
                .is_none_or(|source| log.data_source.as_deref() == Some(source))
            && self
                .health_fingerprint
                .as_deref()
                .is_none_or(|fingerprint| {
                    log.provider_health_fingerprint.as_deref() == Some(fingerprint)
                })
            && self
                .requested_model
                .as_deref()
                .is_none_or(|model| log.requested_model.as_deref() == Some(model))
            && self.data_source_prefix.as_deref().is_none_or(|prefix| {
                log.data_source
                    .as_deref()
                    .is_some_and(|source| source.starts_with(prefix))
            })
            && (!self.pending_router_sync
                || (log.router_last_synced_at_ms.is_none()
                    && log.is_user_inference()
                    && log.share_id.is_some()
                    && log.data_source.as_deref() == Some("router_share")
                    && !log.is_health_check))
            && self.retry_before_ms.is_none_or(|now| {
                log.router_next_sync_attempt_at_ms
                    .is_none_or(|next| next <= now)
            })
            && (!self.exact_probe_identity
                || (log.share_id == self.share_id
                    && log.data_source == self.data_source
                    && log.provider_health_fingerprint == self.health_fingerprint
                    && log.requested_model == self.requested_model))
    }
}

pub(crate) trait UsageHistory: std::fmt::Debug + Send + Sync {
    fn snapshot(&self) -> Result<std::sync::Arc<dyn UsageHistory>>;
    fn visit(
        &self,
        selection: &UsageSelection,
        visitor: &mut dyn FnMut(&UsageLog) -> Result<()>,
    ) -> Result<()>;

    fn request(&self, request_id: &str) -> Result<Option<UsageLog>>;
}
