//! One process-wide admission lane. Genuine throttles are shared; an explicit
//! client-admission refusal applies only to the request profile that was sent.
use crate::proxy::ProxyError;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

static ACTIVE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static QUEUED: Semaphore = Semaphore::const_new(32);
static COOLDOWNS: Mutex<CooldownBook> = Mutex::new(CooldownBook {
    entries: BTreeMap::new(),
});
const MAX_COOLDOWNS: usize = 512;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::proxy) struct LaneProfile([u8; 32]);

impl LaneProfile {
    pub(super) fn from_body(path: &str, body: &Value) -> Self {
        struct HashWriter<'a>(&'a mut Sha256);
        impl Write for HashWriter<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut hash = Sha256::new();
        hash.update(super::WIRE_PROFILE.as_bytes());
        hash.update([0]);
        hash.update(path.as_bytes());
        // No full serialized copy or user conversation in the cache key. Tools,
        // controls and system instructions describe the admission profile.
        if let Some(object) = body.as_object() {
            for (name, value) in object {
                hash.update([0]);
                hash.update(name.as_bytes());
                if matches!(name.as_str(), "messages" | "input") {
                    if let Some(items) = value.as_array() {
                        for item in items {
                            if matches!(item["role"].as_str(), Some("system" | "developer")) {
                                serde_json::to_writer(HashWriter(&mut hash), item)
                                    .expect("hashing JSON cannot fail");
                            }
                        }
                    }
                } else {
                    serde_json::to_writer(HashWriter(&mut hash), value)
                        .expect("hashing JSON cannot fail");
                }
            }
        }
        Self(hash.finalize().into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::proxy) enum CooldownScope {
    Origin,
    Model,
    RequestProfile,
}
impl CooldownScope {
    fn label(self) -> &'static str {
        match self {
            Self::Origin => "public_lane",
            Self::Model => "model",
            Self::RequestProfile => "request_profile",
        }
    }
}

fn key(origin: &str, model: Option<&str>, profile: Option<LaneProfile>) -> String {
    let mut hash = Sha256::new();
    hash.update(origin.as_bytes());
    hash.update([0]);
    if let Some(model) = model {
        hash.update(super::base_model_id(model).as_bytes());
    }
    if let Some(profile) = profile {
        hash.update([0]);
        hash.update(profile.0);
    }
    hex::encode(hash.finalize())
}

#[derive(Clone)]
struct Cooldown {
    until: Instant,
    scope: CooldownScope,
    request_id: String,
    upstream_request_id: String,
    cause: &'static str,
}
struct CooldownBook {
    entries: BTreeMap<String, Cooldown>,
}

fn safe_id(value: &str) -> String {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        value.to_owned()
    } else {
        "unavailable".to_owned()
    }
}

fn request_ref(value: &str) -> String {
    // Keep raw bounded IDs in debug provenance; public classifiers see opaque
    // refs so diagnostic IDs containing words such as `quota` cannot interfere.
    hex::encode(&Sha256::digest(value.as_bytes())[..12])
}

impl CooldownBook {
    fn check(&mut self, origin: &str, model: &str, profile: LaneProfile) -> Result<(), ProxyError> {
        let now = Instant::now();
        self.entries.retain(|_, entry| entry.until > now);
        let keys = [
            key(origin, None, None),
            key(origin, Some(model), None),
            key(origin, Some(model), Some(profile)),
        ];
        if let Some(entry) = keys
            .iter()
            .filter_map(|key| self.entries.get(key))
            .max_by_key(|entry| entry.until)
        {
            let seconds = entry
                .until
                .duration_since(now)
                .as_secs()
                .saturating_add(1)
                .min(86400);
            return Err(ProxyError::rate_limited(format!(
                "OpenCode Zen public lane is cooling down; category=local_cooldown; scope={}; cause={}; source_request_ref={}; upstream_request_ref={}",
                entry.scope.label(), entry.cause, request_ref(&entry.request_id), request_ref(&entry.upstream_request_id),
            ), seconds));
        }
        Ok(())
    }

