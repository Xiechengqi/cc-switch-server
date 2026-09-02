use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeWireProfile {
    pub id: &'static str,
    pub claude_code_version: &'static str,
    pub stainless_package_version: &'static str,
    pub node_version: &'static str,
    pub axios_version: &'static str,
    pub cch_seed: u64,
    pub cch_excluded_keys: &'static [&'static str],
    pub billing_version_strategy: &'static str,
    pub billing_prompt_fingerprint_salt: &'static str,
}

pub const CLAUDE_WIRE_PROFILE: ClaudeWireProfile = ClaudeWireProfile {
    id: "claude-code-2.1.258-audited-2026-09-02",
    claude_code_version: "2.1.258",
    stainless_package_version: "0.112.1",
    node_version: "v26.3.0",
    axios_version: "1.15.2",
    cch_seed: 0x4D659218E32A3268,
    cch_excluded_keys: &["max_tokens", "fallbacks", "fallback_credit_token"],
    billing_version_strategy: "public_cli_version_plus_prompt_fingerprint",
    billing_prompt_fingerprint_salt: "59cf53e54c78",
};

pub const DEFAULT_CLAUDE_CC_ENTRYPOINT: &str = "cli";
pub const DEFAULT_STAINLESS_RUNTIME: &str = "node";
pub const CLAUDE_CODE_IDENTITY_TEXT: &str =
    "You are Claude Code, Anthropic's official CLI for Claude.";
pub const CLAUDE_BILLING_FINGERPRINT_UTF16_INDICES: [usize; 3] = [4, 7, 20];

const CCH_SEED_BY_VERSION_PREFIX: &[(&str, u64)] = &[("2.1.", CLAUDE_WIRE_PROFILE.cch_seed)];
const STAINLESS_IDENTITY_PROFILES: &[(&str, &str)] = &[
    ("MacOS", "arm64"),
    ("MacOS", "x64"),
    ("Linux", "x64"),
    ("Windows", "x64"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCliIdentity {
    pub version: String,
    pub user_agent: String,
    pub source: &'static str,
    pub override_conflict: bool,
    pub stale_override_rejected: bool,
}

pub fn claude_cli_identity() -> ClaudeCliIdentity {
    let user_agent_override = std::env::var("CC_SWITCH_CLI_UA")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let version_override_raw = std::env::var("CC_SWITCH_CLI_UA_VERSION")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let override_conflict = user_agent_override.is_some() && version_override_raw.is_some();
    let user_agent_candidate = user_agent_override.as_ref().and_then(|user_agent| {
        claude_cli_version_from_user_agent(user_agent)
            .map(|version| (user_agent.to_string(), version))
    });
    let version_candidate = version_override_raw
        .as_deref()
        .filter(|value| valid_public_cli_version(value))
        .map(str::to_string);
    let stale_override_rejected = user_agent_candidate
        .as_ref()
        .is_some_and(|(_, version)| !public_cli_version_at_least_profile(version))
        || version_candidate
            .as_deref()
            .is_some_and(|version| !public_cli_version_at_least_profile(version));

    if let Some((user_agent, version)) =
        user_agent_candidate.filter(|(_, version)| public_cli_version_at_least_profile(version))
    {
        return ClaudeCliIdentity {
            version,
            user_agent,
            source: "user_agent_override",
            override_conflict,
            stale_override_rejected,
        };
    }
    if let Some(version) =
        version_candidate.filter(|version| public_cli_version_at_least_profile(version))
    {
        return ClaudeCliIdentity {
            user_agent: format!("claude-cli/{version} (external, cli)"),
            version,
            source: "version_override",
            override_conflict,
            stale_override_rejected,
        };
    }
    ClaudeCliIdentity {
        version: CLAUDE_WIRE_PROFILE.claude_code_version.to_string(),
        user_agent: format!(
            "claude-cli/{} (external, cli)",
            CLAUDE_WIRE_PROFILE.claude_code_version
        ),
        source: "wire_profile",
        override_conflict,
        stale_override_rejected,
    }
}

pub fn claude_cli_version() -> String {
    claude_cli_identity().version
}

pub fn claude_cli_user_agent() -> String {
    claude_cli_identity().user_agent
}

pub fn claude_code_user_agent() -> String {
    format!("claude-code/{}", claude_cli_version())
}

pub fn claude_axios_user_agent() -> String {
    format!("axios/{}", CLAUDE_WIRE_PROFILE.axios_version)
}

pub fn claude_stainless_package_version() -> &'static str {
    CLAUDE_WIRE_PROFILE.stainless_package_version
}

pub fn claude_wire_profile_id() -> &'static str {
    CLAUDE_WIRE_PROFILE.id
}

