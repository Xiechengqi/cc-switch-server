// Included in state tests. Resource readiness is isolated by grant and checked
// again when a scheduled policy becomes effective. Executed in CI only.
#[tokio::test]
async fn value_quota_resource_readiness_is_per_grant_and_fences_scheduled_activation() {
    use crate::domain::sharing::router_contract::{
        PendingValueQuota, ShareUserGrant, ShareUserPolicy,
    };
    use crate::domain::usage::value_pricing::{PricingSnapshot, ValueMeteringBinding, ValueQuota};
    use crate::repository::server_sqlite::{self, value_quota as ledger};
    let state = test_state();
    state
        .configure_value_quota_router_for_test("resource-policy-install")
        .await;
    server_sqlite::activate_authority(&state.config_dir).unwrap();
    state
        .upsert_provider_command(
            AppKind::Codex,
            test_provider("provider", "Provider"),
            Some(ProfileId::parse("codex.openai_api_key").unwrap()),
            None,
            None,
            test_api_key_credential(),
        )
        .await
        .unwrap()
        .unwrap();
    state
        .mutate_shares_immediate(|store| {
            store
                .upsert(router_sync_share_input("resource-policy-share", "provider"))
                .unwrap();
        })
        .await
        .unwrap();
    let config = state.config_snapshot().await;
    let scope = ledger::Scope {
        router_url: config.router_api_base().unwrap().into(),
        installation_id: "resource-policy-install".into(),
        share_id: "resource-policy-share".into(),
        email: "owner@example.com".into(),
        entitlement_id: None,
    };
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/share-value-pricing-v1.json"
    ))
    .unwrap();
    let pricing: PricingSnapshot = serde_json::from_value(fixture["snapshot"].clone()).unwrap();
    let revision = pricing.revision().unwrap();
    let now = now_ms_i64();
    ledger::install_snapshot(&state.config_dir, &scope, &revision, &pricing, now).unwrap();
    ledger::activate_snapshot(&state.config_dir, &scope, &revision).unwrap();
    let providers = state.providers.read().await.clone();
    let usage = state.usage.read().await.clone();
    let mut previous = state.shares.read().await.clone();
    let policy = |tariff: &str| ShareUserPolicy {
        value_quota: Some(ValueQuota {
            limit_usd_micros: "1000000".into(),
            tariff_policy_id: tariff.into(),
        }),
        token_period: crate::domain::sharing::router_contract::ShareTokenPeriod::Lifetime,
        ..Default::default()
    };
    let binding = ValueMeteringBinding {
        quota_policy_epoch: "resource-epoch".into(),
        value_metered_from_ms: now,
        effective_from_ms: now,
        pricing_revision: revision,
    };
    let other = ShareUserGrant {
        email: "resources@example.com".into(),
        active: true,
        policy: policy("api-equivalent-v2"),
        value_metering: Some(binding.clone()),
        ..Default::default()
    };
    previous
        .shares
        .iter_mut()
        .find(|share| share.id == scope.share_id)
        .unwrap()
        .user_grants
        .insert(other.email.clone(), other.clone());
    let owner = previous
        .shares
        .iter_mut()
        .find(|s| s.id == scope.share_id)
        .unwrap()
        .user_grants
        .get_mut("owner@example.com")
        .unwrap();
    owner.policy = policy("api-equivalent-v1");
    owner.value_metering = Some(binding.clone());
    let mut candidate = previous.clone();
    candidate
        .shares
        .iter_mut()
        .find(|share| share.id == scope.share_id)
        .unwrap()
        .user_grants
        .get_mut("owner@example.com")
        .unwrap()
        .policy = policy("api-equivalent-v1");
    state
        .prepare_value_bindings_with_context(
            &mut candidate,
            &previous,
            now,
            &config,
            true,
            &providers,
            Some(&usage),
            &BTreeSet::new(),
        )
        .unwrap();
    let share = candidate.get(&scope.share_id).unwrap();
    assert!(share.user_grants["owner@example.com"]
        .value_metering
        .is_some());
    assert_eq!(share.user_grants["resources@example.com"], other);
    // A v2 policy staged earlier must not activate on a text-only tariff.
    let owner = previous
        .shares
        .iter_mut()
        .find(|share| share.id == scope.share_id)
        .unwrap()
        .user_grants
        .get_mut("owner@example.com")
        .unwrap();
    owner.pending_value_quota = Some(PendingValueQuota {
        policy: policy("api-equivalent-v2"),
        effective_from_ms: now - 1,
        retain_access: true,
    });
    owner.value_metering = Some(binding);
    let scheduled = owner.clone();
    let mut candidate = previous.clone();
    state
        .prepare_value_bindings_with_context(
            &mut candidate,
            &previous,
            now,
            &config,
            true,
            &providers,
            Some(&usage),
            &BTreeSet::new(),
        )
        .unwrap();
    assert_eq!(
        candidate.get(&scope.share_id).unwrap().user_grants["owner@example.com"],
        scheduled
    );
}
