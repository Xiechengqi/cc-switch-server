use std::fmt;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, Method, Request, Response, StatusCode, Url};
use serde_json::Value;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::domain::providers::coding_plan::{
    parse_kimi_quota, parse_minimax_quota, parse_volcengine_afp_quota,
    parse_volcengine_coding_quota, parse_zhipu_quota, volcengine_afp_has_active_window,
    CodingPlanQuotaAdapter, CodingPlanQuotaSpec, CodingPlanQuotaView,
};

const QUOTA_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_QUOTA_RESPONSE_BYTES: usize = 512 * 1024;
const VOLCENGINE_OPENAPI_HOST: &str = "open.volcengineapi.com";
const VOLCENGINE_API_VERSION: &str = "2024-01-01";
const VOLCENGINE_REGION: &str = "cn-beijing";
const VOLCENGINE_SERVICE: &str = "ark";
const VOLCENGINE_CONTENT_TYPE: &str = "application/json; charset=utf-8";
const VOLCENGINE_SIGNED_HEADERS: &str = "host;x-date;x-content-sha256;content-type";

pub enum CodingPlanQuotaCredentials {
    Inference(Zeroizing<String>),
    Volcengine {
        access_key_id: Zeroizing<String>,
        secret_access_key: Zeroizing<String>,
    },
}

impl CodingPlanQuotaCredentials {
    pub fn inference(value: String) -> Self {
        Self::Inference(Zeroizing::new(value))
    }

