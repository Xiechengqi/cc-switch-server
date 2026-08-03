use super::*;

use crate::domain::sharing::router_contract::descriptor_for_share_with_accounts_and_usage;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpsertShareCommand {
    #[serde(default)]
    pub(in crate::api) expected_config_revision: Option<u64>,
    #[serde(flatten)]
    pub(in crate::api) input: UpsertShareInput,
}

pub(in crate::api) async fn list_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(ListSharesResponse {
        ok: true,
        shares: state.shares.read().await.shares.clone(),
    }))
}

pub(in crate::api) async fn export_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListSharesResponse>, ApiError> {
    list_shares(State(state), headers).await
}

pub(in crate::api) async fn import_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<ImportSharesRequest>,
) -> Result<Json<ImportSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    for share in &input.shares {
        crate::domain::sharing::invariants::validate_share_import(share)
            .map_err(map_share_patch_error)?;
    }
    let owner_email = state
        .config
        .read()
        .await
        .owner
        .email
        .clone()
        .ok_or_else(|| ApiError::conflict("client owner email is not configured"))?;
    let reference_guard = state.lock_reference_mutations().await;
    let (providers, accounts) = {
        let providers = state.providers.read().await;
        for share in &input.shares {
            for binding in &share.bindings {
                validate_share_provider_reference(
                    &providers,
                    binding.app,
                    &binding.provider_id,
                    binding.provider_type,
                )?;
            }
        }
        (providers.clone(), state.accounts.read().await.clone())
    };
    let root_key =
        crate::infra::credentials::load_root_key(&state.config_dir).map_err(ApiError::internal)?;
    for share in &mut input.shares {
        share.capacity_pool_id =
            crate::domain::sharing::credential_source::capacity_pool_id_for_bindings(
                &providers,
                &accounts,
                &share.bindings,
                &root_key.key,
            )
            .map_err(map_credential_source_error)?;
    }
    let mut imported_store = ShareStore {
        shares: std::mem::take(&mut input.shares),
        ..ShareStore::default()
    };
    let imported_share_ids = imported_store
        .shares
        .iter()
        .map(|share| share.id.clone())
        .collect::<Vec<_>>();
    for share_id in imported_share_ids {
        imported_store
            .canonicalize_primary_app_settings(&share_id)
            .map_err(map_share_patch_error)?;
    }
    let owner_normalized = imported_store
        .bind_all_to_client_owner(&owner_email)
        .map_err(map_share_patch_error)?
        .len();
    input.shares = imported_store.shares;
    let imported = state
        .try_mutate_shares_immediate(|store| {
            let mut candidate = store.clone();
            let imported = candidate
                .import_shares(input.shares)
                .map_err(map_share_patch_error)?;
            crate::domain::sharing::subscription_identity::validate_subscription_reference_graph_transition(
                &providers,
                &accounts,
                store,
                &providers,
                &accounts,
                &candidate,
            )
            .map_err(map_subscription_binding_error)?;
            *store = candidate;
            Ok::<_, ApiError>(imported)
        })
        .await
        .map_err(ApiError::internal)??;
    drop(reference_guard);
    state.emit_event(
        ServerEvent::new("share.imported", "share").message(format!("imported {imported} shares")),
    );
    Ok(Json(ImportSharesResponse {
        ok: true,
        imported,
        owner_normalized,
    }))
}

