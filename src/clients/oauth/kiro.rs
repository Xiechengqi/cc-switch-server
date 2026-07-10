use reqwest::StatusCode;
use serde_json::{json, Map, Value};

use crate::clients::oauth::kiro_device::{
    default_profile_arn, fetch_available_profile_arn, fetch_usage_limits, find_email_in_value,
    machine_id_from_refresh_token, normalize_region, quota_from_usage_limits, register_client,
    register_client_with_issuer, sha256_hex, string_at, DEFAULT_REGION, DEFAULT_START_URL,
    KIRO_AUTH_METHOD_API_KEY, KIRO_AUTH_METHOD_BUILDER_ID, KIRO_ISSUER_URL,
};
use crate::clients::oauth::refresh::AccountRefreshFailure;
use crate::domain::accounts::oauth::{OAuthErrorKind, OAuthRequestBodyFormat, OAuthTokenResponse};
use crate::domain::accounts::store::{Account, AccountRefreshUpdate, UpsertAccountInput};
use crate::domain::providers::model::ProviderType;

const SOCIAL_AUTH_REGION: &str = "us-east-1";
const SOCIAL_REFRESH_BASE: &str = "https://prod.us-east-1.auth.desktop.kiro.dev";
const KIRO_SCOPES: &[&str] = &[
    "codewhisperer:completions",
    "codewhisperer:analysis",
    "codewhisperer:conversations",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KiroAuthMethod {
    BuilderId,
    Idc,
    Social,
    ExternalIdp,
    ApiKey,
}

pub async fn refresh_kiro_account(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    quota_refresh_interval_ms: i64,
) -> Result<AccountRefreshUpdate, AccountRefreshFailure> {
    if account.provider_type != ProviderType::KiroOAuth {
        return Err(AccountRefreshFailure::bad_request(format!(
            "expected kiro_oauth account, got {}",
            account.provider_type.as_str()
        )));
    }
    let method = auth_method(account);
    if method == KiroAuthMethod::ApiKey {
        return Err(AccountRefreshFailure::bad_request(
            "kiro api_key credentials do not use OAuth refresh",
        ));
    }
    let refresh_token = account
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AccountRefreshFailure::bad_request("kiro refresh token is required"))?;

    let missing_oidc_client = matches!(method, KiroAuthMethod::BuilderId | KiroAuthMethod::Idc)
        && (string_from_account(account, &["/clientId", "/client_id"]).is_none()
            || string_from_account(account, &["/clientSecret", "/client_secret"]).is_none());
    let replacement_client = if missing_oidc_client {
        Some(register_refresh_client(http, account, method).await?)
    } else {
        None
    };
    let first = refresh_with_method(http, account, method, refresh_token, replacement_client).await;
    let token_result = match first {
        Ok(result) => Ok(result),
        Err(error)
            if matches!(method, KiroAuthMethod::BuilderId | KiroAuthMethod::Idc)
                && error.status_code == StatusCode::UNAUTHORIZED.as_u16()
                && error.kind != OAuthErrorKind::InvalidGrant =>
        {
            let client = register_refresh_client(http, account, method).await?;
            refresh_with_method(http, account, method, refresh_token, Some(client)).await
        }
        Err(error) => Err(error),
    }?;

    Ok(update_from_kiro_token_response(
        account,
        token_result,
        now_ms,
        quota_refresh_interval_ms,
        http,
    )
    .await)
}

