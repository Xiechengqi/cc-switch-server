use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PROTOCOL_EPOCH: &str = "namespace-flat-1";
pub const CLIENT_PREFIX_MIN_LEN: usize = 3;
pub const CLIENT_PREFIX_MAX_LEN: usize = 7;
pub const CLIENT_FINGERPRINT_LEN: usize = 20;
pub const SHARE_SLUG_MIN_LEN: usize = 3;
pub const SHARE_SLUG_MAX_LEN: usize = 32;
pub const MARKET_SLUG_MIN_LEN: usize = 3;
pub const MARKET_SLUG_MAX_LEN: usize = 32;

const DNS_LABEL_MAX_LEN: usize = 63;
const DNS_NAME_MAX_LEN: usize = 253;
const BASE32_LOWER: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
const RESERVED_LABELS: &[&str] = &["admin", "api", "cdn-cgi", "router", "www"];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NamespaceError {
    #[error("client prefix must be {CLIENT_PREFIX_MIN_LEN}-{CLIENT_PREFIX_MAX_LEN} lowercase alphanumeric characters and start with a letter")]
    InvalidClientPrefix,
    #[error("client public key must contain exactly 32 bytes")]
    InvalidClientPublicKey,
    #[error("client key must use <prefix>-<20 character base32 fingerprint>")]
    InvalidClientKey,
    #[error("share slug must be {SHARE_SLUG_MIN_LEN}-{SHARE_SLUG_MAX_LEN} lowercase DNS characters without consecutive hyphens")]
    InvalidShareSlug,
    #[error("market slug must be {MARKET_SLUG_MIN_LEN}-{MARKET_SLUG_MAX_LEN} lowercase DNS characters, must not contain '--', and must not match the client-key grammar")]
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

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientKey(String);

impl ClientKey {
    pub fn derive(prefix: &str, public_key: &[u8]) -> Result<Self, NamespaceError> {
        validate_client_prefix(prefix)?;
        if public_key.len() != 32 {
            return Err(NamespaceError::InvalidClientPublicKey);
        }
        let digest = Sha256::digest(public_key);
        let fingerprint = encode_base32_prefix(&digest, CLIENT_FINGERPRINT_LEN);
        Self::parse(&format!("{prefix}-{fingerprint}"))
    }

    pub fn parse(value: &str) -> Result<Self, NamespaceError> {
        let Some((prefix, fingerprint)) = value.split_once('-') else {
            return Err(NamespaceError::InvalidClientKey);
        };
        if fingerprint.contains('-')
            || validate_client_prefix(prefix).is_err()
            || fingerprint.len() != CLIENT_FINGERPRINT_LEN
            || !fingerprint.bytes().all(|byte| BASE32_LOWER.contains(&byte))
        {
            return Err(NamespaceError::InvalidClientKey);
        }
        Ok(Self(value.to_string()))
    }

    pub fn prefix(&self) -> &str {
        self.0.split_once('-').expect("validated client key").0
    }

    pub fn fingerprint(&self) -> &str {
        self.0.split_once('-').expect("validated client key").1
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ClientKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("ClientKey").field(&self.0).finish()
    }
}

impl fmt::Display for ClientKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShareSlug(String);

impl ShareSlug {
    pub fn parse(value: &str) -> Result<Self, NamespaceError> {
        if !valid_slug(value, SHARE_SLUG_MIN_LEN, SHARE_SLUG_MAX_LEN)
            || value.contains("--")
            || RESERVED_LABELS.contains(&value)
        {
            return Err(NamespaceError::InvalidShareSlug);
        }
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
        if !valid_slug(value, MARKET_SLUG_MIN_LEN, MARKET_SLUG_MAX_LEN)
            || value.contains("--")
            || ClientKey::parse(value).is_ok()
            || RESERVED_LABELS.contains(&value)
        {
            return Err(NamespaceError::InvalidMarketSlug);
        }
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
        client_key: &ClientKey,
    ) -> Result<Self, NamespaceError> {
        Self::from_label(base_domain, client_key.as_str())
    }

    pub fn for_share(
        base_domain: &BaseDomain,
        share_slug: &ShareSlug,
        client_key: &ClientKey,
    ) -> Result<Self, NamespaceError> {
        Self::from_label(
            base_domain,
            &format!("{}--{}", share_slug.as_str(), client_key.as_str()),
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
        let label = self
            .0
            .strip_suffix(&suffix)
            .filter(|label| !label.is_empty() && !label.contains('.'))
            .ok_or_else(|| NamespaceError::WrongBaseDomain(base_domain.to_string()))?;
        Ok(label)
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
    client_key: Option<ClientKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slug: Option<String>,
}

impl PublicHostClaim {
    pub fn client(
        base_domain: &BaseDomain,
        client_key: ClientKey,
        subject_id: impl Into<String>,
    ) -> Result<Self, NamespaceError> {
        let host = PublicHost::for_client(base_domain, &client_key)?;
        Self::new(
            host,
            PublicHostKind::Client,
            subject_id,
            Some(client_key),
            None,
        )
    }

    pub fn share(
        base_domain: &BaseDomain,
        share_slug: ShareSlug,
        client_key: ClientKey,
        subject_id: impl Into<String>,
    ) -> Result<Self, NamespaceError> {
        let host = PublicHost::for_share(base_domain, &share_slug, &client_key)?;
        Self::new(
            host,
            PublicHostKind::Share,
            subject_id,
            Some(client_key),
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
        client_key: Option<ClientKey>,
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
            client_key,
            slug,
        })
    }

    pub fn validate_for(&self, base_domain: &BaseDomain) -> Result<(), NamespaceError> {
        if self.subject_id.trim() != self.subject_id
            || self.subject_id.is_empty()
            || self.subject_id.len() > 255
            || self.subject_id.chars().any(char::is_control)
        {
            return Err(NamespaceError::InvalidHostClaim);
        }
        let expected = match self.kind {
            PublicHostKind::Client => PublicHost::for_client(
                base_domain,
                self.client_key
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
                self.client_key
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

    pub fn client_key(&self) -> Option<&ClientKey> {
        self.client_key.as_ref()
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

impl_validated_string_serde!(ClientKey, ClientKey::parse);
impl_validated_string_serde!(ShareSlug, ShareSlug::parse);
impl_validated_string_serde!(MarketSlug, MarketSlug::parse);
impl_validated_string_serde!(BaseDomain, BaseDomain::parse);
impl_validated_string_serde!(PublicHost, PublicHost::parse);

fn validate_client_prefix(prefix: &str) -> Result<(), NamespaceError> {
    if !(CLIENT_PREFIX_MIN_LEN..=CLIENT_PREFIX_MAX_LEN).contains(&prefix.len())
        || RESERVED_LABELS.contains(&prefix)
        || !prefix
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err(NamespaceError::InvalidClientPrefix);
    }
    Ok(())
}

fn valid_slug(value: &str, min_len: usize, max_len: usize) -> bool {
    (min_len..=max_len).contains(&value.len())
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
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

fn encode_base32_prefix(bytes: &[u8], output_len: usize) -> String {
    let mut output = String::with_capacity(output_len);
    let mut accumulator = 0_u32;
    let mut bits = 0_u8;
    for byte in bytes {
        accumulator = (accumulator << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 && output.len() < output_len {
            bits -= 5;
            let index = ((accumulator >> bits) & 0x1f) as usize;
            output.push(BASE32_LOWER[index] as char);
        }
        if output.len() == output_len {
            break;
        }
        accumulator &= (1_u32 << bits).saturating_sub(1);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_stable_100_bit_client_fingerprint() {
        let key = ClientKey::derive("edge", &[7_u8; 32]).unwrap();
        assert_eq!(key.as_str(), "edge-joyg7dsohj3rluqb2vz5");
        assert_eq!(key.prefix(), "edge");
        assert_eq!(key.fingerprint().len(), 20);
        assert_eq!(ClientKey::parse(key.as_str()).unwrap(), key);
    }

    #[test]
    fn constructs_flat_hosts_with_one_dns_wildcard_level() {
        let base = BaseDomain::parse("Router.Example.COM.").unwrap();
        let key = ClientKey::derive("edge", &[9_u8; 32]).unwrap();
        let share = ShareSlug::parse("team-pro").unwrap();
        let market = MarketSlug::parse("official").unwrap();

        assert_eq!(
            PublicHost::for_client(&base, &key).unwrap().as_str(),
            format!("{key}.router.example.com")
        );
        assert_eq!(
            PublicHost::for_share(&base, &share, &key).unwrap().as_str(),
            format!("team-pro--{key}.router.example.com")
        );
        assert_eq!(
            PublicHost::for_market(&base, &market).unwrap().as_str(),
            "official.router.example.com"
        );
    }

    #[test]
    fn rejects_ambiguous_or_invalid_slugs() {
        let key = ClientKey::derive("edge", &[11_u8; 32]).unwrap();
        assert_eq!(
            MarketSlug::parse(key.as_str()),
            Err(NamespaceError::InvalidMarketSlug)
        );
        assert_eq!(
            ShareSlug::parse("team--pro"),
            Err(NamespaceError::InvalidShareSlug)
        );
        assert_eq!(
            MarketSlug::parse("team--pro"),
            Err(NamespaceError::InvalidMarketSlug)
        );
        assert!(ClientKey::parse("ed-ge-abcdefghijklmnopqrst").is_err());
        assert!(ClientKey::parse("Edge-abcdefghijklmnopqrst").is_err());
    }

    #[test]
    fn maximum_share_host_label_is_62_characters() {
        let base = BaseDomain::parse("router.example.com").unwrap();
        let key = ClientKey::derive("prefix7", &[13_u8; 32]).unwrap();
        let share = ShareSlug::parse(&"a".repeat(SHARE_SLUG_MAX_LEN)).unwrap();
        let host = PublicHost::for_share(&base, &share, &key).unwrap();
        assert_eq!(host.label_for(&base).unwrap().len(), 62);
    }

    #[test]
    fn exact_catalog_is_idempotent_and_rejects_cross_subject_collision() {
        let base = BaseDomain::parse("router.example.com").unwrap();
        let key = ClientKey::derive("edge", &[17_u8; 32]).unwrap();
        let first = PublicHostClaim::share(
            &base,
            ShareSlug::parse("shared").unwrap(),
            key.clone(),
            "share-1",
        )
        .unwrap();
        let conflict =
            PublicHostClaim::share(&base, ShareSlug::parse("shared").unwrap(), key, "share-2")
                .unwrap();
        let mut catalog = FlatHostCatalog::default();
        assert!(catalog.insert(first.clone()).unwrap());
        assert!(!catalog.insert(first).unwrap());
        assert!(matches!(
            catalog.insert(conflict),
            Err(NamespaceError::HostConflict { .. })
        ));
    }
}
