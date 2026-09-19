use super::*;

pub fn spawn_periodic_backups(state: ServerState) {
    tokio::spawn(async move {
        loop {
            let policy = match state.prune_backups_to_configured_retention().await {
                Ok((policy, removed)) => {
                    if removed > 0 {
                        tracing::info!(
                            removed,
                            retain_count = policy.retain_count,
                            "pruned backups to configured retention"
                        );
                    }
                    policy
                }
                Err(error) => {
                    let policy = state.configured_backup_policy().await;
                    tracing::warn!(
                        error = %error,
                        retain_count = policy.retain_count,
                        "failed to enforce configured backup retention"
                    );
                    policy
                }
            };

            if policy.interval_hours == 0 {
                state.periodic_backup_wakeup.notified().await;
                continue;
            }

            let interval = Duration::from_secs(policy.interval_hours * 60 * 60);
            let elapsed = tokio::select! {
                _ = sleep(interval) => true,
                _ = state.periodic_backup_wakeup.notified() => false,
            };
            if !elapsed {
                continue;
            }

            match state
                .create_consistent_backup_if_policy_current(
                    Some("periodic".to_string()),
                    Some(policy),
                )
                .await
            {
                Ok(Some(manifest)) => {
                    state.emit_event(
                        ServerEvent::new("backup.created", "backup")
                            .id(manifest.id)
                            .message("periodic"),
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(error = %error, "periodic backup failed");
                }
            }
        }
    });
}

pub fn spawn_auto_upgrade_scheduler(state: ServerState) {
    tokio::spawn(async move {
        loop {
            let (enabled, interval_minutes) = {
                let config = state.config.read().await;
                (
                    config.upgrade_policy.auto_upgrade_enabled,
                    config
                        .upgrade_policy
                        .auto_upgrade_check_interval_minutes
                        .max(5),
                )
            };
            if enabled {
                if let Err(error) = run_auto_upgrade_tick(&state).await {
                    tracing::warn!(error = %error, "auto upgrade tick failed");
                }
                sleep(Duration::from_secs(interval_minutes * 60)).await;
            } else {
                sleep(Duration::from_secs(60)).await;
            }
        }
    });
}

async fn run_auto_upgrade_tick(state: &ServerState) -> anyhow::Result<()> {
    if let Err(error) = report_installation_upgrade_status(state).await {
        tracing::debug!(error = %error, "auto upgrade status report failed");
    }
    if state.upgrade.is_restart_pending().await {
        return Ok(());
    }
    if let Some(handle) = state.upgrade.current().await {
        if matches!(
            *handle.status.lock().await,
            crate::self_update::upgrade::UpgradeStatus::Running
        ) {
            return Ok(());
        }
    }
    let client = state.http_client().await;
    let latest = crate::self_update::version::fetch_latest_release_meta(&client).await;
    if !latest.update_available {
        return Ok(());
    }
    if crate::self_update::version::ensure_binary_writable().is_err() {
        return Ok(());
    }
    let client = crate::infra::http::direct_client_builder()
        .user_agent("cc-switch-server/0.1 auto-upgrade")
        .build()
        .context("build auto-upgrade client")?;
    state
        .upgrade
        .start(
            client,
            Some("auto-upgrade".to_string()),
            true,
            false,
            state.bind_addr,
        )
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(())
}

pub fn spawn_periodic_installation_status_report(state: ServerState) {
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(60 * 60)).await;
            if let Err(error) = report_installation_upgrade_status(&state).await {
                tracing::debug!(error = %error, "periodic installation status report failed");
            }
        }
    });
}

const UPGRADE_TASK_REPORT_POLL_INTERVAL: Duration = Duration::from_secs(2);
const UPGRADE_TASK_RUNNING_REPORT_INTERVAL: Duration = Duration::from_secs(15);
const UPGRADE_TASK_REPORT_WARNING_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub fn spawn_installation_upgrade_task_reporter(state: ServerState) {
    tokio::spawn(async move {
        let mut shutdown = state.subscribe_shutdown();
        let mut last_reported_fingerprint = None::<String>;
        let mut last_running_report = None::<Instant>;
        let mut consecutive_failures = 0_u64;
        let mut last_failure_warning = None::<Instant>;
        loop {
            if *shutdown.borrow() {
                return;
            }
            if let Some(snapshot) = state.upgrade.status_snapshot().await {
                let fingerprint = installation_upgrade_task_fingerprint(&snapshot);
                let running = matches!(
                    snapshot.status,
                    crate::self_update::upgrade::UpgradeStatus::Running
                );
                let running_report_due = running
                    && last_running_report.is_none_or(|reported| {
                        reported.elapsed() >= UPGRADE_TASK_RUNNING_REPORT_INTERVAL
                    });
                if last_reported_fingerprint.as_deref() != Some(fingerprint.as_str())
                    || running_report_due
                {
                    let report_task_id = snapshot.task_id.clone();
                    let report_status = snapshot.status;
                    let config = state.config_snapshot().await;
                    if config.has_registered_router_identity() {
                        let http = state.http_client().await;
                        match client::report_installation_upgrade_task(&http, &config, snapshot)
                            .await
                        {
                            Ok(()) => {
                                crate::metrics::record_router_upgrade_task_report("success");
                                last_reported_fingerprint = Some(fingerprint);
                                consecutive_failures = 0;
                                last_failure_warning = None;
                                if running {
                                    last_running_report = Some(Instant::now());
                                }
                            }
                            Err(error) => {
                                crate::metrics::record_router_upgrade_task_report("failure");
                                consecutive_failures = consecutive_failures.saturating_add(1);
                                let warning_due = last_failure_warning.is_none_or(|warning| {
                                    warning.elapsed() >= UPGRADE_TASK_REPORT_WARNING_INTERVAL
                                });
                                if warning_due {
                                    tracing::warn!(
                                        %error,
                                        consecutive_failures,
                                        task_id = %report_task_id,
                                        status = ?report_status,
                                        "installation upgrade task report failed; retrying"
                                    );
                                    last_failure_warning = Some(Instant::now());
                                } else {
                                    tracing::debug!(
                                        %error,
                                        consecutive_failures,
                                        "installation upgrade task report failed; retrying"
                                    );
                                }
                            }
                        }
                    }
                }
            }
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
                _ = sleep(UPGRADE_TASK_REPORT_POLL_INTERVAL) => {}
            }
        }
    });
}