pub fn import_api_key(
    key: &str,
    region: Option<&str>,
    now_ms: i64,
) -> Result<UpsertAccountInput, AccountRefreshFailure> {
    let key = key.trim();
    if key.is_empty() {
        return Err(AccountRefreshFailure::bad_request(
            "kiro api key is required",
        ));
    }
    if !key.starts_with("ksk_") {
        return Err(AccountRefreshFailure::bad_request(
            "kiro api key must start with ksk_",
        ));
    }
    let region = normalize_region(region.unwrap_or(DEFAULT_REGION))
        .map_err(|error| AccountRefreshFailure::bad_request(error.message))?;
    let profile_arn =
        default_profile_arn(&json!({"authMethod": KIRO_AUTH_METHOD_API_KEY}), &region);
    let account_id = format!("kiro_key_{}", &sha256_hex(key)[..24]);
    Ok(UpsertAccountInput {
        id: Some(account_id.clone()),
        provider_type: ProviderType::KiroOAuth,
        email: None,
        access_token: Some(key.to_string()),
        refresh_token: None,
        id_token: None,
        token_type: Some("API_KEY".to_string()),
        api_key: Some(key.to_string()),
        scopes: KIRO_SCOPES
            .iter()
            .map(|scope| (*scope).to_string())
            .collect(),
        profile: Some(json!({
            "accountId": account_id,
            "profileArn": profile_arn,
            "authRegion": region,
            "apiRegion": region,
            "authMethod": KIRO_AUTH_METHOD_API_KEY,
            "provider": "ApiKey",
        })),
        raw: Some(json!({
            "provider": "ApiKey",
            "authMethod": KIRO_AUTH_METHOD_API_KEY,
            "apiRegion": region,
            "authRegion": region,
            "resolvedProfileArn": profile_arn,
            "importedBy": "kiro_api_key",
            "importedAtMs": now_ms,
        })),
        subscription_level: Some("Kiro API Key".to_string()),
        entitlement_status: None,
        quota_percent: None,
        quota: None,
        quota_refreshed_at: None,
        quota_next_refresh_at: None,
        expires_at: None,
        rate_limited_until: None,
        last_refresh_error: None,
    })
}

pub async fn import_validated_api_key(
    http: &reqwest::Client,
    key: &str,
    region: Option<&str>,
    now_ms: i64,
) -> Result<UpsertAccountInput, AccountRefreshFailure> {
    let mut input = import_api_key(key, region, now_ms)?;
    let region = normalize_region(region.unwrap_or(DEFAULT_REGION))
        .map_err(|error| AccountRefreshFailure::bad_request(error.message))?;
    let profile_arn = fetch_available_profile_arn(http, &region, key.trim(), Some("API_KEY"))
        .await
        .map_err(kiro_device_failure)?
        .ok_or_else(|| {
            AccountRefreshFailure::bad_gateway(
                "kiro API key validation returned no available profile",
            )
        })?;
    if let Some(profile) = input.profile.as_mut().and_then(Value::as_object_mut) {
        profile.insert("profileArn".to_string(), json!(profile_arn));
    }
    if let Some(raw) = input.raw.as_mut().and_then(Value::as_object_mut) {
        raw.insert("resolvedProfileArn".to_string(), json!(profile_arn));
        raw.insert("validatedAtMs".to_string(), json!(now_ms));
    }
    Ok(input)
}

pub fn import_credentials_json(
    credentials: Value,
    now_ms: i64,
) -> Result<UpsertAccountInput, AccountRefreshFailure> {
    let entry = select_credentials_entry(&credentials)
        .ok_or_else(|| AccountRefreshFailure::bad_request("no Kiro credential entry found"))?;
    upsert_from_credentials_entry(entry, now_ms)
}

