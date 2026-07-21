use std::net::Ipv4Addr;
use std::time::Duration;

const PUBLIC_IP_ENDPOINTS: &[&str] = &[
    "https://checkip.amazonaws.com/",
    "https://api.ipify.org/",
    "https://ipv4.icanhazip.com/",
];
const PUBLIC_IP_TIMEOUT: Duration = Duration::from_secs(5);
const PUBLIC_IP_ENV: &str = "CC_SWITCH_PUBLIC_IP";

pub fn normalize_public_ipv4(value: &str) -> Option<String> {
    value
        .trim()
        .parse::<Ipv4Addr>()
        .ok()
        .map(|ip| ip.to_string())
}

/// Resolve the process public IPv4.
/// Prefer `CC_SWITCH_PUBLIC_IP` when set; otherwise probe independent HTTPS endpoints in order.
pub async fn discover_public_ipv4(http: &reqwest::Client) -> Option<String> {
    if let Ok(value) = std::env::var(PUBLIC_IP_ENV) {
        if let Some(ip) = normalize_public_ipv4(&value) {
            return Some(ip);
        }
        tracing::warn!(
            env = PUBLIC_IP_ENV,
            value = %value.trim(),
            "ignored invalid public IP override"
        );
    }

    for endpoint in PUBLIC_IP_ENDPOINTS {
        match probe_endpoint(http, endpoint).await {
            Ok(ip) => return Some(ip),
            Err(error) => {
                tracing::debug!(endpoint, error = %error, "public IP probe failed");
            }
        }
    }
    None
}

async fn probe_endpoint(http: &reqwest::Client, endpoint: &str) -> anyhow::Result<String> {
    let response = http
        .get(endpoint)
        .timeout(PUBLIC_IP_TIMEOUT)
        .send()
        .await?
        .error_for_status()?;
    let body = response.text().await?;
    parse_public_ipv4_response(&body)
        .ok_or_else(|| anyhow::anyhow!("response does not contain a valid IPv4 address"))
}

fn parse_public_ipv4_response(body: &str) -> Option<String> {
    normalize_public_ipv4(body).or_else(|| {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()?
            .get("ip")?
            .as_str()
            .and_then(normalize_public_ipv4)
    })
}

#[cfg(test)]
mod tests {
    use super::{normalize_public_ipv4, parse_public_ipv4_response};

    #[test]
    fn normalizes_dotted_ipv4() {
        assert_eq!(
            normalize_public_ipv4(" 203.0.113.10 ").as_deref(),
            Some("203.0.113.10")
        );
        assert!(normalize_public_ipv4("not-an-ip").is_none());
        assert!(normalize_public_ipv4("2001:db8::1").is_none());
    }

    #[test]
    fn parses_plain_text_and_json_responses() {
        assert_eq!(
            parse_public_ipv4_response("203.0.113.10\n").as_deref(),
            Some("203.0.113.10")
        );
        assert_eq!(
            parse_public_ipv4_response(r#"{"ip":"198.51.100.7"}"#).as_deref(),
            Some("198.51.100.7")
        );
        assert!(parse_public_ipv4_response(r#"{"ip":"invalid"}"#).is_none());
    }
}