pub(in crate::api) async fn upsert_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(command): Json<UpsertShareCommand>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let UpsertShareCommand {
        expected_config_revision,
        mut input,
    } = command;
    crate::domain::sharing::invariants::validate_and_normalize_upsert_input(&mut input)
        .map_err(map_share_patch_error)?;
    input.owner_email = Some(
        state
            .config
            .read()
            .await
            .owner
            .email
            .clone()
            .ok_or_else(|| ApiError::conflict("client owner email is not configured"))?,
    );
    let reference_guard = state.lock_reference_mutations().await;
    let (providers, accounts) = {
        let providers = state.providers.read().await;
        for binding in &input.bindings {
            validate_share_provider_reference(
                &providers,
                binding.app,
                &binding.provider_id,
                binding.provider_type,
            )?;
        }
        (providers.clone(), state.accounts.read().await.clone())
    };
    let root_key =
        crate::infra::credentials::load_root_key(&state.config_dir).map_err(ApiError::internal)?;
    let capacity_pool_id =
        crate::domain::sharing::credential_source::capacity_pool_id_for_bindings(
            &providers,
            &accounts,
            &input.bindings,
            &root_key.key,
        )
        .map_err(map_credential_source_error)?;
    let previous = {
        let shares = state.shares.read().await;
        input
            .id
            .as_deref()
            .and_then(|id| shares.get(id))
            .or_else(|| {
                shares
                    .shares
                    .iter()
                    .find(|share| share.app == input.app && share.provider_id == input.provider_id)
            })
            .cloned()
    };
    match (previous.as_ref(), expected_config_revision) {
        (Some(previous), Some(expected)) if previous.config_revision != expected => {
            return Err(ApiError::conflict_code(
                "cc_switch_share_revision_conflict",
                format!(
                    "Share changed since this editor was opened (expected revision {expected}, current revision {})",
                    previous.config_revision
                ),
            ));
        }
        (None, Some(_)) => {
            return Err(ApiError::conflict_code(
                "cc_switch_share_revision_conflict",
                "cannot apply expectedConfigRevision to a new Share",
            ));
        }
        _ => {}
    }
    let share = state
        .try_mutate_shares_immediate(|store| {
            let current = input
                .id
                .as_deref()
                .and_then(|id| store.get(id))
                .or_else(|| {
                    store.shares.iter().find(|share| {
                        share.app == input.app && share.provider_id == input.provider_id
                    })
                });
            match (current, expected_config_revision) {
                (Some(current), Some(expected)) if current.config_revision != expected => {
                    return Err(ApiError::conflict_code(
                        "cc_switch_share_revision_conflict",
                        format!(
                            "Share changed since this editor was opened (expected revision {expected}, current revision {})",
                            current.config_revision
                        ),
                    ));
                }
                (None, Some(_)) => {
                    return Err(ApiError::conflict_code(
                        "cc_switch_share_revision_conflict",
                        "cannot apply expectedConfigRevision to a new Share",
                    ));
                }
                _ => {}
            }
            let mut candidate = store.clone();
            let share = candidate
                .upsert_with_capacity(input, Some(capacity_pool_id))
                .map_err(map_share_patch_error)?;
            crate::domain::sharing::subscription_identity::validate_subscription_reference_graph_transition(
                &providers,
                &accounts,
                store,
                &providers,
                &accounts,
                &candidate,
            )
            .map_err(map_subscription_binding_error)?;
            *store = candidate;
            Ok::<_, ApiError>(share)
        })
        .await
        .map_err(ApiError::internal)??;
    drop(reference_guard);
    spawn_share_upsert_sync(state.clone(), share.clone());
    let was_running = previous
        .as_ref()
        .is_some_and(crate::state::should_restore_share_tunnel);
    let should_run = crate::state::should_restore_share_tunnel(&share);
    if was_running && !should_run {
        crate::state::stop_share_tunnel(&state, &share.id).await;
    } else if should_run
        && previous
            .as_ref()
            .is_some_and(|previous| previous.tunnel_subdomain != share.tunnel_subdomain)
    {
        crate::state::force_reconnect_share_tunnel(
            state.clone(),
            share.id.clone(),
            "share_subdomain_changed",
        )
        .await;
    } else if should_run {
        crate::state::ensure_share_tunnel_running_for(state.clone(), &share.id, "share_upsert")
            .await;
    }
    emit_share_event(&state, "share.changed", &share, "upserted");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

