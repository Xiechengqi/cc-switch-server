use super::*;

pub(in crate::api) async fn list_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListProvidersQuery>,
) -> Result<Json<ListProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.list(query.app);
    Ok(Json(ListProvidersResponse {
        ok: true,
        providers,
    }))
}

pub(in crate::api) async fn create_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CreateProviderRequest>,
) -> Result<Json<CreateProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    if input.provider.name.trim().is_empty() {
        return Err(ApiError::bad_request("provider name is required"));
    }

    let stored = {
        let mut store = state.providers.write().await;
        store.upsert(input.app, input.provider)
    };
    state.save_providers().await.map_err(ApiError::internal)?;

    Ok(Json(CreateProviderResponse { ok: true, stored }))
}

pub(in crate::api) async fn export_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListProvidersQuery>,
) -> Result<Json<ExportProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.list(query.app);
    Ok(Json(ExportProvidersResponse {
        ok: true,
        providers,
    }))
}

pub(in crate::api) async fn import_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ImportProvidersRequest>,
) -> Result<Json<ImportProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    for item in &input.providers {
        if item.provider.name.trim().is_empty() {
            return Err(ApiError::bad_request("provider name is required"));
        }
    }
    let imported = {
        let mut store = state.providers.write().await;
        input
            .providers
            .into_iter()
            .map(|item| {
                store.upsert(item.app, item.provider);
                1usize
            })
            .sum()
    };
    state.save_providers().await.map_err(ApiError::internal)?;
    Ok(Json(ImportProvidersResponse { ok: true, imported }))
}

pub(in crate::api) async fn list_universal_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListUniversalProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.universal_providers.read().await.providers.clone();
    Ok(Json(ListUniversalProvidersResponse {
        ok: true,
        providers,
    }))
}

pub(in crate::api) async fn universal_provider_presets_route(
) -> Json<UniversalProviderPresetsResponse> {
    Json(UniversalProviderPresetsResponse {
        ok: true,
        presets: universal_provider_presets(),
    })
}

pub(in crate::api) async fn export_universal_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ExportUniversalProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state
        .universal_providers
        .read()
        .await
        .providers
        .values()
        .cloned()
        .collect();
    Ok(Json(ExportUniversalProvidersResponse {
        ok: true,
        providers,
    }))
}

pub(in crate::api) async fn import_universal_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<ImportUniversalProvidersRequest>,
) -> Result<Json<ImportUniversalProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    for provider in &mut input.providers {
        if provider.id.trim().is_empty() {
            provider.id = format!("universal-{}", &generate_session_token()[..16]);
        }
        if provider.name.trim().is_empty() {
            return Err(ApiError::bad_request("universal provider name is required"));
        }
        if provider.base_url.trim().is_empty() {
            return Err(ApiError::bad_request(
                "universal provider baseUrl is required",
            ));
        }
    }
    let imported = {
        let mut store = state.universal_providers.write().await;
        input
            .providers
            .into_iter()
            .map(|provider| {
                store.upsert(provider);
                1usize
            })
            .sum()
    };
    state
        .save_universal_providers()
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(ImportUniversalProvidersResponse {
        ok: true,
        imported,
    }))
}

pub(in crate::api) async fn get_universal_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<GetUniversalProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let provider = state
        .universal_providers
        .read()
        .await
        .providers
        .get(&id)
        .cloned();
    Ok(Json(GetUniversalProviderResponse { ok: true, provider }))
}

pub(in crate::api) async fn upsert_universal_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<UpsertUniversalProviderRequest>,
) -> Result<Json<UpsertUniversalProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    if input.provider.id.trim().is_empty() {
        input.provider.id = format!("universal-{}", &generate_session_token()[..16]);
    }
    if input.provider.name.trim().is_empty() {
        return Err(ApiError::bad_request("universal provider name is required"));
    }
    if input.provider.base_url.trim().is_empty() {
        return Err(ApiError::bad_request(
            "universal provider baseUrl is required",
        ));
    }

    let provider = {
        let mut store = state.universal_providers.write().await;
        store.upsert(input.provider)
    };
    state
        .save_universal_providers()
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(UpsertUniversalProviderResponse { ok: true, provider }))
}

