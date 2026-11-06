use super::*;

fn payload(request: Value, config: Value) -> Result<PreparedQoderPayload, String> {
    build_qoder_payload(
        &request,
        &config,
        QoderSite::Global,
        "auto",
        "fixture-session",
        "fixture-user",
        1,
    )
}

fn text() -> Value {
    json!({"messages":[{"role":"user","content":"hello"}]})
}

#[test]
fn qoder_extended_parameters_are_validated_and_preserved_without_unknown_passthrough() {
    let mut request = text();
    request["temperature"] = json!(0.25);
    request["top_p"] = json!(0.75);
    request["top_k"] = json!(32);
    request["max_tokens"] = json!(999);
    request["untrusted_internal_contract"] = json!({"allow":"everything"});
    let p = payload(request, json!({"key":"auto","max_output_tokens":512})).unwrap();
    assert_eq!(
        p.body["parameters"],
        json!({"max_tokens":512,"temperature":0.25,"top_p":0.75,"top_k":32})
    );
    assert!(p.body.get("untrusted_internal_contract").is_none());
    for (field, value) in [
        ("temperature", json!(2.1)),
        ("top_p", json!(-1)),
        ("top_k", json!(0)),
        ("max_tokens", json!("12")),
        ("stop", json!(["END"])),
        ("seed", json!(1)),
        ("n", json!(2)),
        ("parallel_tool_calls", json!(false)),
    ] {
        let mut request = text();
        request[field] = value;
        assert!(payload(request, json!({"key":"auto"})).is_err(), "{field}");
    }
}

#[test]
fn qoder_native_controls_cannot_be_silently_lost_by_protocol_conversion() {
    use crate::domain::providers::model::AppKind;
    for (app, request) in [
        (
            AppKind::Claude,
            json!({"tool_choice":{"type":"auto","disable_parallel_tool_use":true}}),
        ),
        (AppKind::Gemini, json!({"generationConfig":"invalid"})),
        (
            AppKind::Gemini,
            json!({"generationConfig":{"candidateCount":2}}),
        ),
        (
            AppKind::Gemini,
            json!({"generation_config":{"stop_sequences":["END"]}}),
        ),
        (
            AppKind::Gemini,
            json!({"generationConfig":{"responseMimeType":"text/plain","responseSchema":{"type":"object"}}}),
        ),
        (
            AppKind::Gemini,
            json!({"toolConfig":{"functionCallingConfig":{"mode":"UNKNOWN"}}}),
        ),
    ] {
        assert!(source_controls(app, &request).is_err(), "{request}");
    }
    let controls = source_controls(
        AppKind::Gemini,
        &json!({"generation_config":{"top_k":4,"max_output_tokens":512,"top_p":0.5}}),
    )
    .unwrap();
    assert_eq!(controls["top_k"], 4);
    assert_eq!(controls["max_tokens"], 512);
    assert_eq!(controls["top_p"], 0.5);
}

#[test]
fn qoder_vision_input_uses_exact_live_authority_and_keeps_original_content() {
    let url = "data:image/png;base64,iVBORw0KGgo=";
    let request = json!({"messages":[{"role":"user","content":[{"type":"text","text":"first"},{"type":"image_url","image_url":{"url":url}},{"type":"text","text":"last"}]}]});
    for config in [
        json!({"key":"auto"}),
        json!({"key":"auto","is_vl":false}),
        json!({"key":"auto","is_vl":"true"}),
    ] {
        assert!(payload(request.clone(), config).is_err());
    }
    let p = payload(request.clone(), json!({"key":"auto","is_vl":true})).unwrap();
    assert_eq!(
        p.body["messages"][0]["content"],
        request["messages"][0]["content"]
    );
    assert_eq!(
        p.body["messages"][0]["contents"][1],
        request["messages"][0]["content"][1]
    );
    assert!(p.body["image_urls"].is_null());
    let catalog = parse_qoder_model_catalog(
        &json!({"chat":[{"key":"auto","is_vl":true,"max_output_tokens":512}]}),
        QoderSite::Global,
    )
    .unwrap();
    assert_eq!(
        catalog.capabilities["auto"].input_modalities,
        ["text", "image"]
    );
    assert_eq!(catalog.capabilities["auto"].max_output_tokens, Some(512));
}

#[test]
fn qoder_media_bounds_and_native_preflight_cannot_be_bypassed_by_the_bridge() {
    use crate::domain::providers::model::AppKind;
    for url in [
        "http://example.com/a.png",
        "https://u:p@example.com/a.png",
        "https://example.com/a.png#fragment",
        "data:image/jpeg;base64,iVBORw0KGgo=",
        "data:image/png;base64,AA==",
    ] {
        let request = json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":url}}]}]});
        assert!(payload(request, json!({"key":"auto","is_vl":true})).is_err());
    }
    for (app, request) in [
        (
            AppKind::Claude,
            json!({"messages":[{"role":"user","content":[{"type":"image","source":{"type":"base64","data":"abc"}}]}]}),
        ),
        (
            AppKind::Codex,
            json!({"input":[{"role":"user","content":[{"type":"input_image","file_id":"file-a"}]}]}),
        ),
        (
            AppKind::Gemini,
            json!({"contents":[{"parts":[{"text":"oops","inlineData":{"mimeType":"image/png","data":"abc"}}]}]}),
        ),
        (
            AppKind::Claude,
            json!({"system":{"type":"image","source":{"type":"url","url":"https://example.invalid/a.png"}}}),
        ),
        (
            AppKind::Claude,
            json!({"messages":[{"role":"user","content":[{"type":"input_image","image_url":"https://example.invalid/a.png"}]}]}),
        ),
        (
            AppKind::Codex,
            json!({"input":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"https://example.invalid/a.png"}}]}]}),
        ),
        (
            AppKind::Gemini,
            json!({"systemInstruction":{"parts":{"inlineData":{"mimeType":"image/png","data":"abc"}}}}),
        ),
        (
            AppKind::Gemini,
            json!({"contents":[{"role":"model","parts":[{"inlineData":{"mimeType":"image/png","data":"abc"}}]}]}),
        ),
        (
            AppKind::Gemini,
            json!({"contents":[{"role":"user","parts":[{"fileData":{"mimeType":"image/png","data":"abc"}}]}]}),
        ),
    ] {
        assert!(validate_source_images(app, &request).is_err());
    }
    // An image-shaped schema is not media input.
    assert!(validate_source_images(
        AppKind::Codex,
        &json!({"tools":[{"parameters":{"type":"input_image"}}],"input":"image_url"})
    )
    .is_ok());
    let image =
        json!({"type":"image_url","image_url":{"url":"https://images.example.invalid/a.png"}});
    assert!(payload(
        json!({"messages":[{"role":"user","content":vec![image;17]}]}),
        json!({"key":"auto","is_vl":true})
    )
    .is_err());
}