fn upsert_from_credentials_entry(
    entry: &Value,
    now_ms: i64,
) -> Result<UpsertAccountInput, AccountRefreshFailure> {
    if let Some(api_key) = string_at(
        entry,
        &[
            "/apiKey",
            "/api_key",
            "/accessToken",
            "/access_token",
            "/token",
        ],
    )
    .filter(|value| value.starts_with("ksk_"))
    {
        return import_api_key(
            &api_key,
            string_at(entry, &["/apiRegion", "/region"]).as_deref(),
            now_ms,
        );
    }

    let refresh_token = string_at(entry, &["/refreshToken", "/refresh_token"])
        .ok_or_else(|| AccountRefreshFailure::bad_request("Kiro credentials lack refreshToken"))?;
    let access_token = string_at(entry, &["/accessToken", "/access_token"])
        .unwrap_or_else(|| refresh_token.clone());
    let auth_region = normalize_region(
        string_at(entry, &["/authRegion", "/auth_region", "/region"])
            .as_deref()
            .unwrap_or(DEFAULT_REGION),
    )
    .map_err(|error| AccountRefreshFailure::bad_request(error.message))?;
    let api_region = normalize_region(
        string_at(entry, &["/apiRegion", "/api_region", "/region"])
            .as_deref()
            .unwrap_or(&auth_region),
    )
    .map_err(|error| AccountRefreshFailure::bad_request(error.message))?;
    let token_endpoint = string_at(entry, &["/tokenEndpoint", "/token_endpoint"]);
    let client_id = string_at(entry, &["/clientId", "/client_id"]);
    let client_secret = string_at(entry, &["/clientSecret", "/client_secret"]);
    let auth_method = infer_import_auth_method(
        string_at(entry, &["/authMethod", "/auth_method", "/provider"]).as_deref(),
        token_endpoint.as_deref(),
        client_id.as_deref(),
        client_secret.as_deref(),
    );
    if auth_method == "external_idp" {
        let endpoint = token_endpoint.as_deref().ok_or_else(|| {
            AccountRefreshFailure::bad_request(
                "kiro external_idp credentials require tokenEndpoint",
            )
        })?;
        validate_external_idp_endpoint(endpoint)?;
        if client_id.as_deref().is_none_or(str::is_empty) {
            return Err(AccountRefreshFailure::bad_request(
                "kiro external_idp credentials require clientId",
            ));
        }
    }
    let profile_arn = string_at(
        entry,
        &["/profileArn", "/profile_arn", "/resolvedProfileArn"],
    )
    .unwrap_or_else(|| default_profile_arn(&json!({"authMethod": auth_method}), &api_region));
    let machine_id = string_at(entry, &["/machineId", "/machine_id"])
        .unwrap_or_else(|| machine_id_from_refresh_token(&refresh_token));
    let account_id = string_at(entry, &["/accountId", "/account_id"])
        .unwrap_or_else(|| format!("kiro_{}", &sha256_hex(&refresh_token)[..24]));
    let email = string_at(entry, &["/email", "/accountEmail", "/userEmail"]);
    let expires_at = timestamp_at(entry, &["/expiresAt", "/expires_at"]);
    let client_secret_expires_at = entry
        .pointer("/clientSecretExpiresAt")
        .or_else(|| entry.pointer("/client_secret_expires_at"))
        .and_then(Value::as_i64);
    let start_url = string_at(entry, &["/startUrl", "/start_url"])
        .unwrap_or_else(|| DEFAULT_START_URL.to_string());
    let provider = provider_label_for_auth_method(&auth_method);

    Ok(UpsertAccountInput {
        id: Some(account_id.clone()),
        provider_type: ProviderType::KiroOAuth,
        email: email.clone(),
        access_token: Some(access_token),
        refresh_token: Some(refresh_token),
        id_token: string_at(entry, &["/idToken", "/id_token"]),
        token_type: Some("Bearer".to_string()),
        api_key: None,
        scopes: KIRO_SCOPES
            .iter()
            .map(|scope| (*scope).to_string())
            .collect(),
        profile: Some(json!({
            "accountId": account_id,
            "email": email,
            "profileArn": profile_arn,
            "authRegion": auth_region,
            "apiRegion": api_region,
            "machineId": machine_id,
            "startUrl": start_url,
            "authMethod": auth_method,
            "provider": provider,
        })),
        raw: Some(json!({
            "provider": provider,
            "authMethod": auth_method,
            "clientId": client_id,
            "clientSecret": client_secret,
            "clientSecretExpiresAt": client_secret_expires_at,
            "tokenEndpoint": token_endpoint,
            "startUrl": start_url,
            "authRegion": auth_region,
            "apiRegion": api_region,
            "resolvedProfileArn": profile_arn,
            "machineId": machine_id,
            "importedBy": "kiro_credentials_json",
            "importedAtMs": now_ms,
        })),
        subscription_level: Some("Kiro OAuth".to_string()),
        entitlement_status: None,
        quota_percent: None,
        quota: None,
        quota_refreshed_at: None,
        quota_next_refresh_at: None,
        expires_at,
        rate_limited_until: None,
        last_refresh_error: None,
    })
}

struct KiroRefreshResult {
    token: OAuthTokenResponse,
    raw: Value,
    client_update: Option<crate::clients::oauth::kiro_device::RegisterClientResponse>,
}

async fn register_refresh_client(
    http: &reqwest::Client,
    account: &Account,
    method: KiroAuthMethod,
) -> Result<crate::clients::oauth::kiro_device::RegisterClientResponse, AccountRefreshFailure> {
    let region = auth_region(account)?;
    if method == KiroAuthMethod::Idc {
        let issuer_url = string_from_account(account, &["/issuerUrl", "/issuer_url"])
            .unwrap_or_else(|| KIRO_ISSUER_URL.to_string());
        register_client_with_issuer(http, &region, &issuer_url)
            .await
            .map_err(kiro_device_failure)
    } else {
        register_client(http, &region)
            .await
            .map_err(kiro_device_failure)
    }
}

