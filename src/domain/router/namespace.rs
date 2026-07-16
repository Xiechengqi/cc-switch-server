use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

pub const PROTOCOL_EPOCH: &str = "namespace-flat-1";
pub const PUBLIC_SLUG_MIN_LEN: usize = 6;
pub const PUBLIC_SLUG_MAX_LEN: usize = 30;
pub const SHARE_SLUG_MIN_LEN: usize = PUBLIC_SLUG_MIN_LEN;
pub const SHARE_SLUG_MAX_LEN: usize = PUBLIC_SLUG_MAX_LEN;
pub const MARKET_SLUG_MIN_LEN: usize = PUBLIC_SLUG_MIN_LEN;
pub const MARKET_SLUG_MAX_LEN: usize = PUBLIC_SLUG_MAX_LEN;

const DNS_LABEL_MAX_LEN: usize = 63;
const DNS_NAME_MAX_LEN: usize = 253;
const RESERVED_LABELS: &[&str] = &["admin", "api", "cdn-cgi", "router", "www"];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NamespaceError {
    #[error("client subdomain must be {PUBLIC_SLUG_MIN_LEN}-{PUBLIC_SLUG_MAX_LEN} lowercase DNS characters without '--'")]
    InvalidClientSubdomain,
    #[error("share slug must be {SHARE_SLUG_MIN_LEN}-{SHARE_SLUG_MAX_LEN} lowercase DNS characters without '--'")]
    InvalidShareSlug,
    #[error("market slug must be {MARKET_SLUG_MIN_LEN}-{MARKET_SLUG_MAX_LEN} lowercase DNS characters without '--'")]
    InvalidMarketSlug,
    #[error("invalid Router base domain")]
    InvalidBaseDomain,
    #[error("invalid public host")]
    InvalidPublicHost,
    #[error("public host label exceeds the DNS limit")]
    HostLabelTooLong,
    #[error("public host does not belong to Router domain {0}")]
    WrongBaseDomain(String),
    #[error("public host claim does not match its typed namespace fields")]
    InvalidHostClaim,
    #[error("public host {host} is already claimed by {existing_kind}:{existing_subject}")]
    HostConflict {
        host: String,
        existing_kind: PublicHostKind,
        existing_subject: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientSubdomain(String);

impl ClientSubdomain {
    pub fn parse(value: &str) -> Result<Self, NamespaceError> {
        validate_public_slug(value).map_err(|_| NamespaceError::InvalidClientSubdomain)?;
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ClientSubdomain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShareSlug(String);

impl ShareSlug {
    pub fn parse(value: &str) -> Result<Self, NamespaceError> {
        validate_public_slug(value).map_err(|_| NamespaceError::InvalidShareSlug)?;
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ShareSlug {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MarketSlug(String);

impl MarketSlug {
    pub fn parse(value: &str) -> Result<Self, NamespaceError> {
        validate_public_slug(value).map_err(|_| NamespaceError::InvalidMarketSlug)?;
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MarketSlug {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BaseDomain(String);

impl BaseDomain {
    pub fn parse(value: &str) -> Result<Self, NamespaceError> {
        let value = value.trim_end_matches('.').to_ascii_lowercase();
        if value.is_empty()
            || value.len() > DNS_NAME_MAX_LEN
            || value.parse::<IpAddr>().is_ok()
            || value.split('.').count() < 2
            || !value.split('.').all(valid_dns_label)
        {
            return Err(NamespaceError::InvalidBaseDomain);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BaseDomain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicHost(String);

impl PublicHost {
    pub fn parse(value: &str) -> Result<Self, NamespaceError> {
        let value = value.trim_end_matches('.').to_ascii_lowercase();
        if value.is_empty()
            || value.len() > DNS_NAME_MAX_LEN
            || value.parse::<IpAddr>().is_ok()
            || value.split('.').count() < 2
            || !value.split('.').all(valid_dns_label)
        {
            return Err(NamespaceError::InvalidPublicHost);
        }
        Ok(Self(value))
    }

    pub fn for_client(
        base_domain: &BaseDomain,
        client_subdomain: &ClientSubdomain,
    ) -> Result<Self, NamespaceError> {
        Self::from_label(base_domain, client_subdomain.as_str())
    }

    pub fn for_share(
        base_domain: &BaseDomain,
        share_slug: &ShareSlug,
        client_subdomain: &ClientSubdomain,
    ) -> Result<Self, NamespaceError> {
        Self::from_label(
            base_domain,
            &format!("{}--{}", share_slug.as_str(), client_subdomain.as_str()),
        )
    }

    pub fn for_market(
        base_domain: &BaseDomain,
        market_slug: &MarketSlug,
    ) -> Result<Self, NamespaceError> {
        Self::from_label(base_domain, market_slug.as_str())
    }

    fn from_label(base_domain: &BaseDomain, label: &str) -> Result<Self, NamespaceError> {
        if label.len() > DNS_LABEL_MAX_LEN {
            return Err(NamespaceError::HostLabelTooLong);
        }
        Self::parse(&format!("{label}.{}", base_domain.as_str()))
    }

    pub fn label_for<'a>(&'a self, base_domain: &BaseDomain) -> Result<&'a str, NamespaceError> {
        let suffix = format!(".{}", base_domain.as_str());
        self.0
            .strip_suffix(&suffix)
            .filter(|label| !label.is_empty() && !label.contains('.'))
            .ok_or_else(|| NamespaceError::WrongBaseDomain(base_domain.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PublicHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PublicHostKind {
    Client,
    Share,
    Market,
}

impl PublicHostKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Share => "share",
            Self::Market => "market",
        }
    }

    pub fn parse(value: &str) -> Result<Self, NamespaceError> {
        match value {
            "client" => Ok(Self::Client),
            "share" => Ok(Self::Share),
            "market" => Ok(Self::Market),
            _ => Err(NamespaceError::InvalidHostClaim),
        }
    }
}

impl fmt::Display for PublicHostKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicHostClaim {
    host: PublicHost,
    kind: PublicHostKind,
    subject_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_subdomain: Option<ClientSubdomain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slug: Option<String>,
}

impl PublicHostClaim {
    pub fn client(
        base_domain: &BaseDomain,
        client_subdomain: ClientSubdomain,
        subject_id: impl Into<String>,
    ) -> Result<Self, NamespaceError> {
        let host = PublicHost::for_client(base_domain, &client_subdomain)?;
        Self::new(
            host,
            PublicHostKind::Client,
            subject_id,
            Some(client_subdomain),
            None,
        )
    }

    pub fn share(
        base_domain: &BaseDomain,
        share_slug: ShareSlug,
        client_subdomain: ClientSubdomain,
        subject_id: impl Into<String>,
    ) -> Result<Self, NamespaceError> {
        let host = PublicHost::for_share(base_domain, &share_slug, &client_subdomain)?;
        Self::new(
            host,
            PublicHostKind::Share,
            subject_id,
            Some(client_subdomain),
            Some(share_slug.to_string()),
        )
    }

    pub fn market(
        base_domain: &BaseDomain,
        market_slug: MarketSlug,
        subject_id: impl Into<String>,
    ) -> Result<Self, NamespaceError> {
        let host = PublicHost::for_market(base_domain, &market_slug)?;
        Self::new(
            host,
            PublicHostKind::Market,
            subject_id,
            None,
            Some(market_slug.to_string()),
        )
    }

    fn new(
        host: PublicHost,
        kind: PublicHostKind,
        subject_id: impl Into<String>,
        client_subdomain: Option<ClientSubdomain>,
        slug: Option<String>,
    ) -> Result<Self, NamespaceError> {
        let subject_id = subject_id.into();
        if subject_id.trim() != subject_id
            || subject_id.is_empty()
            || subject_id.len() > 255
            || subject_id.chars().any(char::is_control)
        {
            return Err(NamespaceError::InvalidHostClaim);
        }
        Ok(Self {
            host,
            kind,
            subject_id,
            client_subdomain,
            slug,
        })
    }

    pub fn validate_for(&self, base_domain: &BaseDomain) -> Result<(), NamespaceError> {
        let expected = match self.kind {
            PublicHostKind::Client => PublicHost::for_client(
                base_domain,
                self.client_subdomain
                    .as_ref()
                    .ok_or(NamespaceError::InvalidHostClaim)?,
            )?,
            PublicHostKind::Share => PublicHost::for_share(
                base_domain,
                &ShareSlug::parse(
                    self.slug
                        .as_deref()
                        .ok_or(NamespaceError::InvalidHostClaim)?,
                )?,
                self.client_subdomain
                    .as_ref()
                    .ok_or(NamespaceError::InvalidHostClaim)?,
            )?,
            PublicHostKind::Market => PublicHost::for_market(
                base_domain,
                &MarketSlug::parse(
                    self.slug
                        .as_deref()
                        .ok_or(NamespaceError::InvalidHostClaim)?,
                )?,
            )?,
        };
        if expected != self.host {
            return Err(NamespaceError::InvalidHostClaim);
        }
        Ok(())
    }

    pub fn host(&self) -> &PublicHost {
        &self.host
    }

    pub fn kind(&self) -> PublicHostKind {
        self.kind
    }

    pub fn subject_id(&self) -> &str {
        &self.subject_id
    }

    pub fn client_subdomain(&self) -> Option<&ClientSubdomain> {
        self.client_subdomain.as_ref()
    }

    pub fn slug(&self) -> Option<&str> {
        self.slug.as_deref()
    }
}

#[derive(Debug, Clone, Default)]
pub struct FlatHostCatalog {
    claims: BTreeMap<PublicHost, PublicHostClaim>,
}

impl FlatHostCatalog {
    pub fn insert(&mut self, claim: PublicHostClaim) -> Result<bool, NamespaceError> {
        let (_, base_domain) = claim
            .host()
            .as_str()
            .split_once('.')
            .ok_or(NamespaceError::InvalidHostClaim)?;
        claim.validate_for(&BaseDomain::parse(base_domain)?)?;
        if let Some(existing) = self.claims.get(claim.host()) {
            if existing == &claim {
                return Ok(false);
            }
            return Err(NamespaceError::HostConflict {
                host: claim.host().to_string(),
                existing_kind: existing.kind(),
                existing_subject: existing.subject_id().to_string(),
            });
        }
        self.claims.insert(claim.host().clone(), claim);
        Ok(true)
    }

    pub fn resolve(&self, host: &PublicHost) -> Option<&PublicHostClaim> {
        self.claims.get(host)
    }

    pub fn remove(&mut self, host: &PublicHost) -> Option<PublicHostClaim> {
        self.claims.remove(host)
    }

    pub fn len(&self) -> usize {
        self.claims.len()
    }

    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }
}

macro_rules! impl_validated_string_serde {
    ($type:ty, $parse:path) => {
        impl Serialize for $type {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                $parse(&value).map_err(serde::de::Error::custom)
            }
        }
    };
}

impl_validated_string_serde!(ClientSubdomain, ClientSubdomain::parse);
impl_validated_string_serde!(ShareSlug, ShareSlug::parse);
impl_validated_string_serde!(MarketSlug, MarketSlug::parse);
impl_validated_string_serde!(BaseDomain, BaseDomain::parse);
impl_validated_string_serde!(PublicHost, PublicHost::parse);

fn validate_public_slug(value: &str) -> Result<(), ()> {
    if !(PUBLIC_SLUG_MIN_LEN..=PUBLIC_SLUG_MAX_LEN).contains(&value.len())
        || value.contains("--")
        || RESERVED_LABELS.contains(&value)
        || !value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        || value.ends_with('-')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(());
    }
    Ok(())
}

fn valid_dns_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= DNS_LABEL_MAX_LEN
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_flat_hosts_with_one_dns_wildcard_level() {
        let base = BaseDomain::parse("Router.Example.COM.").unwrap();
        let client = ClientSubdomain::parse("edge-main").unwrap();
        let share = ShareSlug::parse("team-pro").unwrap();
        let market = MarketSlug::parse("official").unwrap();

        assert_eq!(
            PublicHost::for_client(&base, &client).unwrap().as_str(),
            "edge-main.router.example.com"
        );
        assert_eq!(
            PublicHost::for_share(&base, &share, &client)
                .unwrap()
                .as_str(),
            "team-pro--edge-main.router.example.com"
        );
        assert_eq!(
            PublicHost::for_market(&base, &market).unwrap().as_str(),
            "official.router.example.com"
        );
    }

    #[test]
    fn rejects_ambiguous_or_invalid_slugs() {
        assert_eq!(
            ShareSlug::parse("team--pro"),
            Err(NamespaceError::InvalidShareSlug)
        );
        assert!(ClientSubdomain::parse("Edge-main").is_err());
        assert!(ClientSubdomain::parse("short").is_err());
        assert!(ClientSubdomain::parse("edge-").is_err());
    }

    #[test]
    fn maximum_share_host_label_is_62_characters() {
        let base = BaseDomain::parse("router.example.com").unwrap();
        let client = ClientSubdomain::parse(&"c".repeat(PUBLIC_SLUG_MAX_LEN)).unwrap();
        let share = ShareSlug::parse(&"s".repeat(SHARE_SLUG_MAX_LEN)).unwrap();
        let host = PublicHost::for_share(&base, &share, &client).unwrap();
        assert_eq!(host.label_for(&base).unwrap().len(), 62);
    }
}
