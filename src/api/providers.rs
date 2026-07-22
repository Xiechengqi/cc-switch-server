use super::*;

pub(in crate::api) async fn list_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListProvidersQuery>,
) -> Result<Json<ListProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.provider_views(query.app).await;
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
    require_provider_write_contract(&state, &headers)?;
    let app = input.app;
    let stored = state
        .upsert_provider_draft_command(crate::domain::providers::credentials::ProviderWriteDraft {
            app,
            provider: input.provider,
            profile_id: input.profile_id,
            custom_binding: input.custom_binding,
            expected_revision: None,
            client_request_id: input.client_request_id,
            credential_patches: input.credential_patches,
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(map_provider_command_error)?;

    Ok(Json(CreateProviderResponse {
        ok: true,
        stored: crate::domain::providers::credentials::ProviderView::from_stored(&stored),
    }))
}

pub(in crate::api) async fn get_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ProviderResourceQuery>,
) -> Result<Json<crate::domain::providers::credentials::ProviderView>, ApiError> {
    require_session(&state, &headers).await?;
    let stored = resolve_provider_by_key(&state, query.app, &id).await?;
    Ok(Json(
        crate::domain::providers::credentials::ProviderView::from_stored(&stored),
    ))
}

pub(in crate::api) async fn update_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ProviderResourceQuery>,
    Json(mut input): Json<UpdateProviderRequest>,
) -> Result<Json<CreateProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    require_provider_write_contract(&state, &headers)?;
    if input.provider.id.trim().is_empty() {
        input.provider.id = id.clone();
    } else if input.provider.id != id {
        return Err(ApiError::bad_request(
            "provider id in body must match resource path",
        ));
    }
    resolve_provider_by_key(&state, query.app, &id).await?;
    let stored = state
        .upsert_provider_draft_command(crate::domain::providers::credentials::ProviderWriteDraft {
            app: query.app,
            provider: input.provider,
            profile_id: input.profile_id,
            custom_binding: input.custom_binding,
            expected_revision: Some(input.expected_revision),
            client_request_id: None,
            credential_patches: input.credential_patches,
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(map_provider_command_error)?;
    Ok(Json(CreateProviderResponse {
        ok: true,
        stored: crate::domain::providers::credentials::ProviderView::from_stored(&stored),
    }))
}

pub(in crate::api) async fn delete_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<DeleteProviderQuery>,
) -> Result<Json<DeleteProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    require_provider_write_contract(&state, &headers)?;
    let deleted = state
        .delete_provider_command(query.app, id, query.expected_revision)
        .await
        .map_err(ApiError::internal)?
        .map_err(map_provider_command_error)?;
    Ok(Json(DeleteProviderResponse { ok: true, deleted }))
}

pub(in crate::api) async fn provider_delete_preview(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ProviderResourceQuery>,
) -> Result<Json<ProviderDeletePreviewResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let preview = state
        .provider_reference_preview(query.app, &id)
        .await
        .map_err(map_provider_command_error)?;
    Ok(Json(ProviderDeletePreviewResponse { ok: true, preview }))
}

pub(in crate::api) async fn export_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListProvidersQuery>,
) -> Result<Json<ExportProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.provider_views(query.app).await;
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
    require_provider_write_contract(&state, &headers)?;
    let providers = input
        .providers
        .into_iter()
        .map(
            |item| crate::domain::providers::credentials::ProviderWriteDraft {
                app: item.app,
                provider: item.provider,
                profile_id: item.profile_id,
                custom_binding: item.custom_binding,
                expected_revision: item.expected_revision,
                client_request_id: item.client_request_id,
                credential_patches: item.credential_patches,
            },
        )
        .collect::<Vec<_>>();
    let mode = input.mode;
    let preview = match mode {
        ProviderImportMode::Preview => state
            .preview_provider_import_command(providers)
            .await
            .map_err(ApiError::internal)?
            .map_err(map_provider_command_error)?,
        ProviderImportMode::Apply => {
            let preview_token = input.preview_token.ok_or_else(|| {
                ApiError::bad_request("previewToken is required when applying a Provider import")
            })?;
            state
                .apply_provider_import_command(providers, preview_token)
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?
        }
    };
    let imported = match mode {
        ProviderImportMode::Preview => 0,
        ProviderImportMode::Apply => preview.create_count + preview.update_count,
    };
    Ok(Json(ImportProvidersResponse {
        ok: true,
        mode,
        preview,
        imported,
    }))
}

pub(in crate::api) async fn adopt_provider_profile(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ProviderResourceQuery>,
    Json(input): Json<AdoptProviderProfileRequest>,
) -> Result<Json<ProviderIdentityActionResponse>, ApiError> {
    require_session(&state, &headers).await?;
    require_provider_write_contract(&state, &headers)?;
    let mode = input.mode;
    let (preview, stored) = match mode {
        ProviderActionMode::Preview => (
            state
                .preview_adopt_provider_profile_command(
                    query.app,
                    &id,
                    input.expected_revision,
                    input.profile_id,
                    input.account_id,
                )
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?,
            None,
        ),
        ProviderActionMode::Apply => {
            let token = required_provider_action_preview_token(input.preview_token)?;
            let (preview, stored) = state
                .apply_adopt_provider_profile_command(
                    query.app,
                    id,
                    input.expected_revision,
                    input.profile_id,
                    input.account_id,
                    token,
                )
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?;
            (
                preview,
                Some(crate::domain::providers::credentials::ProviderView::from_stored(&stored)),
            )
        }
    };
    Ok(Json(ProviderIdentityActionResponse {
        ok: true,
        mode,
        preview,
        stored,
    }))
}

