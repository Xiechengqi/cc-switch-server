pub const GROK_CLI_BASE_URL: &str = "https://cli-chat-proxy.grok.com/v1";
pub const GROK_CLI_USER_URL: &str = "https://cli-chat-proxy.grok.com/v1/user?include=subscription";
pub const GROK_CLI_WEEKLY_BILLING_URL: &str =
    "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
pub const GROK_CLI_MONTHLY_BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing";
pub const GROK_SUBSCRIPTIONS_URL: &str = "https://grok.com/rest/subscriptions";
pub const GROK_TASK_USAGE_URL: &str = "https://grok.com/rest/tasks/usage";
pub const DEFAULT_GROK_CLI_VERSION: &str = "0.2.111";
pub const GROK_CLI_CLIENT_IDENTIFIER: &str = "grok-shell";
pub const DEFAULT_GROK_CLI_USER_AGENT: &str = "grok-shell/0.2.111 (linux; x86_64)";
pub const GROK_CLI_TOKEN_AUTH: &str = "xai-grok-cli";

#[cfg(test)]
pub(crate) static GROK_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn grok_cli_version() -> String {
    std::env::var("CC_SWITCH_GROK_CLI_VERSION")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 64
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '+')
                })
        })
        .unwrap_or_else(|| DEFAULT_GROK_CLI_VERSION.to_string())
}

pub fn grok_cli_user_agent() -> String {
    if let Some(value) = std::env::var("CC_SWITCH_GROK_CLI_USER_AGENT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 256
                && value
                    .chars()
                    .all(|character| character == '\t' || !character.is_ascii_control())
        })
    {
        return value;
    }
    let version = grok_cli_version();
    if version == DEFAULT_GROK_CLI_VERSION {
        DEFAULT_GROK_CLI_USER_AGENT.to_string()
    } else {
        format!("grok-shell/{version} (linux; x86_64)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvGuard {
        name: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var(name).ok();
            std::env::set_var(name, value);
            Self { name, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.take() {
                std::env::set_var(self.name, previous);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }

    #[tokio::test]
    async fn version_override_updates_default_user_agent() {
        let _lock = GROK_ENV_LOCK.lock().await;
        let _version = EnvGuard::set("CC_SWITCH_GROK_CLI_VERSION", "0.3.1");
        let _user_agent = EnvGuard::set("CC_SWITCH_GROK_CLI_USER_AGENT", "");
        assert_eq!(grok_cli_version(), "0.3.1");
        assert_eq!(grok_cli_user_agent(), "grok-shell/0.3.1 (linux; x86_64)");
    }

    #[tokio::test]
    async fn invalid_header_overrides_fall_back() {
        let _lock = GROK_ENV_LOCK.lock().await;
        let _version = EnvGuard::set("CC_SWITCH_GROK_CLI_VERSION", "bad version");
        let _user_agent = EnvGuard::set("CC_SWITCH_GROK_CLI_USER_AGENT", "bad\r\nheader");
        assert_eq!(grok_cli_version(), DEFAULT_GROK_CLI_VERSION);
        assert_eq!(grok_cli_user_agent(), DEFAULT_GROK_CLI_USER_AGENT);
    }
}