pub(in crate::api) async fn delete_universal_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<DeleteResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let deleted = state.universal_providers.write().await.delete(&id);
    state
        .save_universal_providers()
        .await
        .map_err(ApiError::internal)?;
    if deleted {
        state
            .providers
            .write()
            .await
            .remove_universal_derivatives(&id);
        state.save_providers().await.map_err(ApiError::internal)?;
    }
    Ok(Json(DeleteResponse { ok: true, deleted }))
}

pub(in crate::api) async fn sync_universal_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<SyncUniversalProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let universal = state
        .universal_providers
        .read()
        .await
        .providers
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("universal provider not found"))?;

    let mut result = UniversalProviderSyncResult::default();
    {
        let mut providers = state.providers.write().await;
        for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
            if let Some(provider) = provider_from_universal(&universal, app) {
                providers.upsert_merging_settings(app, provider);
                result.synced.push(app.as_str().to_string());
            } else {
                if providers.remove_universal_derivative(&universal.id, app) {
                    result.removed.push(app.as_str().to_string());
                }
                result.skipped.push(app.as_str().to_string());
            }
        }
    }
    state.save_providers().await.map_err(ApiError::internal)?;

    Ok(Json(SyncUniversalProviderResponse { ok: true, result }))
}

pub(in crate::api) async fn provider_health(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListProvidersQuery>,
) -> Result<Json<ProviderHealthResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.list(query.app);
    let usage = state.usage.read().await;
    Ok(Json(ProviderHealthResponse {
        ok: true,
        providers: crate::domain::health::provider_health_list(&providers, &usage),
    }))
}

pub(in crate::api) async fn failover_snapshot(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<FailoverResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let failover = state.failover.read().await;
    Ok(Json(FailoverResponse {
        ok: true,
        failover: failover.snapshot_for_providers(&providers),
    }))
}

pub(in crate::api) async fn update_failover_app(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(app): Path<AppKind>,
    Json(input): Json<UpdateFailoverAppInput>,
) -> Result<Json<UpdateFailoverAppResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let config = {
        let mut failover = state.failover.write().await;
        failover.update_app_config(app, input, &providers)
    };
    state.save_failover().await.map_err(ApiError::internal)?;
    Ok(Json(UpdateFailoverAppResponse {
        ok: true,
        app,
        config,
    }))
}

pub(in crate::api) async fn reset_failover_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
    Query(query): Query<FailoverProviderResetQuery>,
) -> Result<Json<ResetFailoverProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let app = resolve_failover_provider_app(&state, &provider_id, query.app).await?;
    let breaker = {
        let mut failover = state.failover.write().await;
        failover.reset_provider(app, &provider_id)
    };
    state.save_failover().await.map_err(ApiError::internal)?;
    Ok(Json(ResetFailoverProviderResponse { ok: true, breaker }))
}

pub(in crate::api) async fn resolve_failover_provider_app(
    state: &ServerState,
    provider_id: &str,
    requested_app: Option<AppKind>,
) -> Result<AppKind, ApiError> {
    let providers = state.providers.read().await;
    if let Some(app) = requested_app {
        if providers
            .providers
            .iter()
            .any(|provider| provider.app == app && provider.provider.id == provider_id)
        {
            return Ok(app);
        }
        return Err(ApiError::not_found("provider not found for app"));
    }

    let matches = providers
        .providers
        .iter()
        .filter(|provider| provider.provider.id == provider_id)
        .map(|provider| provider.app)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [app] => Ok(*app),
        [] => Err(ApiError::not_found("provider not found")),
        _ => Err(ApiError::bad_request(
            "provider id is used by multiple apps; specify app query",
        )),
    }
}

