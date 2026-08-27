use serde_json::Value;

use crate::domain::accounts::store::Account;

pub(crate) const DEFAULT_REGION: &str = "us-east-1";
pub(crate) const PROFILE_REGIONS: &[&str] = &["us-east-1", "eu-central-1"];
pub(crate) const BUILDER_ID_PROFILE_ARN: &str =
    "arn:aws:codewhisperer:us-east-1:638616132270:profile/AAAACCCCXXXX";
pub(crate) const SOCIAL_PROFILE_ARN: &str =
    "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK";
const LEGACY_ENTERPRISE_PROFILE_ACCOUNT_ID: &str = "610548660232";
const LEGACY_ENTERPRISE_PROFILE_ID: &str = "VNECVYCYYAWN";

pub(crate) const STATIC_MODEL_IDS: &[&str] = &[
    "claude-fable-5",
    "claude-haiku-4.5",
    "claude-opus-4.5",
    "claude-opus-4.6",
    "claude-opus-4.7",
    "claude-opus-4.8",
    "claude-opus-5",
    "claude-sonnet-4.5",
    "claude-sonnet-4.6",
    "claude-sonnet-4.8",
    "claude-sonnet-5",
    "gpt-5.6-luna",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
];

pub(crate) fn normalize_region(raw: &str) -> Option<String> {
    let region = raw.trim().to_ascii_lowercase();
    let mut parts = region.split('-');
    let prefix = parts.next()?;
    let locality = parts.next()?;
    let ordinal = parts.next()?;
    if parts.next().is_some()
        || prefix.len() != 2
        || !prefix.bytes().all(|byte| byte.is_ascii_lowercase())
        || locality.is_empty()
        || !locality.bytes().all(|byte| byte.is_ascii_lowercase())
        || ordinal.is_empty()
        || ordinal.len() > 2
        || !ordinal.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some(region)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KiroProfileArn {
    pub arn: String,
    pub region: String,
}

pub(crate) fn parse_profile_arn(raw: &str) -> Option<KiroProfileArn> {
    let arn = raw.trim();
    let mut parts = arn.splitn(6, ':');
    if parts.next()? != "arn" || parts.next()? != "aws" || parts.next()? != "codewhisperer" {
        return None;
    }
    let region = normalize_region(parts.next()?)?;
    let account_id = parts.next()?;
    if account_id.len() != 12 || !account_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let profile_id = parts.next()?.strip_prefix("profile/")?;
    if profile_id.is_empty()
        || profile_id.len() > 256
        || !profile_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    Some(KiroProfileArn {
        arn: arn.to_string(),
        region,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KiroRuntimeRegionSource {
    ProfileArn,
    RuntimeRegion,
    LegacyApiRegion,
    Default,
}

impl KiroRuntimeRegionSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ProfileArn => "profile_arn",
            Self::RuntimeRegion => "runtime_region",
            Self::LegacyApiRegion => "legacy_api_region",
            Self::Default => "default_region",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KiroRuntimeIdentity {
    pub profile_arn: Option<String>,
    pub runtime_region: String,
    pub region_source: KiroRuntimeRegionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KiroRuntimeIdentityError {
    InvalidProfileArn,
    InvalidRuntimeRegion,
    InvalidLegacyApiRegion,
    MissingEnterpriseProfileArn,
    UnresolvedEnterpriseProfileArn,
}

impl std::fmt::Display for KiroRuntimeIdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidProfileArn => "invalid Kiro profile ARN",
            Self::InvalidRuntimeRegion => "invalid Kiro runtime region",
            Self::InvalidLegacyApiRegion => "invalid legacy Kiro API region",
            Self::MissingEnterpriseProfileArn => {
                "Kiro enterprise account requires a discovered organization profile ARN"
            }
            Self::UnresolvedEnterpriseProfileArn => {
                "Kiro enterprise account still has a legacy shared fallback profile ARN"
            }
        })
    }
}

pub(crate) fn resolve_runtime_identity(
    profile_arn: Option<&str>,
    runtime_region: Option<&str>,
    legacy_api_region: Option<&str>,
) -> Result<KiroRuntimeIdentity, KiroRuntimeIdentityError> {
    if let Some(profile_arn) = non_empty(profile_arn) {
        let parsed =
            parse_profile_arn(profile_arn).ok_or(KiroRuntimeIdentityError::InvalidProfileArn)?;
        return Ok(KiroRuntimeIdentity {
            profile_arn: Some(parsed.arn),
            runtime_region: parsed.region,
            region_source: KiroRuntimeRegionSource::ProfileArn,
        });
    }
    if let Some(runtime_region) = non_empty(runtime_region) {
        let runtime_region = normalize_region(runtime_region)
            .ok_or(KiroRuntimeIdentityError::InvalidRuntimeRegion)?;
        return Ok(KiroRuntimeIdentity {
            profile_arn: None,
            runtime_region,
            region_source: KiroRuntimeRegionSource::RuntimeRegion,
        });
    }
    if let Some(api_region) = non_empty(legacy_api_region) {
        let runtime_region =
            normalize_region(api_region).ok_or(KiroRuntimeIdentityError::InvalidLegacyApiRegion)?;
        return Ok(KiroRuntimeIdentity {
            profile_arn: None,
            runtime_region,
            region_source: KiroRuntimeRegionSource::LegacyApiRegion,
        });
    }
    Ok(KiroRuntimeIdentity {
        profile_arn: None,
        runtime_region: DEFAULT_REGION.to_string(),
        region_source: KiroRuntimeRegionSource::Default,
    })
}

pub(crate) fn runtime_identity_from_account(
    account: &Account,
) -> Result<KiroRuntimeIdentity, KiroRuntimeIdentityError> {
    runtime_identity_from_values(account.profile.as_ref(), account.raw.as_ref())
}

pub(crate) fn operational_runtime_identity_from_account(
    account: &Account,
) -> Result<KiroRuntimeIdentity, KiroRuntimeIdentityError> {
    operational_runtime_identity_from_values(account.profile.as_ref(), account.raw.as_ref())
}

pub(crate) fn operational_runtime_identity_from_values(
    profile: Option<&Value>,
    raw: Option<&Value>,
) -> Result<KiroRuntimeIdentity, KiroRuntimeIdentityError> {
    let identity = runtime_identity_from_values(profile, raw)?;
    let auth_method = value_pair_string(
        profile,
        raw,
        &["/authMethod", "/auth_method", "/provider"],
        &["/authMethod", "/auth_method", "/provider"],
    )
    .unwrap_or_else(|| "builder-id".to_string())
    .to_ascii_lowercase();

    if is_enterprise_auth_method(&auth_method) {
        let profile_arn = identity
            .profile_arn
            .as_deref()
            .ok_or(KiroRuntimeIdentityError::MissingEnterpriseProfileArn)?;
        let provenance = value_pair_string(
            profile,
            raw,
            &["/profileProvenance", "/profile_provenance"],
            &["/profileProvenance", "/profile_provenance"],
        );
        if provenance
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("auth_method_default"))
            || is_legacy_enterprise_fallback_profile(profile_arn)
        {
            return Err(KiroRuntimeIdentityError::UnresolvedEnterpriseProfileArn);
        }
        return Ok(identity);
    }

    if identity.profile_arn.is_none() {
        let default_profile_arn = match auth_method.as_str() {
            "api_key" | "api-key" | "apikey" => None,
            "social" | "google" | "github" => Some(SOCIAL_PROFILE_ARN),
            _ => Some(BUILDER_ID_PROFILE_ARN),
        };
        if let Some(profile_arn) = default_profile_arn {
            return resolve_runtime_identity(Some(profile_arn), None, None);
        }
    }
    Ok(identity)
}

