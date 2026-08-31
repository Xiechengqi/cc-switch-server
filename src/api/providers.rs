use super::*;
use crate::domain::providers::registry::ProviderKey;
use crate::domain::providers::runtime::{build_provider_model_probe, PROVIDER_MODEL_PROBE_PROMPT};
use base64::Engine;
use serde::Serialize;

const PROVIDER_TEST_RESPONSE_BODY_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const PROVIDER_MODELS_RESPONSE_BODY_LIMIT_BYTES: usize = 8 * 1024 * 1024;
const PROVIDER_MEDIA_TEST_RESPONSE_BODY_LIMIT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::api) struct ProviderInferenceTestRequest {
    operation: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProviderInferenceTestResponse {
    ok: bool,
    operation: String,
    status_code: u16,
    latency_ms: u64,
    content_type: Option<String>,
    body_text: String,
    body_truncated: bool,
}

pub(in crate::api) async fn test_provider_inference(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ProviderInferenceTestRequest>,
) -> Result<Json<ProviderInferenceTestResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let operation = input.operation.trim().to_ascii_lowercase();
    if !matches!(
        operation.as_str(),
        "image_generation" | "image_edit" | "video_generation"
    ) {
        return Err(ApiError::bad_request(
            "operation must be image_generation, image_edit, or video_generation",
        ));
    }
    let execution = resolve_provider_execution_by_key(&state, AppKind::Codex, &id).await?;
    let (path, content_type, body) = match operation.as_str() {
        "image_generation" => (
            "/images/generations".to_string(),
            "application/json".to_string(),
            Bytes::from(
                json!({
                    "model": "grok-imagine",
                    "prompt": "A small blue circle on a plain white background",
                    "n": 1,
                    "response_format": "b64_json"
                })
                .to_string(),
            ),
        ),
        "image_edit" => {
            const TEST_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
            let image = base64::engine::general_purpose::STANDARD
                .decode(TEST_PNG_BASE64)
                .map_err(ApiError::internal)?;
            let boundary = "cc-switch-grok-media-test";
            let mut multipart = Vec::new();
            multipart.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngrok-imagine\r\n").as_bytes());
            multipart.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nMake the dot green\r\n").as_bytes());
            multipart.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"test.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
            multipart.extend_from_slice(&image);
            multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
            (
                "/images/edits".to_string(),
                format!("multipart/form-data; boundary={boundary}"),
                Bytes::from(multipart),
            )
        }
        "video_generation" => (
            "/videos/generations".to_string(),
            "application/json".to_string(),
            Bytes::from(
                json!({
                    "model": "grok-imagine-video",
                    "prompt": "A blue circle slowly moving from left to right",
                    "duration": 6,
                    "resolution": "720p",
                    "aspect_ratio": "16:9"
                })
                .to_string(),
            ),
        ),
        _ => unreachable!("operation was validated"),
    };
    let mut request_headers = HeaderMap::new();
    request_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type).map_err(ApiError::internal)?,
    );
    request_headers.insert("x-cc-switch-health-check", HeaderValue::from_static("1"));
    request_headers.insert(
        "x-cc-switch-share-id",
        HeaderValue::from_str(&format!("test-share:{id}")).map_err(ApiError::internal)?,
    );
    request_headers.insert(
        "x-cc-switch-request-id",
        HeaderValue::from_str(&new_transport_request_id().0).map_err(ApiError::internal)?,
    );
    let started = std::time::Instant::now();
    let response = proxy::forward_grok_media_provider_test(
        state,
        execution,
        Method::POST,
        path,
        request_headers,
        body,
    )
    .await
    .map_err(ApiError::proxy)?;
    let status_code = response.status().as_u16();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(
        response.into_body(),
        PROVIDER_MEDIA_TEST_RESPONSE_BODY_LIMIT_BYTES,
    )
    .await
    .map_err(ApiError::internal)?;
    let preview_len = bytes.len().min(PROVIDER_TEST_RESPONSE_BODY_LIMIT_BYTES);
    Ok(Json(ProviderInferenceTestResponse {
        ok: (200..300).contains(&status_code),
        operation,
        status_code,
        latency_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        content_type,
        body_text: String::from_utf8_lossy(&bytes[..preview_len]).into_owned(),
        body_truncated: preview_len < bytes.len(),
    }))
}

pub(in crate::api) async fn list_provider_bundles(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListProviderBundlesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let bundles = state
        .provider_bundle_views()
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(ListProviderBundlesResponse { ok: true, bundles }))
}

pub(in crate::api) async fn create_provider_bundle(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<crate::domain::providers::bundle::ProviderBundleWriteDraft>,
) -> Result<Json<ProviderBundleResponse>, ApiError> {
    require_session(&state, &headers).await?;
    require_provider_write_contract(&state, &headers)?;
    if input.expected_revision.is_some() {
        return Err(ApiError::bad_request(
            "expectedRevision is only valid when updating a Provider Bundle",
        ));
    }
    let bundle = state
        .upsert_provider_bundle_command(input)
        .await
        .map_err(ApiError::internal)?
        .map_err(map_provider_command_error)?;
    Ok(Json(ProviderBundleResponse { ok: true, bundle }))
}

pub(in crate::api) async fn get_provider_bundle(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ProviderBundleResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let bundle = state
        .provider_bundle_view(&id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("Provider Bundle not found"))?;
    Ok(Json(ProviderBundleResponse { ok: true, bundle }))
}

pub(in crate::api) async fn update_provider_bundle_order(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpdateProviderBundleOrderRequest>,
) -> Result<Json<UpdateProviderBundleOrderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    require_provider_write_contract(&state, &headers)?;
    let changed = state
        .update_provider_bundle_order_command(input.updates)
        .await
        .map_err(ApiError::internal)?
        .map_err(map_provider_command_error)?;
    Ok(Json(UpdateProviderBundleOrderResponse {
        ok: true,
        changed,
    }))
}

pub(in crate::api) async fn update_provider_bundle(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<crate::domain::providers::bundle::ProviderBundleWriteDraft>,
) -> Result<Json<ProviderBundleResponse>, ApiError> {
    require_session(&state, &headers).await?;
    require_provider_write_contract(&state, &headers)?;
    if input.id != id {
        return Err(ApiError::bad_request(
            "Provider Bundle id in body must match resource path",
        ));
    }
    if input.expected_revision.is_none() {
        return Err(ApiError::bad_request(
            "expectedRevision is required when updating a Provider Bundle",
        ));
    }
    let bundle = state
        .upsert_provider_bundle_command(input)
        .await
        .map_err(ApiError::internal)?
        .map_err(map_provider_command_error)?;
    Ok(Json(ProviderBundleResponse { ok: true, bundle }))
}

pub(in crate::api) async fn delete_provider_bundle(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<DeleteProviderBundleQuery>,
) -> Result<Json<DeleteProviderBundleResponse>, ApiError> {
    require_session(&state, &headers).await?;
    require_provider_write_contract(&state, &headers)?;
    let deleted = state
        .delete_provider_bundle_command(id, query.expected_revision)
        .await
        .map_err(ApiError::internal)?
        .map_err(map_provider_command_error)?;
    Ok(Json(DeleteProviderBundleResponse { ok: true, deleted }))
}

pub(in crate::api) async fn provider_bundle_delete_preview(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ProviderBundleDeletePreviewResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let preview = state
        .provider_bundle_reference_preview(&id)
        .await
        .map_err(map_provider_command_error)?;
    Ok(Json(ProviderBundleDeletePreviewResponse {
        ok: true,
        preview,
    }))
}

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
    let provider_store = state.providers.read().await;
    let providers = provider_store.list(query.app);
    let usage = state.usage.read().await;
    Ok(Json(ProviderHealthResponse {
        ok: true,
        providers: providers
            .iter()
            .map(|provider| {
                let plan = provider_store.runtime_plan(provider.app, &provider.provider.id);
                crate::domain::health::provider_health_for_plan(provider, &usage, plan.as_deref())
            })
            .collect(),
    }))
}

pub(in crate::api) async fn get_coding_plan_quota(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ProviderResourceQuery>,
) -> Result<Json<CodingPlanQuotaResponse>, ApiError> {
    coding_plan_quota_response(state, headers, id, query.app, false).await
}

pub(in crate::api) async fn refresh_coding_plan_quota(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ProviderResourceQuery>,
) -> Result<Json<CodingPlanQuotaResponse>, ApiError> {
    coding_plan_quota_response(state, headers, id, query.app, true).await
}

pub(in crate::api) async fn get_provider_account_usage(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ProviderResourceQuery>,
) -> Result<(HeaderMap, Json<OllamaCloudSnapshotResponse>), ApiError> {
    ollama_cloud_snapshot_response(state, headers, id, query.app, false).await
}

pub(in crate::api) async fn refresh_provider_account_usage(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ProviderResourceQuery>,
) -> Result<(HeaderMap, Json<OllamaCloudSnapshotResponse>), ApiError> {
    ollama_cloud_snapshot_response(state, headers, id, query.app, true).await
}

pub(in crate::api) async fn get_cursor_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ProviderResourceQuery>,
) -> Result<(HeaderMap, Json<CursorAccountSnapshotResponse>), ApiError> {
    cursor_account_snapshot_response(state, headers, id, query.app, false).await
}

pub(in crate::api) async fn refresh_cursor_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ProviderResourceQuery>,
) -> Result<(HeaderMap, Json<CursorAccountSnapshotResponse>), ApiError> {
    cursor_account_snapshot_response(state, headers, id, query.app, true).await
}

async fn cursor_account_snapshot_response(
    state: ServerState,
    headers: HeaderMap,
    provider_id: String,
    app: AppKind,
    force_refresh: bool,
) -> Result<(HeaderMap, Json<CursorAccountSnapshotResponse>), ApiError> {
    require_session(&state, &headers).await?;
    let provider_key = ProviderKey::new(app, provider_id).map_err(ApiError::bad_request)?;
    let snapshot = state
        .cursor_account_snapshot(provider_key, force_refresh)
        .await
        .map_err(ApiError::internal)?
        .map_err(map_provider_command_error)?;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store, max-age=0"),
    );
    Ok((
        response_headers,
        Json(CursorAccountSnapshotResponse { ok: true, snapshot }),
    ))
}

async fn ollama_cloud_snapshot_response(
    state: ServerState,
    headers: HeaderMap,
    provider_id: String,
    app: AppKind,
    force_refresh: bool,
) -> Result<(HeaderMap, Json<OllamaCloudSnapshotResponse>), ApiError> {
    require_session(&state, &headers).await?;
    let provider_key = ProviderKey::new(app, provider_id).map_err(ApiError::bad_request)?;
    let snapshot = state
        .ollama_cloud_snapshot(provider_key, force_refresh)
        .await
        .map_err(ApiError::internal)?
        .map_err(map_provider_command_error)?;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store, max-age=0"),
    );
    Ok((
        response_headers,
        Json(OllamaCloudSnapshotResponse { ok: true, snapshot }),
    ))
}

async fn coding_plan_quota_response(
    state: ServerState,
    headers: HeaderMap,
    provider_id: String,
    app: AppKind,
    force_refresh: bool,
) -> Result<Json<CodingPlanQuotaResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let provider_key = crate::domain::providers::registry::ProviderKey::new(app, provider_id)
        .map_err(ApiError::bad_request)?;
    let snapshot = state
        .coding_plan_quota_snapshot(provider_key, force_refresh)
        .await
        .map_err(ApiError::internal)?
        .map_err(map_provider_command_error)?;
    Ok(Json(CodingPlanQuotaResponse { ok: true, snapshot }))
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
    Query(mut query): Query<TestProviderQuery>,
) -> Result<Json<TestProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let health_config = if query.network.unwrap_or(false) {
        Some(web_provider_health_check_config(&state).await)
    } else {
        None
    };
    if query.timeout_ms.is_none() {
        query.timeout_ms = health_config
            .as_ref()
            .map(|config| config.timeout_seconds.saturating_mul(1_000));
    }
    if query.test_prompt.is_none() {
        query.test_prompt = health_config
            .as_ref()
            .map(|_| PROVIDER_MODEL_PROBE_PROMPT.to_string());
    }
    let execution = resolve_provider_execution_by_key(&state, query.app, &id).await?;
    let expected_health_fingerprint = execution.plan.health_fingerprint();
    let provider = execution.runtime_stored_view();
    let response = test_provider_inner(&state, execution, &query).await?;
    if let Some(config) = health_config.as_ref() {
        if let Err(error) = crate::api::provider_health_scheduler::record_provider_test_response(
            &state,
            &provider,
            &response,
            config,
            &expected_health_fingerprint,
            "cc-switch-manual",
        )
        .await
        {
            tracing::warn!(
                app = provider.app.as_str(),
                provider_id = %provider.provider.id,
                error = %error,
                "failed to persist manual Provider health result"
            );
        }
    }
    Ok(Json(response))
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
                source: None,
                stale: None,
                fetched_at_ms: None,
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
            source: None,
            stale: None,
            fetched_at_ms: None,
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
        source: fetched.source,
        stale: fetched.stale,
        fetched_at_ms: fetched.fetched_at_ms,
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
    let health_config = if input.network.unwrap_or(false) {
        Some(web_provider_health_check_config(&state).await)
    } else {
        None
    };
    let mut results = Vec::new();
    for execution in selected {
        let expected_health_fingerprint = execution.plan.health_fingerprint();
        let provider = execution.runtime_stored_view();
        let query = TestProviderQuery {
            app: execution.stored.app,
            network: input.network,
            timeout_ms: input.timeout_ms.or_else(|| {
                health_config
                    .as_ref()
                    .map(|config| config.timeout_seconds.saturating_mul(1_000))
            }),
            model: input.model.clone(),
            test_prompt: input.test_prompt.clone().or_else(|| {
                health_config
                    .as_ref()
                    .map(|_| PROVIDER_MODEL_PROBE_PROMPT.to_string())
            }),
            stream: input.stream,
        };
        let response = test_provider_inner(&state, execution, &query).await?;
        if let Some(config) = health_config.as_ref() {
            if let Err(error) =
                crate::api::provider_health_scheduler::record_provider_test_response(
                    &state,
                    &provider,
                    &response,
                    config,
                    &expected_health_fingerprint,
                    "cc-switch-manual",
                )
                .await
            {
                tracing::warn!(
                    app = provider.app.as_str(),
                    provider_id = %provider.provider.id,
                    error = %error,
                    "failed to persist manual Provider health result"
                );
            }
        }
        results.push(response);
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
    let adapter = proxy::adapters::adapter_for(stored.app, stored.provider_type);
    let capability = adapter.capability(stored.app, stored.provider_type);
    let route = default_test_route(stored.app);
    let requested_stream = query.stream.unwrap_or(false);
    let test_prompt = query.test_prompt.as_deref().unwrap_or("ping");
    if test_prompt.trim().is_empty()
        || test_prompt != test_prompt.trim()
        || test_prompt.len() > 1_000
    {
        return Err(ApiError::bad_request(
            "testPrompt must be non-empty, trimmed, and at most 1000 characters",
        ));
    }
    let model = query
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .or_else(|| execution.plan.test_model.clone())
        .unwrap_or_else(|| provider_test_model(stored.app, stored, None, None));
    let gemini_path = default_gemini_test_path(stored.app, &model, requested_stream);
    let grok_reconciliation_before = grok_reconciliation_snapshot(state, &execution).await;
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
                reconciliation: grok_reconciliation_report(
                    &execution,
                    grok_reconciliation_before.as_ref(),
                    grok_reconciliation_before.as_ref(),
                    true,
                    false,
                    None,
                    None,
                    None,
                    false,
                ),
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
            reconciliation: grok_reconciliation_report(
                &execution,
                grok_reconciliation_before.as_ref(),
                grok_reconciliation_before.as_ref(),
                true,
                false,
                None,
                None,
                None,
                false,
            ),
            message,
        });
    }
    if let Some(snapshot) = grok_reconciliation_before.as_ref() {
        let blocking = snapshot.binding_status != GrokBindingStatus::Current
            || matches!(
                snapshot.credential_reason.as_str(),
                "account_requires_relogin"
                    | "access_and_refresh_token_missing"
                    | "expired_access_token_without_refresh"
            );
        let dry_run_actionable = !query.network.unwrap_or(false)
            && snapshot.credential_action != GrokCredentialAction::None;
        if blocking || dry_run_actionable {
            let outcome = if blocking {
                match snapshot.binding_status {
                    GrokBindingStatus::Current => ProviderOperationOutcome::Auth,
                    _ => ProviderOperationOutcome::InvalidConfig,
                }
            } else {
                ProviderOperationOutcome::Success
            };
            let message = if blocking {
                "Grok Provider binding or credential is not executable; rebind or sign in again"
                    .to_string()
            } else {
                format!(
                    "configuration dry-run completed; planned credential action: {}",
                    snapshot.credential_reason
                )
            };
            return Ok(TestProviderResponse {
                ok: !blocking,
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
                network_error: blocking.then(|| snapshot.credential_reason.clone()),
                reconciliation: grok_reconciliation_report(
                    &execution,
                    Some(snapshot),
                    Some(snapshot),
                    true,
                    false,
                    None,
                    None,
                    None,
                    false,
                ),
                message,
            });
        }
    }

    if execution.driver_is("special.cursor") {
        return test_cursor_provider_inner(
            state,
            execution,
            query,
            route,
            gemini_path,
            model,
            requested_stream,
        )
        .await;
    }

    if execution.driver_is("special.deepseek_account") {
        return test_deepseek_provider_inner(state, execution, query, model, requested_stream)
            .await;
    }

    if query.network.unwrap_or(false) {
        ensure_provider_outbound_allowed(state, &execution).await?;
        if let Some((provider_type, account_id, expected_generation)) =
            execution.managed_account_identity_target()
        {
            let refresh_result = state
                .refresh_managed_account_if_needed_for_generation(
                    provider_type,
                    account_id,
                    expected_generation,
                )
                .await;
            if let Err(error) = refresh_result {
                if provider_type != ProviderType::GrokOAuth {
                    return Err(map_managed_account_refresh_error(error));
                }
                let outcome = managed_account_refresh_failure_outcome(&error);
                let public_error = map_managed_account_refresh_error(error);
                let message = public_error.message;
                let status = public_error.status.as_u16();
                let grok_reconciliation_after =
                    grok_reconciliation_snapshot(state, &execution).await;
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
                    reconciliation: grok_reconciliation_report(
                        &execution,
                        grok_reconciliation_before.as_ref(),
                        grok_reconciliation_after.as_ref(),
                        false,
                        false,
                        Some(status),
                        None,
                        Some(&message),
                        false,
                    ),
                    message,
                });
            }
            if matches!(
                provider_type,
                ProviderType::GeminiCli | ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
            ) {
                state
                    .ensure_gemini_v1internal_project_for_generation(
                        provider_type,
                        account_id,
                        expected_generation,
                    )
                    .await
                    .map_err(map_managed_account_refresh_error)?;
            }
        }
        ensure_provider_outbound_allowed(state, &execution).await?;
    }
    let accounts = state.accounts_snapshot().await;
    let body = provider_test_body(
        stored.app,
        stored,
        Some(&model),
        test_prompt,
        requested_stream,
    );
    let (adapter_request, endpoint, mut target_headers) = if execution
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
            .apply_auth(&mut target_headers, &mut endpoint, &materialized_auth)
            .map_err(ApiError::proxy)?;
        execution
            .finalize_outbound_identity(&mut target_headers)
            .map_err(ApiError::proxy)?;
        execution
            .finalize_protocol_auth(
                &accounts,
                &mut adapter_request,
                &mut endpoint,
                &mut target_headers,
            )
            .map_err(ApiError::proxy)?;
        execution
            .guard_coding_plan_request(route, &adapter_request, &endpoint)
            .map_err(ApiError::proxy)?;
        (adapter_request, endpoint, target_headers)
    };
    let stream = adapter_request.upstream_stream_requested || requested_stream;
    let mut network_status_code = None;
    let mut network_latency_ms = None;
    let mut network_error = None;
    let mut network_stream_completed = None;
    let mut grok_model_rejected = false;
    if query.network.unwrap_or(false) {
        let started = std::time::Instant::now();
        let http_client = state.http_client().await;
        let mut client_headers = HeaderMap::new();
        if stream {
            client_headers.insert(
                axum::http::header::ACCEPT,
                HeaderValue::from_static("text/event-stream"),
            );
        }
        if execution.driver_is("oauth.openai_codex") {
            target_headers.push(("accept-encoding".to_string(), "identity".to_string()));
        }
        let timeout = query
            .timeout_ms
            .filter(|value| *value > 0)
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| execution.request_timeout());
        let request = proxy::outbound_request::build_post_request(
            &http_client,
            proxy::outbound_request::OutboundPostRequest {
                url: &endpoint,
                body: adapter_request.body.clone(),
                client_headers: &client_headers,
                target_headers: &target_headers,
                default_accept: "*/*",
                default_content_type: "application/json",
                timeout,
                stream_requested: false,
            },
        )
        .map_err(ApiError::proxy)?;
        match request.send().await {
            Ok(mut response) => {
                let status = response.status().as_u16();
                network_status_code = Some(status);
                network_latency_ms = Some(started.elapsed().as_millis());
                if !response.status().is_success() {
                    match read_provider_control_response_body(
                        &mut response,
                        PROVIDER_TEST_RESPONSE_BODY_LIMIT_BYTES,
                    )
                    .await
                    {
                        Ok(body) => {
                            grok_model_rejected = grok_probe_model_rejected(status, body.as_ref());
                            network_error =
                                Some(redact_provider_test_error(&String::from_utf8_lossy(&body)));
                        }
                        Err(error) => network_error = Some(error),
                    }
                } else if stream {
                    match read_provider_control_response_body(
                        &mut response,
                        PROVIDER_TEST_RESPONSE_BODY_LIMIT_BYTES,
                    )
                    .await
                    {
                        Ok(body) => {
                            let completed = provider_test_stream_completed(
                                stored.app,
                                &String::from_utf8_lossy(&body),
                            );
                            network_stream_completed = Some(completed);
                            if !completed {
                                network_error = Some(
                                    "stream probe did not observe a provider completion marker"
                                        .to_string(),
                                );
                            }
                        }
                        Err(error) => {
                            network_stream_completed = Some(false);
                            network_error = Some(error);
                        }
                    }
                }
            }
            Err(error) => {
                network_error = Some(redact_provider_test_error(&error.to_string()));
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
    let grok_reconciliation_after = grok_reconciliation_snapshot(state, &execution).await;

    let reconciliation = grok_reconciliation_report(
        &execution,
        grok_reconciliation_before.as_ref(),
        grok_reconciliation_after.as_ref(),
        !query.network.unwrap_or(false),
        query.network.unwrap_or(false),
        network_status_code,
        network_stream_completed,
        network_error.as_deref(),
        grok_model_rejected,
    );
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
        reconciliation,
        message,
    })
}

