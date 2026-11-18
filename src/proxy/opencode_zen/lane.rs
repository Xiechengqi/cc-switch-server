//! One process-wide public lane, independently of Provider/Share count. This
//! is admission/backoff, never account selection, rotation or generation replay.
use crate::proxy::ProxyError;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

static ACTIVE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static QUEUED: Semaphore = Semaphore::const_new(32);
static COOLDOWNS: Mutex<BTreeMap<String, Instant>> = Mutex::new(BTreeMap::new());

fn key(origin: &str, model: Option<&str>) -> String {
    let mut hash = Sha256::new();
    hash.update(origin.as_bytes());
    hash.update([0]);
    if let Some(model) = model {
        hash.update(super::base_model_id(model).as_bytes());
    }
    hex::encode(hash.finalize())
}

fn check(origin: &str, model: &str) -> Result<(), ProxyError> {
    let now = Instant::now();
    let mut entries = COOLDOWNS.lock().unwrap_or_else(|e| e.into_inner());
    entries.retain(|_, until| *until > now);
    let until = [key(origin, None), key(origin, Some(model))]
        .iter()
        .filter_map(|k| entries.get(k))
        .max();
    if let Some(until) = until {
        let seconds = until
            .duration_since(now)
            .as_secs()
            .saturating_add(1)
            .min(86400);
        return Err(ProxyError::rate_limited(
            "OpenCode Zen public lane is cooling down; category=local_cooldown",
            seconds,
        ));
    }
    Ok(())
}

pub(in crate::proxy) async fn acquire(
    origin: &str,
    model: &str,
    deadline: tokio::time::Instant,
) -> Result<OwnedSemaphorePermit, ProxyError> {
    if tokio::time::Instant::now() >= deadline {
        return Err(ProxyError {
            status: http::StatusCode::GATEWAY_TIMEOUT,
            message: "OpenCode Zen admission deadline exceeded".into(),
        });
    }
    check(origin, model)?;
    let queue = QUEUED
        .try_acquire()
        .map_err(|_| ProxyError::rate_limited("OpenCode Zen admission queue is full", 1))?;
    let active = ACTIVE.get_or_init(|| {
        let limit = std::env::var("CC_SWITCH_ZEN_MAX_CONCURRENCY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| (1..=32).contains(v))
            .unwrap_or(4);
        Arc::new(Semaphore::new(limit))
    });
    let permit = tokio::time::timeout_at(deadline, active.clone().acquire_owned())
        .await
        .map_err(|_| ProxyError {
            status: http::StatusCode::GATEWAY_TIMEOUT,
            message: "OpenCode Zen admission deadline exceeded".into(),
        })?
        .map_err(|_| ProxyError::bad_gateway("OpenCode Zen admission is unavailable"))?;
    drop(queue);
    check(origin, model)?;
    Ok(permit)
}

pub(in crate::proxy) fn observe_error(origin: &str, model: &str, error: &ProxyError) {
    let scope = match error.status.as_u16() {
        429 => Some(model),
        401 | 403 => None,
        _ => return,
    };
    let seconds = error.retry_after_seconds().unwrap_or(30).clamp(1, 86400);
    let now = Instant::now();
    let mut entries = COOLDOWNS.lock().unwrap_or_else(|e| e.into_inner());
    entries.retain(|_, until| *until > now);
    if entries.len() >= 512 {
        // Saturation conservatively cools the public lane; no model or Share
        // gains an unbounded index or bypasses a known upstream throttle.
        let until = entries
            .values()
            .copied()
            .max()
            .unwrap_or(now)
            .max(now + Duration::from_secs(seconds));
        entries.clear();
        entries.insert(key(origin, None), until);
        return;
    }
    let until = now + Duration::from_secs(seconds);
    entries
        .entry(key(origin, scope))
        .and_modify(|old| *old = (*old).max(until))
        .or_insert(until);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn zen_lane_shares_model_cooldown_and_preserves_deadlines() {
        let _lock = super::super::CATALOG_TEST_LOCK.lock().await;
        let origin = "http://lane-cooldown.test";
        observe_error(origin, "one-free", &ProxyError::rate_limited("limited", 2));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        assert_eq!(
            acquire(origin, "one-free", deadline)
                .await
                .unwrap_err()
                .status,
            429
        );
        assert!(acquire(origin, "other-free", deadline).await.is_ok());
        observe_error(
            origin,
            "one-free",
            &ProxyError {
                status: http::StatusCode::FORBIDDEN,
                message: "blocked".into(),
            },
        );
        assert!(acquire(origin, "other-free", deadline).await.is_err());
        let expired = acquire(
            "http://expired-lane.test",
            "one-free",
            tokio::time::Instant::now() - Duration::from_secs(1),
        )
        .await;
        assert_eq!(expired.unwrap_err().status, 504);
    }
    #[tokio::test]
    async fn zen_lane_queue_deadline_cancellation_and_drop_release_capacity() {
        let _lock = super::super::CATALOG_TEST_LOCK.lock().await;
        let origin = "http://lane-queue.test";
        let first = acquire(
            origin,
            "free",
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await
        .unwrap();
        let active = ACTIVE.get().unwrap().clone();
        let capacity = active.available_permits();
        let rest = active
            .clone()
            .acquire_many_owned(capacity as u32)
            .await
            .unwrap();
        let blocked = tokio::spawn(async move {
            acquire(
                origin,
                "free",
                tokio::time::Instant::now() + Duration::from_secs(2),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!blocked.is_finished());
        blocked.abort();
        let _ = blocked.await;
        assert_eq!(QUEUED.available_permits(), 32);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(15);
        assert_eq!(
            acquire(origin, "free", deadline).await.unwrap_err().status,
            504
        );
        assert_eq!(QUEUED.available_permits(), 32);
        drop(first);
        assert!(acquire(
            origin,
            "free",
            tokio::time::Instant::now() + Duration::from_secs(1)
        )
        .await
        .is_ok());
        drop(rest);
        let error = ProxyError::rate_limited("limited", 1);
        observe_error(origin, "free", &error);
        assert!(check(origin, "free").is_err());
        tokio::time::sleep(Duration::from_millis(1050)).await;
        assert!(check(origin, "free").is_ok());
    }
}