async fn refresh_with_method(
    http: &reqwest::Client,
    account: &Account,
    method: KiroAuthMethod,
    refresh_token: &str,
    replacement_client: Option<crate::clients::oauth::kiro_device::RegisterClientResponse>,
) -> Result<KiroRefreshResult, AccountRefreshFailure> {
    match method {
        KiroAuthMethod::BuilderId | KiroAuthMethod::Idc => {
            let region = auth_region(account)?;
            let client_id = replacement_client
                .as_ref()
                .map(|client| client.client_id.clone())
                .or_else(|| string_from_account(account, &["/clientId", "/client_id"]))
                .ok_or_else(|| AccountRefreshFailure::bad_request("kiro clientId is required"))?;
            let client_secret = replacement_client
                .as_ref()
                .map(|client| client.client_secret.clone())
                .or_else(|| string_from_account(account, &["/clientSecret", "/client_secret"]))
                .ok_or_else(|| {
                    AccountRefreshFailure::bad_request("kiro clientSecret is required")
                })?;
            let body = json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "refreshToken": refresh_token,
                "grantType": "refresh_token",
            });
            let raw = post_json(
                http,
                &format!("https://oidc.{region}.amazonaws.com/token"),
                &body,
            )
            .await?;
            let token = parse_token_response(raw.clone(), "kiro oidc refresh")?;
            Ok(KiroRefreshResult {
                token,
                raw,
                client_update: replacement_client,
            })
        }
        KiroAuthMethod::Social => {
            let auth_region = string_from_account(account, &["/authRegion", "/auth_region"])
                .unwrap_or_else(|| SOCIAL_AUTH_REGION.to_string());
            let url = if auth_region == SOCIAL_AUTH_REGION {
                format!("{SOCIAL_REFRESH_BASE}/refreshToken")
            } else {
                format!("https://prod.{auth_region}.auth.desktop.kiro.dev/refreshToken")
            };
            let raw = post_json(http, &url, &json!({ "refreshToken": refresh_token })).await?;
            let token = parse_token_response(raw.clone(), "kiro social refresh")?;
            Ok(KiroRefreshResult {
                token,
                raw,
                client_update: None,
            })
        }
        KiroAuthMethod::ExternalIdp => {
            let token_endpoint = string_from_account(
                account,
                &["/tokenEndpoint", "/token_endpoint"],
            )
            .ok_or_else(|| {
                AccountRefreshFailure::bad_request("kiro external_idp tokenEndpoint is required")
            })?;
            validate_external_idp_endpoint(&token_endpoint)?;
            let client_id = string_from_account(account, &["/clientId", "/client_id"]);
            let client_secret = string_from_account(account, &["/clientSecret", "/client_secret"]);
            let mut form = vec![
                ("grant_type", "refresh_token".to_string()),
                ("refresh_token", refresh_token.to_string()),
            ];
            if let Some(client_id) = client_id {
                form.push(("client_id", client_id));
            }
            if let Some(client_secret) = client_secret {
                form.push(("client_secret", client_secret));
            }
            let response = http
                .post(&token_endpoint)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .form(&form)
                .send()
                .await
                .map_err(|error| {
                    AccountRefreshFailure::bad_gateway(format!(
                        "kiro external_idp refresh failed: {error}"
                    ))
                })?;
            let raw = response_json(response, "kiro external_idp refresh").await?;
            let token = parse_token_response(raw.clone(), "kiro external_idp refresh")?;
            Ok(KiroRefreshResult {
                token,
                raw,
                client_update: None,
            })
        }
        KiroAuthMethod::ApiKey => Err(AccountRefreshFailure::bad_request(
            "kiro api key credentials do not refresh",
        )),
    }
}