pub(super) fn installation_upgrade_task_fingerprint(
    snapshot: &crate::self_update::upgrade::UpgradeStatusSnapshot,
) -> String {
    let mut snapshot = snapshot.clone();
    snapshot.updated_at.clear();
    let bytes = serde_json::to_vec(&snapshot).unwrap_or_default();
    hex::encode(Sha256::digest(bytes))
}

const DEFAULT_ROUTER_HEARTBEAT_INTERVAL_SECS: u64 = 60;
const MIN_ROUTER_HEARTBEAT_INTERVAL_SECS: u64 = 15;
const MAX_ROUTER_HEARTBEAT_INTERVAL_SECS: u64 = 60;
const ROUTER_HEARTBEAT_UNREGISTERED_RETRY_SECS: u64 = 5;
const ROUTER_HEARTBEAT_UNREGISTERED_MAX_RETRY_SECS: u64 = 5 * 60;
const ROUTER_HEARTBEAT_WARNING_INTERVAL: Duration = Duration::from_secs(15 * 60);
const PUBLIC_IP_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const PUBLIC_IP_RETRY_INITIAL: Duration = Duration::from_secs(30);
const PUBLIC_IP_RETRY_MAX: Duration = Duration::from_secs(15 * 60);
pub(super) const ROUTER_HEARTBEAT_SUSTAINED_FAILURES: u32 = 3;
pub(super) const ROUTER_HEARTBEAT_STATE_ERROR_MAX_CHARS: usize = 2 * 1024;
const AUDIT_UPLOAD_IDLE_INTERVAL: Duration = Duration::from_secs(1);
const AUDIT_UPLOAD_DISABLED_INTERVAL: Duration = Duration::from_secs(5);
const AUDIT_UPLOAD_RETRY_INITIAL: Duration = Duration::from_secs(1);
const AUDIT_UPLOAD_RETRY_MAX: Duration = Duration::from_secs(60);