pub fn claude_cch_version() -> String {
    claude_cli_version()
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
    claude_billing_header_text_for_prompt("")
}

pub fn claude_billing_header_text_for_prompt(prompt: &str) -> String {
    let version = claude_cch_version();
    let fingerprint = claude_billing_prompt_fingerprint(prompt, &version);
    format!(
        "x-anthropic-billing-header: cc_version={version}.{fingerprint}; cc_entrypoint={}; cch=00000;",
        claude_cc_entrypoint()
    )
}

pub fn claude_billing_prompt_fingerprint(prompt: &str, version: &str) -> String {
    let selected = CLAUDE_BILLING_FINGERPRINT_UTF16_INDICES
        .map(|index| prompt.encode_utf16().nth(index).unwrap_or(u16::from(b'0')));
    // JavaScript indexes strings by UTF-16 code unit. Converting the selected
    // units together also reproduces Node's replacement behavior for an
    // isolated surrogate before the SHA-256 input is UTF-8 encoded.
    let fingerprint_chars = String::from_utf16_lossy(&selected);
    let digest = Sha256::digest(
        format!(
            "{}{fingerprint_chars}{version}",
            CLAUDE_WIRE_PROFILE.billing_prompt_fingerprint_salt
        )
        .as_bytes(),
    );
    format!("{:02x}{:02x}", digest[0], digest[1])
        .chars()
        .take(3)
        .collect()
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
        .unwrap_or_else(|| CLAUDE_WIRE_PROFILE.node_version.to_string())
}

fn claude_cli_version_from_user_agent(user_agent: &str) -> Option<String> {
    if user_agent.len() > 512 || !user_agent.bytes().all(|byte| matches!(byte, 0x20..=0x7e)) {
        return None;
    }
    let after_marker = user_agent.trim().strip_prefix("claude-cli/")?;
    let version = after_marker
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>();
    let suffix = &after_marker[version.len()..];
    (valid_public_cli_version(&version)
        && suffix
            .chars()
            .next()
            .is_none_or(|character| character.is_ascii_whitespace()))
    .then_some(version)
}

fn valid_public_cli_version(value: &str) -> bool {
    public_cli_version_parts(value).is_some()
}

fn public_cli_version_at_least_profile(value: &str) -> bool {
    public_cli_version_parts(value)
        .zip(public_cli_version_parts(
            CLAUDE_WIRE_PROFILE.claude_code_version,
        ))
        .is_some_and(|(candidate, minimum)| candidate >= minimum)
}

fn public_cli_version_parts(value: &str) -> Option<[u32; 3]> {
    let mut parts = value.split('.');
    let parse_part = |part: Option<&str>| {
        let part = part?;
        (!part.is_empty() && part.len() <= 6 && part.chars().all(|ch| ch.is_ascii_digit()))
            .then(|| part.parse::<u32>().ok())
            .flatten()
    };
    let parsed = [
        parse_part(parts.next())?,
        parse_part(parts.next())?,
        parse_part(parts.next())?,
    ];
    parts.next().is_none().then_some(parsed)
}

