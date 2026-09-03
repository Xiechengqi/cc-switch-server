use std::net::{Ipv4Addr, SocketAddr};

use anyhow::{anyhow, Context};
use tokio::net::UdpSocket;
use url::{Host, Url};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundIpv4Identity {
    pub address: Ipv4Addr,
    pub source: &'static str,
}

/// Resolves the local IPv4 identity selected for an HTTP target without sending
/// application data. An explicit value always wins; otherwise the kernel route
/// to the target is consulted, followed by a route-only public IPv4 probe when
/// the target cannot be resolved locally (for example with a remote-DNS proxy).
pub async fn resolve_outbound_ipv4(
    target: &Url,
    configured: Option<&str>,
) -> anyhow::Result<OutboundIpv4Identity> {
    if let Some(configured) = configured.filter(|value| !value.is_empty()) {
        let address = configured
            .parse::<Ipv4Addr>()
            .with_context(|| format!("configured outbound client IP {configured:?} is not IPv4"))?;
        if address.to_string() != configured {
            return Err(anyhow!(
                "configured outbound client IP {configured:?} is not canonical IPv4"
            ));
        }
        validate_client_ipv4(address)?;
        return Ok(OutboundIpv4Identity {
            address,
            source: "configured",
        });
    }

    for target_address in target_ipv4_addresses(target).await? {
        if let Some(address) = routed_local_ipv4(target_address).await {
            return Ok(OutboundIpv4Identity {
                address,
                source: "target_route",
            });
        }
    }

    // UDP connect only asks the kernel to select a route and local address. It
    // does not send a datagram, so this fallback does not contact the endpoint.
    if let Some(address) =
        routed_local_ipv4(SocketAddr::from((Ipv4Addr::new(1, 1, 1, 1), 443))).await
    {
        return Ok(OutboundIpv4Identity {
            address,
            source: "default_route",
        });
    }

    Err(anyhow!(
        "no routed local IPv4 address is available for {}",
        target.origin().ascii_serialization()
    ))
}

async fn target_ipv4_addresses(target: &Url) -> anyhow::Result<Vec<SocketAddr>> {
    let port = target
        .port_or_known_default()
        .ok_or_else(|| anyhow!("outbound target has no known port"))?;
    match target.host() {
        Some(Host::Ipv4(address)) => Ok(vec![SocketAddr::from((address, port))]),
        Some(Host::Ipv6(_)) => Ok(Vec::new()),
        Some(Host::Domain(host)) => match tokio::net::lookup_host((host, port)).await {
            Ok(addresses) => Ok(addresses.filter(SocketAddr::is_ipv4).collect()),
            // The HTTP client may deliberately delegate DNS to an outbound
            // proxy. Local lookup failure therefore falls through to the
            // route-only probe instead of making proxy deployments fail.
            Err(_) => Ok(Vec::new()),
        },
        None => Err(anyhow!("outbound target has no host")),
    }
}

async fn routed_local_ipv4(target: SocketAddr) -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await.ok()?;
    socket.connect(target).await.ok()?;
    let SocketAddr::V4(local) = socket.local_addr().ok()? else {
        return None;
    };
    validate_client_ipv4(*local.ip()).ok()?;
    Some(*local.ip())
}

fn validate_client_ipv4(address: Ipv4Addr) -> anyhow::Result<()> {
    if address.is_unspecified() || address.is_multicast() || address.is_broadcast() {
        anyhow::bail!("outbound client IP {address} is not a usable unicast address");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn configured_ipv4_wins_and_invalid_values_fail_closed() {
        let target = Url::parse("https://example.invalid").unwrap();
        let resolved = resolve_outbound_ipv4(&target, Some("127.0.0.1"))
            .await
            .unwrap();
        assert_eq!(resolved.address, Ipv4Addr::LOCALHOST);
        assert_eq!(resolved.source, "configured");
        assert!(resolve_outbound_ipv4(&target, Some(" 127.0.0.1"))
            .await
            .is_err());
        assert!(resolve_outbound_ipv4(&target, Some("::1")).await.is_err());
        assert!(resolve_outbound_ipv4(&target, Some("0.0.0.0"))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn loopback_target_uses_the_kernel_selected_loopback_address() {
        let target = Url::parse("http://127.0.0.1:15721").unwrap();
        let resolved = resolve_outbound_ipv4(&target, None).await.unwrap();
        assert_eq!(resolved.address, Ipv4Addr::LOCALHOST);
        assert_eq!(resolved.source, "target_route");
    }
}