async fn update_from_kiro_token_response(
    account: &Account,
    result: KiroRefreshResult,
    now_ms: i64,
    quota_refresh_interval_ms: i64,
    http: &reqwest::Client,
) -> AccountRefreshUpdate {
    let mut raw = object_value(account.raw.clone());
    insert_value(&mut raw, "tokenResponse", result.raw.clone());
    insert_value(&mut raw, "lastRefreshAtMs", json!(now_ms));
    if let Some(client) = result.client_update {
        insert_value(&mut raw, "clientId", json!(client.client_id));
        insert_value(&mut raw, "clientSecret", json!(client.client_secret));
        insert_value(
            &mut raw,
            "clientSecretExpiresAt",
            client
                .client_secret_expires_at
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
    }

    let access_token = result.token.access_token.clone();
    let refresh_token = result
        .token
        .refresh_token
        .clone()
        .or_else(|| account.refresh_token.clone());
    let auth_region_value = string_from_value(&raw, &["/authRegion", "/auth_region"])
        .unwrap_or_else(|| DEFAULT_REGION.to_string());
    let api_region_value =
        string_from_value(&raw, &["/apiRegion", "/api_region"]).unwrap_or_else(|| {
            region_from_profile_arn(account).unwrap_or_else(|| auth_region_value.clone())
        });
    let profile_arn = string_from_value(
        &raw,
        &["/resolvedProfileArn", "/profileArn", "/profile_arn"],
    )
    .or_else(|| {
        account
            .profile
            .as_ref()
            .and_then(|profile| string_at(profile, &["/profileArn", "/profile_arn"]))
    })
    .unwrap_or_else(|| default_profile_arn(&raw, &api_region_value));
    let machine_id = string_from_value(&raw, &["/machineId", "/machine_id"])
        .or_else(|| refresh_token.as_deref().map(machine_id_from_refresh_token));
    let usage = if let Some(machine_id) = machine_id.as_deref() {
        fetch_usage_limits(
            http,
            &api_region_value,
            &profile_arn,
            machine_id,
            &access_token,
            kiro_token_type_header(account),
        )
        .await
        .ok()
    } else {
        None
    };
    if let Some(usage) = usage.as_ref() {
        insert_value(&mut raw, "kiroUsageLimits", usage.clone());
    }
    let email = result
        .token
        .extra
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            usage
                .as_ref()
                .and_then(find_email_in_value)
                .map(str::to_string)
        })
        .or_else(|| account.email.clone());
    let subscription_level = usage
        .as_ref()
        .and_then(|value| {
            string_at(
                value,
                &[
                    "/subscriptionInfo/subscriptionTitle",
                    "/subscription_info/subscription_title",
                ],
            )
        })
        .or_else(|| account.subscription_level.clone());
    let quota = usage
        .clone()
        .map(|value| quota_from_usage_limits(value, subscription_level.clone(), now_ms));

    let mut profile = object_value(account.profile.clone());
    insert_value(
        &mut profile,
        "email",
        email.clone().map(Value::from).unwrap_or(Value::Null),
    );
    insert_value(&mut profile, "profileArn", json!(profile_arn));
    insert_value(&mut profile, "authRegion", json!(auth_region_value));
    insert_value(&mut profile, "apiRegion", json!(api_region_value));
    if let Some(machine_id) = machine_id {
        insert_value(&mut profile, "machineId", json!(machine_id));
    }

    AccountRefreshUpdate {
        email,
        access_token: Some(access_token),
        refresh_token: result.token.refresh_token,
        id_token: result.token.id_token,
        token_type: Some(
            result
                .token
                .token_type
                .unwrap_or_else(|| "Bearer".to_string()),
        ),
        scopes: result.token.scope.as_deref().map(split_scopes),
        profile: Some(profile),
        raw: Some(raw),
        subscription_level,
        quota_percent: quota
            .as_ref()
            .and_then(|quota| quota.tiers.first())
            .and_then(|tier| tier.utilization)
            .map(|value| value * 100.0),
        quota,
        quota_refreshed_at: usage.as_ref().map(|_| now_ms),
        quota_next_refresh_at: usage
            .as_ref()
            .map(|_| now_ms.saturating_add(quota_refresh_interval_ms)),
        expires_at: result
            .token
            .expires_in
            .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1000))),
        last_refresh_error: None,
        ..Default::default()
    }
}

async fn post_json(
    http: &reqwest::Client,
    url: &str,
    body: &Value,
) -> Result<Value, AccountRefreshFailure> {
    let response = http
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|error| {
            AccountRefreshFailure::bad_gateway(format!("kiro refresh failed: {error}"))
        })?;
    response_json(response, "kiro refresh").await
}

async fn response_json(
    response: reqwest::Response,
    context: &str,
) -> Result<Value, AccountRefreshFailure> {
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|_| format!("HTTP {status}"));
    if !status.is_success() {
        let kind = classify_kiro_refresh_error(status, &body);
        return Err(AccountRefreshFailure {
            status_code: status.as_u16(),
            message: format!("{context} failed: {}", extract_error_message(&body)),
            kind,
            retryable: matches!(
                kind,
                OAuthErrorKind::Network
                    | OAuthErrorKind::RateLimited
                    | OAuthErrorKind::ExpiredToken
            ) || status.is_server_error(),
        });
    }
    serde_json::from_str(&body).map_err(|error| {
        AccountRefreshFailure::parse(format!("{context} response is not valid JSON: {error}"))
    })
}