fn claude_cch_seed_for_version(version: &str) -> u64 {
    CCH_SEED_BY_VERSION_PREFIX
        .iter()
        .find_map(|(prefix, seed)| version.starts_with(prefix).then_some(*seed))
        .unwrap_or(CLAUDE_WIRE_PROFILE.cch_seed)
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

        fn unset(name: &'static str) -> Self {
            let previous = std::env::var(name).ok();
            std::env::remove_var(name);
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
        assert!(claude_cli_version_from_user_agent("x claude-cli/2.1.236").is_none());
        assert!(claude_cli_version_from_user_agent("claude-cli/2.1").is_none());
        assert!(claude_cli_version_from_user_agent("claude-cli/2.1.241evil").is_none());
        assert!(claude_cli_version_from_user_agent("claude-cli/2.1.241\r\nevil").is_none());
        assert!(
            claude_cli_version_from_user_agent(&format!("claude-cli/2.1.{}", "1".repeat(7)))
                .is_none()
        );
    }

    #[test]
    fn resolved_identity_keeps_all_version_surfaces_coherent() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _ua = EnvGuard::unset("CC_SWITCH_CLI_UA");
        let _version = EnvGuard::set("CC_SWITCH_CLI_UA_VERSION", "2.1.260");

        let identity = claude_cli_identity();
        assert_eq!(identity.version, "2.1.260");
        assert_eq!(identity.user_agent, "claude-cli/2.1.260 (external, cli)");
        assert_eq!(identity.source, "version_override");
        assert!(!identity.override_conflict);
        assert!(!identity.stale_override_rejected);
        assert_eq!(claude_code_user_agent(), "claude-code/2.1.260");
        assert!(claude_billing_header_text().contains("cc_version=2.1.260."));
    }

    #[test]
    fn valid_full_user_agent_wins_and_conflict_is_visible() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _ua = EnvGuard::set(
            "CC_SWITCH_CLI_UA",
            "claude-cli/2.1.261 (external, claude-vscode, agent-sdk/0.3.261)",
        );
        let _version = EnvGuard::set("CC_SWITCH_CLI_UA_VERSION", "2.1.260");

        let identity = claude_cli_identity();
        assert_eq!(identity.version, "2.1.261");
        assert_eq!(identity.source, "user_agent_override");
        assert!(identity.override_conflict);
        assert!(!identity.stale_override_rejected);
    }

    #[test]
    fn invalid_overrides_fail_closed_to_the_wire_profile() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _ua = EnvGuard::set("CC_SWITCH_CLI_UA", "curl/8");
        let _version = EnvGuard::set("CC_SWITCH_CLI_UA_VERSION", "latest");

        let identity = claude_cli_identity();
        assert_eq!(identity.version, CLAUDE_WIRE_PROFILE.claude_code_version);
        assert_eq!(identity.source, "wire_profile");
        assert!(identity.override_conflict);
        assert!(!identity.stale_override_rejected);
    }

    #[test]
    fn stale_overrides_are_rejected_instead_of_downgrading_the_profile() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _ua = EnvGuard::set("CC_SWITCH_CLI_UA", "claude-cli/2.1.236 (external, cli)");
        let _version = EnvGuard::set("CC_SWITCH_CLI_UA_VERSION", "2.1.220");

        let identity = claude_cli_identity();
        assert_eq!(identity.version, CLAUDE_WIRE_PROFILE.claude_code_version);
        assert_eq!(identity.source, "wire_profile");
        assert!(identity.override_conflict);
        assert!(identity.stale_override_rejected);
    }

    #[test]
    fn a_fresh_version_override_survives_a_stale_full_user_agent() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _ua = EnvGuard::set("CC_SWITCH_CLI_UA", "claude-cli/2.1.236 (external, cli)");
        let _version = EnvGuard::set("CC_SWITCH_CLI_UA_VERSION", "2.2.0");

        let identity = claude_cli_identity();
        assert_eq!(identity.version, "2.2.0");
        assert_eq!(identity.source, "version_override");
        assert!(identity.override_conflict);
        assert!(identity.stale_override_rejected);
    }

    #[test]
    fn billing_prompt_fingerprint_matches_official_utf16_vectors() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _ua = EnvGuard::unset("CC_SWITCH_CLI_UA");
        let _version = EnvGuard::unset("CC_SWITCH_CLI_UA_VERSION");
        assert_eq!(claude_billing_prompt_fingerprint("ping", "2.1.258"), "1e2");
        assert_eq!(
            claude_billing_prompt_fingerprint("abcdefghijklmnopqrstuvwxyz", "2.1.258"),
            "d3d"
        );
        assert_eq!(
            claude_billing_prompt_fingerprint("abcd😀ghijklmnopqrstuvwxyz", "2.1.258"),
            "a15"
        );
        assert!(claude_billing_header_text_for_prompt("ping").contains("cc_version=2.1.258.1e2;"));
    }

    #[test]
    fn public_version_comparison_is_numeric() {
        assert!(public_cli_version_at_least_profile("2.1.258"));
        assert!(public_cli_version_at_least_profile("2.2.0"));
        assert!(public_cli_version_at_least_profile("3.0.0"));
        assert!(!public_cli_version_at_least_profile("2.1.99"));
        assert!(!public_cli_version_at_least_profile("1.999999.999999"));
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