pub fn spawn_audit_log_uploader(state: ServerState) {
    tokio::spawn(async move {
        let mut retry_delay = AUDIT_UPLOAD_RETRY_INITIAL;
        let mut last_failure = None::<String>;
        let mut cursor_loss_reported = None::<AuditCursor>;
        loop {
            if !state.audit_log().is_enabled() {
                retry_delay = AUDIT_UPLOAD_RETRY_INITIAL;
                last_failure = None;
                sleep(AUDIT_UPLOAD_DISABLED_INTERVAL).await;
                continue;
            }

            let config = state.config_snapshot().await;
            let Some(router_api_base) = config
                .router_api_base()
                .map(|value| value.trim().trim_end_matches('/').to_string())
                .filter(|value| !value.is_empty())
            else {
                sleep(AUDIT_UPLOAD_DISABLED_INTERVAL).await;
                continue;
            };
            let Some(installation_id) = config
                .registered_router_identity()
                .map(|identity| identity.installation_id.trim().to_string())
                .filter(|value| !value.is_empty())
            else {
                sleep(AUDIT_UPLOAD_DISABLED_INTERVAL).await;
                continue;
            };

            let audit = state.audit_log();
            let loaded_cursor = match tokio::task::spawn_blocking({
                let audit = audit.clone();
                move || audit.load_upload_cursor()
            })
            .await
            {
                Ok(Ok(cursor)) => cursor,
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::InvalidData => {
                    let cursor_error = error.to_string();
                    match fence_audit_upload_destination(
                        audit.clone(),
                        router_api_base.clone(),
                        installation_id.clone(),
                    )
                    .await
                    {
                        Ok(latest_cursor) => {
                            tracing::warn!(
                                target: "cc_switch_server::audit_upload",
                                error = %cursor_error,
                                installation_ref = %opaque_ref("installation", &installation_id),
                                fenced_through_boot_id = latest_cursor.as_ref().map(|cursor| cursor.boot_id.as_str()).unwrap_or("-"),
                                fenced_through_sequence = latest_cursor.as_ref().map(|cursor| cursor.sequence).unwrap_or(0),
                                "invalid audit upload cursor was replaced; retained events were fenced to prevent cross-Client disclosure"
                            );
                            retry_delay = AUDIT_UPLOAD_RETRY_INITIAL;
                            last_failure = None;
                            cursor_loss_reported = None;
                            continue;
                        }
                        Err(fence_error) => {
                            audit_upload_failure(
                                &audit,
                                &mut last_failure,
                                "cursor_recovery",
                                &format!("{cursor_error}; recovery failed: {fence_error}"),
                                retry_delay,
                            );
                            sleep(retry_delay).await;
                            retry_delay = retry_delay.saturating_mul(2).min(AUDIT_UPLOAD_RETRY_MAX);
                            continue;
                        }
                    }
                }
                Ok(Err(error)) => {
                    audit_upload_failure(
                        &audit,
                        &mut last_failure,
                        "cursor_read",
                        &error.to_string(),
                        retry_delay,
                    );
                    sleep(retry_delay).await;
                    retry_delay = retry_delay.saturating_mul(2).min(AUDIT_UPLOAD_RETRY_MAX);
                    continue;
                }
                Err(error) => {
                    audit_upload_failure(
                        &audit,
                        &mut last_failure,
                        "cursor_task",
                        &error.to_string(),
                        retry_delay,
                    );
                    sleep(retry_delay).await;
                    retry_delay = retry_delay.saturating_mul(2).min(AUDIT_UPLOAD_RETRY_MAX);
                    continue;
                }
            };
            if loaded_cursor.is_none() {
                match fence_audit_upload_destination(
                    audit.clone(),
                    router_api_base.clone(),
                    installation_id.clone(),
                )
                .await
                {
                    Ok(latest_cursor) => {
                        tracing::warn!(
                            target: "cc_switch_server::audit_upload",
                            installation_ref = %opaque_ref("installation", &installation_id),
                            fenced_through_boot_id = latest_cursor.as_ref().map(|cursor| cursor.boot_id.as_str()).unwrap_or("-"),
                            fenced_through_sequence = latest_cursor.as_ref().map(|cursor| cursor.sequence).unwrap_or(0),
                            "audit upload cursor was initialized; retained events were fenced because their destination identity could not be proven"
                        );
                        retry_delay = AUDIT_UPLOAD_RETRY_INITIAL;
                        last_failure = None;
                        cursor_loss_reported = None;
                        continue;
                    }
                    Err(error) => {
                        audit_upload_failure(
                            &audit,
                            &mut last_failure,
                            "cursor_initialize",
                            &error,
                            retry_delay,
                        );
                        sleep(retry_delay).await;
                        retry_delay = retry_delay.saturating_mul(2).min(AUDIT_UPLOAD_RETRY_MAX);
                        continue;
                    }
                }
            }
            if loaded_cursor.as_ref().is_some_and(|cursor| {
                !cursor.targets_destination(&router_api_base, &installation_id)
            }) {
                match fence_audit_upload_destination(
                    audit.clone(),
                    router_api_base.clone(),
                    installation_id.clone(),
                )
                .await
                {
                    Ok(latest_cursor) => {
                        let previous_installation_ref = loaded_cursor
                            .as_ref()
                            .map(|cursor| opaque_ref("installation", &cursor.installation_id))
                            .unwrap_or_else(|| "installation_unknown".to_string());
                        tracing::warn!(
                            target: "cc_switch_server::audit_upload",
                            previous_installation_ref,
                            installation_ref = %opaque_ref("installation", &installation_id),
                            fenced_through_boot_id = latest_cursor.as_ref().map(|cursor| cursor.boot_id.as_str()).unwrap_or("-"),
                            fenced_through_sequence = latest_cursor.as_ref().map(|cursor| cursor.sequence).unwrap_or(0),
                            "audit upload destination changed; retained events from the previous Client identity were fenced from the new destination"
                        );
                        retry_delay = AUDIT_UPLOAD_RETRY_INITIAL;
                        last_failure = None;
                        cursor_loss_reported = None;
                        continue;
                    }
                    Err(error) => {
                        audit_upload_failure(
                            &audit,
                            &mut last_failure,
                            "destination_fence",
                            &error,
                            retry_delay,
                        );
                    }
                }
                sleep(retry_delay).await;
                retry_delay = retry_delay.saturating_mul(2).min(AUDIT_UPLOAD_RETRY_MAX);
                continue;
            }
            let event_cursor = loaded_cursor
                .as_ref()
                .and_then(AuditUploadCursor::event_cursor);
            let batch = match tokio::task::spawn_blocking({
                let audit = audit.clone();
                let event_cursor = event_cursor.clone();
                move || {
                    audit.read_batch(
                        event_cursor.as_ref(),
                        crate::logging::AUDIT_UPLOAD_BATCH_LIMIT,
                    )
                }
            })
            .await
            {
                Ok(Ok(batch)) => batch,
                Ok(Err(error)) => {
                    audit_upload_failure(
                        &audit,
                        &mut last_failure,
                        "spool_read",
                        &error.to_string(),
                        retry_delay,
                    );
                    sleep(retry_delay).await;
                    retry_delay = retry_delay.saturating_mul(2).min(AUDIT_UPLOAD_RETRY_MAX);
                    continue;
                }
                Err(error) => {
                    audit_upload_failure(
                        &audit,
                        &mut last_failure,
                        "spool_task",
                        &error.to_string(),
                        retry_delay,
                    );
                    sleep(retry_delay).await;
                    retry_delay = retry_delay.saturating_mul(2).min(AUDIT_UPLOAD_RETRY_MAX);
                    continue;
                }
            };
            if batch.events.is_empty() {
                if audit_upload_recovered(&audit, &mut last_failure) {
                    tracing::info!(
                        target: "cc_switch_server::audit_upload",
                        "router audit upload recovered"
                    );
                }
                retry_delay = AUDIT_UPLOAD_RETRY_INITIAL;
                sleep(AUDIT_UPLOAD_IDLE_INTERVAL).await;
                continue;
            }
            if !batch.cursor_found
                && event_cursor.is_some()
                && cursor_loss_reported.as_ref() != event_cursor.as_ref()
            {
                tracing::warn!(
                    target: "cc_switch_server::audit_upload",
                    boot_id = event_cursor.as_ref().map(|cursor| cursor.boot_id.as_str()).unwrap_or("-"),
                    sequence = event_cursor.as_ref().map(|cursor| cursor.sequence).unwrap_or(0),
                    "audit upload cursor is no longer present in the local spool; resuming from the oldest retained event"
                );
                cursor_loss_reported = event_cursor.clone();
            }

            let boot_id = batch.events[0].boot_id.clone();
            let events = batch
                .events
                .into_iter()
                .take_while(|event| event.boot_id == boot_id)
                .collect::<Vec<_>>();
            let Some(last_event) = events.last() else {
                sleep(AUDIT_UPLOAD_IDLE_INTERVAL).await;
                continue;
            };
            let expected_sequence = last_event.sequence;
            let http = state.http_client().await;
            match client::send_installation_audit_batch(&http, &config, &boot_id, events).await {
                Ok(response)
                    if response.ok
                        && response.boot_id == boot_id
                        && response.sequence == expected_sequence =>
                {
                    let upload_cursor = AuditUploadCursor {
                        router_api_base: router_api_base.clone(),
                        installation_id: installation_id.clone(),
                        boot_id: response.boot_id,
                        sequence: response.sequence,
                    };
                    let stored = tokio::task::spawn_blocking({
                        let audit = audit.clone();
                        let upload_cursor = upload_cursor.clone();
                        move || audit.store_upload_cursor(&upload_cursor)
                    })
                    .await;
                    match stored {
                        Ok(Ok(())) => {
                            if audit_upload_recovered(&audit, &mut last_failure) {
                                tracing::info!(
                                    target: "cc_switch_server::audit_upload",
                                    "router audit upload recovered"
                                );
                            }
                            if response.gap_detected {
                                tracing::warn!(
                                    target: "cc_switch_server::audit_upload",
                                    boot_id = %boot_id,
                                    sequence = expected_sequence,
                                    "router detected an audit sequence gap"
                                );
                            }
                            if response.restart_detected {
                                tracing::info!(
                                    target: "cc_switch_server::audit_upload",
                                    boot_id = %boot_id,
                                    "router accepted a new audit boot stream"
                                );
                            }
                            retry_delay = AUDIT_UPLOAD_RETRY_INITIAL;
                        }
                        Ok(Err(error)) => {
                            audit_upload_failure(
                                &audit,
                                &mut last_failure,
                                "cursor_write",
                                &error.to_string(),
                                retry_delay,
                            );
                            sleep(retry_delay).await;
                            retry_delay = retry_delay.saturating_mul(2).min(AUDIT_UPLOAD_RETRY_MAX);
                        }
                        Err(error) => {
                            audit_upload_failure(
                                &audit,
                                &mut last_failure,
                                "cursor_task",
                                &error.to_string(),
                                retry_delay,
                            );
                            sleep(retry_delay).await;
                            retry_delay = retry_delay.saturating_mul(2).min(AUDIT_UPLOAD_RETRY_MAX);
                        }
                    }
                }
                Ok(response) => {
                    audit_upload_failure(
                        &audit,
                        &mut last_failure,
                        "invalid_ack",
                        &format!(
                            "expected {boot_id}:{expected_sequence}, got {}:{} (ok={})",
                            response.boot_id, response.sequence, response.ok
                        ),
                        retry_delay,
                    );
                    sleep(retry_delay).await;
                    retry_delay = retry_delay.saturating_mul(2).min(AUDIT_UPLOAD_RETRY_MAX);
                }
                Err(error) => {
                    audit_upload_failure(
                        &audit,
                        &mut last_failure,
                        "request",
                        &error.to_string(),
                        retry_delay,
                    );
                    sleep(retry_delay).await;
                    retry_delay = retry_delay.saturating_mul(2).min(AUDIT_UPLOAD_RETRY_MAX);
                }
            }
        }
    });
}

