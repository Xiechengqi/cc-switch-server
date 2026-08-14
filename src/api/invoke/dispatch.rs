use super::super::*;
use std::collections::BTreeMap;

use crate::domain::sharing::router_contract::ShareSettingsPatch;

pub(in crate::api) async fn web_invoke_compat(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(command): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let args = if body.is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_slice(&body).map_err(ApiError::bad_request)?
    };
    let command_def = web_runtime::command(&command)
        .ok_or_else(|| ApiError::web_invoke_unknown(command.clone()))?;
    if command_def.support == WebRuntimeCommandSupport::Excluded {
        return Err(ApiError::feature_disabled(format!(
            "legacy invoke command '{command}' is excluded from cc-switch-server ({})",
            command_def.feature
        )));
    }

    if web_invoke_requires_session(&state, &command).await {
        require_session(&state, &headers).await?;
    }
    if !command_def.implemented {
        return Err(ApiError::web_invoke_not_wired(format!(
            "legacy invoke command '{command}' is registered as {} but is not bridged yet",
            web_runtime_support_label(command_def.support)
        )));
    }

    web_invoke_dispatch(&state, &headers, &command, args)
        .await
        .map(Json)
}

async fn web_invoke_requires_session(state: &ServerState, command: &str) -> bool {
    match command {
        "complete_server_setup" => state.config.read().await.is_setup_complete(),
        "request_admin_email_login_code"
        | "verify_admin_email_login_code"
        | "login_with_api_token" => false,
        _ => true,
    }
}