pub(in crate::api) async fn rebind_custom_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ProviderResourceQuery>,
    Json(input): Json<RebindCustomProviderRequest>,
) -> Result<Json<ProviderIdentityActionResponse>, ApiError> {
    require_session(&state, &headers).await?;
    require_provider_write_contract(&state, &headers)?;
    let mode = input.mode;
    let (preview, stored) = match mode {
        ProviderActionMode::Preview => (
            state
                .preview_rebind_custom_provider_command(
                    query.app,
                    &id,
                    input.expected_revision,
                    input.custom_binding,
                    input.credential_patches,
                )
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?,
            None,
        ),
        ProviderActionMode::Apply => {
            let token = required_provider_action_preview_token(input.preview_token)?;
            let (preview, stored) = state
                .apply_rebind_custom_provider_command(
                    query.app,
                    id,
                    input.expected_revision,
                    input.custom_binding,
                    input.credential_patches,
                    token,
                )
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?;
            (
                preview,
                Some(crate::domain::providers::credentials::ProviderView::from_stored(&stored)),
            )
        }
    };
    Ok(Json(ProviderIdentityActionResponse {
        ok: true,
        mode,
        preview,
        stored,
    }))
}

pub(in crate::api) async fn clone_provider_as_custom(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ProviderResourceQuery>,
    Json(input): Json<CloneProviderAsCustomRequest>,
) -> Result<Json<ProviderIdentityActionResponse>, ApiError> {
    require_session(&state, &headers).await?;
    require_provider_write_contract(&state, &headers)?;
    let mode = input.mode;
    let (preview, stored) = match mode {
        ProviderActionMode::Preview => (
            state
                .preview_clone_provider_as_custom_command(
                    query.app,
                    &id,
                    input.expected_revision,
                    input.target_provider_id,
                    input.target_name,
                    input.custom_binding,
                    input.client_request_id,
                )
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?,
            None,
        ),
        ProviderActionMode::Apply => {
            let token = required_provider_action_preview_token(input.preview_token)?;
            let (preview, stored) = state
                .apply_clone_provider_as_custom_command(
                    query.app,
                    id,
                    input.expected_revision,
                    input.target_provider_id,
                    input.target_name,
                    input.custom_binding,
                    input.client_request_id,
                    token,
                )
                .await
                .map_err(ApiError::internal)?
                .map_err(map_provider_command_error)?;
            (
                preview,
                Some(crate::domain::providers::credentials::ProviderView::from_stored(&stored)),
            )
        }
    };
    Ok(Json(ProviderIdentityActionResponse {
        ok: true,
        mode,
        preview,
        stored,
    }))
}

pub(in crate::api) async fn preview_provider_account_binding_migration(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ProviderAccountBindingMigrationResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let preview = state
        .preview_provider_account_binding_migration_command()
        .await
        .map_err(ApiError::internal)?
        .map_err(map_provider_command_error)?;
    Ok(Json(ProviderAccountBindingMigrationResponse {
        ok: true,
        preview,
        applied: 0,
    }))
}

pub(in crate::api) async fn apply_provider_account_binding_migration(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ApplyProviderAccountBindingMigrationRequest>,
) -> Result<Json<ProviderAccountBindingMigrationResponse>, ApiError> {
    require_session(&state, &headers).await?;
    require_provider_write_contract(&state, &headers)?;
    let (preview, applied) = state
        .apply_provider_account_binding_migration_command(input.preview_token)
        .await
        .map_err(ApiError::internal)?
        .map_err(map_provider_command_error)?;
    Ok(Json(ProviderAccountBindingMigrationResponse {
        ok: true,
        preview,
        applied,
    }))
}

fn required_provider_action_preview_token(token: Option<String>) -> Result<String, ApiError> {
    token
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request("previewToken is required when applying this action"))
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

pub(in crate::api) async fn provider_storage_migration(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<
    Json<crate::domain::providers::storage_migration::ProviderStorageMigrationReport>,
    ApiError,
> {
    require_session(&state, &headers).await?;
    let config_dir = state.config_dir.clone();
    let report = tokio::task::spawn_blocking(move || {
        crate::domain::providers::storage_migration::preflight(&config_dir)
    })
    .await
    .map_err(|error| ApiError::internal(format!("Provider migration preflight panicked: {error}")))?
    .map_err(ApiError::internal)?;
    Ok(Json(report))
}

pub(in crate::api) async fn test_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<TestProviderQuery>,
) -> Result<Json<TestProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let execution = resolve_provider_execution_by_key(&state, query.app, &id).await?;
    Ok(Json(test_provider_inner(&state, execution, &query).await?))
}