#[test]
fn qoder_history_reasoning_preserves_text_signature_and_opaque_item_and_can_disable() {
    let mut request = json!({"messages":[{"role":"assistant","content":"answer","reasoning_content":"thought","reasoning_content_signature":"fixture-signature","reasoning_item":{"type":"reasoning","encrypted_content":"fixture-opaque"}},{"role":"user","content":"next"}]});
    let p = payload(request.clone(), json!({"key":"auto"})).unwrap();
    for field in [
        "reasoning_content",
        "reasoning_content_signature",
        "reasoning_item",
    ] {
        assert_eq!(p.body["messages"][0][field], request["messages"][0][field]);
    }
    request["preserve_thinking"] = json!(false);
    let p = payload(request, json!({"key":"auto"})).unwrap();
    assert!(p.body["messages"][0].get("reasoning_content").is_none());
    assert!(p.body["messages"][0].get("reasoning_item").is_none());
    let p = claude_to_chat(&json!({"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"only reasoning","signature":"fixture-sig"}]},{"role":"user","content":"next"}]})).unwrap();
    assert_eq!(p["messages"][0]["reasoning_content"], "only reasoning");
    assert_eq!(
        p["messages"][0]["reasoning_content_signature"],
        "fixture-sig"
    );
    assert!(claude_to_chat(&json!({"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"a","signature":"ccswitch-server-reasoning-v1:tampered"}]}]})).is_err());
}

#[test]
fn qoder_context_tiers_use_live_bounds_and_reject_unadvertised_choices() {
    let config = json!({"key":"auto","max_input_tokens":200000,"available_context_windows":[64000,128000,200000],"default_context_window":128000});
    let catalog =
        parse_qoder_model_catalog(&json!({"chat":[config.clone()]}), QoderSite::Global).unwrap();
    assert_eq!(catalog.capabilities["auto"].max_input_tokens, Some(128000));
    let mut request = text();
    request["context_length"] = json!(64000);
    let p = payload(request.clone(), config.clone()).unwrap();
    assert_eq!(p.body["parameters"]["context_length"], 64000);
    assert_eq!(p.body["model_config"]["max_input_tokens"], 64000);
    assert_eq!(
        p.body["chat_context"]["extra"]["ideModelConfigOverride"]["max_input_tokens"],
        64000
    );
    request["context_length"] = json!(96000);
    assert!(payload(request, config).is_err());
    assert!(parse_qoder_model_catalog(
        &json!({"chat":[{"key":"auto","available_context_windows":[1,"bad"]}]}),
        QoderSite::Global
    )
    .is_err());
    for config in [
        json!({"key":"auto","max_input_tokens":64000,"available_context_windows":[64000,128000]}),
        json!({"key":"auto","max_input_tokens":64000,"default_context_window":128000}),
    ] {
        assert!(payload(text(), config).is_err());
    }
    let p = payload(
        text(),
        json!({"key":"auto","max_input_tokens":200000,"available_context_windows":[64000,128000]}),
    )
    .unwrap();
    assert_eq!(p.body["model_config"]["max_input_tokens"], 128000);
}

#[test]
fn qoder_additive_official_observations_match_production_projection() {
    let evidence: Value = serde_json::from_str(include_str!(
        "../../../assets/contract/qoder-capability-extension.json"
    ))
    .unwrap();
    assert_eq!(evidence["liveState"], "live_pending");
    let image = &evidence["observations"]["image"];
    let chat = claude_to_chat(&json!({"messages":[image["input"]]})).unwrap();
    let p = payload(chat, json!({"key":"auto","is_vl":true})).unwrap();
    assert_eq!(
        p.body["messages"][0]["content"],
        image["output"][0]["content"]
    );
    let reasoning = &evidence["observations"]["reasoning"];
    let chat = claude_to_chat(&json!({"messages":[reasoning["input"]]})).unwrap();
    let p = payload(chat, json!({"key":"auto"})).unwrap();
    for field in [
        "reasoning_content",
        "reasoning_content_signature",
        "reasoning_item",
    ] {
        assert_eq!(p.body["messages"][0][field], reasoning["output"][0][field]);
    }
    let sampling = &evidence["observations"]["sampling"];
    let mut request = text();
    request["temperature"] = sampling["input"]["temperature"].clone();
    request["top_p"] = sampling["input"]["topP"].clone();
    request["top_k"] = sampling["input"]["topK"].clone();
    request["max_tokens"] = sampling["input"]["maxTokens"].clone();
    assert_eq!(
        payload(request, json!({"key":"auto"})).unwrap().body["parameters"],
        sampling["output"]
    );
}
