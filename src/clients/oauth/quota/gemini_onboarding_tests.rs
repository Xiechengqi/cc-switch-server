#[tokio::test]
async fn gemini_r5_discovery_never_provisions_from_tiers_or_operation_hints() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../assets/contract/r5-gemini-cli.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let (base_url, observations, server) = serve_gemini_code_assist(
            axum::http::StatusCode::OK,
            case["response"].clone(),
            axum::http::StatusCode::OK,
            json!({"buckets": []}),
        )
        .await;
        let account = gemini_code_assist_account(
            ProviderType::GeminiCli,
            json!({"testGeminiCodeAssistBaseUrl":base_url}),
            case.get("boundProject")
                .map(|project| json!({"projectId":project})),
        );
        let client = crate::infra::http::direct_client_builder().build().unwrap();
        let loaded = load_gemini_v1internal_project(&client, &account, 1_000, 5_000).await;
        let update = if case["project"].is_null() {
            let error = loaded.unwrap_err();
            assert_eq!(error.upstream_status, None);
            assert!(
                error.message.contains("no usable projectId"),
                "{}: {error:?}",
                case["id"]
            );
            assert_eq!(
                error.next_refresh_at,
                Some(1_000 + QUOTA_FAILURE_COOLDOWN_MS)
            );
            *error.partial_update.unwrap()
        } else {
            loaded.unwrap()
        };
        assert_eq!(
            update
                .profile
                .as_ref()
                .and_then(|p| p.get("projectId"))
                .cloned()
                .unwrap_or(Value::Null),
            case["project"]
        );
        assert_eq!(
            update
                .subscription_level
                .map(Value::String)
                .unwrap_or(Value::Null),
            case["tier"]
        );
        let seen = observations.lock().unwrap();
        assert_eq!(seen.len(), 1, "{} must not onboard or poll", case["id"]);
        assert_eq!(seen[0].operation, "loadCodeAssist");
        assert_eq!(
            seen[0].body,
            json!({"metadata":fixture["scope"]["metadata"]})
        );
        assert_eq!(seen[0].authorization, "Bearer gemini-code-assist-access");
        assert_eq!(
            seen[0].user_agent,
            crate::provider_identity::gemini_cli_user_agent()
        );
        drop(seen);
        server.abort();
    }
}