pub(in crate::api) async fn fetch_provider_models(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<FetchProviderModelsRequest>,
) -> Result<Json<FetchProviderModelsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let execution = resolve_provider_execution_by_key(&state, input.app, &id).await?;
    if let Err(error) =
        execution.ensure_operation_supported(proxy::provider_ops::ProviderOperation::Discovery)
    {
        if error.status == axum::http::StatusCode::NOT_IMPLEMENTED {
            return Ok(Json(FetchProviderModelsResponse {
                ok: false,
                outcome: ProviderOperationOutcome::Unsupported,
                driver_id: execution.plan.driver_id.to_string(),
                runtime_fingerprint: execution.plan.runtime_fingerprint.clone(),
                message: Some(error.message),
                provider_id: execution.stored.provider.id.clone(),
                app: execution.stored.app,
                provider_type: execution.stored.provider_type,
                provider_revision: execution.stored.resource.revision,
                url: String::new(),
                merged: false,
                merged_count: 0,
                models: Vec::new(),
                provider: None,
            }));
        }
        return Err(ApiError::proxy(error));
    }
    if let Some((outcome, message)) = provider_configuration_outcome(&execution) {
        return Ok(Json(FetchProviderModelsResponse {
            ok: false,
            outcome,
            driver_id: execution.plan.driver_id.to_string(),
            runtime_fingerprint: execution.plan.runtime_fingerprint.clone(),
            message: Some(message),
            provider_id: execution.stored.provider.id.clone(),
            app: execution.stored.app,
            provider_type: execution.stored.provider_type,
            provider_revision: execution.stored.resource.revision,
            url: redact_provider_endpoint(&execution.plan.endpoint),
            merged: false,
            merged_count: 0,
            models: Vec::new(),
            provider: None,
        }));
    }
    let fetched = fetch_provider_models_inner(&state, &execution, input.timeout_ms).await?;
    if input.merge.unwrap_or(false) {
        return Err(ApiError::bad_request(
            "automatic model discovery merge is retired; select a discovered model and save the Provider explicitly",
        ));
    }
    Ok(Json(FetchProviderModelsResponse {
        ok: true,
        outcome: ProviderOperationOutcome::Success,
        driver_id: execution.plan.driver_id.to_string(),
        runtime_fingerprint: execution.plan.runtime_fingerprint.clone(),
        message: None,
        provider_id: execution.stored.provider.id.clone(),
        app: execution.stored.app,
        provider_type: execution.stored.provider_type,
        provider_revision: execution.stored.resource.revision,
        url: redact_provider_endpoint(&fetched.url),
        merged: false,
        merged_count: 0,
        models: fetched.models,
        provider: None,
    }))
}