pub(crate) fn runtime_identity_from_values(
    profile: Option<&Value>,
    raw: Option<&Value>,
) -> Result<KiroRuntimeIdentity, KiroRuntimeIdentityError> {
    let profile_arn = value_pair_string(
        profile,
        raw,
        &["/profileArn", "/profile_arn"],
        &["/resolvedProfileArn", "/profileArn", "/profile_arn"],
    );
    let runtime_region = value_pair_string(
        profile,
        raw,
        &["/runtimeRegion", "/runtime_region"],
        &["/runtimeRegion", "/runtime_region"],
    );
    let api_region = value_pair_string(
        profile,
        raw,
        &["/apiRegion", "/api_region"],
        &["/apiRegion", "/api_region"],
    );
    resolve_runtime_identity(
        profile_arn.as_deref(),
        runtime_region.as_deref(),
        api_region.as_deref(),
    )
}

pub(crate) fn profile_discovery_regions(auth_region: Option<&str>) -> Option<Vec<String>> {
    let auth_region = match auth_region {
        Some(region) => Some(normalize_region(region)?),
        None => None,
    };
    let prefer_eu = auth_region
        .as_deref()
        .is_some_and(|region| matches!(region.split('-').next(), Some("eu" | "af" | "me" | "il")));
    let mut regions = PROFILE_REGIONS
        .iter()
        .copied()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if prefer_eu {
        regions.reverse();
    }
    if let Some(auth_region) = auth_region.filter(|region| !regions.contains(region)) {
        regions.push(auth_region);
    }
    Some(regions)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn value_pair_string(
    profile: Option<&Value>,
    raw: Option<&Value>,
    profile_pointers: &[&str],
    raw_pointers: &[&str],
) -> Option<String> {
    value_string(profile, profile_pointers).or_else(|| value_string(raw, raw_pointers))
}

fn value_string(value: Option<&Value>, pointers: &[&str]) -> Option<String> {
    let value = value?;
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .and_then(|value| non_empty(Some(value)))
            .map(str::to_string)
    })
}

pub(crate) fn is_enterprise_auth_method(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "enterprise"
            | "idc"
            | "iam_sso"
            | "iam-sso"
            | "identity_center"
            | "external_idp"
            | "external-idp"
            | "externalidp"
    )
}