fn validate_share_provider_reference(
    providers: &crate::domain::providers::store::ProviderStore,
    app: AppKind,
    provider_id: &str,
    provider_type: ProviderType,
) -> Result<(), ApiError> {
    let stored = providers
        .providers
        .iter()
        .find(|stored| stored.app == app && stored.provider.id == provider_id)
        .ok_or_else(|| ApiError::not_found("share Provider not found"))?;
    if stored.provider_type != provider_type {
        return Err(ApiError::bad_request(format!(
            "share providerType {} does not match Provider {}",
            provider_type.as_str(),
            stored.provider_type.as_str()
        )));
    }
    Ok(())
}

pub(in crate::api) fn map_credential_source_error(
    error: crate::domain::sharing::credential_source::CredentialSourceError,
) -> ApiError {
    use crate::domain::sharing::credential_source::CredentialSourceError;

    match error {
        CredentialSourceError::Resolution { .. }
        | CredentialSourceError::CapacityPoolDerivation { .. } => ApiError::internal(error),
        _ => ApiError::bad_request_code(error.code(), error.to_string()),
    }
}

pub(in crate::api) async fn share_reuse_candidates(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ShareReuseCandidatesQuery>,
) -> Result<Json<ShareReuseCandidatesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    validate_share_provider_reference(
        &providers,
        query.app,
        &query.provider_id,
        providers
            .providers
            .iter()
            .find(|provider| provider.app == query.app && provider.provider.id == query.provider_id)
            .map(|provider| provider.provider_type)
            .ok_or_else(|| ApiError::not_found("share Provider not found"))?,
    )?;
    let Some(source) =
        crate::domain::sharing::credential_source::resolve_provider_credential_source(
            &providers,
            &accounts,
            query.app,
            &query.provider_id,
        )
        .map_err(ApiError::internal)?
    else {
        return Ok(Json(ShareReuseCandidatesResponse {
            ok: true,
            candidates: Vec::new(),
        }));
    };
    let shares = state.shares.read().await;
    let mut candidates = Vec::new();
    for share in shares
        .shares
        .iter()
        .filter(|share| share.enabled && share.status == "active")
    {
        if share
            .bindings
            .iter()
            .any(|binding| binding.app == query.app)
        {
            continue;
        }
        let existing_source =
            crate::domain::sharing::credential_source::shared_credential_source_for_bindings(
                &providers,
                &accounts,
                &share.bindings,
            )
            .map_err(map_credential_source_error)?;
        if existing_source.as_ref() != Some(&source) {
            continue;
        }
        candidates.push(ShareReuseCandidate {
            share_id: share.id.clone(),
            share_name: share
                .display_name
                .clone()
                .unwrap_or_else(|| share.id.clone()),
            subdomain: share.tunnel_subdomain.clone(),
            apps: share.bindings.iter().map(|binding| binding.app).collect(),
            config_revision: share.config_revision,
        });
    }
    candidates.sort_by(|left, right| left.share_name.cmp(&right.share_name));
    Ok(Json(ShareReuseCandidatesResponse {
        ok: true,
        candidates,
    }))
}

