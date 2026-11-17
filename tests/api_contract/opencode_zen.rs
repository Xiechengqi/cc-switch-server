// Exercise the actual Web Bundle writer, not the legacy single-Provider bridge.
#[tokio::test]
async fn opencode_zen_web_bundle_saves_reloads_and_shares_without_credentials() {
    use cc_switch_server::domain::providers::store::ProviderStore;
    use cc_switch_server::domain::sharing::credential_source::resolve_provider_credential_source;
    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    let mut identities = Vec::new();
    for (index, enabled) in [vec!["claude", "codex"], vec!["claude"], vec!["codex"]]
        .iter()
        .enumerate()
    {
        let id = format!("zen-bundle-{index}");
        let mut draft = json!({"id":id,"familyId":"family.opencode_zen","name":"OpenCode Zen",
            "modelPolicyScope":"global","modelPolicy":"passthrough","testApp":"codex",
            "clientRequestId":format!("zen-create-{index}"),
            "surfaces":[{"app":"claude","enabled":enabled.contains(&"claude"),"profileId":"claude.opencode_zen"},
            {"app":"codex","enabled":enabled.contains(&"codex"),"profileId":"codex.opencode_zen"}]});
        let (status, created) = custom_share_api(
            &app,
            &token,
            Method::POST,
            "/web-api/invoke/upsert_provider_bundle",
            json!({"bundle":draft}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        assert_eq!(created["credentialConfigured"], true);
        assert_eq!(created["credentialSlots"], json!([]));
        assert_eq!(created["revision"], 1);
        let (status, replay) = custom_share_api(
            &app,
            &token,
            Method::POST,
            "/web-api/invoke/upsert_provider_bundle",
            json!({"bundle":draft}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{replay}");
        assert_eq!(replay["revision"], 1);
        draft.as_object_mut().unwrap().remove("clientRequestId");
        draft["expectedRevision"] = json!(1);
        draft["modelPolicy"] = json!("single");
        draft["upstreamModel"] = json!("chat-free");
        let (status, updated) = custom_share_api(
            &app,
            &token,
            Method::POST,
            "/web-api/invoke/upsert_provider_bundle",
            json!({"bundle":draft}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["revision"], 2);
        let (status, conflict) = custom_share_api(
            &app,
            &token,
            Method::POST,
            "/web-api/invoke/upsert_provider_bundle",
            json!({"bundle":draft}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
        for surface in ["claude", "codex"] {
            assert_eq!(
                updated["surfaces"][surface]["runtime"]["modelPolicy"],
                json!({"mode":"single","upstreamModel":"chat-free"})
            );
        }
        let (status, read) = custom_share_api(
            &app,
            &token,
            Method::POST,
            "/web-api/invoke/get_provider_bundle",
            json!({"id":id}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{read}");
        assert_eq!(read["revision"], 2);
        let restored = ProviderStore::load_runtime_or_default(&state.config_dir).unwrap();
        let accounts = state.accounts_snapshot().await;
        let claude = resolve_provider_credential_source(&restored, &accounts, AppKind::Claude, &id)
            .unwrap()
            .unwrap();
        let codex = resolve_provider_credential_source(&restored, &accounts, AppKind::Codex, &id)
            .unwrap()
            .unwrap();
        assert_eq!(claude, codex);
        assert!(!identities.contains(&claude));
        identities.push(claude);
        let persisted: Value = serde_json::from_slice(
            &std::fs::read(state.config_dir.join("providers.json")).unwrap(),
        )
        .unwrap();
        for app in ["claude", "codex"] {
            assert!(persisted["records"][app][&id].is_object());
            assert!(persisted["records"][app][&id]["credentials"].is_null());
            assert!(persisted["records"][app][&id]["credentialSource"].is_null());
        }
        let (status, preflight) = custom_share_api(
            &app,
            &token,
            Method::POST,
            "/web-api/invoke/get_provider_bundle_share_preflight",
            json!({"bundleId":id}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{preflight}");
        assert_eq!(preflight["canShare"], true, "{preflight}");
        let (status, share) = custom_share_api(
            &app,
            &token,
            Method::POST,
            "/web-api/invoke/save_provider_bundle_share",
            custom_bundle_share_input(&id),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{share}");
    }
    let providers = providers_snapshot(&state).await;
    let accounts = state.accounts_snapshot().await;
    let bindings = [
        cc_switch_server::domain::sharing::shares::ShareBinding {
            app: AppKind::Claude,
            provider_id: "zen-bundle-0".into(),
            provider_type: ProviderType::OpenCodeZen,
        },
        cc_switch_server::domain::sharing::shares::ShareBinding {
            app: AppKind::Codex,
            provider_id: "zen-bundle-2".into(),
            provider_type: ProviderType::OpenCodeZen,
        },
    ];
    assert!(cc_switch_server::domain::sharing::credential_source::shared_credential_source_for_bindings(&providers, &accounts, &bindings).is_err());
    cc_switch_server::state::stop_client_tunnel(&state).await;
}
