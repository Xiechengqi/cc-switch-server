use axum::http::StatusCode;

use crate::domain::providers::model::ProviderType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthRecoveryDecision {
    RefreshAndReplaySameBinding,
    ReturnUnauthorized,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct AuthRecoveryState {
    attempted: bool,
}

impl AuthRecoveryState {
    pub(super) fn decide(
        &mut self,
        status: StatusCode,
        provider_type: ProviderType,
        replay_allowed: bool,
    ) -> Option<AuthRecoveryDecision> {
        let decision =
            unauthorized_recovery_decision(status, provider_type, self.attempted, replay_allowed)?;
        if decision == AuthRecoveryDecision::RefreshAndReplaySameBinding {
            self.attempted = true;
        }
        Some(decision)
    }

    pub(super) fn attempted(self) -> bool {
        self.attempted
    }
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
            let mut state = AuthRecoveryState::default();
            assert_eq!(
                state.decide(StatusCode::UNAUTHORIZED, provider_type, true),
                Some(AuthRecoveryDecision::RefreshAndReplaySameBinding)
            );
            assert!(state.attempted());
            assert_eq!(
                state.decide(StatusCode::UNAUTHORIZED, provider_type, true),
                Some(AuthRecoveryDecision::ReturnUnauthorized)
            );
        }
    }

    #[test]
    fn recovery_requires_unauthorized_status_and_replayable_request() {
        let mut state = AuthRecoveryState::default();
        assert_eq!(
            state.decide(StatusCode::FORBIDDEN, ProviderType::ClaudeOAuth, true),
            None
        );
        assert_eq!(
            state.decide(StatusCode::UNAUTHORIZED, ProviderType::ClaudeOAuth, false),
            Some(AuthRecoveryDecision::ReturnUnauthorized)
        );
        assert!(!state.attempted());
        assert_eq!(
            state.decide(StatusCode::UNAUTHORIZED, ProviderType::Claude, true),
            Some(AuthRecoveryDecision::ReturnUnauthorized)
        );
    }
}