pub(in crate::api) async fn test_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<TestProviderQuery>,
) -> Result<Json<TestProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let stored = resolve_provider_by_id(&state, &id, query.app).await?;
    Ok(Json(test_provider_inner(&state, stored, &query).await?))
}

pub(in crate::api) async fn fetch_provider_models(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<FetchProviderModelsRequest>,
) -> Result<Json<FetchProviderModelsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let stored = resolve_provider_by_id(&state, &id, input.app).await?;
    let fetched = fetch_provider_models_inner(&state, &stored, input.timeout_ms).await?;
    let mut provider = None;
    let mut merged_count = 0usize;
    if input.merge.unwrap_or(false) {
        {
            let mut providers = state.providers.write().await;
            let item = providers
                .providers
                .iter_mut()
                .find(|item| item.app == stored.app && item.provider.id == stored.provider.id)
                .ok_or_else(|| ApiError::not_found("provider not found"))?;
            merged_count = merge_fetched_models_into_provider(item, &fetched.models);
            provider = Some(item.clone());
        }
        state.save_providers().await.map_err(ApiError::internal)?;
    }
    Ok(Json(FetchProviderModelsResponse {
        ok: true,
        provider_id: stored.provider.id,
        app: stored.app,
        provider_type: stored.provider_type,
        url: fetched.url,
        merged: input.merge.unwrap_or(false),
        merged_count,
        models: fetched.models,
        provider,
    }))
}

pub(in crate::api) async fn test_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<TestProvidersRequest>,
) -> Result<Json<TestProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let query = TestProviderQuery {
        app: None,
        network: input.network,
        timeout_ms: input.timeout_ms,
        model: input.model,
        stream: input.stream,
    };
    let providers = state.providers.read().await.providers.clone();
    let selected = providers
        .into_iter()
        .filter(|item| input.app.is_none_or(|app| item.app == app))
        .filter(|item| {
            input
                .provider_ids
                .as_ref()
                .is_none_or(|ids| ids.iter().any(|id| id == &item.provider.id))
        })
        .collect::<Vec<_>>();
    let mut results = Vec::new();
    for stored in selected {
        results.push(test_provider_inner(&state, stored, &query).await?);
    }
    Ok(Json(TestProvidersResponse { ok: true, results }))
}

pub(in crate::api) async fn test_provider_inner(
    state: &ServerState,
    stored: StoredProvider,
    query: &TestProviderQuery,
) -> Result<TestProviderResponse, ApiError> {
    let accounts = state.accounts.read().await.clone();
    let adapter = proxy::adapters::adapter_for(stored.app, stored.provider_type);
    let route = default_test_route(stored.app);
    let stream = query.stream.unwrap_or(false);
    let model = provider_test_model(stored.app, &stored, query.model.as_deref());
    let endpoint = adapter
        .resolve_endpoint(
            route,
            default_gemini_test_path(stored.app, &model, stream),
            &stored,
        )
        .map_err(ApiError::proxy)?;
    let target_headers = adapter
        .build_headers(stored.app, &stored, &accounts)
        .map_err(ApiError::proxy)?;
    let capability = adapter.capability(stored.app, stored.provider_type);
    let mut network_status_code = None;
    let mut network_latency_ms = None;
    let mut network_error = None;
    let mut network_stream_completed = None;
    if query.network.unwrap_or(false) {
        let started = std::time::Instant::now();
        let body = provider_test_body(stored.app, &stored, Some(&model), stream);
        let http_client = state.http_client().await;
        let mut request = http_client
            .post(&endpoint)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(body);
        if stream {
            request = request.header(axum::http::header::ACCEPT, "text/event-stream");
        }
        for (name, value) in &target_headers {
            request = request.header(*name, value);
        }
        match request
            .timeout(provider_test_timeout(query.timeout_ms))
            .send()
            .await
        {
            Ok(response) => {
                network_status_code = Some(response.status().as_u16());
                network_latency_ms = Some(started.elapsed().as_millis());
                if !response.status().is_success() {
                    let body = response.text().await.unwrap_or_default();
                    network_error = Some(redact_provider_test_error(&body));
                } else if stream {
                    let body = response.text().await.unwrap_or_default();
                    let completed = provider_test_stream_completed(stored.app, &body);
                    network_stream_completed = Some(completed);
                    if !completed {
                        network_error = Some(
                            "stream probe did not observe a provider completion marker".to_string(),
                        );
                    }
                }
            }
            Err(error) => {
                network_error = Some(error.to_string());
            }
        }
    }

    Ok(TestProviderResponse {
        ok: true,
        provider_id: stored.provider.id,
        app: stored.app,
        provider_type: stored.provider_type,
        adapter: capability.adapter,
        support: capability.support,
        endpoint,
        model,
        stream,
        header_names: target_headers
            .into_iter()
            .map(|(name, _)| name.to_string())
            .collect(),
        network_checked: query.network.unwrap_or(false),
        network_status_code,
        network_latency_ms,
        network_stream_completed,
        network_error,
        message: if query.network.unwrap_or(false) {
            "configuration check passed; upstream network/model call executed".to_string()
        } else {
            "configuration check passed; upstream network/model call is not executed".to_string()
        },
    })
}