    pub fn volcengine(access_key_id: String, secret_access_key: String) -> Self {
        Self::Volcengine {
            access_key_id: Zeroizing::new(access_key_id),
            secret_access_key: Zeroizing::new(secret_access_key),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodingPlanQuotaFetchErrorKind {
    Transient,
    Authentication,
    PlanProbeMiss,
    InvalidResponse,
    Contract,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodingPlanQuotaFetchError {
    pub kind: CodingPlanQuotaFetchErrorKind,
    message: String,
}

impl CodingPlanQuotaFetchError {
    pub fn is_transient(&self) -> bool {
        self.kind == CodingPlanQuotaFetchErrorKind::Transient
    }

    fn is_plan_probe_miss(&self) -> bool {
        self.kind == CodingPlanQuotaFetchErrorKind::PlanProbeMiss
    }

    pub fn public_message(&self) -> &str {
        &self.message
    }

    fn transient(message: impl Into<String>) -> Self {
        Self::new(CodingPlanQuotaFetchErrorKind::Transient, message)
    }

    fn authentication(message: impl Into<String>) -> Self {
        Self::new(CodingPlanQuotaFetchErrorKind::Authentication, message)
    }

    fn plan_probe_miss(message: impl Into<String>) -> Self {
        Self::new(CodingPlanQuotaFetchErrorKind::PlanProbeMiss, message)
    }

    fn invalid_response(message: impl Into<String>) -> Self {
        Self::new(CodingPlanQuotaFetchErrorKind::InvalidResponse, message)
    }

    fn contract(message: impl Into<String>) -> Self {
        Self::new(CodingPlanQuotaFetchErrorKind::Contract, message)
    }

    fn new(kind: CodingPlanQuotaFetchErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for CodingPlanQuotaFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CodingPlanQuotaFetchError {}

pub fn build_coding_plan_quota_client() -> anyhow::Result<Client> {
    crate::infra::http::outbound_client_builder()?
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(4)
        .tcp_keepalive(Duration::from_secs(60))
        .no_gzip()
        .build()
        .map_err(Into::into)
}

pub async fn fetch_coding_plan_quota(
    client: &Client,
    quota: &CodingPlanQuotaSpec,
    credentials: &CodingPlanQuotaCredentials,
    observed_at_ms: i64,
) -> Result<CodingPlanQuotaView, CodingPlanQuotaFetchError> {
    match quota.adapter {
        CodingPlanQuotaAdapter::Unavailable => Ok(CodingPlanQuotaView::unavailable(
            "this coding plan does not publish an authoritative quota API",
        )),
        CodingPlanQuotaAdapter::Volcengine => {
            fetch_volcengine_quota(client, quota, credentials, observed_at_ms).await
        }
        CodingPlanQuotaAdapter::Kimi
        | CodingPlanQuotaAdapter::Zhipu
        | CodingPlanQuotaAdapter::Minimax => {
            let request = build_standard_request(client, quota, credentials)?;
            let expected_url = request.url().clone();
            let body = execute_json_request(client, request, &expected_url, quota.adapter).await?;
            parse_standard_quota(quota.adapter, &body, observed_at_ms)
        }
    }
}

fn parse_standard_quota(
    adapter: CodingPlanQuotaAdapter,
    body: &Value,
    observed_at_ms: i64,
) -> Result<CodingPlanQuotaView, CodingPlanQuotaFetchError> {
    let parsed = match adapter {
        CodingPlanQuotaAdapter::Kimi => parse_kimi_quota(body, observed_at_ms),
        CodingPlanQuotaAdapter::Zhipu => parse_zhipu_quota(body, observed_at_ms),
        CodingPlanQuotaAdapter::Minimax => parse_minimax_quota(body, observed_at_ms),
        _ => {
            return Err(CodingPlanQuotaFetchError::contract(
                "coding-plan quota adapter does not use the standard request contract",
            ))
        }
    };
    parsed.map_err(|_| {
        CodingPlanQuotaFetchError::invalid_response(
            "the coding-plan quota response did not match its reviewed contract",
        )
    })
}

fn build_standard_request(
    client: &Client,
    quota: &CodingPlanQuotaSpec,
    credentials: &CodingPlanQuotaCredentials,
) -> Result<Request, CodingPlanQuotaFetchError> {
    let endpoint = exact_quota_endpoint(quota)?;
    let CodingPlanQuotaCredentials::Inference(secret) = credentials else {
        return Err(CodingPlanQuotaFetchError::contract(
            "coding-plan quota adapter received the wrong credential rail",
        ));
    };
    let secret = required_secret(secret, "coding-plan API key is not configured")?;
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    match quota.adapter {
        CodingPlanQuotaAdapter::Kimi => {
            headers.insert("x-api-key", sensitive_header(secret)?);
        }
        CodingPlanQuotaAdapter::Zhipu => {
            headers.insert(AUTHORIZATION, sensitive_header(secret)?);
        }
        CodingPlanQuotaAdapter::Minimax => {
            let bearer = Zeroizing::new(format!("Bearer {secret}"));
            headers.insert(AUTHORIZATION, sensitive_header(&bearer)?);
        }
        _ => {
            return Err(CodingPlanQuotaFetchError::contract(
                "coding-plan quota adapter cannot build a standard request",
            ))
        }
    }
    client
        .request(Method::GET, endpoint)
        .headers(headers)
        .timeout(QUOTA_REQUEST_TIMEOUT)
        .build()
        .map_err(|_| {
            CodingPlanQuotaFetchError::contract(
                "coding-plan quota request could not be built from the Registry contract",
            )
        })
}

async fn fetch_volcengine_quota(
    client: &Client,
    quota: &CodingPlanQuotaSpec,
    credentials: &CodingPlanQuotaCredentials,
    observed_at_ms: i64,
) -> Result<CodingPlanQuotaView, CodingPlanQuotaFetchError> {
    let CodingPlanQuotaCredentials::Volcengine {
        access_key_id,
        secret_access_key,
    } = credentials
    else {
        return Err(CodingPlanQuotaFetchError::contract(
            "Volcengine quota requires its dedicated Access Key credentials",
        ));
    };
    let access_key_id =
        required_secret(access_key_id, "Volcengine Access Key ID is not configured")?;
    let secret_access_key = required_secret(
        secret_access_key,
        "Volcengine Secret Access Key is not configured",
    )?;

    let afp_request = build_volcengine_request(
        client,
        quota,
        access_key_id,
        secret_access_key,
        "GetAFPUsage",
        Utc::now(),
    )?;
    let expected_url = afp_request.url().clone();
    let afp = match execute_json_request(
        client,
        afp_request,
        &expected_url,
        CodingPlanQuotaAdapter::Volcengine,
    )
    .await
    {
        Ok(afp) => Some(afp),
        Err(error) if error.is_plan_probe_miss() => None,
        Err(error) => return Err(error),
    };
    if let Some(afp) = afp {
        let has_afp_window = volcengine_afp_has_active_window(&afp).map_err(|_| {
            CodingPlanQuotaFetchError::invalid_response(
                "the Volcengine Agent Plan quota response did not match its reviewed contract",
            )
        })?;
        if has_afp_window {
            return parse_volcengine_afp_quota(&afp, observed_at_ms).map_err(|_| {
                CodingPlanQuotaFetchError::invalid_response(
                    "the Volcengine Agent Plan quota response did not match its reviewed contract",
                )
            });
        }
    }

    let coding_request = build_volcengine_request(
        client,
        quota,
        access_key_id,
        secret_access_key,
        "GetCodingPlanUsage",
        Utc::now(),
    )?;
    let expected_url = coding_request.url().clone();
    let coding = execute_json_request(
        client,
        coding_request,
        &expected_url,
        CodingPlanQuotaAdapter::Volcengine,
    )
    .await?;
    parse_volcengine_coding_quota(&coding, observed_at_ms).map_err(|_| {
        CodingPlanQuotaFetchError::invalid_response(
            "the Volcengine Coding Plan quota response did not match its reviewed contract",
        )
    })
}

fn build_volcengine_request(
    client: &Client,
    quota: &CodingPlanQuotaSpec,
    access_key_id: &str,
    secret_access_key: &str,
    action: &str,
    now: DateTime<Utc>,
) -> Result<Request, CodingPlanQuotaFetchError> {
    let mut endpoint = exact_quota_endpoint(quota)?;
    if endpoint.scheme() != "https"
        || endpoint.host_str() != Some(VOLCENGINE_OPENAPI_HOST)
        || endpoint.path() != "/"
        || endpoint.query().is_some()
    {
        return Err(CodingPlanQuotaFetchError::contract(
            "Volcengine quota endpoint is outside the fixed OpenAPI contract",
        ));
    }
    let canonical_query = volcengine_canonical_query(action, VOLCENGINE_REGION)?;
    endpoint.set_query(Some(&canonical_query));
    let signature = volcengine_sign(
        access_key_id,
        secret_access_key,
        VOLCENGINE_REGION,
        &canonical_query,
        b"",
        now,
    );
    let mut headers = HeaderMap::new();
    headers.insert("x-date", header_value(&signature.x_date)?);
    headers.insert(
        "x-content-sha256",
        header_value(&signature.x_content_sha256)?,
    );
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static(VOLCENGINE_CONTENT_TYPE),
    );
    headers.insert(AUTHORIZATION, sensitive_header(&signature.authorization)?);
    client
        .request(Method::POST, endpoint)
        .headers(headers)
        .body(Vec::new())
        .timeout(QUOTA_REQUEST_TIMEOUT)
        .build()
        .map_err(|_| {
            CodingPlanQuotaFetchError::contract(
                "Volcengine quota request could not be built from the fixed contract",
            )
        })
}

async fn execute_json_request(
    client: &Client,
    request: Request,
    expected_url: &Url,
    adapter: CodingPlanQuotaAdapter,
) -> Result<Value, CodingPlanQuotaFetchError> {
    let response = client.execute(request).await.map_err(|_| {
        CodingPlanQuotaFetchError::transient("coding-plan quota network request failed")
    })?;
    if response.url() != expected_url {
        return Err(CodingPlanQuotaFetchError::contract(
            "coding-plan quota endpoint redirected outside its exact Registry URL",
        ));
    }
    classify_status(response, adapter).await
}

async fn classify_status(
    response: Response,
    adapter: CodingPlanQuotaAdapter,
) -> Result<Value, CodingPlanQuotaFetchError> {
    let status = response.status();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(CodingPlanQuotaFetchError::authentication(format!(
            "coding-plan quota authorization was rejected (HTTP {})",
            status.as_u16()
        )));
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Err(CodingPlanQuotaFetchError::transient(format!(
            "coding-plan quota service is temporarily unavailable (HTTP {})",
            status.as_u16()
        )));
    }
    if !status.is_success() && adapter != CodingPlanQuotaAdapter::Volcengine {
        return Err(CodingPlanQuotaFetchError::invalid_response(format!(
            "coding-plan quota request was rejected (HTTP {})",
            status.as_u16()
        )));
    }

    let body = read_bounded_body(response).await?;
    let value: Value = serde_json::from_slice(&body).map_err(|_| {
        if adapter == CodingPlanQuotaAdapter::Volcengine && !status.is_success() {
            CodingPlanQuotaFetchError::plan_probe_miss(format!(
                "Volcengine quota probe was rejected (HTTP {})",
                status.as_u16()
            ))
        } else {
            CodingPlanQuotaFetchError::invalid_response(
                "coding-plan quota service returned invalid JSON",
            )
        }
    })?;
    if adapter == CodingPlanQuotaAdapter::Volcengine {
        if let Some(code) = volcengine_error_code(&value) {
            if volcengine_auth_error_code(code) {
                return Err(CodingPlanQuotaFetchError::authentication(format!(
                    "Volcengine quota authorization was rejected ({})",
                    sanitize_error_code(code)
                )));
            }
            return Err(CodingPlanQuotaFetchError::plan_probe_miss(format!(
                "Volcengine quota OpenAPI returned an error ({})",
                sanitize_error_code(code)
            )));
        }
    }
    if !status.is_success() {
        if adapter == CodingPlanQuotaAdapter::Volcengine {
            return Err(CodingPlanQuotaFetchError::plan_probe_miss(format!(
                "Volcengine quota probe was rejected (HTTP {})",
                status.as_u16()
            )));
        }
        return Err(CodingPlanQuotaFetchError::invalid_response(format!(
            "coding-plan quota request was rejected (HTTP {})",
            status.as_u16()
        )));
    }
    Ok(value)
}

async fn read_bounded_body(response: Response) -> Result<Vec<u8>, CodingPlanQuotaFetchError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_QUOTA_RESPONSE_BYTES as u64)
    {
        return Err(CodingPlanQuotaFetchError::invalid_response(
            "coding-plan quota response exceeded the size limit",
        ));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            CodingPlanQuotaFetchError::transient(
                "coding-plan quota response was interrupted while reading",
            )
        })?;
        if body.len().saturating_add(chunk.len()) > MAX_QUOTA_RESPONSE_BYTES {
            return Err(CodingPlanQuotaFetchError::invalid_response(
                "coding-plan quota response exceeded the size limit",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn exact_quota_endpoint(quota: &CodingPlanQuotaSpec) -> Result<Url, CodingPlanQuotaFetchError> {
    let raw = quota.endpoint.as_deref().ok_or_else(|| {
        CodingPlanQuotaFetchError::contract("coding-plan quota endpoint is not configured")
    })?;
    let endpoint = Url::parse(raw).map_err(|_| {
        CodingPlanQuotaFetchError::contract("coding-plan quota endpoint is invalid")
    })?;
    if endpoint.scheme() != "https"
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(CodingPlanQuotaFetchError::contract(
            "coding-plan quota endpoint is outside the fixed HTTPS contract",
        ));
    }
    Ok(endpoint)
}

fn required_secret<'a>(
    value: &'a str,
    message: &'static str,
) -> Result<&'a str, CodingPlanQuotaFetchError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CodingPlanQuotaFetchError::contract(message));
    }
    Ok(value)
}

fn header_value(value: &str) -> Result<HeaderValue, CodingPlanQuotaFetchError> {
    HeaderValue::from_str(value).map_err(|_| {
        CodingPlanQuotaFetchError::contract(
            "coding-plan quota request contains an invalid header value",
        )
    })
}

fn sensitive_header(value: &str) -> Result<HeaderValue, CodingPlanQuotaFetchError> {
    let mut value = header_value(value)?;
    value.set_sensitive(true);
    Ok(value)
}

struct VolcengineSignature {
    authorization: String,
    x_date: String,
    x_content_sha256: String,
}

fn volcengine_canonical_query(
    action: &str,
    region: &str,
) -> Result<String, CodingPlanQuotaFetchError> {
    if !matches!(action, "GetAFPUsage" | "GetCodingPlanUsage") {
        return Err(CodingPlanQuotaFetchError::contract(
            "Volcengine quota action is not allowed by the fixed contract",
        ));
    }
    let mut pairs = [
        ("Action", action),
        ("Region", region),
        ("Version", VOLCENGINE_API_VERSION),
    ];
    pairs.sort_by(|left, right| left.0.cmp(right.0));
    Ok(pairs
        .iter()
        .map(|(key, value)| format!("{}={}", uri_encode(key), uri_encode(value)))
        .collect::<Vec<_>>()
        .join("&"))
}

fn volcengine_sign(
    access_key_id: &str,
    secret_access_key: &str,
    region: &str,
    canonical_query: &str,
    body: &[u8],
    now: DateTime<Utc>,
) -> VolcengineSignature {
    let x_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let short_date = now.format("%Y%m%d").to_string();
    let x_content_sha256 = sha256_hex(body);
    let canonical_headers = format!(
        "host:{VOLCENGINE_OPENAPI_HOST}\nx-date:{x_date}\nx-content-sha256:{x_content_sha256}\ncontent-type:{VOLCENGINE_CONTENT_TYPE}\n"
    );
    let canonical_request = format!(
        "POST\n/\n{canonical_query}\n{canonical_headers}\n{VOLCENGINE_SIGNED_HEADERS}\n{x_content_sha256}"
    );
    let credential_scope = format!("{short_date}/{region}/{VOLCENGINE_SERVICE}/request");
    let string_to_sign = format!(
        "HMAC-SHA256\n{x_date}\n{credential_scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let k_date = hmac_sha256(secret_access_key.as_bytes(), short_date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, VOLCENGINE_SERVICE.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"request");
    let signature = hex::encode(hmac_sha256(&k_signing, string_to_sign.as_bytes()));
    VolcengineSignature {
        authorization: format!(
            "HMAC-SHA256 Credential={access_key_id}/{credential_scope}, SignedHeaders={VOLCENGINE_SIGNED_HEADERS}, Signature={signature}"
        ),
        x_date,
        x_content_sha256,
    }
}

fn hmac_sha256(key: &[u8], value: &[u8]) -> Vec<u8> {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(value);
    mac.finalize().into_bytes().to_vec()
}

fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn uri_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}

fn volcengine_error_code(body: &Value) -> Option<&str> {
    body.pointer("/ResponseMetadata/Error/Code")
        .or_else(|| body.pointer("/Error/Code"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn volcengine_auth_error_code(code: &str) -> bool {
    let code = code.to_ascii_lowercase();
    [
        "accessdenied",
        "signaturedoesnotmatch",
        "invalidauthorization",
        "unauthorized",
        "invalidaccesskey",
        "invalidaccesskeyid",
        "authfailure",
    ]
    .iter()
    .any(|candidate| code.contains(candidate))
}

fn sanitize_error_code(code: &str) -> String {
    let sanitized = code
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
        .take(80)
        .collect::<String>();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::domain::providers::coding_plan::CodingPlanQuotaCredentialSlot;

    fn quota(adapter: CodingPlanQuotaAdapter, endpoint: &str) -> CodingPlanQuotaSpec {
        CodingPlanQuotaSpec {
            adapter,
            endpoint: Some(endpoint.to_string()),
            credential_slots: Vec::<CodingPlanQuotaCredentialSlot>::new(),
            cache_ttl_ms: 60_000,
            stale_ttl_ms: 900_000,
        }
    }

    fn test_client() -> Client {
        Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    #[test]
    fn standard_request_builders_lock_auth_and_endpoint_contracts() {
        let client = test_client();
        let credentials = CodingPlanQuotaCredentials::inference("plan-key".to_string());

        let kimi = build_standard_request(
            &client,
            &quota(
                CodingPlanQuotaAdapter::Kimi,
                "https://api.kimi.com/coding/v1/usages",
            ),
            &credentials,
        )
        .unwrap();
        assert_eq!(kimi.method(), Method::GET);
        assert_eq!(kimi.url().as_str(), "https://api.kimi.com/coding/v1/usages");
        assert_eq!(kimi.headers().get("x-api-key").unwrap(), "plan-key");
        assert!(kimi.headers().get(AUTHORIZATION).is_none());

        let zhipu = build_standard_request(
            &client,
            &quota(
                CodingPlanQuotaAdapter::Zhipu,
                "https://open.bigmodel.cn/api/monitor/usage/quota/limit",
            ),
            &credentials,
        )
        .unwrap();
        assert_eq!(zhipu.headers().get(AUTHORIZATION).unwrap(), "plan-key");
        assert_ne!(
            zhipu.headers().get(AUTHORIZATION).unwrap(),
            "Bearer plan-key"
        );

        let minimax = build_standard_request(
            &client,
            &quota(
                CodingPlanQuotaAdapter::Minimax,
                "https://api.minimax.io/v1/api/openplatform/coding_plan/remains",
            ),
            &credentials,
        )
        .unwrap();
        assert_eq!(
            minimax.headers().get(AUTHORIZATION).unwrap(),
            "Bearer plan-key"
        );
        assert_eq!(minimax.headers().get(ACCEPT).unwrap(), "application/json");
    }

    #[test]
    fn volcengine_signature_matches_the_fixed_reviewed_vector() {
        let now = DateTime::parse_from_rfc3339("2024-06-21T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let query = volcengine_canonical_query("GetAFPUsage", VOLCENGINE_REGION).unwrap();
        assert_eq!(
            query,
            "Action=GetAFPUsage&Region=cn-beijing&Version=2024-01-01"
        );
        let signature =
            volcengine_sign("AKLTtest", "secretkey", VOLCENGINE_REGION, &query, b"", now);
        assert_eq!(signature.x_date, "20240621T000000Z");
        assert_eq!(
            signature.x_content_sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            signature.authorization,
            "HMAC-SHA256 Credential=AKLTtest/20240621/cn-beijing/ark/request, SignedHeaders=host;x-date;x-content-sha256;content-type, Signature=4ceb5e7c3c834fe8604cccf04eeec5ab09045d2e5a36723921f8cea691b0a186"
        );
    }

    #[test]
    fn volcengine_requests_use_only_the_two_reviewed_actions() {
        let client = test_client();
        let quota = quota(
            CodingPlanQuotaAdapter::Volcengine,
            "https://open.volcengineapi.com/",
        );
        let now = DateTime::parse_from_rfc3339("2024-06-21T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let afp =
            build_volcengine_request(&client, &quota, "AKLTtest", "secretkey", "GetAFPUsage", now)
                .unwrap();
        assert_eq!(afp.method(), Method::POST);
        assert_eq!(
            afp.url().as_str(),
            "https://open.volcengineapi.com/?Action=GetAFPUsage&Region=cn-beijing&Version=2024-01-01"
        );
        assert_eq!(afp.body().and_then(reqwest::Body::as_bytes), Some(&b""[..]));
        assert_eq!(
            afp.headers().get(CONTENT_TYPE).unwrap(),
            VOLCENGINE_CONTENT_TYPE
        );
        assert!(build_volcengine_request(
            &client,
            &quota,
            "AKLTtest",
            "secretkey",
            "ListAccounts",
            now,
        )
        .is_err());
    }

    async fn serve_once(status: u16, body: &str) -> Url {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let body = body.to_string();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0_u8; 4096];
            let _ = stream.read(&mut request).await;
            let reason = match status {
                200 => "OK",
                400 => "Bad Request",
                401 => "Unauthorized",
                429 => "Too Many Requests",
                503 => "Service Unavailable",
                _ => "Error",
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        Url::parse(&format!("http://{address}/quota")).unwrap()
    }

    async fn execute_test_response(
        status: u16,
        body: &str,
        adapter: CodingPlanQuotaAdapter,
    ) -> CodingPlanQuotaFetchError {
        let url = serve_once(status, body).await;
        let request = test_client().get(url.clone()).build().unwrap();
        execute_json_request(&test_client(), request, &url, adapter)
            .await
            .unwrap_err()
    }

    #[tokio::test]
    async fn only_network_429_and_5xx_are_transient() {
        for status in [429, 503] {
            let error = execute_test_response(status, "{}", CodingPlanQuotaAdapter::Kimi).await;
            assert_eq!(error.kind, CodingPlanQuotaFetchErrorKind::Transient);
        }
        let auth = execute_test_response(401, "{}", CodingPlanQuotaAdapter::Kimi).await;
        assert_eq!(auth.kind, CodingPlanQuotaFetchErrorKind::Authentication);
        let rejected = execute_test_response(
            400,
            "{\"error\":\"credential=do-not-log\"}",
            CodingPlanQuotaAdapter::Kimi,
        )
        .await;
        assert_eq!(
            rejected.kind,
            CodingPlanQuotaFetchErrorKind::InvalidResponse
        );
        assert!(!rejected.to_string().contains("do-not-log"));
        let malformed = execute_test_response(200, "not-json", CodingPlanQuotaAdapter::Kimi).await;
        assert_eq!(
            malformed.kind,
            CodingPlanQuotaFetchErrorKind::InvalidResponse
        );
    }

    #[tokio::test]
    async fn volcengine_400_envelope_classifies_auth_without_exposing_body() {
        let error = execute_test_response(
            400,
            "{\"ResponseMetadata\":{\"Error\":{\"Code\":\"SignatureDoesNotMatch\",\"Message\":\"secret body\"}}}",
            CodingPlanQuotaAdapter::Volcengine,
        )
        .await;
        assert_eq!(error.kind, CodingPlanQuotaFetchErrorKind::Authentication);
        assert!(error.to_string().contains("SignatureDoesNotMatch"));
        assert!(!error.to_string().contains("secret body"));
    }

    #[tokio::test]
    async fn volcengine_non_auth_business_error_is_a_plan_probe_miss() {
        let error = execute_test_response(
            400,
            "{\"ResponseMetadata\":{\"Error\":{\"Code\":\"ResourceNotFound\",\"Message\":\"private detail\"}}}",
            CodingPlanQuotaAdapter::Volcengine,
        )
        .await;
        assert_eq!(error.kind, CodingPlanQuotaFetchErrorKind::PlanProbeMiss);
        assert!(error.to_string().contains("ResourceNotFound"));
        assert!(!error.to_string().contains("private detail"));

        let malformed =
            execute_test_response(400, "private non-json", CodingPlanQuotaAdapter::Volcengine)
                .await;
        assert_eq!(malformed.kind, CodingPlanQuotaFetchErrorKind::PlanProbeMiss);
        assert!(!malformed.to_string().contains("private non-json"));
    }
}
