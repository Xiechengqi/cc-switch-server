// Server-owned Agy control-plane contract. Summary buckets are quota windows,
// never model IDs, model availability or permission to onboard an account.
fn agy_tier_label(tier: &Value, label: &str) -> String {
    let suffix = match tier.get("id").and_then(Value::as_str) {
        Some("g1-ultra-lite-tier") => "5x",
        Some("g1-ultra-tier") => "20x",
        _ => return label.to_string(),
    };
    let lower = label.to_ascii_lowercase();
    if lower.contains("ultra") && !lower.contains("5x") && !lower.contains("20x") {
        format!("{label} {suffix}")
    } else {
        label.to_string()
    }
}

fn agy_load_is_ineligible(body: &Value) -> bool {
    let free_allowed = body.pointer("/currentTier/id").and_then(Value::as_str) == Some("free-tier")
        || body
            .get("allowedTiers")
            .and_then(Value::as_array)
            .is_some_and(|tiers| {
                tiers
                    .iter()
                    .any(|tier| tier.get("id").and_then(Value::as_str) == Some("free-tier"))
            });
    let denied = body.get("ineligibleTiers").and_then(Value::as_array);
    denied.is_some_and(|tiers| {
        tiers.iter().any(|tier| {
            let has_reason = ["reasonCode", "reasonMessage"].iter().any(|key| {
                tier.get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
            });
            has_reason
                && (body.get("currentTier").is_none_or(Value::is_null)
                    || (body.get("gcpManaged").and_then(Value::as_bool) != Some(true)
                        && !free_allowed
                        && tier.get("tierId").and_then(Value::as_str) == Some("free-tier")))
        })
    })
}

async fn refresh_agy_summary(
    http: &reqwest::Client,
    account: &Account,
    loaded: GeminiV1InternalLoadResult,
    now_ms: i64,
    cooldown_ms: i64,
    timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let partial = loaded.account_update(account.provider_type);
    let mut request_body = json!({});
    if let Some(project) = &loaded.project_id {
        request_body["project"] = json!(project);
    }
    let request = gemini_code_assist_post_request(
        http,
        account,
        &gemini_code_assist_url(account, "retrieveUserQuotaSummary")?,
        &request_body,
        timeout,
        false,
        &loaded.client_identity,
    )?;
    let body = request_json(account.provider_type, request, now_ms)
        .await
        .map_err(|error| error.with_partial_update(partial.clone()))?;
    let quota = parse_agy_summary(
        &body,
        loaded.body,
        loaded.subscription_level.clone(),
        now_ms,
    )
    .ok_or_else(|| {
        QuotaRefreshFailure::parse(
            account.provider_type,
            "Agy quota summary has no valid quota windows",
            now_ms,
        )
        .with_partial_update(partial.clone())
    })?;
    let mut update = update_from_quota(
        quota,
        loaded.subscription_level,
        loaded.profile,
        now_ms,
        cooldown_ms,
    );
    update
        .capability_observations
        .extend(partial.capability_observations);
    // A quota group does not prove a particular Gemini/Claude/GPT model exists.
    Ok(update)
}