pub(in crate::api) async fn add_share_binding(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<AddShareBindingRequest>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let _references = state.lock_reference_mutations().await;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let stored = providers
        .providers
        .iter()
        .find(|provider| provider.app == input.app && provider.provider.id == input.provider_id)
        .ok_or_else(|| ApiError::not_found("share Provider not found"))?;
    let binding = ShareBinding {
        app: input.app,
        provider_id: input.provider_id.clone(),
        provider_type: stored.provider_type,
    };
    let current = state
        .shares
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    if current.config_revision != input.expected_config_revision {
        return Err(ApiError::conflict_code(
            "cc_switch_share_revision_conflict",
            "Share changed since reuse was confirmed",
        ));
    }
    let mut next_bindings = current.bindings.clone();
    next_bindings.push(binding.clone());
    next_bindings.sort_by_key(|binding| binding.app);
    let root_key =
        crate::infra::credentials::load_root_key(&state.config_dir).map_err(ApiError::internal)?;
    let capacity_pool_id =
        crate::domain::sharing::credential_source::capacity_pool_id_for_bindings(
            &providers,
            &accounts,
            &next_bindings,
            &root_key.key,
        )
        .map_err(map_credential_source_error)?;
    let share = state
        .try_mutate_shares_immediate(|store| {
            let current = store
                .get(&id)
                .ok_or_else(|| ApiError::not_found("share not found"))?;
            if current.config_revision != input.expected_config_revision {
                return Err(ApiError::conflict_code(
                    "cc_switch_share_revision_conflict",
                    "Share changed since reuse was confirmed",
                ));
            }
            let mut candidate = store.clone();
            let share = candidate
                .add_binding_with_capacity(&id, binding, capacity_pool_id)
                .map_err(map_share_patch_error)?;
            crate::domain::sharing::subscription_identity::validate_subscription_reference_graph_transition(
                &providers,
                &accounts,
                store,
                &providers,
                &accounts,
                &candidate,
            )
            .map_err(map_subscription_binding_error)?;
            *store = candidate;
            Ok::<_, ApiError>(share)
        })
        .await
        .map_err(ApiError::internal)??;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "binding_added");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn remove_share_binding(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<RemoveShareBindingRequest>,
) -> Result<Json<RemoveShareBindingResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let _references = state.lock_reference_mutations().await;
    let current = state
        .shares
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    if current.config_revision != input.expected_config_revision {
        return Err(ApiError::conflict_code(
            "cc_switch_share_revision_conflict",
            "Share changed since this Provider page was loaded",
        ));
    }
    if !current
        .bindings
        .iter()
        .any(|binding| binding.app == input.app && binding.provider_id == input.provider_id)
    {
        return Err(ApiError::bad_request("provider is not bound to this Share"));
    }
    if current.bindings.len() == 1 {
        let tombstone = match state
            .delete_share_immediate_at_revision(&id, input.expected_config_revision)
            .await
            .map_err(ApiError::internal)?
        {
            Ok(tombstone) => tombstone,
            Err(crate::state::ConditionalShareDeleteError::NotFound) => {
                return Err(ApiError::not_found("share not found"));
            }
            Err(crate::state::ConditionalShareDeleteError::RevisionConflict {
                current_revision,
            }) => {
                return Err(ApiError::conflict_code(
                    "cc_switch_share_revision_conflict",
                    format!(
                        "Share changed since this Provider page was loaded (expected revision {}, current revision {current_revision})",
                        input.expected_config_revision
                    ),
                ));
            }
            Err(crate::state::ConditionalShareDeleteError::InFlight) => {
                return Err(share_delete_in_flight_error());
            }
        };
        crate::state::stop_share_tunnel(&state, &id).await;
        spawn_share_delete_sync(state.clone(), tombstone);
        state.emit_event(
            ServerEvent::new("share.deleted", "share")
                .id(id)
                .message("final binding removed"),
        );
        return Ok(Json(RemoveShareBindingResponse {
            ok: true,
            deleted_share: true,
            share: None,
        }));
    }
    let share = state
        .try_mutate_shares_immediate(|store| {
            let current = store
                .get(&id)
                .ok_or_else(|| ApiError::not_found("share not found"))?;
            if current.config_revision != input.expected_config_revision {
                return Err(ApiError::conflict_code(
                    "cc_switch_share_revision_conflict",
                    "Share changed since this Provider page was loaded",
                ));
            }
            store
                .remove_binding(&id, input.app, &input.provider_id)
                .map_err(map_share_patch_error)
        })
        .await
        .map_err(ApiError::internal)??;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "binding_removed");
    Ok(Json(RemoveShareBindingResponse {
        ok: true,
        deleted_share: false,
        share: Some(share),
    }))
}

