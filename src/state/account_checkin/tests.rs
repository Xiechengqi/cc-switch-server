use super::*;
use axum::{
    body::Bytes, extract::State, http::Uri, response::IntoResponse, routing::any, Json, Router,
};
use serde_json::json;
use std::collections::VecDeque;

const STATUS: &str = "/v2/billing/meter/checkin-activity-status";
const OLD_STATUS: &str = "/v2/billing/meter/checkin-status";
const SUBMIT: &str = "/v2/billing/meter/daily-checkin";

#[tokio::test]
async fn checkin_native_auth_recovery_refreshes_once_and_queries_again() {
    use crate::domain::codebuddy::{
        CODEBUDDY_ACCOUNTS_PATH, CODEBUDDY_CONFIG_PATH, CODEBUDDY_REFRESH_PATH,
    };
    for rejected_again in [false, true] {
        let second = if rejected_again {
            (401, json!({"code":401}))
        } else {
            (200, json!({"code":0,"data":{"today_checked_in":false}}))
        };
        let (url, observed, task) = mock(vec![
            (STATUS, 401, json!({"code":401})), (STATUS, second.0, second.1),
            (CODEBUDDY_REFRESH_PATH, 200, json!({"accessToken":"fixture-rotated","refreshToken":"fixture-refresh-rotated","expiresIn":3600,"domain":"www.codebuddy.cn"})),
            (CODEBUDDY_ACCOUNTS_PATH, 200, json!({"code":0,"data":{"uid":"uid-cn","nickname":"new display label"}})),
            (CODEBUDDY_CONFIG_PATH, 200, json!({"code":0,"data":{"enterpriseId":"","models":[]}})),
            (SUBMIT, 200, json!({"code":0})),
        ], false).await;
        let mut a = account(&url);
        a.refresh_token = Some("fixture-refresh".into());
        a.raw.as_mut().unwrap()["testCodeBuddyBaseUrl"] = json!(url);
        // A fresh session heartbeat avoids the independent legacy-session
        // refresh, so this fixture exercises only the recovery from HTTP 401.
        a.raw.as_mut().unwrap()["observedAtMs"] = json!(now_ms_i64());
        let state = state_with(&a).await;
        let view = state.run_account_checkin(a.id.clone(), 1).await.unwrap();
        assert_eq!(
            view.last_result.unwrap().status,
            if rejected_again {
                CheckinStatus::AuthenticationRequired
            } else {
                CheckinStatus::Success
            },
            "observed paths: {:?}",
            observed.paths.lock().unwrap()
        );
        let paths = observed.paths.lock().unwrap();
        assert_eq!(
            paths
                .iter()
                .filter(|p| p.as_str() == CODEBUDDY_REFRESH_PATH)
                .count(),
            1
        );
        assert_eq!(paths.iter().filter(|p| p.as_str() == STATUS).count(), 2);
        assert_eq!(
            paths.iter().filter(|p| p.as_str() == SUBMIT).count(),
            usize::from(!rejected_again)
        );
        task.abort();
    }
}

#[tokio::test]
async fn checkin_rate_limit_persists_retry_after_without_fallback_or_post() {
    let (url, observed, task) = mock(vec![(STATUS, 429, json!({"code":429}))], false).await;
    let a = account(&url);
    let state = state_with(&a).await;
    let before = now_ms_i64();
    let view = state.run_account_checkin(a.id.clone(), 1).await.unwrap();
    assert_eq!(
        view.last_result.unwrap().status,
        CheckinStatus::RetryableError
    );
    assert!(view.next_retry_at.unwrap() >= before + 7_200_000);
    assert_eq!(*observed.paths.lock().unwrap(), [STATUS]);
    task.abort();
}

type MockReplyQueues = BTreeMap<String, VecDeque<(u16, Value)>>;

#[derive(Clone)]
struct Mock {
    replies: Arc<Mutex<MockReplyQueues>>,
    paths: Arc<Mutex<Vec<String>>>,
    pause: Option<Arc<(Notify, Notify)>>,
}

