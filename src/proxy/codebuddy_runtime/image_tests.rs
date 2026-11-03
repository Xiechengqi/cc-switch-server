#[test]
fn buddy_image_capabilities_are_strict_and_scoped_to_reviewed_cli_models() {
    for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
        for site in [CodeBuddySite::Cn, CodeBuddySite::Intl] {
            for advertised in [None, Some(false), Some(true)] {
                let mut model = json!({"id":"hy3","supportsToolCall":true});
                if let Some(advertised) = advertised {
                    model["supportsImages"] = json!(advertised);
                }
                let catalog = parse_buddy_model_catalog(&json!({"models":[model,
                    {"id":"glm-5.2","supportsImages":true},
                    {"id":"unreviewed-vision","supportsImages":true}],
                    "agents":[{"name":"cli","models":["hy3"]},{"name":"desktop","models":["glm-5.2","unreviewed-vision"]}]}), product, site).unwrap();
                assert_eq!(catalog.enabled_models, ["hy3"]);
                assert_eq!(
                    catalog.capabilities["hy3"].supports_images,
                    advertised.unwrap_or(false)
                );
                assert_eq!(
                    catalog.raw_configs["hy3"]["supportsImages"],
                    advertised.unwrap_or(false)
                );
            }
            for invalid in [Value::Null, json!(1), json!("true"), json!({})] {
                assert!(parse_buddy_model_catalog(
                    &json!({"models":[{"id":"hy3","supportsImages":invalid}]}),
                    product,
                    site
                )
                .is_err());
            }
        }
    }
}

#[test]
fn buddy_auto_image_selection_intersects_image_tool_and_reasoning_capabilities() {
    for site in [CodeBuddySite::Cn, CodeBuddySite::Intl] {
        let default = if site == CodeBuddySite::Cn {
            "default"
        } else {
            "default-model"
        };
        let catalog = parse_codebuddy_model_catalog(&json!({"models":[
            {"id":default,"supportsToolCall":true,"supportsImages":false},
            {"id":"hy3","supportsImages":true},
            {"id":"glm-5.2","supportsToolCall":true,"supportsImages":true,"supportsReasoning":true,"reasoning":{"supportedEfforts":["high"]}}],
            "agents":[{"name":"cli","models":[default,"hy3","glm-5.2"]}]}), site).unwrap();
        let mut request = json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"https://example.com/receipt.jpg"}}]}]});
        assert_eq!(
            resolve_codebuddy_model_id(site, "auto", &catalog, &request).unwrap(),
            "hy3"
        );
        request["tools"] = json!([{"type":"function","function":{"name":"lookup"}}]);
        request["reasoning"] = json!({"effort":"high"});
        assert_eq!(
            resolve_codebuddy_model_id(site, "auto", &catalog, &request).unwrap(),
            "glm-5.2"
        );
        assert_eq!(
            resolve_codebuddy_model_id(site, default, &catalog, &request).unwrap(),
            default
        );
        assert!(
            build_codebuddy_payload(&request, default, &catalog.capabilities[default])
                .unwrap_err()
                .contains("does not advertise image input")
        );
        let mut revoked = catalog;
        for capability in revoked.capabilities.values_mut() {
            capability.supports_images = false;
        }
        assert!(resolve_codebuddy_model_id(site, "auto", &revoked, &request).is_err());
    }
}

#[test]
fn buddy_payload_preserves_image_bytes_urls_details_and_complete_tool_rounds() {
    let mut capability = text_capability();
    capability.supports_images = true;
    let request = json!({"messages":[
        {"role":"user","content":[{"type":"text","text":"发票识别"},{"type":"image_url","image_url":{"url":"data:image/png;base64,iVBORw0KGgo=","detail":"high"}}]},
        {"role":"assistant","content":"checking","tool_calls":[{"id":"call-image","type":"function","function":{"name":"lookup","arguments":"{}"}}]},
        {"role":"tool","tool_call_id":"call-image","content":[{"type":"image_url","image_url":{"url":"https://example.com/receipt.jpg?signature=private","detail":"low"}}]}],
        "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object","properties":{"image_url":{"type":"string"}}}}}]});
    let payload = build_codebuddy_payload(&request, "default-model", &capability).unwrap();
    for (i, message) in request["messages"].as_array().unwrap().iter().enumerate() {
        assert_eq!(payload["messages"][i + 1], *message);
    }
    assert_eq!(payload["tools"], request["tools"]);
    let text = json!({"messages":[{"role":"user","content":"hi"}],"tools":request["tools"]});
    assert!(build_codebuddy_payload(&text, "default-model", &text_capability()).is_ok());
}
