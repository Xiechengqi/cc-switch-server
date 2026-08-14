use crate::domain::providers::model::ProviderType;

/// Credential-bound Cursor wire profile. A request never changes profile
/// after credential resolution, including authentication recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CursorProtocolRail {
    OAuthCli,
    ApiKeySdk,
}

impl CursorProtocolRail {
    pub fn for_provider(provider_type: ProviderType) -> Option<Self> {
        match provider_type {
            ProviderType::CursorOAuth => Some(Self::OAuthCli),
            ProviderType::CursorApiKey => Some(Self::ApiKeySdk),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::OAuthCli => "oauth_cli",
            Self::ApiKeySdk => "apikey_sdk",
        }
    }

    pub const fn protocol_revision(self) -> &'static str {
        match self {
            Self::OAuthCli => "cursor-oauth-cli/v2",
            Self::ApiKeySdk => "cursor-apikey-sdk/v2",
        }
    }

    pub const fn uses_rich_request_context(self) -> bool {
        matches!(self, Self::ApiKeySdk)
    }

    pub const fn accepts_kv_after_text_terminal(self) -> bool {
        matches!(self, Self::OAuthCli)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_types_select_disjoint_cursor_rails() {
        assert_eq!(
            CursorProtocolRail::for_provider(ProviderType::CursorOAuth),
            Some(CursorProtocolRail::OAuthCli)
        );
        assert_eq!(
            CursorProtocolRail::for_provider(ProviderType::CursorApiKey),
            Some(CursorProtocolRail::ApiKeySdk)
        );
        assert_ne!(
            CursorProtocolRail::OAuthCli.protocol_revision(),
            CursorProtocolRail::ApiKeySdk.protocol_revision()
        );
    }
}
