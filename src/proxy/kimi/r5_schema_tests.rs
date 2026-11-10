#[test]
fn r5_owner_regression_kimi_chat_schema_resolves_nested_refs_after_generic_conversion() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../assets/contract/r5-kimi-code.json")).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "nested_local_refs")
        .unwrap();
    let schema = &case["schema"];
    for app in [AppKind::Codex, AppKind::Gemini] {
        let body = Bytes::from(
            json!({"model":"k3","messages":[{"role":"user","content":"x"}],
            "tools":[{"type":"function","function":{"name":"read","parameters":schema}}]})
            .to_string(),
        );
        let output: Value =
            serde_json::from_slice(&normalize_request(&body, "k3", app).unwrap()).unwrap();
        let parameters = &output["tools"][0]["function"]["parameters"];
        assert_eq!(
            parameters["properties"]["branch"]["properties"]["value"],
            json!({"type":"string","description":"keep"})
        );
        assert!(parameters.get("$defs").is_none());
        assert_eq!(parameters["required"], json!(["branch"]));
        assert_eq!(parameters["additionalProperties"], false);
        assert_eq!(parameters, &case["expected"]);
    }
}

#[test]
fn r5_owner_regression_kimi_chat_schema_cycle_is_rejected_without_weakening() {
    let body = Bytes::from(json!({"model":"k3","messages":[],"tools":[{"type":"function","function":{"name":"read", "parameters":{
        "type":"object", "$defs":{"node":{"type":"object","properties":{"next":{"$ref":"#/$defs/node"}}}},
        "properties":{"head":{"$ref":"#/$defs/node"}}
    }}}]}).to_string());
    let error = normalize_request(&body, "k3", AppKind::Codex).unwrap_err();
    assert_eq!(error.status, reqwest::StatusCode::BAD_REQUEST);
}

#[test]
fn r5_owner_regression_kimi_chat_schema_expansion_has_an_output_budget() {
    let leaf = json!({"type":"string","description":"x".repeat(16 * 1024)});
    let properties: Map<String, Value> = (0..32)
        .map(|index| (format!("p{index}"), json!({"$ref":"#/$defs/leaf"})))
        .collect();
    let body = Bytes::from(json!({"model":"k3","messages":[],"tools":[{"type":"function","function":{"name":"read", "parameters":{
        "type":"object", "$defs":{"leaf":leaf}, "properties":properties
    }}}]}).to_string());
    let error = normalize_request(&body, "k3", AppKind::Codex).unwrap_err();
    assert_eq!(error.status, reqwest::StatusCode::BAD_REQUEST);
}

#[test]
fn r5_kimi_schema_preserves_conjunction_instance_data_and_native_claude() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../assets/contract/r5-kimi-code.json")).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "conflicting_ref_siblings")
        .unwrap();
    let mut schema = case["schema"].clone();
    schema["default"] = json!({"$ref":"literal instance", "$defs":{"keep":true}});
    let chat = Bytes::from(json!({"model":"k3","messages":[],"tools":[{"type":"function","function":{"name":"read","parameters":schema}}]}).to_string());
    let output: Value =
        serde_json::from_slice(&normalize_request(&chat, "k3", AppKind::Codex).unwrap()).unwrap();
    let parameters = &output["tools"][0]["function"]["parameters"];
    assert_eq!(
        parameters["properties"]["value"]["allOf"],
        json!([{"type":"string","minLength":3},{"type":"integer"}])
    );
    assert_eq!(parameters["default"], schema["default"]);
    let native = Bytes::from(json!({"model":"k3","max_tokens":32,"messages":[],"tools":[{"name":"read","input_schema":schema}]}).to_string());
    let output: Value =
        serde_json::from_slice(&normalize_request(&native, "k3", AppKind::Claude).unwrap())
            .unwrap();
    assert_eq!(output["tools"][0]["input_schema"], schema);
    for schema in [
        json!({"$ref":"https://synthetic.invalid/schema"}),
        json!({"$ref":"#/$defs/missing"}),
        json!({"$id":"other-scope","type":"object"}),
    ] {
        let body = Bytes::from(
            json!({"model":"k3","messages":[],"functions":[{"name":"read","parameters":schema}]})
                .to_string(),
        );
        let error = normalize_request(&body, "k3", AppKind::Codex).unwrap_err();
        assert_eq!(error.status, reqwest::StatusCode::BAD_REQUEST);
        assert!(!error.message.contains("synthetic.invalid"));
    }
}

#[test]
fn r5_kimi_schema_runs_after_real_chat_responses_and_gemini_bridges() {
    use crate::domain::providers::store::StoredProvider;
    use crate::proxy::adapters::adapter_for;
    use crate::proxy::ProxyRoute;
    let fixture: Value =
        serde_json::from_str(include_str!("../../../assets/contract/r5-kimi-code.json")).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "nested_local_refs")
        .unwrap();
    for (app, route) in [
        (AppKind::Codex, ProxyRoute::CodexChatCompletions),
        (AppKind::Codex, ProxyRoute::CodexResponses),
        (AppKind::Gemini, ProxyRoute::Gemini),
    ] {
        let stored = StoredProvider {
            app,
            provider: crate::domain::providers::model::Provider {
                id: "kimi-r5-schema".into(),
                name: "Kimi schema".into(),
                settings_config: json!({"modelMapping":{"mode":"single","upstreamModel":"k3"}}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::KimiCode,
            provider_type_id: "kimi_code".into(),
            resource: Default::default(),
        };
        for stream in [false, true] {
            let body = match route {
                ProxyRoute::CodexResponses => {
                    json!({"model":"k3","stream":stream,"input":"x","tools":[{"type":"function","name":"read","parameters":case["schema"]}]})
                }
                ProxyRoute::Gemini => {
                    json!({"contents":[{"role":"user","parts":[{"text":"x"}]}],"tools":[{"functionDeclarations":[{"name":"read","parameters":case["schema"]}]}]})
                }
                _ => {
                    json!({"model":"k3","stream":stream,"messages":[{"role":"user","content":"x"}],"tools":[{"type":"function","function":{"name":"read","parameters":case["schema"]}}]})
                }
            };
            let path = (app == AppKind::Gemini).then_some(if stream {
                "models/k3:streamGenerateContent"
            } else {
                "models/k3:generateContent"
            });
            let mut request = adapter_for(app, ProviderType::KimiCode)
                .transform_request_for_route(Bytes::from(body.to_string()), &stored, route, path)
                .unwrap();
            finalize_request(
                &super::tests::plan_for(app, RuntimeModelPolicy::Passthrough),
                &mut request,
            )
            .unwrap();
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(
                body["tools"][0]["function"]["parameters"], case["expected"],
                "{app:?} {route:?} {stream}"
            );
        }
    }
}
