use axum::http::StatusCode;

use crate::domain::providers::model::ProviderType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthRecoveryDecision {
    RefreshAndReplaySameBinding,
    ReturnUnauthorized,
}

pub(super) fn unauthorized_recovery_decision(
    status: StatusCode,
    provider_type: ProviderType,
    attempted: bool,
    replay_allowed: bool,
) -> Option<AuthRecoveryDecision> {
    if status != StatusCode::UNAUTHORIZED {
        return None;
    }
    if attempted || !replay_allowed || !supports_unauthorized_recovery(provider_type) {
        return Some(AuthRecoveryDecision::ReturnUnauthorized);
    }
    Some(AuthRecoveryDecision::RefreshAndReplaySameBinding)
}

pub(super) fn supports_unauthorized_recovery(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::ClaudeOAuth
            | ProviderType::CodexOAuth
            | ProviderType::GrokOAuth
            | ProviderType::GeminiCli
            | ProviderType::AntigravityOAuth
            | ProviderType::AgyOAuth
            | ProviderType::KiroOAuth
            | ProviderType::AmazonQOAuth
            | ProviderType::GitHubCopilot
            | ProviderType::CursorOAuth
            | ProviderType::CursorApiKey
            | ProviderType::KimiCode
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_auth_recovery_is_exactly_once_for_supported_providers() {
        for provider_type in [
            ProviderType::ClaudeOAuth,
            ProviderType::CodexOAuth,
            ProviderType::GrokOAuth,
            ProviderType::GeminiCli,
            ProviderType::AntigravityOAuth,
            ProviderType::AgyOAuth,
            ProviderType::KiroOAuth,
            ProviderType::AmazonQOAuth,
            ProviderType::GitHubCopilot,
            ProviderType::CursorOAuth,
            ProviderType::CursorApiKey,
            ProviderType::KimiCode,
        ] {
            assert_eq!(
                unauthorized_recovery_decision(
                    StatusCode::UNAUTHORIZED,
                    provider_type,
                    false,
                    true,
                ),
                Some(AuthRecoveryDecision::RefreshAndReplaySameBinding)
            );
            assert_eq!(
                unauthorized_recovery_decision(StatusCode::UNAUTHORIZED, provider_type, true, true,),
                Some(AuthRecoveryDecision::ReturnUnauthorized)
            );
        }
    }

    #[test]
    fn recovery_requires_unauthorized_status_and_replayable_request() {
        assert_eq!(
            unauthorized_recovery_decision(
                StatusCode::FORBIDDEN,
                ProviderType::ClaudeOAuth,
                false,
                true,
            ),
            None
        );
        assert_eq!(
            unauthorized_recovery_decision(
                StatusCode::UNAUTHORIZED,
                ProviderType::ClaudeOAuth,
                false,
                false,
            ),
            Some(AuthRecoveryDecision::ReturnUnauthorized)
        );
        assert_eq!(
            unauthorized_recovery_decision(
                StatusCode::UNAUTHORIZED,
                ProviderType::Claude,
                false,
                true,
            ),
            Some(AuthRecoveryDecision::ReturnUnauthorized)
        );
    }
}