fn parse_token_response(
    raw: Value,
    context: &str,
) -> Result<OAuthTokenResponse, AccountRefreshFailure> {
    serde_json::from_value(raw).map_err(|error| {
        AccountRefreshFailure::parse(format!(
            "{context} response is missing token fields: {error}"
        ))
    })
}

fn classify_kiro_refresh_error(status: StatusCode, body: &str) -> OAuthErrorKind {
    let lower = body.to_ascii_lowercase();
    if lower.contains("invalid_grant") || lower.contains("invalid grant") {
        OAuthErrorKind::InvalidGrant
    } else if status == StatusCode::UNAUTHORIZED || lower.contains("expired") {
        OAuthErrorKind::ExpiredToken
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        OAuthErrorKind::RateLimited
    } else {
        OAuthErrorKind::Network
    }
}

fn extract_error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| value.get("error_description").and_then(Value::as_str))
                .or_else(|| value.get("error").and_then(Value::as_str))
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.to_string())
}

fn validate_external_idp_endpoint(url: &str) -> Result<(), AccountRefreshFailure> {
    let parsed = reqwest::Url::parse(url).map_err(|error| {
        AccountRefreshFailure::bad_request(format!(
            "invalid kiro external_idp tokenEndpoint: {error}"
        ))
    })?;
    let host = parsed.host_str().unwrap_or_default();
    let allowed = parsed.scheme() == "https"
        && (host == "login.microsoftonline.com"
            || host == "login.microsoft.com"
            || host == "login.windows.net"
            || host.ends_with(".login.microsoftonline.com")
            || host.ends_with(".login.microsoft.com")
            || host.ends_with(".login.windows.net"));
    if allowed {
        Ok(())
    } else {
        Err(AccountRefreshFailure::bad_request(format!(
            "kiro external_idp tokenEndpoint host is not allowed: {host}"
        )))
    }
}

fn auth_region(account: &Account) -> Result<String, AccountRefreshFailure> {
    normalize_region(
        string_from_account(account, &["/authRegion", "/auth_region", "/region"])
            .as_deref()
            .unwrap_or(DEFAULT_REGION),
    )
    .map_err(|error| AccountRefreshFailure::bad_request(error.message))
}

fn auth_method(account: &Account) -> KiroAuthMethod {
    let raw = string_from_account(account, &["/authMethod", "/auth_method", "/provider"])
        .unwrap_or_else(|| KIRO_AUTH_METHOD_BUILDER_ID.to_string());
    match normalize_auth_method(Some(&raw)).as_str() {
        "idc" | "iam_sso" | "iam-sso" | "enterprise" => KiroAuthMethod::Idc,
        "social" | "google" | "github" => KiroAuthMethod::Social,
        "external_idp" | "external-idp" | "externalidp" => KiroAuthMethod::ExternalIdp,
        "api_key" | "api-key" | "apikey" => KiroAuthMethod::ApiKey,
        _ => KiroAuthMethod::BuilderId,
    }
}

fn kiro_token_type_header(account: &Account) -> Option<&'static str> {
    match auth_method(account) {
        KiroAuthMethod::ExternalIdp => Some("EXTERNAL_IDP"),
        KiroAuthMethod::ApiKey => Some("API_KEY"),
        _ => None,
    }
}

fn normalize_auth_method(value: Option<&str>) -> String {
    let lower = value
        .unwrap_or(KIRO_AUTH_METHOD_BUILDER_ID)
        .trim()
        .to_ascii_lowercase();
    match lower.as_str() {
        "builderid" | "builder_id" | "builder-id" | "builder id" | "builder" => {
            KIRO_AUTH_METHOD_BUILDER_ID.to_string()
        }
        "iam_sso" | "iam-sso" | "idc" | "identity_center" => "idc".to_string(),
        "google" | "github" | "social" | "import" | "imported" => "social".to_string(),
        "external-idp" | "externalidp" | "external_idp" | "entra" | "microsoft" => {
            "external_idp".to_string()
        }
        "api-key" | "apikey" | "api_key" => KIRO_AUTH_METHOD_API_KEY.to_string(),
        _ => lower,
    }
}