pub(in crate::api) async fn test_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<TestProvidersRequest>,
) -> Result<Json<TestProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await;
    let selected = providers
        .providers
        .iter()
        .filter(|item| input.app.is_none_or(|app| item.app == app))
        .filter(|item| {
            input.provider_keys.as_ref().is_none_or(|keys| {
                keys.iter()
                    .any(|key| key.app == item.app && key.provider_id == item.provider.id)
            })
        })
        .cloned()
        .map(|stored| {
            proxy::provider_ops::ProviderExecution::from_store_for_operation(&providers, stored)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(ApiError::proxy)?;
    drop(providers);
    let mut results = Vec::new();
    for execution in selected {
        let query = TestProviderQuery {
            app: execution.stored.app,
            network: input.network,
            timeout_ms: input.timeout_ms,
            model: input.model.clone(),
            stream: input.stream,
        };
        results.push(test_provider_inner(&state, execution, &query).await?);
    }
    Ok(Json(TestProvidersResponse { ok: true, results }))
}

pub(in crate::api) async fn test_provider_inner(
    state: &ServerState,
    execution: proxy::provider_ops::ProviderExecution,
    query: &TestProviderQuery,
) -> Result<TestProviderResponse, ApiError> {
    let runtime_stored = execution.runtime_stored_view();
    let stored = &runtime_stored;
    let defaults = crate::domain::stream_check::StreamCheckConfig::default();
    let adapter = proxy::adapters::adapter_for(stored.app, stored.provider_type);
    let capability = adapter.capability(stored.app, stored.provider_type);
    let route = default_test_route(stored.app);
    let requested_stream = query.stream.unwrap_or(false);
    let model = provider_test_model(stored.app, stored, query.model.as_deref(), Some(&defaults));
    let gemini_path = default_gemini_test_path(stored.app, &model, requested_stream);
    if let Err(error) =
        execution.ensure_operation_supported(proxy::provider_ops::ProviderOperation::Test)
    {
        if error.status == axum::http::StatusCode::NOT_IMPLEMENTED {
            return Ok(TestProviderResponse {
                ok: false,
                outcome: ProviderOperationOutcome::Unsupported,
                driver_id: execution.plan.driver_id.to_string(),
                runtime_fingerprint: execution.plan.runtime_fingerprint.clone(),
                provider_id: stored.provider.id.clone(),
                app: stored.app,
                provider_type: stored.provider_type,
                provider_revision: stored.resource.revision,
                adapter: capability.adapter,
                support: capability.support,
                endpoint: redact_provider_endpoint(&execution.plan.endpoint),
                model,
                stream: requested_stream,
                header_names: Vec::new(),
                network_checked: false,
                network_status_code: None,
                network_latency_ms: None,
                network_stream_completed: None,
                network_error: Some(error.message.clone()),
                message: error.message,
            });
        }
        return Err(ApiError::proxy(error));
    }
    if let Some((outcome, message)) = provider_configuration_outcome(&execution) {
        return Ok(TestProviderResponse {
            ok: false,
            outcome,
            driver_id: execution.plan.driver_id.to_string(),
            runtime_fingerprint: execution.plan.runtime_fingerprint.clone(),
            provider_id: stored.provider.id.clone(),
            app: stored.app,
            provider_type: stored.provider_type,
            provider_revision: stored.resource.revision,
            adapter: capability.adapter,
            support: capability.support,
            endpoint: redact_provider_endpoint(&execution.plan.endpoint),
            model,
            stream: requested_stream,
            header_names: Vec::new(),
            network_checked: false,
            network_status_code: None,
            network_latency_ms: None,
            network_stream_completed: None,
            network_error: Some(message.clone()),
            message,
        });
    }

    if query.network.unwrap_or(false) {
        if let Some((provider_type, account_id)) = execution.managed_account_target() {
            state
                .refresh_managed_account_if_needed(provider_type, account_id)
                .await
                .map_err(map_managed_account_refresh_error)?;
        }
    }
    let accounts = state.accounts_snapshot().await;
    let body = provider_test_body(stored.app, stored, Some(&model), requested_stream);
    let (adapter_request, endpoint, target_headers) = if execution
        .driver_is("oauth.claude_messages")
    {
        let prepared = execution
            .prepare_claude_request(Bytes::from(body), route, &HeaderMap::new(), &accounts, None)
            .map_err(ApiError::proxy)?;
        (
            prepared.adapter_request,
            prepared.endpoint,
            prepared.headers,
        )
    } else {
        let mut adapter_request = adapter
            .transform_request_for_route(Bytes::from(body), stored, route, gemini_path.as_deref())
            .map_err(ApiError::proxy)?;
        execution
            .enforce_model_policy(&mut adapter_request)
            .map_err(ApiError::proxy)?;
        execution
            .finalize_request(&mut adapter_request)
            .map_err(ApiError::proxy)?;
        let mut endpoint = execution
            .resolve_endpoint(route, gemini_path, &adapter_request)
            .map_err(ApiError::proxy)?;
        let mut target_headers = adapter
            .build_headers(stored.app, stored, &accounts)
            .map_err(ApiError::proxy)?
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect::<Vec<_>>();
        target_headers.extend(
            adapter_request
                .upstream_headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.clone())),
        );
        execution
            .apply_test_forward_contract(
                route,
                &mut adapter_request,
                &mut endpoint,
                &mut target_headers,
            )
            .map_err(ApiError::proxy)?;
        let materialized_auth = execution
            .materialize_auth(&accounts)
            .map_err(ApiError::proxy)?;
        execution
            .apply_auth(
                &mut target_headers,
                &mut endpoint,
                materialized_auth.as_ref(),
            )
            .map_err(ApiError::proxy)?;
        (adapter_request, endpoint, target_headers)
    };
    let stream = adapter_request.stream_requested || requested_stream;
    let mut network_status_code = None;
    let mut network_latency_ms = None;
    let mut network_error = None;
    let mut network_stream_completed = None;
    if query.network.unwrap_or(false) {
        let started = std::time::Instant::now();
        let http_client = state.http_client().await;
        let mut request = http_client
            .post(&endpoint)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(adapter_request.body.clone());
        if stream {
            request = request.header(axum::http::header::ACCEPT, "text/event-stream");
        }
        if execution.driver_is("oauth.openai_codex") {
            request = request.header("accept-encoding", "identity");
        }
        for (name, value) in &target_headers {
            request = request.header(name, value);
        }
        let timeout = query
            .timeout_ms
            .filter(|value| *value > 0)
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| execution.request_timeout());
        match request.timeout(timeout).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                network_status_code = Some(status);
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

    let outcome = provider_test_outcome(
        query.network.unwrap_or(false),
        network_status_code,
        network_error.as_deref(),
    );
    let ok = outcome == ProviderOperationOutcome::Success;
    let message = if !ok {
        network_error
            .clone()
            .unwrap_or_else(|| format!("provider test outcome: {:?}", outcome))
    } else if query.network.unwrap_or(false) {
        "configuration check passed; upstream network/model call executed".to_string()
    } else {
        "configuration check passed; upstream network/model call is not executed".to_string()
    };

    Ok(TestProviderResponse {
        ok,
        outcome,
        driver_id: execution.plan.driver_id.to_string(),
        runtime_fingerprint: execution.plan.runtime_fingerprint.clone(),
        provider_id: stored.provider.id.clone(),
        app: stored.app,
        provider_type: stored.provider_type,
        provider_revision: stored.resource.revision,
        adapter: capability.adapter,
        support: capability.support,
        endpoint: redact_provider_endpoint(&endpoint),
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
        message,
    })
}

fn provider_test_outcome(
    network_checked: bool,
    status: Option<u16>,
    error: Option<&str>,
) -> ProviderOperationOutcome {
    if !network_checked {
        return ProviderOperationOutcome::Success;
    }
    if let Some(status) = status {
        return match status {
            200..=399 if error.is_none() => ProviderOperationOutcome::Success,
            401 | 403 => ProviderOperationOutcome::Auth,
            402 => ProviderOperationOutcome::Quota,
            429 if error.is_some_and(provider_error_mentions_quota) => {
                ProviderOperationOutcome::Quota
            }
            429 => ProviderOperationOutcome::RateLimit,
            400..=499 => ProviderOperationOutcome::Protocol,
            _ => ProviderOperationOutcome::Upstream,
        };
    }
    if error.is_some_and(|error| error.to_ascii_lowercase().contains("timeout")) {
        ProviderOperationOutcome::Timeout
    } else {
        ProviderOperationOutcome::Network
    }
}

