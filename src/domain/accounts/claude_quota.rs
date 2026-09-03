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
}

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
    let Some(quota) = project_account_quota(account, quota, now_ms) else {
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
    ClaudeQuotaVisualFingerprint { tiers }
}

pub fn claude_quota_visual_change_is_urgent(
    before: &ClaudeQuotaVisualFingerprint,
    after: &ClaudeQuotaVisualFingerprint,
) -> bool {
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
        .unwrap_or(false);
    if !stale
        && !conflict
        && resolved_plan
            .is_some_and(|plan| plan.fable_eligibility() == ClaudeFableEligibility::Eligible)
    {
        return ClaudeFableEligibility::Eligible;
    }
    if observation.fable_entitlement_evidence.is_some() {
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
        assert!(
            project_account_quota(&account, account.quota.as_ref(), 20_001)
                .unwrap()
                .tiers
                .iter()
                .all(|tier| tier.name != CLAUDE_FABLE_QUOTA_TIER)
        );
    }
}
