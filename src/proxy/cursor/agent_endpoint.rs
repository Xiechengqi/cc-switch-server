use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use crate::proxy::ProxyError;

use super::agent_proto::{FieldIter, FieldValue};
use super::h2_client::cursor_transport_diagnostic;
use super::identity::{cursor_client_version_for_rail, CursorAccountData};
use super::profile::CursorProtocolRail;

const CURSOR_SERVER_CONFIG_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.ServerConfigService/GetServerConfig";
const CURSOR_AGENT_RUN_PATH: &str = "/agent.v1.AgentService/Run";
const CURSOR_AGENT_ENDPOINT_TTL_MS: i64 = 60 * 60 * 1000;
const MAX_CURSOR_SERVER_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_CURSOR_AGENT_ENDPOINT_ENTRIES: usize = 64;
const AGENT_URL_CONFIG_FIELD: u64 = 27;
const AGENT_URL_FIELD: u64 = 1;
const AGENTN_URL_FIELD: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CursorAgentEndpointScope(String);

impl CursorAgentEndpointScope {
    #[allow(clippy::too_many_arguments)]
    pub fn derive(
        app: &str,
        provider_id: &str,
        provider_revision: u64,
        credential_generation: u64,
        runtime_fingerprint: &str,
        rail: CursorProtocolRail,
        principal: &str,
        access_token: &str,
    ) -> Self {
        let token_digest = Sha256::digest(access_token.as_bytes());
        let mut hasher = Sha256::new();
        hasher.update(b"cc-switch-server:cursor-agent-endpoint:v1\0");
        for value in [
            app,
            provider_id,
            &provider_revision.to_string(),
            &credential_generation.to_string(),
            runtime_fingerprint,
            rail.label(),
            principal,
            &hex::encode(token_digest),
        ] {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value.as_bytes());
        }
        Self(format!("cursor-agent-endpoint-v1:{:x}", hasher.finalize()))
    }
}

#[derive(Debug, Clone)]
struct CachedCursorAgentEndpoint {
    value: String,
    fetched_at_ms: i64,
    expires_at_ms: i64,
}

#[derive(Debug, Default)]
pub struct CursorAgentEndpointCache {
    endpoints: RwLock<HashMap<CursorAgentEndpointScope, CachedCursorAgentEndpoint>>,
    flights: Mutex<HashMap<CursorAgentEndpointScope, Arc<Mutex<()>>>>,
}

impl CursorAgentEndpointCache {
    async fn fresh(&self, scope: &CursorAgentEndpointScope, now_ms: i64) -> Option<String> {
        let mut endpoints = self.endpoints.write().await;
        let value = endpoints
            .get(scope)
            .filter(|entry| entry.expires_at_ms > now_ms)
            .map(|entry| entry.value.clone());
        if value.is_none() {
            endpoints.remove(scope);
        }
        value
    }

    async fn insert(&self, scope: CursorAgentEndpointScope, value: String, now_ms: i64) {
        let mut endpoints = self.endpoints.write().await;
        endpoints.insert(
            scope,
            CachedCursorAgentEndpoint {
                value,
                fetched_at_ms: now_ms,
                expires_at_ms: now_ms.saturating_add(CURSOR_AGENT_ENDPOINT_TTL_MS),
            },
        );
        while endpoints.len() > MAX_CURSOR_AGENT_ENDPOINT_ENTRIES {
            let Some(oldest) = endpoints
                .iter()
                .min_by_key(|(_, entry)| entry.fetched_at_ms)
                .map(|(scope, _)| scope.clone())
            else {
                break;
            };
            endpoints.remove(&oldest);
        }
    }