fn provider_configuration_outcome(
    execution: &proxy::provider_ops::ProviderExecution,
) -> Option<(ProviderOperationOutcome, String)> {
    if execution.plan.configuration_state
        != crate::domain::providers::runtime::RuntimeConfigurationState::NeedsAttention
    {
        return None;
    }
    let outcome = if matches!(
        execution.plan.auth_ref,
        crate::domain::providers::runtime::RuntimeAuthRef::Missing
    ) {
        ProviderOperationOutcome::MissingCredential
    } else {
        ProviderOperationOutcome::InvalidConfig
    };
    let message = if execution.plan.warnings.is_empty() {
        "Provider runtime configuration needs attention".to_string()
    } else {
        execution.plan.warnings.join("; ")
    };
    Some((outcome, message))
}

fn provider_error_mentions_quota(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("quota")
        || error.contains("credit")
        || error.contains("billing")
        || error.contains("usage limit")
}

pub(in crate::api) async fn resolve_provider_by_key(
    state: &ServerState,
    app: AppKind,
    provider_id: &str,
) -> Result<StoredProvider, ApiError> {
    let key = crate::domain::providers::registry::ProviderKey::new(app, provider_id)
        .map_err(ApiError::bad_request)?;
    state
        .providers
        .read()
        .await
        .providers
        .iter()
        .find(|item| item.app == key.app && item.provider.id == key.provider_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("provider not found"))
}

pub(in crate::api) async fn resolve_provider_execution_by_key(
    state: &ServerState,
    app: AppKind,
    provider_id: &str,
) -> Result<proxy::provider_ops::ProviderExecution, ApiError> {
    let key = crate::domain::providers::registry::ProviderKey::new(app, provider_id)
        .map_err(ApiError::bad_request)?;
    let providers = state.providers.read().await;
    let stored = providers
        .providers
        .iter()
        .find(|item| item.app == key.app && item.provider.id == key.provider_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("provider not found"))?;
    proxy::provider_ops::ProviderExecution::from_store_for_operation(&providers, stored)
        .map_err(ApiError::proxy)
}

fn redact_provider_endpoint(endpoint: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(endpoint) else {
        return "[invalid endpoint]".to_string();
    };
    if !url.username().is_empty() {
        let _ = url.set_username("[REDACTED]");
    }
    if url.password().is_some() {
        let _ = url.set_password(Some("[REDACTED]"));
    }
    let query_names = url
        .query_pairs()
        .map(|(name, _)| name.into_owned())
        .collect::<Vec<_>>();
    if !query_names.is_empty() {
        let mut query = url.query_pairs_mut();
        query.clear();
        for name in query_names {
            query.append_pair(&name, "[REDACTED]");
        }
    }
    url.to_string()
}

pub(in crate::api) struct ProviderModelsFetchResult {
    url: String,
    models: Vec<FetchedProviderModel>,
}

