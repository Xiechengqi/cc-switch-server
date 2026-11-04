//! Account-scoped opt-in maintenance. Storage transactions never span HTTP.
use super::*;
use crate::clients::oauth::codebuddy::{query_checkin_status, submit_checkin, CheckinFailure};
use crate::domain::accounts::checkin::{
    self, AccountCheckin, CheckinOutcome, CheckinStatus, CheckinView,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum CheckinControlError {
    #[error("account not found")]
    NotFound,
    #[error("account or check-in policy changed; reload and retry")]
    Conflict,
    #[error("daily check-in supports only China-region personal WorkBuddy accounts")]
    Unsupported,
    #[error("account check-in is busy or waiting for recovery")]
    Busy,
    #[error("account check-in is unavailable during shutdown or degraded persistence")]
    Unavailable,
}

#[derive(Clone)]
struct CheckinFence {
    account: Account,
    incarnation: u64,
}

impl CheckinFence {
    fn current<'a>(&self, store: &'a AccountStore) -> anyhow::Result<&'a Account> {
        let a = store
            .accounts
            .iter()
            .find(|a| a.id == self.account.id)
            .ok_or(CheckinControlError::Conflict)?;
        anyhow::ensure!(
            store.account_incarnation(&a.id) == Some(self.incarnation)
                && a.provider_type == self.account.provider_type
                && a.auth_identity_generation == self.account.auth_identity_generation
                && crate::domain::codebuddy::CodeBuddyAccountProfile::parse_for(
                    a.provider_type,
                    a.profile.as_ref()
                )
                .ok()
                .map(|p| p.domain)
                    == crate::domain::codebuddy::CodeBuddyAccountProfile::parse_for(
                        self.account.provider_type,
                        self.account.profile.as_ref()
                    )
                    .ok()
                    .map(|p| p.domain)
                && checkin::principal(a) == checkin::principal(&self.account),
            CheckinControlError::Conflict
        );
        Ok(a)
    }
}

impl ServerStateInner {
    pub(crate) async fn account_checkin_view(
        &self,
        id: &str,
        generation: u64,
    ) -> anyhow::Result<CheckinView> {
        let accounts = self.accounts.read().await;
        let account = accounts
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or(CheckinControlError::NotFound)?;
        anyhow::ensure!(
            account.auth_identity_generation == generation,
            CheckinControlError::Conflict
        );
        Ok(CheckinView::new(
            account,
            accounts.buddy_checkins.get(id),
            now_ms_i64(),
        ))
    }

    pub(crate) async fn set_account_checkin_policy(
        &self,
        id: &str,
        generation: u64,
        revision: u64,
        enabled: bool,
        time: &str,
    ) -> anyhow::Result<CheckinView> {
        anyhow::ensure!(
            checkin::valid_time(time),
            "check-in time must be HH:mm (Asia/Shanghai)"
        );
        let now = now_ms_i64();
        self.try_mutate_accounts_immediate(|store| {
            let account = store
                .accounts
                .iter()
                .find(|a| a.id == id)
                .cloned()
                .ok_or(CheckinControlError::NotFound)?;
            anyhow::ensure!(
                account.auth_identity_generation == generation,
                CheckinControlError::Conflict
            );
            let principal = checkin::principal(&account).ok_or(CheckinControlError::Unsupported)?;
            let record = store.buddy_checkins.entry(id.into()).or_default();
            if !record.matches(&account) {
                *record = AccountCheckin {
                    principal,
                    ..Default::default()
                };
            }
            anyhow::ensure!(record.revision == revision, CheckinControlError::Conflict);
            record.enabled = enabled;
            record.time = time.into();
            record.revision = record.revision.saturating_add(1);
            Ok(CheckinView::new(&account, Some(record), now))
        })
        .await?
    }

    /// The owned task persists its receipt even if an HTTP caller disconnects.
    pub(crate) async fn run_account_checkin(
        self: &Arc<Self>,
        id: String,
        generation: u64,
    ) -> anyhow::Result<CheckinView> {
        let state = self.clone();
        tokio::spawn(async move { state.execute_account_checkin(&id, generation, false).await })
            .await?
    }

