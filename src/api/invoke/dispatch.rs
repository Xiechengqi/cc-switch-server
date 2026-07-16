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
            "desktop invoke command '{command}' is excluded from cc-switch-server ({})",
            command_def.feature
        )));
    }

    if web_invoke_requires_session(&state, &command).await {
        require_session(&state, &headers).await?;
    }
    if !command_def.implemented {
        return Err(ApiError::web_invoke_not_wired(format!(
            "desktop invoke command '{command}' is registered as {} but is not bridged yet",
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
        "get_admin_version_info" => Ok(json!(
            crate::api::self_update::build_admin_version_response(state).await
        )),
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
            let store = state.ui_settings.read().await;
            let config = state.config.read().await;
            Ok(store.settings_for_frontend(&config))
        }
        "get_owner_payout_profile" => {
            let response =
                crate::api::payout::get_payout_profile(State(state.clone()), headers.clone())
                    .await?;
            Ok(json!(response.0))
        }
        "save_owner_payout_profile" => {
            let value = args.get("profile").cloned().unwrap_or(args);
            let input = serde_json::from_value::<crate::api::payout::SavePayoutProfileInput>(value)
                .map_err(ApiError::bad_request)?;
            let response = crate::api::payout::save_payout_profile(
                State(state.clone()),
                headers.clone(),
                Json(input),
            )
            .await?;
            Ok(json!(response.0))
        }
        "clear_owner_payout_profile" => {
            let response =
                crate::api::payout::clear_payout_profile(State(state.clone()), headers.clone())
                    .await?;
            Ok(json!(response.0))
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
            state
                .apply_ui_settings_patch_immediate(patch)
                .await
                .map_err(ApiError::internal)?;
            Ok(json!(true))
        }
        "get_global_proxy_config" => Ok(json!(web_global_proxy_config_json(state))),
        "get_global_proxy_url" => {
            let config = state.config.read().await;
            Ok(json!(config.upstream_proxy.url))
        }
        "get_upstream_proxy_status" => Ok(json!(web_upstream_proxy_status_json(state).await)),
        "set_global_proxy_url" => {
            let url = web_optional_string_any(&args, &["url", "proxyUrl", "proxy_url"])
                .unwrap_or_default();
            let mut config = state.config.read().await.clone();
            let input = if url.trim().is_empty() {
                UpdateUpstreamProxyInput {
                    url: None,
                    clear: Some(true),
                    follow_system_proxy: None,
                }
            } else {
                UpdateUpstreamProxyInput {
                    url: Some(url),
                    clear: None,
                    follow_system_proxy: None,
                }
            };
            config
                .update_upstream_proxy(input)
                .map_err(ApiError::bad_request)?;
            state
                .replace_config(config)
                .await
                .map_err(ApiError::internal)?;
            Ok(Value::Null)
        }
        "test_proxy_url" => {
            let url = web_arg_string_any(&args, &["url", "proxyUrl"])?;
            Ok(json!(web_test_proxy_url(&url).await))
        }
        "scan_local_proxies" => Ok(json!(web_scan_local_proxies().await)),
        "is_portable_mode" => Ok(json!(false)),
        "get_app_config_dir_override" => Ok(json!(null)),
        "get_app_config_path" => Ok(json!(state.config_dir.display().to_string())),
        "get_config_dir" => {
            let _app = web_arg_app(&args).or_else(|_| web_arg_app_type(&args))?;
            Ok(json!(""))
        }
        "get_providers" => {
            let app = match web_arg_app_for_read(&args)? {
                Some(app) => app,
                None => return Ok(json!({})),
            };
            let providers = state.providers.read().await;
            Ok(json!(provider_record_for_app(&providers.providers, app)))
        }
        "get_current_provider" => {
            let app = match web_arg_app_for_read(&args)? {
                Some(app) => app,
                None => return Ok(json!("")),
            };
            let providers = state.providers.read().await;
            let ui_settings = state.ui_settings.read().await.for_frontend();
            let current =
                current_provider::resolve_current_provider_id(&providers, &ui_settings, app)
                    .unwrap_or_default();
            Ok(json!(current))
        }
        "add_provider" | "update_provider" => {
            let app = web_arg_app(&args)?;
            let provider: Provider = web_arg_value(&args, "provider")?;
            if provider.name.trim().is_empty() {
                return Err(ApiError::bad_request("provider name is required"));
            }
            state
                .mutate_providers_immediate(|providers| providers.upsert(app, provider))
                .await
                .map_err(ApiError::internal)?;
            Ok(json!(true))
        }
        "update_providers_sort_order" => {
            let app = web_arg_app(&args)?;
            let updates: Vec<ProviderSortUpdate> = web_arg_value(&args, "updates")?;
            let _changed = state
                .mutate_providers_immediate_if_changed(|providers| {
                    let changed = providers.update_sort_order(app, updates);
                    (changed, changed)
                })
                .await
                .map_err(ApiError::internal)?;
            Ok(json!(true))
        }
        "delete_provider" => {
            let app = web_arg_app(&args)?;
            let id = web_arg_string(&args, "id")?;
            let deleted = delete_provider_with_share_cascade(state, app, &id).await?;
            Ok(json!(deleted))
        }
        "switch_provider" => {
            let app = web_arg_app(&args)?;
            let id = web_arg_string(&args, "id")?;
            let exists = state
                .providers
                .read()
                .await
                .providers
                .iter()
                .any(|provider| provider.app == app && provider.provider.id == id);
            if !exists {
                return Err(ApiError::not_found("provider not found"));
            }
            state
                .apply_ui_settings_patch_immediate(json!({
                    current_provider::current_provider_settings_key(app): id
                }))
                .await
                .map_err(ApiError::internal)?;
            Ok(json!({ "warnings": [] }))
        }
        "clear_current_provider" => {
            let app = web_arg_app(&args)?;
            state
                .apply_ui_settings_patch_immediate(json!({
                    current_provider::current_provider_settings_key(app): ""
                }))
                .await
                .map_err(ApiError::internal)?;
            Ok(json!({ "warnings": [] }))
        }
        "get_provider_health" => {
            let app = web_arg_app_type(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id"])?;
            let failover = state.failover.read().await;
            let key = format!("{}:{provider_id}", app.as_str());
            let breaker = failover.breakers.get(&key);
            Ok(json!(web_provider_health_json(app, &provider_id, breaker)))
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
        "list_share_markets" => {
            let markets = fetch_public_markets_from_router(state).await?;
            Ok(json!(markets))
        }
        "create_share" => {
            let input = web_share_upsert_input(state, &args).await?;
            let response = upsert_share(State(state.clone()), headers.clone(), Json(input))
                .await?
                .0;
            Ok(web_share_json(
                &state.config_snapshot().await,
                &response.share,
            )?)
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
            let share = web_save_provider_share(state, &args).await?;
            Ok(json!(share))
        }
        "update_share_owner_email" => {
            let share = web_update_share_owner_email(state, headers, &args).await?;
            Ok(json!(share))
        }
        "transfer_share_owner" => {
            let share = web_transfer_share_owner(state, headers, &args).await?;
            Ok(json!(share))
        }
        "authorize_share_market" => {
            let id = web_arg_share_id(&args)?;
            let value = web_payload(&args, &["params", "input"]);
            let market_email = web_arg_string_any(value, &["marketEmail", "market_email"])?;
            let response = authorize_share_market(
                State(state.clone()),
                headers.clone(),
                Path(id),
                Json(AuthorizeShareMarketRequest { market_email }),
            )
            .await?
            .0;
            Ok(json!(response.share))
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
            crate::client_tunnel_provision::claim_client_tunnel_config(state, &config).await?;
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
        "get_usage_summary" => {
            let filter = web_usage_stats_filter_from_args(&args);
            let usage = state.usage.read().await;
            Ok(json!(usage.rollup_filtered(&filter)))
        }
        "get_usage_summary_by_app" => {
            let filter = web_usage_stats_filter_from_args(&args);
            let usage = state.usage.read().await;
            Ok(json!(usage.summary_by_app(&filter)))
        }
        "get_installed_skills" => Ok(json!([])),
        "get_usage_trends" => {
            let usage = state.usage.read().await;
            let filter = UsageStatsFilter {
                window_ms: Some(24 * 60 * 60 * 1000),
                ..UsageStatsFilter::default()
            };
            Ok(json!(usage.trends(&filter)))
        }
        "get_provider_stats" => {
            let usage = state.usage.read().await;
            Ok(json!(usage.provider_stats(&UsageStatsFilter::default())))
        }
        "get_model_stats" => {
            let usage = state.usage.read().await;
            Ok(json!(usage.model_stats(&UsageStatsFilter::default())))
        }
        "get_request_logs" => {
            let usage = state.usage.read().await;
            Ok(web_request_logs_json(&usage, &args))
        }
        "get_request_detail" => {
            let id = web_arg_string(&args, "id").or_else(|_| web_arg_string(&args, "requestId"))?;
            let usage = state.usage.read().await;
            let log = usage.logs.iter().find(|log| log.request_id == id).cloned();
            Ok(json!(log))
        }
        "get_proxy_config_for_app" => {
            let app = web_arg_app_type(&args)?;
            Ok(json!(web_app_proxy_config_json(state, app).await))
        }
        "get_available_providers_for_failover" => {
            let app = web_arg_app_type(&args)?;
            Ok(json!(
                web_available_providers_for_failover(state, app).await
            ))
        }
        "get_proxy_status" => Ok(json!(web_proxy_status_json(state).await)),
        "get_proxy_takeover_status" => Ok(json!(web_proxy_takeover_status_json(state).await)),
        "is_proxy_running" => Ok(json!(true)),
        "is_live_takeover_active" => Ok(json!(web_is_live_takeover_active(state).await)),
        "update_global_proxy_config" => {
            let input = web_upstream_proxy_input(&args)?;
            let response =
                update_upstream_proxy(State(state.clone()), headers.clone(), Json(input))
                    .await?
                    .0;
            Ok(json!(response.upstream_proxy))
        }
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
        "get_auto_failover_enabled" => {
            let app = web_arg_app_type(&args)?;
            let failover = state.failover.read().await;
            let enabled = failover
                .apps
                .get(&app)
                .map(|config| config.enabled)
                .unwrap_or(false);
            Ok(json!(enabled))
        }
        "get_failover_queue" => {
            let app = web_arg_app_type(&args)?;
            let failover = state.failover.read().await;
            let providers = state.providers.read().await;
            let queue = failover
                .apps
                .get(&app)
                .map(|config| config.provider_queue.as_slice())
                .unwrap_or(&[]);
            let items = queue
                .iter()
                .enumerate()
                .map(|(index, provider_id)| {
                    let stored = providers.providers.iter().find(|provider| {
                        provider.app == app && provider.provider.id == *provider_id
                    });
                    json!({
                        "providerId": provider_id,
                        "providerName": stored
                            .map(|provider| provider.provider.name.clone())
                            .unwrap_or_else(|| provider_id.clone()),
                        "sortIndex": index,
                        "providerNotes": stored.and_then(|provider| {
                            provider_extra_string(&provider.provider, "notes")
                        })
                    })
                })
                .collect::<Vec<_>>();
            Ok(json!(items))
        }
        "deepseek_account_status" => {
            let accounts = state.accounts.read().await;
            let deepseek_accounts = accounts
                .accounts
                .iter()
                .filter(|account| account.provider_type == ProviderType::DeepSeekAccount)
                .collect::<Vec<_>>();
            let default_account_id = deepseek_accounts.first().map(|account| account.id.clone());
            let authenticated = deepseek_accounts
                .iter()
                .any(|account| account_is_authenticated(account));
            let mapped_accounts = deepseek_accounts
                .iter()
                .enumerate()
                .map(|(index, account)| {
                    json!({
                        "id": account.id,
                        "login": account.email.clone().unwrap_or_else(|| account.id.clone()),
                        "authenticated_at": account_authenticated_at(account),
                        "is_default": default_account_id
                            .as_deref()
                            .map(|id| id == account.id)
                            .unwrap_or(index == 0),
                        "has_password": true
                    })
                })
                .collect::<Vec<_>>();
            Ok(json!({
                "authenticated": authenticated,
                "default_account_id": default_account_id,
                "accounts": mapped_accounts
            }))
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
            let default_account_id = matching.first().map(|account| account.id.clone());
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
                "accounts": mapped_accounts
            }))
        }
        "auth_list_accounts" => {
            let provider_type = web_optional_auth_provider_type(&args)?;
            let accounts = state
                .accounts
                .read()
                .await
                .accounts
                .iter()
                .filter(|account| {
                    provider_type
                        .map(|provider_type| account.provider_type == provider_type)
                        .unwrap_or(true)
                })
                .cloned()
                .collect::<Vec<_>>();
            Ok(json!(accounts))
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
            let code = web_optional_string_any(&args, &["code"]);
            let response = finish_account_login(
                State(state.clone()),
                headers.clone(),
                Json(FinishAccountLoginRequest {
                    session_id,
                    state: state_arg,
                    code,
                    execute_token_exchange: Some(true),
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
            web_optional_bool(&args, &["force"]),
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
        "switch_proxy_provider" => {
            let app = web_arg_app_type(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id"])?;
            let exists = state
                .providers
                .read()
                .await
                .providers
                .iter()
                .any(|provider| provider.app == app && provider.provider.id == provider_id);
            if !exists {
                return Err(ApiError::not_found("provider not found"));
            }
            Ok(json!(true))
        }
        "get_proxy_config" => Ok(json!(web_global_proxy_config_json(state))),
        "update_proxy_config" => {
            let listen_address =
                web_optional_string_any(&args, &["listenAddress", "listen_address", "address"]);
            let listen_port = args
                .get("listenPort")
                .or_else(|| args.get("listen_port"))
                .or_else(|| args.get("port"))
                .and_then(|value| value.as_u64().map(|port| port as u16));
            if listen_address.is_some() || listen_port.is_some() {
                let mut patch = serde_json::Map::new();
                let mut proxy_patch = serde_json::Map::new();
                if let Some(address) = listen_address {
                    proxy_patch.insert("listenAddress".to_string(), json!(address));
                }
                if let Some(port) = listen_port {
                    proxy_patch.insert("listenPort".to_string(), json!(port));
                }
                patch.insert("proxyRuntimeConfig".to_string(), Value::Object(proxy_patch));
                state
                    .apply_ui_settings_patch_immediate(Value::Object(patch))
                    .await
                    .map_err(ApiError::internal)?;
            }
            Ok(json!(true))
        }
        "update_proxy_config_for_app" => web_update_proxy_config_for_app(state, &args).await,
        "add_to_failover_queue" => {
            let app = web_arg_app_type(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id"])?;
            let providers = state.providers.read().await.clone();
            let config = state
                .mutate_failover_immediate(|failover| {
                    let mut queue = failover
                        .apps
                        .get(&app)
                        .map(|config| config.provider_queue.clone())
                        .unwrap_or_default();
                    if !queue.iter().any(|id| id == &provider_id) {
                        queue.push(provider_id);
                    }
                    failover.update_app_config(
                        app,
                        UpdateFailoverAppInput {
                            provider_queue: Some(queue),
                            ..Default::default()
                        },
                        &providers,
                    )
                })
                .await
                .map_err(ApiError::internal)?;
            Ok(json!(config.enabled))
        }
        "remove_from_failover_queue" => {
            let app = web_arg_app_type(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id"])?;
            let providers = state.providers.read().await.clone();
            let config = state
                .mutate_failover_immediate(|failover| {
                    let queue = failover
                        .apps
                        .get(&app)
                        .map(|config| {
                            config
                                .provider_queue
                                .iter()
                                .filter(|id| **id != provider_id)
                                .cloned()
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    failover.update_app_config(
                        app,
                        UpdateFailoverAppInput {
                            provider_queue: Some(queue),
                            ..Default::default()
                        },
                        &providers,
                    )
                })
                .await
                .map_err(ApiError::internal)?;
            Ok(json!(config.enabled))
        }
        "set_auto_failover_enabled" => {
            let app = web_arg_app_type(&args)?;
            let enabled = args.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let providers = state.providers.read().await.clone();
            let config = state
                .mutate_failover_immediate(|failover| {
                    failover.update_app_config(
                        app,
                        UpdateFailoverAppInput {
                            enabled: Some(enabled),
                            ..Default::default()
                        },
                        &providers,
                    )
                })
                .await
                .map_err(ApiError::internal)?;
            Ok(json!(config.enabled))
        }
        "get_circuit_breaker_config" => {
            let failover = state.failover.read().await;
            let app = web_arg_app_type(&args).unwrap_or(AppKind::Claude);
            let config = failover.apps.get(&app).cloned().unwrap_or_default();
            Ok(json!({
                "failureThreshold": config.failure_threshold,
                "successThreshold": 2,
                "timeoutSeconds": (config.open_duration_ms / 1000).max(1),
                "errorRateThreshold": 0.5,
                "minRequests": 10,
            }))
        }
        "update_circuit_breaker_config" => {
            let config: Value = web_arg_value(&args, "config")?;
            let app = web_arg_app_type(&args).unwrap_or(AppKind::Claude);
            let providers = state.providers.read().await.clone();
            let failure_threshold = config
                .get("failureThreshold")
                .and_then(Value::as_u64)
                .map(|value| value as u32);
            let timeout_seconds = config.get("timeoutSeconds").and_then(Value::as_u64);
            let updated = state
                .mutate_failover_immediate(|failover| {
                    failover.update_app_config(
                        app,
                        UpdateFailoverAppInput {
                            failure_threshold,
                            open_duration_ms: timeout_seconds
                                .map(|seconds| (seconds * 1000) as u128),
                            ..Default::default()
                        },
                        &providers,
                    )
                })
                .await
                .map_err(ApiError::internal)?;
            Ok(json!({
                "failureThreshold": updated.failure_threshold,
                "successThreshold": 2,
                "timeoutSeconds": (updated.open_duration_ms / 1000).max(1),
                "errorRateThreshold": 0.5,
                "minRequests": 10,
            }))
        }
        "get_circuit_breaker_stats" => {
            let app = web_arg_app_type(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id"])?;
            let failover = state.failover.read().await;
            let key = format!("{}:{}", app.as_str(), provider_id);
            let breaker = failover.breakers.get(&key);
            Ok(json!(web_circuit_breaker_stats_json(breaker)))
        }
        "reset_circuit_breaker" => {
            let app = web_arg_app_type(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id"])?;
            let breaker = state
                .mutate_failover_immediate(|failover| failover.reset_provider(app, &provider_id))
                .await
                .map_err(ApiError::internal)?;
            Ok(json!(web_circuit_breaker_stats_json(Some(&breaker))))
        }
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
        "export_config_to_file" => {
            let bundle = crate::domain::settings::transfer::export_config_bundle(
                &state.config_dir,
                &crate::state::backup_targets(&state.config_dir),
            )
            .map_err(ApiError::internal)?;
            let encoded = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                serde_json::to_vec(&bundle).map_err(ApiError::internal)?,
            );
            let stamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            Ok(json!({
                "success": true,
                "message": "config exported",
                "fileName": format!("cc-switch-server-export-{stamp}.json"),
                "contentBase64": encoded,
            }))
        }
        "import_config_from_file" => {
            if let Some(encoded) =
                web_optional_string_any(&args, &["contentBase64", "content_base64", "fileContent"])
            {
                let backup_id =
                    crate::domain::settings::transfer::import_config_bundle_from_base64(
                        &state.config_dir,
                        &crate::state::backup_targets(&state.config_dir),
                        &encoded,
                    )
                    .map_err(ApiError::bad_request)?;
                state
                    .reload_persistent_stores()
                    .await
                    .map_err(ApiError::internal)?;
                return Ok(json!({
                    "success": true,
                    "message": "config imported",
                    "backupId": backup_id,
                }));
            }
            Err(ApiError::bad_request(
                "import requires contentBase64 payload on server web runtime",
            ))
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
        "get_model_pricing" => {
            let response = list_model_pricing(State(state.clone()), headers.clone())
                .await?
                .0;
            Ok(json!(response.models))
        }
        "update_model_pricing" => {
            let model_id = web_arg_string_any(&args, &["modelId", "model_id"])?;
            let input = UpdateModelPricingInput {
                model_id: Some(model_id.clone()),
                display_name: web_arg_string_any(&args, &["displayName", "display_name"])?,
                input_cost_per_million: web_arg_string_any(&args, &["inputCost", "input_cost"])?,
                output_cost_per_million: web_arg_string_any(&args, &["outputCost", "output_cost"])?,
                cache_read_cost_per_million: web_arg_string_any(
                    &args,
                    &["cacheReadCost", "cache_read_cost"],
                )?,
                cache_creation_cost_per_million: web_arg_string_any(
                    &args,
                    &["cacheCreationCost", "cache_creation_cost"],
                )?,
            };
            let response = upsert_model_pricing(State(state.clone()), headers.clone(), Json(input))
                .await?
                .0;
            Ok(json!(response.model))
        }
        "delete_model_pricing" => {
            let model_id = web_arg_string_any(&args, &["modelId", "model_id"])?;
            let _ =
                delete_model_pricing(State(state.clone()), headers.clone(), Path(model_id)).await?;
            Ok(Value::Null)
        }
        "get_pricing_model_source" => {
            let store = state.ui_settings.read().await;
            let source = store
                .value
                .get("pricingModelSource")
                .cloned()
                .unwrap_or_else(|| {
                    json!({
                        "claude": "response",
                        "codex": "response",
                        "gemini": "response",
                    })
                });
            Ok(source)
        }
        "set_pricing_model_source" => {
            let source: Value = web_arg_value(&args, "source")?;
            state
                .apply_ui_settings_patch_immediate(json!({ "pricingModelSource": source }))
                .await
                .map_err(ApiError::internal)?;
            Ok(json!(true))
        }
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
        "deepseek_account_add" | "deepseek_account_remove" | "deepseek_account_set_default" => {
            Ok(json!(true))
        }
        "deepseek_account_list" => Ok(json!([])),
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
            let response = test_provider_inner(
                state,
                stored,
                &TestProviderQuery {
                    app: None,
                    network: Some(true),
                    timeout_ms: Some(config.timeout_secs.saturating_mul(1000)),
                    model: None,
                    stream: Some(true),
                },
            )
            .await?;
            Ok(json!(map_provider_test_to_stream_check_result(
                &response, &config,
            )))
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
                let response = test_provider_inner(
                    state,
                    stored.clone(),
                    &TestProviderQuery {
                        app: None,
                        network: Some(true),
                        timeout_ms: Some(config.timeout_secs.saturating_mul(1000)),
                        model: None,
                        stream: Some(true),
                    },
                )
                .await
                .unwrap_or_else(|error| {
                    let message = error.message.clone();
                    TestProviderResponse {
                        ok: false,
                        provider_id: stored.provider.id.clone(),
                        app: stored.app,
                        provider_type: stored.provider_type,
                        adapter: "unknown",
                        support: proxy::adapters::AdapterSupport::Planned,
                        endpoint: String::new(),
                        model: String::new(),
                        stream: true,
                        header_names: Vec::new(),
                        network_checked: true,
                        network_status_code: None,
                        network_latency_ms: None,
                        network_stream_completed: None,
                        network_error: Some(message.clone()),
                        message,
                    }
                });
                results.push((
                    stored.provider.id,
                    map_provider_test_to_stream_check_result(&response, &config),
                ));
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
            let pricing = web_optional_deserialize::<BTreeMap<String, u16>>(
                payload,
                "forSaleOfficialPricePercentByApp",
            )?
            .or_else(|| {
                web_optional_deserialize::<BTreeMap<String, u16>>(
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
            let response = update_share_subdomain(
                State(state.clone()),
                headers.clone(),
                Path(share_id),
                Json(UpdateShareSubdomainRequest { subdomain }),
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
        "sync_current_providers_live" => Ok(json!({ "imported": 0, "warnings": [] })),
        "sync_session_usage" => Ok(Value::Null),
        "get_usage_data_sources" => Ok(json!(["server"])),
        "check_provider_limits" => Ok(json!({ "ok": true, "withinLimits": true })),
        "get_subscription_quota" => {
            let tool = web_arg_string_any(&args, &["tool"])?;
            Ok(web_subscription_quota(state, headers, &tool).await?)
        }
        "get_default_cost_multiplier" => Ok(json!(1.0)),
        "set_default_cost_multiplier" => Ok(Value::Null),
        "read_live_provider_settings" => Ok(json!({})),
        "test_api_endpoints" => Ok(json!([])),
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
        "codex_banked_reset_status" => {
            let account_id = web_optional_string_any(&args, &["accountId", "account_id"]);
            let account_id = {
                let accounts = state.accounts_snapshot().await;
                let account = accounts
                    .find_for_provider(ProviderType::CodexOAuth, account_id.as_deref())
                    .filter(|account| account.provider_type == ProviderType::CodexOAuth)
                    .ok_or_else(|| ApiError::not_found("codex oauth account not found"))?;
                account.id.clone()
            };
            let response = account_quota(
                State(state.clone()),
                headers.clone(),
                Path(account_id),
                Query(AccountQuotaQuery {
                    refresh: Some(true),
                    force: web_optional_bool(&args, &["force"]),
                }),
            )
            .await?
            .0;
            let account = response
                .account
                .as_ref()
                .ok_or_else(|| ApiError::not_found("codex oauth account not found"))?;
            Ok(
                crate::clients::oauth::quota::codex_banked_reset_status_snapshot(
                    account,
                    crate::infra::time::now_ms() as i64,
                ),
            )
        }
        "codex_banked_reset_invite" | "codex_banked_reset_consume" => Err(
            ApiError::not_implemented("codex banked reset is not available on cc-switch-server"),
        ),
        "open_provider_terminal" => Err(ApiError::not_implemented(
            "open_provider_terminal is not available in server web runtime",
        )),
        _ => Err(ApiError::web_invoke_not_wired(format!(
            "desktop invoke command '{command}' is registered but has no dispatcher"
        ))),
    }
}

fn grok_oauth_default_models() -> Vec<Value> {
    [
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