async fn fence_audit_upload_destination(
    audit: crate::logging::SharedAuditLog,
    router_api_base: String,
    installation_id: String,
) -> Result<Option<AuditCursor>, String> {
    tokio::task::spawn_blocking(move || {
        audit.fence_upload_destination(&router_api_base, &installation_id)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

fn audit_upload_failure(
    audit: &SharedAuditLog,
    last_failure: &mut Option<String>,
    kind: &str,
    error: &str,
    retry_delay: Duration,
) {
    if last_failure.as_deref() == Some(kind) {
        tracing::debug!(
            target: "cc_switch_server::audit_upload",
            failure_kind = kind,
            retry_secs = retry_delay.as_secs(),
            "router audit upload remains unavailable"
        );
        return;
    }
    tracing::warn!(
        target: "cc_switch_server::audit_upload",
        failure_kind = kind,
        retry_secs = retry_delay.as_secs(),
        error,
        "router audit upload failed"
    );
    let mut event = AuditEvent::new("observability.component.degraded");
    event.component = Some("router_log_upload".to_string());
    event.failure_kind = Some(kind.to_string());
    event.network_error_kind = classify_network_error(error).map(str::to_string);
    event.error_fingerprint = Some(error_fingerprint("router_log_upload", kind, error));
    event.retry_decision = Some("retry".to_string());
    event.backoff_ms = Some(u64::try_from(retry_delay.as_millis()).unwrap_or(u64::MAX));
    event.outcome = Some("degraded".to_string());
    event.retryable = Some(true);
    audit.emit_best_effort(event);
    *last_failure = Some(kind.to_string());
}

fn audit_upload_recovered(audit: &SharedAuditLog, last_failure: &mut Option<String>) -> bool {
    let Some(failure_kind) = last_failure.take() else {
        return false;
    };
    let mut event = AuditEvent::new("observability.component.recovered");
    event.component = Some("router_log_upload".to_string());
    event.failure_kind = Some(failure_kind);
    event.retry_decision = Some("resume".to_string());
    event.outcome = Some("recovered".to_string());
    event.retryable = Some(false);
    audit.emit_best_effort(event);
    true
}

pub fn spawn_installation_heartbeat(state: ServerState) {
    tokio::spawn(async move {
        let mut consecutive_failures = 0_u32;
        let mut last_failure_warning = None;
        let mut unregistered_retry_secs = ROUTER_HEARTBEAT_UNREGISTERED_RETRY_SECS;
        loop {
            let delay = if run_installation_heartbeat_once(
                &state,
                &mut consecutive_failures,
                &mut last_failure_warning,
            )
            .await
            {
                unregistered_retry_secs = ROUTER_HEARTBEAT_UNREGISTERED_RETRY_SECS;
                next_router_heartbeat_delay(router_heartbeat_interval_secs())
            } else {
                let delay = Duration::from_secs(unregistered_retry_secs);
                unregistered_retry_secs =
                    next_router_registration_retry_secs(unregistered_retry_secs);
                delay
            };
            tokio::select! {
                _ = sleep(delay) => {}
                _ = state.installation_heartbeat_wakeup.notified() => {}
            };
        }
    });
}

pub fn spawn_public_ip_discovery(state: ServerState) {
    tokio::spawn(async move {
        let mut retry_delay = PUBLIC_IP_RETRY_INITIAL;
        let mut failure_warned = false;
        loop {
            let http = state.http_client().await;
            match crate::infra::public_ip::discover_public_ipv4(&http).await {
                Some(ip) => {
                    if state.reported_public_ip().await.as_deref() != Some(ip.as_str()) {
                        tracing::info!(%ip, "discovered public IPv4 for router client display");
                        state.set_reported_public_ip(Some(ip)).await;
                    }
                    retry_delay = PUBLIC_IP_RETRY_INITIAL;
                    failure_warned = false;
                    sleep(PUBLIC_IP_REFRESH_INTERVAL).await;
                }
                None => {
                    if failure_warned {
                        tracing::debug!(
                            retry_secs = retry_delay.as_secs(),
                            "public IPv4 discovery still unavailable"
                        );
                    } else {
                        tracing::warn!(
                            retry_secs = retry_delay.as_secs(),
                            "public IPv4 discovery failed on all endpoints; keeping the last router-observed IP until retry"
                        );
                        failure_warned = true;
                    }
                    sleep(retry_delay).await;
                    retry_delay = retry_delay.saturating_mul(2).min(PUBLIC_IP_RETRY_MAX);
                }
            }
        }
    });
}

pub(super) async fn run_installation_heartbeat_once(
    state: &ServerState,
    consecutive_failures: &mut u32,
    last_failure_warning: &mut Option<tokio::time::Instant>,
) -> bool {
    let config = state.config_snapshot().await;
    if !config.has_registered_router_identity() {
        if config.is_local_setup_complete() && config.router_api_base().is_some() {
            match state.register_router_installation().await {
                Ok(_) => {
                    match state
                        .complete_router_registration_control_plane("heartbeat_unregistered_retry")
                        .await
                    {
                        Ok(()) => {
                            *consecutive_failures = 0;
                            *last_failure_warning = None;
                        }
                        Err(error) => {
                            tracing::warn!(%error, "complete heartbeat registration retry failed");
                        }
                    }
                    return true;
                }
                Err(error) => {
                    *consecutive_failures = consecutive_failures.saturating_add(1);
                    warn_on_sustained_heartbeat_failure(
                        last_failure_warning,
                        *consecutive_failures,
                        &error,
                    );
                    record_installation_heartbeat_failure(state, error.to_string()).await;
                }
            }
        }
        return false;
    }
    let http_client = state.http_client().await;
    let public_ip = state.reported_public_ip().await;
    let log_collection_enabled = state.router_log_collection_enabled().await;
    match client::send_installation_heartbeat(
        &http_client,
        &config,
        &state.process_instance_id,
        public_ip.as_deref(),
        log_collection_enabled,
    )
    .await
    {
        Ok(()) => {
            *consecutive_failures = 0;
            *last_failure_warning = None;
            record_installation_heartbeat_success(state).await;
            state.retry_pending_setup_completion_notification().await;
        }
        Err(client::InstallationHeartbeatError::RegistrationRequired { status, body }) => {
            *consecutive_failures = consecutive_failures.saturating_add(1);
            let heartbeat_error =
                format!("router installation heartbeat requires registration: {status}: {body}");
            tracing::debug!(%status, response = %body, "router installation heartbeat requires registration recovery");
            let recovery_result = match state.register_router_installation().await {
                Ok(_) => match state
                    .complete_router_registration_control_plane("heartbeat_identity_recovery")
                    .await
                {
                    Ok(()) => {
                        let recovered_config = state.config_snapshot().await;
                        let recovered_http_client = state.http_client().await;
                        match client::send_installation_heartbeat(
                            &recovered_http_client,
                            &recovered_config,
                            &state.process_instance_id,
                            state.reported_public_ip().await.as_deref(),
                            state.router_log_collection_enabled().await,
                        )
                        .await
                        {
                            Ok(()) => Ok(()),
                            Err(error) => Err(error.to_string()),
                        }
                    }
                    Err(error) => Err(format!(
                        "complete heartbeat identity recovery failed: {error}"
                    )),
                },
                Err(error) => Err(format!(
                    "router installation registration recovery failed: {error}"
                )),
            };
            match recovery_result {
                Ok(()) => {
                    *consecutive_failures = 0;
                    *last_failure_warning = None;
                    record_installation_heartbeat_success(state).await;
                }
                Err(recovery_error) => {
                    let error = format!("{heartbeat_error}; {recovery_error}");
                    warn_on_sustained_heartbeat_failure(
                        last_failure_warning,
                        *consecutive_failures,
                        &error,
                    );
                    if *consecutive_failures >= ROUTER_HEARTBEAT_SUSTAINED_FAILURES {
                        record_installation_heartbeat_failure(state, error).await;
                    }
                }
            }
        }
        Err(error) => {
            *consecutive_failures = consecutive_failures.saturating_add(1);
            tracing::debug!(error = %error, consecutive_failures = *consecutive_failures, "router installation heartbeat failed");
            warn_on_sustained_heartbeat_failure(
                last_failure_warning,
                *consecutive_failures,
                &error,
            );
            if *consecutive_failures >= ROUTER_HEARTBEAT_SUSTAINED_FAILURES {
                record_installation_heartbeat_failure(state, error.to_string()).await;
            }
        }
    }
    true
}

async fn record_installation_heartbeat_success(state: &ServerState) {
    state
        .mutate_shares_debounced(|shares| {
            shares.last_router_heartbeat_ms = Some(crate::infra::time::now_ms());
            shares.router_registered = true;
            shares.last_router_error = None;
        })
        .await;
}

async fn record_installation_heartbeat_failure(state: &ServerState, message: String) {
    let message = bounded_router_heartbeat_state_error(message);
    state
        .mutate_shares_debounced(|shares| {
            shares.router_registered = false;
            shares.last_router_error = Some(message);
        })
        .await;
}

pub(super) fn bounded_router_heartbeat_state_error(message: String) -> String {
    message
        .chars()
        .take(ROUTER_HEARTBEAT_STATE_ERROR_MAX_CHARS)
        .collect()
}

fn warn_on_sustained_heartbeat_failure(
    last_warning: &mut Option<tokio::time::Instant>,
    consecutive_failures: u32,
    error: &impl std::fmt::Display,
) {
    if consecutive_failures < ROUTER_HEARTBEAT_SUSTAINED_FAILURES {
        return;
    }
    let now = tokio::time::Instant::now();
    if rate_limited_warning_due(last_warning, now, ROUTER_HEARTBEAT_WARNING_INTERVAL) {
        tracing::warn!(
            %error,
            consecutive_failures,
            "router installation heartbeat is persistently failing"
        );
    }
}

pub(super) fn rate_limited_warning_due(
    last_warning: &mut Option<tokio::time::Instant>,
    now: tokio::time::Instant,
    interval: Duration,
) -> bool {
    if last_warning.is_some_and(|last| now.duration_since(last) < interval) {
        return false;
    }
    *last_warning = Some(now);
    true
}

fn router_heartbeat_interval_secs() -> u64 {
    let value = env::var("CC_SWITCH_SERVER_ROUTER_HEARTBEAT_INTERVAL_SECS").ok();
    normalize_router_heartbeat_interval(value.as_deref())
}

pub(super) fn normalize_router_heartbeat_interval(value: Option<&str>) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_ROUTER_HEARTBEAT_INTERVAL_SECS)
        .clamp(
            MIN_ROUTER_HEARTBEAT_INTERVAL_SECS,
            MAX_ROUTER_HEARTBEAT_INTERVAL_SECS,
        )
}

pub(super) fn next_router_heartbeat_delay(interval_secs: u64) -> Duration {
    let jitter = (interval_secs / 10).max(1);
    let width = jitter.saturating_mul(2).saturating_add(1);
    let offset = i128::from(rand::thread_rng().next_u64() % width) - i128::from(jitter);
    Duration::from_secs((i128::from(interval_secs) + offset).max(1) as u64)
}

pub(super) fn next_router_registration_retry_secs(current_secs: u64) -> u64 {
    current_secs
        .max(ROUTER_HEARTBEAT_UNREGISTERED_RETRY_SECS)
        .saturating_mul(2)
        .min(ROUTER_HEARTBEAT_UNREGISTERED_MAX_RETRY_SECS)
}

pub async fn report_installation_upgrade_status(state: &ServerState) -> anyhow::Result<()> {
    let config = state.config.read().await.clone();
    if !config.has_registered_router_identity() {
        return Ok(());
    }
    let client = state.http_client().await;
    let latest = crate::self_update::version::fetch_latest_release_meta(&client).await;
    let upgrade_capable = crate::self_update::version::ensure_binary_writable().is_ok();
    crate::clients::router::client::report_installation_status(
        &client,
        &config,
        &config.upgrade_policy,
        &latest,
        upgrade_capable,
    )
    .await
}

pub fn spawn_periodic_share_sync_retry(state: ServerState) {
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(30)).await;
            run_periodic_share_sync_retry_once(&state).await;
        }
    });
}

