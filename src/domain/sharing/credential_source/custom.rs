//! A Custom Surface owns its credential storage, but may still prove that its
//! effective authentication and service scope match another Surface. No raw
//! credential or enumerable fingerprint from this module is public API.

use std::collections::BTreeMap;

use http::HeaderValue;
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::Zeroizing;

use crate::domain::accounts::store::AccountStore;
use crate::domain::providers::credentials::{
    reveal_provider_credential, unique_runtime_credential,
};
use crate::domain::providers::registry::UpstreamProtocol;
use crate::domain::providers::runtime::{
    compile_runtime_plan, RuntimeAuthRef, RuntimeConfigurationState,
};
use crate::domain::providers::static_auth::static_auth_carrier;
use crate::domain::providers::store::{ProviderStore, StoredProvider};

use super::{CredentialSourceIdentity, CustomSourceScope};

pub(super) fn resolve(
    providers: &ProviderStore,
    accounts: &AccountStore,
    stored: &StoredProvider,
    family: &'static str,
) -> anyhow::Result<Option<CredentialSourceIdentity>> {
    let materialized = providers.materialize_provider_record(stored)?;
    let Ok(plan) = compile_runtime_plan(&materialized, accounts) else {
        return Ok(None);
    };
    if plan.configuration_state != RuntimeConfigurationState::Ready {
        return Ok(None);
    }
    let RuntimeAuthRef::CustomCredential {
        auth_scheme, slots, ..
    } = &plan.auth_ref
    else {
        return Ok(None);
    };
    let Some(endpoint) = service_scope(&plan.endpoint, plan.upstream_protocol) else {
        return Ok(None);
    };
    // Carrier names and prefixes are identity-significant. Do not assume that
    // the same string sent through different authentication rails is one key.
    let Ok(Some(carrier)) =
        static_auth_carrier(plan.upstream_protocol, *auth_scheme, &plan.driver_options)
    else {
        return Ok(None);
    };
    let authentication = carrier;
    let carrier = authentication.identity_tuple();
    let slots = slots.iter().filter(|slot| {
        !plan
            .extra_headers
            .iter()
            .any(|header| header.credential_slot == **slot)
    });
    let Ok(Some(secret)) =
        unique_runtime_credential(&materialized.provider, slots.map(String::as_str))
    else {
        return Ok(None);
    };
    let secret = Zeroizing::new(secret);
    if HeaderValue::from_str(&secret).is_err() {
        return Ok(None);
    }
    let mut headers = BTreeMap::new();
    for header in &plan.extra_headers {
        let Ok(value) = reveal_provider_credential(&materialized.provider, &header.credential_slot)
        else {
            return Ok(None);
        };
        let value = value.trim().to_string();
        if value.is_empty() || HeaderValue::from_str(&value).is_err() {
            return Ok(None);
        }
        headers.insert(header.name.as_str(), value);
    }
    let endpoint_digest = Sha256::digest(endpoint.as_bytes()).into();
    let carrier_material = serde_json::to_vec(&carrier)?;
    let auth_digest = Sha256::digest(&carrier_material).into();
    let extra_headers_material = Zeroizing::new(serde_json::to_vec(&headers)?);
    // Named and length-framed fields prevent alias-slot and swapped-value
    // collisions. Provider ids, app names and models are deliberately absent.
    let material = Zeroizing::new(serde_json::to_vec(&(
        "cc-switch-custom-credential-source-v1",
        endpoint,
        carrier,
        secret.as_str(),
        &headers,
    ))?);
    Ok(Some(CredentialSourceIdentity {
        family,
        digest: Sha256::digest(&material).into(),
        custom_scope: Some(CustomSourceScope {
            endpoint_digest,
            auth_digest,
            authentication,
            primary_digest: Sha256::digest(secret.as_bytes()).into(),
            extra_headers_digest: Sha256::digest(&extra_headers_material).into(),
        }),
    }))
}

fn service_scope(endpoint: &str, protocol: UpstreamProtocol) -> Option<String> {
    if endpoint.contains('\\') {
        return None;
    }
    let mut url = Url::parse(endpoint).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    // Unknown/encoded tenant routing is not evidence of a shared service.
    if path.contains('%') || path.contains("//") {
        return None;
    }
    let version = match protocol {
        UpstreamProtocol::AnthropicMessages
        | UpstreamProtocol::OpenAiChat
        | UpstreamProtocol::OpenAiResponses => "/v1",
        UpstreamProtocol::GeminiNative => "/v1beta",
        _ => return None,
    };
    let scope = path.strip_suffix(version).unwrap_or(path).to_string();
    url.set_path(&scope);
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_scope_normalizes_only_protocol_versions_and_preserves_tenants() {
        use UpstreamProtocol::*;
        let scope = |url, protocol| service_scope(url, protocol).unwrap();
        assert_eq!(
            scope("https://API.example:443/", AnthropicMessages),
            scope("https://api.example/v1/", OpenAiResponses)
        );
        assert_eq!(
            scope("https://api.example/tenant/v1", AnthropicMessages),
            scope("https://api.example/tenant/", OpenAiChat)
        );
        assert_ne!(
            scope("https://api.example/a/v1", OpenAiResponses),
            scope("https://api.example/b/v1", OpenAiResponses)
        );
        assert_ne!(
            scope("https://api.example/v1", GeminiNative),
            scope("https://api.example/v1beta", GeminiNative)
        );
        assert_ne!(
            scope("https://api.example", OpenAiChat),
            scope("http://api.example", OpenAiChat)
        );
        for url in [
            "https://api.example?tenant=a",
            "https://api.example#tenant",
            "https://user:secret@api.example",
            "https://api.example/a%2fb",
            "https://api.example/a//b",
            "https://api.example\\tenant",
        ] {
            assert!(service_scope(url, OpenAiResponses).is_none());
        }
    }
}
