#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrokSubscriptionPlan {
    Free,
    SuperGrokLite,
    SuperGrok,
    SuperGrokPlus,
    SuperGrokHeavy,
    Business,
    Enterprise,
}

impl GrokSubscriptionPlan {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::SuperGrokLite => "SuperGrok Lite",
            Self::SuperGrok => "SuperGrok",
            Self::SuperGrokPlus => "SuperGrok Plus",
            Self::SuperGrokHeavy => "SuperGrok Heavy",
            Self::Business => "Business",
            Self::Enterprise => "Enterprise",
        }
    }
}

pub fn parse_grok_subscription_plan(value: &str) -> Option<GrokSubscriptionPlan> {
    let key = value
        .trim()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect::<String>();
    match key.as_str() {
        "free" => Some(GrokSubscriptionPlan::Free),
        "supergroklite" => Some(GrokSubscriptionPlan::SuperGrokLite),
        // GrokPro is an upstream/legacy alias for the base SuperGrok plan.
        "grokpro" | "supergrok" => Some(GrokSubscriptionPlan::SuperGrok),
        "supergrokplus" => Some(GrokSubscriptionPlan::SuperGrokPlus),
        "supergrokheavy" => Some(GrokSubscriptionPlan::SuperGrokHeavy),
        "business" => Some(GrokSubscriptionPlan::Business),
        "enterprise" => Some(GrokSubscriptionPlan::Enterprise),
        _ => None,
    }
}

pub fn canonical_grok_subscription_level(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(
        parse_grok_subscription_plan(value)
            .map(GrokSubscriptionPlan::label)
            .unwrap_or(value)
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_public_grok_plan_labels() {
        for (raw, expected) in [
            ("free", "Free"),
            ("supergrok-lite", "SuperGrok Lite"),
            ("super_grok", "SuperGrok"),
            ("SUPERGROK PLUS", "SuperGrok Plus"),
            ("SuperGrokHeavy", "SuperGrok Heavy"),
            ("business", "Business"),
            ("ENTERPRISE", "Enterprise"),
        ] {
            assert_eq!(
                canonical_grok_subscription_level(raw).as_deref(),
                Some(expected),
                "{raw}"
            );
        }
    }

    #[test]
    fn maps_grokpro_to_base_supergrok_without_guessing_unknown_tiers() {
        for raw in ["GrokPro", "grok_pro", "GROK-PRO"] {
            assert_eq!(
                canonical_grok_subscription_level(raw).as_deref(),
                Some("SuperGrok"),
                "{raw}"
            );
        }
        assert_eq!(
            canonical_grok_subscription_level("SuperGrokPro").as_deref(),
            Some("SuperGrokPro")
        );
        assert_eq!(
            canonical_grok_subscription_level("future-tier").as_deref(),
            Some("future-tier")
        );
    }
}