pub(super) async fn run_periodic_share_sync_retry_once(state: &ServerState) {
    let config = state.config_snapshot().await;
    if !config.has_registered_router_identity() {
        return;
    }
    retry_pending_router_share_deletes(state.clone()).await;
    if state.router_share_prune_retry_requested() {
        if let Err(error) = reconcile_all_shares_to_router(state.clone()).await {
            tracing::warn!(error = %error, "periodic router share prune snapshot retry failed");
        }
        return;
    }
    let pending_ids = {
        let shares = state.shares.read().await;
        shares
            .shares
            .iter()
            .filter(|share| {
                shares.descriptor_projection_pending(share)
                    || share.router_last_sync_error.is_some()
            })
            .map(|share| share.id.clone())
            .collect::<Vec<_>>()
    };
    for share_id in pending_ids {
        if let Err(error) = sync_share_to_router_with_runtime_refresh(state, &share_id).await {
            tracing::warn!(share_id = %share_id, error = %error, "periodic router share sync retry failed");
        }
    }
}

pub fn spawn_share_edit_event_listener(state: ServerState) {
    let event_state = state.clone();
    tokio::spawn(async move {
        share_edit_event_loop(event_state).await;
    });
    tokio::spawn(async move {
        share_edit_poll_loop(state).await;
    });
}