pub(in crate::api) async fn update_share_binding(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateShareBindingRequest>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let reference_guard = state.lock_reference_mutations().await;
    let (providers, accounts, usage, app, current_bindings) = {
        let providers = state.providers.read().await;
        let accounts = state.accounts.read().await;
        let usage = state.usage.read().await;
        let shares = state.shares.read().await;
        let share = shares
            .get(&id)
            .ok_or_else(|| ApiError::not_found("share not found"))?;
        validate_share_provider_reference(
            &providers,
            share.app,
            &input.provider_id,
            input.provider_type,
        )?;
        (
            providers.clone(),
            accounts.clone(),
            usage.clone(),
            share.app,
            share.bindings.clone(),
        )
    };
    let binding = ShareBinding {
        app,
        provider_id: input.provider_id.clone(),
        provider_type: input.provider_type,
    };
    let mut next_bindings = current_bindings;
    let next_binding = next_bindings
        .iter_mut()
        .find(|candidate| candidate.app == app)
        .ok_or_else(|| ApiError::bad_request("share binding app must match share.app"))?;
    *next_binding = binding.clone();
    let root_key =
        crate::infra::credentials::load_root_key(&state.config_dir).map_err(ApiError::internal)?;
    let capacity_pool_id =
        crate::domain::sharing::credential_source::capacity_pool_id_for_bindings(
            &providers,
            &accounts,
            &next_bindings,
            &root_key.key,
        )
        .map_err(map_credential_source_error)?;

    let share = state
        .try_mutate_shares_immediate(|store| {
            let current = store
                .get(&id)
                .ok_or_else(|| ApiError::not_found("share not found"))?;
            if current.config_revision != input.expected_config_revision {
                return Err(ApiError::conflict_code(
                    "cc_switch_share_revision_conflict",
                    format!(
                        "Share changed since this binding operation was opened (expected revision {}, current revision {})",
                        input.expected_config_revision, current.config_revision
                    ),
                ));
            }

            let mut candidate = store.clone();
            candidate
                .update_binding_with_capacity(
                    &id,
                    binding,
                    capacity_pool_id,
                )
                .map_err(|error| match error {
                    crate::domain::sharing::shares::ShareUpdateError::NotFound => {
                        ApiError::not_found("share not found")
                    }
                    crate::domain::sharing::shares::ShareUpdateError::MustBePaused => {
                        ApiError::conflict_code(
                            "cc_switch_share_must_be_paused",
                            "share must be paused before updating binding",
                        )
                    }
                    crate::domain::sharing::shares::ShareUpdateError::InvalidApp => {
                        ApiError::bad_request("share binding app must match share.app")
                    }
                    crate::domain::sharing::shares::ShareUpdateError::ProviderAlreadyShared => {
                        ApiError::conflict_code(
                            "cc_switch_provider_already_shared",
                            "provider already has an active share",
                        )
                    }
                })?;
            let provider_keys = std::collections::BTreeSet::from([(
                app,
                input.provider_id.clone(),
            )]);
            candidate.refresh_runtime_snapshots_for_providers(
                &provider_keys,
                &providers,
                Some(&accounts),
                &usage,
            );
            crate::domain::sharing::subscription_identity::validate_subscription_reference_graph_transition(
                &providers,
                &accounts,
                store,
                &providers,
                &accounts,
                &candidate,
            )
            .map_err(map_subscription_binding_error)?;
            let share = candidate
                .get(&id)
                .cloned()
                .ok_or_else(|| ApiError::not_found("share not found"))?;
            *store = candidate;
            Ok::<_, ApiError>(share)
        })
        .await
        .map_err(ApiError::internal)??;
    drop(reference_guard);

    crate::state::stop_share_tunnel(&state, &share.id).await;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "binding_updated");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn share_connect_info(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ShareConnectInfoResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await.clone();
    let share = state
        .shares
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    Ok(Json(connect_info_for_share(&config, &share)?))
}

