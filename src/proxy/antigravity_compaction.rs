use bytes::Bytes;

use crate::domain::providers::model::ProviderType;

use super::{ProxyError, ProxyRoute};

/// Antigravity conversation compaction has no provider receipt proving a
/// supported wire contract. Keep the rail fail-closed until such evidence is
/// frozen locally; in particular, never forward a Codex compaction marker into
/// a Google Code Assist request by accident.
pub(crate) fn enforce_disabled_contract(
    provider_type: ProviderType,
    route: ProxyRoute,
    body: &Bytes,
) -> Result<(), ProxyError> {
    if !matches!(
        provider_type,
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
    ) {
        return Ok(());
    }

    let compact_request = route == ProxyRoute::CodexResponsesCompact
        || (route == ProxyRoute::CodexResponses
            && super::forwarder::codex_responses_body_has_compaction_trigger(body));
    if compact_request {
        return Err(ProxyError::bad_request(
            "Antigravity conversation compaction is disabled pending a verified provider receipt",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn antigravity_compaction_is_fail_closed_without_a_receipt() {
        for provider_type in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
            let direct = enforce_disabled_contract(
                provider_type,
                ProxyRoute::CodexResponsesCompact,
                &Bytes::from_static(br#"{"input":[]}"#),
            )
            .unwrap_err();
            assert_eq!(direct.status, axum::http::StatusCode::BAD_REQUEST);
            assert!(direct.message.contains("disabled pending"));

            let marker = Bytes::from(
                serde_json::to_vec(&json!({
                    "input": [{"type": "compaction_trigger"}]
                }))
                .unwrap(),
            );
            assert!(
                enforce_disabled_contract(provider_type, ProxyRoute::CodexResponses, &marker,)
                    .is_err()
            );
        }
    }

    #[test]
    fn gate_does_not_change_supported_generation_or_other_providers() {
        let generation = Bytes::from_static(br#"{"model":"gemini-3.5-flash"}"#);
        assert!(enforce_disabled_contract(
            ProviderType::AntigravityOAuth,
            ProxyRoute::Gemini,
            &generation,
        )
        .is_ok());

        let marker = Bytes::from_static(br#"{"input":[{"type":"compaction_trigger"}]}"#);
        assert!(enforce_disabled_contract(
            ProviderType::CodexOAuth,
            ProxyRoute::CodexResponses,
            &marker,
        )
        .is_ok());
    }
}