fn infer_import_auth_method(
    value: Option<&str>,
    token_endpoint: Option<&str>,
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> String {
    let raw = value.unwrap_or_default().trim().to_ascii_lowercase();
    let normalized = normalize_auth_method(value);
    if token_endpoint.is_some_and(|endpoint| !endpoint.trim().is_empty()) {
        return "external_idp".to_string();
    }
    if matches!(normalized.as_str(), "external_idp" | "api_key") {
        return normalized;
    }
    if matches!(raw.as_str(), "google" | "github" | "social") {
        return "social".to_string();
    }
    if client_id.is_some_and(|value| !value.trim().is_empty())
        && client_secret.is_some_and(|value| !value.trim().is_empty())
    {
        return if normalized == KIRO_AUTH_METHOD_BUILDER_ID {
            KIRO_AUTH_METHOD_BUILDER_ID.to_string()
        } else {
            "idc".to_string()
        };
    }
    if matches!(normalized.as_str(), "idc") {
        return normalized;
    }
    "social".to_string()
}

fn provider_label_for_auth_method(method: &str) -> &'static str {
    match method {
        "social" => "Social",
        "idc" => "IdC",
        "external_idp" => "ExternalIdP",
        "api_key" => "ApiKey",
        _ => "BuilderId",
    }
}

fn select_credentials_entry(value: &Value) -> Option<&Value> {
    if credential_entry_score(value) > 0 {
        return Some(value);
    }
    for pointer in ["/credentials", "/profiles", "/accounts", "/items"] {
        if let Some(array) = value.pointer(pointer).and_then(Value::as_array) {
            if let Some(entry) = array
                .iter()
                .max_by_key(|entry| credential_entry_score(entry))
            {
                if credential_entry_score(entry) > 0 {
                    return Some(entry);
                }
            }
        }
        if let Some(object) = value.pointer(pointer).and_then(Value::as_object) {
            if let Some(entry) = object
                .values()
                .max_by_key(|entry| credential_entry_score(entry))
            {
                if credential_entry_score(entry) > 0 {
                    return Some(entry);
                }
            }
        }
    }
    value
        .as_object()
        .and_then(|object| {
            object
                .values()
                .max_by_key(|entry| credential_entry_score(entry))
        })
        .filter(|entry| credential_entry_score(entry) > 0)
}

fn credential_entry_score(value: &Value) -> usize {
    let mut score = 0;
    if string_at(value, &["/refreshToken", "/refresh_token"]).is_some() {
        score += 10;
    }
    if string_at(value, &["/accessToken", "/access_token"]).is_some() {
        score += 3;
    }
    if string_at(value, &["/apiKey", "/api_key"]).is_some() {
        score += 10;
    }
    if string_at(value, &["/clientId", "/client_id"]).is_some() {
        score += 2;
    }
    if string_at(value, &["/profileArn", "/profile_arn"]).is_some() {
        score += 2;
    }
    score
}

fn string_from_account(account: &Account, pointers: &[&str]) -> Option<String> {
    account
        .raw
        .as_ref()
        .and_then(|value| string_at(value, pointers))
        .or_else(|| {
            account
                .profile
                .as_ref()
                .and_then(|value| string_at(value, pointers))
        })
}

fn string_from_value(value: &Value, pointers: &[&str]) -> Option<String> {
    string_at(value, pointers)
}

fn region_from_profile_arn(account: &Account) -> Option<String> {
    let arn = account
        .profile
        .as_ref()
        .and_then(|value| string_at(value, &["/profileArn", "/profile_arn"]))
        .or_else(|| {
            account
                .raw
                .as_ref()
                .and_then(|value| string_at(value, &["/resolvedProfileArn", "/profileArn"]))
        })?;
    let mut parts = arn.split(':');
    (parts.next() == Some("arn")).then_some(())?;
    (parts.next() == Some("aws")).then_some(())?;
    (parts.next() == Some("codewhisperer")).then_some(())?;
    parts.next().map(str::to_string)
}

fn object_value(value: Option<Value>) -> Value {
    value
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()))
}

fn insert_value(object: &mut Value, key: &str, value: Value) {
    if let Some(map) = object.as_object_mut() {
        map.insert(key.to_string(), value);
    }
}