async fn mock_handler(
    State(mock): State<Mock>,
    uri: Uri,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    assert!(matches!(
        headers.get("authorization").unwrap().to_str().unwrap(),
        "Bearer fixture-checkin" | "Bearer fixture-rotated"
    ));
    let path = uri.path().to_string();
    if [STATUS, OLD_STATUS, SUBMIT].contains(&path.as_str()) {
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), json!({}));
        assert_eq!(headers.get("origin").unwrap(), "https://www.codebuddy.cn");
    }
    mock.paths.lock().unwrap().push(path.clone());
    if path == STATUS {
        if let Some(pause) = &mock.pause {
            pause.0.notify_one();
            pause.1.notified().await;
        }
    }
    let (status, body) = mock
        .replies
        .lock()
        .unwrap()
        .get_mut(&path)
        .and_then(VecDeque::pop_front)
        .unwrap_or((500, json!({"code":500})));
    let mut response = (
        axum::http::StatusCode::from_u16(status).unwrap(),
        Json(body),
    )
        .into_response();
    if status == 429 {
        response
            .headers_mut()
            .insert("retry-after", "7200".parse().unwrap());
    }
    response
}

async fn mock(
    replies: Vec<(&str, u16, Value)>,
    paused: bool,
) -> (String, Mock, tokio::task::JoinHandle<()>) {
    let mut queues: BTreeMap<String, VecDeque<(u16, Value)>> = BTreeMap::new();
    for (path, code, body) in replies {
        queues
            .entry(path.into())
            .or_default()
            .push_back((code, body));
    }
    let mock = Mock {
        replies: Arc::new(Mutex::new(queues)),
        paths: Default::default(),
        pause: paused.then(|| Arc::new((Notify::new(), Notify::new()))),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(any(mock_handler))
        .with_state(mock.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (origin, mock, task)
}

fn account(origin: &str) -> Account {
    serde_json::from_value(json!({"id":"checkin-cn", "providerType":"workbuddy_oauth", "accessToken":"fixture-checkin", "scopes":[],
        "expiresAt": now_ms_i64() + 86_400_000,
        "profile":{"site":"cn","uid":"uid-cn","enterpriseId":"","domain":"www.codebuddy.cn","productPlatform":"workbuddy"},
        "raw":{"testWorkBuddyCheckinBaseUrl":origin,"testCodeBuddyBillingBaseUrl":"http://127.0.0.1:9"}})).unwrap()
}

fn input(a: &Account) -> UpsertAccountInput {
    serde_json::from_value(serde_json::to_value(a).unwrap()).unwrap()
}

async fn state_with(account: &Account) -> ServerState {
    let directory =
        std::env::temp_dir().join(format!("cc-switch-checkin-test-{}", rand::random::<u64>()));
    let state = ServerStateInner::load(
        Cli {
            host: "127.0.0.1".parse().unwrap(),
            port: 0,
            config_dir: Some(directory),
            web_dist_dir: None,
            log_level: "warn".into(),
            command: None,
        },
        Arc::new(crate::logging::LogCapture::new(128)),
    )
    .unwrap();
    state
        .mutate_accounts_immediate(|s| s.upsert(input(account)))
        .await
        .unwrap();
    state
}

async fn release_retry(state: &ServerState, id: &str) {
    state
        .mutate_accounts_immediate(|s| {
            s.buddy_checkins.get_mut(id).unwrap().next_retry_at = Some(0)
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn checkin_status_is_strict_and_fallback_is_404_only() {
    for data in [
        json!({}),
        json!({"today_checked_in":"false"}),
        json!({"today_checked_in":false,"todayCheckedIn":true}),
    ] {
        let (url, observed, task) =
            mock(vec![(STATUS, 200, json!({"code":0,"data":data}))], false).await;
        assert!(query_checkin_status(
            &crate::infra::http::direct_client().unwrap(),
            &account(&url)
        )
        .await
        .is_err());
        assert_eq!(*observed.paths.lock().unwrap(), [STATUS]);
        task.abort();
    }
    for status in [401, 403, 429, 500] {
        let (url, observed, task) =
            mock(vec![(STATUS, status, json!({"code":status}))], false).await;
        assert!(query_checkin_status(
            &crate::infra::http::direct_client().unwrap(),
            &account(&url)
        )
        .await
        .is_err());
        assert_eq!(*observed.paths.lock().unwrap(), [STATUS]);
        task.abort();
    }
    let (url, observed, task) = mock(
        vec![
            (STATUS, 404, json!({})),
            (
                OLD_STATUS,
                200,
                json!({"code":0,"data":{"todayCheckedIn":true}}),
            ),
        ],
        false,
    )
    .await;
    assert!(query_checkin_status(
        &crate::infra::http::direct_client().unwrap(),
        &account(&url)
    )
    .await
    .unwrap());
    assert_eq!(*observed.paths.lock().unwrap(), [STATUS, OLD_STATUS]);
    task.abort();
}

#[tokio::test]
async fn checkin_success_is_account_scoped_and_does_not_clear_inference_cooldown() {
    let (url, observed, task) = mock(
        vec![
            (
                STATUS,
                200,
                json!({"code":0,"data":{"today_checked_in":false}}),
            ),
            (SUBMIT, 200, json!({"code":0,"data":{}})),
        ],
        false,
    )
    .await;
    let mut a = account(&url);
    a.rate_limited_until = Some(now_ms_i64() + 9_000_000);
    a.last_refresh_error = Some("unrelated inference diagnostic".into());
    let state = state_with(&a).await;
    let view = state.run_account_checkin(a.id.clone(), 1).await.unwrap();
    assert!(view.today_checked_in);
    assert!(!view.enabled);
    assert!(view.quota_pending);
    assert_eq!(view.last_result.unwrap().status, CheckinStatus::Success);
    // Repeated callers (including Providers/Apps) share the confirmed receipt.
    state.run_account_checkin(a.id.clone(), 1).await.unwrap();
    assert_eq!(*observed.paths.lock().unwrap(), [STATUS, SUBMIT]);
    let stored = state.accounts_snapshot().await;
    assert_eq!(stored.accounts[0].rate_limited_until, a.rate_limited_until);
    assert_eq!(stored.accounts[0].last_refresh_error, a.last_refresh_error);
    assert_eq!(stored.buddy_checkins[&a.id].attempts, 1);
    task.abort();
}

#[tokio::test]
async fn checkin_unknown_recovers_via_status_without_duplicate_post() {
    let (url, observed, task) = mock(
        vec![
            (
                STATUS,
                200,
                json!({"code":0,"data":{"today_checked_in":false}}),
            ),
            (SUBMIT, 502, json!({"code":0})),
            (
                STATUS,
                200,
                json!({"code":0,"data":{"today_checked_in":true}}),
            ),
        ],
        false,
    )
    .await;
    let a = account(&url);
    let state = state_with(&a).await;
    let view = state.run_account_checkin(a.id.clone(), 1).await.unwrap();
    assert_eq!(view.last_result.unwrap().status, CheckinStatus::Unknown);
    assert!(!view.today_checked_in);
    assert!(state.run_account_checkin(a.id.clone(), 1).await.is_err());
    release_retry(&state, &a.id).await;
    // Automatic maintenance reconciles a manual uncertain intent even though
    // the opt-in remains off; it must not issue another daily-checkin POST.
    let recovered = state.execute_account_checkin(&a.id, 1, true).await.unwrap();
    assert!(recovered.today_checked_in);
    assert_eq!(
        recovered.last_result.unwrap().status,
        CheckinStatus::Already
    );
    assert_eq!(*observed.paths.lock().unwrap(), [STATUS, SUBMIT, STATUS]);
    task.abort();
}

#[tokio::test]
async fn checkin_concurrent_clicks_and_disconnected_callers_keep_one_owned_task() {
    let (url, observed, task) = mock(
        vec![
            (
                STATUS,
                200,
                json!({"code":0,"data":{"todayCheckedIn":false}}),
            ),
            (SUBMIT, 200, json!({"code":0})),
        ],
        true,
    )
    .await;
    let a = account(&url);
    let state = state_with(&a).await;
    let running = tokio::spawn({
        let state = state.clone();
        let id = a.id.clone();
        async move { state.run_account_checkin(id, 1).await }
    });
    observed.pause.as_ref().unwrap().0.notified().await;
    assert!(state
        .run_account_checkin(a.id.clone(), 1)
        .await
        .unwrap_err()
        .is::<CheckinControlError>());
    running.abort();
    observed.pause.as_ref().unwrap().1.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if state
                .account_checkin_view(&a.id, 1)
                .await
                .unwrap()
                .today_checked_in
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*observed.paths.lock().unwrap(), [STATUS, SUBMIT]);
    task.abort();
}

#[tokio::test]
async fn checkin_recreation_or_disabled_policy_prevents_stale_post() {
    for recreate in [true, false] {
        let (url, observed, task) = mock(
            vec![(
                STATUS,
                200,
                json!({"code":0,"data":{"todayCheckedIn":false}}),
            )],
            true,
        )
        .await;
        let a = account(&url);
        let state = state_with(&a).await;
        state
            .set_account_checkin_policy(&a.id, 1, 0, true, "00:00")
            .await
            .unwrap();
        let run = tokio::spawn({
            let state = state.clone();
            let id = a.id.clone();
            async move { state.execute_account_checkin(&id, 1, true).await }
        });
        observed.pause.as_ref().unwrap().0.notified().await;
        if recreate {
            state
                .mutate_accounts_immediate(|s| {
                    s.delete(&a.id);
                    s.upsert(input(&a));
                })
                .await
                .unwrap();
        } else {
            state
                .set_account_checkin_policy(&a.id, 1, 1, false, "00:00")
                .await
                .unwrap();
        }
        observed.pause.as_ref().unwrap().1.notify_one();
        let result = run.await.unwrap();
        if recreate {
            assert!(result.is_err());
        } else {
            assert!(!result.unwrap().enabled);
        }
        assert_eq!(*observed.paths.lock().unwrap(), [STATUS]);
        task.abort();
    }
}

#[tokio::test]
async fn checkin_policies_validate_generation_revision_and_scope() {
    let a = account("http://127.0.0.1:9");
    let state = state_with(&a).await;
    assert!(!state.account_checkin_view(&a.id, 1).await.unwrap().enabled);
    assert!(state
        .set_account_checkin_policy(&a.id, 2, 0, true, "09:00")
        .await
        .is_err());
    assert!(state
        .set_account_checkin_policy(&a.id, 1, 0, true, "24:00")
        .await
        .is_err());
    let saved = state
        .set_account_checkin_policy(&a.id, 1, 0, true, "10:30")
        .await
        .unwrap();
    assert!(saved.enabled);
    assert_eq!(saved.revision, 1);
    assert!(state
        .set_account_checkin_policy(&a.id, 1, 0, false, "09:00")
        .await
        .is_err());
    state
        .mutate_accounts_immediate(|s| {
            let mut a = a.clone();
            a.access_token = Some("rotated-fixture".into());
            s.upsert(input(&a));
        })
        .await
        .unwrap();
    assert!(state.account_checkin_view(&a.id, 1).await.unwrap().enabled);
    for product in [ProviderType::WorkBuddyOAuth, ProviderType::CodeBuddyOAuth] {
        let mut unsupported = a.clone();
        unsupported.id = format!("unsupported-{product:?}");
        unsupported.provider_type = product;
        let p = unsupported.profile.as_mut().unwrap();
        p["site"] = json!("intl");
        p["domain"] = json!("www.workbuddy.ai");
        p["productPlatform"] = json!("workbuddy-ai");
        state
            .mutate_accounts_immediate(|s| s.upsert(input(&unsupported)))
            .await
            .unwrap();
        assert!(state
            .set_account_checkin_policy(&unsupported.id, 1, 0, true, "09:00")
            .await
            .is_err());
        assert!(state
            .run_account_checkin(unsupported.id.clone(), 1)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn checkin_http_400_already_and_inactive_are_distinct() {
    for (message, expected) in [
        ("今天已签到，请明天再来", CheckinStatus::Already),
        ("签到活动已过期", CheckinStatus::Inactive),
    ] {
        let (url, _, task) = mock(
            vec![
                (
                    STATUS,
                    200,
                    json!({"code":0,"data":{"todayCheckedIn":false}}),
                ),
                (SUBMIT, 400, json!({"code":400,"message":message})),
            ],
            false,
        )
        .await;
        let a = account(&url);
        let state = state_with(&a).await;
        let view = state.run_account_checkin(a.id.clone(), 1).await.unwrap();
        assert_eq!(view.last_result.unwrap().status, expected);
        assert_eq!(view.today_checked_in, expected.confirmed());
        assert_eq!(view.next_retry_at, None);
        task.abort();
    }
}

#[tokio::test]
async fn checkin_records_survive_json_and_sqlite_with_pending_recovery() {
    let a = account("http://127.0.0.1:9");
    let state = state_with(&a).await;
    state
        .set_account_checkin_policy(&a.id, 1, 0, true, "09:15")
        .await
        .unwrap();
    state
        .mutate_accounts_immediate(|s| s.buddy_checkins.get_mut(&a.id).unwrap().begin(now_ms_i64()))
        .await
        .unwrap();
    let dir = &state.config_dir;
    let json_store = AccountStore::load_or_default(dir).unwrap();
    assert!(json_store.buddy_checkins[&a.id].enabled);
    assert!(json_store.buddy_checkins[&a.id].pending);
    assert_eq!(json_store.buddy_checkins[&a.id].time, "09:15");
    let providers = ProviderStore::default();
    let shares = ShareStore::default();
    let usage = UsageStore::default();
    crate::repository::server_sqlite::refresh_shadow(
        dir,
        crate::repository::server_sqlite::ShadowImportInput {
            providers: &providers,
            accounts: &json_store,
            shares: &shares,
            usage: &usage,
        },
    )
    .unwrap();
    crate::repository::server_sqlite::activate_authority(dir).unwrap();
    let mut sqlite = crate::repository::server_sqlite::load_authoritative(dir)
        .unwrap()
        .accounts;
    assert!(sqlite.buddy_checkins[&a.id].pending);
    sqlite.buddy_checkins.get_mut(&a.id).unwrap().enabled = false;
    crate::repository::server_sqlite::persist_accounts(dir, &sqlite).unwrap();
    let sqlite = crate::repository::server_sqlite::load_authoritative(dir)
        .unwrap()
        .accounts;
    assert!(!sqlite.buddy_checkins[&a.id].enabled);
    assert!(sqlite.buddy_checkins[&a.id].pending);
}

#[tokio::test]
async fn checkin_resource_refresh_preserves_official_usage_and_cooldown() {
    use crate::domain::codebuddy::{
        CODEBUDDY_RESOURCE_FREE_PACKAGES_PATH, CODEBUDDY_RESOURCE_PAID_PACKAGES_PATH,
        CODEBUDDY_RESOURCE_PATH, CODEBUDDY_RESOURCE_SUMMARY_PATH,
    };
    let (url, observed, task) = mock(vec![
        (STATUS, 200, json!({"code":0,"data":{"today_checked_in":true}})),
        (CODEBUDDY_RESOURCE_SUMMARY_PATH, 404, json!({})),
        (CODEBUDDY_RESOURCE_PAID_PACKAGES_PATH, 404, json!({})),
        (CODEBUDDY_RESOURCE_FREE_PACKAGES_PATH, 404, json!({})),
        (CODEBUDDY_RESOURCE_PATH, 200, json!({"code":0,"data":{"Response":{"Data":{"TotalCount":1,"TotalDosage":3,
            "Accounts":[{"PackageName":"Actual reward","CapacitySize":100,"CapacityRemain":97,"CapacityUnit":"credits"}]}}}})),
    ], false).await;
    let mut a = account(&url);
    a.raw.as_mut().unwrap()["testCodeBuddyBillingBaseUrl"] = json!(url);
    a.rate_limited_until = Some(now_ms_i64() + 1_000_000);
    a.last_refresh_error = Some("inference failure".into());
    a.quota_next_refresh_at = Some(now_ms_i64() + 3_600_000);
    let usage = json!({"status":"complete","collectedAt":17,"today":4});
    a.quota = Some(crate::domain::accounts::store::AccountQuota {
        extra_usage: Some(json!({"codeBuddyOfficialUsage":usage})),
        ..Default::default()
    });
    let state = state_with(&a).await;
    let view = state.run_account_checkin(a.id.clone(), 1).await.unwrap();
    assert!(view.today_checked_in);
    assert!(!view.quota_pending);
    let current = &state.accounts_snapshot().await.accounts[0];
    assert_eq!(
        current
            .quota
            .as_ref()
            .unwrap()
            .extra_usage
            .as_ref()
            .unwrap()["codeBuddyOfficialUsage"],
        usage
    );
    assert_eq!(current.quota.as_ref().unwrap().tiers[0].limit, Some(100.0));
    assert_eq!(current.rate_limited_until, a.rate_limited_until);
    assert_eq!(current.last_refresh_error, a.last_refresh_error);
    assert_eq!(current.quota_next_refresh_at, a.quota_next_refresh_at);
    assert_eq!(observed.paths.lock().unwrap().len(), 5);
    task.abort();
}

#[tokio::test]
async fn checkin_never_dispatches_when_intent_cannot_be_persisted() {
    let (url, observed, task) = mock(vec![], false).await;
    let a = account(&url);
    let state = state_with(&a).await;
    let accounts_path = crate::domain::accounts::store::accounts_path(&state.config_dir);
    std::fs::remove_file(&accounts_path).unwrap();
    std::fs::create_dir(&accounts_path).unwrap();
    assert!(state.run_account_checkin(a.id, 1).await.is_err());
    assert!(observed.paths.lock().unwrap().is_empty());
    task.abort();
}

#[tokio::test]
async fn checkin_regular_quota_refresh_resolves_pending_resource_marker() {
    let a = account("http://127.0.0.1:9");
    let state = state_with(&a).await;
    state
        .mutate_accounts_immediate(|store| {
            let mut record = AccountCheckin {
                principal: checkin::principal(&a).unwrap(),
                ..Default::default()
            };
            record.begin(now_ms_i64());
            record.finish(
                CheckinOutcome {
                    status: CheckinStatus::Already,
                    at_ms: now_ms_i64(),
                    message: "already_checked_in".into(),
                },
                None,
            );
            store.buddy_checkins.insert(a.id.clone(), record);
        })
        .await
        .unwrap();
    let current = state.accounts_snapshot().await.accounts[0].clone();
    state
        .commit_account_quota_refresh_update(
            &current,
            AccountRefreshUpdate {
                quota: Some(Default::default()),
                quota_refreshed_at: Some(now_ms_i64()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        !state
            .account_checkin_view(&a.id, 1)
            .await
            .unwrap()
            .quota_pending
    );
}

#[tokio::test]
async fn checkin_relogin_resumes_schedule_without_resetting_daily_budget() {
    let a = account("http://127.0.0.1:9");
    let state = state_with(&a).await;
    state
        .set_account_checkin_policy(&a.id, 1, 0, true, "00:00")
        .await
        .unwrap();
    state
        .mutate_accounts_immediate(|store| {
            let record = store.buddy_checkins.get_mut(&a.id).unwrap();
            record.begin(now_ms_i64());
            record.finish(
                CheckinOutcome {
                    status: CheckinStatus::AuthenticationRequired,
                    at_ms: now_ms_i64(),
                    message: "authentication_required".into(),
                },
                None,
            );
            assert!(!record.due(&a, now_ms_i64()));
            let mut rotated = a.clone();
            rotated.access_token = Some("fixture-reauthorized".into());
            store.upsert(input(&rotated));
        })
        .await
        .unwrap();
    let store = state.accounts_snapshot().await;
    let record = &store.buddy_checkins[&a.id];
    assert!(record.due(&store.accounts[0], now_ms_i64()));
    assert_eq!(record.attempts, 1);
}