pub fn spawn_account_quota_refresh(state: ServerState) {
    tokio::spawn(async move {
        let mut previous_expiry_scan_ms = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
        state
            .schedule_loaded_claude_quota_observation_expiries()
            .await;
        if let Err(error) = state
            .reconcile_recurring_subscription_metadata(None, previous_expiry_scan_ms)
            .await
        {
            tracing::warn!(%error, "startup subscription expiry reconciliation failed");
        }
        sleep(Duration::from_secs(60)).await;
        loop {
            let now_ms = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
            if let Err(error) = state
                .reconcile_recurring_subscription_metadata(Some(previous_expiry_scan_ms), now_ms)
                .await
            {
                tracing::warn!(%error, "periodic subscription expiry reconciliation failed");
            }
            previous_expiry_scan_ms = now_ms;
            refresh_due_native_account_tokens(&state).await;
            refresh_due_account_quotas(&state).await;
            let delay = next_account_quota_refresh_delay(&state).await;
            sleep(delay).await;
        }
    });
}

/// Refreshes Provider-owned Cursor API-key presentation/usage snapshots.
/// This is deliberately separate from managed Account quota scheduling: one
/// Provider owns one static credential and no account-pool selection occurs.
pub fn spawn_cursor_account_refresh(state: ServerState) {
    tokio::spawn(async move {
        loop {
            let provider_keys = {
                let providers = state.providers.read().await;
                providers
                    .providers
                    .iter()
                    .filter(|provider| provider.provider_type == ProviderType::CursorApiKey)
                    .filter_map(|provider| {
                        ProviderKey::new(provider.app, provider.provider.id.clone()).ok()
                    })
                    .collect::<Vec<_>>()
            };
            for provider_key in provider_keys {
                match state
                    .cursor_account_snapshot(provider_key.clone(), true)
                    .await
                {
                    Ok(Ok(snapshot)) => {
                        if matches!(
                            snapshot.status,
                            crate::domain::providers::cursor_account::CursorAccountSnapshotStatus::Error
                                | crate::domain::providers::cursor_account::CursorAccountSnapshotStatus::Unconfigured
                        ) {
                            tracing::debug!(
                                app = provider_key.app.as_str(),
                                provider_id = %provider_key.provider_id,
                                status = ?snapshot.status,
                                "background Cursor API-key account refresh returned no usable data"
                            );
                        }
                    }
                    Ok(Err(ProviderCommandError::NotFound)) => {}
                    Ok(Err(error)) => tracing::debug!(
                        app = provider_key.app.as_str(),
                        provider_id = %provider_key.provider_id,
                        error = %error,
                        "background Cursor API-key account refresh was skipped"
                    ),
                    Err(error) => tracing::warn!(
                        app = provider_key.app.as_str(),
                        provider_id = %provider_key.provider_id,
                        error = %error,
                        "background Cursor API-key account refresh failed"
                    ),
                }
            }
            sleep(Duration::from_secs(15 * 60)).await;
        }
    });
}

pub fn spawn_codex_cli_version_sync(state: ServerState) {
    if let Err(error) = crate::codex_identity::initialize_synced_version_cache(&state.config_dir) {
        tracing::warn!(
            error = %error,
            "ignored invalid Codex CLI version sync cache"
        );
    }
    if crate::codex_identity::version_sync_disabled() {
        tracing::info!("Codex CLI version synchronization is disabled by environment");
        return;
    }
    tokio::spawn(async move {
        loop {
            let http = state.http_client().await;
            match crate::codex_identity::sync_latest_version(&http, &state.config_dir).await {
                Ok(version) => {
                    tracing::info!(version, "synchronized the official Codex CLI version")
                }
                Err(error) => tracing::warn!(
                    error = %error,
                    effective_version = %crate::codex_identity::configured_version(),
                    "Codex CLI version synchronization failed; retaining the cached or built-in version"
                ),
            }
            tokio::time::sleep(crate::codex_identity::version_sync_interval()).await;
        }
    });
}

async fn refresh_due_native_account_tokens(state: &ServerState) {
    if state.credential_persistence_degraded() {
        return;
    }
    let now = crate::infra::time::now_ms() as i64;
    let accounts = state.accounts.read().await;
    let candidates = due_native_refresh_candidates(&accounts, now);
    drop(accounts);
    for account in candidates {
        refresh_one_native_account_token(state, account).await;
    }
}