pub(in crate::api) async fn update_share_subdomain(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateShareSubdomainRequest>,
) -> Result<Json<UpdateShareSubdomainResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let subdomain = crate::domain::sharing::shares::normalize_share_subdomain(&input.subdomain)
        .map_err(ApiError::bad_request)?;
    let config = state.config.read().await.clone();
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let current = state
        .shares
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    if input
        .expected_config_revision
        .is_some_and(|expected| expected != current.config_revision)
    {
        return Err(ApiError::conflict_code(
            "cc_switch_share_revision_conflict",
            format!(
                "Share changed since this editor was opened (expected revision {}, current revision {})",
                input.expected_config_revision.unwrap_or_default(),
                current.config_revision
            ),
        ));
    }
    let expected_config_revision = current.config_revision;
    let mut candidate = current.clone();
    candidate.tunnel_subdomain = Some(subdomain.clone());
    let descriptor = descriptor_for_share_with_accounts_and_usage(
        &candidate,
        &providers,
        Some(&accounts),
        Some(&usage),
    );
    let mut remote_claimed = false;
    if config.has_registered_router_identity() {
        let http_client = state.http_client().await;
        if let Err(error) =
            crate::clients::router::client::claim_share_subdomain(&http_client, &config, descriptor)
                .await
        {
            if let Err(reconcile_error) =
                crate::state::reconcile_router_share_after_failed_claim(&state, &id).await
            {
                tracing::warn!(
                    share_id = %id,
                    error = %reconcile_error,
                    "Router Share reconciliation after an uncertain subdomain claim failed"
                );
            }
            return Err(ApiError::bad_gateway(error.to_string()));
        }
        remote_claimed = true;
    }
    let share = match state
        .try_mutate_shares_immediate(|store| {
            let current = store
                .get(&id)
                .ok_or_else(|| ApiError::not_found("share not found"))?;
            if current.config_revision != expected_config_revision {
                return Err(ApiError::conflict_code(
                    "cc_switch_share_revision_conflict",
                    format!(
                        "Share changed during the subdomain claim (expected revision {}, current revision {})",
                        expected_config_revision, current.config_revision
                    ),
                ));
            }
            store
                .update_subdomain(&id, subdomain)
                .map_err(map_share_patch_error)
        })
        .await
    {
        Ok(Ok(share)) => share,
        Ok(Err(error)) => {
            if remote_claimed {
                if let Err(reconcile_error) =
                    crate::state::reconcile_router_share_after_failed_claim(&state, &id).await
                {
                    tracing::warn!(
                        share_id = %id,
                        error = %reconcile_error,
                        "Router Share reconciliation after a rejected local subdomain update failed"
                    );
                }
            }
            return Err(error);
        }
        Err(error) => {
            if remote_claimed {
                if let Err(reconcile_error) =
                    crate::state::reconcile_router_share_after_failed_claim(&state, &id).await
                {
                    tracing::warn!(
                        share_id = %id,
                        error = %reconcile_error,
                        "Router Share reconciliation after a failed local subdomain save failed"
                    );
                }
            }
            return Err(ApiError::internal(error));
        }
    };
    spawn_share_upsert_sync(state.clone(), share.clone());
    crate::state::force_reconnect_share_tunnel(
        state.clone(),
        share.id.clone(),
        "share_subdomain_changed",
    )
    .await;
    emit_share_event(&state, "share.changed", &share, "subdomain_updated");
    Ok(Json(UpdateShareSubdomainResponse {
        ok: true,
        remote_claimed,
        share,
    }))
}

pub(in crate::api) async fn delete_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<DeleteResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let tombstone = state
        .delete_share_immediate(&id)
        .await
        .map_err(ApiError::internal)?
        .map_err(|error| match error {
            crate::state::ShareDeleteError::InFlight => share_delete_in_flight_error(),
        })?;
    if let Some(tombstone) = tombstone.as_ref() {
        crate::state::stop_share_tunnel(&state, &id).await;
        spawn_share_delete_sync(state.clone(), tombstone.clone());
        state.emit_event(
            ServerEvent::new("share.deleted", "share")
                .id(id.clone())
                .message("deleted"),
        );
    }
    Ok(Json(DeleteResponse {
        ok: true,
        deleted: tombstone.is_some(),
    }))
}

fn share_delete_in_flight_error() -> ApiError {
    ApiError::conflict_code(
        "cc_switch_share_in_flight",
        "Share cannot be deleted while requests are in flight",
    )
}

