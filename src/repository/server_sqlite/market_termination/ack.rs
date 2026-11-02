//! Durable transport outbox, separate from immutable final usage receipts.
//! A successful HTTP ACK alone cannot atomically clear the local Share marker.
use super::*;

pub(crate) struct QueuedAck {
    pub operation_id: String,
    pub termination_id: String,
    pub payload: Value,
}

fn enqueue_in(
    c: &Connection,
    router: &str,
    installation: &str,
    share: &str,
    payload: &Value,
    now: i64,
) -> anyhow::Result<()> {
    let receipt: TerminationReceipt =
        serde_json::from_value(payload["terminationReceipt"].clone())?;
    ensure!(
        payload["status"] == "applied"
            && payload["editId"].as_str().is_some_and(|id| !id.is_empty())
            && payload["revision"].as_i64().is_some_and(|r| r > 0)
            && receipt.share_id == share
            && !router.is_empty()
            && !installation.is_empty(),
        "invalid prepared Market ACK"
    );
    let original: String = c.query_row(
        "SELECT payload_json FROM market_settlement_receipts WHERE operation_id=?1",
        [&receipt.operation_id],
        |r| r.get(0),
    )?;
    ensure!(
        serde_json::from_str::<TerminationReceipt>(&original)? == receipt,
        "prepared Market ACK does not match final usage receipt"
    );
    c.execute(
        "INSERT OR IGNORE INTO market_settlement_ack_outbox VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",
        params![
            receipt.operation_id,
            router.trim_end_matches('/'),
            installation,
            share,
            receipt.termination_id,
            serde_json::to_string(payload)?,
            now
        ],
    )?;
    let matches: bool = c.query_row(
        "SELECT router_api_base=?2 AND installation_id=?3 AND share_id=?4
            AND termination_id=?5 AND payload_json=?6
        FROM market_settlement_ack_outbox WHERE operation_id=?1",
        params![
            receipt.operation_id,
            router.trim_end_matches('/'),
            installation,
            share,
            receipt.termination_id,
            serde_json::to_string(payload)?
        ],
        |r| r.get(0),
    )?;
    ensure!(matches, "prepared Market ACK identity conflict");
    Ok(())
}

pub(crate) fn enqueue_ack(
    dir: &Path,
    router: &str,
    installation: &str,
    share: &str,
    payload: &Value,
) -> anyhow::Result<()> {
    super::super::assert_committed(dir)?;
    let mut c = super::super::open_database(&super::super::database_path(dir), false)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    enqueue_in(
        &tx,
        router,
        installation,
        share,
        payload,
        crate::infra::time::now_ms() as i64,
    )?;
    tx.commit()?;
    Ok(())
}

fn pending_in(c: &Connection, router: &str, installation: &str) -> anyhow::Result<Vec<QueuedAck>> {
    let mut q = c.prepare(
        "SELECT operation_id,termination_id,payload_json FROM market_settlement_ack_outbox
        WHERE router_api_base=?1 AND installation_id=?2
        ORDER BY last_attempt_at_ms,operation_id LIMIT 25",
    )?;
    let rows = q.query_map(params![router.trim_end_matches('/'), installation], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    rows.map(|row| {
        let (operation_id, termination_id, raw) = row?;
        Ok(QueuedAck {
            operation_id,
            termination_id,
            payload: serde_json::from_str(&raw)?,
        })
    })
    .collect()
}

pub(crate) fn pending_acks(
    dir: &Path,
    router: &str,
    installation: &str,
) -> anyhow::Result<Vec<QueuedAck>> {
    if !super::super::is_committed(dir)? {
        return Ok(Vec::new());
    }
    let c = super::super::open_database(&super::super::database_path(dir), false)?;
    pending_in(&c, router, installation)
}

pub(crate) fn touch_ack(dir: &Path, operation: &str) -> anyhow::Result<()> {
    super::super::assert_committed(dir)?;
    let c = super::super::open_database(&super::super::database_path(dir), false)?;
    c.execute(
        "UPDATE market_settlement_ack_outbox SET last_attempt_at_ms=?2 WHERE operation_id=?1",
        params![operation, crate::infra::time::now_ms() as i64],
    )?;
    Ok(())
}

pub(crate) fn delete_ack(dir: &Path, operation: &str) -> anyhow::Result<()> {
    super::super::assert_committed(dir)?;
    let c = super::super::open_database(&super::super::database_path(dir), false)?;
    c.execute(
        "DELETE FROM market_settlement_ack_outbox WHERE operation_id=?1",
        [operation],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn successful_ack_survives_lost_response_and_is_bound_to_its_original_router() {
        let (mut c, _, op) = super::super::tests::fixture();
        let tx = c.transaction().unwrap();
        let receipt = super::super::receipt_in(&tx, "share", &op, 700).unwrap();
        let payload = json!({"editId":"edit", "revision":1, "status":"applied", "terminationReceipt":receipt});
        enqueue_in(
            &tx,
            "https://router.example/",
            "installation",
            "share",
            &payload,
            1,
        )
        .unwrap();
        tx.commit().unwrap();
        // Even when the Router stops listing the edit, the Server retains
        // exactly the successful payload needed for an idempotent retry.
        let rows = pending_in(&c, "https://router.example", "installation").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].payload, payload);
        assert!(pending_in(&c, "https://other.example", "installation")
            .unwrap()
            .is_empty());
        assert!(pending_in(&c, "https://router.example", "other")
            .unwrap()
            .is_empty());
        enqueue_in(
            &c,
            "https://router.example",
            "installation",
            "share",
            &payload,
            2,
        )
        .unwrap();
        let mut changed = payload.clone();
        changed["editId"] = json!("different");
        assert!(enqueue_in(
            &c,
            "https://router.example",
            "installation",
            "share",
            &changed,
            3
        )
        .is_err());
        assert!(c
            .execute(
                "UPDATE market_settlement_ack_outbox SET payload_json='{}'",
                []
            )
            .is_err());
        c.execute(
            "UPDATE market_settlement_ack_outbox SET last_attempt_at_ms=4",
            [],
        )
        .unwrap();
        c.execute(
            "DELETE FROM market_settlement_ack_outbox WHERE operation_id='operation'",
            [],
        )
        .unwrap();
        assert!(pending_in(&c, "https://router.example", "installation")
            .unwrap()
            .is_empty());
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM market_settlement_receipts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