async fn web_invoke_dispatch(
    state: &ServerState,
    headers: &HeaderMap,
    command: &str,
    args: Value,
) -> Result<Value, ApiError> {
    match command {
        "get_build_info" => {
            let mut response = json!(build_info());
            response["processId"] = json!(std::process::id());
            response["processInstanceId"] = json!(state.process_instance_id.clone());
            Ok(response)
        }
        "get_admin_version_info" => {
            let check_remote = args
                .get("checkRemote")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(json!(if check_remote {
                crate::api::self_update::build_admin_version_response(state).await
            } else {
                crate::api::self_update::build_admin_runtime_version_response(state).await
            }))
        }
        "restart_server_service" => {
            let response =
                crate::api::self_update::admin_restart(State(state.clone()), headers.clone())
                    .await?;
            Ok(json!(response.0))
        }
        "rollback_server_service" => {
            let response =
                crate::api::self_update::admin_rollback(State(state.clone()), headers.clone())
                    .await?;
            Ok(json!(response.0))
        }
        "start_admin_upgrade" => {
            let restart_after = args
                .get("restartAfter")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
            let response = crate::api::self_update::admin_upgrade_start(
                State(state.clone()),
                headers.clone(),
                Json(crate::api::self_update::start_upgrade_request(
                    restart_after,
                    force,
                )),
            )
            .await?;
            Ok(json!(response.0))
        }
        "get_upgrade_policy" => Ok(crate::api::settings::upgrade_policy_snapshot(state).await),
        "set_upgrade_policy" => {
            let policy = args.get("policy").cloned().unwrap_or_else(|| args.clone());
            Ok(json!(
                crate::api::settings::save_upgrade_policy(state, headers.clone(), policy).await?
            ))
        }
        "complete_server_setup" => {
            let password = web_arg_string_any(&args, &["password"])?;
            let owner_email = web_arg_string_any(&args, &["ownerEmail", "owner_email"])?;
            let router_url = web_arg_string_any(&args, &["routerUrl", "router_url"])?;
            let client_tunnel_subdomain = web_optional_string_any(
                &args,
                &["clientTunnelSubdomain", "client_tunnel_subdomain"],
            );
            let options = args.get("options").and_then(|value| {
                serde_json::from_value::<crate::domain::settings::config::SetupOptions>(
                    value.clone(),
                )
                .ok()
            });
            let response = crate::api::settings::setup(
                State(state.clone()),
                Json(crate::domain::settings::config::SetupInput {
                    password,
                    owner_email,
                    router_url,
                    client_tunnel_subdomain,
                    options,
                }),
            )
            .await?;
            Ok(json!(response.0))
        }
        "login_with_api_token" => {
            let api_token = web_arg_string_any(&args, &["apiToken", "api_token"])?;
            let response = crate::api::settings::login(
                State(state.clone()),
                Json(LoginRequest {
                    method: "api_token".to_string(),
                    password: String::new(),
                    api_token: Some(api_token),
                    email: None,
                    code: None,
                }),
            )
            .await?;
            Ok(json!(response.0))
        }
        "request_admin_email_login_code" => {
            let email = web_arg_string_any(&args, &["email"])?;
            let response = crate::api::settings::request_email_login_code(
                State(state.clone()),
                Json(EmailLoginCodeRequest { email }),
            )
            .await?;
            Ok(json!(response.0))
        }
        "verify_admin_email_login_code" => {
            let email = web_arg_string_any(&args, &["email"])?;
            let code = web_arg_string_any(&args, &["code"])?;
            let response = crate::api::settings::verify_email_login_code(
                State(state.clone()),
                Json(EmailLoginVerifyCodeRequest { email, code }),
            )
            .await?;
            Ok(json!(response.0))
        }
        "get_settings" => {
            let config = state.config.read().await.clone();
            let store = state.ui_settings.read().await;
            Ok(store.settings_for_frontend(&config))
        }
        "get_rectifier_config" => {
            let store = state.ui_settings.read().await;
            Ok(ui_settings::rectifier_config_for_frontend(&store))
        }
        "get_optimizer_config" => {
            let store = state.ui_settings.read().await;
            Ok(ui_settings::optimizer_config_for_frontend(&store))
        }
        "set_rectifier_config" => {
            let config: Value = web_arg_value(&args, "config")?;
            state
                .apply_ui_settings_patch_immediate(json!({ "rectifierConfig": config }))
                .await
                .map_err(ApiError::internal)?;
            Ok(json!(true))
        }
        "set_optimizer_config" => {
            let config: Value = web_arg_value(&args, "config")?;
            state
                .apply_ui_settings_patch_immediate(json!({ "optimizerConfig": config }))
                .await
                .map_err(ApiError::internal)?;
            Ok(json!(true))
        }
        "get_log_config" => {
            let store = state.ui_settings.read().await;
            Ok(ui_settings::log_config_for_frontend(&store))
        }
        "set_log_config" => {
            let config: Value = web_arg_value(&args, "config")?;
            ui_settings::validate_log_config(&config).map_err(ApiError::bad_request)?;
            state
                .apply_ui_settings_patch_immediate(json!({ "logConfig": config }))
                .await
                .map_err(ApiError::internal)?;
            state.sync_log_config_from_ui_settings().await;
            Ok(json!(true))
        }
        "get_api_management" => Ok(crate::api::debug::api_management_snapshot(state).await),
        "set_api_management" => {
            let config: Value = web_arg_value(&args, "config")?;
            crate::api::debug::save_api_management(state, config).await
        }
        "generate_debug_token" => {
            let ttl_hours = args.get("ttlHours").and_then(Value::as_u64);
            crate::api::debug::generate_debug_token(state, ttl_hours).await
        }
        "revoke_debug_token" => crate::api::debug::revoke_debug_token(state).await,
        "get_stream_check_config" => {
            let store = state.ui_settings.read().await;
            Ok(ui_settings::stream_check_config_for_frontend(&store))
        }
        "save_stream_check_config" => {
            let config: Value = web_arg_value(&args, "config")?;
            state
                .apply_ui_settings_patch_immediate(json!({ "streamCheckConfig": config }))
                .await
                .map_err(ApiError::internal)?;
            Ok(json!(true))
        }
        "save_settings" => {
            let patch =
                ui_settings::settings_patch_from_args(&args).map_err(ApiError::bad_request)?;
            let updates_log_config = patch.get("logConfig").is_some();
            state
                .apply_ui_settings_patch_immediate(patch)
                .await
                .map_err(ApiError::internal)?;
            if updates_log_config {
                state.sync_log_config_from_ui_settings().await;
            }
            Ok(json!(true))
        }
        "is_portable_mode" => Ok(json!(false)),
        "get_app_config_dir_override" => Ok(json!(null)),
        "get_app_config_path" => Ok(json!(state.config_dir.display().to_string())),
        "get_config_dir" => {
            let _app = web_arg_app(&args).or_else(|_| web_arg_app_type(&args))?;
            Ok(json!(""))
        }
        "get_provider_bundles" => Ok(json!(state
            .provider_bundle_views()
            .await
            .map_err(ApiError::internal)?)),
        "get_provider_bundle" => {
            let id = web_arg_string_any(&args, &["id", "bundleId"])?;
            let bundle = state
                .provider_bundle_view(&id)
                .await
                .map_err(ApiError::internal)?
                .ok_or_else(|| ApiError::not_found("Provider Bundle not found"))?;
            Ok(json!(bundle))
        }
        "get_provider_bundle_delete_preview" => {
            let id = web_arg_string_any(&args, &["id", "bundleId"])?;
            let preview = state
                .provider_bundle_reference_preview(&id)
                .await
                .map_err(map_provider_command_error)?;
            Ok(json!(preview))
        }
        "update_provider_bundles_sort_order" => {
            require_provider_write_contract(state, headers)?;
            let updates: Vec<ProviderSortUpdate> = web_arg_value(&args, "updates")?;
            let changed = state
                .update_provider_bundle_order_command(updates)
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?;
            Ok(json!(changed))
        }
        "upsert_provider_bundle" => {
            require_provider_write_contract(state, headers)?;
            let draft = web_arg_value(&args, "bundle")?;
            let bundle = state
                .upsert_provider_bundle_command(draft)
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?;
            Ok(json!(bundle))
        }
        "delete_provider_bundle" => {
            require_provider_write_contract(state, headers)?;
            let id = web_arg_string_any(&args, &["id", "bundleId"])?;
            let expected_revision = web_arg_value(&args, "expectedRevision")?;
            let deleted = state
                .delete_provider_bundle_command(id, expected_revision)
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?;
            Ok(json!(deleted))
        }
        "get_providers" => {
            let app = match web_arg_app_for_read(&args)? {
                Some(app) => app,
                None => return Ok(json!({})),
            };
            Ok(json!(state.redacted_provider_record(app).await))
        }
        "get_provider_registry" => Ok(json!(
            crate::domain::providers::registry::provider_registry()
        )),
        "get_provider_credential" => {
            require_provider_write_contract(state, headers)?;
            let app = web_arg_app(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id"])?;
            let slot = web_arg_string_any(&args, &["slot"])?;
            let key = crate::domain::providers::registry::ProviderKey::new(app, provider_id)
                .map_err(ApiError::bad_request)?;
            let credential = state
                .reveal_provider_credential_command(&key, &slot)
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?;
            Ok(json!(credential))
        }
        "get_provider_store_migration" => {
            let config_dir = state.config_dir.clone();
            let report = tokio::task::spawn_blocking(move || {
                crate::domain::providers::storage_migration::preflight(&config_dir)
            })
            .await
            .map_err(|error| {
                ApiError::internal(format!("Provider migration preflight panicked: {error}"))
            })?
            .map_err(ApiError::internal)?;
            Ok(json!(report))
        }
        "get_provider_resources" => {
            let app = match web_arg_app_for_read(&args)? {
                Some(app) => app,
                None => return Ok(json!([])),
            };
            Ok(json!(state.provider_views(Some(app)).await))
        }
        "get_coding_plan_quota" | "refresh_coding_plan_quota" => {
            let app = web_arg_app(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id", "id"])?;
            let provider_key =
                crate::domain::providers::registry::ProviderKey::new(app, provider_id)
                    .map_err(ApiError::bad_request)?;
            let snapshot = state
                .coding_plan_quota_snapshot(
                    provider_key,
                    command == "refresh_coding_plan_quota",
                )
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?;
            Ok(json!(snapshot))
        }
        "add_provider" | "update_provider" => {
            require_provider_write_contract(state, headers)?;
            let app = web_arg_app(&args)?;
            let provider: Provider = web_arg_value(&args, "provider")?;
            let profile_id = web_optional_deserialize(&args, "profileId")?;
            let custom_binding = web_optional_deserialize(&args, "customBinding")?;
            let expected_revision = web_optional_deserialize(&args, "expectedRevision")?;
            if command == "update_provider" && expected_revision.is_none() {
                return Err(ApiError::bad_request(
                    "expectedRevision is required to update a Provider",
                ));
            }
            let client_request_id: Option<String> =
                web_optional_deserialize(&args, "clientRequestId")?;
            let credential_patches =
                web_optional_deserialize(&args, "credentialPatches")?.unwrap_or_default();
            let stored = state
                .upsert_provider_draft_command(
                    crate::domain::providers::credentials::ProviderWriteDraft {
                        app,
                        provider,
                        profile_id,
                        custom_binding,
                        expected_revision,
                        client_request_id,
                        credential_patches,
                    },
                )
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?;
            Ok(json!(
                crate::domain::providers::credentials::ProviderView::from_stored(&stored)
            ))
        }
        "adopt_provider_profile" => {
            require_provider_write_contract(state, headers)?;
            let app = web_arg_app(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "id"])?;
            let expected_revision = web_arg_value(&args, "expectedRevision")?;
            let profile_id = web_arg_value(&args, "profileId")?;
            let account_id = web_optional_deserialize(&args, "accountId")?;
            let mode: ProviderActionMode = web_arg_value(&args, "mode")?;
            let (preview, stored) = match mode {
                ProviderActionMode::Preview => (
                    state
                        .preview_adopt_provider_profile_command(
                            app,
                            &provider_id,
                            expected_revision,
                            profile_id,
                            account_id,
                        )
                        .await
                        .map_err(ApiError::internal)?
                        .map_err(map_provider_command_error)?,
                    None,
                ),
                ProviderActionMode::Apply => {
                    let preview_token: String = web_arg_value(&args, "previewToken")?;
                    let (preview, stored) = state
                        .apply_adopt_provider_profile_command(
                            app,
                            provider_id,
                            expected_revision,
                            profile_id,
                            account_id,
                            preview_token,
                        )
                        .await
                        .map_err(ApiError::internal)?
                        .map_err(map_provider_command_error)?;
                    (
                        preview,
                        Some(
                            crate::domain::providers::credentials::ProviderView::from_stored(
                                &stored,
                            ),
                        ),
                    )
                }
            };
            Ok(json!({"ok": true, "mode": mode, "preview": preview, "stored": stored}))
        }
        "rebind_custom_provider" => {
            require_provider_write_contract(state, headers)?;
            let app = web_arg_app(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "id"])?;
            let expected_revision = web_arg_value(&args, "expectedRevision")?;
            let custom_binding = web_arg_value(&args, "customBinding")?;
            let credential_patches =
                web_optional_deserialize(&args, "credentialPatches")?.unwrap_or_default();
            let mode: ProviderActionMode = web_arg_value(&args, "mode")?;
            let (preview, stored) = match mode {
                ProviderActionMode::Preview => (
                    state
                        .preview_rebind_custom_provider_command(
                            app,
                            &provider_id,
                            expected_revision,
                            custom_binding,
                            credential_patches,
                        )
                        .await
                        .map_err(ApiError::internal)?
                        .map_err(map_provider_command_error)?,
                    None,
                ),
                ProviderActionMode::Apply => {
                    let preview_token: String = web_arg_value(&args, "previewToken")?;
                    let (preview, stored) = state
                        .apply_rebind_custom_provider_command(
                            app,
                            provider_id,
                            expected_revision,
                            custom_binding,
                            credential_patches,
                            preview_token,
                        )
                        .await
                        .map_err(ApiError::internal)?
                        .map_err(map_provider_command_error)?;
                    (
                        preview,
                        Some(
                            crate::domain::providers::credentials::ProviderView::from_stored(
                                &stored,
                            ),
                        ),
                    )
                }
            };
            Ok(json!({"ok": true, "mode": mode, "preview": preview, "stored": stored}))
        }
        "clone_provider_as_custom" => {
            require_provider_write_contract(state, headers)?;
            let app = web_arg_app(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "id"])?;
            let expected_revision = web_arg_value(&args, "expectedRevision")?;
            let target_provider_id = web_arg_string(&args, "targetProviderId")?;
            let target_name = web_arg_string(&args, "targetName")?;
            let custom_binding = web_arg_value(&args, "customBinding")?;
            let client_request_id = web_arg_string(&args, "clientRequestId")?;
            let mode: ProviderActionMode = web_arg_value(&args, "mode")?;
            let (preview, stored) = match mode {
                ProviderActionMode::Preview => (
                    state
                        .preview_clone_provider_as_custom_command(
                            app,
                            &provider_id,
                            expected_revision,
                            target_provider_id,
                            target_name,
                            custom_binding,
                            client_request_id,
                        )
                        .await
                        .map_err(ApiError::internal)?
                        .map_err(map_provider_command_error)?,
                    None,
                ),
                ProviderActionMode::Apply => {
                    let preview_token: String = web_arg_value(&args, "previewToken")?;
                    let (preview, stored) = state
                        .apply_clone_provider_as_custom_command(
                            app,
                            provider_id,
                            expected_revision,
                            target_provider_id,
                            target_name,
                            custom_binding,
                            client_request_id,
                            preview_token,
                        )
                        .await
                        .map_err(ApiError::internal)?
                        .map_err(map_provider_command_error)?;
                    (
                        preview,
                        Some(
                            crate::domain::providers::credentials::ProviderView::from_stored(
                                &stored,
                            ),
                        ),
                    )
                }
            };
            Ok(json!({"ok": true, "mode": mode, "preview": preview, "stored": stored}))
        }
        "preview_provider_account_binding_migration" => {
            let preview = state
                .preview_provider_account_binding_migration_command()
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?;
            Ok(json!({"ok": true, "preview": preview, "applied": 0}))
        }
        "apply_provider_account_binding_migration" => {
            require_provider_write_contract(state, headers)?;
            let preview_token = web_arg_string(&args, "previewToken")?;
            let (preview, applied) = state
                .apply_provider_account_binding_migration_command(preview_token)
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?;
            Ok(json!({"ok": true, "preview": preview, "applied": applied}))
        }
        "update_providers_sort_order" => {
            require_provider_write_contract(state, headers)?;
            let app = web_arg_app(&args)?;
            let updates: Vec<ProviderSortUpdate> = web_arg_value(&args, "updates")?;
            let changed = state
                .mutate_providers_immediate_if_changed(move |providers| {
                    match providers.update_sort_order(app, updates) {
                        Ok(changed) => (Ok(changed), changed),
                        Err(error) => (Err(error.to_string()), false),
                    }
                })
                .await
                .map_err(ApiError::internal)?;
            changed.map_err(ApiError::bad_request)?;
            Ok(json!(true))
        }
        "delete_provider" => {
            require_provider_write_contract(state, headers)?;
            let app = web_arg_app(&args)?;
            let id = web_arg_string(&args, "id")?;
            let expected_revision: u64 = web_arg_value(&args, "expectedRevision")?;
            let deleted = state
                .delete_provider_command(app, id, expected_revision)
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?;
            Ok(json!(deleted))
        }
        "get_provider_health" => {
            let app = web_arg_app_type(&args)?;
            if let Some(provider_id) = args
                .get("providerId")
                .or_else(|| args.get("provider_id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Ok(web_provider_health_json(state, app, provider_id).await?)
            } else {
                Ok(web_provider_health_list_json(state, app).await)
            }
        }
        "list_shares" | "export_all_shares" => {
            let config = state.config_snapshot().await;
            let shares = state.shares.read().await.shares.clone();
            Ok(Value::Array(
                shares
                    .iter()
                    .map(|share| web_share_json(&config, share))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
        "list_share_reuse_candidates" => {
            let value = web_payload(&args, &["params", "input"]);
            let query = serde_json::from_value::<ShareReuseCandidatesQuery>(value.clone())
                .map_err(ApiError::bad_request)?;
            let response =
                share_reuse_candidates(State(state.clone()), headers.clone(), Query(query))
                    .await?
                    .0;
            Ok(json!(response))
        }
        "get_share_detail" => {
            let id = web_arg_share_id(&args)?;
            let share = state.shares.read().await.get(&id).cloned();
            let config = state.config_snapshot().await;
            share
                .as_ref()
                .map(|share| web_share_json(&config, share))
                .transpose()
                .map(|share| json!(share))
        }
        "get_share_connect_info" => {
            let id = web_arg_share_id(&args)?;
            let config = state.config.read().await.clone();
            let share = state
                .shares
                .read()
                .await
                .get(&id)
                .cloned()
                .ok_or_else(|| ApiError::not_found("share not found"))?;
            Ok(json!(connect_info_for_share(&config, &share)?))
        }
        "list_token_markets" => {
            let markets = fetch_public_token_markets_from_router(state).await?;
            Ok(json!(markets))
        }
        "create_share" => {
            let input = web_share_upsert_input(state, &args).await?;
            let value = web_payload(&args, &["params", "input", "share"]);
            let expected_config_revision =
                web_optional_deserialize(value, "expectedConfigRevision")?;
            let response = upsert_share(
                State(state.clone()),
                headers.clone(),
                Json(UpsertShareCommand {
                    expected_config_revision,
                    input,
                }),
            )
            .await?
            .0;
            Ok(web_share_json(
                &state.config_snapshot().await,
                &response.share,
            )?)
        }
        "add_share_binding" => {
            let value = web_payload(&args, &["params", "input"]);
            let share_id = web_arg_string_any(value, &["shareId", "share_id", "id"])?;
            let input = serde_json::from_value::<AddShareBindingRequest>(value.clone())
                .map_err(ApiError::bad_request)?;
            let response = add_share_binding(
                State(state.clone()),
                headers.clone(),
                Path(share_id),
                Json(input),
            )
            .await?
            .0;
            Ok(web_share_json(
                &state.config_snapshot().await,
                &response.share,
            )?)
        }
        "remove_share_binding" => {
            let value = web_payload(&args, &["params", "input"]);
            let share_id = web_arg_string_any(value, &["shareId", "share_id", "id"])?;
            let input = serde_json::from_value::<RemoveShareBindingRequest>(value.clone())
                .map_err(ApiError::bad_request)?;
            let response = remove_share_binding(
                State(state.clone()),
                headers.clone(),
                Path(share_id),
                Json(input),
            )
            .await?
            .0;
            Ok(json!(response))
        }
        "delete_share" => {
            let id = web_arg_share_id(&args)?;
            let response = delete_share(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(json!(response.deleted))
        }
        "pause_share" => {
            let id = web_arg_share_id(&args)?;
            let response = pause_share(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(web_share_json(
                &state.config_snapshot().await,
                &response.share,
            )?)
        }
        "resume_share" => {
            let id = web_arg_share_id(&args)?;
            let response = resume_share(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(web_share_json(
                &state.config_snapshot().await,
                &response.share,
            )?)
        }
        "reset_share_usage" => {
            let id = web_arg_share_id(&args)?;
            let response = reset_share_usage(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(web_share_json(
                &state.config_snapshot().await,
                &response.share,
            )?)
        }
        "email_auth_request_code" => {
            let response = web_email_auth_request_code(state, &args).await?;
            Ok(json!(response))
        }
        "email_auth_verify_code" => {
            let response = web_email_auth_verify_code(state, &args).await?;
            Ok(json!(response))
        }
        "email_auth_request_owner_change_code" => {
            let response = web_email_auth_request_owner_change_code(state, &args).await?;
            Ok(json!(response))
        }
        "email_auth_change_owner_email" => {
            let response = web_email_auth_change_owner_email(state, &args).await?;
            Ok(json!(response))
        }
        "email_auth_get_status" => {
            let response = web_email_auth_get_status(state)?;
            Ok(json!(response))
        }
        "email_auth_session_me" => {
            let response = web_email_auth_session_me(state).await?;
            Ok(json!(response))
        }
        "email_auth_logout" => web_email_auth_logout(state).await,
        "update_share_acl" => {
            let share = web_update_share_acl(state, &args).await?;
            Ok(json!(share))
        }
        "save_provider_share" => {
            let share = Box::pin(web_save_provider_share(state, &args)).await?;
            Ok(json!(share))
        }
        "save_provider_bundle_share" => {
            let share = Box::pin(web_save_provider_bundle_share(state, &args)).await?;
            match share.as_ref() {
                Some(share) => web_share_json(&state.config_snapshot().await, share),
                None => Ok(Value::Null),
            }
        }
        "update_share_owner_email" => {
            let share = web_update_share_owner_email(state, headers, &args).await?;
            Ok(json!(share))
        }
        "transfer_share_owner" => {
            let share = web_transfer_share_owner(state, headers, &args).await?;
            Ok(json!(share))
        }
        "start_share_tunnel" => {
            let id = web_arg_share_id(&args)?;
            let response = start_share_tunnel(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(json!(response.share))
        }
        "stop_share_tunnel" => {
            let id = web_arg_share_id(&args)?;
            let response = stop_share_tunnel(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(json!(response.share))
        }
        "get_tunnel_status" => {
            if let Ok(id) = web_arg_share_id(&args) {
                return Ok(json!(web_share_tunnel_status(state, &id).await?));
            }
            let response = router_tunnels(State(state.clone()), headers.clone())
                .await?
                .0;
            Ok(json!(response.tunnels))
        }
        "get_client_tunnel" => Ok(web_client_tunnel_state(state).await),
        "get_client_tunnel_status" => {
            let runtime = state
                .tunnels
                .status(&crate::clients::router::tunnel::client_tunnel_key())
                .await;
            Ok(web_client_tunnel_share_status(runtime))
        }
        "get_share_health_status" => Ok(web_share_health_status(state).await),
        "check_client_tunnel_subdomain" => {
            let subdomain = web_arg_string_any(&args, &["subdomain", "tunnelSubdomain"])?;
            let config = state.config.read().await;
            let subdomain = ServerConfig::preview_client_subdomain(&subdomain)
                .map_err(ApiError::bad_request)?;
            let router_url = config
                .router_api_base()
                .ok_or_else(|| ApiError::bad_request("router url is not configured"))?;
            let installation_id = config
                .router
                .identity
                .as_ref()
                .map(|identity| identity.installation_id.as_str());
            let availability = crate::client_tunnel_provision::check_subdomain_for_router(
                state,
                router_url,
                &subdomain,
                installation_id,
            )
            .await?;
            Ok(json!({
                "ok": true,
                "available": availability.available,
                "reason": availability.reason,
            }))
        }
        "suggest_client_tunnel_subdomain" => {
            let config = state.config.read().await;
            let router_url = config
                .router_api_base()
                .ok_or_else(|| ApiError::bad_request("router url is not configured"))?;
            let installation_id = config
                .router
                .identity
                .as_ref()
                .map(|identity| identity.installation_id.as_str());
            let outcome = crate::client_tunnel_provision::suggest_client_tunnel_subdomain(
                state,
                router_url,
                installation_id,
            )
            .await?;
            Ok(json!(outcome))
        }
        "suggest_share_slug" => {
            let shares = state.shares.read().await;
            let mut selected = None;
            for attempt in 0..crate::domain::subdomain_suggest::SUGGEST_MAX_ATTEMPTS {
                let candidate = crate::domain::subdomain_suggest::generate_candidate(
                    &mut rand::thread_rng(),
                    attempt,
                );
                if !shares.shares.iter().any(|share| {
                    share.status != "deleted"
                        && share.tunnel_subdomain.as_deref() == Some(candidate.as_str())
                }) {
                    selected = Some((candidate, attempt + 1));
                    break;
                }
            }
            let (subdomain, attempts) = selected
                .ok_or_else(|| ApiError::conflict("unable to generate an available share slug"))?;
            Ok(json!({
                "subdomain": subdomain,
                "available": true,
                "checked": true,
                "attempts": attempts,
            }))
        }
        "check_router_reachable" => {
            let config = state.config.read().await;
            let router_url = config
                .router_api_base()
                .ok_or_else(|| ApiError::bad_request("router url is not configured"))?;
            let outcome =
                crate::client_tunnel_provision::check_router_reachable(state, router_url).await?;
            Ok(json!(outcome))
        }
        "claim_client_tunnel" => {
            let mut config = state.config.read().await.clone();
            if web_has_payload(&args) {
                let value = web_payload(&args, &["params", "input", "config"]);
                let owner_email = web_optional_string_any(value, &["ownerEmail", "owner_email"]);
                let subdomain = web_optional_string_any(value, &["tunnelSubdomain", "subdomain"]);
                if let Some(email) = owner_email {
                    let email = crate::domain::settings::config::normalize_email(&email)
                        .map_err(ApiError::bad_request)?;
                    if !config
                        .owner
                        .email
                        .as_deref()
                        .is_some_and(|owner| owner.eq_ignore_ascii_case(&email))
                    {
                        return Err(ApiError::conflict(
                            "client owner must be changed through verified email ownership",
                        ));
                    }
                }
                if let Some(subdomain) = subdomain {
                    config
                        .update_client_tunnel(UpdateClientTunnelInput {
                            tunnel_subdomain: Some(subdomain),
                            tunnel_status: None,
                        })
                        .map_err(ApiError::bad_request)?;
                }
            }
            crate::client_tunnel_provision::claim_client_tunnel_config(state).await?;
            if web_optional_bool(&args, &["autoStart", "auto_start"]).unwrap_or(true) {
                crate::state::ensure_client_tunnel_running(state.clone(), "client_tunnel_claim")
                    .await;
            }
            Ok(web_client_tunnel_state(state).await)
        }
        "update_client_tunnel" => {
            let input = web_client_tunnel_input(&args)?;
            let _ =
                update_client_tunnel(State(state.clone()), headers.clone(), Json(input)).await?;
            Ok(web_client_tunnel_state(state).await)
        }
        "start_client_tunnel" => {
            let response = issue_client_tunnel_lease(State(state.clone()), headers.clone())
                .await?
                .0;
            Ok(json!(response))
        }
        "stop_client_tunnel" => {
            let response = stop_client_tunnel(State(state.clone()), headers.clone())
                .await?
                .0;
            Ok(json!(response))
        }
        "get_installed_skills" => Ok(json!([])),
        "get_proxy_status" => Ok(json!(web_proxy_status_json(state).await)),
        "get_proxy_takeover_status" => Ok(json!(web_proxy_takeover_status_json(state).await)),
        "is_proxy_running" => Ok(json!(true)),
        "is_live_takeover_active" => Ok(json!(web_is_live_takeover_active(state).await)),
        "list_db_backups" => {
            let response = list_backups(State(state.clone()), headers.clone()).await?.0;
            Ok(json!(crate::infra::backup::backup_entries_for_frontend(
                &response.backups
            )))
        }
        "create_db_backup" => {
            let body = web_create_backup_request(&args)?;
            let response = create_backup(State(state.clone()), headers.clone(), body)
                .await?
                .0;
            Ok(json!(response.backup))
        }
        "restore_db_backup" => {
            let id = web_arg_string_any(&args, &["id", "backupId", "filename"])?;
            let response = restore_backup(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(json!(response.result))
        }
        "get_account_capabilities" => Ok(json!({
            "ok": true,
            "capabilities": crate::domain::accounts::managers::all_capabilities(),
        })),
        "deepseek_account_status" => {
            let accounts = state.accounts.read().await;
            Ok(deepseek_account_status_json(&accounts))
        }
        "auth_get_status" => {
            let provider_type = web_auth_provider_type(&args)?;
            let provider_label = managed_auth_provider_label(provider_type);
            let accounts = state.accounts.read().await;
            let matching = accounts
                .accounts
                .iter()
                .filter(|account| account.provider_type == provider_type)
                .collect::<Vec<_>>();
            let default_account_id =
                managed_auth_default_account_id(&accounts, provider_type).map(str::to_string);
            let codex_oauth = (provider_type == ProviderType::CodexOAuth)
                .then(|| accounts.codex_oauth_selection());
            let authenticated = matching
                .iter()
                .any(|account| account_is_authenticated(account));
            let mapped_accounts = matching
                .iter()
                .map(|account| {
                    map_managed_auth_account(account, provider_label, default_account_id.as_deref())
                })
                .collect::<Vec<_>>();
            Ok(json!({
                "provider": provider_label,
                "authenticated": authenticated,
                "default_account_id": default_account_id,
                "migration_error": Value::Null,
                "codex_oauth": codex_oauth,
                "accounts": mapped_accounts
            }))
        }
        "auth_list_accounts" => {
            let provider_type = web_optional_auth_provider_type(&args)?;
            let accounts = state.accounts.read().await;
            let mapped = accounts
                .accounts
                .iter()
                .filter(|account| {
                    provider_type
                        .map(|provider_type| account.provider_type == provider_type)
                        .unwrap_or(true)
                })
                .map(|account| {
                    let default_account_id =
                        managed_auth_default_account_id(&accounts, account.provider_type);
                    map_managed_auth_account(
                        account,
                        managed_auth_provider_label(account.provider_type),
                        default_account_id,
                    )
                })
                .collect::<Vec<_>>();
            Ok(json!(mapped))
        }
        "auth_start_login" => {
            web_managed_auth_start_login(state.clone(), headers.clone(), &args).await
        }
        "auth_poll_for_account" => {
            web_managed_auth_poll_for_account(state.clone(), headers.clone(), &args).await
        }
        "auth_cancel_login" => {
            web_managed_auth_cancel_login(state.clone(), headers.clone(), &args).await
        }
        "auth_remove_account" => {
            web_managed_auth_remove_account(state.clone(), headers.clone(), &args).await
        }
        "auth_set_default_account" => {
            web_managed_auth_set_default_account(state.clone(), headers.clone(), &args).await
        }
        "auth_set_manual_subscription_expiry" => {
            web_managed_auth_set_manual_subscription_expiry(state.clone(), headers.clone(), &args)
                .await
        }
        "auth_set_subscription_expiry_rule" => {
            web_managed_auth_set_subscription_expiry_rule(state.clone(), headers.clone(), &args)
                .await
        }
        "auth_set_workspace" => {
            web_managed_auth_set_workspace(state.clone(), headers.clone(), &args).await
        }
        "auth_logout" => web_managed_auth_logout(state.clone(), headers.clone(), &args).await,
        "grok_import_auth_json" => {
            let auth_json = args
                .get("authJson")
                .or_else(|| args.get("auth_json"))
                .cloned()
                .ok_or_else(|| ApiError::bad_request("authJson is required"))?;
            let response = import_grok_auth_json(
                State(state.clone()),
                headers.clone(),
                Json(ImportGrokAuthJsonRequest { auth_json }),
            )
            .await?
            .0;
            let account = web_managed_auth_account_by_id(
                state,
                &response.account.id,
                managed_auth_provider_label(ProviderType::GrokOAuth),
            )
            .await?;
            Ok(json!({
                "ok": response.ok,
                "account": account
            }))
        }
        "kiro_import_credentials_json" => {
            let credentials = args
                .get("credentials")
                .cloned()
                .ok_or_else(|| ApiError::bad_request("credentials is required"))?;
            let response = import_kiro_credentials_json(
                State(state.clone()),
                headers.clone(),
                Json(ImportKiroCredentialsRequest { credentials }),
            )
            .await?
            .0;
            let account = web_managed_auth_account_by_id(
                state,
                &response.account.id,
                managed_auth_provider_label(ProviderType::KiroOAuth),
            )
            .await?;
            Ok(json!({ "ok": response.ok, "account": account, "source": response.source }))
        }
        "kiro_import_local_credentials" => {
            let response = import_kiro_local_credentials(
                State(state.clone()),
                headers.clone(),
                Json(ImportKiroLocalCredentialsRequest {
                    path: web_optional_string_any(&args, &["path"]),
                }),
            )
            .await?
            .0;
            let account = web_managed_auth_account_by_id(
                state,
                &response.account.id,
                managed_auth_provider_label(ProviderType::KiroOAuth),
            )
            .await?;
            Ok(json!({ "ok": response.ok, "account": account, "source": response.source }))
        }
        "kiro_import_api_key" => {
            let api_key = web_arg_string_any(&args, &["apiKey", "api_key"])?;
            let response = import_kiro_api_key(
                State(state.clone()),
                headers.clone(),
                Json(ImportKiroApiKeyRequest {
                    api_key,
                    region: web_optional_string_any(&args, &["region"]),
                }),
            )
            .await?
            .0;
            let account = web_managed_auth_account_by_id(
                state,
                &response.account.id,
                managed_auth_provider_label(ProviderType::KiroOAuth),
            )
            .await?;
            Ok(json!({ "ok": response.ok, "account": account, "source": response.source }))
        }
        "qoder_import_pat" => {
            let personal_token =
                web_arg_string_any(&args, &["personalToken", "personal_token", "pat"])?;
            let response = import_qoder_pat(
                State(state.clone()),
                headers.clone(),
                Json(ImportQoderPatRequest { personal_token }),
            )
            .await?
            .0;
            let account = web_managed_auth_account_by_id(
                state,
                &response.account.id,
                managed_auth_provider_label(ProviderType::QoderCosy),
            )
            .await?;
            Ok(json!({ "ok": response.ok, "account": account }))
        }
        "cursor_import_local_auth" => {
            let response = import_cursor_local_auth(State(state.clone()), headers.clone())
                .await?
                .0;
            let account = web_managed_auth_account_by_id(
                state,
                &response.account.id,
                managed_auth_provider_label(ProviderType::CursorOAuth),
            )
            .await?;
            Ok(json!({
                "ok": response.ok,
                "account": account,
                "source": response.source,
                "path": response.path,
                "profileError": response.profile_error,
            }))
        }
        "auth_submit_oauth_code" => {
            let provider_type = web_auth_provider_type(&args)?;
            let provider_label = managed_auth_provider_label(provider_type);
            let session_id = web_optional_string_any(&args, &["sessionId", "session_id"]);
            let state_arg = web_optional_string_any(&args, &["state"]).or_else(|| {
                session_id
                    .is_none()
                    .then(|| web_optional_string_any(&args, &["deviceCode", "device_code"]))
                    .flatten()
            });
            let callback_input =
                web_optional_string_any(&args, &["callbackUrl", "callback_url", "code"]);
            let (session_id, state_arg, code) = if provider_type == ProviderType::CodexOAuth {
                require_secure_manual_cli_origin(state, headers).await?;
                let callback_input = callback_input.ok_or_else(|| {
                    ApiError::bad_request("a complete OpenAI callback URL is required")
                })?;
                let (code, callback_state) = parse_openai_cli_callback_input(&callback_input)?;
                let manual_session_id = session_id.or_else(|| {
                    web_optional_string_any(&args, &["deviceCode", "device_code"])
                        .and_then(|value| value.strip_prefix("manual:").map(str::to_string))
                });
                (manual_session_id, Some(callback_state), Some(code))
            } else {
                (session_id, state_arg, callback_input)
            };
            let response = finish_account_login(
                State(state.clone()),
                headers.clone(),
                Json(FinishAccountLoginRequest {
                    session_id,
                    state: state_arg,
                    code,
                    execute_token_exchange: Some(true),
                    expected_provider_type: Some(provider_type),
                }),
            )
            .await?
            .0;
            let account_id = response
                .account
                .as_ref()
                .map(|account| account.id.as_str())
                .ok_or_else(|| {
                    ApiError::bad_gateway("oauth code exchange did not import account")
                })?;
            web_managed_auth_account_by_id(state, account_id, provider_label).await
        }
        "refresh_oauth_quota" => Ok(web_cached_oauth_quota(
            state,
            headers,
            &args,
            true,
            Some(web_optional_bool(&args, &["force"]).unwrap_or(true)),
        )
        .await?),
        "get_cached_oauth_quota" => {
            Ok(web_cached_oauth_quota(state, headers, &args, false, None).await?)
        }
        "get_claude_oauth_quota" => {
            let response =
                web_provider_quota(state, headers, &args, ProviderType::ClaudeOAuth).await?;
            Ok(response)
        }
        "get_codex_oauth_quota" => {
            let response =
                web_provider_quota(state, headers, &args, ProviderType::CodexOAuth).await?;
            Ok(response)
        }
        "copilot_start_device_flow" => {
            let response = start_copilot_device_login(
                State(state.clone()),
                headers.clone(),
                Json(StartCopilotDeviceLoginRequest {
                    github_domain: web_optional_string_any(
                        &args,
                        &["githubDomain", "github_domain"],
                    ),
                }),
            )
            .await?
            .0;
            Ok(json!(response.device))
        }
        "copilot_poll_for_auth" => {
            let device_code = web_arg_string_any(&args, &["deviceCode", "device_code"])?;
            let response = poll_copilot_device_login(
                State(state.clone()),
                headers.clone(),
                Json(PollCopilotDeviceLoginRequest {
                    device_code,
                    github_domain: web_optional_string_any(
                        &args,
                        &["githubDomain", "github_domain"],
                    ),
                }),
            )
            .await?
            .0;
            Ok(json!(response))
        }
        "start_proxy_server" => Ok(json!({
            "address": state.bind_addr.ip().to_string(),
            "port": state.bind_addr.port(),
        })),
        "stop_proxy_server" | "stop_proxy_with_restore" => Ok(json!(true)),
        "set_proxy_takeover_for_app" => Ok(json!(true)),
        "delete_db_backup" => {
            let id = web_arg_string_any(&args, &["filename", "id", "backupId"])?;
            crate::infra::backup::delete_backup(&state.config_dir, &id)
                .map_err(ApiError::bad_request)?;
            Ok(Value::Null)
        }
        "rename_db_backup" => {
            let id = web_arg_string_any(&args, &["oldFilename", "filename", "id"])?;
            let new_name = web_arg_string_any(&args, &["newName", "new_name"])?;
            let manifest = crate::infra::backup::rename_backup(&state.config_dir, &id, &new_name)
                .map_err(ApiError::bad_request)?;
            Ok(json!(manifest.id))
        }
        "open_file_dialog" | "save_file_dialog" | "pick_directory" => Ok(Value::Null),
        "open_external" => Ok(json!(true)),
        "open_config_folder" | "open_app_config_folder" => Ok(json!(true)),
        "restart_app" | "check_for_updates" | "install_update_and_restart" | "update_tray_menu" => {
            Ok(json!(true))
        }
        "has_codex_unify_history_backup" => Ok(json!(false)),
        "restore_codex_unified_history" => Ok(json!({
            "restoredJsonlFiles": 0,
            "restoredStateRows": 0,
            "skippedReason": "not_supported_on_server",
        })),
        "webdav_sync_save_settings" => {
            let settings: Value = web_arg_value(&args, "settings")?;
            state
                .apply_ui_settings_patch_immediate(json!({ "webdavSync": settings }))
                .await
                .map_err(ApiError::internal)?;
            Ok(json!({ "success": true }))
        }
        "s3_sync_save_settings" => {
            let settings: Value = web_arg_value(&args, "settings")?;
            state
                .apply_ui_settings_patch_immediate(json!({ "s3Sync": settings }))
                .await
                .map_err(ApiError::internal)?;
            Ok(json!({ "success": true }))
        }
        "webdav_test_connection" | "s3_test_connection" => Ok(json!({
            "success": true,
            "message": "connection test is not available on server web runtime; settings saved only",
        })),
        "webdav_sync_fetch_remote_info" | "s3_sync_fetch_remote_info" => {
            Ok(json!({ "empty": true }))
        }
        "webdav_sync_upload" | "s3_sync_upload" => {
            let backup = crate::infra::backup::create_backup(
                &state.config_dir,
                &crate::state::backup_targets(&state.config_dir),
                Some("cloud-sync-upload".to_string()),
            )
            .map_err(ApiError::internal)?;
            Ok(json!({ "status": format!("uploaded:{}", backup.id) }))
        }
        "webdav_sync_download" | "s3_sync_download" => {
            Ok(json!({ "status": "download_not_configured" }))
        }
        "get_tool_versions" => Ok(json!([])),
        "probe_tool_installations" => {
            let tool_names = args
                .get("toolNames")
                .or_else(|| args.get("tool_names"))
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok(json!(tool_names
                .into_iter()
                .map(|name| json!({
                    "toolName": name,
                    "installed": false,
                    "needs_confirmation": false,
                }))
                .collect::<Vec<_>>()))
        }
        "run_tool_lifecycle_action" => Err(ApiError::bad_request(
            "tool lifecycle actions are not available on server web runtime",
        )),
        "copilot_list_accounts" => Ok(json!([])),
        "copilot_is_authenticated" => Ok(json!(false)),
        "copilot_get_auth_status" => Ok(json!({ "authenticated": false, "accounts": [] })),
        "copilot_get_token" | "copilot_get_token_for_account" => Ok(Value::Null),
        "copilot_get_models" | "copilot_get_models_for_account" => Ok(json!([])),
        "copilot_get_usage" | "copilot_get_usage_for_account" => Ok(Value::Null),
        "copilot_logout" | "copilot_remove_account" => Ok(json!(true)),
        "copilot_set_default_account" => Ok(json!(true)),
        "copilot_poll_for_account" => Ok(Value::Null),
        "deepseek_account_add" => {
            let access_token =
                web_arg_string_any(&args, &["accessToken", "access_token", "token"])?;
            let email = web_optional_string_any(&args, &["email", "mobile", "identifier"]);
            let input = UpsertAccountInput {
                id: None,
                provider_type: ProviderType::DeepSeekAccount,
                email,
                access_token: Some(access_token),
                refresh_token: None,
                id_token: None,
                token_type: Some("Bearer".to_string()),
                api_key: None,
                extra_headers: None,
                scopes: Vec::new(),
                profile: Some(json!({ "source": "deepseek_access_token_import" })),
                raw: Some(json!({
                    "source": "deepseek_access_token_import",
                    "importedAtMs": now_ms(),
                })),
                subscription_level: None,
                entitlement_status: None,
                quota_percent: None,
                quota: None,
                quota_refreshed_at: None,
                quota_next_refresh_at: None,
                expires_at: None,
                rate_limited_until: None,
                last_refresh_error: None,
            };
            let response = upsert_account(State(state.clone()), headers.clone(), Json(input))
                .await?
                .0;
            let account_id = response.account.id;
            let accounts = state.accounts.read().await;
            let default_account_id =
                managed_auth_default_account_id(&accounts, ProviderType::DeepSeekAccount);
            let account = accounts
                .find_for_provider(ProviderType::DeepSeekAccount, Some(&account_id))
                .ok_or_else(|| ApiError::not_found("account not found after import"))?;
            Ok(deepseek_account_json(account, default_account_id))
        }
        "deepseek_account_list" => {
            let accounts = state.accounts.read().await;
            Ok(deepseek_account_status_json(&accounts)["accounts"].clone())
        }
        "deepseek_account_remove" => {
            let mut managed_args = args;
            managed_args
                .as_object_mut()
                .ok_or_else(|| ApiError::bad_request("arguments must be an object"))?
                .insert("authProvider".to_string(), json!("deepseek_account"));
            web_managed_auth_remove_account(state.clone(), headers.clone(), &managed_args).await?;
            Ok(json!(true))
        }
        "deepseek_account_set_default" => {
            let mut managed_args = args;
            managed_args
                .as_object_mut()
                .ok_or_else(|| ApiError::bad_request("arguments must be an object"))?
                .insert("authProvider".to_string(), json!("deepseek_account"));
            web_managed_auth_set_default_account(state.clone(), headers.clone(), &managed_args)
                .await?;
            Ok(json!(true))
        }
        "get_common_config_snippet" => {
            let app_type = web_arg_common_config_app_type(&args)?;
            let store = state.ui_settings.read().await;
            Ok(ui_settings::common_config_snippet_for_frontend(
                &store, app_type,
            ))
        }
        "set_common_config_snippet" => {
            let app_type = web_arg_common_config_app_type(&args)?;
            let snippet = web_arg_string_any(&args, &["snippet", "value"])?;
            state
                .mutate_ui_settings_immediate(|store| {
                    let mut snippets = store
                        .value
                        .get("commonConfigSnippets")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    if let Some(map) = snippets.as_object_mut() {
                        if snippet.trim().is_empty() {
                            map.remove(app_type);
                        } else {
                            map.insert(app_type.to_string(), json!(snippet));
                        }
                    }
                    store.apply_patch(json!({ "commonConfigSnippets": snippets }));
                })
                .await
                .map_err(ApiError::internal)?;
            Ok(Value::Null)
        }
        "extract_common_config_snippet" => {
            let _app_type = web_arg_common_config_app_type(&args)?;
            if let Some(settings_config) = args.get("settingsConfig").and_then(Value::as_str) {
                let trimmed = settings_config.trim();
                if trimmed.is_empty() {
                    return Ok(json!("{}"));
                }
                return Ok(json!(trimmed));
            }
            Ok(json!("{}"))
        }
        "stream_check_provider" => {
            let stored = web_resolve_stored_provider(state, &args).await?;
            ensure_stored_provider_outbound_allowed(state, &stored).await?;
            let config = web_stream_check_config(state).await;
            let http_client = state.http_client().await;
            let result = crate::domain::stream_check::check_provider_reachability(
                &http_client,
                &stored,
                &config,
                resolve_stream_check_probe_url,
            )
            .await;
            Ok(json!(result))
        }
        "stream_check_all_providers" => {
            let app = web_arg_app_type(&args)?;
            let proxy_targets_only = args
                .get("proxyTargetsOnly")
                .or_else(|| args.get("proxy_targets_only"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let config = web_stream_check_config(state).await;
            let http_client = state.http_client().await;
            let allowed_ids = if proxy_targets_only {
                Some(web_proxy_target_provider_ids(state, app).await)
            } else {
                None
            };
            let providers = state.providers.read().await.providers.clone();
            let mut results = Vec::new();
            for stored in providers.into_iter().filter(|item| item.app == app) {
                if allowed_ids
                    .as_ref()
                    .is_some_and(|ids| !ids.contains(&stored.provider.id))
                {
                    continue;
                }
                if ensure_stored_provider_outbound_allowed(state, &stored)
                    .await
                    .is_err()
                {
                    continue;
                }
                let result = crate::domain::stream_check::check_provider_reachability(
                    &http_client,
                    &stored,
                    &config,
                    resolve_stream_check_probe_url,
                )
                .await;
                results.push((stored.provider.id.clone(), result));
            }
            Ok(json!(results))
        }
        "model_test_provider" => {
            let stored = web_resolve_stored_provider(state, &args).await?;
            let config = web_stream_check_config(state).await;
            let probe = crate::api::provider_health_scheduler::probe_provider_and_record(
                state,
                &stored,
                &config,
                "cc-switch-manual",
            )
            .await
            .map_err(ApiError::internal)?;
            if let Err(error) =
                crate::api::provider_health_scheduler::project_recorded_probe_to_active_shares(
                    state,
                    &stored,
                    &probe,
                    "cc-switch-manual",
                )
                .await
            {
                tracing::warn!(
                    app = stored.app.as_str(),
                    provider_id = %stored.provider.id,
                    error = %error,
                    "failed to project manual Provider health result to Shares"
                );
            }
            Ok(json!(probe.result))
        }
        "model_test_all_providers" => {
            let app = web_arg_app_type(&args)?;
            let proxy_targets_only = args
                .get("proxyTargetsOnly")
                .or_else(|| args.get("proxy_targets_only"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let config = web_stream_check_config(state).await;
            let allowed_ids = if proxy_targets_only {
                Some(web_proxy_target_provider_ids(state, app).await)
            } else {
                None
            };
            let providers = state.providers.read().await.providers.clone();
            let mut results = Vec::new();
            for stored in providers.into_iter().filter(|item| item.app == app) {
                if allowed_ids
                    .as_ref()
                    .is_some_and(|ids| !ids.contains(&stored.provider.id))
                {
                    continue;
                }
                let probe = crate::api::provider_health_scheduler::probe_provider_and_record(
                    state,
                    &stored,
                    &config,
                    "cc-switch-manual",
                )
                .await
                .map_err(ApiError::internal)?;
                if let Err(error) =
                    crate::api::provider_health_scheduler::project_recorded_probe_to_active_shares(
                        state,
                        &stored,
                        &probe,
                        "cc-switch-manual",
                    )
                    .await
                {
                    tracing::warn!(
                        app = stored.app.as_str(),
                        provider_id = %stored.provider.id,
                        error = %error,
                        "failed to project manual Provider health result to Shares"
                    );
                }
                results.push((stored.provider.id, probe.result));
            }
            Ok(json!(results))
        }
        "fetch_models_for_config" => web_fetch_models_for_config(state, &args).await,
        "get_codex_oauth_models" | "get_antigravity_oauth_models" => Ok(json!([])),
        "get_grok_oauth_models" => Ok(json!(grok_oauth_default_models())),
        "update_share_description" => {
            let payload = web_payload(&args, &["params", "input"]);
            let share = web_patch_share_settings(
                state,
                payload,
                ShareSettingsPatch {
                    description: web_optional_string_any(payload, &["description", "value"])
                        .map(Some),
                    ..ShareSettingsPatch::default()
                },
            )
            .await?;
            Ok(json!(share))
        }
        "update_share_for_sale" => {
            let payload = web_payload(&args, &["params", "input"]);
            let share = web_patch_share_settings(
                state,
                payload,
                ShareSettingsPatch {
                    for_sale: web_optional_string_any(payload, &["forSale", "for_sale"]),
                    ..ShareSettingsPatch::default()
                },
            )
            .await?;
            Ok(json!(share))
        }
        "update_share_token_limit" => {
            let payload = web_payload(&args, &["params", "input"]);
            let token_limit = payload
                .get("tokenLimit")
                .or_else(|| payload.get("token_limit"))
                .and_then(Value::as_i64);
            let share = web_patch_share_settings(
                state,
                payload,
                ShareSettingsPatch {
                    token_limit,
                    ..ShareSettingsPatch::default()
                },
            )
            .await?;
            Ok(json!(share))
        }
        "update_share_parallel_limit" => {
            let payload = web_payload(&args, &["params", "input"]);
            let parallel_limit = payload
                .get("parallelLimit")
                .or_else(|| payload.get("parallel_limit"))
                .and_then(Value::as_i64);
            let share = web_patch_share_settings(
                state,
                payload,
                ShareSettingsPatch {
                    parallel_limit,
                    ..ShareSettingsPatch::default()
                },
            )
            .await?;
            Ok(json!(share))
        }
        "update_share_expiration" => {
            let payload = web_payload(&args, &["params", "input"]);
            let expires_at = web_optional_string_any(payload, &["expiresAt", "expires_at"]);
            let share = web_patch_share_settings(
                state,
                payload,
                ShareSettingsPatch {
                    expires_at,
                    ..ShareSettingsPatch::default()
                },
            )
            .await?;
            Ok(json!(share))
        }
        "update_share_for_sale_official_price_percent" => {
            let payload = web_payload(&args, &["params", "input"]);
            let official_price_percent = match payload
                .get("officialPricePercent")
                .or_else(|| payload.get("official_price_percent"))
            {
                Some(Value::Null) => Some(None),
                Some(raw) => Some(Some(
                    raw.as_u64()
                        .and_then(|value| u16::try_from(value).ok())
                        .ok_or_else(|| {
                            ApiError::bad_request(
                                "officialPricePercent must be an integer between 1 and 100",
                            )
                        })?,
                )),
                None => None,
            };
            let pricing = web_optional_deserialize::<BTreeMap<AppKind, u16>>(
                payload,
                "forSaleOfficialPricePercentByApp",
            )?
            .or_else(|| {
                web_optional_deserialize::<BTreeMap<AppKind, u16>>(
                    payload,
                    "for_sale_official_price_percent_by_app",
                )
                .ok()
                .flatten()
            });
            let share = web_patch_share_settings(
                state,
                payload,
                ShareSettingsPatch {
                    for_sale_official_price_percent_by_app: pricing,
                    official_price_percent,
                    ..ShareSettingsPatch::default()
                },
            )
            .await?;
            Ok(json!(share))
        }
        "update_share_subdomain" => {
            let payload = web_payload(&args, &["params", "input"]);
            let share_id = web_arg_share_id(payload)?;
            let subdomain = web_arg_string_any(payload, &["subdomain"])?;
            let expected_config_revision = web_optional_i64(
                payload,
                &["expectedConfigRevision", "expected_config_revision"],
            )
            .map(|revision| {
                u64::try_from(revision).map_err(|_| {
                    ApiError::bad_request("expectedConfigRevision must be non-negative")
                })
            })
            .transpose()?;
            let response = update_share_subdomain(
                State(state.clone()),
                headers.clone(),
                Path(share_id),
                Json(UpdateShareSubdomainRequest {
                    subdomain,
                    expected_config_revision,
                }),
            )
            .await?
            .0;
            Ok(json!(response.share))
        }
        "enable_share" => {
            let share_id = web_arg_share_id(&args)?;
            let response = resume_share(State(state.clone()), headers.clone(), Path(share_id))
                .await?
                .0;
            Ok(json!(response.share))
        }
        "disable_share" => {
            let share_id = web_arg_share_id(&args)?;
            let response = pause_share(State(state.clone()), headers.clone(), Path(share_id))
                .await?
                .0;
            Ok(json!(response.share))
        }
        "import_shares" => {
            let shares: Vec<Share> = web_arg_value_any(&args, &["shares"])?;
            for share in &shares {
                crate::domain::sharing::invariants::validate_share_import(share)
                    .map_err(map_share_patch_error)?;
            }
            let response = import_shares(
                State(state.clone()),
                headers.clone(),
                Json(ImportSharesRequest { shares }),
            )
            .await?
            .0;
            Ok(json!(response.imported))
        }
        "configure_tunnel" => {
            web_configure_share_tunnel(state, &args).await?;
            Ok(Value::Null)
        }
        "get_claude_common_config_snippet" => {
            let store = state.ui_settings.read().await;
            Ok(ui_settings::common_config_snippet_for_frontend(
                &store, "claude",
            ))
        }
        "set_claude_common_config_snippet" => {
            let snippet = web_arg_string_any(&args, &["snippet", "value"])?;
            state
                .mutate_ui_settings_immediate(|store| {
                    let mut snippets = store
                        .value
                        .get("commonConfigSnippets")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    if let Some(map) = snippets.as_object_mut() {
                        if snippet.trim().is_empty() {
                            map.remove("claude");
                        } else {
                            map.insert("claude".to_string(), json!(snippet));
                        }
                    }
                    store.apply_patch(json!({ "commonConfigSnippets": snippets }));
                })
                .await
                .map_err(ApiError::internal)?;
            Ok(Value::Null)
        }
        "check_env_conflicts" => Ok(json!([])),
        "delete_env_vars" => Ok(json!({ "backupPath": Value::Null })),
        "restore_env_backup" => Ok(Value::Null),
        "get_auto_launch_status" => Ok(json!(false)),
        "set_auto_launch" => Ok(Value::Null),
        "get_subscription_quota" => {
            let tool = web_arg_string_any(&args, &["tool"])?;
            let force = web_optional_bool(&args, &["force"]).unwrap_or(false);
            Ok(web_subscription_quota(state, headers, &tool, force).await?)
        }
        "read_live_provider_settings" => Ok(json!({})),
        "test_api_endpoints" => {
            let urls: Vec<String> = web_arg_value(&args, "urls")?;
            let timeout_secs = web_optional_u64(&args, &["timeoutSecs", "timeout_secs"]);
            let http_client = state.http_client().await;
            Ok(json!(
                super::endpoint_latency::test_api_endpoints(&http_client, urls, timeout_secs,)
                    .await?
            ))
        }
        "get_custom_endpoints" => Ok(json!([])),
        "add_custom_endpoint" | "remove_custom_endpoint" | "update_endpoint_last_used" => {
            Ok(Value::Null)
        }
        "remove_provider_from_live_config" => Ok(json!(true)),
        "import_opencode_providers_from_live"
        | "import_openclaw_providers_from_live"
        | "import_hermes_providers_from_live" => Ok(json!([])),
        "get_opencode_live_provider_ids"
        | "get_openclaw_live_provider_ids"
        | "get_hermes_live_provider_ids" => Ok(json!([])),
        "import_claude_desktop_providers_from_claude"
        | "ensure_claude_desktop_official_provider" => Ok(json!(false)),
        "get_claude_desktop_status" => Ok(json!({ "installed": false, "configured": false })),
        "get_claude_desktop_default_routes" => Ok(json!([])),
        "get_claude_code_config_path" => Ok(json!("")),
        "set_app_config_dir_override" => Ok(Value::Null),
        "apply_claude_plugin_config"
        | "apply_claude_onboarding_skip"
        | "clear_claude_onboarding_skip" => Ok(Value::Null),
        "codex_referral_eligibility" => {
            Box::pin(async {
            let provider_id = web_optional_string_any(&args, &["providerId", "provider_id"])
                .ok_or_else(|| ApiError::bad_request("providerId is required"))?;
            let expected_revision = codex_provider_expected_revision(&args)?;
            let mut target =
                prepare_codex_provider_control_target(state, &provider_id, expected_revision)
                    .await?;
            let timeout = codex_control_timeout(state).await;
            let mut result = crate::clients::oauth::codex_referrals::query_eligibility(
                &target.session_key,
                target.access_token()?,
                &target.workspace_id,
                timeout,
            )
            .await
            .map_err(map_referral_error)?;
            if result.unauthorized() {
                target = refresh_codex_provider_control_target(
                    state,
                    &provider_id,
                    expected_revision,
                    &target,
                )
                .await?;
                result = crate::clients::oauth::codex_referrals::query_eligibility(
                    &target.session_key,
                    target.access_token()?,
                    &target.workspace_id,
                    timeout,
                )
                .await
                .map_err(map_referral_error)?;
            }
            serde_json::to_value(result).map_err(ApiError::bad_request)
            })
            .await
        }
        "codex_referral_tracking" => {
            Box::pin(async {
            let provider_id = web_optional_string_any(&args, &["providerId", "provider_id"])
                .ok_or_else(|| ApiError::bad_request("providerId is required"))?;
            let expected_revision = codex_provider_expected_revision(&args)?;
            let limit = web_optional_u64(&args, &["limit"])
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(100);
            let mut target =
                prepare_codex_provider_control_target(state, &provider_id, expected_revision)
                    .await?;
            let timeout = codex_control_timeout(state).await;
            let mut result = crate::clients::oauth::codex_referrals::query_tracking(
                &target.session_key,
                target.access_token()?,
                &target.workspace_id,
                limit,
                timeout,
            )
            .await
            .map_err(map_referral_error)?;
            if result.unauthorized() {
                target = refresh_codex_provider_control_target(
                    state,
                    &provider_id,
                    expected_revision,
                    &target,
                )
                .await?;
                result = crate::clients::oauth::codex_referrals::query_tracking(
                    &target.session_key,
                    target.access_token()?,
                    &target.workspace_id,
                    limit,
                    timeout,
                )
                .await
                .map_err(map_referral_error)?;
            }
            serde_json::to_value(result).map_err(ApiError::bad_request)
            })
            .await
        }
        "codex_referral_send" => {
            Box::pin(async {
            let provider_id = web_optional_string_any(&args, &["providerId", "provider_id"])
                .ok_or_else(|| ApiError::bad_request("providerId is required"))?;
            let expected_revision = codex_provider_expected_revision(&args)?;
            let emails =
                web_optional_deserialize::<Vec<String>>(&args, "emails")?.unwrap_or_default();
            let emails = crate::clients::oauth::codex_referrals::normalize_referral_emails(&emails)
                .map_err(map_referral_error)?;
            let mut target =
                prepare_codex_provider_control_target(state, &provider_id, expected_revision)
                    .await?;
            let timeout = codex_control_timeout(state).await;
            let mut result = crate::clients::oauth::codex_referrals::send_invites(
                &target.session_key,
                target.access_token()?,
                &target.workspace_id,
                &emails,
                timeout,
            )
            .await
            .map_err(map_referral_error)?;
            if result.unauthorized() {
                target = refresh_codex_provider_control_target(
                    state,
                    &provider_id,
                    expected_revision,
                    &target,
                )
                .await?;
                result = crate::clients::oauth::codex_referrals::send_invites(
                    &target.session_key,
                    target.access_token()?,
                    &target.workspace_id,
                    &emails,
                    timeout,
                )
                .await
                .map_err(map_referral_error)?;
            }
            serde_json::to_value(result).map_err(ApiError::bad_request)
            })
            .await
        }
        "codex_banked_reset_status" => {
            Box::pin(async {
            let provider_id = web_optional_string_any(&args, &["providerId", "provider_id"]);
            let expected_revision =
                web_optional_u64(&args, &["expectedRevision", "expected_revision"]);
            let account_id = web_optional_string_any(&args, &["accountId", "account_id"]);
            let mut target = resolve_codex_control_account(
                state,
                provider_id.as_deref(),
                expected_revision,
                account_id.as_deref(),
            )
            .await?;
            let account_id = target.account.id.clone();
            if provider_id
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty())
            {
                state
                    .refresh_managed_account_if_needed_for_generation(
                        ProviderType::CodexOAuth,
                        &account_id,
                        target.account.auth_identity_generation,
                    )
                    .await
                    .map_err(crate::api::providers::map_managed_account_refresh_error)?;
                target = resolve_codex_control_account(
                    state,
                    provider_id.as_deref(),
                    expected_revision,
                    Some(account_id.as_str()),
                )
                .await?;
                state
                    .refresh_codex_quota_for_account(
                        &account_id,
                        target.account.auth_identity_generation,
                        web_optional_bool(&args, &["force"]).unwrap_or(false),
                    )
                    .await
                    .map_err(|error| ApiError::bad_gateway(error.to_string()))?;
            } else {
                let _response = account_quota(
                    State(state.clone()),
                    headers.clone(),
                    Path(account_id.clone()),
                    Query(AccountQuotaQuery {
                        refresh: Some(true),
                        force: web_optional_bool(&args, &["force"]),
                    }),
                )
                .await?
                .0;
            }
            let account = state
                .find_account_by_id(&account_id)
                .await
                .ok_or_else(|| ApiError::not_found("codex oauth account not found"))?;
            Ok(
                crate::clients::oauth::quota::codex_banked_reset_status_snapshot(
                    &account,
                    crate::infra::time::now_ms() as i64,
                ),
            )
            })
            .await
        }
        "codex_banked_reset_consume" => {
            Box::pin(async {
            let provider_id = web_optional_string_any(&args, &["providerId", "provider_id"]);
            let expected_revision =
                web_optional_u64(&args, &["expectedRevision", "expected_revision"]);
            let account_id = web_optional_string_any(&args, &["accountId", "account_id"]);
            let credit_id =
                web_optional_string_any(&args, &["creditId", "credit_id"]).unwrap_or_default();
            let target = resolve_codex_control_account(
                state,
                provider_id.as_deref(),
                expected_revision,
                account_id.as_deref(),
            )
            .await?;
            state
                .refresh_managed_account_if_needed_for_generation(
                    ProviderType::CodexOAuth,
                    &target.account.id,
                    target.account.auth_identity_generation,
                )
                .await
                .map_err(crate::api::providers::map_managed_account_refresh_error)?;
            let account_id = target.account.id.clone();
            let (result, auth_identity_generation) = {
                let _process_lock = state.lock_codex_banked_reset(&account_id).await;
                let _file_lock = state
                    .acquire_codex_banked_reset_file_lock(&account_id)
                    .await
                    .map_err(ApiError::bad_gateway)?;
                let mut target = resolve_codex_control_account(
                    state,
                    provider_id.as_deref(),
                    expected_revision,
                    Some(account_id.as_str()),
                )
                .await?;
                let http = state.http_client().await;
                let timeout_ms = state.oauth_quota_refresh_timeout_ms().await;
                let timeout = std::time::Duration::from_millis(timeout_ms.max(1_000) as u64);
                let redeem_request_id =
                    crate::clients::oauth::codex_reset_credits::generate_redeem_request_id();
                let mut result = crate::clients::oauth::codex_reset_credits::consume_reset_credit_with_request_id(
                    &http,
                    target.access_token()?,
                    Some(&target.workspace_id),
                    &credit_id,
                    &redeem_request_id,
                    timeout,
                )
                .await;
                if matches!(
                    result,
                    Err(crate::clients::oauth::codex_reset_credits::BankedResetActionError::UpstreamHttp(401, _))
                ) {
                    state
                        .refresh_managed_account_now_for_generation(
                            ProviderType::CodexOAuth,
                            &account_id,
                            target.account.auth_identity_generation,
                        )
                        .await
                        .map_err(crate::api::providers::map_managed_account_refresh_error)?;
                    let refreshed = resolve_codex_control_account(
                        state,
                        provider_id.as_deref(),
                        expected_revision,
                        Some(account_id.as_str()),
                    )
                    .await?;
                    ensure_codex_control_identity_unchanged(&target, &refreshed)?;
                    target = refreshed;
                    result = crate::clients::oauth::codex_reset_credits::consume_reset_credit_with_request_id(
                        &http,
                        target.access_token()?,
                        Some(&target.workspace_id),
                        &credit_id,
                        &redeem_request_id,
                        timeout,
                    )
                    .await;
                }
                let result = result.map_err(|error| ApiError::bad_gateway(error.message()))?;
                (result, target.account.auth_identity_generation)
            };
            if let Err(error) = state
                .refresh_codex_quota_for_account(&account_id, auth_identity_generation, true)
                .await
            {
                tracing::warn!(
                    account_id = %account_id,
                    error = %error,
                    "banked reset credit was consumed but the follow-up quota refresh failed"
                );
            }
            Ok(json!({
                "code": result.code,
                "creditId": result.credit_id,
                "redeemRequestId": result.redeem_request_id,
                "windowsReset": result.windows_reset,
                "availableCount": result.available_count,
                "remainingCredits": result.remaining_credits,
            }))
            })
            .await
        }
        "open_provider_terminal" => Err(ApiError::not_implemented(
            "open_provider_terminal is not available in server web runtime",
        )),
        _ => Err(ApiError::web_invoke_not_wired(format!(
            "legacy invoke command '{command}' is registered but has no dispatcher"
        ))),
    }
}

#[derive(Debug, Clone)]
struct CodexControlTarget {
    account: crate::domain::accounts::store::Account,
    workspace_id: String,
    session_key: String,
}

impl CodexControlTarget {
    fn access_token(&self) -> Result<&str, ApiError> {
        self.account
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::bad_request("codex oauth account requires an access token"))
    }
}

async fn codex_control_timeout(state: &ServerState) -> std::time::Duration {
    let timeout_ms = state.oauth_quota_refresh_timeout_ms().await;
    std::time::Duration::from_millis(timeout_ms.max(1_000) as u64)
}

fn codex_provider_expected_revision(args: &Value) -> Result<u64, ApiError> {
    web_optional_u64(args, &["expectedRevision", "expected_revision"])
        .ok_or_else(|| ApiError::bad_request("expectedRevision is required"))
}

fn map_referral_error(error: crate::clients::oauth::codex_referrals::ReferralError) -> ApiError {
    match error {
        crate::clients::oauth::codex_referrals::ReferralError::InvalidInput(message) => {
            ApiError::bad_request(message)
        }
        other => ApiError::bad_gateway(other.message()),
    }
}

async fn prepare_codex_provider_control_target(
    state: &ServerState,
    provider_id: &str,
    expected_revision: u64,
) -> Result<CodexControlTarget, ApiError> {
    let target =
        resolve_codex_provider_control_target(state, provider_id, expected_revision).await?;
    state
        .refresh_managed_account_if_needed_for_generation(
            ProviderType::CodexOAuth,
            &target.account.id,
            target.account.auth_identity_generation,
        )
        .await
        .map_err(crate::api::providers::map_managed_account_refresh_error)?;
    resolve_codex_provider_control_target(state, provider_id, expected_revision).await
}

async fn refresh_codex_provider_control_target(
    state: &ServerState,
    provider_id: &str,
    expected_revision: u64,
    current: &CodexControlTarget,
) -> Result<CodexControlTarget, ApiError> {
    state
        .refresh_managed_account_now_for_generation(
            ProviderType::CodexOAuth,
            &current.account.id,
            current.account.auth_identity_generation,
        )
        .await
        .map_err(crate::api::providers::map_managed_account_refresh_error)?;
    resolve_codex_provider_control_target(state, provider_id, expected_revision).await
}

async fn resolve_codex_provider_control_target(
    state: &ServerState,
    provider_id: &str,
    expected_revision: u64,
) -> Result<CodexControlTarget, ApiError> {
    if state.credential_persistence_degraded() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "managed account credentials are waiting for durable persistence",
        ));
    }
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return Err(ApiError::bad_request("providerId is required"));
    }
    let providers = state.providers_snapshot().await;
    let surfaces = providers
        .providers
        .iter()
        .filter(|stored| {
            stored.provider.id == provider_id
                || crate::domain::providers::bundle::bundle_id(&stored.provider)
                    == Some(provider_id)
        })
        .collect::<Vec<_>>();
    if surfaces.is_empty() {
        return Err(ApiError::not_found("OpenAI OAuth Provider not found"));
    }
    let revision = surfaces
        .iter()
        .map(|stored| stored.resource.revision)
        .max()
        .unwrap_or_default();
    if expected_revision != revision {
        return Err(ApiError::conflict_code(
            "cc_switch_provider_revision_conflict",
            format!(
                "Provider revision conflict: expected {}, current {revision}",
                expected_revision
            ),
        ));
    }
    let accounts = state.accounts_snapshot().await;
    let mut resolved: Option<crate::domain::accounts::store::Account> = None;
    for stored in surfaces {
        if crate::domain::providers::runtime::managed_account_provider_type(stored)
            != Some(ProviderType::CodexOAuth)
        {
            continue;
        }
        let account =
            crate::domain::providers::runtime::authoritative_managed_account(stored, &accounts)
                .cloned()
                .ok_or_else(|| {
                    ApiError::conflict_code(
                        "cc_switch_provider_account_identity_stale",
                        "OpenAI OAuth Provider account binding is missing or stale",
                    )
                })?;
        if resolved.as_ref().is_some_and(|current| {
            current.id != account.id
                || current.auth_identity_generation != account.auth_identity_generation
        }) {
            return Err(ApiError::conflict_code(
                "cc_switch_provider_identity_conflict",
                "OpenAI OAuth Provider surfaces do not share one account identity",
            ));
        }
        resolved = Some(account);
    }
    let account = resolved.ok_or_else(|| {
        ApiError::bad_request("Provider is not backed by an OpenAI OAuth account")
    })?;
    let mut target = codex_control_target(account)?;
    target.session_key = format!(
        "{}:{provider_id}:{revision}:{}",
        state.process_instance_id, target.session_key
    );
    Ok(target)
}

async fn resolve_codex_control_account(
    state: &ServerState,
    provider_id: Option<&str>,
    expected_revision: Option<u64>,
    account_id: Option<&str>,
) -> Result<CodexControlTarget, ApiError> {
    if let Some(provider_id) = provider_id.map(str::trim).filter(|value| !value.is_empty()) {
        let expected_revision = expected_revision
            .ok_or_else(|| ApiError::bad_request("expectedRevision is required"))?;
        let target =
            resolve_codex_provider_control_target(state, provider_id, expected_revision).await?;
        if account_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some_and(|account_id| account_id != target.account.id)
        {
            return Err(ApiError::conflict_code(
                "cc_switch_provider_account_binding_mismatch",
                "requested account does not match the Provider binding",
            ));
        }
        return Ok(target);
    }
    if state.credential_persistence_degraded() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "managed account credentials are waiting for durable persistence",
        ));
    }
    let accounts = state.accounts_snapshot().await;
    if !accounts
        .accounts
        .iter()
        .any(|account| account.provider_type == ProviderType::CodexOAuth)
    {
        return Err(ApiError::not_found("codex oauth account not found"));
    }
    let account = accounts.active_codex_oauth_account().ok_or_else(|| {
        ApiError::conflict_code(
            "cc_switch_codex_inactive_account",
            "select the active Codex OAuth account before using banked reset credits",
        )
    })?;
    if account_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|requested| requested != account.id)
    {
        return Err(ApiError::conflict_code(
            "cc_switch_codex_inactive_account",
            "Codex OAuth account is not active",
        ));
    }
    codex_control_target(account.clone())
}

fn ensure_codex_control_identity_unchanged(
    before: &CodexControlTarget,
    after: &CodexControlTarget,
) -> Result<(), ApiError> {
    if before.account.id != after.account.id
        || before.account.auth_identity_generation != after.account.auth_identity_generation
        || before.workspace_id != after.workspace_id
    {
        return Err(ApiError::conflict_code(
            "cc_switch_provider_account_identity_stale",
            "OpenAI OAuth account identity changed while consuming a reset credit",
        ));
    }
    Ok(())
}

fn codex_control_target(
    account: crate::domain::accounts::store::Account,
) -> Result<CodexControlTarget, ApiError> {
    let workspace_id = crate::domain::accounts::store::effective_codex_workspace_id(&account)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request("codex oauth account requires a verified workspace")
        })?;
    let session_key = format!(
        "{}:{}:{}",
        account.id, account.auth_identity_generation, workspace_id
    );
    let target = CodexControlTarget {
        account,
        workspace_id,
        session_key,
    };
    target.access_token()?;
    Ok(target)
}

fn grok_oauth_default_models() -> Vec<Value> {
    [
        ("grok-4.5", "Grok 4.5"),
        ("grok-4.3", "Grok 4.3"),
        ("grok-build-0.1", "Grok Build 0.1"),
        ("grok-composer-2.5-fast", "Grok Composer 2.5 Fast"),
        ("grok-4.20-0309-reasoning", "Grok 4.20 Reasoning"),
        ("grok-4.20-0309-non-reasoning", "Grok 4.20 Non-Reasoning"),
    ]
    .into_iter()
    .map(|(id, display_name)| {
        json!({
            "id": id,
            "ownedBy": "xai",
            "displayName": display_name,
        })
    })
    .collect()
}

fn resolve_stream_check_probe_url(
    stored: &crate::domain::providers::store::StoredProvider,
    model: &str,
) -> Result<String, String> {
    let adapter = proxy::adapters::adapter_for(stored.app, stored.provider_type);
    let route = match stored.app {
        AppKind::Claude => ProxyRoute::ClaudeMessages,
        AppKind::Codex => ProxyRoute::CodexResponses,
        AppKind::Gemini => ProxyRoute::Gemini,
    };
    let gemini_path = if stored.app == AppKind::Gemini {
        format!("/v1beta/models/{model}:generateContent")
    } else {
        String::new()
    };
    let endpoint = adapter
        .resolve_endpoint(
            route,
            (!gemini_path.is_empty()).then_some(gemini_path),
            stored,
        )
        .map_err(|error| error.to_string())?;
    Ok(crate::domain::stream_check::reachability_origin(&endpoint))
}