pub(in crate::api) async fn resolve_provider_by_id(
    state: &ServerState,
    provider_id: &str,
    app: Option<AppKind>,
) -> Result<StoredProvider, ApiError> {
    let matches = state
        .providers
        .read()
        .await
        .providers
        .iter()
        .filter(|item| item.provider.id == provider_id && app.is_none_or(|app| item.app == app))
        .cloned()
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [stored] => Ok(stored.clone()),
        [] => Err(ApiError::not_found("provider not found")),
        _ => Err(ApiError::bad_request(
            "provider id is used by multiple apps; pass app in the request body",
        )),
    }
}

pub(in crate::api) struct ProviderModelsFetchResult {
    url: String,
    models: Vec<FetchedProviderModel>,
}

pub(in crate::api) async fn fetch_provider_models_inner(
    state: &ServerState,
    stored: &StoredProvider,
    timeout_ms: Option<u64>,
) -> Result<ProviderModelsFetchResult, ApiError> {
    let accounts = state.accounts.read().await.clone();
    let adapter = proxy::adapters::adapter_for(stored.app, stored.provider_type);
    let model = provider_test_model(stored.app, stored, None);
    let endpoint = adapter
        .resolve_endpoint(
            default_test_route(stored.app),
            default_gemini_test_path(stored.app, &model, false),
            stored,
        )
        .map_err(ApiError::proxy)?;
    let url = model_list_url_from_endpoint(&endpoint).ok_or_else(|| {
        ApiError::bad_request("provider endpoint cannot be mapped to a model list URL")
    })?;
    let target_headers = adapter
        .build_headers(stored.app, stored, &accounts)
        .map_err(ApiError::proxy)?;
    let http_client = state.http_client().await;
    let mut request = http_client.get(&url);
    for (name, value) in target_headers {
        request = request.header(name, value);
    }
    let response = request
        .timeout(provider_test_timeout(timeout_ms))
        .send()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("fetch provider models failed: {error}")))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::bad_gateway(format!(
            "fetch provider models failed: {status}: {}",
            redact_provider_test_error(&body)
        )));
    }
    let raw = response
        .json::<Value>()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("parse provider models failed: {error}")))?;
    Ok(ProviderModelsFetchResult {
        url,
        models: parse_provider_models(&raw),
    })
}

pub(in crate::api) fn model_list_url_from_endpoint(endpoint: &str) -> Option<String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return None;
    }
    if let Some(index) = endpoint.find("/models/") {
        return Some(format!("{}/models", &endpoint[..index]));
    }
    for suffix in [
        "/chat/completions",
        "/responses",
        "/messages",
        "/completions",
    ] {
        if let Some(index) = endpoint.rfind(suffix) {
            return Some(format!("{}/models", &endpoint[..index]));
        }
    }
    endpoint.ends_with("/models").then(|| endpoint.to_string())
}

