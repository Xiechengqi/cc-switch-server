//! Execution evidence only. This adds no balance or second quota ledger.
use super::*;
use crate::domain::usage::value_pricing::ResourcePlan;
use rusqlite::OptionalExtension;

#[derive(Clone)]
pub(crate) struct SettingsReceipt {
    pub router_url: String,
    pub installation_id: String,
    pub share_id: String,
    pub receipt: crate::domain::sharing::router_contract::ShareSettingsReceipt,
}
pub(super) fn persist_settings_receipt(
    tx: &Transaction<'_>,
    saved: &SettingsReceipt,
) -> anyhow::Result<()> {
    let json = serde_json::to_string(&saved.receipt)?;
    let old:Option<(String,String)>=tx.query_row("SELECT share_id,receipt_json FROM share_settings_receipts WHERE router_url=?1 AND installation_id=?2 AND operation_id=?3",params![saved.router_url,saved.installation_id,saved.receipt.operation_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some(old) = old {
        ensure!(
            old == (saved.share_id.clone(), json),
            "CAPABILITY_STATE_CHANGED"
        );
    } else {
        tx.execute(
            "INSERT INTO share_settings_receipts VALUES(?1,?2,?3,?4,?5)",
            params![
                saved.router_url,
                saved.installation_id,
                saved.receipt.operation_id,
                saved.share_id,
                json
            ],
        )?;
    }
    Ok(())
}

pub(crate) fn scope(
    config: &crate::domain::settings::config::ServerConfig,
    context: &crate::domain::usage::store::UsageLogContext,
) -> anyhow::Result<String> {
    let installation = config
        .registered_router_identity()
        .context("CAPABILITY_UPGRADE_REQUIRED")?;
    Ok(serde_json::to_string(&(
        config.router_api_base(),
        &installation.installation_id,
        context
            .share_id
            .as_deref()
            .context("CAPABILITY_STATE_CHANGED")?,
        context
            .user_email
            .as_deref()
            .unwrap_or("$owner")
            .to_ascii_lowercase(),
        context.quota_app,
    ))?)
}
pub(crate) fn admit(
    dir: &Path,
    scope: &str,
    request_id: &str,
    attempt: u32,
    plan: &ResourcePlan,
    now: i64,
) -> anyhow::Result<()> {
    ensure!(is_committed(dir)?, "CAPABILITY_UPGRADE_REQUIRED");
    let binding = plan
        .binding_fingerprint
        .as_deref()
        .context("CAPABILITY_STATE_CHANGED")?;
    let mut conn = open_database(&database_path(dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    ensure!(
        !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM share_capability_incidents WHERE scope=?1 AND binding=?2)",
            params![scope, binding],
            |r| r.get::<_, bool>(0)
        )?,
        "CAPABILITY_EXECUTION_QUARANTINED"
    );
    let json = serde_json::to_string(plan)?;
    let old: Option<(String, String, String)> = tx
        .query_row(
            "SELECT scope,binding,plan_json FROM share_capability_admissions WHERE request_id=?1 AND attempt=?2",
            params![request_id, attempt],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some(old) = old {
        ensure!(
            old == (scope.into(), binding.into(), json),
            "CAPABILITY_STATE_CHANGED"
        );
    } else {
        tx.execute(
            "INSERT INTO share_capability_admissions VALUES(?1,?2,?3,?4,?5,?6)",
            params![request_id, attempt, scope, binding, json, now],
        )?;
    }
    tx.execute(
        "DELETE FROM share_capability_contexts WHERE rowid IN (SELECT rowid FROM share_capability_contexts WHERE expires_at_ms<?1 LIMIT 100)",
        [now],
    )?;
    tx.execute(
        "DELETE FROM share_capability_admissions WHERE rowid IN (SELECT rowid FROM share_capability_admissions WHERE admitted_at_ms<?1 LIMIT 100)",
        [now.saturating_sub(30 * 86400000)],
    )?;
    tx.commit()?;
    Ok(())
}
pub(crate) fn previous(
    dir: &Path,
    scope: &str,
    binding: &str,
    response_id: &str,
    now: i64,
) -> anyhow::Result<ResourcePlan> {
    ensure!(response_id.len() <= 256, "CAPABILITY_CONTEXT_REQUIRED");
    let conn = open_database(&database_path(dir), true)?;
    let raw:Option<String>=conn.query_row("SELECT plan_json FROM share_capability_contexts WHERE scope=?1 AND binding=?2 AND response_id=?3 AND expires_at_ms>?4",params![scope,binding,response_id,now],|r|r.get(0)).optional()?;
    serde_json::from_str(&raw.context("CAPABILITY_CONTEXT_REQUIRED")?)
        .context("CAPABILITY_CONTEXT_REQUIRED")
}
pub(super) fn observe(tx: &Transaction<'_>, log: &UsageLog) -> anyhow::Result<()> {
    let Some(usage) = &log.value_resources else {
        return Ok(());
    };
    let row:Option<(String,String,String)>=tx.query_row("SELECT scope,binding,plan_json FROM share_capability_admissions WHERE request_id=?1 AND attempt=?2",params![log.request_id,log.attempt_count.max(1)],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((scope, binding, raw)) = row else {
        return Ok(());
    };
    // A revised observation may add evidence, but cannot change the Share or
    // principal admitted for this request/attempt.
    let admitted_scope: Value = serde_json::from_str(&scope)?;
    ensure!(
        admitted_scope.get(2).and_then(Value::as_str) == log.share_id.as_deref()
            && admitted_scope.get(3).and_then(Value::as_str)
                == Some(
                    log.user_email
                        .as_deref()
                        .unwrap_or("$owner")
                        .to_ascii_lowercase()
                        .as_str()
                ),
        "CAPABILITY_STATE_CHANGED"
    );
    let plan: ResourcePlan = serde_json::from_str(&raw)?;
    if let Some(id) = &usage.response_id {
        ensure!(
            !id.is_empty() && id.len() <= 256,
            "CAPABILITY_REQUEST_INVALID"
        );
        // Duplicate/revised observations merge capabilities; they can never erase them.
        let existing:Option<String>=tx.query_row("SELECT plan_json FROM share_capability_contexts WHERE scope=?1 AND binding=?2 AND response_id=?3",params![scope,binding,id],|r|r.get(0)).optional()?;
        let mut combined = plan.clone();
        if let Some(old) = existing {
            let old: ResourcePlan = serde_json::from_str(&old)?;
            combined.operations.extend(old.operations);
            combined.operations.sort();
            combined.operations.dedup();
            for (k, v) in old.quantities {
                let n = combined.quantities.entry(k).or_default();
                *n = (*n).max(v);
            }
        }
        tx.execute("INSERT INTO share_capability_contexts VALUES(?1,?2,?3,?4,?5) ON CONFLICT(scope,binding,response_id) DO UPDATE SET plan_json=excluded.plan_json,expires_at_ms=MAX(expires_at_ms,excluded.expires_at_ms)",params![scope,binding,id,serde_json::to_string(&combined)?,(log.created_at_ms.min(i64::MAX as u128) as i64).saturating_add(86400000)])?;
    }
    for (resource, count) in &usage.quantities {
        if *count == 0 {
            continue;
        }
        let base = resource.split('/').next().unwrap_or(resource);
        use crate::domain::sharing::capabilities::ShareOperation;
        let op = ShareOperation::ALL
            .into_iter()
            .find(|op| op.resource() == base)
            .or(match base {
                "image.output_token" => Some(ShareOperation::ImageGenerate),
                "audio.output_millisecond" => Some(ShareOperation::AudioGenerate),
                _ => None,
            });
        if op.is_some_and(|op| {
            !plan
                .operations
                .iter()
                .any(|allowed| allowed.resource() == op.resource())
        }) || op.is_some()
            && !plan
                .quantities
                .keys()
                .any(|key| key.split('/').next() == Some(base))
        {
            tx.execute(
                "INSERT OR IGNORE INTO share_capability_incidents VALUES(?1,?2,?3,?4,?5)",
                params![
                    scope,
                    binding,
                    log.request_id,
                    resource,
                    log.created_at_ms.min(i64::MAX as u128) as i64
                ],
            )?;
        }
    }
    Ok(())
}

/// Manager-only projection of authoritative settled attempts. Current catalog
/// re-estimates are deliberately not substituted for the frozen ledger amount.
pub(crate) fn incidents(
    dir: &Path,
    router: &str,
    installation: &str,
    share: &str,
) -> anyhow::Result<Vec<Value>> {
    if !is_committed(dir)? {
        return Ok(vec![]);
    }
    let conn = open_database(&database_path(dir), true)?;
    let mut statement = conn.prepare("SELECT scope,binding,request_id,resource,detected_at_ms FROM share_capability_incidents WHERE json_extract(scope,'$[0]')=?1 AND json_extract(scope,'$[1]')=?2 AND json_extract(scope,'$[2]')=?3 ORDER BY detected_at_ms DESC LIMIT 50")?;
    let rows = statement.query_map(params![router, installation, share], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    rows.map(|r| { let (scope,binding,request,resource,at)=r?; let scope:Value=serde_json::from_str(&scope)?; Ok(serde_json::json!({"scope":scope[3],"bindingFingerprint":binding,"requestId":request,"resource":resource,"detectedAtMs":at})) }).collect()
}

pub(crate) fn recent(
    dir: &Path,
    router: &str,
    installation: &str,
    share: &str,
) -> anyhow::Result<Vec<Value>> {
    metering_attempts(dir, router, installation, share, None)
}

pub(crate) fn request_metering(
    dir: &Path,
    router: &str,
    installation: &str,
    share: &str,
    request: &str,
) -> anyhow::Result<Vec<Value>> {
    ensure!(
        !request.is_empty()
            && request.len() <= 192
            && request.trim() == request
            && !request.chars().any(char::is_control),
        "invalid request identity"
    );
    metering_attempts(dir, router, installation, share, Some(request))
}

fn metering_attempts(
    dir: &Path,
    router: &str,
    installation: &str,
    share: &str,
    request: Option<&str>,
) -> anyhow::Result<Vec<Value>> {
    if !is_committed(dir)? {
        return Ok(vec![]);
    }
    let conn = open_database(&database_path(dir), true)?;
    let mut query = conn.prepare(
        "WITH attempts AS (
            SELECT request_id, attempt, scope, plan_json, admitted_at_ms
            FROM share_capability_admissions
            WHERE json_extract(scope,'$[0]')=?1 AND json_extract(scope,'$[1]')=?2
              AND json_extract(scope,'$[2]')=?3 AND (?4 IS NULL OR request_id=?4)
            UNION ALL
            SELECT a.request_id, CAST(a.attempt_id AS INTEGER),
              json_array(?1,?2,?3,json_extract(a.scope,'$.email'),NULL), '{}', a.started_at_ms
            FROM value_quota_attempts a
            WHERE a.request_id=?4 AND json_extract(a.scope,'$.routerUrl')=?1
              AND json_extract(a.scope,'$.installationId')=?2 AND json_extract(a.scope,'$.shareId')=?3
              AND NOT EXISTS(SELECT 1 FROM share_capability_admissions c
                WHERE c.request_id=a.request_id AND CAST(c.attempt AS TEXT)=a.attempt_id)
        )
        SELECT c.request_id,c.attempt,c.scope,c.plan_json,c.admitted_at_ms,
          a.usage_picos,a.pending_risk_picos,a.metering_status,a.pricing_revision,a.unpriced_reason,a.frozen_tariff_json,
          (SELECT r.observation_json FROM value_quota_revisions r
            WHERE r.request_id=a.request_id AND r.attempt_id=a.attempt_id
            ORDER BY usage_revision DESC LIMIT 1),
          u.payload_json,a.quota_enforced,a.policy_epoch,a.effective_model
        FROM attempts c
        LEFT JOIN value_quota_attempts a
          ON a.request_id=c.request_id AND a.attempt_id=CAST(c.attempt AS TEXT)
          AND json_extract(a.scope,'$.routerUrl')=?1 AND json_extract(a.scope,'$.installationId')=?2
          AND json_extract(a.scope,'$.shareId')=?3 AND json_extract(a.scope,'$.email')=json_extract(c.scope,'$[3]')
        LEFT JOIN usage_records u ON u.request_id=c.request_id
        ORDER BY CASE WHEN ?4 IS NOT NULL THEN -c.attempt ELSE c.admitted_at_ms END DESC LIMIT ?5"
    )?;
    let rows = query.query_map(
        params![
            router,
            installation,
            share,
            request,
            if request.is_some() { 100 } else { 20 }
        ],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, Option<String>>(11)?,
                r.get::<_, Option<String>>(12)?,
                r.get::<_, Option<bool>>(13)?,
                r.get::<_, Option<String>>(14)?,
                r.get::<_, Option<String>>(15)?,
            ))
        },
    )?;
    rows.map(|row| {
        let (request,attempt,scope,plan,at,amount,risk,status,revision,unpriced,tariff,observation,usage_log,enforced,epoch,model)=row?;
        let scope:Value=serde_json::from_str(&scope)?;
        let observation:Option<Value>=observation.as_deref().map(serde_json::from_str).transpose()?;
        let tariff:Option<Value>=tariff.as_deref().map(serde_json::from_str).transpose()?;
        // Token/owner/unlimited calls still have native resource facts, but no
        // USD attempt. Read their own matching attempt; never reprice current
        // catalog rates or attach a later retry's usage to an earlier admission.
        let usage_log:Option<Value>=usage_log.as_deref().map(serde_json::from_str).transpose()?;
        let logged_resources=usage_log.as_ref().filter(|log| status.is_none()
            && log["attemptCount"].as_u64().unwrap_or(1).max(1)==u64::from(attempt)
            && log["shareId"]==scope[2]
            && log["app"]==scope[4]
            && log["userEmail"].as_str().unwrap_or("$owner").to_ascii_lowercase()==scope[3].as_str().unwrap_or_default())
            .and_then(|log|log.get("valueResources"));
        let resources=observation.as_ref().and_then(|o|o.get("resources")).or(logged_resources);
        let quantities=resources.and_then(|resources|resources.get("quantities")).cloned();
        let mut dimensions=Vec::new();
        if let Some(quantities)=quantities.as_ref().and_then(Value::as_object) {
            for (dimension,quantity) in quantities {
                let rate=tariff.as_ref().and_then(|t|t["resources"].get(dimension).or_else(||t["resources"].get(dimension.split('/').next().unwrap_or(dimension)))).and_then(Value::as_str);
                let cost=rate.and_then(|rate|crate::domain::usage::value_pricing::unsigned_decimal(rate).ok()).and_then(|rate|quantity.as_u64().and_then(|n|rate.checked_mul(n.into()))).map(|n|n.to_string());
                dimensions.push(serde_json::json!({"dimension":dimension,"quantity":quantity,"ratePicos":rate,"amountPicos":cost,"pricing":if tariff.is_none(){"not_in_usd_ledger"}else if rate==Some("0"){"explicit_free"}else if rate.is_some(){"priced"}else{"missing"}}));
            }
        }
        let mode = match (enforced, epoch.as_deref(), status.as_deref()) {
            (Some(true), _, _) => "debited",
            (Some(false), _, _) | (None, Some("evidence"), _) => "estimate",
            (_, _, None) => "none",
            _ => "unknown",
        };
        let plan: Value = serde_json::from_str(&plan)?;
        let operations = plan["operations"].as_array().cloned().unwrap_or_default();
        Ok(serde_json::json!({"requestId":request,"attempt":attempt,"scope":scope[3],
            "chargeMode":mode,"model":model,"operations":operations,"admittedAtMs":at,"settledPicos":amount,"pendingRiskPicos":risk,"meteringStatus":status.unwrap_or_else(||"not_in_usd_ledger".into()),"pricingRevision":revision,"unpricedReason":unpriced,"dimensions":dimensions,"estimated":resources.and_then(|resources|resources.get("estimated")).cloned().unwrap_or(Value::Null)}))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::sharing::capabilities::ShareOperation;
    use crate::domain::usage::{
        store::{TokenUsage, UsageModelMetadata},
        value_pricing::{ResourceUsage, RESOURCE_ESTIMATE_VERSION},
    };
    #[test]
    fn capability_context_remembers_omitted_controls_and_quarantines_unplanned_outputs_idempotently(
    ) {
        let mut conn = Connection::open_in_memory().unwrap();
        super::super::schema::initialize_schema(&conn).unwrap();
        let plan = ResourcePlan {
            operations: vec![ShareOperation::WebSearch],
            binding_fingerprint: Some("b".repeat(64)),
            policy_revision: Some(2),
            quantities: BTreeMap::from([("tool.web_search".into(), 8)]),
            ..Default::default()
        };
        conn.execute(
            "INSERT INTO share_capability_admissions VALUES('request',1,'[\"router\",\"installation\",\"share\",\"user@example.test\",\"codex\"]','binding',?1,100)",
            [serde_json::to_string(&plan).unwrap()],
        )
        .unwrap();
        let mut log = UsageLog::new(
            crate::domain::providers::model::AppKind::Codex,
            "provider".into(),
            "Provider".into(),
            crate::domain::providers::model::ProviderType::Codex,
            200,
            1,
            UsageModelMetadata::default(),
            TokenUsage::default(),
        );
        log.request_id = "request".into();
        log.share_id = Some("share".into());
        log.user_email = Some("user@example.test".into());
        log.created_at_ms = 100;
        log.value_resources = Some(ResourceUsage {
            response_id: Some("response-1".into()),
            quantities: BTreeMap::from([("tool.web_search".into(), 2)]),
            source: "upstream.protocol".into(),
            estimated: false,
            version: RESOURCE_ESTIMATE_VERSION.into(),
        });
        let tx = conn.transaction().unwrap();
        observe(&tx, &log).unwrap();
        observe(&tx, &log).unwrap();
        let mut foreign = log.clone();
        foreign.user_email = Some("other@example.test".into());
        assert!(observe(&tx, &foreign).is_err());
        foreign = log.clone();
        foreign.share_id = Some("other-share".into());
        assert!(observe(&tx, &foreign).is_err());
        let saved: String = tx
            .query_row(
                "SELECT plan_json FROM share_capability_contexts WHERE response_id='response-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<ResourcePlan>(&saved)
                .unwrap()
                .operations,
            vec![ShareOperation::WebSearch]
        );
        assert_eq!(
            tx.query_row("SELECT count(*) FROM share_capability_incidents", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            0
        );
        log.value_resources
            .as_mut()
            .unwrap()
            .quantities
            .insert("image.output".into(), 1);
        observe(&tx, &log).unwrap();
        observe(&tx, &log).unwrap();
        assert_eq!(
            tx.query_row("SELECT count(*) FROM share_capability_incidents", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            tx.query_row("SELECT count(*) FROM value_quota_attempts", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            0
        );
        // Alternate native units still represent generated output, never an
        // understanding input that can avoid execution quarantine.
        log.value_resources
            .as_mut()
            .unwrap()
            .quantities
            .insert("audio.output_millisecond".into(), 1000);
        observe(&tx, &log).unwrap();
        assert_eq!(
            tx.query_row("SELECT count(*) FROM share_capability_incidents", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            2
        );
        tx.commit().unwrap();
    }
}