pub(in crate::api) async fn fetch_provider_models_inner(
    state: &ServerState,
    execution: &proxy::provider_ops::ProviderExecution,
    timeout_ms: Option<u64>,
) -> Result<ProviderModelsFetchResult, ApiError> {
    let runtime_stored = execution.runtime_stored_view();
    let stored = &runtime_stored;
    execution
        .ensure_operation_supported(proxy::provider_ops::ProviderOperation::Discovery)
        .map_err(ApiError::proxy)?;
    let accounts = state.accounts_snapshot().await;
    let adapter = proxy::adapters::adapter_for(stored.app, stored.provider_type);
    let mut url = execution.discovery_url().map_err(ApiError::proxy)?;
    let mut target_headers = adapter
        .build_headers(stored.app, stored, &accounts)
        .map_err(ApiError::proxy)?
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect::<Vec<_>>();
    let materialized_auth = execution
        .materialize_auth(&accounts)
        .map_err(ApiError::proxy)?;
    execution
        .apply_auth(&mut target_headers, &mut url, materialized_auth.as_ref())
        .map_err(ApiError::proxy)?;
    let http_client = state.http_client().await;
    let mut request = http_client.get(&url);
    for (name, value) in target_headers {
        request = request.header(name, value);
    }
    let timeout = timeout_ms
        .filter(|value| *value > 0)
        .map(std::time::Duration::from_millis)
        .unwrap_or_else(|| execution.request_timeout());
    let response =
        request.timeout(timeout).send().await.map_err(|error| {
            ApiError::bad_gateway(format!("fetch provider models failed: {error}"))
        })?;
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

pub(in crate::api) async fn create_provider_from_preset(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CreateProviderFromPresetRequest>,
) -> Result<Json<CreateProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    require_provider_write_contract(&state, &headers)?;
    let profile = match input.profile_id {
        Some(profile_id) => crate::domain::providers::registry::profile_by_id(profile_id.as_str())
            .filter(|profile| profile.app == input.app)
            .ok_or_else(|| ApiError::not_found("Provider profile not found"))?,
        None => {
            let name = input.name.as_deref().ok_or_else(|| {
                ApiError::bad_request("profileId is required to create a Provider from a preset")
            })?;
            crate::domain::providers::registry::profile_for_legacy_preset(input.app, name)
                .ok_or_else(|| ApiError::not_found("provider preset not found"))?
        }
    };
    if profile.creation_policy != crate::domain::providers::registry::CreationPolicy::CreateAllowed
    {
        return Err(ApiError::bad_request(
            "Provider profile does not allow new resources",
        ));
    }
    let mut provider = s1_provider_for_profile(&state, profile)?;
    if let Some(account_id) = input.account_id {
        let account_provider_type = match &profile.credential_policy {
            crate::domain::providers::registry::CredentialPolicy::ManagedAccount {
                account_provider_type,
            } => *account_provider_type,
            _ => {
                return Err(ApiError::bad_request(
                    "accountId is only valid for a managed-account Provider profile",
                ));
            }
        };
        let meta = provider
            .meta
            .get_or_insert_with(crate::domain::providers::model::ProviderMeta::default);
        meta.auth_binding = Some(crate::domain::providers::model::AuthBinding {
            source: Some("account".to_string()),
            auth_provider: Some(account_provider_type.as_str().to_string()),
            account_id: Some(account_id),
            auth_identity_generation: None,
        });
    }
    let app = input.app;
    let stored = state
        .upsert_provider_draft_command(crate::domain::providers::credentials::ProviderWriteDraft {
            app,
            provider,
            profile_id: Some(profile.profile_id.clone()),
            custom_binding: input.custom_binding,
            expected_revision: None,
            client_request_id: input.client_request_id,
            credential_patches: input.credential_patches,
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(map_provider_command_error)?;
    Ok(Json(CreateProviderResponse {
        ok: true,
        stored: crate::domain::providers::credentials::ProviderView::from_stored(&stored),
    }))
}

fn s1_provider_for_profile(
    state: &ServerState,
    profile: &crate::domain::providers::registry::ProfileSpec,
) -> Result<Provider, ApiError> {
    if let Some(legacy_name) = crate::domain::providers::registry::provider_registry()
        .legacy_preset_mappings
        .iter()
        .find(|mapping| mapping.app == profile.app && mapping.profile_id == profile.profile_id)
        .map(|mapping| mapping.legacy_name.as_str())
    {
        let mut provider = fixtures_for_app(&state.provider_coverage, profile.app)
            .into_iter()
            .find(|item| item.name == legacy_name)
            .map(|fixture| fixture.provider.clone())
            .ok_or_else(|| ApiError::not_found("provider preset fixture not found"))?;
        provider.id.clear();
        apply_s1_profile_creation_defaults(profile, &mut provider)?;
        return Ok(provider);
    }

    let (name, settings_config) = match profile.profile_id.as_str() {
        "claude.anthropic_api_key" => (
            "Anthropic API Key",
            json!({
                "env": {"ANTHROPIC_BASE_URL": "https://api.anthropic.com"},
                "modelMapping": {"mode": "passthrough"}
            }),
        ),
        "codex.openai_api_key" => (
            "OpenAI API Key",
            json!({
                "auth": {},
                "config": "model = \"gpt-5.4\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true",
                "modelMapping": {"mode": "passthrough"}
            }),
        ),
        "gemini.google_api_key" => ("Google Gemini API Key", json!({"env": {}})),
        "claude.custom_http" | "codex.custom_http" | "gemini.custom_http" => {
            ("Custom Provider", json!({}))
        }
        _ => {
            return Err(ApiError::bad_request(
                "Provider profile does not have an S1 creation bridge",
            ));
        }
    };
    let mut provider = Provider {
        id: String::new(),
        name: name.to_string(),
        settings_config,
        category: None,
        meta: None,
        extra: Default::default(),
    };
    apply_s1_profile_creation_defaults(profile, &mut provider)?;
    Ok(provider)
}

fn apply_s1_profile_creation_defaults(
    profile: &crate::domain::providers::registry::ProfileSpec,
    provider: &mut Provider,
) -> Result<(), ApiError> {
    use crate::domain::providers::registry::ModelPolicyKind;

    if !provider.settings_config.is_object() {
        provider.settings_config = json!({});
    }
    let settings = provider
        .settings_config
        .as_object_mut()
        .expect("Provider settings were normalized to an object");
    match profile.model_policy {
        ModelPolicyKind::Passthrough => {
            settings.insert("modelMapping".to_string(), json!({"mode": "passthrough"}));
        }
        ModelPolicyKind::Single => {
            let upstream_model = s1_profile_default_upstream_model(profile.profile_id.as_str())
                .ok_or_else(|| {
                    ApiError::bad_request(format!(
                        "Provider profile {} has no S1 default upstream model",
                        profile.profile_id
                    ))
                })?;
            settings.insert(
                "modelMapping".to_string(),
                json!({"mode": "single", "upstreamModel": upstream_model}),
            );
        }
    }
    Ok(())
}

fn s1_profile_default_upstream_model(profile_id: &str) -> Option<&'static str> {
    Some(match profile_id {
        "claude.openai_oauth" => "gpt-5.6-sol",
        "claude.grok_oauth" | "codex.grok_oauth" | "gemini.grok_oauth" => "grok-4.5",
        "claude.kiro_oauth" => "claude-sonnet-4-8",
        "claude.ollama_cloud" | "codex.ollama_cloud" => "kimi-k2.7-code",
        "claude.cursor_oauth" | "claude.cursor_api_key" => "composer-2.5",
        "claude.antigravity_oauth" | "claude.antigravity_cli" => "claude-sonnet-4-6",
        "claude.github_copilot" => "claude-sonnet-5",
        "claude.deepseek_account" | "claude.deepseek_api" | "codex.deepseek_api" => {
            "deepseek-v4-flash"
        }
        "claude.aws_bedrock_aksk" | "claude.aws_bedrock_api_key" => {
            "global.anthropic.claude-opus-4-8"
        }
        "claude.openrouter" => "anthropic/claude-sonnet-4.6",
        "claude.nvidia" | "codex.nvidia" => "moonshotai/kimi-k2.5",
        "codex.cursor_api_key" | "codex.cursor_oauth" => "gpt-5.5",
        "codex.openrouter" => "gpt-5.4",
        "gemini.antigravity_oauth" | "gemini.antigravity_cli" => "gemini-3.5-flash-medium",
        "gemini.openrouter" => "gemini-3.5-flash",
        "claude.custom_http" => "claude-sonnet-4-6",
        "codex.custom_http" => "gpt-5.4",
        "gemini.custom_http" => "gemini-3.5-flash",
        _ => return None,
    })
}

pub(in crate::api) async fn provider_registry() -> Json<ProviderRegistryResponse> {
    Json(ProviderRegistryResponse {
        ok: true,
        registry: crate::domain::providers::registry::provider_registry().clone(),
    })
}

pub(in crate::api) fn map_provider_command_error(
    error: crate::domain::providers::credentials::ProviderCommandError,
) -> ApiError {
    match error {
        crate::domain::providers::credentials::ProviderCommandError::NotFound => {
            ApiError::not_found("provider not found")
        }
        crate::domain::providers::credentials::ProviderCommandError::Invalid(message) => {
            ApiError::bad_request(message)
        }
        crate::domain::providers::credentials::ProviderCommandError::Conflict { code, message } => {
            ApiError::conflict_code(code, message)
        }
    }
}

pub(in crate::api) fn require_provider_write_contract(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    if state.web_dist_dir.is_none() {
        return Ok(());
    }
    let version = headers
        .get(web_runtime::PROVIDER_CONTRACT_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u32>().ok());
    if version.is_some_and(|version| {
        (web_runtime::PROVIDER_CONTRACT_MIN_SUPPORTED
            ..=web_runtime::PROVIDER_CONTRACT_MAX_SUPPORTED)
            .contains(&version)
    }) {
        return Ok(());
    }
    Err(ApiError::provider_contract_mismatch(format!(
        "Provider write requires contract version {}-{}",
        web_runtime::PROVIDER_CONTRACT_MIN_SUPPORTED,
        web_runtime::PROVIDER_CONTRACT_MAX_SUPPORTED
    )))
}

pub(in crate::api) async fn provider_presets(
    State(state): State<ServerState>,
    Query(query): Query<ProviderPresetsQuery>,
) -> Json<ProviderPresetsResponse> {
    use crate::domain::providers::registry::{CreationPolicy, ProfileVisibility};

    let legacy_presets = match query.app {
        Some(AppKind::Claude) => &state.provider_coverage.presets.claude,
        Some(AppKind::Codex) => &state.provider_coverage.presets.codex,
        Some(AppKind::Gemini) => &state.provider_coverage.presets.gemini,
        None => {
            return Json(ProviderPresetsResponse {
                ok: true,
                presets: Vec::new(),
            })
        }
    };
    let app = query.app.expect("app was matched above");
    let registry = crate::domain::providers::registry::provider_registry();
    let presets = registry
        .profiles
        .iter()
        .filter(|profile| {
            profile.app == app
                && profile.visibility == ProfileVisibility::Visible
                && profile.creation_policy == CreationPolicy::CreateAllowed
        })
        .map(|profile| {
            let legacy_name = registry
                .legacy_preset_mappings
                .iter()
                .find(|mapping| mapping.app == app && mapping.profile_id == profile.profile_id)
                .map(|mapping| mapping.legacy_name.as_str());
            let mut summary = legacy_name
                .and_then(|name| legacy_presets.iter().find(|preset| preset.name == name))
                .cloned()
                .unwrap_or_else(|| crate::api::web::coverage::PresetSummary {
                    name: profile.label.clone(),
                    profile_id: None,
                    profile_schema_revision: None,
                    provider_type: profile
                        .compatibility_provider_type
                        .map(|provider_type| provider_type.as_str().to_string()),
                    api_format: None,
                    base_url: None,
                });
            summary.profile_id = Some(profile.profile_id.clone());
            summary.profile_schema_revision = Some(profile.profile_schema_revision);
            summary
        })
        .collect();
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
    defaults: Option<&crate::domain::stream_check::StreamCheckConfig>,
) -> String {
    let defaults = defaults.cloned().unwrap_or_default();
    let resolved = override_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .or_else(|| {
            stored
                .provider
                .settings_config
                .pointer("/testConfig/testModel")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            stored
                .provider
                .settings_config
                .pointer("/testConfig/model")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            stored
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.test_config.as_ref())
                .and_then(|value| value.get("testModel").or_else(|| value.get("model")))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            stored
                .provider
                .settings_config
                .pointer("/modelMapping/upstreamModel")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
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
                .map(str::to_string)
        })
        .or_else(|| extract_codex_model_from_settings(&stored.provider.settings_config))
        .unwrap_or_else(|| match app {
            AppKind::Claude => defaults.claude_model.clone(),
            AppKind::Codex => defaults.codex_model.clone(),
            AppKind::Gemini => defaults.gemini_model.clone(),
        });

    if stored.provider_type == ProviderType::CodexOAuth && app == AppKind::Codex {
        normalize_codex_oauth_test_model(&resolved)
    } else {
        resolved
    }
}

