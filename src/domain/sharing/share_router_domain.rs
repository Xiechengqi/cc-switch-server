const KNOWN_SHARE_ROUTER_REGIONS: &[(&str, &str)] = &[
    ("japan", "jptokenswitch.cc"),
    ("singapore", "sgptokenswitch.cc"),
];

pub const DEFAULT_SHARE_ROUTER_DOMAIN: &str = "jptokenswitch.cc";

pub fn normalize_share_router_domain(input: &str) -> Result<String, String> {
    let mut value = input.trim().trim_end_matches('/').to_string();
    if value.is_empty() {
        return Err("share.validation.required".to_string());
    }
    if value.contains(char::is_whitespace) {
        return Err("share.validation.invalidRouterDomain".to_string());
    }

    let lower = value.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let parsed = reqwest::Url::parse(&value)
            .map_err(|_| "share.validation.invalidRouterDomain".to_string())?;
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err("share.validation.invalidRouterDomain".to_string());
        }
        value = parsed.host_str().unwrap_or_default().to_string();
    } else if value.contains("://")
        || value.contains('/')
        || value.contains('?')
        || value.contains('#')
    {
        return Err("share.validation.invalidRouterDomain".to_string());
    }

    let authority = value.to_ascii_lowercase();
    validate_share_router_authority(&authority)?;
    Ok(authority)
}

pub fn share_router_region_for_domain(domain: &str) -> Option<&'static str> {
    let normalized = domain.trim().trim_end_matches('/').to_ascii_lowercase();
    KNOWN_SHARE_ROUTER_REGIONS
        .iter()
        .find_map(|(region, base_url)| (*base_url == normalized).then_some(*region))
}

pub fn router_domain_from_url(url: Option<&str>) -> Option<String> {
    let value = url?.trim();
    let without_scheme = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    without_scheme
        .split('/')
        .next()
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_string)
}

fn validate_share_router_authority(authority: &str) -> Result<(), String> {
    if authority.is_empty()
        || authority.len() > 253
        || authority.contains('@')
        || authority.contains('[')
        || authority.contains(']')
    {
        return Err("share.validation.invalidRouterDomain".to_string());
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port
                .parse::<u16>()
                .map_err(|_| "share.validation.invalidRouterDomain".to_string())?;
            if port == 0 {
                return Err("share.validation.invalidRouterDomain".to_string());
            }
            (host, Some(port))
        }
        None => (authority, None),
    };

    if matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0") {
        return Ok(());
    }
    if host == "example.com" || host.ends_with(".example.com") {
        return Err("share.validation.invalidRouterDomain".to_string());
    }
    if host.split('.').all(|part| part.parse::<u8>().is_ok()) && host.matches('.').count() == 3 {
        return Ok(());
    }
    if !host.contains('.') {
        return Err("share.validation.invalidRouterDomain".to_string());
    }
    for label in host.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        {
            return Err("share.validation.invalidRouterDomain".to_string());
        }
    }
    let _ = port;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_share_router_domain() {
        assert_eq!(
            normalize_share_router_domain("HTTPS://JP.TokenSwitch.CC/").unwrap(),
            "jptokenswitch.cc"
        );
        assert_eq!(
            share_router_region_for_domain("jptokenswitch.cc"),
            Some("japan")
        );
    }
}