async fn test_cursor_provider_inner(
    state: &ServerState,
    execution: proxy::provider_ops::ProviderExecution,
    query: &TestProviderQuery,
    route: ProxyRoute,
    gemini_path: Option<String>,
    model: String,
    requested_stream: bool,
) -> Result<TestProviderResponse, ApiError> {
    let stored = execution.runtime_stored_view();
    let body = provider_test_body(
        stored.app,
        &stored,
        Some(&model),
        query.test_prompt.as_deref().unwrap_or("ping"),
        requested_stream,
    );
    let accounts = state.accounts_snapshot().await;
    if let Err(error) = validate_cursor_provider_test_configuration(
        &execution,
        &accounts,
        route,
        gemini_path.as_deref(),
        &body,
    ) {
        let message = redact_provider_test_error(&error.message);
        return Ok(cursor_provider_test_response(
            &execution,
            &model,
            requested_stream,
            false,
            None,
            None,
            None,
            Some(message.clone()),
            cursor_provider_preflight_outcome(error.status),
            message,
        ));
    }

    if !query.network.unwrap_or(false) {
        return Ok(cursor_provider_test_response(
            &execution,
            &model,
            requested_stream,
            false,
            None,
            None,
            None,
            None,
            ProviderOperationOutcome::Success,
            "configuration check passed; Cursor runtime secrets and exact credential binding are valid; upstream network/model call is not executed".to_string(),
        ));
    }

    if let Err(error) = ensure_provider_outbound_allowed(state, &execution).await {
        let message = redact_provider_test_error(&error.message);
        return Ok(cursor_provider_test_response(
            &execution,
            &model,
            requested_stream,
            false,
            None,
            None,
            None,
            Some(message.clone()),
            ProviderOperationOutcome::InvalidConfig,
            message,
        ));
    }
    if let Some((provider_type, account_id, expected_generation)) =
        execution.managed_account_identity_target()
    {
        if let Err(error) = state
            .refresh_managed_account_if_needed_for_generation(
                provider_type,
                account_id,
                expected_generation,
            )
            .await
        {
            let outcome = managed_account_refresh_failure_outcome(&error);
            let error = map_managed_account_refresh_error(error);
            let message = redact_provider_test_error(&error.message);
            return Ok(cursor_provider_test_response(
                &execution,
                &model,
                requested_stream,
                false,
                None,
                None,
                None,
                Some(message.clone()),
                outcome,
                message,
            ));
        }
    }
    if let Err(error) = ensure_provider_outbound_allowed(state, &execution).await {
        let message = redact_provider_test_error(&error.message);
        return Ok(cursor_provider_test_response(
            &execution,
            &model,
            requested_stream,
            false,
            None,
            None,
            None,
            Some(message.clone()),
            ProviderOperationOutcome::InvalidConfig,
            message,
        ));
    }

    let refreshed_accounts = state.accounts_snapshot().await;
    if let Err(error) = execution.materialize_auth(&refreshed_accounts) {
        let message = redact_provider_test_error(&error.message);
        return Ok(cursor_provider_test_response(
            &execution,
            &model,
            requested_stream,
            false,
            None,
            None,
            None,
            Some(message.clone()),
            cursor_provider_preflight_outcome(error.status),
            message,
        ));
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if requested_stream {
        headers.insert(
            axum::http::header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );
    }
    let timeout = query
        .timeout_ms
        .filter(|value| *value > 0)
        .map(std::time::Duration::from_millis)
        .unwrap_or_else(|| execution.request_timeout());
    let deadline = tokio::time::Instant::now() + timeout;
    let started = std::time::Instant::now();
    let forward = proxy::forward_provider_test(
        state.clone(),
        route,
        execution.clone(),
        gemini_path,
        headers,
        Bytes::from(body),
    );
    let response = match tokio::time::timeout_at(deadline, forward).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            let status = error.status.as_u16();
            let message = redact_provider_test_error(&error.message);
            let outcome = provider_test_outcome(true, Some(status), Some(&message));
            return Ok(cursor_provider_test_response(
                &execution,
                &model,
                requested_stream,
                true,
                Some(status),
                Some(started.elapsed().as_millis()),
                None,
                Some(message.clone()),
                outcome,
                message,
            ));
        }
        Err(_) => {
            let message = "Cursor provider test timed out".to_string();
            return Ok(cursor_provider_test_response(
                &execution,
                &model,
                requested_stream,
                true,
                None,
                Some(started.elapsed().as_millis()),
                requested_stream.then_some(false),
                Some(message.clone()),
                ProviderOperationOutcome::Timeout,
                message,
            ));
        }
    };
    let status = response.status().as_u16();
    let response_body = match tokio::time::timeout_at(
        deadline,
        axum::body::to_bytes(
            response.into_body(),
            PROVIDER_TEST_RESPONSE_BODY_LIMIT_BYTES,
        ),
    )
    .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(error)) => {
            let message = redact_provider_test_error(&format!(
                "failed to read Cursor provider test response: {error}"
            ));
            let outcome = provider_test_outcome(true, Some(status), Some(&message));
            return Ok(cursor_provider_test_response(
                &execution,
                &model,
                requested_stream,
                true,
                Some(status),
                Some(started.elapsed().as_millis()),
                requested_stream.then_some(false),
                Some(message.clone()),
                outcome,
                message,
            ));
        }
        Err(_) => {
            let message = "Cursor provider test timed out while reading the response".to_string();
            return Ok(cursor_provider_test_response(
                &execution,
                &model,
                requested_stream,
                true,
                Some(status),
                Some(started.elapsed().as_millis()),
                requested_stream.then_some(false),
                Some(message.clone()),
                ProviderOperationOutcome::Timeout,
                message,
            ));
        }
    };

    let mut network_error = None;
    let mut network_stream_completed = None;
    if !(200..=399).contains(&status) {
        let detail = redact_provider_test_error(&String::from_utf8_lossy(&response_body));
        network_error = Some(if detail.trim().is_empty() {
            format!("Cursor provider returned HTTP {status}")
        } else {
            detail
        });
    } else if requested_stream {
        let completed =
            provider_test_stream_completed(stored.app, &String::from_utf8_lossy(&response_body));
        network_stream_completed = Some(completed);
        if !completed {
            network_error =
                Some("stream probe did not observe a provider completion marker".to_string());
        }
    }
    let outcome = provider_test_outcome(true, Some(status), network_error.as_deref());
    let ok = outcome == ProviderOperationOutcome::Success;
    let message = if ok {
        "configuration check passed; Cursor native upstream network/model call executed".to_string()
    } else {
        network_error
            .clone()
            .unwrap_or_else(|| format!("provider test outcome: {outcome:?}"))
    };
    Ok(cursor_provider_test_response(
        &execution,
        &model,
        requested_stream,
        true,
        Some(status),
        Some(started.elapsed().as_millis()),
        network_stream_completed,
        network_error,
        outcome,
        message,
    ))
}

async fn test_deepseek_provider_inner(
    state: &ServerState,
    execution: proxy::provider_ops::ProviderExecution,
    query: &TestProviderQuery,
    model: String,
    requested_stream: bool,
) -> Result<TestProviderResponse, ApiError> {
    let stored = execution.runtime_stored_view();
    let (account_id, expected_generation) = match execution.managed_account_identity_target() {
        Some((ProviderType::DeepSeekAccount, account_id, expected_generation))
            if stored.app == AppKind::Claude =>
        {
            (account_id.to_string(), expected_generation)
        }
        _ => {
            return Ok(deepseek_provider_test_response(
                &execution,
                &model,
                requested_stream,
                false,
                None,
                None,
                None,
                Some(
                    "DeepSeek provider test requires one explicit managed Account binding"
                        .to_string(),
                ),
                ProviderOperationOutcome::InvalidConfig,
                "DeepSeek provider test requires one explicit managed Account binding".to_string(),
            ));
        }
    };
    let accounts = state.accounts_snapshot().await;
    let account = accounts.accounts.iter().find(|account| {
        account.id == account_id && account.provider_type == ProviderType::DeepSeekAccount
    });
    let account_ready = account.is_some_and(|account| {
        account.auth_identity_generation == expected_generation
            && account
                .access_token
                .as_deref()
                .map(str::trim)
                .is_some_and(|token| !token.is_empty())
    });
    if !account_ready {
        let message =
            "DeepSeek managed Account binding changed or its imported bearer token is unavailable"
                .to_string();
        return Ok(deepseek_provider_test_response(
            &execution,
            &model,
            requested_stream,
            false,
            None,
            None,
            None,
            Some(message.clone()),
            ProviderOperationOutcome::MissingCredential,
            message,
        ));
    }
    if let Err(error) = execution.materialize_auth(&accounts) {
        let message = redact_provider_test_error(&error.message);
        return Ok(deepseek_provider_test_response(
            &execution,
            &model,
            requested_stream,
            false,
            None,
            None,
            None,
            Some(message.clone()),
            provider_test_outcome(false, Some(error.status.as_u16()), Some(&message)),
            message,
        ));
    }

    let deepseek_model = match crate::proxy::deepseek::resolve_model(&model) {
        Ok(model) => model,
        Err(error) => {
            let message = redact_provider_test_error(&error.message);
            return Ok(deepseek_provider_test_response(
                &execution,
                &model,
                requested_stream,
                false,
                None,
                None,
                None,
                Some(message.clone()),
                ProviderOperationOutcome::InvalidConfig,
                message,
            ));
        }
    };
    let body = provider_test_body(
        stored.app,
        &stored,
        Some(&model),
        query.test_prompt.as_deref().unwrap_or("ping"),
        requested_stream,
    );
    let request_body: Value = serde_json::from_str(&body).map_err(ApiError::internal)?;
    let prepared = match crate::proxy::deepseek::prepare_request(&request_body, &deepseek_model) {
        Ok(prepared) => prepared,
        Err(error) => {
            let message = redact_provider_test_error(&error.message);
            return Ok(deepseek_provider_test_response(
                &execution,
                &model,
                requested_stream,
                false,
                None,
                None,
                None,
                Some(message.clone()),
                ProviderOperationOutcome::InvalidConfig,
                message,
            ));
        }
    };

    if !query.network.unwrap_or(false) {
        return Ok(deepseek_provider_test_response(
            &execution,
            &model,
            requested_stream,
            false,
            None,
            None,
            None,
            None,
            ProviderOperationOutcome::Success,
            "configuration check passed; DeepSeek fixed origin, reviewed model, bearer-only Account rail, and generation binding are valid; upstream network/model call is not executed".to_string(),
        ));
    }

    if let Err(error) = ensure_provider_outbound_allowed(state, &execution).await {
        let message = redact_provider_test_error(&error.message);
        return Ok(deepseek_provider_test_response(
            &execution,
            &model,
            requested_stream,
            false,
            None,
            None,
            None,
            Some(message.clone()),
            ProviderOperationOutcome::InvalidConfig,
            message,
        ));
    }
    let timeout = query
        .timeout_ms
        .filter(|value| *value > 0)
        .map(std::time::Duration::from_millis)
        .unwrap_or_else(|| execution.request_timeout());
    let deadline = tokio::time::Instant::now() + timeout;
    let started = std::time::Instant::now();
    let client_session_id = crate::proxy::deepseek::request_scoped_session_id();
    let provider_test_scope = format!("provider-test:{}", stored.provider.id);
    let upstream = state.start_deepseek_chat_completion(
        stored.app,
        &stored.provider.id,
        stored.resource.revision,
        &execution.plan.runtime_fingerprint,
        &account_id,
        expected_generation,
        &provider_test_scope,
        "provider-test",
        &client_session_id,
        &deepseek_model,
        &prepared.prompt,
        prepared.thinking_enabled,
        prepared.search_enabled,
    );
    let mut response = match tokio::time::timeout_at(deadline, upstream).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            let (outcome, status, message) = deepseek_provider_test_upstream_error(error);
            return Ok(deepseek_provider_test_response(
                &execution,
                &model,
                requested_stream,
                true,
                status,
                Some(started.elapsed().as_millis()),
                requested_stream.then_some(false),
                Some(message.clone()),
                outcome,
                message,
            ));
        }
        Err(_) => {
            let message = "DeepSeek provider test timed out before response headers".to_string();
            return Ok(deepseek_provider_test_response(
                &execution,
                &model,
                requested_stream,
                true,
                None,
                Some(started.elapsed().as_millis()),
                requested_stream.then_some(false),
                Some(message.clone()),
                ProviderOperationOutcome::Timeout,
                message,
            ));
        }
    };
    let status = response.status().as_u16();
    let response_body = match tokio::time::timeout_at(
        deadline,
        read_provider_control_response_body(&mut response, PROVIDER_TEST_RESPONSE_BODY_LIMIT_BYTES),
    )
    .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(message)) => {
            let message = redact_provider_test_error(&message);
            let outcome = provider_test_outcome(true, Some(status), Some(&message));
            return Ok(deepseek_provider_test_response(
                &execution,
                &model,
                requested_stream,
                true,
                Some(status),
                Some(started.elapsed().as_millis()),
                requested_stream.then_some(false),
                Some(message.clone()),
                outcome,
                message,
            ));
        }
        Err(_) => {
            let message = "DeepSeek provider test timed out while reading the response".to_string();
            return Ok(deepseek_provider_test_response(
                &execution,
                &model,
                requested_stream,
                true,
                Some(status),
                Some(started.elapsed().as_millis()),
                requested_stream.then_some(false),
                Some(message.clone()),
                ProviderOperationOutcome::Timeout,
                message,
            ));
        }
    };

    if !(200..=299).contains(&status) {
        let detail = redact_provider_test_error(&String::from_utf8_lossy(&response_body));
        let message = if detail.trim().is_empty() {
            format!("DeepSeek provider returned HTTP {status}")
        } else {
            detail
        };
        let outcome = provider_test_outcome(true, Some(status), Some(&message));
        return Ok(deepseek_provider_test_response(
            &execution,
            &model,
            requested_stream,
            true,
            Some(status),
            Some(started.elapsed().as_millis()),
            requested_stream.then_some(false),
            Some(message.clone()),
            outcome,
            message,
        ));
    }

    let collected = crate::proxy::deepseek::collect_deepseek_response(
        futures_util::stream::iter(vec![Ok::<_, reqwest::Error>(response_body)]),
        prepared.thinking_enabled,
    )
    .await;
    let network_error = collected
        .and_then(|collected| {
            crate::proxy::deepseek::finalize_collected_response(collected, &prepared)
        })
        .err()
        .map(|error| redact_provider_test_error(&error.message));
    let outcome = provider_test_outcome(true, Some(status), network_error.as_deref());
    let message = network_error.clone().unwrap_or_else(|| {
        "configuration check passed; DeepSeek native session, PoW, terminal stream, and bound Account network/model call executed".to_string()
    });
    Ok(deepseek_provider_test_response(
        &execution,
        &model,
        requested_stream,
        true,
        Some(status),
        Some(started.elapsed().as_millis()),
        requested_stream.then_some(network_error.is_none()),
        network_error,
        outcome,
        message,
    ))
}

fn deepseek_provider_test_upstream_error(
    error: crate::state::DeepSeekUpstreamError,
) -> (ProviderOperationOutcome, Option<u16>, String) {
    use crate::state::DeepSeekUpstreamError;
    let (outcome, status, message) = match error {
        DeepSeekUpstreamError::NotFound => (
            ProviderOperationOutcome::InvalidConfig,
            None,
            "DeepSeek managed Account was not found".to_string(),
        ),
        DeepSeekUpstreamError::IdentityChanged => (
            ProviderOperationOutcome::InvalidConfig,
            None,
            "DeepSeek Provider or Account identity changed during the test".to_string(),
        ),
        DeepSeekUpstreamError::CredentialPersistenceDegraded => (
            ProviderOperationOutcome::InvalidConfig,
            None,
            "DeepSeek managed credential persistence is degraded".to_string(),
        ),
        DeepSeekUpstreamError::MissingToken => (
            ProviderOperationOutcome::MissingCredential,
            None,
            "DeepSeek imported bearer token is missing".to_string(),
        ),
        DeepSeekUpstreamError::Network { operation, message } => (
            ProviderOperationOutcome::Network,
            None,
            format!("DeepSeek {operation} network error: {message}"),
        ),
        DeepSeekUpstreamError::Upstream {
            operation,
            status_code,
            message,
        } => {
            let message = format!("DeepSeek {operation} returned HTTP {status_code}: {message}");
            (
                provider_test_outcome(true, Some(status_code), Some(&message)),
                Some(status_code),
                message,
            )
        }
        DeepSeekUpstreamError::Protocol { operation, message } => (
            ProviderOperationOutcome::Protocol,
            None,
            format!("DeepSeek {operation} protocol error: {message}"),
        ),
    };
    (outcome, status, redact_provider_test_error(&message))
}

#[allow(clippy::too_many_arguments)]
fn deepseek_provider_test_response(
    execution: &proxy::provider_ops::ProviderExecution,
    model: &str,
    stream: bool,
    network_checked: bool,
    network_status_code: Option<u16>,
    network_latency_ms: Option<u128>,
    network_stream_completed: Option<bool>,
    network_error: Option<String>,
    outcome: ProviderOperationOutcome,
    message: String,
) -> TestProviderResponse {
    let stored = execution.runtime_stored_view();
    let capability = proxy::adapters::adapter_for(stored.app, stored.provider_type)
        .capability(stored.app, stored.provider_type);
    TestProviderResponse {
        ok: outcome == ProviderOperationOutcome::Success,
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
        model: model.to_string(),
        stream,
        header_names: vec![
            "authorization".to_string(),
            "x-ds-pow-response".to_string(),
            "x-client-platform".to_string(),
            "x-client-version".to_string(),
        ],
        network_checked,
        network_status_code,
        network_latency_ms,
        network_stream_completed,
        network_error,
        reconciliation: None,
        message,
    }
}