fn extract_codex_model_from_settings(settings: &serde_json::Value) -> Option<String> {
    let config_text = settings.get("config").and_then(serde_json::Value::as_str)?;
    for line in config_text.lines() {
        let line = line.split('#').next().unwrap_or(line).trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "model" {
            continue;
        }
        let model = value.trim().trim_matches('"').trim_matches('\'').trim();
        if !model.is_empty() {
            return Some(model.to_string());
        }
    }
    None
}

fn normalize_codex_oauth_test_model(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return "gpt-5.6-sol@low".to_string();
    }
    if trimmed.contains('@') || trimmed.contains('#') {
        return trimmed.to_string();
    }
    format!("{trimmed}@low")
}

fn parse_model_with_effort(model: &str) -> (String, Option<String>) {
    if let Some(pos) = model.find('@').or_else(|| model.find('#')) {
        let actual_model = model[..pos].trim().to_string();
        let effort = model[pos + 1..].trim().to_string();
        if !actual_model.is_empty() && !effort.is_empty() {
            return (actual_model, Some(effort));
        }
    }
    (model.trim().to_string(), None)
}

pub(in crate::api) fn provider_test_body(
    app: AppKind,
    stored: &StoredProvider,
    override_model: Option<&str>,
    stream: bool,
) -> String {
    let model = provider_test_model(app, stored, override_model, None);
    let value = match app {
        AppKind::Claude => serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "ping"}],
            "stream": stream
        }),
        AppKind::Codex => {
            let (actual_model, effort) = parse_model_with_effort(&model);
            let mut body = serde_json::json!({
                "model": actual_model,
                "input": [{
                    "role": "user",
                    "content": "ping"
                }],
                "stream": stream
            });
            if let Some(effort) = effort {
                body["reasoning"] = serde_json::json!({ "effort": effort });
            } else if stored.provider_type == ProviderType::CodexOAuth {
                body["reasoning"] = serde_json::json!({ "effort": "low" });
            } else {
                body["max_output_tokens"] = serde_json::json!(1);
            }
            if stored.provider_type == ProviderType::CodexOAuth {
                body["store"] = serde_json::json!(false);
                body["include"] = serde_json::json!(["reasoning.encrypted_content"]);
                body["instructions"] = serde_json::json!("");
                body["tools"] = serde_json::json!([]);
                body["parallel_tool_calls"] = serde_json::json!(false);
            }
            body
        }
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