    fn observe(
        &mut self,
        origin: &str,
        model: &str,
        profile: LaneProfile,
        scope: CooldownScope,
        request_id: &str,
        error: &ProxyError,
    ) {
        if !matches!(error.status.as_u16(), 401 | 403 | 429) {
            return;
        }
        let seconds = error.retry_after_seconds().unwrap_or(30).clamp(1, 86400);
        let now = Instant::now();
        self.entries.retain(|_, entry| entry.until > now);
        let mut until = now + Duration::from_secs(seconds);
        let mut scope = scope;
        let mut inherited = None;
        let mut index = match scope {
            CooldownScope::Origin => key(origin, None, None),
            CooldownScope::Model => key(origin, Some(model), None),
            CooldownScope::RequestProfile => key(origin, Some(model), Some(profile)),
        };
        if self.entries.len() >= MAX_COOLDOWNS && !self.entries.contains_key(&index) {
            if let Some(oldest) = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.scope == CooldownScope::RequestProfile)
                .min_by_key(|(_, entry)| entry.until)
                .map(|(key, _)| key.clone())
            {
                // Negative profile caching is bounded and never promoted to a
                // lane-wide refusal. The original caller still has Retry-After.
                self.entries.remove(&oldest);
            } else if scope == CooldownScope::RequestProfile {
                return;
            } else {
                // Preserve the existing conservative real-throttle overflow.
                if let Some(previous) = self.entries.values().max_by_key(|entry| entry.until) {
                    if previous.until > until {
                        until = previous.until;
                        inherited = Some(previous.clone());
                    }
                }
                self.entries.clear();
                scope = CooldownScope::Origin;
                index = key(origin, None, None);
            }
        }
        if self
            .entries
            .get(&index)
            .is_some_and(|entry| entry.until > until)
        {
            return;
        }
        let upstream_request_id = error
            .client_message()
            .split("request_id=")
            .nth(1)
            .and_then(|value| value.split([';', ' ']).next())
            .map(safe_id)
            .unwrap_or_else(|| "unavailable".into());
        let cause = match scope {
            CooldownScope::RequestProfile => "free_lane_gate",
            _ if error.status.as_u16() == 429 => "upstream_rate_limit",
            _ => "upstream_access_rejected",
        };
        let request_id = safe_id(request_id);
        let (request_id, upstream_request_id, cause) = inherited
            .map_or((request_id, upstream_request_id, cause), |entry| {
                (entry.request_id, entry.upstream_request_id, entry.cause)
            });
        let cached_seconds = until.duration_since(now).as_secs();
        tracing::debug!("OpenCode Zen cooldown observed: scope={} cause={} source_request_id={} upstream_request_id={} source_request_ref={} upstream_request_ref={} profile={} retry_after_seconds={} expires_at_ms={}",
            scope.label(), cause, request_id, upstream_request_id, request_ref(&request_id), request_ref(&upstream_request_id), hex::encode(&profile.0[..8]), cached_seconds,
            chrono::Utc::now().timestamp_millis().saturating_add(cached_seconds.saturating_mul(1000).min(i64::MAX as u64) as i64));
        self.entries.insert(
            index,
            Cooldown {
                until,
                scope,
                request_id,
                upstream_request_id,
                cause,
            },
        );
    }
}

pub(in crate::proxy) fn check(
    origin: &str,
    model: &str,
    profile: LaneProfile,
) -> Result<(), ProxyError> {
    COOLDOWNS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .check(origin, model, profile)
}

pub(in crate::proxy) async fn acquire(
    origin: &str,
    model: &str,
    profile: LaneProfile,
    deadline: tokio::time::Instant,
) -> Result<OwnedSemaphorePermit, ProxyError> {
    if tokio::time::Instant::now() >= deadline {
        return Err(ProxyError {
            status: http::StatusCode::GATEWAY_TIMEOUT,
            message: "OpenCode Zen admission deadline exceeded".into(),
        });
    }
    check(origin, model, profile)?;
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
    check(origin, model, profile)?;
    Ok(permit)
}