pub(super) fn due_native_refresh_candidates(accounts: &AccountStore, now: i64) -> Vec<Account> {
    let active_codex_account_id = accounts
        .active_codex_oauth_account()
        .map(|account| account.id.as_str());
    accounts
        .accounts
        .iter()
        .filter(|account| account_needs_native_refresh(account, now))
        .filter(|account| {
            account.provider_type != ProviderType::CodexOAuth
                || active_codex_account_id == Some(account.id.as_str())
        })
        .cloned()
        .collect()
}

async fn refresh_one_native_account_token(state: &ServerState, account: Account) {
    if state.credential_persistence_degraded() {
        return;
    }
    let is_still_eligible = {
        let accounts = state.accounts.read().await;
        accounts
            .find_for_provider(account.provider_type, Some(&account.id))
            .filter(|account| {
                account.provider_type != ProviderType::CodexOAuth
                    || accounts
                        .active_codex_oauth_account()
                        .is_some_and(|active| active.id == account.id)
            })
            .is_some()
    };
    if !is_still_eligible {
        return;
    }

    match state
        .refresh_managed_account_if_needed_for_generation(
            account.provider_type,
            &account.id,
            account.auth_identity_generation,
        )
        .await
    {
        Ok(()) => {
            crate::metrics::record_warm_refresh(account.provider_type.as_str(), "success");
        }
        Err(error) => {
            let metric_result = match error {
                ManagedAccountRefreshError::IdentityChanged { .. }
                | ManagedAccountRefreshError::InactiveCodexAccount
                | ManagedAccountRefreshError::NotFound => "superseded",
                ManagedAccountRefreshError::CredentialPersistenceDegraded => "persistence_degraded",
                ManagedAccountRefreshError::Conflict { .. }
                | ManagedAccountRefreshError::Refresh { .. } => "failure",
            };
            crate::metrics::record_warm_refresh(account.provider_type.as_str(), metric_result);
            tracing::warn!(
                account_id = %account.id,
                provider_type = %account.provider_type.as_str(),
                error = ?error,
                "background OAuth token warm-refresh failed"
            );
        }
    }
}

async fn next_account_quota_refresh_delay(state: &ServerState) -> Duration {
    let now = crate::infra::time::now_ms() as i64;
    let interval_ms = state.oauth_quota_refresh_interval_ms().await;
    let accounts = state.accounts_snapshot().await;
    let next_due = accounts
        .accounts
        .iter()
        .filter(|account| account_quota_refresh_candidate(&accounts, account))
        .filter_map(|account| account.quota_next_refresh_at)
        .min();
    let delay_ms = next_due
        .map(|due| due.saturating_sub(now).max(0) as u64)
        .unwrap_or(interval_ms as u64);
    Duration::from_millis(delay_ms.clamp(1_000, (interval_ms as u64).min(60_000)))
}

pub(super) fn account_quota_refresh_candidate(accounts: &AccountStore, account: &Account) -> bool {
    if account.needs_relogin {
        return false;
    }
    if account.provider_type == ProviderType::CodexOAuth
        && accounts
            .active_codex_oauth_account()
            .map(|active| active.id.as_str())
            != Some(account.id.as_str())
    {
        return false;
    }
    match account.provider_type {
        ProviderType::CodexOAuth
        | ProviderType::ClaudeOAuth
        | ProviderType::GeminiCli
        | ProviderType::AntigravityOAuth
        | ProviderType::AgyOAuth
        | ProviderType::GitHubCopilot
        | ProviderType::KiroOAuth
        | ProviderType::AmazonQOAuth
        | ProviderType::CursorOAuth
        | ProviderType::CursorApiKey
        | ProviderType::GrokOAuth
        | ProviderType::QoderCosy
        | ProviderType::CodeBuddyOAuth
        | ProviderType::OllamaCloud => {
            account
                .access_token
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || account
                    .refresh_token
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                || account
                    .api_key
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
        }
        _ => false,
    }
}

async fn refresh_due_account_quotas(state: &ServerState) {
    let now = crate::infra::time::now_ms() as i64;
    let accounts = state.accounts_snapshot().await;
    let due_accounts = accounts
        .accounts
        .iter()
        .filter(|account| account_quota_refresh_due(&accounts, account, now))
        .cloned()
        .collect::<Vec<_>>();
    for account in due_accounts {
        refresh_one_account_quota(state, account, now).await;
    }
}