pub(in crate::api) fn parse_provider_models(raw: &Value) -> Vec<FetchedProviderModel> {
    let models = raw
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| raw.get("models").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();
    models
        .into_iter()
        .filter_map(|model| {
            let upstream_model = model
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| model.get("name").and_then(Value::as_str))?
                .trim()
                .to_string();
            if upstream_model.is_empty() {
                return None;
            }
            let id = upstream_model
                .strip_prefix("models/")
                .unwrap_or(&upstream_model)
                .to_string();
            let display_name = model
                .get("displayName")
                .or_else(|| model.get("display_name"))
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(FetchedProviderModel {
                id,
                upstream_model,
                display_name,
                raw: model,
            })
        })
        .collect()
}

pub(in crate::api) fn merge_fetched_models_into_provider(
    stored: &mut StoredProvider,
    models: &[FetchedProviderModel],
) -> usize {
    if !stored.provider.settings_config.is_object() {
        stored.provider.settings_config = json!({});
    }
    let settings = stored
        .provider
        .settings_config
        .as_object_mut()
        .expect("settings_config object");
    let catalog = settings
        .entry("modelCatalog".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !catalog.is_object() {
        *catalog = Value::Object(Map::new());
    }
    let catalog = catalog.as_object_mut().expect("modelCatalog object");
    let mut merged = 0usize;
    for model in models {
        if catalog.contains_key(&model.id) {
            continue;
        }
        catalog.insert(
            model.id.clone(),
            json!({
                "upstreamModel": model.upstream_model.clone(),
                "displayName": model.display_name.clone(),
            }),
        );
        merged += 1;
    }
    merged
}

pub(in crate::api) async fn create_provider_from_preset(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CreateProviderFromPresetRequest>,
) -> Result<Json<CreateProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let fixtures = fixtures_for_app(&state.provider_coverage, input.app);
    let fixture = fixtures
        .into_iter()
        .find(|item| item.name == input.name)
        .ok_or_else(|| ApiError::not_found("provider preset not found"))?;
    let stored = {
        let mut store = state.providers.write().await;
        store.upsert(input.app, fixture.provider.clone())
    };
    state.save_providers().await.map_err(ApiError::internal)?;
    Ok(Json(CreateProviderResponse { ok: true, stored }))
}

pub(in crate::api) async fn provider_presets(
    State(state): State<ServerState>,
    Query(query): Query<ProviderPresetsQuery>,
) -> Json<ProviderPresetsResponse> {
    let presets = match query.app {
        Some(AppKind::Claude) => state.provider_coverage.presets.claude.clone(),
        Some(AppKind::Codex) => state.provider_coverage.presets.codex.clone(),
        Some(AppKind::Gemini) => state.provider_coverage.presets.gemini.clone(),
        None => Vec::new(),
    };
    Json(ProviderPresetsResponse { ok: true, presets })
}

pub(in crate::api) fn default_test_route(app: AppKind) -> ProxyRoute {
    match app {
        AppKind::Claude => ProxyRoute::ClaudeMessages,
        AppKind::Codex => ProxyRoute::CodexResponses,
        AppKind::Gemini => ProxyRoute::Gemini,
    }
}

pub(in crate::api) fn default_gemini_test_path(
    app: AppKind,
    model: &str,
    stream: bool,
) -> Option<String> {
    (app == AppKind::Gemini).then(|| {
        let method = if stream {
            "streamGenerateContent"
        } else {
            "generateContent"
        };
        format!("{}:{method}", gemini_model_name(model))
    })
}

