// Frozen from the official authorization page on 2026-11-06:
// page.b9f91f9896.js, SHA256 2ac2c688e1715c2524188583f9cd44b2067cb4c9ed277dc3ca8a1b6caa33cfa0.
// This external callback contract was missing from the original flow fixtures.
#[test]
fn trae_browser_contract_requires_plain_ipv4_authorize_and_pkce_for_every_channel() {
    let website_callback = regex::Regex::new(r"^http://127\.0\.0\.1:(\d+)/authorize$").unwrap();
    for provider_type in [
        ProviderType::TraeSolo,
        ProviderType::TraeRaw,
        ProviderType::TraeIntl,
    ] {
        let (start, flow) = start_login_for_provider(
            "http://127.0.0.1:15721/authorize",
            provider_type,
            None,
            1000,
        )
        .unwrap();
        let url = Url::parse(&start.auth_url).unwrap();
        let query = url.query_pairs().collect::<BTreeMap<_, _>>();
        assert!(website_callback.is_match(&query["auth_callback_url"]));
        assert_eq!(
            query["auth_callback_url"],
            "http://127.0.0.1:15721/authorize"
        );
        assert_eq!(query["auth_from"], "trae");
        assert_eq!(query["client_id"], "ono9krqynydwx5");
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(query["login_trace_id"], start.flow_id);
        assert_eq!(start.flow_id.len(), 64);
        assert_eq!(flow.code_verifier.len(), 43);
        assert_eq!(
            query["code_challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(flow.code_verifier.as_bytes()))
        );
        let exposed = format!("{} {flow:?}", serde_json::to_string(&start).unwrap());
        assert!(!exposed.contains(flow.code_verifier.as_str()));
        assert!(!exposed.contains(flow.device_private_key_pem.as_str()));
        assert_eq!(
            flow.endpoints.code_exchange_origin.as_str(),
            if provider_type == ProviderType::TraeIntl {
                "https://grow-normal.trae.ai/"
            } else {
                "https://api.trae.cn/"
            }
        );
    }
    for bad_base in [
        "http://localhost:15721/authorize",
        "http://127.0.0.1:15721/api/accounts/trae/login/callback",
        "http://127.0.0.1:15721/authorize?flowId=test",
        "http://127.0.0.1:0/authorize",
        "http://127.0.0.1:65536/authorize",
        "https://127.0.0.1:15721/authorize",
    ] {
        assert!(start_login(bad_base, None, 0).is_err());
    }
}

#[test]
fn trae_callback_rejects_ambiguous_state_codes_addresses_and_legacy_downgrades() {
    let (start, _) = start_login("http://127.0.0.1:15721/authorize", None, 1000).unwrap();
    let valid = callback_from_start(&start, "code-never-log-me");
    assert!(parse_callback_url(&valid).is_ok());
    for suffix in [
        "&loginTraceID=",
        "&login_trace_id=other",
        "&scope=trae",
        "&scope=solo",
        "&authCodeInfo=%7B%22AuthCode%22%3A%22other%22%7D",
        "&auth_code_info=",
        "&refreshToken=secret",
        "&userJwt=secret",
        "&code=secret",
        "&flowId=other",
        "#fragment",
        "&accessToken=secret",
    ] {
        assert!(
            parse_callback_url(&format!("{valid}{suffix}")).is_err(),
            "{suffix}"
        );
    }
    for replacement in [
        "http://localhost:15721/authorize",
        "https://127.0.0.1:15721/authorize",
        "http://127.0.0.1:15721/callback",
        "http://127.0.0.1:15721/authorize/",
        "http://127.0.0.1:15721/%61uthorize",
        "http://attacker.invalid:15721/authorize",
        "http://user@127.0.0.1:15721/authorize",
        "http://127.1:15721/authorize",
    ] {
        assert!(parse_callback_url(&valid.replacen(
            "http://127.0.0.1:15721/authorize",
            replacement,
            1
        ))
        .is_err());
    }
    for info in [
        r#"{"AuthCode":""}"#,
        r#"{"AuthCode":"one","AuthCode":"two"}"#,
        r#"{"AuthCode":"secret",bad}"#,
    ] {
        let mut url = Url::parse("http://127.0.0.1:15721/authorize").unwrap();
        url.query_pairs_mut()
            .append_pair("loginTraceID", &start.flow_id)
            .append_pair("scope", "trae")
            .append_pair("authCodeInfo", info);
        let error = parse_callback_url(url.as_str()).unwrap_err();
        assert!(!error.message.contains("secret"));
    }
    assert!(parse_callback_url(&"x".repeat(MAX_CALLBACK_BYTES + 1)).is_err());
    let callback = parse_callback_url(&valid).unwrap();
    assert!(!format!("{callback:?}").contains("code-never-log-me"));
    let with_space = valid.replace(&start.flow_id, &format!("%20{}", start.flow_id));
    assert!(parse_callback_url(&with_space).is_err());
}

#[test]
fn trae_callback_cannot_consume_another_flow_or_use_another_callback_port() {
    let (a, flow_a) = start_login("http://127.0.0.1:9911/authorize", None, 1000).unwrap();
    let (b, flow_b) = start_login("http://127.0.0.1:9911/authorize", None, 1000).unwrap();
    let callback_a = parse_callback_url(&callback_from_start(&a, "code-a")).unwrap();
    let callback_b = parse_callback_url(&callback_from_start(&b, "code-b")).unwrap();
    let mut store = TraeLoginFlowStore::default();
    store.insert(a.flow_id.clone(), flow_a, 1000);
    store.insert(b.flow_id.clone(), flow_b, 1000);
    assert_eq!(
        store
            .begin_completion(&a.flow_id, &callback_b, 1001)
            .unwrap_err(),
        TraeLoginFlowError::StateMismatch
    );
    let wrong_port =
        parse_callback_url(&callback_from_start(&a, "code-a").replace(":9911/", ":9912/")).unwrap();
    assert_eq!(
        store
            .begin_completion(&a.flow_id, &wrong_port, 1001)
            .unwrap_err(),
        TraeLoginFlowError::CallbackMismatch
    );
    store
        .begin_completion(&a.flow_id, &callback_a, 1001)
        .unwrap();
    store.cancel(&b.flow_id);
    assert_eq!(
        store
            .begin_completion(&b.flow_id, &callback_b, 1001)
            .unwrap_err(),
        TraeLoginFlowError::NotFound
    );
}

#[tokio::test]
async fn trae_code_exchange_registers_the_matching_device_key_and_ignores_callback_host() {
    use p256::pkcs8::{DecodePrivateKey, DecodePublicKey};
    let (origin, observations, server) = serve_oauth_fixture("uid-1", "enterprise-1").await;
    let (start, flow) = start_test_login(
        "http://127.0.0.1:9911/authorize",
        ProviderType::TraeSolo,
        &origin,
        1000,
    );
    let callback = parse_callback_url(&format!(
        "{}&host=https%3A%2F%2Fattacker.invalid&userRegion=us",
        callback_from_start(&start, "test-code")
    ))
    .unwrap();
    let input = complete_login(
        &crate::infra::http::direct_client().unwrap(),
        &flow,
        &callback,
        1001,
    )
    .await
    .unwrap();
    let seen = observations.lock().unwrap();
    let body = &seen[0].body;
    assert_eq!(body["AuthCode"], "test-code");
    assert_eq!(body["CodeVerifier"], flow.code_verifier.as_str());
    assert_eq!(body["IDEVersion"], "3.4.3");
    assert_eq!(body["DeviceInfo"]["MachineID"], "test-machine");
    assert_eq!(body["DeviceInfo"]["DeviceID"], "test-device");
    assert_eq!(body["DeviceInfo"]["PlatformCode"], "IDE_PC");
    let registered_key = p256::PublicKey::from_public_key_pem(
        body["DeviceInfo"]["DevicePublicKey"].as_str().unwrap(),
    )
    .unwrap();
    let stored_key = p256::SecretKey::from_pkcs8_pem(
        input.raw.as_ref().unwrap()["devicePrivateKeyPem"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(registered_key, stored_key.public_key());
    assert_eq!(input.raw.as_ref().unwrap()["oauthClientId"], IDE_CLIENT_ID);
    drop(seen);
    server.abort();
}

#[tokio::test]
async fn trae_pkce_accounts_refresh_with_issuing_client_while_legacy_solo_keeps_legacy_client() {
    for modern in [false, true] {
        let (origin, observations, server) = serve_oauth_fixture("uid-1", "enterprise-1").await;
        let mut account = refresh_account(&origin);
        if modern {
            account.raw.as_mut().unwrap()["oauthClientId"] = json!(IDE_CLIENT_ID);
            account.raw.as_mut().unwrap()["devicePrivateKeyPem"] =
                json!(device_proof::generate_key_pem().unwrap().as_str());
        }
        refresh_trae_account(
            &crate::infra::http::direct_client().unwrap(),
            &account,
            1000,
            &mut |_| Ok(()),
        )
        .await
        .unwrap();
        assert_eq!(
            observations.lock().unwrap()[0].body["ClientID"],
            if modern {
                IDE_CLIENT_ID
            } else {
                TRAE_CLIENT_ID
            }
        );
        assert!(observations.lock().unwrap()[0]
            .body
            .get("DeviceProof")
            .is_none());
        server.abort();
    }
}

#[tokio::test]
async fn trae_relogin_reuses_registered_keys_and_checks_expected_enterprise() {
    for provider in [
        ProviderType::TraeSolo,
        ProviderType::TraeRaw,
        ProviderType::TraeIntl,
    ] {
        let mut account = refresh_account("http://127.0.0.1:9");
        account.provider_type = provider;
        account.id = trae_account_id_for(provider, "uid-1").unwrap();
        let key = device_proof::generate_key_pem().unwrap();
        account.raw.as_mut().unwrap()["devicePrivateKeyPem"] = json!(key.as_str());
        let (start, mut flow) = start_login_for_provider(
            "http://127.0.0.1:9911/authorize",
            provider,
            Some(&account),
            1000,
        )
        .unwrap();
        assert_eq!(flow.device_private_key_pem.as_str(), key.as_str());
        let (origin, _, server) = serve_oauth_fixture("uid-1", "different-enterprise").await;
        flow.endpoints.oauth_origin = Url::parse(&origin).unwrap();
        flow.endpoints.code_exchange_origin = Url::parse(&origin).unwrap();
        // Remove untrusted identity hints to test the target-account identity
        // check itself rather than the independent callback userInfo check.
        let mut callback = parse_callback_url(&callback_from_start(&start, "code")).unwrap();
        callback.callback_uid = None;
        callback.callback_enterprise_id = None;
        let error = complete_login(&login_http_client().unwrap(), &flow, &callback, 1001)
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(error.message.contains("enterprise"));
        server.abort();
    }
}

#[test]
fn trae_default_port_final_callback_is_canonicalized_and_expiry_discards_inflight_result() {
    let (start, flow) = start_login("http://127.0.0.1:80/authorize", None, 1000).unwrap();
    let callback = parse_callback_url(&callback_from_start(&start, "code")).unwrap();
    assert_eq!(callback.callback_base_url, "http://127.0.0.1:80/authorize");
    let mut store = TraeLoginFlowStore::default();
    store.insert(start.flow_id.clone(), flow, 1000);
    store
        .begin_completion(&start.flow_id, &callback, 1001)
        .unwrap();
    let input: UpsertAccountInput =
        serde_json::from_value(json!({"providerType":"trae_solo","accessToken":"secret"})).unwrap();
    assert_eq!(
        store
            .finish_completion(&start.flow_id, input, start.expires_at)
            .unwrap_err(),
        TraeLoginFlowError::NotFound
    );
    assert!(store.status(&start.flow_id, start.expires_at).is_none());
}
