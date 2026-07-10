use sha2::{Digest, Sha256};

pub const DEFAULT_CLAUDE_CLI_VERSION: &str = "2.1.195";
pub const DEFAULT_CLAUDE_CLI_BUILD: &str = "47e";
pub const DEFAULT_CLAUDE_CC_ENTRYPOINT: &str = "cli";
pub const DEFAULT_STAINLESS_PACKAGE_VERSION: &str = "0.55.1";
pub const DEFAULT_STAINLESS_RUNTIME: &str = "node";
pub const DEFAULT_STAINLESS_RUNTIME_VERSION: &str = "v20.19.0";
pub const DEFAULT_CCH_SEED: u64 = 0x6E52736AC806831E;
pub const CLAUDE_CODE_IDENTITY_TEXT: &str =
    "You are Claude Code, Anthropic's official CLI for Claude.";

const CCH_SEED_BY_VERSION_PREFIX: &[(&str, u64)] = &[("2.1.", DEFAULT_CCH_SEED)];
const STAINLESS_IDENTITY_PROFILES: &[(&str, &str)] = &[
    ("MacOS", "arm64"),
    ("MacOS", "x64"),
    ("Linux", "x64"),
    ("Windows", "x64"),
];

pub fn claude_cli_version() -> String {
    std::env::var("CC_SWITCH_CLI_UA")
        .ok()
        .and_then(|ua| claude_cli_version_from_user_agent(&ua))
        .or_else(|| std::env::var("CC_SWITCH_CLI_UA_VERSION").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_CLAUDE_CLI_VERSION.to_string())
}

pub fn claude_cli_user_agent() -> String {
    std::env::var("CC_SWITCH_CLI_UA")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("claude-cli/{} (external, cli)", claude_cli_version()))
}

pub fn claude_cch_version() -> String {
    format!("{}.{}", claude_cli_version(), DEFAULT_CLAUDE_CLI_BUILD)
}

pub fn claude_cch_seed() -> u64 {
    std::env::var("CC_SWITCH_CCH_SALT_HEX")
        .ok()
        .and_then(|value| parse_cch_seed_hex(&value))
        .unwrap_or_else(|| claude_cch_seed_for_version(&claude_cli_version()))
}

pub fn claude_cc_entrypoint() -> String {
    std::env::var("CC_SWITCH_CLAUDE_CC_ENTRYPOINT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_CLAUDE_CC_ENTRYPOINT.to_string())
}

pub fn claude_billing_header_text() -> String {
    format!(
        "x-anthropic-billing-header: cc_version={}; cc_entrypoint={}; cch=00000;",
        claude_cch_version(),
        claude_cc_entrypoint()
    )
}

pub fn claude_stainless_os(identity_seed: Option<&str>) -> String {
    std::env::var("CC_SWITCH_CLI_STAINLESS_OS")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| stainless_identity_profile(identity_seed).0.to_string())
}

pub fn claude_stainless_arch(identity_seed: Option<&str>) -> String {
    std::env::var("CC_SWITCH_CLI_STAINLESS_ARCH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| stainless_identity_profile(identity_seed).1.to_string())
}

pub fn claude_stainless_runtime() -> String {
    std::env::var("CC_SWITCH_CLI_STAINLESS_RUNTIME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_STAINLESS_RUNTIME.to_string())
}

pub fn claude_stainless_runtime_version() -> String {
    std::env::var("CC_SWITCH_CLI_STAINLESS_RUNTIME_VERSION")
        .or_else(|_| std::env::var("CC_SWITCH_CLI_NODE_VERSION"))
        .or_else(|_| std::env::var("NODE_VERSION"))
        .ok()
        .map(|value| normalize_node_version(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_STAINLESS_RUNTIME_VERSION.to_string())
}

fn claude_cli_version_from_user_agent(user_agent: &str) -> Option<String> {
    let after_marker = user_agent.split_once("claude-cli/")?.1;
    let version = after_marker
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>();
    (!version.is_empty()).then_some(version)
}

fn claude_cch_seed_for_version(version: &str) -> u64 {
    CCH_SEED_BY_VERSION_PREFIX
        .iter()
        .find_map(|(prefix, seed)| version.starts_with(prefix).then_some(*seed))
        .unwrap_or(DEFAULT_CCH_SEED)
}

fn parse_cch_seed_hex(value: &str) -> Option<u64> {
    let value = value.trim();
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if value.is_empty() {
        return None;
    }
    u64::from_str_radix(value, 16).ok()
}

fn stainless_identity_profile(identity_seed: Option<&str>) -> (&'static str, &'static str) {
    let Some(identity_seed) = identity_seed
        .map(str::trim)
        .filter(|identity_seed| !identity_seed.is_empty())
    else {
        return host_stainless_profile();
    };
    let digest = Sha256::digest(identity_seed.as_bytes());
    let index = usize::from(digest[0]) % STAINLESS_IDENTITY_PROFILES.len();
    STAINLESS_IDENTITY_PROFILES[index]
}

fn host_stainless_profile() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "macos" => "MacOS",
        "windows" => "Windows",
        _ => "Linux",
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86" | "x86_64" => "x64",
        _ => "x64",
    };
    (os, arch)
}

fn normalize_node_version(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() || value.starts_with('v') {
        value.to_string()
    } else {
        format!("v{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
            if let Some(previous) = self.previous.as_deref() {
                std::env::set_var(self.name, previous);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }

    #[test]
    fn parses_claude_cli_version_from_user_agent() {
        assert_eq!(
            claude_cli_version_from_user_agent("claude-cli/2.1.195 (external, cli)").as_deref(),
            Some("2.1.195")
        );
        assert!(claude_cli_version_from_user_agent("curl/8").is_none());
    }

    #[test]
    fn cch_seed_accepts_hex_env_override() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::set("CC_SWITCH_CCH_SALT_HEX", "0x00000000000000ff");

        assert_eq!(claude_cch_seed(), 0xff);
    }

    #[test]
    fn stainless_identity_profile_is_stable_per_seed() {
        assert_eq!(
            claude_stainless_os(Some("account-a")),
            claude_stainless_os(Some("account-a"))
        );
        assert_eq!(
            claude_stainless_arch(Some("account-a")),
            claude_stainless_arch(Some("account-a"))
        );
    }

    #[test]
    fn stainless_env_overrides_are_used() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _os = EnvGuard::set("CC_SWITCH_CLI_STAINLESS_OS", "MacOS");
        let _arch = EnvGuard::set("CC_SWITCH_CLI_STAINLESS_ARCH", "arm64");
        let _runtime = EnvGuard::set("CC_SWITCH_CLI_STAINLESS_RUNTIME", "node");
        let _runtime_version = EnvGuard::set("CC_SWITCH_CLI_STAINLESS_RUNTIME_VERSION", "22.17.0");

        assert_eq!(claude_stainless_os(Some("account-a")), "MacOS");
        assert_eq!(claude_stainless_arch(Some("account-a")), "arm64");
        assert_eq!(claude_stainless_runtime(), "node");
        assert_eq!(claude_stainless_runtime_version(), "v22.17.0");
    }
}
