use super::*;
use crate::domain::providers::bundle::{
    ProviderBundleBindingChange, ProviderBundleRebindPreview, ProviderBundleRuntimeChange,
};

impl ServerStateInner {
    /// Read-only snapshot. The write path always revalidates; a preflight is
    /// information for the administrator, not an authorization token.
    pub async fn provider_bundle_share_preflight(
        &self,
        bundle_id: &str,
    ) -> Option<crate::domain::sharing::credential_source::BundleSharePreflight> {
        let _references = self.lock_reference_mutations().await;
        let providers = self.providers.read().await;
        let accounts = self.accounts.read().await;
        crate::domain::sharing::credential_source::bundle_share_preflight(
            &providers, &accounts, bundle_id,
        )
    }
}

fn bundle_invalid(error: impl std::fmt::Display) -> ProviderCommandError {
    ProviderCommandError::Invalid(error.to_string())
}

fn bundle_surface_drafts(
    draft: &ProviderBundleWriteDraft,
) -> Result<Vec<ProviderWriteDraft>, ProviderCommandError> {
    let family = draft.validate().map_err(bundle_invalid)?;
    let profile = profile_by_id(family.credential_profile_id.as_str())
        .expect("Provider family credential profile is registry-validated");
    let shared = family.surfaces.len() > 1
        && matches!(
            profile.credential_policy,
            CredentialPolicy::StaticSecret { .. } | CredentialPolicy::Aws { .. }
        );
    if matches!(
        profile.credential_policy,
        CredentialPolicy::Custom
            | CredentialPolicy::ManagedAccount { .. }
            | CredentialPolicy::Legacy
    ) && !draft.credential_patches.is_empty()
    {
        return Err(bundle_invalid(
            "bundle credentialPatches are not supported by this Provider family",
        ));
    }
    if shared
        && draft
            .surfaces
            .iter()
            .any(|surface| !surface.credential_patches.is_empty())
    {
        return Err(bundle_invalid(
            "shared Provider credentials must be submitted at Bundle scope",
        ));
    }
    let source = credential_source_app(family).map_err(bundle_invalid)?;
    if !draft.surfaces.iter().any(|surface| surface.app == source) {
        return Err(bundle_invalid(
            "Provider Bundle credential source Surface is missing",
        ));
    }
    draft
        .surfaces
        .iter()
        .map(|surface| {
            let mut patches = surface.credential_patches.clone();
            if shared || family.surfaces.len() == 1 {
                for (slot, patch) in &draft.credential_patches {
                    if patches.insert(slot.clone(), patch.clone()).is_some() {
                        return Err(bundle_invalid(format!(
                            "credential patch slot {slot} is repeated at Bundle and Surface scope"
                        )));
                    }
                }
            }
            Ok(ProviderWriteDraft {
                app: surface.app,
                provider: draft
                    .provider_for_surface(surface)
                    .map_err(bundle_invalid)?,
                profile_id: Some(surface.profile_id.clone()),
                custom_binding: surface.custom_binding.clone(),
                expected_revision: None,
                client_request_id: draft
                    .client_request_id
                    .as_ref()
                    .map(|id| format!("{id}:{}", surface.app.as_str())),
                credential_patches: patches,
            })
        })
        .collect()
}

fn binding_changes(
    existing: &[StoredProvider],
    drafts: &[ProviderWriteDraft],
) -> Vec<ProviderBundleBindingChange> {
    drafts
        .iter()
        .filter_map(|draft| {
            let before = existing
                .iter()
                .find(|stored| stored.app == draft.app)?
                .resource
                .custom_binding
                .as_ref()?;
            let after = draft.custom_binding.as_ref()?;
            (before != after).then(|| ProviderBundleBindingChange {
                app: draft.app,
                before: before.clone(),
                after: after.clone(),
            })
        })
        .collect()
}

