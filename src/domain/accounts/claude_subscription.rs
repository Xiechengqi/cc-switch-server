#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaudeSubscriptionPlan {
    Free,
    Pro,
    Max,
    Max5x,
    Max20x,
    Team,
    Enterprise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeFableEligibility {
    Eligible,
    Ineligible,
    Unknown,
}

pub const CLAUDE_FABLE_MODEL_FAMILY: &str = "claude-fable-5";

pub fn is_claude_fable_5_model(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    normalized == CLAUDE_FABLE_MODEL_FAMILY
        || normalized
            .strip_prefix(CLAUDE_FABLE_MODEL_FAMILY)
            .is_some_and(|suffix| suffix.starts_with('-') || suffix.starts_with(':'))
}

impl ClaudeSubscriptionPlan {
    pub const fn fable_eligibility(self) -> ClaudeFableEligibility {
        match self {
            Self::Max5x | Self::Max20x => ClaudeFableEligibility::Eligible,
            Self::Free | Self::Pro | Self::Team | Self::Enterprise => {
                ClaudeFableEligibility::Ineligible
            }
            Self::Max => ClaudeFableEligibility::Unknown,
        }
    }
    pub const fn plan_type(self) -> &'static str {
        match self {
            Self::Free => "claude_free",
            Self::Pro => "claude_pro",
            Self::Max => "claude_max",
            Self::Max5x => "claude_max_5x",
            Self::Max20x => "claude_max_20x",
            Self::Team => "claude_team",
            Self::Enterprise => "claude_enterprise",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Free => "Claude Free",
            Self::Pro => "Claude Pro",
            Self::Max => "Claude Max",
            Self::Max5x => "Claude Max 5x",
            Self::Max20x => "Claude Max 20x",
            Self::Team => "Claude Team",
            Self::Enterprise => "Claude Enterprise",
        }
    }

    const fn family(self) -> ClaudeSubscriptionFamily {
        match self {
            Self::Free => ClaudeSubscriptionFamily::Free,
            Self::Pro => ClaudeSubscriptionFamily::Pro,
            Self::Max | Self::Max5x | Self::Max20x => ClaudeSubscriptionFamily::Max,
            Self::Team => ClaudeSubscriptionFamily::Team,
            Self::Enterprise => ClaudeSubscriptionFamily::Enterprise,
        }
    }

    const fn specificity(self) -> u8 {
        match self {
            Self::Max5x | Self::Max20x => 2,
            _ => 1,
        }
    }
}

