use std::net::Ipv4Addr;
use std::time::Duration;

use serde::Deserialize;

const PUBLIC_IP_ENDPOINTS: &[&str] = &["http://3.0.3.0/", "http://3.0.2.1/", "http://3.0.2.9/"];
const PUBLIC_IP_TIMEOUT: Duration = Duration::from_secs(8);
const PUBLIC_IP_ENV: &str = "CC_SWITCH_PUBLIC_IP";

#[derive(Debug, Deserialize)]
struct PublicIpResponse {
    ip: String,
}

pub fn normalize_public_ipv4(value: &str) -> Option<String> {
    value
        .trim()
        .parse::<Ipv4Addr>()
        .ok()
        .map(|ip| ip.to_string())
}

/// Resolve the process public IPv4 once at startup.
/// Prefer `CC_SWITCH_PUBLIC_IP` when set; otherwise probe the configured endpoints in order.
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
    let body = response.json::<PublicIpResponse>().await?;
    normalize_public_ipv4(&body.ip)
        .ok_or_else(|| anyhow::anyhow!("response ip is not a valid IPv4 address"))
}

#[cfg(test)]
mod tests {
    use super::normalize_public_ipv4;

    #[test]
    fn normalizes_dotted_ipv4() {
        assert_eq!(
            normalize_public_ipv4(" 203.0.113.10 ").as_deref(),
            Some("203.0.113.10")
        );
        assert!(normalize_public_ipv4("not-an-ip").is_none());
        assert!(normalize_public_ipv4("2001:db8::1").is_none());
    }
}