/// Mutates a candidate only. Ordinary saves and explicit rebinds share every validation.
fn apply_bundle_candidate(
    store: &mut ProviderStore,
    draft: &ProviderBundleWriteDraft,
    mut provider_drafts: Vec<ProviderWriteDraft>,
    accounts: &AccountStore,
    shares: &ShareStore,
    cursor_verified_identities: &[Option<CursorVerifiedIdentity>],
    binding_mode: ProviderBindingWriteMode,
) -> Result<(ProviderBundleView, bool), ProviderCommandError> {
    let family = draft.validate().map_err(bundle_invalid)?;
    let bundle_id = draft.id.clone();
    let family_id = draft.family_id.clone();
    let expected_revision = draft.expected_revision;
    if store.providers.iter().any(|stored| {
        !is_explicit_bundle_surface(&stored.provider) && stored.provider.id == bundle_id
    }) {
        return Err(provider_management_id_conflict(&bundle_id));
    }
    let existing = store
        .providers
        .iter()
        .filter(|stored| {
            is_explicit_bundle_surface(&stored.provider)
                && provider_bundle_id(&stored.provider) == Some(bundle_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    let actual_revision = existing
        .iter()
        .map(|stored| stored.resource.revision)
        .max()
        .unwrap_or_default();
    let is_create_replay = !existing.is_empty()
        && expected_revision.is_none()
        && provider_drafts.iter().all(|draft| {
            draft
                .client_request_id
                .as_deref()
                .is_some_and(|request_id| {
                    existing.iter().any(|stored| {
                        stored.app == draft.app
                            && stored.resource.create_request_id.as_deref() == Some(request_id)
                    })
                })
        });
    match (existing.is_empty(), expected_revision) {
        (true, Some(expected)) if expected != 0 => {
            return Err(provider_revision_conflict(expected, 0));
        }
        (false, None) if !is_create_replay => {
            return Err(ProviderCommandError::Conflict {
                code: "cc_switch_provider_revision_conflict",
                message: "expectedRevision is required when updating a Provider Bundle".to_string(),
            });
        }
        (false, Some(expected)) if expected != actual_revision => {
            return Err(provider_revision_conflict(expected, actual_revision));
        }
        _ => {}
    }
    if existing.iter().any(|stored| {
        stored
            .provider
            .extra
            .get("familyId")
            .and_then(Value::as_str)
            != Some(family_id.as_str())
    }) {
        return Err(ProviderCommandError::Conflict {
            code: "cc_switch_provider_bundle_family_conflict",
            message: "Provider Bundle family is immutable".to_string(),
        });
    }
    let existing_apps = existing
        .iter()
        .map(|stored| stored.app)
        .collect::<BTreeSet<_>>();
    let requested_apps = provider_drafts
        .iter()
        .map(|provider| provider.app)
        .collect::<BTreeSet<_>>();
    if !existing.is_empty() && existing_apps != requested_apps {
        return Err(ProviderCommandError::Conflict {
            code: "cc_switch_provider_bundle_surface_conflict",
            message: "Provider Bundle Surface set is immutable".to_string(),
        });
    }
    let changes = binding_changes(&existing, &provider_drafts);
    if binding_mode == ProviderBindingWriteMode::CustomBundleRebind {
        if existing.is_empty() {
            return Err(ProviderCommandError::NotFound);
        }
        if expected_revision.is_none() || draft.client_request_id.is_some() {
            return Err(bundle_invalid("custom Bundle rebind requires expectedRevision and does not accept a create clientRequestId"));
        }
        if family.surfaces.iter().any(|surface| {
            !matches!(
                profile_by_id(surface.profile_id.as_str()).map(|profile| &profile.driver_binding),
                Some(DriverBinding::Custom { .. })
            )
        }) {
            return Err(bundle_invalid(
                "rebind-custom only accepts a Custom Provider Bundle",
            ));
        }
        if changes.is_empty() {
            return Err(bundle_invalid(
                "custom Bundle rebind requires a protocol/auth binding change",
            ));
        }
    } else if let Some(change) = changes.first() {
        return Err(ProviderCommandError::Conflict {
            code: "cc_switch_provider_bundle_custom_binding_conflict",
            message: format!("{} Custom Bundle binding changes from {:?} to {:?}; preview and apply the Bundle rebind-custom action", change.app.as_str(), change.before, change.after),
        });
    }
    for provider_draft in &mut provider_drafts {
        provider_draft.expected_revision = existing
            .iter()
            .find(|stored| stored.app == provider_draft.app)
            .map(|stored| stored.resource.revision);
    }
    let current = store.clone();
    let mut result = apply_provider_write_drafts_with_binding_mode(
        store,
        provider_drafts,
        accounts,
        binding_mode,
    )?;
    result.1 |= apply_cursor_verified_identities(
        store,
        &mut result.0,
        &current,
        cursor_verified_identities,
    );
    if result.1 {
        let bundle_revision = actual_revision.saturating_add(1);
        for surface in &mut result.0 {
            if surface.resource.revision == bundle_revision {
                continue;
            }
            surface.resource.revision = bundle_revision;
            *surface = store.upsert_with_resource(
                surface.app,
                surface.provider.clone(),
                surface.resource.clone(),
            );
        }
    }
    validate_provider_bundle_field_scopes(family, &result.0, accounts, store.runtime_defaults())?;
    if result.1 {
        validate_ordinary_provider_subscription_change(&current, store, accounts, shares)?;
    }
    store.validate_for_commit().map_err(bundle_invalid)?;
    store
        .rebuild_runtime_index(accounts)
        .map_err(bundle_invalid)?;
    let views = result
        .0
        .iter()
        .map(|stored| {
            ProviderView::from_stored_with_order_and_runtime(
                stored,
                store.provider_order_index(stored),
                compile_runtime_plan_with_defaults(stored, accounts, store.runtime_defaults()).ok(),
            )
        })
        .collect::<Vec<_>>();
    let bundle = ProviderBundleView::from_surface_views(views)
        .map_err(|error| ProviderCommandError::Invalid(error.to_string()))?;
    Ok((bundle, result.1))
}

fn rebind_preview(
    current: &ProviderStore,
    accounts: &AccountStore,
    shares: &ShareStore,
    preview_key: &[u8; 32],
    draft: &ProviderBundleWriteDraft,
) -> Result<(ProviderStore, ProviderBundleRebindPreview), ProviderCommandError> {
    let drafts = bundle_surface_drafts(draft)?;
    let existing = current
        .providers
        .iter()
        .filter(|stored| {
            is_explicit_bundle_surface(&stored.provider)
                && provider_bundle_id(&stored.provider) == Some(draft.id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    let changes = binding_changes(&existing, &drafts);
    let mut candidate = current.clone();
    let (target, _) = apply_bundle_candidate(
        &mut candidate,
        draft,
        drafts,
        accounts,
        shares,
        &[],
        ProviderBindingWriteMode::CustomBundleRebind,
    )?;
    let share_ids = existing
        .iter()
        .flat_map(|stored| shares.share_ids_for_provider(stored.app, &stored.provider.id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let references = share_ids
        .iter()
        .filter_map(|id| shares.get(id).map(|share| (id, share.config_revision)))
        .collect::<Vec<_>>();
    let preview_token = provider_action_preview_token(
        preview_key,
        b"cc-switch-provider-bundle-rebind-custom-v1",
        current,
        accounts,
        &serde_json::json!({"bundle": draft, "references": references}),
    )?;
    let mut runtimes = BTreeMap::new();
    for before in &existing {
        let after = candidate
            .providers
            .iter()
            .find(|stored| stored.app == before.app && stored.provider.id == before.provider.id)
            .expect("candidate retains the Bundle Surface set");
        let fingerprint = |store: &ProviderStore,
                           stored: &StoredProvider|
         -> Result<Option<String>, ProviderCommandError> {
            if !surface_enabled(&stored.provider) {
                return Ok(None);
            }
            Ok(Some(
                compile_runtime_plan_with_defaults(stored, accounts, store.runtime_defaults())
                    .map_err(bundle_invalid)?
                    .runtime_fingerprint,
            ))
        };
        runtimes.insert(
            before.app,
            ProviderBundleRuntimeChange {
                before_fingerprint: fingerprint(current, before)?,
                after_fingerprint: fingerprint(&candidate, after)?,
            },
        );
    }
    let preview = ProviderBundleRebindPreview {
        preview_token,
        bundle_id: draft.id.clone(),
        source_revision: draft
            .expected_revision
            .expect("rebind candidate requires a revision"),
        target,
        changes,
        runtimes,
        share_ids,
    };
    Ok((candidate, preview))
}

impl ServerStateInner {
    pub async fn upsert_provider_bundle_command(
        self: &Arc<Self>,
        draft: ProviderBundleWriteDraft,
    ) -> anyhow::Result<Result<ProviderBundleView, ProviderCommandError>> {
        let drafts = match bundle_surface_drafts(&draft) {
            Ok(drafts) => drafts,
            Err(error) => return Ok(Err(error)),
        };
        // Bundle drafts always carry authoritative Profile identities. Only
        // Cursor API-key Profiles need pre-commit network verification; running
        // the ordinary Provider preflight for Custom Bundles would bypass the
        // Bundle-level conflict diagnostics before its shared candidate checks.
        let requires_cursor_verification = drafts.iter().any(|draft| {
            draft
                .profile_id
                .as_ref()
                .and_then(|id| profile_by_id(id.as_str()))
                .is_some_and(|profile| {
                    profile.compatibility_provider_type == Some(ProviderType::CursorApiKey)
                })
        });
        let identities = if requires_cursor_verification {
            match self
                .validate_cursor_api_key_provider_drafts(&drafts, true)
                .await
            {
                Ok(identities) => identities,
                Err(error) => return Ok(Err(error)),
            }
        } else {
            vec![None; drafts.len()]
        };
        let guard = self.reference_mutations.clone().lock_owned().await;
        let accounts = self.accounts.read().await.clone();
        let shares = self.shares.read().await.clone();
        self.commit_provider_change_under_reference_guard(guard, move |store| {
            apply_bundle_candidate(
                store,
                &draft,
                drafts,
                &accounts,
                &shares,
                &identities,
                ProviderBindingWriteMode::Ordinary,
            )
        })
        .await
    }

    pub async fn preview_rebind_custom_provider_bundle_command(
        &self,
        draft: ProviderBundleWriteDraft,
    ) -> anyhow::Result<Result<ProviderBundleRebindPreview, ProviderCommandError>> {
        let _guard = self.reference_mutations.lock().await;
        let current = self.providers.read().await.materialized_clone()?;
        let accounts = self.accounts.read().await.clone();
        let shares = self.shares.read().await.clone();
        Ok(rebind_preview(
            &current,
            &accounts,
            &shares,
            &self.provider_import_preview_key,
            &draft,
        )
        .map(|(_, preview)| preview))
    }

    pub async fn apply_rebind_custom_provider_bundle_command(
        self: &Arc<Self>,
        draft: ProviderBundleWriteDraft,
        token: String,
    ) -> anyhow::Result<Result<ProviderBundleRebindPreview, ProviderCommandError>> {
        let guard = self.reference_mutations.clone().lock_owned().await;
        let accounts = self.accounts.read().await.clone();
        let shares = self.shares.read().await.clone();
        let key = self.provider_import_preview_key;
        let result = self
            .commit_provider_change_under_reference_guard(guard, move |store| {
                let (candidate, preview) = rebind_preview(store, &accounts, &shares, &key, &draft)?;
                validate_provider_action_preview_token(&preview.preview_token, &token)?;
                *store = candidate;
                Ok((preview, true))
            })
            .await?;
        if let Ok(preview) = &result {
            tracing::info!(bundle_id = %preview.bundle_id, source_revision = preview.source_revision,
                changes = ?preview.changes, "custom Provider Bundle rebound");
        }
        Ok(result)
    }
}