pub(in crate::api) async fn pause_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .pause(&id)
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
        .map_err(ApiError::internal)??;
    crate::state::stop_share_tunnel(&state, &share.id).await;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "paused");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn resume_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .resume(&id)
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
        .map_err(ApiError::internal)??;
    spawn_share_upsert_sync(state.clone(), share.clone());
    crate::state::ensure_share_tunnel_running_for(state.clone(), &share.id, "share_resumed").await;
    emit_share_event(&state, "share.changed", &share, "resumed");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn start_share_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .set_share_tunnel_status(&id, "active", None)
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
        .map_err(ApiError::internal)??;
    crate::state::ensure_share_tunnel_running_for(state.clone(), &id, "share_tunnel_api_start")
        .await;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "tunnel_started");
    emit_tunnel_event(&state, "tunnel.changed", &share.id, "share_started");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn stop_share_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .set_share_tunnel_status(&id, "stopped", None)
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
        .map_err(ApiError::internal)??;
    crate::state::stop_share_tunnel(&state, &id).await;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "tunnel_stopped");
    emit_tunnel_event(&state, "tunnel.changed", &share.id, "share_stopped");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn restore_share_tunnels(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let shares = state
        .mutate_shares_immediate(|store| store.restore_auto_start())
        .await
        .map_err(ApiError::internal)?;
    for share in shares
        .iter()
        .filter(|share| crate::state::should_restore_share_tunnel(share))
    {
        crate::state::ensure_share_tunnel_running_for(
            state.clone(),
            &share.id,
            "share_tunnel_restore",
        )
        .await;
        spawn_share_upsert_sync(state.clone(), share.clone());
        emit_share_event(&state, "share.changed", share, "tunnel_restored");
        emit_tunnel_event(&state, "tunnel.changed", &share.id, "share_restored");
    }
    Ok(Json(ListSharesResponse { ok: true, shares }))
}

pub(in crate::api) async fn reset_share_usage(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .reset_usage(&id)
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
        .map_err(ApiError::internal)??;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "usage_reset");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn replace_share_acl(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ReplaceShareAclRequest>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .replace_acl(&id, input.acl)
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
        .map_err(ApiError::internal)??;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "acl_replaced");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn list_token_markets(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListTokenMarketsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let markets = fetch_public_token_markets_from_router(&state).await?;
    Ok(Json(ListTokenMarketsResponse { ok: true, markets }))
}

pub(in crate::api) async fn refresh_share_snapshots(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let shares = state
        .mutate_shares_debounced(|store| {
            store.refresh_runtime_snapshots(&providers, Some(&accounts), &usage)
        })
        .await;
    state.emit_event(ServerEvent::new("share.changed", "share").message("runtime_snapshot"));
    Ok(Json(ListSharesResponse { ok: true, shares }))
}

pub(in crate::api) fn emit_share_event(
    state: &ServerState,
    event_type: &str,
    share: &Share,
    message: &str,
) {
    state.emit_event(
        ServerEvent::new(event_type, "share")
            .id(share.id.clone())
            .app(share.app)
            .message(message),
    );
}

pub(in crate::api) fn emit_tunnel_event(
    state: &ServerState,
    event_type: &str,
    tunnel_id: &str,
    message: &str,
) {
    state.emit_event(
        ServerEvent::new(event_type, "tunnel")
            .id(tunnel_id.to_string())
            .message(message),
    );
}