pub(super) async fn refresh_one_account_quota(state: &ServerState, account: Account, now: i64) {
    let locked_provider_type = account.provider_type;
    let refresh_generation_before_lock = (
        account.auth_identity_generation,
        account.token_refresh_generation,
    );
    if !background_quota_outbound_allowed(state, &account).await {
        return;
    }
    if let Err(error) = state
        .refresh_managed_account_if_needed_for_generation(
            locked_provider_type,
            &account.id,
            account.auth_identity_generation,
        )
        .await
    {
        tracing::warn!(
            account_id = %account.id,
            provider_type = %account.provider_type.as_str(),
            error = ?error,
            "background quota OAuth token refresh failed"
        );
        defer_background_quota_after_managed_refresh_failure(state, &account, now).await;
        return;
    }
    let Some(mut _refresh_guard) = state
        .account_refresh_locks
        .try_lock(locked_provider_type, &account.id)
    else {
        return;
    };
    // The periodic scan clones accounts before acquiring the per-account lock.
    // A workspace switch may complete between those two operations, so always
    // re-read under the lock and re-check due state before issuing requests.
    let accounts = state.accounts_snapshot().await;
    let Some(account) = accounts
        .accounts
        .iter()
        .find(|candidate| candidate.id == account.id)
        .cloned()
    else {
        return;
    };
    if account.provider_type != locked_provider_type
        || !account_quota_refresh_due(&accounts, &account, now)
    {
        return;
    }
    if !background_quota_outbound_allowed(state, &account).await {
        return;
    }
    let success_cooldown_ms = state.oauth_quota_refresh_interval_ms().await;
    let account_before_refresh = account.clone();
    let mut active_account = account;
    let native_refresh_attempted = (
        active_account.auth_identity_generation,
        active_account.token_refresh_generation,
    ) != refresh_generation_before_lock;

    if !background_quota_outbound_allowed(state, &active_account).await {
        return;
    }
    let http_client = state.http_client().await;
    let timeout_ms = state.oauth_quota_refresh_timeout_ms().await;
    let mut quota_result = refresh_account_quota(
        &http_client,
        &active_account,
        now,
        false,
        success_cooldown_ms,
        timeout_ms,
    )
    .await;
    if !native_refresh_attempted
        && quota_result
            .as_ref()
            .is_err_and(|error| error.upstream_status == Some(401))
        && !active_account.needs_relogin
        && provider_native_refresh_available(active_account.provider_type)
        && account_has_refresh_token(&active_account)
    {
        let recovery_auth_identity_generation = active_account.auth_identity_generation;
        _refresh_guard.release();
        if let Err(error) = state
            .refresh_managed_account_now_for_generation(
                active_account.provider_type,
                &active_account.id,
                recovery_auth_identity_generation,
            )
            .await
        {
            tracing::warn!(
                account_id = %active_account.id,
                provider_type = %active_account.provider_type.as_str(),
                error = ?error,
                "background quota 401 OAuth recovery failed"
            );
            defer_background_quota_after_managed_refresh_failure(state, &active_account, now).await;
            return;
        }
        let Some(guard) = state
            .account_refresh_locks
            .try_lock(active_account.provider_type, &active_account.id)
        else {
            return;
        };
        _refresh_guard = guard;
        let Some(refreshed) =
            state
                .find_account_by_id(&active_account.id)
                .await
                .filter(|account| {
                    account.provider_type == locked_provider_type
                        && account.auth_identity_generation == recovery_auth_identity_generation
                })
        else {
            return;
        };
        active_account = refreshed;
        if !background_quota_outbound_allowed(state, &active_account).await {
            return;
        }
        quota_result = refresh_account_quota(
            &http_client,
            &active_account,
            now,
            true,
            success_cooldown_ms,
            timeout_ms,
        )
        .await;
    }
    match quota_result {
        Ok(QuotaRefreshResult::Updated { update, .. }) => {
            let account = match state
                .commit_account_quota_refresh_update(&active_account, update)
                .await
            {
                Ok(Ok(account)) => account,
                Ok(Err(AccountQuotaCommitSkip::Stale(current))) => {
                    tracing::info!(
                        account_id = %current.id,
                        provider_type = %current.provider_type.as_str(),
                        "discarded background quota response for superseded credentials"
                    );
                    return;
                }
                Ok(Err(AccountQuotaCommitSkip::NotFound)) => return,
                Err(error) => {
                    tracing::error!(
                        account_id = %active_account.id,
                        %error,
                        "background quota success persistence failed"
                    );
                    return;
                }
            };
            if let Err(error) = state
                .refresh_account_runtime_metadata_if_changed(&account_before_refresh, &account)
                .await
            {
                tracing::warn!(
                    account_id = %account.id,
                    %error,
                    "background quota refresh could not persist account runtime metadata change"
                );
            }
            emit_oauth_quota_updated(state, &account, true);
        }
        Ok(QuotaRefreshResult::SkippedCooldown { .. }) => {}
        Err(error) => {
            let mut update = error
                .partial_update
                .map(|update| *update)
                .unwrap_or_default();
            update.quota_next_refresh_at = error.next_refresh_at;
            update.last_refresh_error = Some(error.message);
            if update.quota_next_refresh_at.is_none() {
                update.quota_next_refresh_at = Some(
                    now.saturating_add(crate::clients::oauth::quota::QUOTA_FAILURE_COOLDOWN_MS),
                );
            }
            let account = match state
                .commit_account_quota_refresh_update(&active_account, update)
                .await
            {
                Ok(Ok(account)) => account,
                Ok(Err(AccountQuotaCommitSkip::Stale(current))) => {
                    tracing::info!(
                        account_id = %current.id,
                        provider_type = %current.provider_type.as_str(),
                        "discarded background quota failure for superseded credentials"
                    );
                    return;
                }
                Ok(Err(AccountQuotaCommitSkip::NotFound)) => return,
                Err(error) => {
                    tracing::error!(
                        account_id = %active_account.id,
                        %error,
                        "background quota failure metadata persistence failed"
                    );
                    return;
                }
            };
            if let Err(error) = state
                .refresh_account_runtime_metadata_if_changed(&account_before_refresh, &account)
                .await
            {
                tracing::warn!(
                    account_id = %account.id,
                    %error,
                    "background quota refresh failure Share descriptor sync remains pending"
                );
            }
        }
    }
}

async fn defer_background_quota_after_managed_refresh_failure(
    state: &ServerState,
    expected: &Account,
    now: i64,
) {
    let Some(current) = state
        .find_account_by_id(&expected.id)
        .await
        .filter(|account| {
            account.provider_type == expected.provider_type
                && account.auth_identity_generation == expected.auth_identity_generation
        })
    else {
        return;
    };
    let next_refresh_at = current
        .quota_next_refresh_at
        .unwrap_or(i64::MIN)
        .max(now.saturating_add(crate::clients::oauth::quota::QUOTA_FAILURE_COOLDOWN_MS));
    let update = AccountRefreshUpdate {
        quota_next_refresh_at: Some(next_refresh_at),
        last_refresh_error: current.last_refresh_error.clone(),
        ..Default::default()
    };
    match state
        .commit_account_quota_refresh_update(&current, update)
        .await
    {
        Ok(Ok(updated)) => {
            if let Err(error) = state
                .refresh_account_runtime_metadata_if_changed(&current, &updated)
                .await
            {
                tracing::warn!(
                    account_id = %updated.id,
                    %error,
                    "background OAuth refresh cooldown Share metadata sync remains pending"
                );
            }
        }
        Ok(Err(_)) => {}
        Err(error) => tracing::error!(
            account_id = %current.id,
            %error,
            "persisting background OAuth refresh cooldown failed"
        ),
    }
}

async fn background_quota_outbound_allowed(state: &ServerState, account: &Account) -> bool {
    if state.credential_persistence_degraded() {
        return false;
    }
    if account.provider_type != ProviderType::CodexOAuth {
        return true;
    }
    let accounts = state.accounts_snapshot().await;
    accounts
        .active_codex_oauth_account()
        .map(|active| active.id.as_str())
        == Some(account.id.as_str())
}

pub(super) fn account_quota_refresh_due(
    accounts: &AccountStore,
    account: &Account,
    now: i64,
) -> bool {
    if account
        .quota_next_refresh_at
        .is_some_and(|next_refresh_at| next_refresh_at > now)
    {
        return false;
    }
    account_quota_refresh_candidate(accounts, account)
}