    async fn lock(&self, scope: &CursorAgentEndpointScope) -> OwnedMutexGuard<()> {
        let flight = {
            let mut flights = self.flights.lock().await;
            flights.retain(|key, flight| key == scope || Arc::strong_count(flight) > 1);
            flights
                .entry(scope.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        flight.lock_owned().await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CursorAgentUrls {
    agent_url: String,
    agentn_url: String,
}

pub struct CursorAgentEndpointRequest<'a> {
    pub scope: CursorAgentEndpointScope,
    pub access_token: &'a str,
    pub rail: CursorProtocolRail,
    pub account: &'a CursorAccountData,
    pub discovery_url: &'a str,
    pub request_timeout: Duration,
}

pub async fn resolve_cursor_agent_endpoint(
    client: &reqwest::Client,
    cache: &CursorAgentEndpointCache,
    request: CursorAgentEndpointRequest<'_>,
) -> Result<String, ProxyError> {
    let CursorAgentEndpointRequest {
        scope,
        access_token,
        rail,
        account,
        discovery_url,
        request_timeout,
    } = request;
    let now_ms = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    if let Some(endpoint) = cache.fresh(&scope, now_ms).await {
        return Ok(endpoint);
    }
    let _flight = cache.lock(&scope).await;
    let now_ms = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    if let Some(endpoint) = cache.fresh(&scope, now_ms).await {
        return Ok(endpoint);
    }

    let response = client
        .post(discovery_url)
        .bearer_auth(access_token)
        .header("accept", "application/proto")
        .header("connect-protocol-version", "1")
        .header("content-type", "application/proto")
        .header("user-agent", "connect-es/1.6.1")
        .header(
            "x-cursor-client-type",
            match rail {
                CursorProtocolRail::OAuthCli => "cli",
                CursorProtocolRail::ApiKeySdk => "sdk",
            },
        )
        .header(
            "x-cursor-client-version",
            cursor_client_version_for_rail(rail, account),
        )
        .body(Vec::new())
        .timeout(request_timeout.min(Duration::from_secs(10)))
        .send()
        .await
        .map_err(|error| {
            ProxyError::bad_gateway(format!(
                "Cursor server-config discovery failed: {}",
                cursor_transport_diagnostic(&error)
            ))
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(ProxyError {
            status: match status {
                reqwest::StatusCode::UNAUTHORIZED => axum::http::StatusCode::UNAUTHORIZED,
                reqwest::StatusCode::FORBIDDEN => axum::http::StatusCode::FORBIDDEN,
                reqwest::StatusCode::TOO_MANY_REQUESTS => axum::http::StatusCode::TOO_MANY_REQUESTS,
                _ => axum::http::StatusCode::BAD_GATEWAY,
            },
            message: format!(
                "Cursor server-config discovery returned HTTP {}",
                status.as_u16()
            ),
        });
    }
    let body = read_server_config_body(response).await?;
    let urls = parse_cursor_agent_urls(&body).map_err(|message| {
        ProxyError::bad_gateway(format!(
            "Cursor server-config discovery failed closed: {message}"
        ))
    })?;
    let endpoint = format!("{}{}", urls.agent_url, CURSOR_AGENT_RUN_PATH);
    cache.insert(scope, endpoint.clone(), now_ms).await;
    Ok(endpoint)
}

async fn read_server_config_body(response: reqwest::Response) -> Result<Vec<u8>, ProxyError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CURSOR_SERVER_CONFIG_BYTES as u64)
    {
        return Err(ProxyError::bad_gateway(
            "Cursor server-config response exceeded 1 MiB",
        ));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            ProxyError::bad_gateway(format!(
                "Cursor server-config response read failed: {}",
                cursor_transport_diagnostic(&error)
            ))
        })?;
        if body.len().saturating_add(chunk.len()) > MAX_CURSOR_SERVER_CONFIG_BYTES {
            return Err(ProxyError::bad_gateway(
                "Cursor server-config response exceeded 1 MiB",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn parse_cursor_agent_urls(payload: &[u8]) -> Result<CursorAgentUrls, String> {
    let config = required_bytes_field(payload, AGENT_URL_CONFIG_FIELD)?
        .ok_or_else(|| "response omitted AgentUrlConfig field 27".to_string())?;
    let agent_url = required_bytes_field(config, AGENT_URL_FIELD)?
        .ok_or_else(|| "AgentUrlConfig omitted agentUrl field 1".to_string())?;
    let agentn_url = required_bytes_field(config, AGENTN_URL_FIELD)?
        .ok_or_else(|| "AgentUrlConfig omitted agentnUrl field 2".to_string())?;
    Ok(CursorAgentUrls {
        agent_url: validate_discovered_cursor_origin(agent_url)?,
        agentn_url: validate_discovered_cursor_origin(agentn_url)?,
    })
}

fn required_bytes_field(payload: &[u8], field_number: u64) -> Result<Option<&[u8]>, String> {
    let mut found = None;
    for field in FieldIter::new(payload) {
        let field = field.map_err(|error| format!("invalid protobuf: {error}"))?;
        if field.field != field_number {
            continue;
        }
        match field.value {
            FieldValue::Bytes(value) => found = Some(value),
            _ => {
                return Err(format!(
                    "protobuf field {field_number} has the wrong wire type"
                ))
            }
        }
    }
    Ok(found)
}

fn validate_discovered_cursor_origin(value: &[u8]) -> Result<String, String> {
    let value = std::str::from_utf8(value)
        .map_err(|_| "Agent URL is not valid UTF-8".to_string())?
        .trim();
    let url = reqwest::Url::parse(value).map_err(|_| "Agent URL is invalid".to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "Agent URL has no host".to_string())?;
    let allowed_host = host == "api5.cursor.sh" || host.ends_with(".api5.cursor.sh");
    if url.scheme() != "https"
        || !allowed_host
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || (url.path() != "" && url.path() != "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Agent URL is outside the trusted Cursor API origin set".to_string());
    }
    Ok(url.origin().ascii_serialization())
}

pub fn default_cursor_server_config_url() -> &'static str {
    CURSOR_SERVER_CONFIG_URL
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::proxy::cursor::agent_proto::{concat_bytes, encode_message, encode_string};
    use crate::proxy::cursor::identity::cursor_account_for_api_key;

    fn server_config(agent_url: &str, agentn_url: &str) -> Bytes {
        encode_message(
            AGENT_URL_CONFIG_FIELD,
            &[
                encode_string(AGENT_URL_FIELD, agent_url),
                encode_string(AGENTN_URL_FIELD, agentn_url),
            ],
        )
    }

    #[test]
    fn parses_both_assigned_agent_origins_and_builds_no_untrusted_path() {
        let payload = concat_bytes(&[
            encode_string(3, "ignored"),
            server_config(
                "https://agent.eu.api5.cursor.sh",
                "https://agentn.eu.api5.cursor.sh/",
            ),
        ]);
        let urls = parse_cursor_agent_urls(&payload).unwrap();
        assert_eq!(urls.agent_url, "https://agent.eu.api5.cursor.sh");
        assert_eq!(urls.agentn_url, "https://agentn.eu.api5.cursor.sh");
    }

    #[test]
    fn malformed_or_untrusted_discovery_fails_closed() {
        for payload in [
            encode_string(1, "missing config"),
            server_config("https://agent.us.api5.cursor.sh", "https://evil.example"),
            server_config(
                "https://agent.us.api5.cursor.sh/path",
                "https://agentn.us.api5.cursor.sh",
            ),
            server_config(
                "http://agent.us.api5.cursor.sh",
                "https://agentn.us.api5.cursor.sh",
            ),
            server_config(
                "https://agent.us.api5.cursor.sh:8443",
                "https://agentn.us.api5.cursor.sh",
            ),
        ] {
            assert!(parse_cursor_agent_urls(&payload).is_err());
        }
    }

    #[tokio::test]
    async fn cache_scope_fences_provider_rail_principal_and_token() {
        let cache = CursorAgentEndpointCache::default();
        let scope = CursorAgentEndpointScope::derive(
            "claude",
            "cursor-a",
            2,
            3,
            "runtime-a",
            CursorProtocolRail::OAuthCli,
            "account-a:1:4",
            "token-a",
        );
        cache
            .insert(
                scope.clone(),
                "https://agent.us.api5.cursor.sh/agent.v1.AgentService/Run".to_string(),
                1_000,
            )
            .await;
        assert!(cache.fresh(&scope, 2_000).await.is_some());

        for other in [
            CursorAgentEndpointScope::derive(
                "claude",
                "cursor-b",
                2,
                3,
                "runtime-a",
                CursorProtocolRail::OAuthCli,
                "account-a:1:4",
                "token-a",
            ),
            CursorAgentEndpointScope::derive(
                "claude",
                "cursor-a",
                2,
                3,
                "runtime-a",
                CursorProtocolRail::ApiKeySdk,
                "account-a:1:4",
                "token-a",
            ),
            CursorAgentEndpointScope::derive(
                "claude",
                "cursor-a",
                2,
                3,
                "runtime-a",
                CursorProtocolRail::OAuthCli,
                "account-b:1:4",
                "token-a",
            ),
            CursorAgentEndpointScope::derive(
                "claude",
                "cursor-a",
                2,
                3,
                "runtime-a",
                CursorProtocolRail::OAuthCli,
                "account-a:1:4",
                "token-b",
            ),
        ] {
            assert!(cache.fresh(&other, 2_000).await.is_none());
        }
    }

    #[tokio::test]
    async fn discovery_sends_bound_bearer_and_reuses_exact_scope_cache() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let payload = server_config(
            "https://agent.us.api5.cursor.sh",
            "https://agentn.us.api5.cursor.sh",
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0u8; 1024];
                let read = socket.read(&mut chunk).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
            assert!(request
                .starts_with("post /aiserver.v1.serverconfigservice/getserverconfig http/1.1\r\n"));
            assert!(request.contains("authorization: bearer bound-session-token\r\n"));
            assert!(request.contains("connect-protocol-version: 1\r\n"));
            assert!(request.contains("content-type: application/proto\r\n"));
            assert!(request.contains("x-cursor-client-type: sdk\r\n"));
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/proto\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                payload.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(&payload).await.unwrap();
        });

        let cache = CursorAgentEndpointCache::default();
        let scope = CursorAgentEndpointScope::derive(
            "codex",
            "cursor-sdk",
            4,
            5,
            "runtime-sdk",
            CursorProtocolRail::ApiKeySdk,
            "cursor-apikey-principal",
            "bound-session-token",
        );
        let account = cursor_account_for_api_key("crsr_fixture");
        let discovery_url =
            format!("http://{address}/aiserver.v1.ServerConfigService/GetServerConfig");
        let client = reqwest::Client::new();
        let first = resolve_cursor_agent_endpoint(
            &client,
            &cache,
            CursorAgentEndpointRequest {
                scope: scope.clone(),
                access_token: "bound-session-token",
                rail: CursorProtocolRail::ApiKeySdk,
                account: &account,
                discovery_url: &discovery_url,
                request_timeout: Duration::from_secs(2),
            },
        )
        .await
        .unwrap();
        let second = resolve_cursor_agent_endpoint(
            &client,
            &cache,
            CursorAgentEndpointRequest {
                scope,
                access_token: "bound-session-token",
                rail: CursorProtocolRail::ApiKeySdk,
                account: &account,
                discovery_url: "http://127.0.0.1:1/must-not-be-called",
                request_timeout: Duration::from_secs(2),
            },
        )
        .await
        .unwrap();

        assert_eq!(
            first,
            "https://agent.us.api5.cursor.sh/agent.v1.AgentService/Run"
        );
        assert_eq!(second, first);
        server.await.unwrap();
    }
}