pub(in crate::api) fn connect_info_for_share(
    config: &ServerConfig,
    share: &Share,
) -> Result<ShareConnectInfoResponse, ApiError> {
    let share_slug = share
        .tunnel_subdomain
        .clone()
        .ok_or_else(|| ApiError::conflict("share slug is not configured"))?;
    let share_slug = crate::domain::router::ShareSlug::parse(&share_slug)
        .map_err(|error| ApiError::conflict(error.to_string()))?;
    let client_subdomain = config
        .client
        .tunnel_subdomain
        .as_deref()
        .ok_or_else(|| ApiError::conflict("client subdomain is not configured"))
        .and_then(|value| {
            crate::domain::router::ClientSubdomain::parse(value)
                .map_err(|error| ApiError::conflict(error.to_string()))
        })?;
    let subdomain = format!("{}--{}", share_slug, client_subdomain);
    let router_domain = config
        .router
        .domain
        .clone()
        .or_else(|| router_domain_from_url(config.router.url.as_deref()))
        .ok_or_else(|| ApiError::conflict("router domain is not configured"))?;
    let direct_url = format!("https://{subdomain}.{router_domain}");
    let snippets = [
        (
            AppKind::Claude,
            "Claude / Anthropic",
            vec![
                ("ANTHROPIC_BASE_URL", direct_url.clone()),
                ("ANTHROPIC_AUTH_TOKEN", "<user_api_token>".to_string()),
            ],
        ),
        (
            AppKind::Codex,
            "Codex / OpenAI-compatible",
            vec![
                (
                    "OPENAI_BASE_URL",
                    format!("{}/v1", direct_url.trim_end_matches('/')),
                ),
                ("OPENAI_API_KEY", "<user_api_token>".to_string()),
            ],
        ),
        (
            AppKind::Gemini,
            "Gemini",
            vec![
                ("GEMINI_BASE_URL", direct_url.clone()),
                ("GEMINI_API_KEY", "<user_api_token>".to_string()),
            ],
        ),
    ]
    .into_iter()
    .filter(|(app, _, _)| share.bindings.iter().any(|binding| binding.app == *app))
    .map(|(app, title, values)| {
        let env = values
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<BTreeMap<_, _>>();
        ShareConnectSnippet {
            app,
            title: title.to_string(),
            env,
        }
    })
    .collect::<Vec<_>>();
    Ok(ShareConnectInfoResponse {
        ok: true,
        share_id: share.id.clone(),
        direct_url,
        subdomain,
        router_domain,
        snippets,
        note: "The caller must use their own cc-switch user_api_token as the bearer/API key."
            .to_string(),
    })
}

pub(in crate::api) fn router_domain_from_url(url: Option<&str>) -> Option<String> {
    let value = url?.trim();
    let without_scheme = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    without_scheme
        .split('/')
        .next()
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_string)
}

pub(in crate::api) async fn fetch_public_token_markets_from_router(
    state: &ServerState,
) -> Result<Vec<PublicTokenMarket>, ApiError> {
    let config = state.config.read().await.clone();
    let api_base = config
        .router_api_base()
        .ok_or_else(|| ApiError::conflict("router API base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let http_client = state.http_client().await;
    let response = http_client
        .get(format!("{api_base}/v1/markets"))
        .send()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("fetch token markets failed: {error}")))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::bad_gateway(format!(
            "fetch token markets failed: {status}: {body}"
        )));
    }
    let response = response
        .json::<ListTokenMarketsResponse>()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("parse token markets failed: {error}")))?;
    Ok(response
        .markets
        .into_iter()
        .filter(|market| market.market_kind == "usage")
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_info_uses_canonical_share_host_and_router_domain() {
        let mut config = ServerConfig::empty();
        config.client.tunnel_subdomain = Some("client-alpha".to_string());
        config.router.domain = Some("router.example.com".to_string());
        config.router.api_base = Some("https://api.internal.example".to_string());
        let share: Share = serde_json::from_value(serde_json::json!({
            "id": "share-1",
            "app": "codex",
            "providerId": "provider-1",
            "providerType": "codex",
            "tunnelSubdomain": "codex-pro",
            "routerUrl": "https://stale.example.com"
        }))
        .expect("minimal Share fixture must deserialize");

        let response = connect_info_for_share(&config, &share).expect("connect info");
        assert_eq!(response.subdomain, "codex-pro--client-alpha");
        assert_eq!(
            response.direct_url,
            "https://codex-pro--client-alpha.router.example.com"
        );
        assert_eq!(response.router_domain, "router.example.com");
    }
}
