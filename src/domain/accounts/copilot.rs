//! Copilot's catalog advertises protocol capability, not arbitrary target URLs.
//! Selection never guesses from model names or retries another endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopilotEndpoint {
    Messages,
    Responses,
    Chat,
}

impl CopilotEndpoint {
    pub fn path(self) -> &'static str {
        match self {
            Self::Messages => "/v1/messages",
            Self::Responses => "/responses",
            Self::Chat => "/chat/completions",
        }
    }

    pub fn api_format(self) -> &'static str {
        match self {
            Self::Messages => "anthropic",
            Self::Responses => "openai_responses",
            Self::Chat => "openai_chat",
        }
    }

    pub fn select(advertised: &[String], preferred: Self) -> Result<Self, &'static str> {
        // Older authenticated catalogs omitted endpoint metadata and used Chat.
        // This compatibility applies only to a model present in a fresh catalog.
        if advertised.is_empty() {
            return Ok(Self::Chat);
        }
        let supports = |endpoint: Self| advertised.iter().any(|path| path == endpoint.path());
        let order = match preferred {
            Self::Messages => [Self::Messages, Self::Responses, Self::Chat],
            Self::Responses => [Self::Responses, Self::Messages, Self::Chat],
            Self::Chat => [Self::Chat, Self::Responses, Self::Messages],
        };
        order.into_iter().find(|endpoint| supports(*endpoint)).ok_or(
            "Copilot model has no reviewed supported endpoint; endpoint URLs and model-name heuristics are not accepted",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copilot_endpoint_uses_advertised_capability_not_model_family() {
        for endpoint in [
            CopilotEndpoint::Messages,
            CopilotEndpoint::Responses,
            CopilotEndpoint::Chat,
        ] {
            for preferred in [
                CopilotEndpoint::Messages,
                CopilotEndpoint::Responses,
                CopilotEndpoint::Chat,
            ] {
                assert_eq!(
                    CopilotEndpoint::select(&[endpoint.path().into()], preferred),
                    Ok(endpoint)
                );
            }
        }
    }

    #[test]
    fn copilot_endpoint_prefers_native_and_rejects_unreviewed_paths() {
        let endpoints = vec![
            "/responses".into(),
            "/chat/completions".into(),
            "/v1/messages".into(),
        ];
        assert_eq!(
            CopilotEndpoint::select(&endpoints, CopilotEndpoint::Messages),
            Ok(CopilotEndpoint::Messages)
        );
        assert_eq!(
            CopilotEndpoint::select(&endpoints, CopilotEndpoint::Responses),
            Ok(CopilotEndpoint::Responses)
        );
        for path in [
            "https://evil.example/responses",
            "/responses?paid=true",
            "/unknown/chat/completions",
            "/completions",
        ] {
            assert!(CopilotEndpoint::select(&[path.into()], CopilotEndpoint::Chat).is_err());
        }
        assert_eq!(
            CopilotEndpoint::select(&[], CopilotEndpoint::Messages),
            Ok(CopilotEndpoint::Chat)
        );
    }
}