    async fn checkin_fence(&self, id: &str, generation: u64) -> anyhow::Result<CheckinFence> {
        let accounts = self.accounts.read().await;
        let account = accounts
            .accounts
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or(CheckinControlError::NotFound)?;
        anyhow::ensure!(
            account.auth_identity_generation == generation,
            CheckinControlError::Conflict
        );
        anyhow::ensure!(
            checkin::unsupported_reason(&account).is_none(),
            CheckinControlError::Unsupported
        );
        Ok(CheckinFence {
            incarnation: accounts
                .account_incarnation(id)
                .ok_or(CheckinControlError::Conflict)?,
            account,
        })
    }

    async fn checkin_file_lock(&self, id: &str) -> anyhow::Result<std::fs::File> {
        let directory = self.config_dir.join(".workbuddy-checkin-locks");
        let path = directory.join(format!(
            "{}.lock",
            hex::encode(Sha256::digest(id.as_bytes()))
        ));
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(directory)?;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)?;
            fs2::FileExt::try_lock_exclusive(&file).map_err(|_| CheckinControlError::Busy)?;
            Ok(file)
        })
        .await?
    }

    async fn execute_account_checkin(
        self: &Arc<Self>,
        id: &str,
        generation: u64,
        automatic: bool,
    ) -> anyhow::Result<CheckinView> {
        anyhow::ensure!(
            !self.is_shutting_down() && !self.credential_persistence_degraded(),
            CheckinControlError::Unavailable
        );
        let _operation_lock = self
            .buddy_checkin_locks
            .try_lock(ProviderType::WorkBuddyOAuth, id)
            .ok_or(CheckinControlError::Busy)?;
        let _file_lock = self.checkin_file_lock(id).await?;
        let fence = self.checkin_fence(id, generation).await?;
        let now = now_ms_i64();
        let (proceed, recovering, revision) = self
            .try_mutate_accounts_immediate(|store| {
                let account = fence.current(store)?.clone();
                let record =
                    store
                        .buddy_checkins
                        .entry(id.into())
                        .or_insert_with(|| AccountCheckin {
                            principal: checkin::principal(&account).unwrap_or_default(),
                            ..Default::default()
                        });
                anyhow::ensure!(record.matches(&account), CheckinControlError::Conflict);
                if record.confirmed_today(now) {
                    return Ok((false, false, record.revision));
                }
                if automatic && !record.due(&account, now) {
                    return Ok((false, false, record.revision));
                }
                // A crashed/uncertain submission must respect its persisted lease,
                // including Retry-After, even for repeated manual clicks.
                anyhow::ensure!(
                    record.next_retry_at.is_none_or(|at| at <= now)
                        && (record.attempt_day != checkin::local_day(now)
                            || record.attempts < checkin::CHECKIN_DAILY_ATTEMPTS),
                    CheckinControlError::Busy
                );
                let recovering = record.pending;
                record.begin(now);
                Ok((true, recovering, record.revision))
            })
            .await??;
        if proceed {
            let mut result = self.checkin_network(&fence, automatic, revision).await;
            if recovering {
                if let Err(error) = &mut result {
                    if error.status == CheckinStatus::RetryableError {
                        error.status = CheckinStatus::Unknown;
                    }
                }
            }
            let finished = now_ms_i64();
            // The protocol has no date-bound receipt. Never confirm another day
            // if the request crossed China midnight.
            if checkin::local_day(now) != checkin::local_day(finished) {
                result = Err(CheckinFailure::new(CheckinStatus::Unknown, "day_changed"));
            }
            let (status, message, delay) = match result {
                Ok(status) => (
                    status,
                    if status == CheckinStatus::Already {
                        "already_checked_in"
                    } else {
                        "checked_in"
                    },
                    None,
                ),
                Err(e) => (e.status, e.message, e.retry_after_ms),
            };
            self.try_mutate_accounts_immediate(|store| {
                fence.current(store)?;
                let record = store
                    .buddy_checkins
                    .get_mut(id)
                    .ok_or(CheckinControlError::Conflict)?;
                record.finish(
                    CheckinOutcome {
                        status,
                        at_ms: finished,
                        message: message.into(),
                    },
                    delay,
                );
                Ok::<_, anyhow::Error>(())
            })
            .await??;
        }
        // Quota failures do not turn a confirmed check-in into a failed sign-in.
        if let Err(error) = self.refresh_checkin_resources(&fence).await {
            tracing::debug!(account_id = %id, error = %error, "WorkBuddy check-in resource refresh deferred");
        }
        self.account_checkin_view(id, generation).await
    }

    async fn checkin_network(
        self: &Arc<Self>,
        fence: &CheckinFence,
        automatic: bool,
        revision: u64,
    ) -> Result<CheckinStatus, CheckinFailure> {
        let auth_error = || {
            CheckinFailure::new(
                CheckinStatus::AuthenticationRequired,
                "authentication_required",
            )
        };
        self.refresh_managed_account_if_needed_for_generation(
            ProviderType::WorkBuddyOAuth,
            &fence.account.id,
            fence.account.auth_identity_generation,
        )
        .await
        .map_err(checkin_refresh_failure)?;
        for attempt in 0..2 {
            let guard = self
                .account_refresh_locks
                .lock(ProviderType::WorkBuddyOAuth, &fence.account.id)
                .await;
            let account = {
                let accounts = self.accounts.read().await;
                fence
                    .current(&accounts)
                    .map_err(|_| CheckinFailure::new(CheckinStatus::Error, "account_changed"))?
                    .clone()
            };
            if account.needs_relogin {
                return Err(auth_error());
            }
            let http = self.http_client().await;
            let result = match query_checkin_status(&http, &account).await {
                Ok(true) => Ok(CheckinStatus::Already),
                Ok(false) => {
                    let accounts = self.accounts.read().await;
                    let current = fence.current(&accounts).map_err(|_| {
                        CheckinFailure::new(CheckinStatus::Error, "account_changed")
                    })?;
                    if !native_refresh_snapshot_matches(current, &account) {
                        return Err(CheckinFailure::new(
                            CheckinStatus::RetryableError,
                            "credentials_changed",
                        ));
                    }
                    let allowed = !self.is_shutting_down()
                        && !self.credential_persistence_degraded()
                        && (!automatic
                            || accounts
                                .buddy_checkins
                                .get(&account.id)
                                .is_some_and(|r| r.enabled && r.revision == revision));
                    drop(accounts);
                    if !allowed {
                        return Err(CheckinFailure::new(
                            CheckinStatus::Error,
                            "operation_cancelled",
                        ));
                    }
                    submit_checkin(&http, &account).await
                }
                Err(e) => Err(e),
            };
            if result
                .as_ref()
                .is_err_and(|e| e.status == CheckinStatus::AuthenticationRequired)
                && attempt == 0
            {
                guard.release();
                self.refresh_managed_account_now_for_generation(
                    ProviderType::WorkBuddyOAuth,
                    &account.id,
                    account.auth_identity_generation,
                )
                .await
                .map_err(checkin_refresh_failure)?;
                // Always query again after auth recovery, never blindly re-POST.
                continue;
            }
            return result;
        }
        Err(auth_error())
    }

    async fn refresh_checkin_resources(
        self: &Arc<Self>,
        fence: &CheckinFence,
    ) -> anyhow::Result<()> {
        let now = now_ms_i64();
        let needed = {
            let accounts = self.accounts.read().await;
            fence.current(&accounts)?;
            accounts
                .buddy_checkins
                .get(&fence.account.id)
                .is_some_and(|r| {
                    r.quota_pending
                        && r.quota_attempts < 6
                        && r.quota_retry_at.is_some_and(|at| at <= now)
                })
        };
        if !needed || self.is_shutting_down() || self.credential_persistence_degraded() {
            return Ok(());
        }
        // Persist the independent quota retry budget before refresh/network,
        // so crashes and token refresh failures cannot create a tight loop.
        self.try_mutate_accounts_immediate(|store| {
            fence.current(store)?;
            let record = store
                .buddy_checkins
                .get_mut(&fence.account.id)
                .ok_or(CheckinControlError::Conflict)?;
            record.quota_attempts = record.quota_attempts.saturating_add(1);
            record.quota_retry_at =
                (record.quota_attempts < 6).then_some(now.saturating_add(15 * 60_000));
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        self.refresh_managed_account_if_needed_for_generation(
            ProviderType::WorkBuddyOAuth,
            &fence.account.id,
            fence.account.auth_identity_generation,
        )
        .await
        .map_err(|_| anyhow::anyhow!("WorkBuddy resource refresh authentication unavailable"))?;
        let _guard = self
            .account_refresh_locks
            .lock(ProviderType::WorkBuddyOAuth, &fence.account.id)
            .await;
        let account = {
            let accounts = self.accounts.read().await;
            fence.current(&accounts)?.clone()
        };
        let http = self.http_client().await;
        let timeout =
            Duration::from_millis(self.oauth_quota_refresh_timeout_ms().await.max(1_000) as u64);
        let result = crate::clients::oauth::quota::refresh_buddy_resources_only(
            &http, &account, now, timeout,
        )
        .await;
        let updated = self
            .try_mutate_accounts_immediate(|store| {
                fence.current(store)?;
                let current = store
                    .accounts
                    .iter_mut()
                    .find(|a| a.id == account.id)
                    .ok_or(CheckinControlError::Conflict)?;
                anyhow::ensure!(
                    quota_refresh_snapshot_matches(current, &account),
                    CheckinControlError::Conflict
                );
                let record = store
                    .buddy_checkins
                    .get_mut(&account.id)
                    .ok_or(CheckinControlError::Conflict)?;
                match result {
                    Ok(update) => {
                        // Only package fields: do not clear inference cooldown,
                        // authentication failures or unrelated refresh diagnostics.
                        current.quota = update.quota;
                        current.quota_percent = update.quota_percent;
                        current.quota_refreshed_at = update.quota_refreshed_at;
                        record.quota_pending = false;
                        record.quota_retry_at = None;
                        Ok(Some(current.clone()))
                    }
                    Err(_) => {
                        record.quota_retry_at =
                            (record.quota_attempts < 6).then_some(now.saturating_add(15 * 60_000));
                        Ok(None)
                    }
                }
            })
            .await??;
        if let Some(updated) = updated {
            self.refresh_account_runtime_metadata_if_changed(&account, &updated)
                .await?;
            emit_oauth_quota_updated(self, &updated, true);
        }
        Ok(())
    }
}

fn checkin_refresh_failure(error: ManagedAccountRefreshError) -> CheckinFailure {
    match error {
        ManagedAccountRefreshError::Refresh {
            status_code: 408 | 429 | 500..=599,
            retry_after_ms,
            ..
        } => {
            let mut failure =
                CheckinFailure::new(CheckinStatus::RetryableError, "oauth_refresh_unavailable");
            failure.retry_after_ms = retry_after_ms;
            failure
        }
        ManagedAccountRefreshError::CredentialPersistenceDegraded => {
            CheckinFailure::new(CheckinStatus::RetryableError, "persistence_unavailable")
        }
        _ => CheckinFailure::new(
            CheckinStatus::AuthenticationRequired,
            "authentication_required",
        ),
    }
}

#[cfg(test)]
mod tests;

pub fn spawn_account_checkin(state: ServerState) {
    tokio::spawn(async move {
        let mut shutdown = state.subscribe_shutdown();
        loop {
            if *shutdown.borrow() {
                break;
            }
            let now = now_ms_i64();
            let (due, quotas) = {
                let store = state.accounts.read().await;
                let due = store
                    .accounts
                    .iter()
                    .filter(|a| {
                        store
                            .buddy_checkins
                            .get(&a.id)
                            .is_some_and(|r| r.due(a, now))
                    })
                    .take(16)
                    .map(|a| (a.id.clone(), a.auth_identity_generation))
                    .collect::<Vec<_>>();
                let quotas = store
                    .accounts
                    .iter()
                    .filter(|a| {
                        store.buddy_checkins.get(&a.id).is_some_and(|r| {
                            r.matches(a)
                                && r.quota_pending
                                && r.quota_attempts < 6
                                && r.quota_retry_at.is_some_and(|at| at <= now)
                        })
                    })
                    .take(16)
                    .map(|a| (a.id.clone(), a.auth_identity_generation))
                    .collect::<Vec<_>>();
                (due, quotas)
            };
            for (id, generation) in due {
                if state.is_shutting_down() {
                    return;
                }
                if let Err(error) = state.execute_account_checkin(&id, generation, true).await {
                    tracing::debug!(account_id = %id, error = %error, "WorkBuddy automatic check-in deferred");
                }
            }
            for (id, generation) in quotas {
                if state.is_shutting_down() {
                    return;
                }
                if let Some(_guard) = state
                    .buddy_checkin_locks
                    .try_lock(ProviderType::WorkBuddyOAuth, &id)
                {
                    if let Ok(fence) = state.checkin_fence(&id, generation).await {
                        let _ = state.refresh_checkin_resources(&fence).await;
                    }
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(60)) => {},
                _ = shutdown.changed() => break,
            }
        }
    });
}
