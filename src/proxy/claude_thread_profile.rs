//! Recoverable protocol metadata only. No bearer, prompt, message body or signature.
use super::{
    request_memory::{RequestMemoryBudget, RequestMemoryComponent, RequestMemoryReservation},
    ProxyError,
};
use crate::infra::resident_memory::{
    process_resident_memory, serialized_json_bytes, ResidentMemoryReservation,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

const TTL_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_ENTRY: usize = 64 * 1024;
const MAX_BYTES: i64 = 8 * 1024 * 1024;
const MAX_ENTRIES: i64 = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClaudeThreadProfile {
    pub betas: String,
    pub policy: String,
    pub aliases: BTreeMap<String, String>,
}

pub(crate) struct LoadedProfile {
    pub profile: ClaudeThreadProfile,
    _memory: Option<RequestMemoryReservation>,
    _resident: Option<ResidentMemoryReservation>,
}

pub(crate) fn unavailable() -> ProxyError {
    ProxyError {
        status: axum::http::StatusCode::CONFLICT,
        message: "Claude Thread protocol state is missing, expired, conflicting or unavailable; replay the full conversation with thread create".into(),
    }
}

pub(crate) fn policy() -> String {
    let mut digest = Sha256::new();
    digest.update(crate::domain::claude_cli::CLAUDE_WIRE_PROFILE.id.as_bytes());
    for name in [
        "CC_SWITCH_CLAUDE_BETA_PROFILE",
        "CC_SWITCH_CLAUDE_NATIVE_PASSTHROUGH",
        "CC_SWITCH_CLAUDE_CUSTOM_TOOL_ALIAS",
        "CC_SWITCH_CLAUDE_CACHE_REWRITE",
        "CC_SWITCH_CLAUDE_CACHE_TTL",
        "CC_SWITCH_CLAUDE_SCHEMA_DESCRIPTION_FALLBACK",
        "CC_SWITCH_CLAUDE_DATELINE_NORMALIZATION",
        "CC_SWITCH_CLAUDE_CCH_POLICY",
        "CC_SWITCH_CCH_SALT_HEX",
        "CC_SWITCH_CLI_UA",
        "CC_SWITCH_CLI_UA_VERSION",
    ] {
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(std::env::var(name).unwrap_or_default().as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}

fn key(scope: &str, message_id: &str) -> String {
    hex::encode(Sha256::digest(format!(
        "cc-switch:claude-thread-profile:v1\0{scope}\0{message_id}"
    )))
}

fn valid_profile(profile: &ClaudeThreadProfile) -> bool {
    profile.aliases.len() <= 128
        && !profile.betas.is_empty()
        && profile.betas.len() <= 8192
        && !profile.betas.chars().any(char::is_control)
        && profile.policy.len() == 64
        && profile.policy.bytes().all(|ch| ch.is_ascii_hexdigit())
}

fn connect(dir: &Path) -> anyhow::Result<Connection> {
    let path = dir.join("claude-thread-profiles.sqlite3");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let _file = options.open(&path)?;
    let db = Connection::open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    db.busy_timeout(std::time::Duration::from_secs(2))?;
    db.execute_batch("PRAGMA page_size=4096; PRAGMA cache_size=-64; PRAGMA max_page_count=4096;
        CREATE TABLE IF NOT EXISTS thread_profiles (key TEXT PRIMARY KEY, payload TEXT NOT NULL, expires INTEGER NOT NULL, conflicted INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX IF NOT EXISTS thread_profiles_expiry ON thread_profiles(expires);")?;
    let page_size: i64 = db.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let page_count: i64 = db.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    anyhow::ensure!(
        page_size <= 4096 && page_count <= 4096,
        "thread cache disk limit"
    );
    Ok(db)
}

pub(crate) fn load(
    dir: &Path,
    scope: &str,
    message_id: &str,
    now: i64,
    memory: Option<RequestMemoryBudget>,
) -> Result<Option<LoadedProfile>, ProxyError> {
    let db = connect(dir).map_err(|_| unavailable())?;
    let key = key(scope, message_id);
    let size: Option<i64> = db.query_row("SELECT length(CAST(payload AS BLOB)) FROM thread_profiles WHERE key=?1 AND expires>?2 AND conflicted=0",params![key,now],|row|row.get(0)).optional().map_err(|_|unavailable())?;
    let Some(size) = size.filter(|size| *size > 0 && *size <= MAX_ENTRY as i64) else {
        return Ok(None);
    };
    let charge = (size as usize).saturating_mul(16);
    let (reservation, resident) = if let Some(memory) = memory {
        (
            Some(
                memory
                    .reserve(RequestMemoryComponent::StreamRetainedState, charge)
                    .map_err(|e| e.into_proxy_error())?,
            ),
            None,
        )
    } else {
        (
            None,
            Some(
                process_resident_memory()
                    .reserve(charge)
                    .ok_or_else(unavailable)?,
            ),
        )
    };
    let payload: String = db.query_row("SELECT payload FROM thread_profiles WHERE key=?1 AND expires>?2 AND conflicted=0 AND length(CAST(payload AS BLOB))=?3",params![key,now,size],|row|row.get(0)).map_err(|_|unavailable())?;
    let profile: ClaudeThreadProfile = serde_json::from_str(&payload).map_err(|_| unavailable())?;
    if !valid_profile(&profile) {
        return Err(unavailable());
    }
    Ok(Some(LoadedProfile {
        profile,
        _memory: reservation,
        _resident: resident,
    }))
}

pub(crate) fn save(
    dir: &Path,
    scope: &str,
    message_id: &str,
    profile: &ClaudeThreadProfile,
    now: i64,
) -> anyhow::Result<bool> {
    anyhow::ensure!(
        message_id.len() <= 512 && !message_id.is_empty(),
        "invalid thread response identity"
    );
    anyhow::ensure!(valid_profile(profile), "invalid thread profile");
    let size = serialized_json_bytes(profile, MAX_ENTRY)
        .ok_or_else(|| anyhow::anyhow!("thread profile exceeds budget"))?;
    let _memory = process_resident_memory()
        .reserve(size.saturating_mul(2))
        .ok_or_else(|| anyhow::anyhow!("thread profile resident budget exhausted"))?;
    let payload = serde_json::to_string(profile)?;
    let mut db = connect(dir)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM thread_profiles WHERE expires<=?1", [now])?;
    let key = key(scope, message_id);
    // Compare inside SQLite: never allocate an unbounded corrupted prior payload.
    let existing: Option<(bool, bool)> = tx
        .query_row(
            "SELECT payload=?2,conflicted FROM thread_profiles WHERE key=?1",
            params![key, payload],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((identical, conflicted)) = existing {
        if conflicted || !identical {
            tx.execute(
                "UPDATE thread_profiles SET conflicted=1 WHERE key=?1",
                [&key],
            )?;
            tx.commit()?;
            return Ok(false);
        }
    }
    tx.execute("INSERT INTO thread_profiles(key,payload,expires) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET expires=excluded.expires",params![key,payload,now.saturating_add(TTL_MS)])?;
    loop {
        let (count, bytes): (i64, i64) = tx.query_row(
            "SELECT count(*),coalesce(sum(length(CAST(payload AS BLOB))),0) FROM thread_profiles",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if count <= MAX_ENTRIES && bytes <= MAX_BYTES {
            break;
        }
        tx.execute("DELETE FROM thread_profiles WHERE key=(SELECT key FROM thread_profiles WHERE key!=?1 ORDER BY expires,key LIMIT 1)",[&key])?;
    }
    tx.commit()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn thread_profile_bounds_keep_newest_receipt_and_reject_oversized_rows_before_decode() {
        let path =
            std::env::temp_dir().join(format!("cc-switch-thread-bounds-{}", rand::random::<u64>()));
        std::fs::create_dir(&path).unwrap();
        let profile = ClaudeThreadProfile {
            betas: "message-threads-2026-08-12".into(),
            policy: "a".repeat(64),
            aliases: BTreeMap::new(),
        };
        {
            let mut db = connect(&path).unwrap();
            let tx = db.transaction().unwrap();
            let payload = serde_json::to_string(&profile).unwrap();
            for index in 0..MAX_ENTRIES {
                tx.execute(
                    "INSERT INTO thread_profiles(key,payload,expires) VALUES(?1,?2,?3)",
                    params![format!("old-{index}"), payload, TTL_MS + 1],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        assert!(save(&path, "scope", "current", &profile, 1).unwrap());
        assert!(load(&path, "scope", "current", 2, None).unwrap().is_some());
        let db = connect(&path).unwrap();
        let count: i64 = db
            .query_row("SELECT count(*) FROM thread_profiles", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, MAX_ENTRIES);
        db.execute(
            "INSERT INTO thread_profiles(key,payload,expires) VALUES(?1,?2,?3)",
            params![
                key("scope", "oversized"),
                "x".repeat(MAX_ENTRY + 1),
                TTL_MS + 1
            ],
        )
        .unwrap();
        assert!(load(&path, "scope", "oversized", 2, None)
            .unwrap()
            .is_none());
        let mut oversized = profile.clone();
        oversized.betas = "x".repeat(8193);
        assert!(save(&path, "scope", "invalid", &oversized, 2).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path.join("claude-thread-profiles.sqlite3"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        drop(db);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[tokio::test]
    async fn thread_profile_survives_reopen_and_fails_closed_on_conflict_expiry_or_scope() {
        let path = std::env::temp_dir().join(format!(
            "cc-switch-thread-profile-{}",
            rand::random::<u64>()
        ));
        std::fs::create_dir(&path).unwrap();
        let profile = ClaudeThreadProfile {
            betas: "oauth,binding".into(),
            policy: policy(),
            aliases: BTreeMap::from([("wire".into(), "original".into())]),
        };
        assert!(save(&path, "one", "msg_1", &profile, 0).unwrap());
        assert_eq!(
            load(&path, "one", "msg_1", 1, None)
                .unwrap()
                .unwrap()
                .profile,
            profile
        );
        assert!(load(&path, "two", "msg_1", 1, None).unwrap().is_none());
        assert!(load(&path, "one", "msg_1", TTL_MS, None).unwrap().is_none());
        let mut different = profile.clone();
        different.betas = "other".into();
        assert!(!save(&path, "one", "msg_1", &different, 2).unwrap());
        assert!(load(&path, "one", "msg_1", 3, None).unwrap().is_none());
        assert!(!save(&path, "one", "msg_1", &profile, 4).unwrap());
        std::fs::remove_dir_all(path).unwrap();
    }
}
