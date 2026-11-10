#[test]
fn static_r5_all_26_profiles_compile_exact_rails_models_auth_and_routes() {
    let registry = crate::domain::providers::registry::provider_registry();
    let accounts = AccountStore::default();
    let profiles: Vec<_> = registry
        .profiles
        .iter()
        .filter(|profile| {
            profile.coding_plan.is_some()
                || matches!(
                    profile.profile_id.as_str(),
                    "claude.aws_bedrock_aksk"
                        | "claude.aws_bedrock_api_key"
                        | "claude.nvidia"
                        | "codex.nvidia"
                        | "claude.deepseek_api"
                        | "codex.deepseek_api"
                )
        })
        .collect();
    assert_eq!(profiles.len(), 26);
    for profile in profiles {
        let id = profile.profile_id.as_str();
        let coding = profile.coding_plan.as_ref();
        let model = coding
            .map(|contract| contract.models[0].id.as_str())
            .or(profile.default_upstream_model.as_deref())
            .unwrap();
        for stream in [false, true] {
            let execution = typed_execution(
                profile.app,
                id,
                Provider {
                    id: id.to_string(),
                    name: id.to_string(),
                    settings_config: json!({"apiKey":"provider-key","env":{"ANTHROPIC_BASE_URL":"https://bedrock-runtime.us-west-2.amazonaws.com","ANTHROPIC_AUTH_TOKEN":"provider-key","OPENAI_API_KEY":"provider-key","AWS_BEARER_TOKEN_BEDROCK":"provider-key","AWS_REGION":"us-west-2","AWS_ACCESS_KEY_ID":"AKIATESTSTATIC","AWS_SECRET_ACCESS_KEY":"test-secret","AWS_SESSION_TOKEN":"test-session"}}),
                    category: None,
                    meta: None,
                    extra: Default::default(),
                },
                &accounts,
                4,
            );
            let stored = execution.runtime_stored_view();
            let adapter = adapters::adapter_for(profile.app, stored.provider_type);
            let route = if profile.app == AppKind::Claude {
                ProxyRoute::ClaudeMessages
            } else {
                ProxyRoute::CodexResponses
            };
            let input = if profile.app == AppKind::Claude {
                json!({"model":model,"stream":stream,"max_tokens":128,"messages":[{"role":"user","content":"read"}],"tools":[{"name":"Read","input_schema":{"type":"object"}}]})
            } else {
                json!({"model":model,"stream":stream,"input":"read","tools":[{"type":"function","name":"Read","parameters":{"type":"object"}}]})
            };
            let mut request = adapter
                .transform_request_for_route(
                    Bytes::from(serde_json::to_vec(&input).unwrap()),
                    &stored,
                    route,
                    None,
                )
                .unwrap();
            execution.enforce_model_policy(&mut request).unwrap();
            execution.finalize_request(&mut request).unwrap();
            let mut endpoint = execution.resolve_endpoint(route, None, &request).unwrap();
            let mut headers = Vec::new();
            let auth = execution.materialize_auth(&accounts).unwrap();
            execution
                .apply_auth(&mut headers, &mut endpoint, &auth)
                .unwrap();
            execution
                .finalize_protocol_auth(&accounts, &mut request, &mut endpoint, &mut headers)
                .unwrap();
            execution
                .guard_coding_plan_request(route, &request, &endpoint)
                .unwrap();
            assert_eq!(request.actual_model.as_deref(), Some(model), "{id}");
            let header = headers
                .iter()
                .find(|(name, _)| {
                    name.eq_ignore_ascii_case("authorization")
                        || name.eq_ignore_ascii_case("x-api-key")
                })
                .unwrap();
            assert!(!header.1.is_empty(), "{id}");
            assert!(
                String::from_utf8_lossy(&request.body).contains("Read"),
                "{id}"
            );
            match id {
                "claude.aws_bedrock_aksk" | "claude.aws_bedrock_api_key" => assert!(
                    endpoint.ends_with(if stream {
                        "/converse-stream"
                    } else {
                        "/converse"
                    }),
                    "{id}: {endpoint}"
                ),
                "claude.deepseek_api" => {
                    assert_eq!(endpoint, "https://api.deepseek.com/anthropic/v1/messages")
                }
                "codex.deepseek_api" => {
                    assert_eq!(endpoint, "https://api.deepseek.com/v1/chat/completions")
                }
                "claude.nvidia" | "codex.nvidia" => assert_eq!(
                    endpoint,
                    "https://integrate.api.nvidia.com/v1/chat/completions"
                ),
                _ => {
                    let contract = execution.plan.coding_plan.as_ref().unwrap();
                    assert!(endpoint.starts_with(&contract.fixed_origin), "{id}");
                    assert!(execution
                        .guard_coding_plan_request(
                            route,
                            &request,
                            "https://other-region.invalid/v1/messages"
                        )
                        .is_err());
                }
            }
        }
    }
}