fn split_scopes(scopes: &str) -> Vec<String> {
    scopes
        .split_whitespace()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn timestamp_at(value: &Value, pointers: &[&str]) -> Option<i64> {
    let value = pointers.iter().find_map(|pointer| value.pointer(pointer))?;
    if let Some(number) = value.as_i64() {
        return Some(if number.abs() < 10_000_000_000 {
            number.saturating_mul(1000)
        } else {
            number
        });
    }
    let raw = value.as_str()?.trim();
    raw.parse::<i64>()
        .ok()
        .map(|number| {
            if number.abs() < 10_000_000_000 {
                number.saturating_mul(1000)
            } else {
                number
            }
        })
        .or_else(|| {
            chrono::DateTime::parse_from_rfc3339(raw)
                .ok()
                .map(|value| value.timestamp_millis())
        })
}

fn kiro_device_failure(
    error: crate::clients::oauth::kiro_device::KiroDeviceError,
) -> AccountRefreshFailure {
    AccountRefreshFailure {
        status_code: error.status.as_u16(),
        message: error.message,
        kind: if error.status == StatusCode::TOO_MANY_REQUESTS {
            OAuthErrorKind::RateLimited
        } else if error.status == StatusCode::UNAUTHORIZED {
            OAuthErrorKind::ExpiredToken
        } else {
            OAuthErrorKind::Network
        },
        retryable: error.status.is_server_error() || error.status == StatusCode::TOO_MANY_REQUESTS,
    }
}

#[allow(dead_code)]
async fn register_client_for_start_url(
    http: &reqwest::Client,
    region: &str,
    issuer_url: Option<&str>,
) -> Result<crate::clients::oauth::kiro_device::RegisterClientResponse, AccountRefreshFailure> {
    let issuer = issuer_url.unwrap_or(KIRO_ISSUER_URL);
    register_client_with_issuer(http, region, issuer)
        .await
        .map_err(kiro_device_failure)
}

#[allow(dead_code)]
fn _request_body_format_marker() -> OAuthRequestBodyFormat {
    OAuthRequestBodyFormat::Json
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_idp_endpoint_allows_only_microsoft_login_hosts() {
        assert!(validate_external_idp_endpoint(
            "https://login.microsoftonline.com/common/oauth2/v2.0/token"
        )
        .is_ok());
        assert!(validate_external_idp_endpoint("https://evil.example/token").is_err());
    }

    #[test]
    fn imports_api_key_as_non_refreshing_kiro_account() {
        let account = import_api_key("ksk_fixture", Some("US-EAST-1"), 1000).unwrap();
        assert_eq!(account.provider_type, ProviderType::KiroOAuth);
        assert_eq!(account.api_key.as_deref(), Some("ksk_fixture"));
        assert_eq!(account.refresh_token, None);
        assert_eq!(
            account
                .profile
                .as_ref()
                .and_then(|value| value.pointer("/authMethod"))
                .and_then(Value::as_str),
            Some("api_key")
        );
    }

    #[test]
    fn imports_credentials_json_entry() {
        let account = import_credentials_json(
            json!({
                "credentials": {
                    "default": {
                        "accessToken": "access",
                        "refreshToken": "refresh",
                        "clientId": "client",
                        "clientSecret": "secret",
                        "authMethod": "google",
                        "email": "user@example.com"
                    }
                }
            }),
            1000,
        )
        .unwrap();
        assert_eq!(account.id.as_deref(), Some("kiro_d6cc0a088c07683c65cd2668"));
        assert_eq!(account.email.as_deref(), Some("user@example.com"));
        assert_eq!(
            account
                .profile
                .as_ref()
                .and_then(|value| value.pointer("/authMethod"))
                .and_then(Value::as_str),
            Some("social")
        );
    }

    #[test]
    fn import_infers_external_idp_and_social_auth_methods() {
        assert_eq!(
            infer_import_auth_method(
                Some("imported"),
                Some("https://login.microsoftonline.com/common/oauth2/v2.0/token"),
                Some("client"),
                None,
            ),
            "external_idp"
        );
        assert_eq!(
            infer_import_auth_method(Some("imported"), None, None, None),
            "social"
        );
        assert!(import_credentials_json(
            json!({
                "accessToken": "access",
                "refreshToken": "refresh",
                "authMethod": "external_idp",
                "tokenEndpoint": "https://login.microsoftonline.com/common/oauth2/v2.0/token"
            }),
            1000,
        )
        .is_err());
    }

    #[test]
    fn import_parses_iso_expiry_without_retaining_raw_credentials() {
        let account = import_credentials_json(
            json!({
                "accessToken": "access",
                "refreshToken": "refresh",
                "authMethod": "social",
                "expiresAt": "2025-12-31T00:00:00Z"
            }),
            1000,
        )
        .unwrap();
        assert_eq!(account.expires_at, Some(1_767_139_200_000));
        assert!(account
            .raw
            .as_ref()
            .and_then(|value| value.get("credentialSource"))
            .is_none());
    }
}