pub(in crate::api) fn provider_test_model(
    app: AppKind,
    stored: &StoredProvider,
    override_model: Option<&str>,
) -> String {
    override_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .or_else(|| {
            stored
                .provider
                .settings_config
                .pointer("/testConfig/testModel")
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            stored
                .provider
                .settings_config
                .pointer("/testConfig/model")
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            stored
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.test_config.as_ref())
                .and_then(|value| value.get("testModel").or_else(|| value.get("model")))
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            stored
                .provider
                .settings_config
                .pointer("/modelMapping/upstreamModel")
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            stored
                .provider
                .settings_config
                .get("models")
                .and_then(serde_json::Value::as_array)
                .and_then(|items| items.first())
                .and_then(|value| {
                    value.as_str().or_else(|| {
                        value
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .or_else(|| value.get("name").and_then(serde_json::Value::as_str))
                    })
                })
        })
        .unwrap_or(match app {
            AppKind::Claude => "claude-3-5-haiku-latest",
            AppKind::Codex => "gpt-4.1-mini",
            AppKind::Gemini => "gemini-2.5-flash",
        })
        .to_string()
}

pub(in crate::api) fn provider_test_body(
    app: AppKind,
    stored: &StoredProvider,
    override_model: Option<&str>,
    stream: bool,
) -> String {
    let model = provider_test_model(app, stored, override_model);
    let value = match app {
        AppKind::Claude => serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "ping"}],
            "stream": stream
        }),
        AppKind::Codex => serde_json::json!({
            "model": model,
            "input": "ping",
            "max_output_tokens": 1,
            "stream": stream
        }),
        AppKind::Gemini => serde_json::json!({
            "contents": [{"role": "user", "parts": [{"text": "ping"}]}],
            "generationConfig": {"maxOutputTokens": 1}
        }),
    };
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
}

pub(in crate::api) fn provider_test_stream_completed(app: AppKind, body: &str) -> bool {
    match app {
        AppKind::Claude => body.contains("message_stop") || body.contains("[DONE]"),
        AppKind::Codex => {
            body.contains("response.completed")
                || body.contains("\"status\":\"completed\"")
                || body.contains("[DONE]")
        }
        AppKind::Gemini => body.contains("finishReason") || body.contains("\"candidates\""),
    }
}

pub(in crate::api) fn redact_provider_test_error(value: &str) -> String {
    let mut redacted = value.to_string();
    for marker in ["sk-", "ya29.", "Bearer "] {
        while let Some(index) = redacted.find(marker) {
            let end = redacted[index..]
                .find(|ch: char| ch.is_whitespace() || ch == '"' || ch == '\'')
                .map(|offset| index + offset)
                .unwrap_or_else(|| redacted.len());
            redacted.replace_range(index..end, "[REDACTED]");
        }
    }
    redacted.chars().take(800).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::domain::providers::model::{AppKind, Provider, ProviderType};

    #[test]
    fn provider_test_body_prefers_test_config_model() {
        let stored = StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "p1".to_string(),
                name: "provider".to_string(),
                settings_config: json!({
                    "testConfig": {"model": "test-model"},
                    "modelMapping": {"upstreamModel": "mapped-model"}
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::Codex,
            provider_type_id: "codex".to_string(),
        };

        let body = provider_test_body(AppKind::Codex, &stored, None, false);
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert_eq!(
            value.get("model").and_then(serde_json::Value::as_str),
            Some("test-model")
        );
        assert_eq!(
            value.get("stream").and_then(serde_json::Value::as_bool),
            Some(false)
        );

        let stream_body = provider_test_body(AppKind::Codex, &stored, Some("override-model"), true);
        let stream_value: serde_json::Value = serde_json::from_str(&stream_body).unwrap();
        assert_eq!(
            stream_value
                .get("model")
                .and_then(serde_json::Value::as_str),
            Some("override-model")
        );
        assert_eq!(
            stream_value
                .get("stream")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn provider_test_error_redaction_removes_common_secret_shapes() {
        let redacted = redact_provider_test_error(
            r#"{"error":"bad sk-abc1234567890 and ya29.secret-token and Bearer abc.def"}"#,
        );

        assert!(!redacted.contains("sk-abc"));
        assert!(!redacted.contains("ya29.secret"));
        assert!(!redacted.contains("Bearer abc"));
        assert!(redacted.contains("[REDACTED]"));
    }
}