fn validate_cursor_provider_test_configuration(
    execution: &proxy::provider_ops::ProviderExecution,
    accounts: &crate::domain::accounts::store::AccountStore,
    route: ProxyRoute,
    gemini_path: Option<&str>,
    body: &str,
) -> Result<(), proxy::ProxyError> {
    let stored = execution.runtime_stored_view();
    proxy::cursor::validate_runtime_configuration(&stored)?;
    let _auth = execution.materialize_auth(accounts)?;
    let mut request = proxy::adapters::cursor_agentservice_request(
        Bytes::copy_from_slice(body.as_bytes()),
        &stored,
        route,
        gemini_path,
    )?;
    execution.enforce_model_policy(&mut request)?;
    proxy::cursor::apply_agentservice_model_selection(&mut request)?;
    if proxy::cursor::build_agent_plan_preview(route, &stored, &request.body)?.is_none() {
        return Err(proxy::ProxyError {
            status: StatusCode::NOT_IMPLEMENTED,
            message: "Cursor AgentService driver does not support the Provider test route"
                .to_string(),
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cursor_provider_test_response(
    execution: &proxy::provider_ops::ProviderExecution,
    model: &str,
    stream: bool,
    network_checked: bool,
    network_status_code: Option<u16>,
    network_latency_ms: Option<u128>,
    network_stream_completed: Option<bool>,
    network_error: Option<String>,
    outcome: ProviderOperationOutcome,
    message: String,
) -> TestProviderResponse {
    let stored = execution.runtime_stored_view();
    let capability = proxy::adapters::adapter_for(stored.app, stored.provider_type)
        .capability(stored.app, stored.provider_type);
    TestProviderResponse {
        ok: outcome == ProviderOperationOutcome::Success,
        outcome,
        driver_id: execution.plan.driver_id.to_string(),
        runtime_fingerprint: execution.plan.runtime_fingerprint.clone(),
        provider_id: stored.provider.id.clone(),
        app: stored.app,
        provider_type: stored.provider_type,
        provider_revision: stored.resource.revision,
        adapter: capability.adapter,
        support: capability.support,
        endpoint: "[runtime secret]".to_string(),
        model: model.to_string(),
        stream,
        header_names: Vec::new(),
        network_checked,
        network_status_code,
        network_latency_ms,
        network_stream_completed,
        network_error,
        reconciliation: None,
        message,
    }
}

fn cursor_provider_preflight_outcome(status: StatusCode) -> ProviderOperationOutcome {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProviderOperationOutcome::Auth,
        StatusCode::TOO_MANY_REQUESTS => ProviderOperationOutcome::RateLimit,
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => {
            ProviderOperationOutcome::Timeout
        }
        _ => ProviderOperationOutcome::InvalidConfig,
    }
}

#[derive(Debug, Clone)]
struct GrokReconciliationSnapshot {
    binding_status: GrokBindingStatus,
    credential_action: GrokCredentialAction,
    credential_reason: String,
    expected_auth_identity_generation: Option<u64>,
    observed_auth_identity_generation: Option<u64>,
    token_refresh_generation: Option<u64>,
    capability_evidence:
        Option<crate::domain::accounts::capability_evidence::AccountCapabilityProjection>,
}

async fn grok_reconciliation_snapshot(
    state: &ServerState,
    execution: &proxy::provider_ops::ProviderExecution,
) -> Option<GrokReconciliationSnapshot> {
    if execution.stored.provider_type != ProviderType::GrokOAuth {
        return None;
    }
    let (account_id, expected_generation) = match &execution.plan.auth_ref {
        crate::domain::providers::runtime::RuntimeAuthRef::ManagedAccount {
            account_id,
            expected_provider_type: ProviderType::GrokOAuth,
            auth_identity_generation,
        } if !account_id.trim().is_empty() => {
            (account_id.as_str(), Some(*auth_identity_generation))
        }
        _ => {
            return Some(GrokReconciliationSnapshot {
                binding_status: GrokBindingStatus::MissingBinding,
                credential_action: GrokCredentialAction::Relogin,
                credential_reason: "managed_account_binding_missing".to_string(),
                expected_auth_identity_generation: None,
                observed_auth_identity_generation: None,
                token_refresh_generation: None,
                capability_evidence: None,
            });
        }
    };
    let Some(account) = state.find_account_by_id(account_id).await else {
        return Some(GrokReconciliationSnapshot {
            binding_status: GrokBindingStatus::MissingAccount,
            credential_action: GrokCredentialAction::Relogin,
            credential_reason: "bound_account_missing".to_string(),
            expected_auth_identity_generation: expected_generation,
            observed_auth_identity_generation: None,
            token_refresh_generation: None,
            capability_evidence: None,
        });
    };
    let binding_status = if account.provider_type != ProviderType::GrokOAuth {
        GrokBindingStatus::ProviderTypeMismatch
    } else if expected_generation != Some(account.auth_identity_generation) {
        GrokBindingStatus::StaleIdentity
    } else {
        GrokBindingStatus::Current
    };
    let now_ms = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    let (credential_action, credential_reason) =
        grok_credential_reconciliation(&account, binding_status, now_ms);
    let capability_evidence =
        crate::domain::accounts::capability_evidence::account_capability_projections(
            &account, now_ms,
        )
        .into_iter()
        .find(|projection| {
            projection.capability
                == crate::domain::accounts::capability_evidence::GROK_CODE_PLAN_CAPABILITY
        });
    Some(GrokReconciliationSnapshot {
        binding_status,
        credential_action,
        credential_reason: credential_reason.to_string(),
        expected_auth_identity_generation: expected_generation,
        observed_auth_identity_generation: Some(account.auth_identity_generation),
        token_refresh_generation: Some(account.token_refresh_generation),
        capability_evidence,
    })
}

fn grok_credential_reconciliation(
    account: &crate::domain::accounts::store::Account,
    binding_status: GrokBindingStatus,
    now_ms: i64,
) -> (GrokCredentialAction, &'static str) {
    if binding_status != GrokBindingStatus::Current {
        return (GrokCredentialAction::Relogin, "binding_not_current");
    }
    if account.needs_relogin {
        return (GrokCredentialAction::Relogin, "account_requires_relogin");
    }
    let access_token_missing = account
        .access_token
        .as_deref()
        .is_none_or(|token| token.trim().is_empty());
    let refresh_token_missing = !crate::clients::oauth::refresh::account_has_refresh_token(account);
    if access_token_missing && refresh_token_missing {
        return (
            GrokCredentialAction::Relogin,
            "access_and_refresh_token_missing",
        );
    }
    if refresh_token_missing {
        if account
            .expires_at
            .is_some_and(|expires_at| expires_at <= now_ms)
        {
            return (
                GrokCredentialAction::Relogin,
                "expired_access_token_without_refresh",
            );
        }
        return (GrokCredentialAction::Relogin, "refresh_token_missing");
    }
    if access_token_missing {
        return (GrokCredentialAction::Refresh, "access_token_missing");
    }
    if account.expires_at.is_none() {
        return (GrokCredentialAction::Refresh, "access_token_expiry_missing");
    }
    if crate::clients::oauth::refresh::account_needs_native_refresh(account, now_ms) {
        return (GrokCredentialAction::Refresh, "access_token_expires_soon");
    }
    (GrokCredentialAction::None, "credentials_current")
}

#[allow(clippy::too_many_arguments)]
fn grok_reconciliation_report(
    execution: &proxy::provider_ops::ProviderExecution,
    before: Option<&GrokReconciliationSnapshot>,
    after: Option<&GrokReconciliationSnapshot>,
    read_only: bool,
    probe_checked: bool,
    network_status_code: Option<u16>,
    network_stream_completed: Option<bool>,
    network_error: Option<&str>,
    model_rejected: bool,
) -> Option<GrokProviderReconciliationReport> {
    let before = before?;
    let after = after.unwrap_or(before);
    let successful_probe = probe_checked
        && network_status_code.is_some_and(|status| (200..300).contains(&status))
        && network_stream_completed.unwrap_or(true)
        && network_error.is_none();
    let model_rejected = probe_checked && model_rejected;
    let endpoint_rejected =
        probe_checked && matches!(network_status_code, Some(404 | 405)) && !model_rejected;
    let status = |unsupported: bool| {
        if !probe_checked {
            GrokProbeCapabilityStatus::NotChecked
        } else if successful_probe {
            GrokProbeCapabilityStatus::Supported
        } else if unsupported {
            GrokProbeCapabilityStatus::Unsupported
        } else {
            GrokProbeCapabilityStatus::Inconclusive
        }
    };
    let endpoint_is_expected = execution.plan.endpoint.trim_end_matches('/')
        == crate::proxy::grok_fixed_api_base_url().trim_end_matches('/');
    let endpoint_policy_status = if endpoint_is_expected
        && execution
            .plan
            .warnings
            .iter()
            .any(|warning| warning.contains("fixed endpoint policy ignored"))
    {
        GrokEndpointPolicyStatus::ConfiguredOverrideIgnored
    } else if endpoint_is_expected {
        GrokEndpointPolicyStatus::Expected
    } else {
        GrokEndpointPolicyStatus::UnexpectedRuntimeEndpoint
    };
    let identity_stable = before.binding_status == GrokBindingStatus::Current
        && after.binding_status == GrokBindingStatus::Current
        && before.expected_auth_identity_generation == before.observed_auth_identity_generation
        && before.observed_auth_identity_generation == after.observed_auth_identity_generation;
    let refresh_performed = identity_stable
        && before
            .token_refresh_generation
            .zip(after.token_refresh_generation)
            .is_some_and(|(before, after)| after > before);
    Some(GrokProviderReconciliationReport {
        read_only,
        binding_status_before: before.binding_status,
        binding_status_after: after.binding_status,
        planned_credential_action: before.credential_action,
        planned_credential_reason: before.credential_reason.clone(),
        remaining_credential_action: after.credential_action,
        remaining_credential_reason: after.credential_reason.clone(),
        expected_auth_identity_generation: before.expected_auth_identity_generation,
        observed_auth_identity_generation_before: before.observed_auth_identity_generation,
        observed_auth_identity_generation_after: after.observed_auth_identity_generation,
        token_refresh_generation_before: before.token_refresh_generation,
        token_refresh_generation_after: after.token_refresh_generation,
        refresh_performed,
        identity_stable,
        endpoint_policy_status,
        responses_endpoint_status: status(endpoint_rejected),
        model_status: status(model_rejected),
        capability_evidence: after.capability_evidence.clone(),
    })
}

fn grok_probe_model_rejected(status: u16, body: &[u8]) -> bool {
    if !matches!(status, 400 | 403 | 404 | 422) || body.is_empty() {
        return false;
    }
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    let mut evidence = Vec::new();
    for pointer in [
        "/error/code",
        "/error/type",
        "/error/message",
        "/code",
        "/type",
        "/message",
        "/detail",
    ] {
        if let Some(value) = payload.pointer(pointer).and_then(serde_json::Value::as_str) {
            let normalized = value.trim().to_ascii_lowercase().replace(['-', ' '], "_");
            if !normalized.is_empty() {
                evidence.push(normalized);
            }
        }
    }
    evidence.iter().any(|value| {
        matches!(
            value.as_str(),
            "model_not_found"
                | "unknown_model"
                | "unsupported_model"
                | "model_unsupported"
                | "invalid_model"
        ) || value.contains("model_not_found")
            || value.contains("model_was_not_found")
            || value.contains("requested_model_not_found")
            || value.contains("unknown_model")
            || value.contains("unsupported_model")
            || value.contains("model_is_not_supported")
            || value.contains("does_not_support_model")
            || value.contains("model_does_not_exist")
            || value.contains("invalid_model")
    })
}

fn managed_account_refresh_failure_outcome(
    error: &crate::state::ManagedAccountRefreshError,
) -> ProviderOperationOutcome {
    use crate::state::ManagedAccountRefreshError;

    match error {
        ManagedAccountRefreshError::Conflict { .. }
        | ManagedAccountRefreshError::InactiveCodexAccount
        | ManagedAccountRefreshError::IdentityChanged { .. }
        | ManagedAccountRefreshError::NotFound => ProviderOperationOutcome::InvalidConfig,
        ManagedAccountRefreshError::CredentialPersistenceDegraded => {
            ProviderOperationOutcome::Upstream
        }
        ManagedAccountRefreshError::Refresh { status_code, .. } => match status_code {
            400 | 401 | 403 => ProviderOperationOutcome::Auth,
            408 | 504 => ProviderOperationOutcome::Timeout,
            429 => ProviderOperationOutcome::RateLimit,
            500..=599 => ProviderOperationOutcome::Upstream,
            _ => ProviderOperationOutcome::Protocol,
        },
    }
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
            408 | 504 => ProviderOperationOutcome::Timeout,
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

#[derive(Debug)]
pub(in crate::api) struct ProviderModelsFetchResult {
    url: String,
    models: Vec<FetchedProviderModel>,
    source: Option<String>,
    stale: Option<bool>,
    fetched_at_ms: Option<i64>,
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
    if let Some(contract) = execution.plan.coding_plan.as_ref() {
        return Ok(ProviderModelsFetchResult {
            url: format!("registry://{}/models", execution.plan.profile_id),
            models: contract
                .models
                .iter()
                .map(|model| FetchedProviderModel {
                    id: model.id.clone(),
                    upstream_model: model.id.clone(),
                    display_name: Some(model.display_name.clone()),
                    raw: serde_json::to_value(model).unwrap_or(Value::Null),
                })
                .collect(),
            source: Some("registry_coding_plan".to_string()),
            stale: Some(false),
            fetched_at_ms: chrono::DateTime::parse_from_rfc3339(&contract.pricing.captured_at)
                .ok()
                .map(|value| value.timestamp_millis()),
        });
    }
    if stored.provider_type == ProviderType::ClaudeOAuth
        && execution.driver_is("oauth.claude_messages")
    {
        return Ok(claude_provider_models_fetch_result(
            crate::clients::oauth::claude_models::static_claude_model_catalog(),
        ));
    }
    if crate::proxy::web_session::is_web_session_driver(execution.plan.driver_id.as_str()) {
        let models = crate::proxy::web_session::reviewed_models_for_driver(
            execution.plan.driver_id.as_str(),
        )
        .ok_or_else(|| ApiError::bad_request("Web Session Driver has no reviewed model catalog"))?;
        return Ok(ProviderModelsFetchResult {
            url: format!("registry://{}/models", execution.plan.profile_id),
            models: models
                .iter()
                .map(|model| FetchedProviderModel {
                    id: model.id.to_string(),
                    upstream_model: model.id.to_string(),
                    display_name: Some(model.display_name.to_string()),
                    raw: serde_json::json!({
                        "id": model.id,
                        "source": "reviewed_web_session_catalog",
                        "fixtureState": "fixture_verified",
                        "liveState": "live_pending",
                        "entitlement": "not_asserted",
                        "capabilities": {
                            "stream": true,
                            "tools": false,
                            "images": false,
                            "inputModalities": ["text"],
                            "outputModalities": ["text"]
                        }
                    }),
                })
                .collect(),
            source: Some("reviewed_web_session_catalog".to_string()),
            stale: Some(false),
            fetched_at_ms: None,
        });
    }
    ensure_provider_outbound_allowed(state, execution).await?;
    if stored.provider_type == ProviderType::DeepSeekAccount {
        let (account_id, expected_generation) = match &execution.plan.auth_ref {
            crate::domain::providers::runtime::RuntimeAuthRef::ManagedAccount {
                account_id,
                expected_provider_type: ProviderType::DeepSeekAccount,
                auth_identity_generation,
            } if execution.driver_is("special.deepseek_account")
                && !account_id.trim().is_empty() =>
            {
                (account_id.as_str(), *auth_identity_generation)
            }
            _ => {
                return Err(ApiError::bad_request(
                    "deepseek_account model discovery requires one explicit managed account binding",
                ));
            }
        };
        let account = state
            .find_account_for_provider(ProviderType::DeepSeekAccount, account_id)
            .await
            .filter(|account| account.auth_identity_generation == expected_generation)
            .ok_or_else(|| {
                ApiError::conflict(
                    "deepseek_account model discovery binding changed; rebind the Provider",
                )
            })?;
        if account
            .access_token
            .as_deref()
            .map(str::trim)
            .is_none_or(|token| token.is_empty())
        {
            return Err(ApiError::unauthorized(
                "deepseek_account model discovery requires the imported bearer token",
            ));
        }
        return Ok(ProviderModelsFetchResult {
            url: format!("registry://{}/models", execution.plan.profile_id),
            models: crate::proxy::deepseek::reviewed_model_catalog()
                .iter()
                .map(|model| FetchedProviderModel {
                    id: model.id.to_string(),
                    upstream_model: model.id.to_string(),
                    display_name: Some(model.display_name.to_string()),
                    raw: serde_json::json!({
                        "id": model.id,
                        "source": "reviewed_deepseek_web_catalog",
                        "entitlement": "live_pending",
                        "capabilities": {
                            "thinking": model.thinking,
                            "search": model.search,
                            "tools": true,
                            "inputModalities": ["text"]
                        }
                    }),
                })
                .collect(),
            source: Some("reviewed_deepseek_web_catalog".to_string()),
            stale: Some(false),
            fetched_at_ms: None,
        });
    }
    if stored.provider_type == ProviderType::GitHubCopilot {
        let (account_id, expected_generation) = match &execution.plan.auth_ref {
            crate::domain::providers::runtime::RuntimeAuthRef::ManagedAccount {
                account_id,
                expected_provider_type: ProviderType::GitHubCopilot,
                auth_identity_generation,
            } if execution.driver_is("special.copilot") && !account_id.trim().is_empty() => {
                (account_id.as_str(), *auth_identity_generation)
            }
            _ => {
                return Err(ApiError::bad_request(
                    "github_copilot model discovery requires one explicit managed account binding",
                ));
            }
        };
        let timeout = timeout_ms
            .filter(|value| *value > 0)
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| execution.request_timeout());
        #[cfg(test)]
        let models_url_override = execution
            .plan
            .driver_options
            .get("testCopilotModelsUrl")
            .and_then(Value::as_str);
        let catalog = state
            .copilot_model_catalog(
                account_id,
                expected_generation,
                timeout,
                #[cfg(test)]
                models_url_override,
            )
            .await
            .map_err(copilot_model_catalog_error_to_api_error)?;
        return Ok(copilot_provider_models_fetch_result(catalog));
    }
    if stored.provider_type == ProviderType::GeminiCli {
        let (account_id, expected_generation) = match &execution.plan.auth_ref {
            crate::domain::providers::runtime::RuntimeAuthRef::ManagedAccount {
                account_id,
                expected_provider_type: ProviderType::GeminiCli,
                auth_identity_generation,
            } if execution.driver_is("oauth.gemini_code_assist")
                && !account_id.trim().is_empty() =>
            {
                (account_id.as_str(), *auth_identity_generation)
            }
            _ => {
                return Err(ApiError::bad_request(
                    "gemini_cli model discovery requires one explicit managed account binding",
                ));
            }
        };
        let refresh = state
            .refresh_bound_account_quota_for_generation(
                ProviderType::GeminiCli,
                account_id,
                expected_generation,
                true,
            )
            .await;
        let (account, force_stale) = match refresh {
            Ok(account) => (account, false),
            Err(error) => {
                tracing::warn!(
                    account_id,
                    error = %error,
                    "Gemini Code Assist model discovery quota refresh failed"
                );
                if !error
                    .downcast_ref::<crate::state::BoundAccountQuotaRefreshFailure>()
                    .is_some_and(|failure| failure.allows_same_generation_stale_catalog())
                {
                    return Err(ApiError::bad_gateway(
                        "Gemini Code Assist model discovery failed for the bound account",
                    ));
                }
                let account = state
                    .find_account_for_provider(ProviderType::GeminiCli, account_id)
                    .await
                    .filter(|account| account.auth_identity_generation == expected_generation)
                    .ok_or_else(|| {
                        ApiError::bad_gateway(
                            "Gemini Code Assist model discovery failed for the bound account",
                        )
                    })?;
                (account, true)
            }
        };
        let max_age_ms = state
            .oauth_quota_refresh_interval_ms()
            .await
            .saturating_mul(2)
            .max(60_000);
        let catalog = crate::clients::oauth::gemini_models::model_catalog_from_account(
            &account,
            crate::infra::time::now_ms() as i64,
            max_age_ms,
            force_stale,
        );
        if catalog.descriptors.is_empty() && catalog.stale {
            return Err(ApiError::bad_gateway(
                "Gemini Code Assist model discovery has no current bound-account catalog",
            ));
        }
        return Ok(gemini_provider_models_fetch_result(catalog));
    }
    if matches!(
        stored.provider_type,
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
    ) {
        let provider_type = stored.provider_type;
        let expected_driver = match provider_type {
            ProviderType::AntigravityOAuth => "special.antigravity",
            ProviderType::AgyOAuth => "special.agy",
            _ => unreachable!("provider type was matched above"),
        };
        let (account_id, expected_generation) = match &execution.plan.auth_ref {
            crate::domain::providers::runtime::RuntimeAuthRef::ManagedAccount {
                account_id,
                expected_provider_type,
                auth_identity_generation,
            } if *expected_provider_type == provider_type
                && execution.driver_is(expected_driver)
                && !account_id.trim().is_empty() =>
            {
                (account_id.as_str(), *auth_identity_generation)
            }
            _ => {
                return Err(ApiError::bad_request(format!(
                    "{} model discovery requires one explicit managed account binding",
                    provider_type.as_str()
                )));
            }
        };
        state
            .refresh_managed_account_if_needed_for_generation(
                provider_type,
                account_id,
                expected_generation,
            )
            .await
            .map_err(map_managed_account_refresh_error)?;
        state
            .ensure_gemini_v1internal_project_for_generation(
                provider_type,
                account_id,
                expected_generation,
            )
            .await
            .map_err(map_managed_account_refresh_error)?;
        let timeout = timeout_ms
            .filter(|value| *value > 0)
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| execution.request_timeout());
        #[cfg(test)]
        let endpoint_override = execution
            .plan
            .driver_options
            .get("testAntigravityModelsUrl")
            .and_then(Value::as_str)
            .map(str::to_string);
        #[cfg(not(test))]
        let endpoint_override: Option<String> = None;
        let mut account = state
            .find_account_for_provider(provider_type, account_id)
            .await
            .filter(|account| account.auth_identity_generation == expected_generation)
            .ok_or_else(|| {
                ApiError::conflict("bound Antigravity account identity changed during discovery")
            })?;
        #[cfg(test)]
        let endpoint_override = endpoint_override.or_else(|| {
            account.raw.as_ref().and_then(|raw| {
                raw.get("testAntigravityModelsUrl")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        raw.get("testGeminiCodeAssistBaseUrl")
                            .and_then(Value::as_str)
                            .map(|base| {
                                format!(
                                    "{}/v1internal:fetchAvailableModels",
                                    base.trim_end_matches('/')
                                )
                            })
                    })
            })
        });
        let http = state.http_client().await;
        let mut catalog = crate::clients::oauth::antigravity_models::model_catalog(
            &http,
            &account,
            timeout,
            endpoint_override.as_deref(),
        )
        .await;
        if catalog.as_ref().is_err_and(|error| error.is_unauthorized())
            && !account.needs_relogin
            && account
                .refresh_token
                .as_deref()
                .is_some_and(|token| !token.trim().is_empty())
        {
            state
                .refresh_managed_account_now_for_generation(
                    provider_type,
                    account_id,
                    expected_generation,
                )
                .await
                .map_err(map_managed_account_refresh_error)?;
            account = state
                .find_account_for_provider(provider_type, account_id)
                .await
                .filter(|account| account.auth_identity_generation == expected_generation)
                .ok_or_else(|| {
                    ApiError::conflict(
                        "bound Antigravity account identity changed during auth recovery",
                    )
                })?;
            catalog = crate::clients::oauth::antigravity_models::model_catalog(
                &http,
                &account,
                timeout,
                endpoint_override.as_deref(),
            )
            .await;
        }
        let catalog = catalog.map_err(|error| {
            tracing::warn!(
                account_id,
                provider_type = provider_type.as_str(),
                status_code = error.status_code,
                retryable = error.retryable,
                error = %error,
                "Antigravity model discovery failed"
            );
            ApiError::bad_gateway("Antigravity model discovery failed for the bound account")
        })?;
        let current_plan = state
            .provider_runtime_plan(stored.app, &stored.provider.id)
            .await
            .filter(|plan| {
                plan.provider_revision == execution.plan.provider_revision
                    && plan.runtime_fingerprint == execution.plan.runtime_fingerprint
            })
            .ok_or_else(|| {
                ApiError::conflict("Provider binding changed during Antigravity model discovery")
            })?;
        if !matches!(
            &current_plan.auth_ref,
            crate::domain::providers::runtime::RuntimeAuthRef::ManagedAccount {
                account_id: current_account_id,
                expected_provider_type,
                auth_identity_generation,
            } if current_account_id == account_id
                && *expected_provider_type == provider_type
                && *auth_identity_generation == expected_generation
        ) {
            return Err(ApiError::conflict(
                "Provider account binding changed during Antigravity model discovery",
            ));
        }
        state
            .record_antigravity_model_catalog_observations_for_generation(
                provider_type,
                account_id,
                expected_generation,
                &catalog,
            )
            .await
            .map_err(|error| {
                tracing::warn!(account_id, error = %error, "persisting Antigravity model evidence failed");
                ApiError::new(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "Antigravity model evidence could not be durably persisted",
                )
            })?;
        return Ok(antigravity_provider_models_fetch_result(catalog));
    }
    if stored.provider_type == ProviderType::KiroOAuth {
        let managed_binding = match &execution.plan.auth_ref {
            crate::domain::providers::runtime::RuntimeAuthRef::ManagedAccount {
                account_id,
                expected_provider_type: ProviderType::KiroOAuth,
                auth_identity_generation,
            } if execution.driver_is("special.kiro") && !account_id.trim().is_empty() => {
                Some((account_id.as_str(), *auth_identity_generation))
            }
            _ => None,
        };
        let mut result_url = "unavailable://kiro/models".to_string();
        let catalog = match managed_binding {
            None => crate::clients::oauth::kiro_runtime::unavailable_model_catalog(
                "managed_account_binding_unavailable",
            ),
            Some((account_id, expected_generation)) => {
                #[cfg(test)]
                let endpoint_override = execution
                    .plan
                    .driver_options
                    .get("testKiroModelsUrl")
                    .and_then(Value::as_str);
                #[cfg(not(test))]
                let endpoint_override: Option<&str> = None;
                let discovery_url = state
                    .find_account_for_provider(ProviderType::KiroOAuth, account_id)
                    .await
                    .and_then(|account| {
                        crate::clients::oauth::kiro_runtime::model_discovery_url(&account).ok()
                    });
                result_url = endpoint_override
                    .map(str::to_string)
                    .or(discovery_url)
                    .unwrap_or_else(|| "unavailable://kiro/models".to_string());
                let timeout = timeout_ms
                    .filter(|value| *value > 0)
                    .map(std::time::Duration::from_millis)
                    .unwrap_or_else(|| execution.request_timeout());
                state
                    .kiro_model_catalog(
                        stored.app,
                        &stored.provider.id,
                        execution.plan.provider_revision,
                        &execution.plan.runtime_fingerprint,
                        account_id,
                        expected_generation,
                        endpoint_override,
                        timeout,
                    )
                    .await
            }
        };
        return Ok(kiro_provider_models_fetch_result(catalog, &result_url));
    }
    if stored.provider_type == ProviderType::AmazonQOAuth {
        let managed_binding = match &execution.plan.auth_ref {
            crate::domain::providers::runtime::RuntimeAuthRef::ManagedAccount {
                account_id,
                expected_provider_type: ProviderType::AmazonQOAuth,
                auth_identity_generation,
            } if execution.driver_is("special.amazon_q") && !account_id.trim().is_empty() => {
                Some((account_id.as_str(), *auth_identity_generation))
            }
            _ => None,
        };
        let mut result_url = "unavailable://amazon-q/models".to_string();
        let catalog = match managed_binding {
            None => crate::clients::oauth::amazon_q_runtime::unavailable_model_catalog(
                "managed_account_binding_unavailable",
            ),
            Some((account_id, expected_generation)) => {
                #[cfg(test)]
                let endpoint_override = execution
                    .plan
                    .driver_options
                    .get("testAmazonQModelsUrl")
                    .and_then(Value::as_str);
                #[cfg(not(test))]
                let endpoint_override: Option<&str> = None;
                let discovery_url = state
                    .find_account_for_provider(ProviderType::AmazonQOAuth, account_id)
                    .await
                    .and_then(|account| {
                        crate::clients::oauth::amazon_q_runtime::model_discovery_url(&account).ok()
                    });
                result_url = endpoint_override
                    .map(str::to_string)
                    .or(discovery_url)
                    .unwrap_or_else(|| "unavailable://amazon-q/models".to_string());
                let timeout = timeout_ms
                    .filter(|value| *value > 0)
                    .map(std::time::Duration::from_millis)
                    .unwrap_or_else(|| execution.request_timeout());
                state
                    .amazon_q_model_catalog(
                        stored.app,
                        &stored.provider.id,
                        execution.plan.provider_revision,
                        &execution.plan.runtime_fingerprint,
                        account_id,
                        expected_generation,
                        endpoint_override,
                        timeout,
                    )
                    .await
            }
        };
        return Ok(amazon_q_provider_models_fetch_result(catalog, &result_url));
    }
    if stored.provider_type == ProviderType::GrokOAuth {
        let timeout = timeout_ms
            .filter(|value| *value > 0)
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| execution.request_timeout());
        let catalog = fetch_bound_grok_model_catalog(state, execution, timeout).await?;
        return Ok(grok_provider_models_fetch_result(catalog));
    }
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
        .apply_auth(&mut target_headers, &mut url, &materialized_auth)
        .map_err(ApiError::proxy)?;
    execution
        .finalize_outbound_identity(&mut target_headers)
        .map_err(ApiError::proxy)?;
    let http_client = state.http_client().await;
    let mut headers = HeaderMap::new();
    proxy::outbound_request::insert_target_headers(&mut headers, &target_headers)
        .map_err(ApiError::proxy)?;
    let request = http_client.get(&url).headers(headers);
    let timeout = timeout_ms
        .filter(|value| *value > 0)
        .map(std::time::Duration::from_millis)
        .unwrap_or_else(|| execution.request_timeout());
    let mut response = request.timeout(timeout).send().await.map_err(|error| {
        ApiError::bad_gateway(format!(
            "fetch provider models failed: {}",
            redact_provider_test_error(&error.to_string())
        ))
    })?;
    let status = response.status();
    let body = read_provider_control_response_body(
        &mut response,
        PROVIDER_MODELS_RESPONSE_BODY_LIMIT_BYTES,
    )
    .await
    .map_err(|error| ApiError::bad_gateway(format!("fetch provider models failed: {error}")))?;
    if !status.is_success() {
        return Err(ApiError::bad_gateway(format!(
            "fetch provider models failed: {status}: {}",
            redact_provider_test_error(&String::from_utf8_lossy(&body))
        )));
    }
    let raw = serde_json::from_slice::<Value>(&body)
        .map_err(|error| ApiError::bad_gateway(format!("parse provider models failed: {error}")))?;
    Ok(ProviderModelsFetchResult {
        url,
        models: parse_provider_models(&raw),
        source: Some("upstream".to_string()),
        stale: Some(false),
        fetched_at_ms: Some(chrono::Utc::now().timestamp_millis()),
    })
}

