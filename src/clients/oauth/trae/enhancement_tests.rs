#[tokio::test]
async fn trae_raw_login_and_refresh_use_the_bound_realm_client_and_receipt() {
    use p256::ecdsa::{signature::Verifier, Signature, SigningKey};
    use p256::pkcs8::EncodePrivateKey;
    for provider_type in [ProviderType::TraeRaw, ProviderType::TraeIntl] {
        let (origin, observations, server) = serve_oauth_fixture("uid-1", "enterprise-1").await;
        let mut endpoints = TraeOAuthEndpoints::for_test(&origin);
        endpoints.protocol =
            crate::domain::trae::TraeProtocol::for_provider(provider_type).unwrap();
        let (start, flow) = start_login_with_endpoints(
            "http://127.0.0.1:9911/authorize",
            "machine-1".into(),
            "device-1".into(),
            None,
            None,
            endpoints,
            1000,
        )
        .unwrap();
        let callback = parse_callback_url(&callback_from_start(&start, "refresh-old")).unwrap();
        let input = complete_login(
            &crate::infra::http::direct_client().unwrap(),
            &flow,
            &callback,
            1001,
        )
        .await
        .unwrap();
        assert_eq!(input.provider_type, provider_type);
        assert_eq!(
            input.id.as_deref(),
            Some(
                crate::domain::trae::trae_account_id_for(provider_type, "uid-1")
                    .unwrap()
                    .as_str()
            )
        );
        let auth = Url::parse(&start.auth_url).unwrap();
        assert_eq!(
            auth.host_str(),
            Some(if provider_type == ProviderType::TraeIntl {
                "www.trae.ai"
            } else {
                "www.trae.cn"
            })
        );
        assert!(auth
            .query_pairs()
            .any(|(key, value)| key == "client_id" && value == "ono9krqynydwx5"));
        let mut account = refresh_account(&origin);
        account.provider_type = provider_type;
        let secret = p256::SecretKey::from_slice(&[7u8; 32]).unwrap();
        let signing = SigningKey::from(secret.clone());
        account.raw.as_mut().unwrap()["devicePrivateKeyPem"] = json!(secret
            .to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
            .unwrap()
            .as_str());
        let mut receipt = None;
        let mut hook = |update: &AccountRefreshUpdate| {
            receipt = Some(update.clone());
            Ok(())
        };
        let update = refresh_trae_account(
            &crate::infra::http::direct_client().unwrap(),
            &account,
            2000,
            &mut hook,
        )
        .await
        .unwrap();
        assert_eq!(
            receipt.unwrap().refresh_token.as_deref(),
            Some("refresh-rotated")
        );
        assert_eq!(
            update.token_type.as_deref(),
            Some(if provider_type == ProviderType::TraeIntl {
                "Bearer"
            } else {
                "Cloud-IDE-JWT"
            })
        );
        let seen = observations.lock().unwrap();
        assert_eq!(seen[0].body["ClientID"], "ono9krqynydwx5");
        assert_eq!(seen[0].body["AuthCode"], "refresh-old");
        assert_eq!(seen[0].body["DeviceInfo"]["DeviceID"], "device-1");
        let body = &seen[2].body;
        assert_eq!(body["ClientID"], "ono9krqynydwx5");
        assert_eq!(body["UserID"], "uid-1");
        if provider_type == ProviderType::TraeIntl {
            assert_eq!(body["DeviceInfo"]["DeviceId"], "device-1");
            assert_eq!(body["DeviceProof"]["Timestamp"], 2000);
            let nonce = body["DeviceProof"]["Nonce"].as_str().unwrap();
            let encoded = body["DeviceProof"]["Signature"].as_str().unwrap();
            let signature = Signature::from_der(
                &base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .unwrap(),
            )
            .unwrap();
            let message = format!(
                "POST {} ono9krqynydwx5 refresh-old 2000 {}",
                TRAE_EXCHANGE_TOKEN_PATH, nonce
            );
            signing
                .verifying_key()
                .verify(message.as_bytes(), &signature)
                .unwrap();
        } else {
            assert!(body.get("DeviceProof").is_none());
        }
        drop(seen);
        server.abort();
    }
}

#[test]
fn trae_channel_identity_and_production_origins_cannot_cross_realms() {
    let cn = crate::domain::trae::TraeProtocol::for_provider(ProviderType::TraeRaw).unwrap();
    let intl = crate::domain::trae::TraeProtocol::for_provider(ProviderType::TraeIntl).unwrap();
    assert_eq!(cn.oauth_origin, "https://api.trae.com.cn");
    assert_eq!(intl.oauth_origin, "https://grow-normal.trae.ai");
    assert_eq!(intl.agent_origin, "https://core-normal.traeapi.us");
    assert!(intl.catalog_path.is_none());
    assert_ne!(
        crate::domain::trae::trae_account_id_for(ProviderType::TraeRaw, "uid").unwrap(),
        crate::domain::trae::trae_account_id_for(ProviderType::TraeIntl, "uid").unwrap()
    );
    let mut account = refresh_account("http://127.0.0.1:9");
    account.provider_type = ProviderType::TraeIntl;
    account.raw.as_mut().unwrap()["devicePrivateKeyPem"] = json!("malformed");
    let error =
        device_proof::device_proof(&account, "uid-1", IDE_CLIENT_ID, "refresh", 1000).unwrap_err();
    assert!(!error.message.contains("malformed"));
}
