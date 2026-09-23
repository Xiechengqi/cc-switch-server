use serde_json::{Map, Value};

use crate::domain::accounts::claude_subscription::{
    parse_claude_subscription_plan, ClaudeFableEligibility, CLAUDE_FABLE_MODEL_FAMILY,
};
use crate::domain::accounts::store::{
    active_account_quota_window_observation, Account, AccountQuota, AccountQuotaTier,
    AccountQuotaWindowObservation, CLAUDE_FABLE_CAPACITY_POOL, CLAUDE_FABLE_QUOTA_TIER,
    CLAUDE_FABLE_RELATIVE_WEEKLY_CAPACITY, CLAUDE_FIVE_HOUR_QUOTA_TIER,
    CLAUDE_SEVEN_DAY_QUOTA_TIER,
};
use crate::domain::providers::model::ProviderType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeQuotaVisualTierFingerprint {
    pub name: String,
    pub utilization_percent: i16,
    pub resets_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeQuotaVisualFingerprint {
    pub tiers: Vec<ClaudeQuotaVisualTierFingerprint>,
    pub unobserved_tiers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountQuotaTierHint {
    pub name: String,
    pub label: String,
    pub scope: String,
    pub capacity_pool: String,
    pub model_family: Option<String>,
    pub relative_weekly_capacity: Option<f64>,
    pub source: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountQuotaProjection {
    pub quota: Option<AccountQuota>,
    pub unobserved_tiers: Vec<AccountQuotaTierHint>,
}

pub const CLAUDE_SUBSCRIPTION_PLAN_QUOTA_SOURCE: &str = "claude_subscription_plan";
pub const AWAITING_UPSTREAM_QUOTA_OBSERVATION_REASON: &str = "awaiting_upstream_observation";

pub fn project_account_quota(
    account: &Account,
    quota: Option<&AccountQuota>,
    now_ms: i64,
) -> Option<AccountQuota> {
    let mut quota = quota.cloned()?;
    if account.provider_type != ProviderType::ClaudeOAuth {
        return Some(quota);
    }

    let mut contributed_at_ms = None;
    for tier_name in [
        CLAUDE_FIVE_HOUR_QUOTA_TIER,
        CLAUDE_SEVEN_DAY_QUOTA_TIER,
        CLAUDE_FABLE_QUOTA_TIER,
    ] {
        if quota.tiers.iter().any(|tier| tier.name == tier_name) {
            continue;
        }
        let Some(observation) = active_account_quota_window_observation(account, tier_name, now_ms)
        else {
            continue;
        };
        let Some(utilization) = observation.utilization else {
            continue;
        };
        if tier_name == CLAUDE_FABLE_QUOTA_TIER
            && claude_fable_observation_eligibility(account, &quota, observation)
                != ClaudeFableEligibility::Eligible
        {
            continue;
        }
        insert_projected_tier(
            &mut quota.tiers,
            projected_tier(tier_name, utilization, observation),
        );
        contributed_at_ms = Some(
            contributed_at_ms
                .unwrap_or(i64::MIN)
                .max(observation.observed_at_ms),
        );
    }

    if let Some(observed_at_ms) = contributed_at_ms {
        update_projected_queried_at(&mut quota, observed_at_ms);
    }
    Some(quota)
}

pub fn project_account_quota_presentation(
    account: &Account,
    quota: Option<&AccountQuota>,
    now_ms: i64,
) -> AccountQuotaProjection {
    let quota = project_account_quota(account, quota, now_ms);
    let unobserved_tiers = quota
        .as_ref()
        .filter(|quota| {
            account.provider_type == ProviderType::ClaudeOAuth
                && claude_fable_plan_eligibility(account, quota) == ClaudeFableEligibility::Eligible
                && !quota.tiers.iter().any(is_fable_quota_tier)
        })
        .map(|_| vec![unobserved_fable_quota_tier()])
        .unwrap_or_default();
    AccountQuotaProjection {
        quota,
        unobserved_tiers,
    }
}

fn is_fable_quota_tier(tier: &AccountQuotaTier) -> bool {
    tier.name == CLAUDE_FABLE_QUOTA_TIER
        || tier.capacity_pool.as_deref() == Some(CLAUDE_FABLE_CAPACITY_POOL)
}

fn unobserved_fable_quota_tier() -> AccountQuotaTierHint {
    AccountQuotaTierHint {
        name: CLAUDE_FABLE_QUOTA_TIER.to_string(),
        label: "Fable 7d".to_string(),
        scope: "model_family".to_string(),
        capacity_pool: CLAUDE_FABLE_CAPACITY_POOL.to_string(),
        model_family: Some(CLAUDE_FABLE_MODEL_FAMILY.to_string()),
        relative_weekly_capacity: Some(CLAUDE_FABLE_RELATIVE_WEEKLY_CAPACITY),
        source: CLAUDE_SUBSCRIPTION_PLAN_QUOTA_SOURCE.to_string(),
        reason: AWAITING_UPSTREAM_QUOTA_OBSERVATION_REASON.to_string(),
    }
}

pub fn projected_quota_queried_at(quota: &AccountQuota, fallback: Option<i64>) -> Option<i64> {
    quota
        .extra_usage
        .as_ref()
        .and_then(|extra| extra.get("queriedAt"))
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .or(fallback)
}

pub fn claude_quota_visual_fingerprint(
    account: &Account,
    quota: Option<&AccountQuota>,
    now_ms: i64,
) -> ClaudeQuotaVisualFingerprint {
    let projection = project_account_quota_presentation(account, quota, now_ms);
    let Some(quota) = projection.quota else {
        return ClaudeQuotaVisualFingerprint::default();
    };
    let mut tiers = quota
        .tiers
        .iter()
        .filter(|tier| {
            matches!(
                tier.name.as_str(),
                CLAUDE_FIVE_HOUR_QUOTA_TIER | CLAUDE_SEVEN_DAY_QUOTA_TIER | CLAUDE_FABLE_QUOTA_TIER
            )
        })
        .filter_map(|tier| {
            let utilization = tier.utilization?;
            utilization
                .is_finite()
                .then(|| ClaudeQuotaVisualTierFingerprint {
                    name: tier.name.clone(),
                    utilization_percent: (utilization.clamp(0.0, 1.0) * 100.0).round() as i16,
                    resets_at_ms: tier.resets_at,
                })
        })
        .collect::<Vec<_>>();
    tiers.sort_by_key(|tier| tier_order(&tier.name));
    let mut unobserved_tiers = projection
        .unobserved_tiers
        .into_iter()
        .map(|tier| tier.name)
        .collect::<Vec<_>>();
    unobserved_tiers.sort();
    ClaudeQuotaVisualFingerprint {
        tiers,
        unobserved_tiers,
    }
}

pub fn claude_quota_visual_change_is_urgent(
    before: &ClaudeQuotaVisualFingerprint,
    after: &ClaudeQuotaVisualFingerprint,
) -> bool {
    if before.unobserved_tiers != after.unobserved_tiers {
        return true;
    }
    if before.tiers.is_empty() != after.tiers.is_empty() {
        return true;
    }
    for tier_name in [
        CLAUDE_FIVE_HOUR_QUOTA_TIER,
        CLAUDE_SEVEN_DAY_QUOTA_TIER,
        CLAUDE_FABLE_QUOTA_TIER,
    ] {
        let before_tier = before.tiers.iter().find(|tier| tier.name == tier_name);
        let after_tier = after.tiers.iter().find(|tier| tier.name == tier_name);
        match (before_tier, after_tier) {
            (None, None) => {}
            (None, Some(_)) | (Some(_), None) => return true,
            (Some(before_tier), Some(after_tier)) => {
                if before_tier.resets_at_ms != after_tier.resets_at_ms
                    || (before_tier.utilization_percent < 100
                        && after_tier.utilization_percent >= 100)
                {
                    return true;
                }
            }
        }
    }
    false
}

fn projected_tier(
    tier_name: &str,
    utilization: f64,
    observation: &AccountQuotaWindowObservation,
) -> AccountQuotaTier {
    let is_fable = tier_name == CLAUDE_FABLE_QUOTA_TIER;
    AccountQuotaTier {
        name: tier_name.to_string(),
        label: None,
        utilization: Some(utilization.clamp(0.0, 1.0)),
        used: None,
        limit: None,
        unit: Some("percent".to_string()),
        resets_at: observation.resets_at_ms,
        scope: is_fable.then(|| "model_family".to_string()),
        capacity_pool: is_fable.then(|| CLAUDE_FABLE_CAPACITY_POOL.to_string()),
        model_family: is_fable.then(|| CLAUDE_FABLE_MODEL_FAMILY.to_string()),
        relative_weekly_capacity: is_fable.then_some(CLAUDE_FABLE_RELATIVE_WEEKLY_CAPACITY),
        source: Some(observation.source.clone()),
    }
}

fn insert_projected_tier(tiers: &mut Vec<AccountQuotaTier>, tier: AccountQuotaTier) {
    let insert_at = match tier.name.as_str() {
        CLAUDE_FIVE_HOUR_QUOTA_TIER => 0,
        CLAUDE_SEVEN_DAY_QUOTA_TIER => tiers
            .iter()
            .position(|candidate| candidate.name == CLAUDE_FIVE_HOUR_QUOTA_TIER)
            .map(|index| index + 1)
            .unwrap_or(0),
        CLAUDE_FABLE_QUOTA_TIER => tiers
            .iter()
            .rposition(|candidate| {
                matches!(
                    candidate.name.as_str(),
                    CLAUDE_FIVE_HOUR_QUOTA_TIER | CLAUDE_SEVEN_DAY_QUOTA_TIER
                )
            })
            .map(|index| index + 1)
            .unwrap_or(tiers.len()),
        _ => tiers.len(),
    };
    tiers.insert(insert_at, tier);
}

fn update_projected_queried_at(quota: &mut AccountQuota, observed_at_ms: i64) {
    if quota.extra_usage.is_none() || quota.extra_usage.as_ref().is_some_and(Value::is_null) {
        quota.extra_usage = Some(Value::Object(Map::new()));
    }
    let Some(extra) = quota.extra_usage.as_mut().and_then(Value::as_object_mut) else {
        return;
    };
    let queried_at = extra
        .get("queriedAt")
        .and_then(Value::as_i64)
        .unwrap_or(i64::MIN)
        .max(observed_at_ms);
    extra.insert("queriedAt".to_string(), Value::from(queried_at));
}

fn claude_fable_observation_eligibility(
    account: &Account,
    quota: &AccountQuota,
    observation: &AccountQuotaWindowObservation,
) -> ClaudeFableEligibility {
    match claude_fable_plan_eligibility(account, quota) {
        ClaudeFableEligibility::Eligible => return ClaudeFableEligibility::Eligible,
        ClaudeFableEligibility::Ineligible => return ClaudeFableEligibility::Ineligible,
        ClaudeFableEligibility::Unknown => {}
    }
    if observation.fable_entitlement_evidence.is_some() {
        return ClaudeFableEligibility::Eligible;
    }
    ClaudeFableEligibility::Unknown
}

fn claude_fable_plan_eligibility(
    account: &Account,
    quota: &AccountQuota,
) -> ClaudeFableEligibility {
    if !quota.success {
        return ClaudeFableEligibility::Unknown;
    }
    let subscription = quota
        .extra_usage
        .as_ref()
        .and_then(|extra| extra.get("subscription"));
    let subscription_evidence = quota
        .extra_usage
        .as_ref()
        .and_then(|extra| extra.get("subscriptionEvidence"));

    let resolved_plan = subscription.and_then(|subscription| {
        ["planType", "planLabel"].into_iter().find_map(|key| {
            subscription
                .get(key)
                .and_then(Value::as_str)
                .and_then(parse_claude_subscription_plan)
        })
    });
    let conflicting_ineligible = subscription_evidence
        .and_then(|evidence| evidence.get("conflictingPlanTypes"))
        .and_then(Value::as_array)
        .is_some_and(|plans| {
            plans.iter().any(|plan| {
                plan.as_str()
                    .and_then(parse_claude_subscription_plan)
                    .is_some_and(|plan| {
                        plan.fable_eligibility() == ClaudeFableEligibility::Ineligible
                    })
            })
        });
    if conflicting_ineligible
        || resolved_plan
            .is_some_and(|plan| plan.fable_eligibility() == ClaudeFableEligibility::Ineligible)
        || account
            .subscription_level
            .as_deref()
            .and_then(parse_claude_subscription_plan)
            .is_some_and(|plan| plan.fable_eligibility() == ClaudeFableEligibility::Ineligible)
    {
        return ClaudeFableEligibility::Ineligible;
    }

    let stale = subscription
        .and_then(|subscription| subscription.get("planStale"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let conflict = subscription_evidence
        .and_then(|evidence| evidence.get("conflict"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !stale
        && !conflict
        && resolved_plan
            .is_some_and(|plan| plan.fable_eligibility() == ClaudeFableEligibility::Eligible)
    {
        return ClaudeFableEligibility::Eligible;
    }
    ClaudeFableEligibility::Unknown
}

fn tier_order(name: &str) -> usize {
    match name {
        CLAUDE_FIVE_HOUR_QUOTA_TIER => 0,
        CLAUDE_SEVEN_DAY_QUOTA_TIER => 1,
        CLAUDE_FABLE_QUOTA_TIER => 2,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::domain::accounts::store::{
        ClaudeFableEntitlementEvidence, CLAUDE_RATELIMIT_7D_OI_SOURCE,
    };

    fn account(plan: &str, stale: bool, observation: AccountQuotaWindowObservation) -> Account {
        serde_json::from_value(json!({
            "id": "claude-account",
            "providerType": "claude_oauth",
            "authIdentityGeneration": 7,
            "subscriptionLevel": plan,
            "quota": {
                "success": true,
                "credentialMessage": plan,
                "tiers": [
                    {"name": "five_hour", "utilization": 0.03},
                    {"name": "seven_day", "utilization": 0.24}
                ],
                "extraUsage": {
                    "subscription": {
                        "planType": plan,
                        "planLabel": plan,
                        "planStale": stale
                    },
                    "subscriptionEvidence": {"conflict": false},
                    "queriedAt": 1_000
                }
            },
            "quotaWindowObservations": BTreeMap::from([(
                CLAUDE_FABLE_QUOTA_TIER.to_string(), observation
            )])
        }))
        .unwrap()
    }

    fn fable_observation(
        evidence: Option<ClaudeFableEntitlementEvidence>,
    ) -> AccountQuotaWindowObservation {
        AccountQuotaWindowObservation {
            utilization: Some(0.41),
            resets_at_ms: Some(20_000),
            observed_at_ms: 2_000,
            auth_identity_generation: 7,
            source: CLAUDE_RATELIMIT_7D_OI_SOURCE.to_string(),
            fable_entitlement_evidence: evidence,
        }
    }

    #[test]
    fn fills_missing_fable_tier_for_fresh_max_plan() {
        let account = account("claude_max_20x", false, fable_observation(None));
        let quota = project_account_quota(&account, account.quota.as_ref(), 3_000).unwrap();
        let fable = quota
            .tiers
            .iter()
            .find(|tier| tier.name == CLAUDE_FABLE_QUOTA_TIER)
            .unwrap();
        assert_eq!(fable.utilization, Some(0.41));
        assert_eq!(fable.scope.as_deref(), Some("model_family"));
        assert_eq!(
            fable.capacity_pool.as_deref(),
            Some(CLAUDE_FABLE_CAPACITY_POOL)
        );
        assert_eq!(
            fable.model_family.as_deref(),
            Some(CLAUDE_FABLE_MODEL_FAMILY)
        );
        assert_eq!(fable.relative_weekly_capacity, Some(0.5));
        assert_eq!(fable.source.as_deref(), Some(CLAUDE_RATELIMIT_7D_OI_SOURCE));
        assert_eq!(projected_quota_queried_at(&quota, None), Some(2_000));
    }

    #[test]
    fn fresh_specific_max_plan_projects_unobserved_fable_hint_without_zero_usage() {
        for plan in ["claude_max_5x", "claude_max_20x"] {
            let mut account = account(plan, false, fable_observation(None));
            account.quota_window_observations.clear();

            let projection =
                project_account_quota_presentation(&account, account.quota.as_ref(), 3_000);
            let quota = projection.quota.expect("projected quota");
            assert!(
                quota
                    .tiers
                    .iter()
                    .all(|tier| tier.name != CLAUDE_FABLE_QUOTA_TIER),
                "an unobserved pool must not become a numeric tier"
            );
            assert_eq!(projection.unobserved_tiers.len(), 1);
            let hint = &projection.unobserved_tiers[0];
            assert_eq!(hint.name, CLAUDE_FABLE_QUOTA_TIER);
            assert_eq!(hint.capacity_pool, CLAUDE_FABLE_CAPACITY_POOL);
            assert_eq!(
                hint.model_family.as_deref(),
                Some(CLAUDE_FABLE_MODEL_FAMILY)
            );
            assert_eq!(hint.relative_weekly_capacity, Some(0.5));
            assert_eq!(hint.source, CLAUDE_SUBSCRIPTION_PLAN_QUOTA_SOURCE);
            assert_eq!(hint.reason, AWAITING_UPSTREAM_QUOTA_OBSERVATION_REASON);
        }
    }

    #[test]
    fn unobserved_fable_hint_fails_closed_for_uncertain_or_ineligible_plans() {
        for (plan, stale) in [
            ("claude_max", false),
            ("claude_max_20x", true),
            ("claude_free", false),
            ("claude_pro", false),
            ("claude_team", false),
            ("claude_enterprise", false),
        ] {
            let mut account = account(plan, stale, fable_observation(None));
            account.quota_window_observations.clear();
            assert!(
                project_account_quota_presentation(&account, account.quota.as_ref(), 3_000)
                    .unobserved_tiers
                    .is_empty(),
                "unexpected Fable hint for plan={plan} stale={stale}"
            );
        }

        let mut conflict = account("claude_max_20x", false, fable_observation(None));
        conflict.quota_window_observations.clear();
        conflict
            .quota
            .as_mut()
            .unwrap()
            .extra_usage
            .as_mut()
            .unwrap()["subscriptionEvidence"]["conflict"] = json!(true);
        assert!(
            project_account_quota_presentation(&conflict, conflict.quota.as_ref(), 3_000)
                .unobserved_tiers
                .is_empty()
        );
    }

    #[test]
    fn direct_fable_success_can_lift_stale_max_but_not_explicit_pro() {
        let evidence = Some(ClaudeFableEntitlementEvidence::SuccessfulFableRequest);
        let max = account("claude_max_20x", true, fable_observation(evidence));
        assert!(project_account_quota(&max, max.quota.as_ref(), 3_000)
            .unwrap()
            .tiers
            .iter()
            .any(|tier| tier.name == CLAUDE_FABLE_QUOTA_TIER));

        let pro = account("claude_pro", false, fable_observation(evidence));
        assert!(project_account_quota(&pro, pro.quota.as_ref(), 3_000)
            .unwrap()
            .tiers
            .iter()
            .all(|tier| tier.name != CLAUDE_FABLE_QUOTA_TIER));
    }

    #[test]
    fn active_fable_tier_wins_and_expired_passive_tier_is_ignored() {
        let mut account = account("claude_max_20x", false, fable_observation(None));
        account
            .quota
            .as_mut()
            .unwrap()
            .tiers
            .push(AccountQuotaTier {
                name: CLAUDE_FABLE_QUOTA_TIER.to_string(),
                utilization: Some(0.12),
                ..Default::default()
            });
        let quota = project_account_quota(&account, account.quota.as_ref(), 3_000).unwrap();
        assert_eq!(
            quota
                .tiers
                .iter()
                .find(|tier| tier.name == CLAUDE_FABLE_QUOTA_TIER)
                .and_then(|tier| tier.utilization),
            Some(0.12)
        );

        account.quota.as_mut().unwrap().tiers.pop();
        let expired = project_account_quota_presentation(&account, account.quota.as_ref(), 20_001);
        assert!(expired
            .quota
            .unwrap()
            .tiers
            .iter()
            .all(|tier| tier.name != CLAUDE_FABLE_QUOTA_TIER));
        assert_eq!(expired.unobserved_tiers.len(), 1);
    }

    #[test]
    fn visual_fingerprint_treats_hint_transitions_as_urgent() {
        let mut account = account("claude_max_20x", false, fable_observation(None));
        account.quota_window_observations.clear();
        let with_hint = claude_quota_visual_fingerprint(&account, account.quota.as_ref(), 3_000);
        assert_eq!(with_hint.unobserved_tiers, [CLAUDE_FABLE_QUOTA_TIER]);

        account
            .quota
            .as_mut()
            .unwrap()
            .extra_usage
            .as_mut()
            .unwrap()["subscription"]["planStale"] = json!(true);
        let without_hint = claude_quota_visual_fingerprint(&account, account.quota.as_ref(), 3_000);
        assert!(without_hint.unobserved_tiers.is_empty());
        assert!(claude_quota_visual_change_is_urgent(
            &with_hint,
            &without_hint
        ));
    }
}