fn copilot_model_catalog_error_to_api_error(
    error: crate::state::CopilotModelCatalogError,
) -> ApiError {
    match error {
        crate::state::CopilotModelCatalogError::Auth(error) => {
            let (status, message) = match error {
                crate::state::CopilotUpstreamAuthError::NotFound => (
                    axum::http::StatusCode::NOT_FOUND,
                    "github_copilot managed account not found".to_string(),
                ),
                crate::state::CopilotUpstreamAuthError::IdentityChanged { .. } => (
                    axum::http::StatusCode::CONFLICT,
                    "github_copilot account identity changed during model discovery".to_string(),
                ),
                crate::state::CopilotUpstreamAuthError::CredentialPersistenceDegraded => (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "managed account credentials are waiting for durable persistence".to_string(),
                ),
                crate::state::CopilotUpstreamAuthError::MissingGitHubToken { .. } => (
                    axum::http::StatusCode::BAD_REQUEST,
                    "github_copilot managed account lacks its GitHub OAuth token".to_string(),
                ),
                crate::state::CopilotUpstreamAuthError::TokenExchange {
                    status_code,
                    message,
                } => (
                    axum::http::StatusCode::from_u16(status_code)
                        .unwrap_or(axum::http::StatusCode::BAD_GATEWAY),
                    format!("github_copilot token exchange failed: {message}"),
                ),
            };
            ApiError::new(status, message)
        }
        crate::state::CopilotModelCatalogError::Discovery {
            status_code,
            message,
        } => ApiError::new(
            axum::http::StatusCode::from_u16(status_code)
                .unwrap_or(axum::http::StatusCode::BAD_GATEWAY),
            format!("github_copilot model discovery failed: {message}"),
        ),
    }
}

async fn read_provider_control_response_body(
    response: &mut reqwest::Response,
    limit: usize,
) -> Result<Bytes, String> {
    crate::infra::http::read_response_body_limited(response, limit)
        .await
        .map_err(|error| redact_provider_test_error(&error.to_string()))
}

pub(in crate::api) async fn ensure_stored_provider_outbound_allowed(
    state: &ServerState,
    stored: &StoredProvider,
) -> Result<(), ApiError> {
    let has_managed_binding =
        crate::domain::providers::runtime::managed_account_binding(stored).is_some();
    if has_managed_binding && state.credential_persistence_degraded() {
        return Err(ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "managed account credentials are waiting for durable persistence",
        ));
    }
    Ok(())
}

async fn ensure_provider_outbound_allowed(
    state: &ServerState,
    execution: &proxy::provider_ops::ProviderExecution,
) -> Result<(), ApiError> {
    ensure_stored_provider_outbound_allowed(state, &execution.stored).await
}

pub(in crate::api) async fn fetch_bound_grok_model_catalog(
    state: &ServerState,
    execution: &proxy::provider_ops::ProviderExecution,
    timeout: std::time::Duration,
) -> Result<crate::clients::oauth::grok_models::GrokModelCatalog, ApiError> {
    let (account_id, expected_generation) = match &execution.plan.auth_ref {
        crate::domain::providers::runtime::RuntimeAuthRef::ManagedAccount {
            account_id,
            expected_provider_type: ProviderType::GrokOAuth,
            auth_identity_generation,
        } if execution.stored.provider_type == ProviderType::GrokOAuth
            && execution.driver_is("oauth.grok_responses")
            && !account_id.trim().is_empty() =>
        {
            (account_id.as_str(), *auth_identity_generation)
        }
        _ => {
            return Err(ApiError::bad_request(
                "Grok model discovery requires one explicit Grok OAuth managed account binding",
            ));
        }
    };
    ensure_stored_provider_outbound_allowed(state, &execution.stored).await?;
    state
        .refresh_managed_account_if_needed_for_generation(
            ProviderType::GrokOAuth,
            account_id,
            expected_generation,
        )
        .await
        .map_err(map_managed_account_refresh_error)?;
    ensure_stored_provider_outbound_allowed(state, &execution.stored).await?;

    let mut account =
        current_grok_catalog_account(state, execution, account_id, expected_generation, None)
            .await?;
    let mut scope = grok_model_catalog_scope(execution, &account);
    let mut catalog =
        fetch_grok_model_catalog_once(state, execution, &scope, &account, timeout).await;
    if catalog
        .as_ref()
        .is_err_and(crate::clients::oauth::grok_models::GrokModelCatalogFailure::is_unauthorized)
    {
        state
            .refresh_managed_account_now_for_generation(
                ProviderType::GrokOAuth,
                account_id,
                expected_generation,
            )
            .await
            .map_err(map_managed_account_refresh_error)?;
        ensure_stored_provider_outbound_allowed(state, &execution.stored).await?;
        account =
            current_grok_catalog_account(state, execution, account_id, expected_generation, None)
                .await?;
        scope = grok_model_catalog_scope(execution, &account);
        catalog = fetch_grok_model_catalog_once(state, execution, &scope, &account, timeout).await;
    }
    let catalog = catalog.map_err(grok_model_catalog_error_to_api_error)?;
    current_grok_catalog_account(
        state,
        execution,
        account_id,
        expected_generation,
        Some(scope.token_refresh_generation),
    )
    .await?;
    Ok(catalog)
}

async fn current_grok_catalog_account(
    state: &ServerState,
    execution: &proxy::provider_ops::ProviderExecution,
    account_id: &str,
    expected_generation: u64,
    expected_token_generation: Option<u64>,
) -> Result<crate::domain::accounts::store::Account, ApiError> {
    let current_plan = state
        .provider_runtime_plan(execution.stored.app, &execution.stored.provider.id)
        .await
        .filter(|plan| {
            plan.provider_revision == execution.plan.provider_revision
                && plan.runtime_fingerprint == execution.plan.runtime_fingerprint
        })
        .ok_or_else(|| {
            ApiError::conflict("Provider binding changed during Grok model discovery")
        })?;
    if !matches!(
        &current_plan.auth_ref,
        crate::domain::providers::runtime::RuntimeAuthRef::ManagedAccount {
            account_id: current_account_id,
            expected_provider_type: ProviderType::GrokOAuth,
            auth_identity_generation,
        } if current_account_id == account_id
            && *auth_identity_generation == expected_generation
    ) {
        return Err(ApiError::conflict(
            "Provider account binding changed during Grok model discovery",
        ));
    }
    let account = state
        .find_account_for_provider(ProviderType::GrokOAuth, account_id)
        .await
        .filter(|account| account.auth_identity_generation == expected_generation)
        .ok_or_else(|| ApiError::conflict("Bound Grok account changed during model discovery"))?;
    if expected_token_generation
        .is_some_and(|generation| account.token_refresh_generation != generation)
    {
        return Err(ApiError::conflict(
            "Bound Grok account token changed during model discovery",
        ));
    }
    if account
        .access_token
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        return Err(ApiError::unauthorized(
            "Bound Grok account has no usable access token",
        ));
    }
    Ok(account)
}

fn grok_model_catalog_scope(
    execution: &proxy::provider_ops::ProviderExecution,
    account: &crate::domain::accounts::store::Account,
) -> crate::clients::oauth::grok_models::GrokModelCatalogScope {
    crate::clients::oauth::grok_models::GrokModelCatalogScope {
        app: execution.stored.app.as_str().to_string(),
        provider_id: execution.stored.provider.id.clone(),
        provider_revision: execution.plan.provider_revision,
        runtime_fingerprint: execution.plan.runtime_fingerprint.clone(),
        account_id: account.id.clone(),
        auth_identity_generation: account.auth_identity_generation,
        token_refresh_generation: account.token_refresh_generation,
    }
}

async fn fetch_grok_model_catalog_once(
    state: &ServerState,
    execution: &proxy::provider_ops::ProviderExecution,
    scope: &crate::clients::oauth::grok_models::GrokModelCatalogScope,
    account: &crate::domain::accounts::store::Account,
    timeout: std::time::Duration,
) -> Result<
    crate::clients::oauth::grok_models::GrokModelCatalog,
    crate::clients::oauth::grok_models::GrokModelCatalogFailure,
> {
    let access_token = account
        .access_token
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    #[cfg(test)]
    if let Some(url) = execution
        .plan
        .driver_options
        .get("testGrokModelsUrl")
        .and_then(Value::as_str)
    {
        return crate::clients::oauth::grok_models::grok_model_catalog_at_test_url(
            &state.http_client().await,
            scope,
            access_token,
            url,
            timeout,
        )
        .await;
    }
    #[cfg(not(test))]
    let _ = execution;
    crate::clients::oauth::grok_models::grok_model_catalog(
        &state.http_client().await,
        scope,
        access_token,
        timeout,
    )
    .await
}

fn grok_model_catalog_error_to_api_error(
    error: crate::clients::oauth::grok_models::GrokModelCatalogFailure,
) -> ApiError {
    let status = axum::http::StatusCode::from_u16(error.status_code)
        .unwrap_or(axum::http::StatusCode::BAD_GATEWAY);
    tracing::warn!(
        status_code = error.status_code,
        retryable = error.retryable,
        error = %error,
        "Grok model discovery failed"
    );
    ApiError::new(
        status,
        if matches!(error.status_code, 401 | 403) {
            "Grok model discovery authorization was rejected for the bound account"
        } else {
            "Grok model discovery failed for the bound account"
        },
    )
    .with_retryable(error.retryable)
}

fn grok_provider_models_fetch_result(
    catalog: crate::clients::oauth::grok_models::GrokModelCatalog,
) -> ProviderModelsFetchResult {
    let models = catalog
        .models
        .iter()
        .map(|id| FetchedProviderModel {
            id: id.clone(),
            upstream_model: id.clone(),
            display_name: None,
            raw: serde_json::json!({
                "id": id,
                "object": "model",
                "capabilityManifestVersion": 1,
                "modelFamily": grok_model_family(id),
                "accountCapability": grok_account_capability_manifest(id),
            }),
        })
        .collect();
    ProviderModelsFetchResult {
        url: catalog.source_url.clone(),
        models,
        source: Some(catalog.source.to_string()),
        stale: Some(catalog.stale),
        fetched_at_ms: catalog.fetched_at_ms,
    }
}

fn grok_model_family(id: &str) -> &'static str {
    let id = id.to_ascii_lowercase();
    if id.contains("imagine-video") || id.contains("video") {
        "video"
    } else if id.contains("imagine") || id.contains("image") {
        "image"
    } else if id.starts_with("grok-build") || id.contains("composer") {
        "build_text"
    } else if id.starts_with("grok-") {
        "text"
    } else {
        "unknown"
    }
}

fn grok_account_capability_manifest(id: &str) -> Value {
    let family = grok_model_family(id);
    let text_entitlement = matches!(family, "text" | "build_text");
    serde_json::json!({
        "scope": "bound_account_entitlement",
        "evidence": "authoritative_model_catalog",
        "responsesHttpSse": if text_entitlement { "supported" } else { "unknown" },
        "responsesWebSocket": if text_entitlement { "supported" } else { "unknown" },
        "hostedSearch": "unknown",
        "imageGeneration": "unknown",
        "imageEdit": "unknown",
        "videoGeneration": "unknown",
        "asyncTask": "unknown",
    })
}

fn copilot_provider_models_fetch_result(
    catalog: crate::clients::oauth::copilot_models::CopilotModelCatalog,
) -> ProviderModelsFetchResult {
    let models = catalog
        .descriptors
        .iter()
        .map(|descriptor| FetchedProviderModel {
            id: descriptor.model_id.clone(),
            upstream_model: descriptor.model_id.clone(),
            display_name: descriptor.display_name.clone(),
            raw: serde_json::json!({
                "id": descriptor.model_id,
                "object": "model",
                "owned_by": descriptor.vendor.as_deref().unwrap_or("github"),
                "modelPickerEnabled": descriptor.model_picker_enabled,
                "policyState": descriptor.policy_state,
                "preview": descriptor.preview,
                "modelType": descriptor.model_type,
                "supportedEndpoints": descriptor.supported_endpoints,
                "limits": {
                    "maxContextWindowTokens": descriptor.max_context_window_tokens,
                    "maxOutputTokens": descriptor.max_output_tokens,
                },
                "capabilities": {
                    "tools": descriptor.supports_tools,
                    "vision": descriptor.supports_vision,
                    "reasoning": descriptor.supports_reasoning,
                },
                "entitlementSource": "copilot_models_api",
                "githubDomain": catalog.github_domain,
                "apiOrigin": catalog.api_origin,
            }),
        })
        .collect();
    ProviderModelsFetchResult {
        url: catalog
            .api_origin
            .as_deref()
            .map(|origin| format!("{origin}/models"))
            .unwrap_or_else(|| "unavailable://github-copilot/models".to_string()),
        models,
        source: Some(catalog.source.to_string()),
        stale: Some(catalog.stale),
        fetched_at_ms: catalog.fetched_at_ms,
    }
}

fn gemini_provider_models_fetch_result(
    catalog: crate::clients::oauth::gemini_models::GeminiCodeAssistModelCatalog,
) -> ProviderModelsFetchResult {
    let models = catalog
        .descriptors
        .iter()
        .map(|descriptor| FetchedProviderModel {
            id: descriptor.model_id.clone(),
            upstream_model: descriptor.model_id.clone(),
            display_name: descriptor.display_name.clone(),
            raw: serde_json::json!({
                "id": descriptor.model_id,
                "object": "model",
                "entitlementSource": "retrieveUserQuota",
                "remainingFraction": descriptor.remaining_fraction,
                "resetTime": descriptor.reset_time,
            }),
        })
        .collect();
    ProviderModelsFetchResult {
        url: crate::clients::oauth::gemini_models::GEMINI_CODE_ASSIST_MODELS_SOURCE_URL.to_string(),
        models,
        source: Some(catalog.source),
        stale: Some(catalog.stale),
        fetched_at_ms: catalog.fetched_at_ms,
    }
}

