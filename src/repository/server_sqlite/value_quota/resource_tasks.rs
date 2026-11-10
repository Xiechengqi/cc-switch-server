use super::*;

pub(crate) fn attach_task(
    config_dir: &Path,
    request: &str,
    attempt: &str,
    task: &str,
    expires: i64,
) -> anyhow::Result<()> {
    ensure!(
        !task.is_empty() && task.len() <= 256 && !task.chars().any(char::is_control),
        "invalid monetary task identity"
    );
    assert_committed(config_dir)?;
    let mut conn = open_database(&database_path(config_dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let row = read_attempt_row(&tx, request, attempt)?.context("VALUE_QUOTA_ATTEMPT_UNKNOWN")?;
    let existing: Option<(String, String)> = tx
        .query_row(
            "SELECT request_id,attempt_id FROM value_async_tasks WHERE scope=?1 AND task_id=?2",
            params![row.0, task],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some(existing) = existing {
        ensure!(
            existing == (request.into(), attempt.into()),
            "VALUE_QUOTA_ASYNC_TASK_CONFLICT"
        );
    } else {
        tx.execute("INSERT INTO value_async_tasks(scope,task_id,request_id,attempt_id,status,expires_at_ms) VALUES(?1,?2,?3,?4,'pending',?5)", params![row.0,task,request,attempt,expires])?;
    }
    tx.commit()?;
    Ok(())
}

/// Polling owns no monetary attempt. Its already-verified ownership binding
/// can supply terminal evidence to the original attempt, even after restart.
pub(super) fn observe_task_in_tx(tx: &Transaction<'_>, log: &UsageLog) -> anyhow::Result<bool> {
    use crate::domain::usage::store::{InferenceOperation, UsageState};
    if log.operation != InferenceOperation::VideoStatus {
        return Ok(false);
    }
    let (Some(task), Some(parent)) = (&log.media_task_id, &log.parent_request_id) else {
        return Ok(false);
    };
    let task_row: Option<(String,String,String,Option<String>,String)> = tx.query_row(
        "SELECT scope,request_id,attempt_id,observation_sha256,status FROM value_async_tasks WHERE task_id=?1 AND request_id=?2", params![task,parent],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let Some((scope, request, attempt, previous, status)) = task_row else {
        return Ok(false);
    };
    let identity: Scope = serde_json::from_str(&scope)?;
    ensure!(
        log.share_id.as_deref() == Some(&identity.share_id)
            && log
                .user_email
                .as_deref()
                .is_some_and(|email| email.eq_ignore_ascii_case(&identity.email)),
        "VALUE_QUOTA_ASYNC_OWNER_CONFLICT"
    );
    let completed = matches!(
        log.media_status.as_deref(),
        Some("completed" | "succeeded" | "done")
    );
    let failed = matches!(
        log.media_status.as_deref(),
        Some("failed" | "cancelled" | "canceled" | "expired")
    );
    if !completed && !failed {
        return Ok(false);
    }
    // Late failures cannot erase a completed generation; later successful
    // evidence may still refine the quantity or recover an earlier failure.
    if status == "completed" && failed {
        return Ok(false);
    }
    let digest = sha256_hex(
        serde_json::to_string(&(
            completed,
            failed,
            &log.value_resources,
            log.video_duration_seconds,
        ))?
        .as_bytes(),
    );
    if previous.as_deref() == Some(&digest) {
        return Ok(false);
    }
    let old =
        read_attempt_row(tx, &request, &attempt)?.context("VALUE_QUOTA_ASYNC_ATTEMPT_MISSING")?;
    let mut evidence = log.clone();
    evidence.request_id = request.clone();
    evidence.operation = InferenceOperation::VideoGeneration;
    evidence.attempt_count = attempt.parse()?;
    evidence.actual_model = Some(old.1);
    evidence.effective_service_tier = Some(old.2);
    evidence.usage_state = UsageState::NotApplicable;
    evidence.is_streaming = false;
    evidence.stream_status = Some(if completed { "completed" } else { "failed" }.into());
    evidence.usage_revision = u64::try_from(old.4)?
        .checked_add(1)
        .context("value usage revision overflow")?;
    // A failed task may already have consumed resources. Unknown execution
    // retains the reservation, with any later trusted completion revisable.
    if failed {
        evidence.failure_kind = Some("upstream_outcome_unknown".into());
        evidence.status_code = 502;
    }
    let changed = settle_log_in_tx(tx, &evidence, false)?;
    tx.execute("UPDATE value_async_tasks SET status=?3,observation_sha256=?4 WHERE scope=?1 AND task_id=?2",params![scope,task,if completed {"completed"} else {"failed"},digest])?;
    Ok(changed)
}