fn parse_agy_summary(
    body: &Value,
    load: Value,
    plan: Option<String>,
    now_ms: i64,
) -> Option<AccountQuota> {
    let groups = body.get("groups")?.as_array()?;
    let mut tiers = std::collections::BTreeMap::new();
    let mut bucket_count = 0_usize;
    for group in groups {
        for bucket in group
            .get("buckets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            bucket_count += 1;
            if bucket_count > 256 {
                return None;
            }
            let Some(id) = bucket
                .get("bucketId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty() && id.len() <= 128)
            else {
                continue;
            };
            let Some(remaining) = bucket
                .get("remainingFraction")
                .and_then(Value::as_f64)
                .filter(|remaining| remaining.is_finite() && (0.0..=1.0).contains(remaining))
            else {
                continue;
            };
            // Duplicate windows must agree, rather than inventing a merged allowance.
            let tier = AccountQuotaTier {
                name: id.to_string(),
                label: group
                    .get("displayName")
                    .and_then(Value::as_str)
                    .map(|text| text.chars().take(160).collect()),
                utilization: Some(1.0 - remaining),
                unit: Some("percent".to_string()),
                resets_at: bucket
                    .get("resetTime")
                    .and_then(Value::as_str)
                    .and_then(rfc3339_to_unix_ms),
                scope: Some("quota_window".to_string()),
                source: Some("retrieve_user_quota_summary".to_string()),
                ..Default::default()
            };
            if let Some(previous) = tiers.insert(id.to_string(), tier.clone()) {
                if previous.utilization != tier.utilization || previous.resets_at != tier.resets_at
                {
                    return None;
                }
            }
        }
    }
    if tiers.is_empty() {
        return None;
    }
    Some(AccountQuota {
        success: true,
        credential_message: plan,
        tiers: tiers.into_values().collect(),
        extra_usage: Some(
            json!({"loadCodeAssist": load, "retrieveUserQuotaSummary": body, "queriedAt": now_ms}),
        ),
    })
}

#[cfg(test)]
mod agy_native_tests {
    use super::*;

    #[test]
    fn agy_summary_preserves_windows_without_inventing_model_entitlement() {
        let quota = parse_agy_summary(&json!({"groups": [{"displayName":"Gemini", "buckets":[
            {"bucketId":"gemini-5h", "remainingFraction":0.25, "resetTime":"2026-11-16T12:00:00Z"},
            {"bucketId":"gemini-weekly", "remainingFraction":1},
            {"bucketId":"3p-5h", "remainingFraction":0},
            {"bucketId":"missing-fraction"}, {"bucketId":"bad", "remainingFraction":2}
        ]}]}), json!({}), Some("Google AI Ultra".into()), 1000).unwrap();
        assert_eq!(quota.tiers.len(), 3);
        assert_eq!(
            quota
                .tiers
                .iter()
                .find(|tier| tier.name == "gemini-5h")
                .unwrap()
                .utilization,
            Some(0.75)
        );
        assert!(quota.tiers.iter().all(|tier| tier.model_family.is_none()));
        assert!(quota
            .extra_usage
            .unwrap()
            .get("retrieveUserQuota")
            .is_none());
        assert!(parse_agy_summary(
            &json!({"buckets":[{"modelId":"gemini-x"}]}),
            json!({}),
            None,
            1
        )
        .is_none());
        assert!(parse_agy_summary(&json!({"groups":[]}), json!({}), None, 1).is_none());
    }

    #[test]
    fn agy_summary_rejects_conflicting_duplicate_or_unbounded_windows() {
        let bucket = json!({"bucketId":"window", "remainingFraction":0.5});
        assert!(parse_agy_summary(
            &json!({"groups":[{"buckets":[bucket.clone(), bucket.clone()]}]}),
            json!({}),
            None,
            1
        )
        .is_some());
        assert!(parse_agy_summary(&json!({"groups":[{"buckets":[bucket.clone(), {"bucketId":"window", "remainingFraction":0.8}]}]}), json!({}), None, 1).is_none());
        assert!(parse_agy_summary(
            &json!({"groups":[{"buckets":vec![bucket; 257]}]}),
            json!({}),
            None,
            1
        )
        .is_none());
    }

    #[test]
    fn agy_ineligibility_uses_structured_tier_evidence() {
        assert_eq!(
            agy_tier_label(&json!({"id":"g1-ultra-lite-tier"}), "Google AI Ultra"),
            "Google AI Ultra 5x"
        );
        assert_eq!(
            agy_tier_label(&json!({"id":"g1-ultra-tier"}), "Google AI Ultra"),
            "Google AI Ultra 20x"
        );
        assert_eq!(
            agy_tier_label(&json!({"id":"unknown-tier"}), "Unknown"),
            "Unknown"
        );
        let mut body = json!({"paidTier":{"name":"Ultra"},"currentTier":{"id":"standard-tier"},
            "ineligibleTiers":[{"tierId":"free-tier","reasonCode":"UNSUPPORTED_LOCATION"}]});
        assert!(agy_load_is_ineligible(&body));
        body["gcpManaged"] = json!(true);
        assert!(!agy_load_is_ineligible(&body));
        assert!(!agy_load_is_ineligible(
            &json!({"message":"UNSUPPORTED_LOCATION"})
        ));
    }

    #[tokio::test]
    async fn agy_summary_http_contract_is_rail_specific_and_preserves_partial_project() {
        use axum::{
            http::{HeaderMap, StatusCode, Uri},
            Json,
        };
        let summary = json!({"groups":[{"displayName":"Gemini","buckets":[
            {"bucketId":"gemini-5h","remainingFraction":0.5,"window":"5h"},
            {"bucketId":"gemini-weekly","remainingFraction":0.9,"window":"weekly"}]}]});
        for (status, response, expected_success) in [
            (200, summary, true),
            (200, json!({"groups":[]}), false),
            (401, json!({"error":{"code":401}}), false),
            (403, json!({"error":{"code":403}}), false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let observed = calls.clone();
            let app = axum::Router::new().fallback(axum::routing::post(
                move |uri: Uri, headers: HeaderMap, Json(body): Json<Value>| {
                    observed.lock().unwrap().push((uri.path().to_string(), headers, body));
                    let (code, body) = if uri.path() == "/v1internal:loadCodeAssist" {
                        (200, json!({"cloudaicompanionProject":"agy-project","paidTier":{"id":"g1-ultra-lite-tier","name":"Google AI Ultra"}}))
                    } else {
                        assert_eq!(uri.path(), "/v1internal:retrieveUserQuotaSummary");
                        (status, response.clone())
                    };
                    async move { (StatusCode::from_u16(code).unwrap(), Json(body)) }
                }
            ));
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let account: Account = serde_json::from_value(
                json!({"id":"agy-summary","providerType":"agy_oauth","accessToken":"agy-token",
                "raw":{"testGeminiCodeAssistBaseUrl":format!("http://{address}")}}),
            )
            .unwrap();
            let result = refresh_antigravity_quota(
                &reqwest::Client::new(),
                &account,
                1000,
                30000,
                Duration::from_secs(5),
            )
            .await;
            assert_eq!(
                result.is_ok(),
                expected_success,
                "status={status}: {result:?}"
            );
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].2, json!({"metadata":{"ideType":"ANTIGRAVITY"}}));
            assert_eq!(calls[1].2, json!({"project":"agy-project"}));
            let identity = crate::domain::antigravity_identity::antigravity_client_identity_for(
                ProviderType::AgyOAuth,
            );
            for (_, headers, _) in calls.iter() {
                assert_eq!(
                    headers.get("user-agent").unwrap().to_str().unwrap(),
                    identity.user_agent
                );
                assert_eq!(headers.get("authorization").unwrap(), "Bearer agy-token");
            }
            match result {
                Ok(update) => {
                    assert_eq!(status, 200);
                    assert_eq!(update.quota.unwrap().tiers.len(), 2);
                    assert!(update
                        .capability_observations
                        .iter()
                        .all(|item| item.antigravity_client_identity.as_ref() == Some(&identity)));
                    assert!(!update
                        .capability_observations
                        .iter()
                        .any(|item| item.dimension == MODEL_ENTITLEMENT_DIMENSION
                            && item.state == AccountCapabilityObservationState::Supported));
                }
                Err(failure) => {
                    if status != 200 {
                        assert_eq!(failure.upstream_status, Some(status));
                    }
                    let partial = failure.partial_update.unwrap();
                    assert_eq!(partial.profile.unwrap()["projectId"], "agy-project");
                    assert!(partial
                        .capability_observations
                        .iter()
                        .all(|item| item.antigravity_client_identity.as_ref() == Some(&identity)));
                }
            }
            drop(calls);
            server.abort();
        }
        let agy: Account =
            serde_json::from_value(json!({"id":"agy","providerType":"agy_oauth"})).unwrap();
        assert_eq!(
            gemini_code_assist_url(&agy, "retrieveUserQuotaSummary").unwrap(),
            "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary"
        );
        let hub: Account =
            serde_json::from_value(json!({"id":"hub","providerType":"antigravity_oauth"})).unwrap();
        assert_eq!(
            gemini_code_assist_url(&hub, "retrieveUserQuota").unwrap(),
            "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota"
        );
    }
}