fn antigravity_provider_models_fetch_result(
    catalog: crate::clients::oauth::antigravity_models::AntigravityModelCatalog,
) -> ProviderModelsFetchResult {
    let models = catalog
        .descriptors
        .iter()
        .map(|descriptor| FetchedProviderModel {
            id: descriptor.model_id.clone(),
            upstream_model: descriptor.model_id.clone(),
            display_name: descriptor.display_name.clone(),
            raw: serde_json::json!({
                "id": descriptor.model_id,
                "object": "model",
                "entitlementSource": "fetchAvailableModels",
                "family": descriptor.family,
                "quota": {
                    "remainingFraction": descriptor.remaining_fraction,
                    "resetTime": descriptor.reset_time,
                },
                "capabilities": {
                    "supportsImages": descriptor.supports_images,
                    "supportsThinking": descriptor.supports_thinking,
                    "thinkingBudget": descriptor.thinking_budget,
                    "recommended": descriptor.recommended,
                    "maxTokens": descriptor.max_tokens,
                    "maxOutputTokens": descriptor.max_output_tokens,
                    "supportedMimeTypes": descriptor.supported_mime_types,
                },
                "deprecatedAliases": descriptor.deprecated_aliases,
            }),
        })
        .collect();
    ProviderModelsFetchResult {
        url: catalog.source_url,
        models,
        source: Some(catalog.source.to_string()),
        stale: Some(catalog.stale),
        fetched_at_ms: Some(catalog.fetched_at_ms),
    }
}

fn kiro_provider_models_fetch_result(
    catalog: crate::clients::oauth::kiro_runtime::KiroModelCatalog,
    url: &str,
) -> ProviderModelsFetchResult {
    let models = catalog
        .descriptors
        .iter()
        .map(|descriptor| FetchedProviderModel {
            id: descriptor.model_id.clone(),
            upstream_model: descriptor.model_id.clone(),
            display_name: descriptor.display_name.clone(),
            raw: serde_json::json!({
                "id": descriptor.model_id,
                "object": "model",
                "description": descriptor.description,
                "token_limits": {
                    "max_input_tokens": descriptor.max_input_tokens,
                    "max_output_tokens": descriptor.max_output_tokens,
                }
            }),
        })
        .collect();
    ProviderModelsFetchResult {
        url: url.to_string(),
        models,
        source: Some(catalog.source.to_string()),
        stale: Some(catalog.stale),
        fetched_at_ms: catalog.fetched_at_ms,
    }
}

fn amazon_q_provider_models_fetch_result(
    catalog: crate::clients::oauth::amazon_q_runtime::AmazonQModelCatalog,
    url: &str,
) -> ProviderModelsFetchResult {
    let models = catalog
        .descriptors
        .iter()
        .map(|descriptor| FetchedProviderModel {
            id: descriptor.model_id.clone(),
            upstream_model: descriptor.model_id.clone(),
            display_name: descriptor.display_name.clone(),
            raw: serde_json::json!({
                "id": descriptor.model_id,
                "object": "model",
                "description": descriptor.description,
                "default": catalog.default_model_id.as_deref() == Some(descriptor.model_id.as_str()),
                "token_limits": {
                    "max_input_tokens": descriptor.max_input_tokens,
                    "max_output_tokens": descriptor.max_output_tokens,
                },
                "supported_input_types": descriptor.supported_input_types,
                "supports_prompt_cache": descriptor.supports_prompt_cache,
                "origin": crate::clients::oauth::amazon_q_runtime::AMAZON_Q_ORIGIN,
            }),
        })
        .collect();
    ProviderModelsFetchResult {
        url: url.to_string(),
        models,
        source: Some(catalog.source.to_string()),
        stale: Some(catalog.stale),
        fetched_at_ms: catalog.fetched_at_ms,
    }
}