pub(in crate::proxy) fn observe_error(
    origin: &str,
    model: &str,
    profile: LaneProfile,
    scope: CooldownScope,
    request_id: &str,
    error: &ProxyError,
) {
    COOLDOWNS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .observe(origin, model, profile, scope, request_id, error);
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn acquire(
        origin: &str,
        model: &str,
        deadline: tokio::time::Instant,
    ) -> Result<OwnedSemaphorePermit, ProxyError> {
        super::acquire(origin, model, LaneProfile::default(), deadline).await
    }
    fn observe_error(origin: &str, model: &str, error: &ProxyError) {
        super::observe_error(
            origin,
            model,
            LaneProfile::default(),
            if error.status.as_u16() == 429 {
                CooldownScope::Model
            } else {
                CooldownScope::Origin
            },
            "lane-test",
            error,
        )
    }
    fn check(origin: &str, model: &str) -> Result<(), ProxyError> {
        super::check(origin, model, LaneProfile::default())
    }

    #[test]
    fn zen_request_profile_cooldown_preserves_provenance_and_real_model_limits() {
        use serde_json::json;
        let a = LaneProfile::from_body(
            "/chat",
            &json!({"tools":[{"name":"a"}],"messages":[{"role":"user","content":"one"}]}),
        );
        let a_next = LaneProfile::from_body(
            "/chat",
            &json!({"tools":[{"name":"a"}],"messages":[{"role":"user","content":"two"}]}),
        );
        let b = LaneProfile::from_body("/chat", &json!({"tools":[{"name":"b"}],"messages":[]}));
        assert_eq!(a, a_next);
        assert_ne!(a, b);
        assert_ne!(
            a,
            LaneProfile::from_body("/messages", &json!({"tools":[{"name":"a"}],"messages":[]}))
        );
        let mut book = CooldownBook {
            entries: BTreeMap::new(),
        };
        let error = ProxyError::rate_limited("OpenCode Zen upstream rate_limit: HTTP 429; request_id=ray-safe; category=free_lane_gate",43200);
        book.observe(
            "origin",
            "free",
            a,
            CooldownScope::RequestProfile,
            "request-one",
            &error,
        );
        let blocked = book.check("origin", "free", a_next).unwrap_err();
        assert!(blocked.client_message().contains("scope=request_profile"));
        assert!(blocked.client_message().contains(&format!(
            "source_request_ref={}",
            request_ref("request-one")
        )));
        assert!(blocked
            .client_message()
            .contains(&format!("upstream_request_ref={}", request_ref("ray-safe"))));
        assert!(matches!(blocked.retry_after_seconds(), Some(43199..=43201)));
        assert!(book.check("origin", "free", b).is_ok());
        assert!(book.check("origin", "other-free", a).is_ok());
        book.observe(
            "origin",
            "free",
            a,
            CooldownScope::Model,
            "request-two",
            &ProxyError::rate_limited("genuine", 60),
        );
        let quota = book.check("origin", "free", b).unwrap_err();
        assert!(quota.client_message().contains("scope=model"));
        assert!(quota.client_message().contains(&format!(
            "source_request_ref={}",
            request_ref("request-two")
        )));
        assert!(book.check("origin", "other-free", b).is_ok());
        book.observe(
            "origin",
            "free",
            a,
            CooldownScope::Origin,
            "bad?secret",
            &ProxyError {
                status: http::StatusCode::FORBIDDEN,
                message: "denied".into(),
            },
        );
        let forbidden = book.check("origin", "other-free", b).unwrap_err();
        assert!(forbidden.client_message().contains(&format!(
            "source_request_ref={}",
            request_ref("unavailable")
        )));
        assert!(!forbidden.client_message().contains("secret"));
        for entry in book.entries.values_mut() {
            entry.until = Instant::now() - Duration::from_secs(1);
        }
        assert!(book.check("origin", "free", a).is_ok());
    }

    #[test]
    fn zen_profile_cache_saturation_never_promotes_a_gate_to_model_or_lane() {
        let mut book = CooldownBook {
            entries: BTreeMap::new(),
        };
        let error = ProxyError::rate_limited("gate", 43200);
        for index in 0..MAX_COOLDOWNS + 2 {
            let profile = LaneProfile::from_body(
                "/chat",
                &serde_json::json!({"tools":[{"name":format!("tool-{index}")}]}),
            );
            book.observe(
                "origin",
                "free",
                profile,
                CooldownScope::RequestProfile,
                "request-gate",
                &error,
            );
        }
        assert_eq!(book.entries.len(), MAX_COOLDOWNS);
        assert!(book
            .entries
            .values()
            .all(|entry| entry.scope == CooldownScope::RequestProfile));
        assert!(book.check("origin", "free", LaneProfile::default()).is_ok());
        book.observe(
            "origin",
            "free",
            LaneProfile::default(),
            CooldownScope::Model,
            "quota",
            &error,
        );
        assert!(book
            .check("origin", "free", LaneProfile::default())
            .is_err());
        assert!(book
            .check("origin", "other", LaneProfile::default())
            .is_ok());
    }

    #[test]
    fn zen_real_throttle_overflow_keeps_the_longest_observation_provenance() {
        let mut book = CooldownBook {
            entries: BTreeMap::new(),
        };
        for index in 0..MAX_COOLDOWNS {
            let seconds = if index == 0 { 43200 } else { 30 };
            let source = if index == 0 {
                "long-observation"
            } else {
                "short-observation"
            };
            book.observe(
                "origin",
                &format!("model-{index}"),
                LaneProfile::default(),
                CooldownScope::Model,
                source,
                &ProxyError::rate_limited("limited", seconds),
            );
        }
        book.observe(
            "origin",
            "new-model",
            LaneProfile::default(),
            CooldownScope::Model,
            "new-short-observation",
            &ProxyError::rate_limited("limited", 1),
        );
        let blocked = book
            .check("origin", "another-model", LaneProfile::default())
            .unwrap_err();
        assert!(matches!(blocked.retry_after_seconds(), Some(43195..=43201)));
        assert!(blocked
            .client_message()
            .contains(&request_ref("long-observation")));
        assert!(!blocked
            .client_message()
            .contains(&request_ref("new-short-observation")));
    }

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
