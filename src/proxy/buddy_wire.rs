//! Reviewed Buddy CLI generation wire. Authentication and catalog headers remain separate.
//! Keep the 2.142.0 wire until independent product/site acceptance evidence exists.
use crate::domain::codebuddy::{buddy_product_policy, CodeBuddyAccountProfile, CodeBuddySite};
use crate::domain::providers::model::ProviderType;
use serde_json::Value;

pub(super) fn encode_body(payload: &Value) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(payload)
}

pub(super) fn generation_headers(
    product: ProviderType,
    profile: &CodeBuddyAccountProfile,
    access_token: &str,
    request_id: &str,
) -> Result<Vec<(String, String)>, String> {
    let identity = buddy_product_policy(product, profile.site)?.client_identity;
    let mut headers = vec![
        ("accept".to_string(), "text/event-stream".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        (
            "authorization".to_string(),
            format!("Bearer {}", access_token),
        ),
        ("x-user-id".to_string(), profile.uid.clone()),
        ("x-domain".to_string(), profile.domain.clone()),
        ("x-product".to_string(), "SaaS".to_string()),
        ("x-requested-with".to_string(), "XMLHttpRequest".to_string()),
        ("x-request-id".to_string(), request_id.to_string()),
        ("x-conversation-id".to_string(), request_id.to_string()),
        (
            "x-conversation-request-id".to_string(),
            request_id.to_string(),
        ),
        (
            "x-conversation-message-id".to_string(),
            request_id.to_string(),
        ),
        ("x-root-request-id".to_string(), request_id.to_string()),
        ("x-agent-intent".to_string(), "craft".to_string()),
        ("x-agent-purpose".to_string(), "conversation".to_string()),
        ("x-agent-type".to_string(), "main".to_string()),
        ("x-private-data".to_string(), "false".to_string()),
        ("x-codebuddy-request".to_string(), "1".to_string()),
        ("user-agent".to_string(), identity.user_agent().to_string()),
    ];
    if profile.enterprise_id.trim().is_empty() {
        headers.push(("x-no-enterprise-id".to_string(), "true".to_string()));
    } else {
        headers.push(("x-enterprise-id".to_string(), profile.enterprise_id.clone()));
    }
    if profile.site == CodeBuddySite::Cn {
        let origin = profile.site.profile().browser_origin.to_string();
        headers.push(("origin".to_string(), origin.clone()));
        headers.push(("referer".to_string(), format!("{origin}/")));
    }
    Ok(headers)
}