fn map_managed_account_refresh_error(error: crate::state::ManagedAccountRefreshError) -> ApiError {
    use crate::state::ManagedAccountRefreshError;
    use axum::http::StatusCode;
    match error {
        ManagedAccountRefreshError::Conflict { provider_type } => ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "{} account refresh is already in progress",
                provider_type.as_str()
            ),
        ),
        ManagedAccountRefreshError::NotFound => ApiError::not_found("managed account not found"),
        ManagedAccountRefreshError::Refresh {
            status_code,
            message,
        } => ApiError::new(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY),
            format!("managed account refresh failed: {message}"),
        ),
    }
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
            resource: Default::default(),
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
    fn codex_oauth_test_model_appends_low_effort_suffix() {
        let stored = StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "p1".to_string(),
                name: "OpenAI OAuth".to_string(),
                settings_config: json!({
                    "config": "model = \"gpt-5.6-sol\"\n"
                }),
                category: Some("official".to_string()),
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::CodexOAuth,
            provider_type_id: "codex_oauth".to_string(),
            resource: Default::default(),
        };

        assert_eq!(
            provider_test_model(AppKind::Codex, &stored, None, None),
            "gpt-5.6-sol@low"
        );
    }

    #[test]
    fn codex_oauth_test_body_includes_required_responses_fields() {
        let stored = StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "p1".to_string(),
                name: "OpenAI OAuth".to_string(),
                settings_config: json!({
                    "config": "model = \"gpt-5.6-sol\"\n"
                }),
                category: Some("official".to_string()),
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::CodexOAuth,
            provider_type_id: "codex_oauth".to_string(),
            resource: Default::default(),
        };

        let value: serde_json::Value =
            serde_json::from_str(&provider_test_body(AppKind::Codex, &stored, None, true)).unwrap();

        assert_eq!(
            value.get("model").and_then(serde_json::Value::as_str),
            Some("gpt-5.6-sol")
        );
        assert_eq!(
            value
                .pointer("/reasoning/effort")
                .and_then(serde_json::Value::as_str),
            Some("low")
        );
        assert_eq!(
            value.get("store").and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert!(value
            .get("include")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|items| !items.is_empty()));
        assert!(value.get("max_output_tokens").is_none());
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

    #[test]
    fn provider_endpoint_redaction_hides_userinfo_and_query_values() {
        let redacted = redact_provider_endpoint(
            "https://client:password@example.com/v1/models?api_key=secret&tenant=private",
        );

        assert!(!redacted.contains("client"));
        assert!(!redacted.contains("password"));
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("private"));
        assert!(redacted.contains("api_key=%5BREDACTED%5D"));
        assert!(redacted.contains("tenant=%5BREDACTED%5D"));
    }

    #[test]
    fn provider_test_outcome_distinguishes_quota_rate_limit_and_timeout() {
        assert_eq!(
            provider_test_outcome(true, Some(402), Some("payment required")),
            ProviderOperationOutcome::Quota
        );
        assert_eq!(
            provider_test_outcome(true, Some(429), Some("account quota exhausted")),
            ProviderOperationOutcome::Quota
        );
        assert_eq!(
            provider_test_outcome(true, Some(429), Some("too many requests")),
            ProviderOperationOutcome::RateLimit
        );
        assert_eq!(
            provider_test_outcome(true, None, Some("request timeout")),
            ProviderOperationOutcome::Timeout
        );
    }
}