pub(crate) fn is_legacy_enterprise_fallback_profile(profile_arn: &str) -> bool {
    parse_profile_arn(profile_arn).is_some()
        && profile_arn.contains(&format!(":{LEGACY_ENTERPRISE_PROFILE_ACCOUNT_ID}:"))
        && profile_arn.ends_with(&format!("profile/{LEGACY_ENTERPRISE_PROFILE_ID}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_is_normalized_as_a_canonical_aws_region() {
        assert_eq!(
            normalize_region(" US-EAST-1 ").as_deref(),
            Some("us-east-1")
        );
        for invalid in [
            "",
            "-us-east-1",
            "us-east-1-",
            "../us-east-1",
            "us.east-1",
            "us-east-1:443",
            "us-east-1\r\nx-bad",
            "us-gov-west-1",
            "useast-1",
            "us-east-one",
            "abc-east-1",
        ] {
            assert_eq!(normalize_region(invalid), None, "region={invalid:?}");
        }
        assert_eq!(normalize_region(&"a".repeat(64)), None);
    }

    #[test]
    fn profile_arn_is_strict_and_exposes_the_runtime_region() {
        let parsed = parse_profile_arn(
            "arn:aws:codewhisperer:eu-central-1:123456789012:profile/profile-id_1",
        )
        .unwrap();
        assert_eq!(parsed.region, "eu-central-1");
        for invalid in [
            "arn:aws:codewhisperer:us-east-1.attacker.invalid:123456789012:profile/id",
            "arn:aws-cn:codewhisperer:us-east-1:123456789012:profile/id",
            "arn:aws:s3:us-east-1:123456789012:profile/id",
            "arn:aws:codewhisperer:us-east-1:not-an-account:profile/id",
            "arn:aws:codewhisperer:us-east-1:123456789012:other/id",
            "arn:aws:codewhisperer:us-east-1:123456789012:profile/id/extra",
        ] {
            assert_eq!(parse_profile_arn(invalid), None, "arn={invalid:?}");
        }
    }

    #[test]
    fn profile_arn_region_overrides_legacy_region_without_parsing_it() {
        let identity = resolve_runtime_identity(
            Some("arn:aws:codewhisperer:eu-central-1:123456789012:profile/id"),
            Some("us-east-1"),
            Some("us-east-1.attacker.invalid"),
        )
        .unwrap();
        assert_eq!(identity.runtime_region, "eu-central-1");
        assert_eq!(identity.region_source, KiroRuntimeRegionSource::ProfileArn);
    }

    #[test]
    fn profile_discovery_is_bounded_and_geography_aware() {
        assert_eq!(
            profile_discovery_regions(Some("eu-north-1")).unwrap(),
            vec!["eu-central-1", "us-east-1", "eu-north-1"]
        );
        assert_eq!(
            profile_discovery_regions(Some("ap-southeast-2")).unwrap(),
            vec!["us-east-1", "eu-central-1", "ap-southeast-2"]
        );
        assert_eq!(
            profile_discovery_regions(Some("us-east-1")).unwrap(),
            vec!["us-east-1", "eu-central-1"]
        );
        assert!(profile_discovery_regions(Some("us-east-1.attacker.invalid")).is_none());
    }

    #[test]
    fn operational_identity_defaults_only_shared_consumer_profiles() {
        let builder = operational_runtime_identity_from_values(
            Some(&serde_json::json!({"authMethod": "builder-id"})),
            None,
        )
        .unwrap();
        assert_eq!(builder.profile_arn.as_deref(), Some(BUILDER_ID_PROFILE_ARN));

        let social = operational_runtime_identity_from_values(
            Some(&serde_json::json!({"authMethod": "social"})),
            None,
        )
        .unwrap();
        assert_eq!(social.profile_arn.as_deref(), Some(SOCIAL_PROFILE_ARN));

        let api_key = operational_runtime_identity_from_values(
            Some(&serde_json::json!({
                "authMethod": "api_key",
                "runtimeRegion": "eu-central-1"
            })),
            None,
        )
        .unwrap();
        assert_eq!(api_key.profile_arn, None);
        assert_eq!(api_key.runtime_region, "eu-central-1");
    }

    #[test]
    fn operational_identity_rejects_profileless_and_legacy_enterprise_accounts() {
        assert_eq!(
            operational_runtime_identity_from_values(
                Some(&serde_json::json!({
                    "authMethod": "idc",
                    "runtimeRegion": "eu-central-1"
                })),
                None,
            )
            .unwrap_err(),
            KiroRuntimeIdentityError::MissingEnterpriseProfileArn
        );
        assert_eq!(
            operational_runtime_identity_from_values(
                Some(&serde_json::json!({
                    "authMethod": "idc",
                    "profileArn": "arn:aws:codewhisperer:eu-central-1:610548660232:profile/VNECVYCYYAWN"
                })),
                None,
            )
            .unwrap_err(),
            KiroRuntimeIdentityError::UnresolvedEnterpriseProfileArn
        );

        let organization = operational_runtime_identity_from_values(
            Some(&serde_json::json!({
                "authMethod": "idc",
                "profileArn": "arn:aws:codewhisperer:eu-central-1:123456789012:profile/org-profile"
            })),
            None,
        )
        .unwrap();
        assert_eq!(organization.runtime_region, "eu-central-1");
    }
}