fn claude_provider_models_fetch_result(
    catalog: crate::clients::oauth::claude_models::ClaudeModelCatalog,
) -> ProviderModelsFetchResult {
    let models = catalog
        .models
        .iter()
        .map(|id| FetchedProviderModel {
            id: id.clone(),
            upstream_model: id.clone(),
            display_name: None,
            raw: serde_json::json!({
                "id": id,
                "object": "model",
                "wireProfileId": catalog.wire_profile_id,
                "claudeCodeVersion": catalog.claude_code_version,
            }),
        })
        .collect();
    ProviderModelsFetchResult {
        url: format!("static://{}/models", catalog.wire_profile_id),
        models,
        source: Some(catalog.source.to_string()),
        stale: Some(catalog.stale),
        fetched_at_ms: Some(catalog.fetched_at_ms),
    }
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
            source: Some(
                crate::domain::providers::model::MANAGED_ACCOUNT_AUTH_BINDING_SOURCE.to_string(),
            ),
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
    // These are server-native Profiles. Amazon Q retains a legacy display-name
    // mapping for import compatibility, while the high-risk Web Session
    // Profiles are intentionally hidden and addressable only by explicit ID.
    // None has an upstream cc-switch fixture to clone. Keep that distinction
    // explicit instead of weakening historical missing-fixture failures.
    if matches!(
        profile.profile_id.as_str(),
        "claude.amazon_q_oauth"
            | "codex.amazon_q_oauth"
            | "claude.grok_web_session"
            | "codex.grok_web_session"
            | "claude.perplexity_web_session"
            | "codex.perplexity_web_session"
    ) {
        let mut provider = Provider {
            id: String::new(),
            name: profile.label.clone(),
            settings_config: json!({}),
            category: None,
            meta: None,
            extra: Default::default(),
        };
        apply_s1_profile_creation_defaults(profile, &mut provider)?;
        return Ok(provider);
    }

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
        "claude.google_oauth" => (
            "Google Gemini OAuth",
            json!({
                "env": {"ANTHROPIC_BASE_URL": "https://cloudcode-pa.googleapis.com"}
            }),
        ),
        "claude.github_copilot" | "codex.github_copilot" | "gemini.github_copilot" => {
            ("GitHub Copilot", json!({}))
        }
        "codex.kiro_oauth" => ("Kiro OAuth", json!({})),
        "claude.kimi_code" | "codex.kimi_code" | "gemini.kimi_code" => {
            ("Kimi OAuth", json!({}))
        }
        "claude.qoder_cosy" | "codex.qoder_cosy" | "gemini.qoder_cosy" => ("Qoder OAuth", json!({})),
        "gemini.cursor_api_key" => ("Cursor API Key", json!({})),
        "gemini.cursor_oauth" => ("Cursor OAuth", json!({})),
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
        _ if profile.coding_plan.is_some() => (profile.label.as_str(), json!({})),
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

    if let Some(provider_type) = profile.compatibility_provider_type {
        provider
            .meta
            .get_or_insert_with(crate::domain::providers::model::ProviderMeta::default)
            .provider_type = Some(provider_type.as_str().to_string());
    }
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
            let upstream_model = profile
                .default_upstream_model
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty())
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
    defaults: Option<&crate::domain::providers::runtime::ProviderHealthCheckConfig>,
) -> String {
    let defaults = defaults.cloned().unwrap_or_default();
    if crate::domain::providers::bundle::is_explicit_bundle_surface(&stored.provider) {
        let resolved = override_model
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_string)
            .or_else(|| {
                stored
                    .provider
                    .settings_config
                    .get("testModel")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|model| !model.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| {
                crate::domain::providers::bundle::bundle_test_model_override(&stored.provider)
                    .map(str::to_string)
            })
            .unwrap_or(match app {
                AppKind::Claude => defaults.test_models.claude,
                AppKind::Codex => defaults.test_models.codex,
                AppKind::Gemini => defaults.test_models.gemini,
            });
        return if stored.provider_type == ProviderType::CodexOAuth && app == AppKind::Codex {
            normalize_codex_oauth_test_model(&resolved)
        } else {
            resolved
        };
    }
    let resolved = override_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .or_else(|| {
            stored
                .provider
                .settings_config
                .get("testModel")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
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
            AppKind::Claude => defaults.test_models.claude.clone(),
            AppKind::Codex => defaults.test_models.codex.clone(),
            AppKind::Gemini => defaults.test_models.gemini.clone(),
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
        return "gpt-5.6-luna@low".to_string();
    }
    if trimmed.contains('@') || trimmed.contains('#') {
        return trimmed.to_string();
    }
    format!("{trimmed}@low")
}

pub(in crate::api) fn provider_test_body(
    app: AppKind,
    stored: &StoredProvider,
    override_model: Option<&str>,
    test_prompt: &str,
    stream: bool,
) -> String {
    let model = provider_test_model(app, stored, override_model, None);
    build_provider_model_probe(app, stored.provider_type, &model, test_prompt, stream, "")
        .body_json()
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
    let mut redacted = crate::logging::redact_sensitive_text(&redact_urls_in_text(value));
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

fn redact_urls_in_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < value.len() {
        let remaining = &value[cursor..];
        let next_http = remaining.find("http://");
        let next_https = remaining.find("https://");
        let Some(relative_start) = next_http.into_iter().chain(next_https).min() else {
            output.push_str(remaining);
            break;
        };
        let start = cursor + relative_start;
        output.push_str(&value[cursor..start]);
        let end = value[start..]
            .char_indices()
            .find(|(_, character)| {
                character.is_whitespace() || matches!(character, '"' | '\'' | '<' | '>' | ')' | ']')
            })
            .map(|(offset, _)| start + offset)
            .unwrap_or(value.len());
        let candidate = &value[start..end];
        if reqwest::Url::parse(candidate).is_ok() {
            output.push_str(&redact_provider_endpoint(candidate));
        } else {
            output.push_str(candidate);
        }
        cursor = end;
    }
    output
}

pub(in crate::api) fn map_managed_account_refresh_error(
    error: crate::state::ManagedAccountRefreshError,
) -> ApiError {
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
        ManagedAccountRefreshError::InactiveCodexAccount => ApiError::conflict_code(
            "cc_switch_codex_inactive_account",
            "Codex OAuth account is no longer active",
        ),
        ManagedAccountRefreshError::IdentityChanged { provider_type } => ApiError::conflict_code(
            "cc_switch_provider_account_identity_stale",
            format!(
                "bound {} account identity changed; rebind the Provider",
                provider_type.as_str()
            ),
        ),
        ManagedAccountRefreshError::NotFound => ApiError::not_found("managed account not found"),
        ManagedAccountRefreshError::CredentialPersistenceDegraded => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "managed account credentials are waiting for durable persistence",
        ),
        ManagedAccountRefreshError::Refresh {
            status_code,
            message: _,
            retry_after_ms,
        } => ApiError::new(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY),
            match status_code {
                400 | 401 | 403 => "managed account refresh was rejected; sign in again",
                429 => "managed account refresh was rate limited; retry later",
                _ => "managed account refresh failed",
            },
        )
        .with_retry_after_ms(retry_after_ms),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::domain::providers::model::{
        AppKind, AuthBinding, Provider, ProviderMeta, ProviderType,
    };

    fn provider_api_test_state(name: &str) -> ServerState {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        crate::state::ServerStateInner::load(
            crate::cli::Cli {
                host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port: 0,
                config_dir: Some(
                    std::env::temp_dir()
                        .join(format!("cc-switch-server-provider-api-{name}-{nanos}")),
                ),
                web_dist_dir: None,
                log_level: "warn".to_string(),
                command: None,
            },
            std::sync::Arc::new(crate::logging::LogCapture::new(
                crate::logging::RING_BUFFER_CAPACITY,
            )),
        )
        .unwrap()
    }

    async fn install_cursor_provider_test_execution(
        state: &ServerState,
        provider_type: ProviderType,
        rail_enabled: bool,
    ) -> proxy::provider_ops::ProviderExecution {
        let account_id = "cursor-provider-test-account";
        if provider_type == ProviderType::CursorOAuth {
            state
                .mutate_accounts_immediate(move |accounts| {
                    accounts.upsert(
                        serde_json::from_value(json!({
                            "id": account_id,
                            "providerType": "cursor_oauth",
                            "email": "cursor-provider-test@example.com",
                            "accessToken": "cursor-provider-test-access",
                            "refreshToken": "cursor-provider-test-refresh",
                            "expiresAt": i64::MAX / 2
                        }))
                        .unwrap(),
                    );
                })
                .await
                .unwrap();
        }
        let (profile_id, settings_config) = match provider_type {
            ProviderType::CursorOAuth => (
                "codex.cursor_oauth",
                json!({
                    "CURSOR_OAUTH_AGENT_ENDPOINT": "http://127.0.0.1:8787/oauth/test",
                    "cursorAgentService": {"oauthEnabled": rail_enabled},
                    "modelMapping": {"mode": "single", "upstreamModel": "gpt-5.5"}
                }),
            ),
            ProviderType::CursorApiKey => (
                "codex.cursor_api_key",
                json!({
                    "apiKey": "cursor-provider-test-api-key",
                    "CURSOR_APIKEY_AGENT_ENDPOINT": "http://127.0.0.1:8787/api-key/test",
                    "CURSOR_APIKEY_EXCHANGE_ENDPOINT": "http://127.0.0.1:8787/api-key/exchange",
                    "cursorAgentService": {"apiKeyEnabled": rail_enabled},
                    "modelMapping": {"mode": "single", "upstreamModel": "gpt-5.5"}
                }),
            ),
            _ => unreachable!("Cursor Provider test helper requires a Cursor type"),
        };
        let stored = StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: format!("{}-provider-test", provider_type.as_str()),
                name: "Cursor Provider test".to_string(),
                settings_config,
                category: None,
                meta: Some(ProviderMeta {
                    provider_type: Some(provider_type.as_str().to_string()),
                    auth_binding: (provider_type == ProviderType::CursorOAuth).then(|| {
                        AuthBinding {
                            source: Some("account_store".to_string()),
                            auth_provider: Some("cursor_oauth".to_string()),
                            account_id: Some(account_id.to_string()),
                            auth_identity_generation: Some(1),
                        }
                    }),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
            resource: crate::domain::providers::store::ProviderResourceMetadata {
                profile_id: Some(
                    crate::domain::providers::registry::ProfileId::parse(profile_id).unwrap(),
                ),
                profile_schema_revision: Some(1),
                revision: 1,
                credential_generation: 1,
                ..Default::default()
            },
        };
        let accounts = state.accounts_snapshot().await;
        let mut providers = ProviderStore {
            providers: vec![stored.clone()],
            ..ProviderStore::default()
        };
        providers.rebuild_runtime_index(&accounts).unwrap();
        let execution =
            proxy::provider_ops::ProviderExecution::from_store(&providers, stored).unwrap();
        state.replace_provider_store_for_test(providers).await;
        execution
    }

    async fn install_deepseek_provider_test_execution(
        state: &ServerState,
        test_api_base: Option<String>,
    ) -> proxy::provider_ops::ProviderExecution {
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "deepseek-provider-test-account",
                        "providerType": "deepseek_account",
                        "email": "deepseek-provider-test@example.com",
                        "accessToken": "deepseek-bound-bearer",
                        "expiresAt": i64::MAX / 2,
                        "raw": test_api_base.map(|base| json!({"testDeepSeekApiBase": base}))
                    }))
                    .unwrap(),
                );
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "deepseek-provider-test-decoy",
                        "providerType": "deepseek_account",
                        "accessToken": "deepseek-decoy-bearer",
                        "expiresAt": i64::MAX / 2
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        let stored = StoredProvider {
            app: AppKind::Claude,
            provider: Provider {
                id: "deepseek-provider-test".to_string(),
                name: "DeepSeek Provider test".to_string(),
                settings_config: json!({
                    "modelMapping": {
                        "mode": "single",
                        "upstreamModel": "deepseek-v4-flash"
                    }
                }),
                category: None,
                meta: Some(ProviderMeta {
                    provider_type: Some("deepseek_account".to_string()),
                    auth_binding: Some(AuthBinding {
                        source: Some("account_store".to_string()),
                        auth_provider: Some("deepseek_account".to_string()),
                        account_id: Some("deepseek-provider-test-account".to_string()),
                        auth_identity_generation: Some(1),
                    }),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::DeepSeekAccount,
            provider_type_id: "deepseek_account".to_string(),
            resource: crate::domain::providers::store::ProviderResourceMetadata {
                profile_id: Some(
                    crate::domain::providers::registry::ProfileId::parse("claude.deepseek_account")
                        .unwrap(),
                ),
                profile_schema_revision: Some(1),
                revision: 3,
                credential_generation: 1,
                ..Default::default()
            },
        };
        let accounts = state.accounts_snapshot().await;
        let mut providers = ProviderStore {
            providers: vec![stored.clone()],
            ..ProviderStore::default()
        };
        providers.rebuild_runtime_index(&accounts).unwrap();
        let execution =
            proxy::provider_ops::ProviderExecution::from_store(&providers, stored).unwrap();
        state.replace_provider_store_for_test(providers).await;
        execution
    }

    async fn install_web_session_provider_test_execution(
        state: &ServerState,
        app: AppKind,
        driver: &str,
    ) -> proxy::provider_ops::ProviderExecution {
        let (profile_id, provider_type, cookie, upstream_model) = match (app, driver) {
            (AppKind::Claude, "grok") => (
                "claude.grok_web_session",
                ProviderType::Claude,
                "sso=reviewed-session",
                "fast",
            ),
            (AppKind::Codex, "perplexity") => (
                "codex.perplexity_web_session",
                ProviderType::Codex,
                "__Secure-next-auth.session-token=reviewed-session",
                "pplx-auto",
            ),
            _ => panic!("unsupported Web Session discovery test fixture"),
        };
        let stored = StoredProvider {
            app,
            provider: Provider {
                id: format!("{driver}-web-session-discovery"),
                name: format!("{driver} Web Session discovery"),
                settings_config: json!({
                    "webSession": {"cookie": cookie},
                    "modelMapping": {"mode": "single", "upstreamModel": upstream_model}
                }),
                category: None,
                meta: Some(ProviderMeta {
                    provider_type: Some(provider_type.as_str().to_string()),
                    ..ProviderMeta::default()
                }),
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
            resource: crate::domain::providers::store::ProviderResourceMetadata {
                profile_id: Some(
                    crate::domain::providers::registry::ProfileId::parse(profile_id).unwrap(),
                ),
                profile_schema_revision: Some(1),
                revision: 1,
                credential_generation: 1,
                ..Default::default()
            },
        };
        let accounts = state.accounts_snapshot().await;
        let mut providers = ProviderStore {
            providers: vec![stored.clone()],
            ..ProviderStore::default()
        };
        providers.rebuild_runtime_index(&accounts).unwrap();
        let execution =
            proxy::provider_ops::ProviderExecution::from_store(&providers, stored).unwrap();
        state.replace_provider_store_for_test(providers).await;
        execution
    }

    #[tokio::test]
    async fn web_session_discovery_is_reviewed_static_and_never_claims_live_entitlement() {
        let state = provider_api_test_state("web-session-reviewed-discovery");
        let config_dir = state.config_dir.clone();
        for (app, driver, expected_ids) in [
            (AppKind::Claude, "grok", vec!["fast", "expert", "heavy"]),
            (
                AppKind::Codex,
                "perplexity",
                vec!["pplx-auto", "pplx-sonar", "pplx-sonnet", "pplx-opus"],
            ),
        ] {
            let execution = install_web_session_provider_test_execution(&state, app, driver).await;
            let fetched = fetch_provider_models_inner(&state, &execution, Some(1))
                .await
                .unwrap();
            assert_eq!(
                fetched.source.as_deref(),
                Some("reviewed_web_session_catalog")
            );
            assert_eq!(fetched.stale, Some(false));
            assert_eq!(fetched.fetched_at_ms, None);
            assert!(fetched.url.starts_with("registry://"));
            assert_eq!(
                fetched
                    .models
                    .iter()
                    .map(|model| model.id.as_str())
                    .collect::<Vec<_>>(),
                expected_ids
            );
            for model in fetched.models {
                assert_eq!(model.raw["source"], "reviewed_web_session_catalog");
                assert_eq!(model.raw["fixtureState"], "fixture_verified");
                assert_eq!(model.raw["liveState"], "live_pending");
                assert_eq!(model.raw["entitlement"], "not_asserted");
                assert_eq!(model.raw["capabilities"]["tools"], false);
                assert_eq!(model.raw["capabilities"]["images"], false);
            }
        }
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn deepseek_provider_test_dry_run_validates_fixed_bearer_binding_without_network() {
        let state = provider_api_test_state("deepseek-dry-run");
        let config_dir = state.config_dir.clone();
        let execution = install_deepseek_provider_test_execution(
            &state,
            Some("http://127.0.0.1:9/must-not-run".to_string()),
        )
        .await;

        let response = test_provider_inner(
            &state,
            execution,
            &TestProviderQuery {
                app: AppKind::Claude,
                network: Some(false),
                timeout_ms: Some(1_000),
                model: Some("deepseek-v4-pro-search".to_string()),
                test_prompt: Some("dry-run only".to_string()),
                stream: Some(true),
            },
        )
        .await
        .unwrap();

        assert!(response.ok, "{}", response.message);
        assert_eq!(response.outcome, ProviderOperationOutcome::Success);
        assert_eq!(response.driver_id, "special.deepseek_account");
        assert_eq!(response.provider_type, ProviderType::DeepSeekAccount);
        assert_eq!(response.model, "deepseek-v4-pro-search");
        assert!(!response.network_checked);
        assert!(response.endpoint.contains("chat.deepseek.com"));
        assert!(!response.endpoint.contains("127.0.0.1"));
        assert!(response.header_names.contains(&"authorization".to_string()));
        assert!(response
            .header_names
            .contains(&"x-ds-pow-response".to_string()));
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn deepseek_reviewed_discovery_requires_the_exact_account_generation() {
        let state = provider_api_test_state("deepseek-reviewed-discovery");
        let config_dir = state.config_dir.clone();
        let execution = install_deepseek_provider_test_execution(&state, None).await;

        let fetched = fetch_provider_models_inner(&state, &execution, Some(1_000))
            .await
            .unwrap();

        assert_eq!(
            fetched.source.as_deref(),
            Some("reviewed_deepseek_web_catalog")
        );
        assert_eq!(fetched.stale, Some(false));
        assert_eq!(
            fetched
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            crate::proxy::deepseek::reviewed_model_catalog()
                .iter()
                .map(|model| model.id)
                .collect::<Vec<_>>()
        );
        assert!(fetched.models.iter().all(|model| {
            model.raw["entitlement"] == "live_pending"
                && model.raw["capabilities"]["tools"] == true
                && model.raw["capabilities"]["inputModalities"] == json!(["text"])
        }));

        state
            .mutate_accounts_immediate(|accounts| {
                accounts
                    .accounts
                    .iter_mut()
                    .find(|account| account.id == "deepseek-provider-test-account")
                    .unwrap()
                    .auth_identity_generation = 2;
            })
            .await
            .unwrap();
        let error = fetch_provider_models_inner(&state, &execution, Some(1_000))
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn deepseek_provider_network_test_executes_native_chain_on_bound_account() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let request_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let authorizations = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let completion_observations =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::<(bool, String)>::new()));
        let expire_at = chrono::Utc::now().timestamp() + 60;
        let salt = "deepseek-provider-test-pow";
        let answer = 0_i64;
        let challenge = hex::encode(crate::clients::deepseek::pow::deepseek_hash_v1(
            format!("{salt}_{expire_at}_{answer}").as_bytes(),
        ));
        let app = axum::Router::new()
            .route(
                "/api/v0/chat_session/create",
                axum::routing::post({
                    let request_count = std::sync::Arc::clone(&request_count);
                    let authorizations = std::sync::Arc::clone(&authorizations);
                    move |headers: axum::http::HeaderMap| {
                        let request_count = std::sync::Arc::clone(&request_count);
                        let authorizations = std::sync::Arc::clone(&authorizations);
                        async move {
                            request_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            authorizations.lock().unwrap().push(
                                headers
                                    .get(axum::http::header::AUTHORIZATION)
                                    .and_then(|value| value.to_str().ok())
                                    .unwrap_or_default()
                                    .to_string(),
                            );
                            axum::Json(json!({
                                "code": 0,
                                "data": {"biz_code": 0, "biz_data": {"id": "provider-test-session"}}
                            }))
                        }
                    }
                }),
            )
            .route(
                "/api/v0/chat/create_pow_challenge",
                axum::routing::post({
                    let request_count = std::sync::Arc::clone(&request_count);
                    let authorizations = std::sync::Arc::clone(&authorizations);
                    let challenge = challenge.clone();
                    move |headers: axum::http::HeaderMap| {
                        let request_count = std::sync::Arc::clone(&request_count);
                        let authorizations = std::sync::Arc::clone(&authorizations);
                        let challenge = challenge.clone();
                        async move {
                            request_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            authorizations.lock().unwrap().push(
                                headers
                                    .get(axum::http::header::AUTHORIZATION)
                                    .and_then(|value| value.to_str().ok())
                                    .unwrap_or_default()
                                    .to_string(),
                            );
                            axum::Json(json!({
                                "code": 0,
                                "data": {
                                    "biz_code": 0,
                                    "biz_data": {"challenge": {
                                        "algorithm": "DeepSeekHashV1",
                                        "challenge": challenge,
                                        "salt": salt,
                                        "expire_at": expire_at,
                                        "difficulty": 1,
                                        "expire_after": 60,
                                        "signature": "provider-test-signature",
                                        "target_path": "/api/v0/chat/completion"
                                    }}
                                }
                            }))
                        }
                    }
                }),
            )
            .route(
                "/api/v0/chat/completion",
                axum::routing::post({
                    let request_count = std::sync::Arc::clone(&request_count);
                    let authorizations = std::sync::Arc::clone(&authorizations);
                    let completion_observations = std::sync::Arc::clone(&completion_observations);
                    move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| {
                        let request_count = std::sync::Arc::clone(&request_count);
                        let authorizations = std::sync::Arc::clone(&authorizations);
                        let completion_observations =
                            std::sync::Arc::clone(&completion_observations);
                        async move {
                            request_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            authorizations.lock().unwrap().push(
                                headers
                                    .get(axum::http::header::AUTHORIZATION)
                                    .and_then(|value| value.to_str().ok())
                                    .unwrap_or_default()
                                    .to_string(),
                            );
                            completion_observations.lock().unwrap().push((
                                headers.get("x-ds-pow-response").is_some(),
                                body["chat_session_id"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string(),
                            ));
                            (
                                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                                concat!(
                                    "data: {\"p\":\"response/content\",\"v\":\"pong\"}\n",
                                    "data: {\"p\":\"response/status\",\"v\":\"FINISHED\"}\n"
                                ),
                            )
                        }
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let state = provider_api_test_state("deepseek-native-network-test");
        let config_dir = state.config_dir.clone();
        let execution =
            install_deepseek_provider_test_execution(&state, Some(format!("http://{address}")))
                .await;

        let response = test_provider_inner(
            &state,
            execution,
            &TestProviderQuery {
                app: AppKind::Claude,
                network: Some(true),
                timeout_ms: Some(5_000),
                model: Some("deepseek-v4-flash".to_string()),
                test_prompt: Some("ping".to_string()),
                stream: Some(true),
            },
        )
        .await
        .unwrap();

        assert!(response.ok, "{}", response.message);
        assert_eq!(response.outcome, ProviderOperationOutcome::Success);
        assert!(response.network_checked);
        assert_eq!(response.network_status_code, Some(200));
        assert_eq!(response.network_stream_completed, Some(true));
        assert_eq!(request_count.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert!(authorizations
            .lock()
            .unwrap()
            .iter()
            .all(|authorization| authorization == "Bearer deepseek-bound-bearer"));
        assert_eq!(
            completion_observations.lock().unwrap().as_slice(),
            [(true, "provider-test-session".to_string())]
        );
        assert_eq!(
            state
                .find_account_by_id("deepseek-provider-test-decoy")
                .await
                .unwrap()
                .access_token
                .as_deref(),
            Some("deepseek-decoy-bearer")
        );
        server.abort();
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn cursor_provider_test_dry_run_validates_both_rails_without_exposing_endpoints() {
        for provider_type in [ProviderType::CursorOAuth, ProviderType::CursorApiKey] {
            let state = provider_api_test_state(provider_type.as_str());
            let config_dir = state.config_dir.clone();
            let execution =
                install_cursor_provider_test_execution(&state, provider_type, true).await;

            let response = test_provider_inner(
                &state,
                execution,
                &TestProviderQuery {
                    app: AppKind::Codex,
                    network: Some(false),
                    timeout_ms: Some(2_000),
                    model: Some("gpt-5.5".to_string()),
                    test_prompt: None,
                    stream: Some(true),
                },
            )
            .await
            .unwrap();

            assert!(
                response.ok,
                "{}: {}",
                provider_type.as_str(),
                response.message
            );
            assert_eq!(response.outcome, ProviderOperationOutcome::Success);
            assert!(!response.network_checked);
            assert_eq!(response.endpoint, "[runtime secret]");
            assert!(!response.message.contains("127.0.0.1"));
            drop(state);
            std::fs::remove_dir_all(config_dir).unwrap();
        }
    }

    #[tokio::test]
    async fn cursor_provider_test_reports_a_disabled_rail_as_invalid_configuration() {
        let state = provider_api_test_state("cursor-disabled-rail");
        let config_dir = state.config_dir.clone();
        let execution =
            install_cursor_provider_test_execution(&state, ProviderType::CursorApiKey, false).await;

        let response = test_provider_inner(
            &state,
            execution,
            &TestProviderQuery {
                app: AppKind::Codex,
                network: Some(false),
                timeout_ms: Some(2_000),
                model: Some("gpt-5.5".to_string()),
                test_prompt: None,
                stream: Some(false),
            },
        )
        .await
        .unwrap();

        assert!(!response.ok);
        assert_eq!(response.outcome, ProviderOperationOutcome::InvalidConfig);
        assert!(response.message.contains("disabled"));
        assert!(!response.message.contains("127.0.0.1"));
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn provider_control_response_body_limit_is_enforced() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        PROVIDER_TEST_RESPONSE_BODY_LIMIT_BYTES + 1
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let mut response = reqwest::Client::new()
            .get(format!("http://{address}/probe"))
            .send()
            .await
            .unwrap();

        let error = read_provider_control_response_body(
            &mut response,
            PROVIDER_TEST_RESPONSE_BODY_LIMIT_BYTES,
        )
        .await
        .unwrap_err();

        assert!(error.contains("response body exceeds"));
        server.await.unwrap();
    }

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

        let body = provider_test_body(
            AppKind::Codex,
            &stored,
            None,
            "configured probe prompt",
            false,
        );
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert_eq!(
            value.get("model").and_then(serde_json::Value::as_str),
            Some("test-model")
        );
        assert_eq!(
            value.get("stream").and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            value
                .pointer("/input/0/content")
                .and_then(serde_json::Value::as_str),
            Some("configured probe prompt")
        );

        let stream_body = provider_test_body(
            AppKind::Codex,
            &stored,
            Some("override-model"),
            "ping",
            true,
        );
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
    fn grok_reconciliation_classifies_credentials_and_probe_drift() {
        let now_ms = 1_000_000;
        let mut account: crate::domain::accounts::store::Account = serde_json::from_value(json!({
            "id": "grok-reconcile-account",
            "providerType": "grok_oauth",
            "accessToken": "access",
            "refreshToken": "refresh",
            "expiresAt": now_ms + 3_600_000
        }))
        .unwrap();
        assert_eq!(
            grok_credential_reconciliation(&account, GrokBindingStatus::Current, now_ms),
            (GrokCredentialAction::None, "credentials_current")
        );
        account.expires_at = None;
        assert_eq!(
            grok_credential_reconciliation(&account, GrokBindingStatus::Current, now_ms),
            (GrokCredentialAction::Refresh, "access_token_expiry_missing")
        );
        account.expires_at = Some(now_ms + 1_000);
        assert_eq!(
            grok_credential_reconciliation(&account, GrokBindingStatus::Current, now_ms),
            (GrokCredentialAction::Refresh, "access_token_expires_soon")
        );
        account.refresh_token = None;
        account.expires_at = Some(now_ms - 1);
        assert_eq!(
            grok_credential_reconciliation(&account, GrokBindingStatus::Current, now_ms),
            (
                GrokCredentialAction::Relogin,
                "expired_access_token_without_refresh"
            )
        );
        assert_eq!(
            grok_credential_reconciliation(&account, GrokBindingStatus::StaleIdentity, now_ms),
            (GrokCredentialAction::Relogin, "binding_not_current")
        );
        assert!(grok_probe_model_rejected(
            404,
            br#"{"error":{"code":"model_not_found","message":"requested model was not found"}}"#
        ));
        assert!(grok_probe_model_rejected(
            400,
            br#"{"detail":"Unknown model grok-retired"}"#
        ));
        assert!(grok_probe_model_rejected(
            403,
            br#"{"error":{"type":"unsupported_model"}}"#
        ));
        assert!(!grok_probe_model_rejected(
            404,
            br#"{"error":{"message":"endpoint not found"}}"#
        ));
        assert!(!grok_probe_model_rejected(
            403,
            br#"{"error":{"message":"subscription entitlement required"}}"#
        ));
        assert!(!grok_probe_model_rejected(404, b"404 page not found"));
        assert!(!grok_probe_model_rejected(
            500,
            br#"{"error":{"code":"model_not_found"}}"#
        ));
    }

    async fn install_grok_reconciliation_test_provider(
        state: &ServerState,
        endpoint: String,
        token_url: String,
        account_id: &str,
        refresh_token: &str,
    ) -> proxy::provider_ops::ProviderExecution {
        let account_id = account_id.to_string();
        let account_id_for_binding = account_id.clone();
        let refresh_token = refresh_token.to_string();
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": account_id,
                        "providerType": "grok_oauth",
                        "accessToken": "grok-reconciliation-old-access",
                        "refreshToken": refresh_token,
                        "expiresAt": 1,
                        "profile": {
                            "verifiedGrokClaims": {"subject": "grok-reconciliation-subject"}
                        },
                        "raw": {"testOAuthTokenUrl": token_url}
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: "grok-reconciliation-provider".to_string(),
                        name: "Grok reconciliation provider".to_string(),
                        settings_config: json!({"testRuntimeEndpoint": endpoint}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("grok_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("grok_oauth".to_string()),
                                account_id: Some(account_id_for_binding),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        resolve_provider_execution_by_key(state, AppKind::Codex, "grok-reconciliation-provider")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn grok_provider_reconciliation_dry_run_is_local_and_network_refresh_is_generation_safe()
    {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let response_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let response_requests_for_route = std::sync::Arc::clone(&response_requests);
        let app = axum::Router::new()
            .route(
                "/token",
                axum::routing::post(move || {
                    let requests = std::sync::Arc::clone(&token_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        axum::Json(json!({
                            "access_token": "grok-reconciliation-new-access",
                            "refresh_token": "grok-reconciliation-rotated-refresh",
                            "expires_in": 3600
                        }))
                    }
                }),
            )
            .route(
                "/responses",
                axum::routing::post(move |headers: HeaderMap| {
                    let requests = std::sync::Arc::clone(&response_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        assert_eq!(
                            headers
                                .get(axum::http::header::AUTHORIZATION)
                                .and_then(|value| value.to_str().ok()),
                            Some("Bearer grok-reconciliation-new-access")
                        );
                        axum::Json(json!({
                            "id": "resp-reconciliation",
                            "object": "response",
                            "status": "completed",
                            "output": []
                        }))
                    }
                }),
            )
            .route(
                "/v1/responses",
                axum::routing::post({
                    let response_requests = std::sync::Arc::clone(&response_requests);
                    move |headers: HeaderMap| {
                        let requests = std::sync::Arc::clone(&response_requests);
                        async move {
                            requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            assert_eq!(
                                headers
                                    .get(axum::http::header::AUTHORIZATION)
                                    .and_then(|value| value.to_str().ok()),
                                Some("Bearer grok-reconciliation-new-access")
                            );
                            axum::Json(json!({
                                "id": "resp-reconciliation",
                                "object": "response",
                                "status": "completed",
                                "output": []
                            }))
                        }
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let state = provider_api_test_state("grok-reconciliation");
        let execution = install_grok_reconciliation_test_provider(
            &state,
            format!("http://{address}"),
            format!("http://{address}/token"),
            "grok-reconciliation-success-account",
            "grok-reconciliation-success-refresh",
        )
        .await;
        let dry_run = test_provider_inner(
            &state,
            execution.clone(),
            &TestProviderQuery {
                app: AppKind::Codex,
                network: Some(false),
                timeout_ms: Some(2_000),
                model: Some("grok-4.6".to_string()),
                test_prompt: None,
                stream: Some(false),
            },
        )
        .await
        .unwrap();
        assert!(dry_run.ok);
        let report = dry_run.reconciliation.unwrap();
        assert!(report.read_only);
        assert_eq!(report.binding_status_before, GrokBindingStatus::Current);
        assert_eq!(
            report.planned_credential_action,
            GrokCredentialAction::Refresh
        );
        assert_eq!(
            report.planned_credential_reason,
            "access_token_expires_soon"
        );
        assert!(!report.refresh_performed);
        assert_eq!(
            report.responses_endpoint_status,
            GrokProbeCapabilityStatus::NotChecked
        );
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            response_requests.load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        let network = test_provider_inner(
            &state,
            execution,
            &TestProviderQuery {
                app: AppKind::Codex,
                network: Some(true),
                timeout_ms: Some(2_000),
                model: Some("grok-4.6".to_string()),
                test_prompt: None,
                stream: Some(false),
            },
        )
        .await
        .unwrap();
        assert!(
            network.ok,
            "message={:?} status={:?} error={:?} endpoint={} token_requests={} response_requests={}",
            network.message,
            network.network_status_code,
            network.network_error,
            network.endpoint,
            token_requests.load(std::sync::atomic::Ordering::SeqCst),
            response_requests.load(std::sync::atomic::Ordering::SeqCst),
        );
        let report = network.reconciliation.unwrap();
        assert!(!report.read_only);
        assert_eq!(report.binding_status_after, GrokBindingStatus::Current);
        assert_eq!(
            report.remaining_credential_action,
            GrokCredentialAction::None
        );
        assert!(report.refresh_performed);
        assert!(report.identity_stable);
        assert_eq!(report.expected_auth_identity_generation, Some(1));
        assert_eq!(report.observed_auth_identity_generation_before, Some(1));
        assert_eq!(report.observed_auth_identity_generation_after, Some(1));
        assert!(
            report.token_refresh_generation_after.unwrap()
                > report.token_refresh_generation_before.unwrap()
        );
        assert_eq!(
            report.responses_endpoint_status,
            GrokProbeCapabilityStatus::Supported
        );
        assert_eq!(report.model_status, GrokProbeCapabilityStatus::Supported);
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            response_requests.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        server.abort();
    }

    #[tokio::test]
    async fn grok_provider_reconciliation_refresh_rejection_is_structured_and_stops_before_probe() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let response_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let response_requests_for_route = std::sync::Arc::clone(&response_requests);
        let app = axum::Router::new()
            .route(
                "/token",
                axum::routing::post(move || {
                    let requests = std::sync::Arc::clone(&token_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        (
                            StatusCode::UNAUTHORIZED,
                            axum::Json(json!({
                                "error": "invalid_grant",
                                "error_description": "rejected grok-reconciliation-refresh secret"
                            })),
                        )
                    }
                }),
            )
            .route(
                "/responses",
                axum::routing::post(move || {
                    let requests = std::sync::Arc::clone(&response_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        axum::Json(json!({"status": "must-not-be-requested"}))
                    }
                }),
            )
            .route(
                "/v1/responses",
                axum::routing::post({
                    let response_requests = std::sync::Arc::clone(&response_requests);
                    move || {
                        let requests = std::sync::Arc::clone(&response_requests);
                        async move {
                            requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            axum::Json(json!({"status": "must-not-be-requested"}))
                        }
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let state = provider_api_test_state("grok-reconciliation-refresh-rejected");
        let execution = install_grok_reconciliation_test_provider(
            &state,
            format!("http://{address}"),
            format!("http://{address}/token"),
            "grok-reconciliation-rejected-account",
            "grok-reconciliation-rejected-refresh",
        )
        .await;

        let response = test_provider_inner(
            &state,
            execution,
            &TestProviderQuery {
                app: AppKind::Codex,
                network: Some(true),
                timeout_ms: Some(2_000),
                model: Some("grok-4.6".to_string()),
                test_prompt: None,
                stream: Some(false),
            },
        )
        .await
        .unwrap();

        assert!(!response.ok);
        assert_eq!(response.outcome, ProviderOperationOutcome::Auth);
        assert!(!response.network_checked);
        assert_eq!(response.network_status_code, None);
        assert!(response.header_names.is_empty());
        assert_eq!(
            response.network_error.as_deref(),
            Some("managed account refresh was rejected; sign in again")
        );
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains("error_description"));
        assert!(!serialized.contains("rejected grok-reconciliation-refresh secret"));
        assert!(!serialized.contains("grok-reconciliation-old-access"));
        let report = response.reconciliation.unwrap();
        assert!(!report.read_only);
        assert_eq!(report.binding_status_before, GrokBindingStatus::Current);
        assert_eq!(report.binding_status_after, GrokBindingStatus::Current);
        assert_eq!(
            report.planned_credential_action,
            GrokCredentialAction::Refresh
        );
        assert_eq!(
            report.remaining_credential_action,
            GrokCredentialAction::Refresh
        );
        assert_eq!(
            report.remaining_credential_reason,
            "access_token_expires_soon"
        );
        assert!(!report.refresh_performed);
        assert!(report.identity_stable);
        assert_eq!(report.expected_auth_identity_generation, Some(1));
        assert_eq!(report.observed_auth_identity_generation_before, Some(1));
        assert_eq!(report.observed_auth_identity_generation_after, Some(1));
        assert_eq!(
            report.token_refresh_generation_before,
            report.token_refresh_generation_after
        );
        assert_eq!(
            report.responses_endpoint_status,
            GrokProbeCapabilityStatus::NotChecked
        );
        assert_eq!(report.model_status, GrokProbeCapabilityStatus::NotChecked);
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            response_requests.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let account = state
            .find_account_by_id("grok-reconciliation-rejected-account")
            .await
            .unwrap();
        assert_eq!(account.refresh_consecutive_failures, 1);
        assert!(!account.needs_relogin);
        server.abort();
    }

    #[tokio::test]
    async fn grok_provider_reconciliation_stale_identity_blocks_before_network() {
        let state = provider_api_test_state("grok-reconciliation-stale");
        let execution = install_grok_reconciliation_test_provider(
            &state,
            "http://127.0.0.1:9".to_string(),
            "http://127.0.0.1:9/token".to_string(),
            "grok-reconciliation-stale-account",
            "grok-reconciliation-stale-refresh",
        )
        .await;
        state
            .mutate_accounts_immediate(|accounts| {
                let account = accounts
                    .accounts
                    .iter_mut()
                    .find(|account| account.id == "grok-reconciliation-stale-account")
                    .unwrap();
                account.auth_identity_generation += 1;
            })
            .await
            .unwrap();

        let response = test_provider_inner(
            &state,
            execution,
            &TestProviderQuery {
                app: AppKind::Codex,
                network: Some(true),
                timeout_ms: Some(100),
                model: Some("grok-4.6".to_string()),
                test_prompt: None,
                stream: Some(false),
            },
        )
        .await
        .unwrap();
        assert!(!response.ok);
        assert_eq!(response.outcome, ProviderOperationOutcome::InvalidConfig);
        assert!(!response.network_checked);
        let report = response.reconciliation.unwrap();
        assert_eq!(
            report.binding_status_before,
            GrokBindingStatus::StaleIdentity
        );
        assert_eq!(
            report.planned_credential_action,
            GrokCredentialAction::Relogin
        );
        assert!(!report.identity_stable);
        assert!(!report.refresh_performed);
    }

    #[tokio::test]
    async fn grok_model_discovery_exposes_source_stale_and_fetch_time() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let observed_authorization = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let observed_for_route = std::sync::Arc::clone(&observed_authorization);
        let app = axum::Router::new().route(
            "/models",
            axum::routing::get(move |headers: HeaderMap| {
                let observed = std::sync::Arc::clone(&observed_for_route);
                async move {
                    *observed.lock().unwrap() = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    axum::Json(json!({"data": [{"id": "grok-mock-live"}]}))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = provider_api_test_state("grok-model-catalog");
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "grok-model-account",
                        "providerType": "grok_oauth",
                        "accessToken": "grok-model-access",
                        "expiresAt": i64::MAX / 2,
                        "profile": {
                            "verifiedGrokClaims": {"subject": "grok-model-subject"}
                        }
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        let models_url = format!("http://{address}/models");
        let expected_models_url = models_url.clone();
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: "grok-model-provider".to_string(),
                        name: "Grok model provider".to_string(),
                        settings_config: json!({
                            "testGrokModelsUrl": models_url,
                            "models": ["configured-model-must-not-leak"]
                        }),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("grok_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("grok_oauth".to_string()),
                                account_id: Some("grok-model-account".to_string()),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        let execution =
            resolve_provider_execution_by_key(&state, AppKind::Codex, "grok-model-provider")
                .await
                .unwrap();
        let fetched = fetch_provider_models_inner(&state, &execution, Some(2_000))
            .await
            .unwrap();

        assert_eq!(fetched.source.as_deref(), Some("upstream"));
        assert_eq!(fetched.stale, Some(false));
        assert!(fetched.fetched_at_ms.is_some_and(|value| value > 0));
        assert_eq!(fetched.url, expected_models_url);
        assert_eq!(fetched.models.len(), 1);
        assert_eq!(fetched.models[0].id, "grok-mock-live");
        assert_eq!(fetched.models[0].raw["modelFamily"], "text");
        assert_eq!(
            fetched.models[0].raw["accountCapability"]["responsesHttpSse"],
            "supported"
        );
        assert_eq!(
            fetched.models[0].raw["accountCapability"]["hostedSearch"],
            "unknown"
        );
        assert_eq!(
            observed_authorization.lock().unwrap().as_str(),
            "Bearer grok-model-access"
        );
        server.abort();
    }

    #[tokio::test]
    async fn grok_model_discovery_401_refreshes_and_replays_only_the_bound_account_once() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let model_authorizations = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let authorizations_for_route = std::sync::Arc::clone(&model_authorizations);
        let app = axum::Router::new()
            .route(
                "/token",
                axum::routing::post(move || {
                    let requests = std::sync::Arc::clone(&token_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        axum::Json(json!({
                            "access_token": "refreshed-bound-grok-token",
                            "refresh_token": "refreshed-bound-grok-refresh",
                            "expires_in": 3_600,
                            "token_type": "Bearer"
                        }))
                    }
                }),
            )
            .route(
                "/models",
                axum::routing::get(move |headers: HeaderMap| {
                    let authorizations = std::sync::Arc::clone(&authorizations_for_route);
                    async move {
                        let authorization = headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string();
                        authorizations.lock().unwrap().push(authorization.clone());
                        if authorization == "Bearer initial-bound-grok-token" {
                            (
                                StatusCode::UNAUTHORIZED,
                                axum::Json(json!({"error": "expired"})),
                            )
                        } else {
                            (
                                StatusCode::OK,
                                axum::Json(json!({"data": [{"id": "grok-refreshed"}]})),
                            )
                        }
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = provider_api_test_state("grok-model-catalog-401");
        let token_url = format!("http://{address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                for (id, access_token, refresh_token, subject, raw) in [
                    (
                        "grok-model-401-distractor",
                        "distractor-grok-token",
                        "distractor-grok-refresh",
                        "distractor-grok-subject",
                        None,
                    ),
                    (
                        "grok-model-401-bound",
                        "initial-bound-grok-token",
                        "initial-bound-grok-refresh",
                        "bound-grok-subject",
                        Some(json!({"testOAuthTokenUrl": token_url})),
                    ),
                ] {
                    accounts.upsert(
                        serde_json::from_value(json!({
                            "id": id,
                            "providerType": "grok_oauth",
                            "accessToken": access_token,
                            "refreshToken": refresh_token,
                            "expiresAt": i64::MAX / 2,
                            "profile": {"verifiedGrokClaims": {"subject": subject}},
                            "raw": raw
                        }))
                        .unwrap(),
                    );
                }
            })
            .await
            .unwrap();
        let models_url = format!("http://{address}/models");
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: "grok-model-401-provider".to_string(),
                        name: "Grok model 401 provider".to_string(),
                        settings_config: json!({"testGrokModelsUrl": models_url}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("grok_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("grok_oauth".to_string()),
                                account_id: Some("grok-model-401-bound".to_string()),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        let execution =
            resolve_provider_execution_by_key(&state, AppKind::Codex, "grok-model-401-provider")
                .await
                .unwrap();

        let fetched = fetch_provider_models_inner(&state, &execution, Some(2_000))
            .await
            .unwrap();

        assert_eq!(fetched.models.len(), 1);
        assert_eq!(fetched.models[0].id, "grok-refreshed");
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            model_authorizations.lock().unwrap().as_slice(),
            &[
                "Bearer initial-bound-grok-token".to_string(),
                "Bearer refreshed-bound-grok-token".to_string(),
            ]
        );
        let account = state
            .find_account_for_provider(ProviderType::GrokOAuth, "grok-model-401-bound")
            .await
            .unwrap();
        assert_eq!(account.token_refresh_generation, 2);
        assert_eq!(
            account.access_token.as_deref(),
            Some("refreshed-bound-grok-token")
        );
        let distractor = state
            .find_account_for_provider(ProviderType::GrokOAuth, "grok-model-401-distractor")
            .await
            .unwrap();
        assert_eq!(
            distractor.access_token.as_deref(),
            Some("distractor-grok-token")
        );
        server.abort();
    }

    #[tokio::test]
    async fn gemini_code_assist_discovery_refreshes_only_the_bound_account_and_keeps_stale_cache() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let failure_status = std::sync::Arc::new(std::sync::atomic::AtomicU16::new(0));
        let observed_authorizations =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let failure_status_for_route = std::sync::Arc::clone(&failure_status);
        let authorizations_for_route = std::sync::Arc::clone(&observed_authorizations);
        let app = axum::Router::new().fallback(axum::routing::post(
            move |uri: axum::http::Uri, headers: HeaderMap| {
                let failure_status = std::sync::Arc::clone(&failure_status_for_route);
                let observed = std::sync::Arc::clone(&authorizations_for_route);
                async move {
                    observed.lock().unwrap().push(
                        headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    );
                    let failure_status =
                        failure_status.load(std::sync::atomic::Ordering::SeqCst);
                    if failure_status != 0 {
                        return (
                            StatusCode::from_u16(failure_status).unwrap(),
                            axum::Json(json!({"error": {"message": "temporary failure"}})),
                        );
                    }
                    match uri.path() {
                        "/v1internal:loadCodeAssist" => (
                            StatusCode::OK,
                            axum::Json(json!({
                                "cloudaicompanionProject": "bound-project",
                                "paidTier": {"id": "standard-tier", "name": "Gemini Code Assist"}
                            })),
                        ),
                        "/v1internal:retrieveUserQuota" => (
                            StatusCode::OK,
                            axum::Json(json!({
                                "buckets": [
                                    {"modelId": "gemini-3.1-pro", "remainingFraction": 0.75},
                                    {"modelId": "models/gemini-3.1-flash", "remainingFraction": 0.0},
                                    {"modelId": "claude-sonnet-4-6", "remainingFraction": 1.0}
                                ]
                            })),
                        ),
                        path => panic!("unexpected Gemini model discovery request: {path}"),
                    }
                }
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = provider_api_test_state("gemini-code-assist-model-catalog");
        let test_base_url = format!("http://{address}");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "gemini-bound-model-account",
                        "providerType": "gemini_cli",
                        "accessToken": "gemini-bound-access",
                        "expiresAt": i64::MAX / 2,
                        "raw": {"testGeminiCodeAssistBaseUrl": test_base_url}
                    }))
                    .unwrap(),
                );
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "gemini-unbound-model-account",
                        "providerType": "gemini_cli",
                        "accessToken": "gemini-unbound-must-not-be-used",
                        "expiresAt": i64::MAX / 2
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        state
            .mutate_providers_immediate(|providers| {
                providers.upsert(
                    AppKind::Gemini,
                    Provider {
                        id: "gemini-bound-model-provider".to_string(),
                        name: "Gemini bound model provider".to_string(),
                        settings_config: json!({}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("gemini_cli".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("gemini_cli".to_string()),
                                account_id: Some("gemini-bound-model-account".to_string()),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        let execution = resolve_provider_execution_by_key(
            &state,
            AppKind::Gemini,
            "gemini-bound-model-provider",
        )
        .await
        .unwrap();
        assert!(execution.driver_is("oauth.gemini_code_assist"));

        let fresh = fetch_provider_models_inner(&state, &execution, Some(2_000))
            .await
            .unwrap();
        assert_eq!(
            fresh.source.as_deref(),
            Some("authenticated_retrieve_user_quota")
        );
        assert_eq!(fresh.stale, Some(false));
        assert_eq!(
            fresh
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["gemini-3.1-flash", "gemini-3.1-pro"]
        );
        assert!(observed_authorizations
            .lock()
            .unwrap()
            .iter()
            .all(|value| value == "Bearer gemini-bound-access"));

        failure_status.store(
            StatusCode::SERVICE_UNAVAILABLE.as_u16(),
            std::sync::atomic::Ordering::SeqCst,
        );
        let stale = fetch_provider_models_inner(&state, &execution, Some(2_000))
            .await
            .unwrap();
        assert_eq!(
            stale.source.as_deref(),
            Some("same_account_cached_retrieve_user_quota")
        );
        assert_eq!(stale.stale, Some(true));
        assert_eq!(stale.models.len(), 2);

        failure_status.store(
            StatusCode::UNAUTHORIZED.as_u16(),
            std::sync::atomic::Ordering::SeqCst,
        );
        let auth_error = match fetch_provider_models_inner(&state, &execution, Some(2_000)).await {
            Ok(_) => panic!("authentication failure must not use a stale model catalog"),
            Err(error) => error,
        };
        assert_eq!(auth_error.status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            auth_error.message,
            "Gemini Code Assist model discovery failed for the bound account"
        );

        let conformance = crate::domain::providers::registry::provider_registry()
            .conformance
            .iter()
            .find(|item| item.driver_id.as_str() == "oauth.gemini_code_assist")
            .unwrap();
        assert_eq!(
            conformance.discovery,
            crate::domain::providers::registry::ConformanceState::FixtureVerified
        );
        server.abort();
    }

    #[tokio::test]
    async fn antigravity_discovery_uses_exact_account_capabilities_and_generation_scoped_stale() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let model_status = std::sync::Arc::new(std::sync::atomic::AtomicU16::new(200));
        let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
        let model_status_for_route = std::sync::Arc::clone(&model_status);
        let observed_for_route = std::sync::Arc::clone(&observed);
        let app = axum::Router::new().fallback(axum::routing::post(
            move |uri: axum::http::Uri, headers: HeaderMap, body: Bytes| {
                let model_status = std::sync::Arc::clone(&model_status_for_route);
                let observed = std::sync::Arc::clone(&observed_for_route);
                async move {
                    let authorization = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    observed
                        .lock()
                        .unwrap()
                        .push((uri.path().to_string(), authorization));
                    match uri.path() {
                        "/v1internal:loadCodeAssist" => (
                            StatusCode::OK,
                            axum::Json(json!({
                                "cloudaicompanionProject": "antigravity-bound-project",
                                "paidTier": {"id": "standard-tier", "name": "Antigravity Pro"}
                            })),
                        ),
                        "/v1internal:fetchAvailableModels" => {
                            assert_eq!(
                                serde_json::from_slice::<Value>(&body).unwrap()["project"],
                                "antigravity-bound-project"
                            );
                            let status = StatusCode::from_u16(
                                model_status.load(std::sync::atomic::Ordering::SeqCst),
                            )
                            .unwrap();
                            if status != StatusCode::OK {
                                return (status, axum::Json(json!({"error": "catalog failure"})));
                            }
                            (
                                StatusCode::OK,
                                axum::Json(json!({
                                    "models": {
                                        "gemini-3.1-pro": {
                                            "displayName": "Gemini 3.1 Pro",
                                            "quotaInfo": {"remainingFraction": 0.0, "resetTime": "2026-09-01T00:00:00Z"},
                                            "supportsImages": true,
                                            "supportsThinking": true,
                                            "thinkingBudget": 32768,
                                            "maxTokens": 1048576,
                                            "maxOutputTokens": 65536,
                                            "supportedMimeTypes": {"image/png": true}
                                        },
                                        "claude-sonnet-4-6": {"supportsThinking": true},
                                        "gpt-oss-120b-medium": {"recommended": true}
                                    },
                                    "deprecatedModelIds": {
                                        "gemini-old": {"newModelId": "gemini-3.1-pro"}
                                    }
                                })),
                            )
                        }
                        path => panic!("unexpected Antigravity model request: {path}"),
                    }
                }
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = provider_api_test_state("antigravity-model-catalog");
        let base_url = format!("http://{address}");
        let models_url = format!("{base_url}/v1internal:fetchAvailableModels");
        let account_base_url = base_url.clone();
        state
            .mutate_accounts_immediate(move |accounts| {
                for (id, provider_type, access_token) in [
                    (
                        "antigravity-bound-model-account",
                        "antigravity_oauth",
                        "antigravity-bound-access",
                    ),
                    ("agy-bound-model-account", "agy_oauth", "agy-bound-access"),
                ] {
                    accounts.upsert(
                        serde_json::from_value(json!({
                            "id": id,
                            "providerType": provider_type,
                            "accessToken": access_token,
                            "expiresAt": i64::MAX / 2,
                            "raw": {"testGeminiCodeAssistBaseUrl": account_base_url}
                        }))
                        .unwrap(),
                    );
                }
            })
            .await
            .unwrap();
        state
            .mutate_providers_immediate(move |providers| {
                for (id, name, provider_type, account_id) in [
                    (
                        "antigravity-bound-model-provider",
                        "Antigravity bound model provider",
                        "antigravity_oauth",
                        "antigravity-bound-model-account",
                    ),
                    (
                        "agy-bound-model-provider",
                        "Agy bound model provider",
                        "agy_oauth",
                        "agy-bound-model-account",
                    ),
                ] {
                    providers.upsert(
                        AppKind::Gemini,
                        Provider {
                            id: id.to_string(),
                            name: name.to_string(),
                            settings_config: json!({"testAntigravityModelsUrl": models_url}),
                            category: None,
                            meta: Some(ProviderMeta {
                                provider_type: Some(provider_type.to_string()),
                                auth_binding: Some(AuthBinding {
                                    source: Some("account_store".to_string()),
                                    auth_provider: Some(provider_type.to_string()),
                                    account_id: Some(account_id.to_string()),
                                    auth_identity_generation: Some(1),
                                }),
                                ..Default::default()
                            }),
                            extra: Default::default(),
                        },
                    );
                }
            })
            .await
            .unwrap();

        let execution = resolve_provider_execution_by_key(
            &state,
            AppKind::Gemini,
            "antigravity-bound-model-provider",
        )
        .await
        .unwrap();
        assert!(execution.driver_is("special.antigravity"));
        let fresh = fetch_provider_models_inner(&state, &execution, Some(2_000))
            .await
            .unwrap();
        assert_eq!(
            fresh.source.as_deref(),
            Some("authenticated_fetch_available_models")
        );
        assert_eq!(fresh.stale, Some(false));
        assert_eq!(
            fresh
                .models
                .iter()
                .map(|model| (model.id.as_str(), model.raw["family"].as_str().unwrap()))
                .collect::<Vec<_>>(),
            [
                ("claude-sonnet-4-6", "claude"),
                ("gemini-3.1-pro", "gemini"),
                ("gpt-oss-120b-medium", "gpt"),
            ]
        );
        let gemini = fresh
            .models
            .iter()
            .find(|model| model.id == "gemini-3.1-pro")
            .unwrap();
        assert_eq!(gemini.raw["quota"]["remainingFraction"], 0.0);
        assert_eq!(gemini.raw["capabilities"]["supportsImages"], true);
        assert_eq!(gemini.raw["deprecatedAliases"], json!(["gemini-old"]));

        model_status.store(503, std::sync::atomic::Ordering::SeqCst);
        let stale = fetch_provider_models_inner(&state, &execution, Some(2_000))
            .await
            .unwrap();
        assert_eq!(
            stale.source.as_deref(),
            Some("same_identity_cached_fetch_available_models")
        );
        assert_eq!(stale.stale, Some(true));

        let agy_execution =
            resolve_provider_execution_by_key(&state, AppKind::Gemini, "agy-bound-model-provider")
                .await
                .unwrap();
        let agy_without_own_cache =
            match fetch_provider_models_inner(&state, &agy_execution, Some(2_000)).await {
                Ok(_) => panic!("Agy must not reuse the Antigravity account cache"),
                Err(error) => error,
            };
        assert_eq!(agy_without_own_cache.status, StatusCode::BAD_GATEWAY);

        model_status.store(401, std::sync::atomic::Ordering::SeqCst);
        let auth_error = match fetch_provider_models_inner(&state, &execution, Some(2_000)).await {
            Ok(_) => panic!("authentication failure must not use a stale catalog"),
            Err(error) => error,
        };
        assert_eq!(auth_error.status, StatusCode::BAD_GATEWAY);

        let account = state
            .find_account_for_provider(
                ProviderType::AntigravityOAuth,
                "antigravity-bound-model-account",
            )
            .await
            .unwrap();
        for dimension in [
            crate::domain::accounts::capability_evidence::MODEL_CATALOG_DIMENSION,
            crate::domain::accounts::capability_evidence::GEMINI_QUOTA_FAMILY_DIMENSION,
            crate::domain::accounts::capability_evidence::CLAUDE_QUOTA_FAMILY_DIMENSION,
            crate::domain::accounts::capability_evidence::GPT_QUOTA_FAMILY_DIMENSION,
            crate::domain::accounts::capability_evidence::MODEL_CAPACITY_DIMENSION,
        ] {
            let key = crate::domain::accounts::capability_evidence::observation_key(
                crate::domain::accounts::capability_evidence::ANTIGRAVITY_CODE_PLAN_CAPABILITY,
                dimension,
            );
            assert_eq!(
                account.capability_observations[&key].auth_identity_generation,
                1
            );
        }
        assert!(observed.lock().unwrap().iter().all(|(_, authorization)| {
            authorization == "Bearer antigravity-bound-access"
                || authorization == "Bearer agy-bound-access"
        }));
        let requests_before_generation_drift = observed.lock().unwrap().len();
        state
            .mutate_accounts_immediate(|accounts| {
                accounts
                    .accounts
                    .iter_mut()
                    .find(|account| account.id == "antigravity-bound-model-account")
                    .unwrap()
                    .auth_identity_generation += 1;
            })
            .await
            .unwrap();
        let generation_error =
            match fetch_provider_models_inner(&state, &execution, Some(2_000)).await {
                Ok(_) => panic!("old Provider binding must not reuse a prior-generation catalog"),
                Err(error) => error,
            };
        assert_eq!(generation_error.status, StatusCode::CONFLICT);
        assert_eq!(
            observed.lock().unwrap().len(),
            requests_before_generation_drift
        );
        for driver_id in ["special.antigravity", "special.agy"] {
            let conformance = crate::domain::providers::registry::provider_registry()
                .conformance
                .iter()
                .find(|item| item.driver_id.as_str() == driver_id)
                .unwrap();
            assert_eq!(
                conformance.discovery,
                crate::domain::providers::registry::ConformanceState::FixtureVerified
            );
        }
        server.abort();
    }

    #[tokio::test]
    async fn gemini_code_assist_discovery_rejects_binding_generation_drift_before_network() {
        let state = provider_api_test_state("gemini-code-assist-model-generation");
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "gemini-generation-account",
                        "providerType": "gemini_cli",
                        "authIdentityGeneration": 1,
                        "accessToken": "gemini-generation-access",
                        "expiresAt": i64::MAX / 2
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        state
            .mutate_providers_immediate(|providers| {
                providers.upsert(
                    AppKind::Gemini,
                    Provider {
                        id: "gemini-generation-provider".to_string(),
                        name: "Gemini generation provider".to_string(),
                        settings_config: json!({}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("gemini_cli".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("gemini_cli".to_string()),
                                account_id: Some("gemini-generation-account".to_string()),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        let execution = resolve_provider_execution_by_key(
            &state,
            AppKind::Gemini,
            "gemini-generation-provider",
        )
        .await
        .unwrap();
        state
            .mutate_accounts_immediate(|accounts| {
                accounts
                    .accounts
                    .iter_mut()
                    .find(|account| account.id == "gemini-generation-account")
                    .unwrap()
                    .auth_identity_generation = 2;
            })
            .await
            .unwrap();

        let error = match fetch_provider_models_inner(&state, &execution, Some(100)).await {
            Ok(_) => panic!("generation drift must fail model discovery"),
            Err(error) => error,
        };
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            error.message,
            "Gemini Code Assist model discovery failed for the bound account"
        );
    }

    #[tokio::test]
    async fn copilot_provider_api_discovery_proves_fixture_verified_contract() {
        #[derive(Debug, Clone, Default)]
        struct Observation {
            authorization: String,
            user_agent: String,
            editor_version: String,
            plugin_version: String,
            api_version: String,
        }

        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let observation = std::sync::Arc::new(std::sync::Mutex::new(Observation::default()));
        let observation_for_route = std::sync::Arc::clone(&observation);
        let app = axum::Router::new().route(
            "/models",
            axum::routing::get(move |headers: HeaderMap| {
                let observation = std::sync::Arc::clone(&observation_for_route);
                async move {
                    let header = |name: &str| {
                        headers
                            .get(name)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string()
                    };
                    *observation.lock().unwrap() = Observation {
                        authorization: header("authorization"),
                        user_agent: header("user-agent"),
                        editor_version: header("editor-version"),
                        plugin_version: header("editor-plugin-version"),
                        api_version: header("x-github-api-version"),
                    };
                    axum::Json(json!({
                        "data": [
                            {
                                "id": "claude-sonnet-5",
                                "name": "Claude Sonnet 5",
                                "vendor": "anthropic",
                                "model_picker_enabled": true,
                                "preview": true,
                                "policy": {"state": "enabled"},
                                "supported_endpoints": ["/v1/messages"],
                                "capabilities": {
                                    "type": "chat",
                                    "limits": {
                                        "max_context_window_tokens": 200000,
                                        "max_output_tokens": 32000
                                    },
                                    "supports": {"tool_calls": true, "vision": true, "reasoning": true}
                                }
                            },
                            {"id": "claude-sonnet-5"},
                            {"id": "gpt-5.6-codex"},
                            {"id": "disabled-model", "policy": {"state": "disabled"}, "capabilities": {"type": "chat"}},
                            {"id": "embedding-model", "capabilities": {"type": "embeddings"}},
                            {"id": "   "},
                            {"not_a_model": "ignored"}
                        ]
                    }))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = provider_api_test_state("copilot-model-catalog");
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "copilot-model-account",
                        "providerType": "github_copilot",
                        "accessToken": "copilot-model-sub-token",
                        "refreshToken": "github-oauth-must-not-reach-models",
                        "expiresAt": i64::MAX / 2,
                        "profile": {"githubDomain": "github.com", "ghes": false},
                        "raw": {
                            "githubDomain": "github.com",
                            "githubToken": "github-oauth-must-not-reach-models",
                            "copilotToken": {"token": "copilot-model-sub-token"},
                            "copilotApiBase": "https://api.githubcopilot.com"
                        }
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        let models_url = format!("http://{address}/models");
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Claude,
                    Provider {
                        id: "copilot-model-provider".to_string(),
                        name: "Copilot model provider".to_string(),
                        settings_config: json!({"testCopilotModelsUrl": models_url}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("github_copilot".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("github_copilot".to_string()),
                                account_id: Some("copilot-model-account".to_string()),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        let execution =
            resolve_provider_execution_by_key(&state, AppKind::Claude, "copilot-model-provider")
                .await
                .unwrap();
        assert_eq!(execution.plan.driver_id.as_str(), "special.copilot");
        assert_eq!(
            execution
                .plan
                .driver_options
                .get("testCopilotModelsUrl")
                .and_then(Value::as_str),
            Some(format!("http://{address}/models").as_str())
        );

        let fetched = fetch_provider_models_inner(&state, &execution, Some(2_000))
            .await
            .unwrap();
        assert_eq!(fetched.source.as_deref(), Some("copilot_models_api"));
        assert_eq!(fetched.stale, Some(false));
        assert!(fetched.fetched_at_ms.is_some_and(|value| value > 0));
        assert_eq!(fetched.url, "https://api.githubcopilot.com/models");
        assert_eq!(
            fetched
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["claude-sonnet-5", "gpt-5.6-codex"]
        );
        let claude = fetched
            .models
            .iter()
            .find(|model| model.id == "claude-sonnet-5")
            .unwrap();
        assert_eq!(claude.display_name.as_deref(), Some("Claude Sonnet 5"));
        assert_eq!(
            claude.raw.pointer("/owned_by").and_then(Value::as_str),
            Some("anthropic")
        );
        assert_eq!(
            claude.raw.pointer("/policyState").and_then(Value::as_str),
            Some("enabled")
        );
        assert_eq!(
            claude
                .raw
                .pointer("/limits/maxContextWindowTokens")
                .and_then(Value::as_u64),
            Some(200_000)
        );
        assert_eq!(
            claude
                .raw
                .pointer("/capabilities/tools")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            claude.raw.pointer("/githubDomain").and_then(Value::as_str),
            Some("github.com")
        );
        assert_eq!(
            claude.raw.pointer("/apiOrigin").and_then(Value::as_str),
            Some("https://api.githubcopilot.com")
        );
        let observation = observation.lock().unwrap();
        assert_eq!(observation.authorization, "Bearer copilot-model-sub-token");
        assert_eq!(
            observation.user_agent,
            crate::provider_identity::COPILOT_USER_AGENT
        );
        assert_eq!(
            observation.editor_version,
            crate::provider_identity::COPILOT_EDITOR_VERSION
        );
        assert_eq!(
            observation.plugin_version,
            crate::provider_identity::COPILOT_PLUGIN_VERSION
        );
        assert_eq!(
            observation.api_version,
            crate::provider_identity::COPILOT_API_VERSION
        );
        assert!(!format!("{observation:?}").contains("github-oauth-must-not-reach-models"));
        drop(observation);

        let conformance = crate::domain::providers::registry::provider_registry()
            .conformance
            .iter()
            .find(|item| item.driver_id.as_str() == "special.copilot")
            .unwrap();
        assert_eq!(
            conformance.discovery,
            crate::domain::providers::registry::ConformanceState::FixtureVerified
        );
        server.abort();
    }

    #[tokio::test]
    async fn kiro_model_discovery_uses_only_the_explicit_codex_binding() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let observed_authorizations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_for_route = std::sync::Arc::clone(&observed_authorizations);
        let app = axum::Router::new().route(
            "/models",
            axum::routing::get(move |headers: HeaderMap| {
                let observed = std::sync::Arc::clone(&observed_for_route);
                async move {
                    observed.lock().unwrap().push(
                        headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    );
                    axum::Json(json!({
                        "models": [
                            {"modelId": "kiro-bound-live"},
                            {"modelId": "kiro-bound-live"}
                        ]
                    }))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = provider_api_test_state("kiro-bound-model-catalog");
        state
            .mutate_accounts_immediate(|accounts| {
                for (id, token) in [
                    ("kiro-bound-model-account", "kiro-bound-model-access"),
                    ("kiro-unbound-model-account", "kiro-unbound-model-access"),
                ] {
                    accounts.upsert(
                        serde_json::from_value(json!({
                            "id": id,
                            "providerType": "kiro_oauth",
                            "accessToken": token,
                            "expiresAt": i64::MAX / 2,
                            "profile": {
                                "profileArn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/test",
                                "apiRegion": "us-east-1",
                                "machineId": id,
                                "authMethod": "social"
                            }
                        }))
                        .unwrap(),
                    );
                }
            })
            .await
            .unwrap();
        let models_url = format!("http://{address}/models");
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: "kiro-codex-model-provider".to_string(),
                        name: "Kiro Codex model provider".to_string(),
                        settings_config: json!({"testKiroModelsUrl": models_url}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("kiro_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("kiro_oauth".to_string()),
                                account_id: Some("kiro-bound-model-account".to_string()),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        let execution =
            resolve_provider_execution_by_key(&state, AppKind::Codex, "kiro-codex-model-provider")
                .await
                .unwrap();
        assert!(execution.driver_is("special.kiro"));

        let fetched = fetch_provider_models_inner(&state, &execution, Some(2_000))
            .await
            .unwrap();

        assert_eq!(
            fetched.source.as_deref(),
            Some("kiro_list_available_models")
        );
        assert_eq!(fetched.stale, Some(false));
        assert_eq!(fetched.models.len(), 1);
        assert_eq!(fetched.models[0].id, "kiro-bound-live");
        assert_eq!(
            observed_authorizations.lock().unwrap().as_slice(),
            ["Bearer kiro-bound-model-access"]
        );
        server.abort();
    }

    #[tokio::test]
    async fn kiro_model_discovery_401_refreshes_and_replays_only_the_bound_account_once() {
        use axum::response::IntoResponse;

        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_for_models = std::sync::Arc::clone(&observed);
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let app = axum::Router::new()
            .route(
                "/models",
                axum::routing::get(move |headers: HeaderMap| {
                    let observed = std::sync::Arc::clone(&observed_for_models);
                    async move {
                        let authorization = headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string();
                        observed.lock().unwrap().push(authorization.clone());
                        if authorization == "Bearer kiro-old-model-token" {
                            (
                                axum::http::StatusCode::UNAUTHORIZED,
                                axum::Json(json!({"message": "expired"})),
                            )
                                .into_response()
                        } else {
                            axum::Json(json!({
                                "models": [{"modelId": "kiro-refreshed-model"}]
                            }))
                            .into_response()
                        }
                    }
                }),
            )
            .route(
                "/token",
                axum::routing::post(move || {
                    let requests = std::sync::Arc::clone(&token_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        axum::Json(json!({
                            "accessToken": "kiro-new-model-token",
                            "refreshToken": "kiro-new-model-refresh",
                            "tokenType": "Bearer",
                            "expiresIn": 3600
                        }))
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = provider_api_test_state("kiro-model-catalog-401");
        let token_url = format!("http://{address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                for (id, access_token, refresh_token, raw) in [
                    (
                        "kiro-model-401-bound",
                        "kiro-old-model-token",
                        "kiro-old-model-refresh",
                        Some(json!({
                            "authMethod": "social",
                            "testOAuthTokenUrl": token_url
                        })),
                    ),
                    (
                        "kiro-model-401-decoy",
                        "kiro-decoy-model-token",
                        "kiro-decoy-model-refresh",
                        None,
                    ),
                ] {
                    accounts.upsert(
                        serde_json::from_value(json!({
                            "id": id,
                            "providerType": "kiro_oauth",
                            "accessToken": access_token,
                            "refreshToken": refresh_token,
                            "expiresAt": i64::MAX / 2,
                            "profile": {
                                "profileArn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/test",
                                "apiRegion": "us-east-1",
                                "machineId": id,
                                "authMethod": "social"
                            },
                            "raw": raw
                        }))
                        .unwrap(),
                    );
                }
            })
            .await
            .unwrap();
        let models_url = format!("http://{address}/models");
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: "kiro-model-401-provider".to_string(),
                        name: "Kiro model 401 Provider".to_string(),
                        settings_config: json!({"testKiroModelsUrl": models_url}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("kiro_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("kiro_oauth".to_string()),
                                account_id: Some("kiro-model-401-bound".to_string()),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        let execution =
            resolve_provider_execution_by_key(&state, AppKind::Codex, "kiro-model-401-provider")
                .await
                .unwrap();

        let fetched = fetch_provider_models_inner(&state, &execution, Some(2_000))
            .await
            .unwrap();

        assert_eq!(fetched.models.len(), 1);
        assert_eq!(fetched.models[0].id, "kiro-refreshed-model");
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            observed.lock().unwrap().as_slice(),
            ["Bearer kiro-old-model-token", "Bearer kiro-new-model-token"]
        );
        assert_eq!(
            state
                .find_account_by_id("kiro-model-401-bound")
                .await
                .unwrap()
                .access_token
                .as_deref(),
            Some("kiro-new-model-token")
        );
        assert_eq!(
            state
                .find_account_by_id("kiro-model-401-decoy")
                .await
                .unwrap()
                .access_token
                .as_deref(),
            Some("kiro-decoy-model-token")
        );
        server.abort();
    }

    #[tokio::test]
    async fn kiro_provider_model_discovery_is_empty_while_idc_profile_is_unresolved() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests_for_route = std::sync::Arc::clone(&requests);
        let app = axum::Router::new().route(
            "/models",
            axum::routing::get(move || {
                let requests = std::sync::Arc::clone(&requests_for_route);
                async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::Json(json!({
                        "models": [{"modelId": "must-not-be-requested"}]
                    }))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = provider_api_test_state("kiro-unresolved-idc-model-catalog");
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "kiro-unresolved-idc-model-account",
                        "providerType": "kiro_oauth",
                        "accessToken": "rotated-but-unresolved-access",
                        "expiresAt": i64::MAX / 2,
                        "profile": {
                            "authMethod": "idc",
                            "authRegion": "eu-north-1",
                            "runtimeRegion": "eu-central-1",
                            "profileProvenance": "profile_resolution_required"
                        },
                        "raw": {
                            "authMethod": "idc",
                            "profileProvenance": "profile_resolution_required"
                        }
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        let models_url = format!("http://{address}/models");
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: "kiro-unresolved-idc-model-provider".to_string(),
                        name: "Kiro unresolved IdC model provider".to_string(),
                        settings_config: json!({"testKiroModelsUrl": models_url}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("kiro_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("kiro_oauth".to_string()),
                                account_id: Some("kiro-unresolved-idc-model-account".to_string()),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        let execution = resolve_provider_execution_by_key(
            &state,
            AppKind::Codex,
            "kiro-unresolved-idc-model-provider",
        )
        .await
        .unwrap();

        let fetched = fetch_provider_models_inner(&state, &execution, Some(2_000))
            .await
            .unwrap();

        assert!(fetched.models.is_empty());
        assert_eq!(fetched.source.as_deref(), Some("kiro_identity_unresolved"));
        assert_eq!(fetched.stale, Some(false));
        assert_eq!(fetched.fetched_at_ms, None);
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn claude_oauth_model_discovery_is_static_and_performs_no_oauth_request() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let app = axum::Router::new().route(
            "/token",
            axum::routing::post(move || {
                let requests = std::sync::Arc::clone(&token_requests_for_route);
                async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::Json(json!({"access_token": "must-not-be-requested"}))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = provider_api_test_state("claude-static-model-catalog");
        let token_url = format!("http://{address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "claude-static-model-account",
                        "providerType": "claude_oauth",
                        "accessToken": "expired-access-token",
                        "refreshToken": "refresh-that-must-not-be-used",
                        "expiresAt": 1,
                        "raw": {"testOAuthTokenUrl": token_url}
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        state
            .mutate_providers_immediate(|providers| {
                providers.upsert(
                    AppKind::Claude,
                    Provider {
                        id: "claude-static-model-provider".to_string(),
                        name: "Claude static model provider".to_string(),
                        settings_config: json!({}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("claude_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("claude_oauth".to_string()),
                                account_id: Some("claude-static-model-account".to_string()),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        let execution = resolve_provider_execution_by_key(
            &state,
            AppKind::Claude,
            "claude-static-model-provider",
        )
        .await
        .unwrap();

        let fetched = fetch_provider_models_inner(&state, &execution, Some(2_000))
            .await
            .unwrap();

        assert_eq!(fetched.source.as_deref(), Some("claude_code_wire_profile"));
        assert_eq!(fetched.stale, Some(false));
        assert_eq!(
            fetched.fetched_at_ms,
            Some(crate::clients::oauth::claude_models::CLAUDE_MODEL_CATALOG_CAPTURED_AT_MS)
        );
        assert_eq!(
            fetched
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            crate::clients::oauth::claude_models::CLAUDE_MODEL_IDS
        );
        assert!(fetched.models.iter().all(|model| {
            model.raw["wireProfileId"] == crate::domain::claude_cli::CLAUDE_WIRE_PROFILE.id
                && model.raw["claudeCodeVersion"]
                    == crate::domain::claude_cli::CLAUDE_WIRE_PROFILE.claude_code_version
        }));
        tokio::task::yield_now().await;
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn grok_model_discovery_stops_before_models_when_token_refresh_fails() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let model_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let model_requests_for_route = std::sync::Arc::clone(&model_requests);
        let app = axum::Router::new()
            .route(
                "/token",
                axum::routing::post(move || {
                    let requests = std::sync::Arc::clone(&token_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        (
                            StatusCode::UNAUTHORIZED,
                            axum::Json(json!({"error": "invalid_grant"})),
                        )
                    }
                }),
            )
            .route(
                "/models",
                axum::routing::get(move || {
                    let requests = std::sync::Arc::clone(&model_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        axum::Json(json!({
                            "data": [{"id": "must-not-be-requested"}]
                        }))
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = provider_api_test_state("grok-model-refresh-rejected");
        let token_url = format!("http://{address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "grok-provider-refresh-rejected-account",
                        "providerType": "grok_oauth",
                        "accessToken": "stale-provider-model-access",
                        "refreshToken": "rejected-provider-model-refresh",
                        "expiresAt": 1,
                        "profile": {
                            "verifiedGrokClaims": {
                                "subject": "grok-provider-refresh-rejected-subject"
                            }
                        },
                        "raw": {"testOAuthTokenUrl": token_url}
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        let models_url = format!("http://{address}/models");
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: "grok-provider-refresh-rejected".to_string(),
                        name: "Grok provider refresh rejected".to_string(),
                        settings_config: json!({"testGrokModelsUrl": models_url}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("grok_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("grok_oauth".to_string()),
                                account_id: Some(
                                    "grok-provider-refresh-rejected-account".to_string(),
                                ),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        let execution = resolve_provider_execution_by_key(
            &state,
            AppKind::Codex,
            "grok-provider-refresh-rejected",
        )
        .await
        .unwrap();

        let error = fetch_provider_models_inner(&state, &execution, Some(2_000))
            .await
            .unwrap_err();

        assert!(matches!(
            error.status,
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED
        ));
        assert!(!state.credential_persistence_degraded());
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(model_requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn grok_model_discovery_never_borrows_an_account_for_legacy_api_key_auth() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let model_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let model_requests_for_route = std::sync::Arc::clone(&model_requests);
        let app = axum::Router::new()
            .route(
                "/token",
                axum::routing::post(move || {
                    let requests = std::sync::Arc::clone(&token_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        axum::Json(json!({"access_token": "must-not-be-requested"}))
                    }
                }),
            )
            .route(
                "/models",
                axum::routing::get(move || {
                    let requests = std::sync::Arc::clone(&model_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        axum::Json(json!({"data": [{"id": "must-not-be-requested"}]}))
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = provider_api_test_state("grok-model-legacy-api-key");
        let token_url = format!("http://{address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "grok-model-unrelated-account",
                        "providerType": "grok_oauth",
                        "accessToken": "unrelated-expired-access",
                        "refreshToken": "unrelated-refresh",
                        "expiresAt": 1,
                        "profile": {
                            "verifiedGrokClaims": {"subject": "unrelated-subject"}
                        },
                        "raw": {"testOAuthTokenUrl": token_url}
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        let models_url = format!("http://{address}/models");
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: "grok-model-legacy-api-key".to_string(),
                        name: "Grok legacy API key".to_string(),
                        settings_config: json!({
                            "env": {"OPENAI_API_KEY": "legacy-api-key"},
                            "testGrokModelsUrl": models_url
                        }),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("grok_oauth".to_string()),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        let execution =
            resolve_provider_execution_by_key(&state, AppKind::Codex, "grok-model-legacy-api-key")
                .await
                .unwrap();

        let error = fetch_provider_models_inner(&state, &execution, Some(2_000))
            .await
            .unwrap_err();

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(model_requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        server.abort();
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
    fn codex_oauth_test_model_defaults_to_luna() {
        let stored = StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "p1".to_string(),
                name: "OpenAI OAuth".to_string(),
                settings_config: json!({}),
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
            "gpt-5.6-luna@low"
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

        let value: serde_json::Value = serde_json::from_str(&provider_test_body(
            AppKind::Codex,
            &stored,
            None,
            "ping",
            true,
        ))
        .unwrap();

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
            "bad sk-abc1234567890 and ya29.secret-token and Bearer abc.def\napi_key=unknown-secret\nrequest https://client:password@example.com/v1/models?tenant=private failed",
        );

        assert!(!redacted.contains("sk-abc"));
        assert!(!redacted.contains("ya29.secret"));
        assert!(!redacted.contains("Bearer abc"));
        assert!(!redacted.contains("unknown-secret"));
        assert!(!redacted.contains("client"));
        assert!(!redacted.contains("password"));
        assert!(!redacted.contains("private"));
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
        assert_eq!(
            provider_test_outcome(true, Some(408), Some("request timeout")),
            ProviderOperationOutcome::Timeout
        );
    }

    #[test]
    fn managed_account_refresh_outcome_preserves_retryable_failures_without_a_model_probe() {
        use crate::state::ManagedAccountRefreshError;

        assert_eq!(
            managed_account_refresh_failure_outcome(&ManagedAccountRefreshError::Refresh {
                status_code: 503,
                message: "upstream unavailable".to_string(),
                retry_after_ms: None,
            }),
            ProviderOperationOutcome::Upstream
        );
        assert_eq!(
            managed_account_refresh_failure_outcome(&ManagedAccountRefreshError::Refresh {
                status_code: 504,
                message: "upstream timeout".to_string(),
                retry_after_ms: None,
            }),
            ProviderOperationOutcome::Timeout
        );
        assert_eq!(
            managed_account_refresh_failure_outcome(&ManagedAccountRefreshError::IdentityChanged {
                provider_type: ProviderType::CursorOAuth,
            }),
            ProviderOperationOutcome::InvalidConfig
        );
    }
}