impl ClaudeSubscriptionResolution {
    pub const fn fable_eligibility(&self) -> ClaudeFableEligibility {
        if self.conflict || self.stale {
            ClaudeFableEligibility::Unknown
        } else {
            self.plan.fable_eligibility()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeSubscriptionFamily {
    Free,
    Pro,
    Max,
    Team,
    Enterprise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaudeSubscriptionSource {
    UsageTier,
    UsagePlan,
    UsageSubscriptionType,
    BootstrapRateLimitTier,
    ProfileRateLimitTier,
    BootstrapOrganizationType,
    ProfileOrganizationType,
    CachedProfileRateLimitTier,
    CachedProfileOrganizationType,
    CachedSubscriptionLevel,
}

impl ClaudeSubscriptionSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UsageTier => "usage_tier",
            Self::UsagePlan => "usage_plan",
            Self::UsageSubscriptionType => "usage_subscription_type",
            Self::BootstrapRateLimitTier => "bootstrap_rate_limit_tier",
            Self::ProfileRateLimitTier => "profile_rate_limit_tier",
            Self::BootstrapOrganizationType => "bootstrap_organization_type",
            Self::ProfileOrganizationType => "profile_organization_type",
            Self::CachedProfileRateLimitTier => "cached_profile_rate_limit_tier",
            Self::CachedProfileOrganizationType => "cached_profile_organization_type",
            Self::CachedSubscriptionLevel => "cached_subscription_level",
        }
    }

    pub const fn is_fresh(self) -> bool {
        !matches!(
            self,
            Self::CachedProfileRateLimitTier
                | Self::CachedProfileOrganizationType
                | Self::CachedSubscriptionLevel
        )
    }

    const fn authority(self) -> u8 {
        match self {
            Self::UsageTier => 100,
            Self::UsagePlan => 95,
            Self::UsageSubscriptionType => 90,
            Self::BootstrapRateLimitTier => 85,
            Self::ProfileRateLimitTier => 80,
            Self::BootstrapOrganizationType => 70,
            Self::ProfileOrganizationType => 60,
            // `subscription_level` is the canonical result persisted by the
            // previous resolution. Raw cached profile fields may contain a
            // lower-authority value from the same refresh.
            Self::CachedSubscriptionLevel => 40,
            Self::CachedProfileRateLimitTier => 30,
            Self::CachedProfileOrganizationType => 20,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ClaudeSubscriptionCandidate<'a> {
    pub source: ClaudeSubscriptionSource,
    pub value: &'a str,
}

impl<'a> ClaudeSubscriptionCandidate<'a> {
    pub const fn new(source: ClaudeSubscriptionSource, value: &'a str) -> Self {
        Self { source, value }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeSubscriptionObservation {
    pub source: ClaudeSubscriptionSource,
    pub plan: ClaudeSubscriptionPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSubscriptionResolution {
    pub plan: ClaudeSubscriptionPlan,
    pub source: ClaudeSubscriptionSource,
    pub stale: bool,
    pub conflict: bool,
    pub conflicting_plan_types: Vec<&'static str>,
    pub observations: Vec<ClaudeSubscriptionObservation>,
}

pub fn resolve_claude_subscription<'a>(
    candidates: impl IntoIterator<Item = ClaudeSubscriptionCandidate<'a>>,
) -> Option<ClaudeSubscriptionResolution> {
    let observations = candidates
        .into_iter()
        .filter_map(|candidate| {
            parse_claude_subscription_plan(candidate.value).map(|plan| {
                ClaudeSubscriptionObservation {
                    source: candidate.source,
                    plan,
                }
            })
        })
        .collect::<Vec<_>>();
    if observations.is_empty() {
        return None;
    }

    let fresh = observations
        .iter()
        .copied()
        .filter(|observation| observation.source.is_fresh())
        .collect::<Vec<_>>();
    let cached = observations
        .iter()
        .copied()
        .filter(|observation| !observation.source.is_fresh())
        .collect::<Vec<_>>();

    let mut selected =
        select_preferred_observation(if fresh.is_empty() { &cached } else { &fresh })?;

    // A fresh generic family marker (for example `claude_max`) does not
    // invalidate a previously observed multiplier from the same family.
    if selected.plan.specificity() == 1 {
        if let Some(refinement) = cached
            .iter()
            .filter(|observation| observation.plan.family() == selected.plan.family())
            .filter(|observation| observation.plan.specificity() > selected.plan.specificity())
            .max_by_key(|observation| {
                (
                    observation.plan.specificity(),
                    observation.source.authority(),
                )
            })
        {
            selected = *refinement;
        }
    }

    let conflict = fresh.iter().enumerate().any(|(index, left)| {
        fresh
            .iter()
            .skip(index + 1)
            .any(|right| !plans_are_compatible(left.plan, right.plan))
    });
    let conflicting_plan_types = if conflict {
        let mut values = fresh
            .iter()
            .map(|observation| observation.plan.plan_type())
            .collect::<Vec<_>>();
        values.sort_unstable();
        values.dedup();
        values
    } else {
        Vec::new()
    };

    Some(ClaudeSubscriptionResolution {
        plan: selected.plan,
        source: selected.source,
        stale: !selected.source.is_fresh(),
        conflict,
        conflicting_plan_types,
        observations,
    })
}

pub fn parse_claude_subscription_plan(value: &str) -> Option<ClaudeSubscriptionPlan> {
    let key = normalize_plan_key(value);
    match key.as_str() {
        "default_claude_max_5x" | "claude_max_5x" | "max_5x" => Some(ClaudeSubscriptionPlan::Max5x),
        "default_claude_max_20x" | "claude_max_20x" | "max_20x" => {
            Some(ClaudeSubscriptionPlan::Max20x)
        }
        "default_claude_max" | "claude_max" | "max" => Some(ClaudeSubscriptionPlan::Max),
        "default_claude_pro" | "claude_pro" | "pro" => Some(ClaudeSubscriptionPlan::Pro),
        "default_claude_free" | "claude_free" | "free" => Some(ClaudeSubscriptionPlan::Free),
        "default_claude_team" | "claude_team" | "team" => Some(ClaudeSubscriptionPlan::Team),
        "default_claude_enterprise" | "claude_enterprise" | "enterprise" => {
            Some(ClaudeSubscriptionPlan::Enterprise)
        }
        _ => None,
    }
}

fn select_preferred_observation(
    observations: &[ClaudeSubscriptionObservation],
) -> Option<ClaudeSubscriptionObservation> {
    let authoritative = observations
        .iter()
        .max_by_key(|observation| observation.source.authority())?;
    observations
        .iter()
        .filter(|observation| observation.plan.family() == authoritative.plan.family())
        .max_by_key(|observation| {
            (
                observation.plan.specificity(),
                observation.source.authority(),
            )
        })
        .copied()
}

fn plans_are_compatible(left: ClaudeSubscriptionPlan, right: ClaudeSubscriptionPlan) -> bool {
    if left.family() != right.family() {
        return false;
    }
    left == right || left.specificity() == 1 || right.specificity() == 1
}

fn normalize_plan_key(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut last_was_separator = false;
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() {
            normalized.push(character.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator && !normalized.is_empty() {
            normalized.push('_');
            last_was_separator = true;
        }
    }
    while normalized.ends_with('_') {
        normalized.pop();
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(source: ClaudeSubscriptionSource, value: &str) -> ClaudeSubscriptionCandidate<'_> {
        ClaudeSubscriptionCandidate::new(source, value)
    }

    #[test]
    fn parses_max_multipliers_and_safe_labels() {
        for (value, expected, label) in [
            (
                "default_claude_max_5x",
                ClaudeSubscriptionPlan::Max5x,
                "Claude Max 5x",
            ),
            (
                "DEFAULT-CLAUDE-MAX-20X",
                ClaudeSubscriptionPlan::Max20x,
                "Claude Max 20x",
            ),
            (
                "Claude Max 20x",
                ClaudeSubscriptionPlan::Max20x,
                "Claude Max 20x",
            ),
        ] {
            let plan = parse_claude_subscription_plan(value).unwrap();
            assert_eq!(plan, expected);
            assert_eq!(plan.label(), label);
        }
    }

    #[test]
    fn ignores_non_plan_organization_and_rate_limit_markers() {
        for value in ["default_claude_ai", "tier-2", "premium", "claude_max_50x"] {
            assert_eq!(parse_claude_subscription_plan(value), None, "{value}");
        }
    }

    #[test]
    fn specific_fresh_multiplier_refines_generic_usage_plan() {
        let resolution = resolve_claude_subscription([
            candidate(ClaudeSubscriptionSource::UsagePlan, "claude_max"),
            candidate(
                ClaudeSubscriptionSource::BootstrapRateLimitTier,
                "default_claude_max_20x",
            ),
        ])
        .unwrap();

        assert_eq!(resolution.plan, ClaudeSubscriptionPlan::Max20x);
        assert_eq!(
            resolution.source,
            ClaudeSubscriptionSource::BootstrapRateLimitTier
        );
        assert!(!resolution.stale);
        assert!(!resolution.conflict);
    }

    #[test]
    fn higher_authority_usage_wins_incompatible_fresh_conflict() {
        let resolution = resolve_claude_subscription([
            candidate(ClaudeSubscriptionSource::UsageTier, "claude_pro"),
            candidate(
                ClaudeSubscriptionSource::BootstrapRateLimitTier,
                "default_claude_max_20x",
            ),
        ])
        .unwrap();

        assert_eq!(resolution.plan, ClaudeSubscriptionPlan::Pro);
        assert_eq!(resolution.source, ClaudeSubscriptionSource::UsageTier);
        assert!(resolution.conflict);
        assert_eq!(
            resolution.conflicting_plan_types,
            vec!["claude_max_20x", "claude_pro"]
        );
    }

    #[test]
    fn cached_multiplier_can_refine_compatible_fresh_family() {
        let resolution = resolve_claude_subscription([
            candidate(ClaudeSubscriptionSource::UsagePlan, "claude_max"),
            candidate(
                ClaudeSubscriptionSource::CachedProfileRateLimitTier,
                "default_claude_max_5x",
            ),
        ])
        .unwrap();

        assert_eq!(resolution.plan, ClaudeSubscriptionPlan::Max5x);
        assert!(resolution.stale);
        assert!(!resolution.conflict);
    }

    #[test]
    fn incompatible_cached_multiplier_never_overrides_fresh_family() {
        let resolution = resolve_claude_subscription([
            candidate(ClaudeSubscriptionSource::UsagePlan, "claude_pro"),
            candidate(
                ClaudeSubscriptionSource::CachedProfileRateLimitTier,
                "default_claude_max_20x",
            ),
        ])
        .unwrap();

        assert_eq!(resolution.plan, ClaudeSubscriptionPlan::Pro);
        assert!(!resolution.stale);
        assert!(!resolution.conflict);
    }

    #[test]
    fn cached_canonical_resolution_wins_conflicting_raw_profile_tier() {
        let resolution = resolve_claude_subscription([
            candidate(
                ClaudeSubscriptionSource::CachedProfileRateLimitTier,
                "default_claude_max_5x",
            ),
            candidate(
                ClaudeSubscriptionSource::CachedSubscriptionLevel,
                "Claude Max 20x",
            ),
        ])
        .unwrap();

        assert_eq!(resolution.plan, ClaudeSubscriptionPlan::Max20x);
        assert_eq!(
            resolution.source,
            ClaudeSubscriptionSource::CachedSubscriptionLevel
        );
        assert!(resolution.stale);
    }

    #[test]
    fn fable_eligibility_requires_a_fresh_specific_max_multiplier() {
        assert_eq!(
            ClaudeSubscriptionPlan::Max5x.fable_eligibility(),
            ClaudeFableEligibility::Eligible
        );
        assert_eq!(
            ClaudeSubscriptionPlan::Max20x.fable_eligibility(),
            ClaudeFableEligibility::Eligible
        );
        assert_eq!(
            ClaudeSubscriptionPlan::Pro.fable_eligibility(),
            ClaudeFableEligibility::Ineligible
        );
        assert_eq!(
            ClaudeSubscriptionPlan::Max.fable_eligibility(),
            ClaudeFableEligibility::Unknown
        );

        let stale = resolve_claude_subscription([candidate(
            ClaudeSubscriptionSource::CachedSubscriptionLevel,
            "Claude Max 20x",
        )])
        .unwrap();
        assert_eq!(stale.fable_eligibility(), ClaudeFableEligibility::Unknown);
    }
}
