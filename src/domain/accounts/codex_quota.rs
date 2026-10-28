//! Sparse passive quota observations. These describe an account, never the
//! tokens consumed by a generation, and do not change account health.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observed<T> {
    pub value: T,
    pub observed_at_ms: i64,
}

fn merge<T: Clone>(current: &mut Option<Observed<T>>, incoming: &Option<Observed<T>>) -> bool {
    if let Some(value) = incoming.as_ref().filter(|value| {
        current
            .as_ref()
            .is_none_or(|old| value.observed_at_ms > old.observed_at_ms)
    }) {
        *current = Some(value.clone());
        true
    } else {
        false
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CodexQuotaWindow {
    pub used_percent: Option<Observed<f64>>,
    pub window_seconds: Option<Observed<i64>>,
    pub resets_at_ms: Option<Observed<i64>>,
}

impl CodexQuotaWindow {
    fn merge(&mut self, update: &Self) -> bool {
        merge(&mut self.used_percent, &update.used_percent)
            | merge(&mut self.window_seconds, &update.window_seconds)
            | merge(&mut self.resets_at_ms, &update.resets_at_ms)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexCredits {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<Observed<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CodexQuotaSnapshot {
    pub primary: CodexQuotaWindow,
    pub secondary: CodexQuotaWindow,
    pub plan_type: Option<Observed<String>>,
    pub credits: Option<Observed<CodexCredits>>,
}

impl CodexQuotaSnapshot {
    pub(crate) fn merge(&mut self, update: &Self) -> bool {
        let mut changed = self.primary.merge(&update.primary)
            | self.secondary.merge(&update.secondary)
            | merge(&mut self.plan_type, &update.plan_type);
        if let Some(incoming) = &update.credits {
            match &mut self.credits {
                Some(current) => {
                    if incoming.observed_at_ms > current.observed_at_ms {
                        current.value.has_credits = incoming.value.has_credits;
                        current.value.unlimited = incoming.value.unlimited;
                        current.observed_at_ms = incoming.observed_at_ms;
                        changed = true;
                    }
                    changed |= merge(&mut current.value.balance, &incoming.value.balance);
                }
                None => {
                    self.credits = Some(incoming.clone());
                    changed = true;
                }
            }
        }
        changed
    }

    pub(crate) fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self
                .plan_type
                .as_ref()
                .map_or(0, |value| value.value.capacity())
            + self
                .credits
                .as_ref()
                .and_then(|value| value.value.balance.as_ref())
                .map_or(0, |balance| balance.value.capacity())
    }

    pub(crate) fn weekly_observation(
        &self,
    ) -> Option<crate::domain::sharing::quota_cycle::QuotaWindowObservation> {
        use crate::domain::sharing::quota_cycle::{QuotaTimeBasis, QuotaWindowObservation};
        let mut windows = [&self.primary, &self.secondary]
            .into_iter()
            .filter_map(|window| {
                let duration = window.window_seconds.as_ref()?;
                let reset = window.resets_at_ms.as_ref()?;
                (duration.value == 604_800 && duration.observed_at_ms == reset.observed_at_ms)
                    .then_some(reset)
            });
        let reset = windows.next()?;
        if windows.next().is_some() {
            return None;
        }
        let observation = QuotaWindowObservation {
            bucket_id: "shared_weekly".into(),
            starts_at_ms: None,
            ends_at_ms: reset.value,
            duration_ms: Some(604_800_000),
            observed_at_ms: reset.observed_at_ms,
            basis: QuotaTimeBasis::DerivedFromWindow,
        };
        observation.start().ok()?;
        Some(observation)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexPassiveQuota {
    pub auth_identity_generation: u64,
    pub snapshot: CodexQuotaSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CodexQuotaTarget {
    pub(crate) provider_key: crate::domain::providers::registry::ProviderKey,
    pub(crate) provider_revision: u64,
    pub(crate) account_id: String,
    pub(crate) account_incarnation: u64,
    pub(crate) auth_identity_generation: u64,
    pub(crate) token_refresh_generation: u64,
}

impl super::store::AccountStore {
    pub(crate) fn record_codex_passive_quota(
        &mut self,
        target: &CodexQuotaTarget,
        update: &CodexQuotaSnapshot,
    ) -> bool {
        use crate::domain::providers::model::ProviderType;
        if self.account_incarnation(&target.account_id) != Some(target.account_incarnation) {
            return false;
        }
        let Some(account) = self.accounts.iter_mut().find(|account| {
            account.id == target.account_id
                && account.provider_type == ProviderType::CodexOAuth
                && account.auth_identity_generation == target.auth_identity_generation
                && account.token_refresh_generation == target.token_refresh_generation
        }) else {
            return false;
        };
        let mut update = update.clone();
        // A completed authoritative query is more recent than older passive
        // fields. Partial frames do not clear its quota snapshot or health.
        let after = account.quota_refreshed_at.unwrap_or(-1);
        macro_rules! fresh {
            ($field:expr) => {
                if $field
                    .as_ref()
                    .is_some_and(|value| value.observed_at_ms <= after)
                {
                    $field = None;
                }
            };
        }
        for window in [&mut update.primary, &mut update.secondary] {
            fresh!(window.used_percent);
            fresh!(window.window_seconds);
            fresh!(window.resets_at_ms);
        }
        fresh!(update.plan_type);
        fresh!(update.credits);
        if let Some(credits) = &mut update.credits {
            fresh!(credits.value.balance);
        }
        if update.is_empty() {
            return false;
        }
        let snapshot = account
            .codex_passive_quota
            .get_or_insert_with(|| CodexPassiveQuota {
                auth_identity_generation: target.auth_identity_generation,
                snapshot: Default::default(),
            });
        if snapshot.auth_identity_generation != target.auth_identity_generation {
            *snapshot = CodexPassiveQuota {
                auth_identity_generation: target.auth_identity_generation,
                snapshot: Default::default(),
            };
        }
        snapshot.snapshot.merge(&update)
    }
}

fn integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

fn decimal(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|value| value.is_finite())
}

fn text(value: &Value, max: usize) -> Option<String> {
    let text = match value {
        Value::String(value) => value.trim().to_string(),
        Value::Number(value) => value.to_string(),
        _ => return None,
    };
    (!text.is_empty() && text.len() <= max && !text.chars().any(char::is_control)).then_some(text)
}

fn stamped<T>(value: T, observed_at_ms: i64) -> Observed<T> {
    Observed {
        value,
        observed_at_ms,
    }
}

fn window(value: &Value, at: i64) -> CodexQuotaWindow {
    let seconds = value
        .get("limit_window_seconds")
        .and_then(integer)
        .or_else(|| {
            value
                .get("window_minutes")
                .and_then(integer)?
                .checked_mul(60)
        })
        .filter(|seconds| *seconds > 0 && *seconds <= 604_800);
    // Invalid absolute values cannot overflow or become an unbounded deadline.
    let absolute = value
        .get("reset_at")
        .or_else(|| value.get("resets_at"))
        .and_then(integer)
        .and_then(|seconds| seconds.checked_mul(1000));
    let relative = value
        .get("reset_after_seconds")
        .or_else(|| value.get("resets_in_seconds"))
        .and_then(integer)
        .filter(|seconds| *seconds >= 0 && *seconds <= 604_800)
        .and_then(|seconds| seconds.checked_mul(1000))
        .and_then(|delta| at.checked_add(delta));
    let reset = absolute.or(relative).filter(|reset| {
        *reset >= at.saturating_sub(300_000) && *reset <= at.saturating_add(604_800_000)
    });
    CodexQuotaWindow {
        used_percent: value
            .get("used_percent")
            .and_then(decimal)
            .filter(|value| (0.0..=100.0).contains(value))
            .map(|value| stamped(value, at)),
        window_seconds: seconds.map(|value| stamped(value, at)),
        resets_at_ms: reset.map(|value| stamped(value, at)),
    }
}

/// The proxy separately reviews the envelope and size. Only default buckets
/// can update these fields; every explicit bucket selector must agree.
pub(crate) fn frame_snapshot(value: &Value, at: i64) -> Option<CodexQuotaSnapshot> {
    if at < 0
        || ["metered_limit_name", "limit_id", "limit_name"]
            .iter()
            .any(|key| {
                value
                    .get(key)
                    .filter(|value| !value.is_null())
                    .is_some_and(|value| value.as_str() != Some("codex"))
            })
    {
        return None;
    }
    let snapshot = match value.get("type").and_then(Value::as_str)? {
        "codex.rate_limits" | "rate_limits.updated" => {
            // Multiple dialects in a frame have no reviewed precedence.
            if value
                .get("rate_limit")
                .is_some_and(|value| !value.is_null())
                && value
                    .get("rate_limits")
                    .is_some_and(|value| !value.is_null())
            {
                return None;
            }
            let rate = value.get("rate_limits").filter(|value| value.is_object());
            let legacy = value.get("rate_limit").filter(|value| value.is_object());
            let get_window = |name: &str| {
                rate.and_then(|value| value.get(name))
                    .or_else(|| legacy?.get(format!("{name}_window")))
                    .map(|value| window(value, at))
                    .unwrap_or_default()
            };
            let credits = value.get("credits").and_then(|value| {
                Some(CodexCredits {
                    has_credits: value.get("has_credits")?.as_bool()?,
                    unlimited: value.get("unlimited")?.as_bool()?,
                    balance: value
                        .get("balance")
                        .and_then(|value| text(value, 128))
                        .map(|value| stamped(value, at)),
                })
            });
            CodexQuotaSnapshot {
                primary: get_window("primary"),
                secondary: get_window("secondary"),
                plan_type: value
                    .get("plan_type")
                    .and_then(|value| text(value, 32))
                    .map(|value| stamped(value, at)),
                credits: credits.map(|value| stamped(value, at)),
            }
        }
        "response.metadata" | "codex.response.metadata" => {
            let headers = value.get("headers")?.as_object()?;
            let mut normalized = serde_json::Map::new();
            for (key, value) in headers {
                let key = key.to_ascii_lowercase();
                if !quota_header(&key) {
                    continue;
                }
                let value = value
                    .as_array()
                    .and_then(|values| values.first())
                    .unwrap_or(value);
                if normalized.insert(key, value.clone()).is_some() {
                    return None;
                }
            }
            let get_window = |name: &str| {
                let field = |suffix: &str| {
                    normalized
                        .get(&format!("x-codex-{name}-{suffix}"))
                        .cloned()
                        .unwrap_or(Value::Null)
                };
                window(
                    &serde_json::json!({"used_percent":field("used-percent"),
                    "window_minutes":field("window-minutes"), "reset_at":field("reset-at"),
                    "reset_after_seconds":field("reset-after-seconds")}),
                    at,
                )
            };
            let boolean = |name: &str| {
                normalized.get(name).and_then(|value| {
                    value.as_bool().or_else(|| match value.as_str()? {
                        "true" => Some(true),
                        "false" => Some(false),
                        _ => None,
                    })
                })
            };
            let credits = boolean("x-codex-credits-has-credits")
                .zip(boolean("x-codex-credits-unlimited"))
                .map(|(has_credits, unlimited)| CodexCredits {
                    has_credits,
                    unlimited,
                    balance: normalized
                        .get("x-codex-credits-balance")
                        .and_then(|value| text(value, 128))
                        .map(|value| stamped(value, at)),
                });
            CodexQuotaSnapshot {
                primary: get_window("primary"),
                secondary: get_window("secondary"),
                plan_type: normalized
                    .get("x-codex-plan-type")
                    .and_then(|value| text(value, 32))
                    .map(|value| stamped(value, at)),
                credits: credits.map(|value| stamped(value, at)),
            }
        }
        _ => return None,
    };
    (!snapshot.is_empty()).then_some(snapshot)
}

pub(crate) fn quota_header(key: &str) -> bool {
    matches!(
        key,
        "x-codex-plan-type"
            | "x-codex-credits-has-credits"
            | "x-codex-credits-unlimited"
            | "x-codex-credits-balance"
    ) || ["primary", "secondary"].iter().any(|window| {
        [
            "used-percent",
            "window-minutes",
            "reset-at",
            "reset-after-seconds",
        ]
        .iter()
        .any(|field| key == format!("x-codex-{window}-{field}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_r4_sparse_credit_balance_keeps_its_own_observation_time() {
        let credits = |at, value| {
            frame_snapshot(&json!({"type":"codex.rate_limits","credits":value}), at).unwrap()
        };
        let mut snapshot = credits(
            100,
            json!({"has_credits":false,"unlimited":false,"balance":"0"}),
        );
        snapshot.merge(&credits(300, json!({"has_credits":true,"unlimited":false})));
        let current = snapshot.credits.as_ref().unwrap();
        assert!(current.value.has_credits);
        assert_eq!(current.value.balance.as_ref().unwrap().observed_at_ms, 100);
        snapshot.merge(&credits(
            200,
            json!({"has_credits":false,"unlimited":false,"balance":"1"}),
        ));
        let current = snapshot.credits.as_ref().unwrap();
        assert!(current.value.has_credits);
        assert_eq!(current.observed_at_ms, 300);
        assert_eq!(current.value.balance.as_ref().unwrap().value, "1");
        assert_eq!(current.value.balance.as_ref().unwrap().observed_at_ms, 200);
    }

    #[test]
    fn codex_r4_quota_identity_time_fences_and_persisted_snapshots_survive_restart() {
        use super::super::store::{AccountStore, UpsertAccountInput};
        use crate::domain::providers::{model::AppKind, registry::ProviderKey};
        let mut store = AccountStore::default();
        let input: UpsertAccountInput = serde_json::from_value(json!({
            "id":"quota-fence", "providerType":"codex_oauth", "accessToken":"fixture-token", "scopes":[]
        })).unwrap();
        let account = store.upsert(input.clone());
        let mut target = CodexQuotaTarget {
            provider_key: ProviderKey::new(AppKind::Codex, "p").unwrap(),
            provider_revision: 1,
            account_id: account.id.clone(),
            account_incarnation: store.account_incarnation(&account.id).unwrap(),
            auth_identity_generation: account.auth_identity_generation,
            token_refresh_generation: account.token_refresh_generation,
        };
        let update = frame_snapshot(
            &json!({"type":"codex.rate_limits","rate_limits":{"primary":{"used_percent":0}},
            "credits":{"has_credits":false,"unlimited":false,"balance":"0"}}),
            200,
        )
        .unwrap();
        assert!(store.record_codex_passive_quota(&target, &update));
        assert!(!store.record_codex_passive_quota(&target, &update));
        store.accounts[0].quota_refreshed_at = Some(300);
        assert!(!store.record_codex_passive_quota(&target, &update));
        let mut refreshed = input.clone();
        refreshed.access_token = Some("fixture-next-token".into());
        let next = store.upsert(refreshed);
        let newer = frame_snapshot(
            &json!({"type":"codex.rate_limits","rate_limits":{"secondary":{"used_percent":20}}}),
            400,
        )
        .unwrap();
        assert!(!store.record_codex_passive_quota(&target, &newer));
        target.token_refresh_generation = next.token_refresh_generation;
        assert!(store.record_codex_passive_quota(&target, &newer));
        assert!(store.accounts[0].rate_limited_until.is_none());
        assert!(store.accounts[0].quota.is_none());
        let dir = std::env::temp_dir().join(format!(
            "codex-r4-passive-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        store.save(&dir).unwrap();
        let reloaded = AccountStore::load_or_default(&dir).unwrap();
        assert_eq!(
            store.accounts[0].codex_passive_quota,
            reloaded.accounts[0].codex_passive_quota
        );
        assert_eq!(
            reloaded.accounts[0]
                .codex_passive_quota
                .as_ref()
                .unwrap()
                .snapshot
                .credits
                .as_ref()
                .unwrap()
                .value
                .balance
                .as_ref()
                .map(|balance| balance.value.as_str()),
            Some("0")
        );
        std::fs::remove_dir_all(dir).unwrap();
        store.delete(&account.id);
        store.upsert(input);
        assert!(!store.record_codex_passive_quota(&target, &newer));
    }

    #[test]
    fn codex_r4_quota_dialects_sparse_zero_false_buckets_and_reset_bounds() {
        let at = 1_800_000_000_000i64;
        let frame = json!({"type":"codex.rate_limits","rate_limits":{"primary":{"used_percent":0,"window_minutes":300,"reset_at":(at+100_000)/1000},
            "secondary":{"used_percent":34,"window_minutes":10080,"resets_in_seconds":200}},
            "plan_type":"pro","credits":{"has_credits":false,"unlimited":false,"balance":"0"}});
        let mut snapshot = frame_snapshot(&frame, at).unwrap();
        assert_eq!(snapshot.primary.used_percent.as_ref().unwrap().value, 0.0);
        assert!(!snapshot.credits.as_ref().unwrap().value.has_credits);
        assert_eq!(
            snapshot.weekly_observation().unwrap().ends_at_ms,
            at + 200_000
        );
        for selector in ["limit_id", "limit_name", "metered_limit_name"] {
            let mut metered = frame.clone();
            metered[selector] = json!("spark");
            assert!(frame_snapshot(&metered, at).is_none());
        }
        let sparse = frame_snapshot(&json!({"type":"rate_limits.updated","rate_limit":{"primary_window":{"used_percent":5,"limit_window_seconds":18000,"reset_after_seconds":10}},
            "credits":{"has_credits":true}}), at + 1).unwrap();
        assert!(snapshot.merge(&sparse));
        assert_eq!(snapshot.primary.used_percent.as_ref().unwrap().value, 5.0);
        assert_eq!(
            snapshot.secondary.used_percent.as_ref().unwrap().value,
            34.0
        );
        assert!(!snapshot.credits.as_ref().unwrap().value.has_credits);
        assert!(!snapshot.merge(&sparse));
        snapshot.merge(&frame_snapshot(&frame, at - 1).unwrap());
        assert_eq!(snapshot.primary.used_percent.as_ref().unwrap().value, 5.0);
        for reset in [json!(i64::MAX), json!(-1), json!(1.25), json!("9e999")] {
            let mut invalid = frame.clone();
            invalid["rate_limits"]["primary"]["reset_at"] = reset;
            assert!(frame_snapshot(&invalid, at)
                .unwrap()
                .primary
                .resets_at_ms
                .is_none());
        }
        let headers = frame_snapshot(
            &json!({"type":"codex.response.metadata","headers":{
            "X-Codex-Primary-Used-Percent":["0"],"x-codex-primary-window-minutes":"300",
            "x-codex-primary-reset-at":(at+50_000)/1000,"x-codex-primary-reset-after-seconds":100,
            "x-codex-credits-has-credits":false,"x-codex-credits-unlimited":"false",
            "authorization":"private-ignored","x-models-etag":"private-ignored"}}),
            at,
        )
        .unwrap();
        assert_eq!(headers.primary.used_percent.unwrap().value, 0.0);
        assert_eq!(headers.primary.resets_at_ms.unwrap().value, at + 50_000);
        assert!(!serde_json::to_string(&headers.credits)
            .unwrap()
            .contains("private-ignored"));
    }
}
