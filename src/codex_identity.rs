use std::env;

pub(crate) const DEFAULT_CODEX_ORIGINATOR: &str = "codex_cli_rs";
pub(crate) const DEFAULT_CODEX_VERSION: &str = "0.144.1";
pub(crate) const MIN_CODEX_UPSTREAM_VERSION: &str = "0.144.0";

const CODEX_VERSION_ENV: &str = "CC_SWITCH_CODEX_CLIENT_VERSION";
const CODEX_USER_AGENT_ENV: &str = "CC_SWITCH_CODEX_USER_AGENT";
const MAX_ORIGINATOR_LEN: usize = 64;

const OFFICIAL_ORIGINATORS: &[&str] = &[
    "codex_cli_rs",
    "codex-tui",
    "codex_vscode",
    "codex_vscode_copilot",
    "codex_app",
    "codex_chatgpt_desktop",
    "codex_atlas",
    "codex_exec",
    "codex_sdk_ts",
];

pub(crate) fn configured_version() -> String {
    env::var(CODEX_VERSION_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| version_at_least(value, MIN_CODEX_UPSTREAM_VERSION))
        .unwrap_or_else(|| DEFAULT_CODEX_VERSION.to_string())
}

pub(crate) fn default_user_agent() -> String {
    let version = configured_version();
    let fallback = default_user_agent_for_version(&version);
    let Some(candidate) = env::var(CODEX_USER_AGENT_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return fallback;
    };
    pair_user_agent(&candidate)
        .map(|(_, user_agent)| user_agent)
        .unwrap_or(fallback)
}

pub(crate) fn finalize_headers(headers: &mut Vec<(&'static str, String)>) {
    let configured_version = configured_version();
    let candidate = header_value(headers, "user-agent")
        .map(str::to_string)
        .unwrap_or_else(default_user_agent);
    let (originator, user_agent) = pair_user_agent(&candidate).unwrap_or_else(|| {
        (
            DEFAULT_CODEX_ORIGINATOR.to_string(),
            default_user_agent_for_version(&configured_version),
        )
    });
    let version = header_value(headers, "version")
        .filter(|value| version_at_least(value, MIN_CODEX_UPSTREAM_VERSION))
        .map(str::to_string)
        .unwrap_or(configured_version);

    replace_or_push(headers, "user-agent", user_agent);
    replace_or_push(headers, "originator", originator);
    replace_or_push(headers, "version", version);
}

fn default_user_agent_for_version(version: &str) -> String {
    format!("codex_cli_rs/{version} (Ubuntu 22.04.0; x86_64) xterm-256color")
}

fn header_value<'a>(headers: &'a [(&'static str, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .rev()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

fn replace_or_push(headers: &mut Vec<(&'static str, String)>, name: &'static str, value: String) {
    headers.retain(|(candidate, _)| !candidate.eq_ignore_ascii_case(name));
    headers.push((name, value));
}

fn pair_user_agent(user_agent: &str) -> Option<(String, String)> {
    let user_agent = user_agent.trim();
    let slash = user_agent.find('/')?;
    if slash == 0 {
        return None;
    }
    let leading = user_agent[..slash].trim();
    if sane_originator(leading) && official_originator(leading) {
        let originator = canonical_originator(leading);
        return Some((
            originator.clone(),
            format!("{originator}{}", &user_agent[slash..]),
        ));
    }

    let trailer = trailer_originator(user_agent)?;
    if trailer.contains('/') || !sane_originator(trailer) || !official_originator(trailer) {
        return None;
    }
    let originator = canonical_originator(trailer);
    Some((
        originator.clone(),
        format!("{originator}{}", &user_agent[slash..]),
    ))
}

fn trailer_originator(user_agent: &str) -> Option<&str> {
    let open = user_agent.rfind('(')?;
    let rest = &user_agent[open + 1..];
    let close = rest.find(')')?;
    if !rest[close + 1..].trim().is_empty() {
        return None;
    }
    let inside = rest[..close].trim();
    let name = inside
        .split_once(';')
        .map_or(inside, |(name, _)| name)
        .trim();
    (!name.is_empty()).then_some(name)
}

fn official_originator(originator: &str) -> bool {
    let lower = originator.trim().to_ascii_lowercase();
    OFFICIAL_ORIGINATORS.contains(&lower.as_str()) || originator.starts_with("Codex ")
}

fn canonical_originator(originator: &str) -> String {
    let lower = originator.trim().to_ascii_lowercase();
    if OFFICIAL_ORIGINATORS.contains(&lower.as_str()) {
        lower
    } else {
        originator.trim().to_string()
    }
}

fn sane_originator(originator: &str) -> bool {
    !originator.is_empty()
        && originator.len() <= MAX_ORIGINATOR_LEN
        && originator
            .bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
}

fn version_at_least(candidate: &str, minimum: &str) -> bool {
    parse_version(candidate)
        .zip(parse_version(minimum))
        .is_some_and(|(candidate, minimum)| candidate >= minimum)
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let value = value.trim().trim_start_matches('v');
    let numeric = value.split(['-', '+']).next()?;
    let mut parts = numeric.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_official_identity_and_recovers_override_trailer() {
        assert_eq!(
            pair_user_agent("CODEX_CLI_RS/0.144.1 (Ubuntu; x86_64) xterm"),
            Some((
                "codex_cli_rs".to_string(),
                "codex_cli_rs/0.144.1 (Ubuntu; x86_64) xterm".to_string()
            ))
        );
        assert_eq!(
            pair_user_agent("cccc/0.144.1 (Ubuntu 22.04; x86_64) xterm (codex-tui; 0.144.1)"),
            Some((
                "codex-tui".to_string(),
                "codex-tui/0.144.1 (Ubuntu 22.04; x86_64) xterm (codex-tui; 0.144.1)".to_string()
            ))
        );
    }

    #[test]
    fn rejects_third_party_and_malformed_identity() {
        assert!(pair_user_agent("luna/1.0.0").is_none());
        assert!(pair_user_agent("codex_cli_rs_evil/1.0.0").is_none());
        assert!(pair_user_agent("Codex \u{1}evil/1.0.0").is_none());
        assert!(pair_user_agent("curl").is_none());
    }

    #[test]
    fn finalizer_repairs_pair_and_version_or_falls_back() {
        let mut headers = vec![
            ("originator", "codex_cli_rs".to_string()),
            ("version", "0.125.0".to_string()),
            (
                "user-agent",
                "codex-tui/0.140.2 (Mac OS X; arm64) iTerm".to_string(),
            ),
        ];
        finalize_headers(&mut headers);
        assert_eq!(header_value(&headers, "originator"), Some("codex-tui"));
        assert_eq!(
            header_value(&headers, "version"),
            Some(DEFAULT_CODEX_VERSION)
        );

        replace_or_push(&mut headers, "user-agent", "PostmanRuntime/7".to_string());
        finalize_headers(&mut headers);
        assert_eq!(header_value(&headers, "originator"), Some("codex_cli_rs"));
        assert!(header_value(&headers, "user-agent")
            .is_some_and(|value| value.starts_with("codex_cli_rs/0.144.1")));
    }
}
