//! Installation-owned lease journal. The running workspace is fixed at process start.
//! A journal transition is durable before authentication or the selected directory changes.
use crate::infra::storage::{sync_directory, write_json_pretty};
use anyhow::{bail, ensure, Context};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
pub(crate) const ACCESS_EVENT_BATCH: usize = 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Grant {
    pub lease_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buyer_id: Option<String>,
    pub tenant_email: String,
    pub alias: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Operation {
    List { owner_email: String },
    Unlist,
    Begin { grant: Grant },
    Extend { lease_id: String, expires_at: i64 },
    Cancel { lease_id: String },
    Fence { lease_id: String },
    Clean { lease_id: String },
    Return { lease_id: String },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Command {
    pub sequence: i64,
    pub action: Operation,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Journal {
    #[serde(default)]
    pub events: Vec<AccessEvent>,
    #[serde(default)]
    pub managed_activity: bool,
    #[serde(default)]
    sealed_until: i64,
    #[serde(default)]
    token_hash: Option<String>,
    pub owner: Option<String>,
    pub grant: Option<Grant>,
    pub fenced: bool,
    pub purging: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting_return: Option<String>,
    pub last: Option<Command>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrences: Option<u32>,
    pub id: String,
    pub kind: String,
    pub at: i64,
}

#[derive(Debug)]
pub struct Workspace {
    pub root: PathBuf,
    pub boot_lease: Option<String>,
    journal: RwLock<Journal>,
    pub(crate) commands: tokio::sync::Mutex<()>,
    pub(crate) chat: tokio::sync::Mutex<()>,
}

pub static PROCESS_LEASE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// These names are native backup paths. Control databases, server.json,
/// tunnel configuration, sessions and installation keys never enter an export.
pub(crate) fn portable_name(name: &str) -> bool {
    if name.contains('\\')
        || std::path::Path::new(name)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return false;
    }
    matches!(
        name,
        "usage"
            | "accounts.key"
            | "accounts.json"
            | "providers.json"
            | "shares.json"
            | "provider-health.json"
            | "server-store.sqlite3"
            | "server-store-migration.json"
            | "grok-media-tasks.json"
            | "workspace-chat.json"
    ) || name.starts_with("usage/") && (name.ends_with(".json") || name.ends_with(".jsonl"))
}
fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_digit() || (b'a'..=b'f').contains(&c)
            }
        })
}
fn directory(root: &Path, id: &str) -> anyhow::Result<PathBuf> {
    ensure!(valid_id(id), "invalid lease ID");
    let parent = root.join("workspaces");
    let path = parent.join(format!("lease-{id}"));
    for dir in [&parent, &path] {
        match std::fs::symlink_metadata(dir) {
            Ok(meta) => ensure!(
                meta.is_dir() && !meta.is_symlink(),
                "invalid workspace directory"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(path)
}

/// Keep the active owner's paths compatible with offline repair and backup tools.
/// Park their private stores before opening any tenant database, under the
/// installation lock. A restart can resume either direction after any rename.
fn park_owner_stores(root: &Path, leased: bool) -> anyhow::Result<()> {
    let workspaces = root.join("workspaces");
    let owner = workspaces.join("owner");
    if !leased && !owner.try_exists()? {
        return Ok(());
    }
    for directory in [&workspaces, &owner] {
        match std::fs::symlink_metadata(directory) {
            Ok(meta) => ensure!(
                meta.is_dir() && !meta.is_symlink(),
                "invalid owner workspace directory"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && leased => {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new().mode(0o700).create(directory)?;
                sync_directory(directory.parent().context("owner workspace parent")?)?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    let (source, destination) = if leased {
        (root, owner.as_path())
    } else {
        (owner.as_path(), root)
    };
    for name in [
        "accounts.key",
        "accounts.json",
        "providers.json",
        "shares.json",
        "usage",
        "provider-health.json",
        "server-store.sqlite3",
        "server-store.sqlite3-wal",
        "server-store.sqlite3-shm",
        "server-store-migration.json",
        "server-store-activation-request.json",
        "grok-media-tasks.json",
        "workspace-chat.json",
        "backups",
        "tunnels.json",
        "ui-settings.json",
    ] {
        let from = source.join(name);
        match std::fs::symlink_metadata(&from) {
            Ok(meta) => ensure!(
                meta.is_file() || meta.is_dir(),
                "unexpected owner workspace entry"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        }
        let to = destination.join(name);
        ensure!(
            matches!(std::fs::symlink_metadata(&to),Err(ref e) if e.kind()==std::io::ErrorKind::NotFound),
            "owner workspace contains conflicting copies of {name}"
        );
        std::fs::rename(from, to).with_context(|| format!("move owner workspace entry {name}"))?;
        sync_directory(destination)?;
        sync_directory(source)?;
    }
    Ok(())
}

impl Workspace {
    pub fn open(root: PathBuf) -> anyhow::Result<Self> {
        let file = root.join("market-lease.json");
        let mut journal: Journal = match std::fs::read(&file) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).context("read installation lease journal")?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Journal::default(),
            Err(e) => return Err(e.into()),
        };
        if journal.purging {
            let grant = journal
                .grant
                .as_ref()
                .context("missing lease during cleanup")?;
            let path = directory(&root, &grant.lease_id)?;
            if path.try_exists()? {
                std::fs::remove_dir_all(&path)?;
                sync_directory(path.parent().unwrap())?;
            }
            journal.awaiting_return = Some(grant.lease_id.clone());
            journal.grant = None;
            // Receipts belong to the purged lease. Without its grant the Router
            // can no longer acknowledge them, so they must not outlive cleanup.
            journal.events.clear();
            journal.managed_activity = false;
            journal.token_hash = None;
            journal.sealed_until = 0;
            journal.fenced = false;
            journal.purging = false;
            write_json_pretty(&file, &journal)?;
        }
        if let Some(grant) = &journal.grant {
            ensure!(
                directory(&root, &grant.lease_id)?.is_dir(),
                "leased workspace is missing"
            );
        }
        park_owner_stores(&root, journal.grant.is_some())?;
        Ok(Self {
            root,
            boot_lease: journal.grant.as_ref().map(|g| g.lease_id.clone()),
            journal: RwLock::new(journal),
            commands: Default::default(),
            chat: Default::default(),
        })
    }
    pub fn data_dir(&self) -> anyhow::Result<PathBuf> {
        match &self.boot_lease {
            Some(id) => directory(&self.root, id),
            None => Ok(self.root.clone()),
        }
    }
    pub fn view(&self) -> Journal {
        self.journal.read().expect("lease journal").clone()
    }
    pub fn record_event(&self, lease: &str, kind: &str) -> anyhow::Result<()> {
        let mut current = self.journal.write().expect("lease journal");
        ensure!(
            current.grant.as_ref().is_some_and(|g| g.lease_id == lease),
            "lease changed"
        );
        // Cleanup already verified an empty queue and the workspace is about to be
        // deleted. A receipt recorded during the shutdown that follows could never
        // be acknowledged, so it is dropped rather than blocking the operation.
        if current.purging {
            return Ok(());
        }
        let mut next = current.clone();
        let overflow = next.events.len() >= ACCESS_EVENT_BATCH;
        let event = AccessEvent {
            id: format!("{:032x}", rand::random::<u128>()),
            kind: kind.into(),
            at: chrono::Utc::now().timestamp(),
            occurrences: overflow.then_some(1),
        };
        // Published events are immutable until acknowledged. Compact only overflow
        // that has never been included in a control response.
        if let Some(summary) = next
            .events
            .iter_mut()
            .skip(ACCESS_EVENT_BATCH)
            .find(|e| e.kind == kind)
        {
            summary.occurrences = Some(summary.occurrences.unwrap_or(1).saturating_add(1));
        } else {
            next.events.push(event);
        }
        write_json_pretty(&self.root.join("market-lease.json"), &next)?;
        *current = next;
        Ok(())
    }
    pub fn acknowledge(&self, lease: &str, ids: &[String]) -> anyhow::Result<()> {
        let mut current = self.journal.write().expect("lease journal");
        if current.events.is_empty()
            || ids.is_empty()
            || current.grant.as_ref().is_none_or(|g| g.lease_id != lease)
        {
            return Ok(());
        }
        let mut next = current.clone();
        next.events.retain(|e| !ids.contains(&e.id));
        if next.events.len() != current.events.len() {
            write_json_pretty(&self.root.join("market-lease.json"), &next)?;
            *current = next;
        }
        Ok(())
    }
    /// The Router commits release before sending this acknowledgement. Its signed
    /// acknowledgement leaves the sequence for any newer pending Begin untouched.
    pub fn acknowledge_released(&self, lease: &str) -> anyhow::Result<()> {
        let mut journal = self.journal.write().expect("lease journal");
        ensure!(
            self.boot_lease.is_none()
                && journal.grant.is_none()
                && journal.awaiting_return.as_deref() == Some(lease),
            "unexpected released lease"
        );
        let mut next = journal.clone();
        next.awaiting_return = None;
        // Keep the idle journal readable by clients predating Return.
        if let Some(command) = next.last.as_mut() {
            if matches!(command.action, Operation::Return { .. }) {
                command.action = Operation::Cancel {
                    lease_id: lease.to_owned(),
                };
            }
        }
        write_json_pretty(&self.root.join("market-lease.json"), &next)?;
        *journal = next;
        Ok(())
    }
    pub fn management_locked(&self) -> bool {
        let journal = self.journal.read().expect("lease journal");
        journal.grant.is_some() || journal.awaiting_return.is_some() || self.boot_lease.is_some()
    }
    pub fn access(&self) -> (Option<String>, Option<Grant>, bool) {
        let j = self.journal.read().expect("lease journal");
        let live = j.grant.as_ref().is_some_and(|g| {
            self.boot_lease.as_deref() == Some(&g.lease_id)
                && !j.purging
                && !j.fenced
                && g.expires_at > chrono::Utc::now().timestamp()
                && g.expires_at > j.sealed_until
        });
        (j.owner.clone(), j.grant.clone(), live)
    }
    /// Latch expiry across clock rollback or reboot. A later signed extension
    /// may establish a new deadline for the same lease.
    pub fn seal_expired(&self, now: i64) -> anyhow::Result<()> {
        let mut j = self.journal.write().expect("lease journal");
        if let Some(expires) = j
            .grant
            .as_ref()
            .map(|g| g.expires_at)
            .filter(|t| *t <= now && *t > j.sealed_until)
        {
            let mut next = j.clone();
            next.sealed_until = expires;
            write_json_pretty(&self.root.join("market-lease.json"), &next)?;
            *j = next;
        }
        Ok(())
    }
    /// With no third-party liabilities or undelivered activity receipts the Client can
    /// finish an expired rental offline. Access was already disabled at expiry.
    /// A short grace period allows an in-flight, funded renewal to reach the Client.
    pub fn reap_expired(&self, now: i64, drained: bool) -> anyhow::Result<bool> {
        let mut current = self.journal.write().expect("lease journal");
        if !drained
            || current.purging
            || current.managed_activity
            || !current.events.is_empty()
            || !current.grant.as_ref().is_some_and(|g| {
                self.boot_lease.as_deref() == Some(&g.lease_id)
                    && g.expires_at.saturating_add(300) <= now
            })
        {
            return Ok(false);
        }
        let mut next = current.clone();
        next.fenced = true;
        next.purging = true;
        write_json_pretty(&self.root.join("market-lease.json"), &next)?;
        *current = next;
        Ok(true)
    }
    pub fn mark_managed_activity(&self) -> anyhow::Result<()> {
        let mut j = self.journal.write().expect("lease journal");
        if j.grant.is_some() && !j.managed_activity {
            let mut next = j.clone();
            next.managed_activity = true;
            write_json_pretty(&self.root.join("market-lease.json"), &next)?;
            *j = next;
        }
        Ok(())
    }
    pub fn rotate_token(&self, id: &str) -> anyhow::Result<String> {
        use sha2::{Digest, Sha256};
        let mut j = self.journal.write().expect("lease journal");
        ensure!(
            j.grant
                .as_ref()
                .is_some_and(|g| g.lease_id == id && g.expires_at > chrono::Utc::now().timestamp())
                && !j.fenced
                && !j.purging,
            "workspace unavailable"
        );
        let token = hex::encode(rand::random::<[u8; 32]>());
        let mut next = j.clone();
        next.token_hash = Some(hex::encode(Sha256::digest(token.as_bytes())));
        write_json_pretty(&self.root.join("market-lease.json"), &next)?;
        *j = next;
        Ok(token)
    }
    pub fn token_matches(&self, token: &str) -> bool {
        use sha2::{Digest, Sha256};
        token.len() == 64
            && self
                .journal
                .read()
                .expect("lease journal")
                .token_hash
                .as_deref()
                == Some(&hex::encode(Sha256::digest(token.as_bytes())))
    }
    pub fn effective_config(
        &self,
    ) -> anyhow::Result<crate::domain::settings::config::ServerConfig> {
        use crate::domain::settings::config::ServerConfig;
        let mut config = ServerConfig::load_or_default(&self.data_dir()?)?;
        if let Some(grant) = self.view().grant {
            let installation = ServerConfig::load_or_default(&self.root)?;
            config.auth = Default::default();
            config.owner = installation.owner;
            config.client = installation.client;
            config.router = installation.router;
            // Older workspace server.json snapshots could contain the installation policy.
            config.upgrade_policy =
                match std::fs::read(self.data_dir()?.join("lease-upgrade-policy.json")) {
                    Ok(bytes) => {
                        serde_json::from_slice(&bytes).context("read tenant upgrade policy")?
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => Default::default(),
                    Err(err) => return Err(err.into()),
                };
            config.upgrade_policy.delegate_upgrade_to_router_owner = false;
            config.request_body_limits = installation.request_body_limits;
            config.enable_web_terminal = false;
            config.workspace = Some(grant);
        }
        Ok(config)
    }
    pub fn switch_required(&self) -> bool {
        let j = self.journal.read().expect("lease journal");
        j.purging || self.boot_lease.as_deref() != j.grant.as_ref().map(|g| g.lease_id.as_str())
    }
    pub fn apply(
        &self,
        command: Command,
        verified_owner: &str,
        now: i64,
        cleanup_safe: bool,
    ) -> anyhow::Result<()> {
        let mut current = self.journal.write().expect("lease journal");
        if let Some(owner) = &current.owner {
            ensure!(
                owner.eq_ignore_ascii_case(verified_owner),
                "installation owner changed"
            );
        }
        if let Some(last) = &current.last {
            if last == &command {
                return Ok(());
            }
            ensure!(
                command.sequence > last.sequence,
                "stale or conflicting operation"
            );
        }
        ensure!(
            command.sequence > 0 && !current.purging,
            "invalid operation sequence or cleanup in progress"
        );
        if let Some(pending) = &current.awaiting_return {
            ensure!(
                matches!(&command.action, Operation::Return { lease_id } | Operation::Cancel { lease_id } if lease_id == pending),
                "workspace handback pending"
            );
        }
        let mut next = current.clone();
        match &command.action {
            Operation::List { owner_email } => {
                ensure!(
                    current.grant.is_none() && owner_email.eq_ignore_ascii_case(verified_owner),
                    "installation owner changed"
                );
                next.owner = Some(owner_email.to_ascii_lowercase());
            }
            Operation::Unlist => {
                ensure!(current.grant.is_none(), "lease still held");
                next.owner = None;
            }
            Operation::Begin { grant } => {
                ensure!(
                    current
                        .owner
                        .as_deref()
                        .is_some_and(|o| o.eq_ignore_ascii_case(verified_owner))
                        && current.grant.is_none()
                        && self.boot_lease.is_none(),
                    "listing unavailable"
                );
                ensure!(
                    grant.expires_at > now
                        && grant.tenant_email.len() <= 254
                        && grant
                            .alias
                            .strip_prefix("rent-")
                            .is_some_and(|tail| tail.len() == 20
                                && tail.bytes().all(|c| c.is_ascii_hexdigit()))
                        && grant.tenant_email.contains('@')
                        && !grant.tenant_email.chars().any(char::is_control),
                    "invalid grant"
                );
                crate::domain::router::namespace::ClientSubdomain::parse(&grant.alias)?;
                let dir = directory(&self.root, &grant.lease_id)?;
                std::fs::create_dir_all(dir.parent().unwrap())?;
                // A crash before the journal write can only leave an empty directory.
                if dir.try_exists()? {
                    ensure!(
                        dir.read_dir()?.next().is_none(),
                        "workspace ID already used"
                    );
                } else {
                    use std::os::unix::fs::DirBuilderExt;
                    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
                }
                sync_directory(dir.parent().unwrap())?;
                next.grant = Some(grant.clone());
                next.events.clear();
                next.fenced = false;
            }
            Operation::Extend {
                lease_id,
                expires_at,
            } => {
                let grant = next.grant.as_mut().context("no lease")?;
                ensure!(
                    grant.lease_id == *lease_id
                        && !current.fenced
                        && *expires_at >= grant.expires_at
                        && *expires_at > now,
                    "extension does not match lease"
                );
                grant.expires_at = *expires_at;
            }
            Operation::Cancel { lease_id } => {
                ensure!(
                    valid_id(lease_id) && current.grant.is_none() && self.boot_lease.is_none(),
                    "workspace still active"
                );
            }
            Operation::Return { lease_id } => {
                ensure!(
                    current.awaiting_return.as_ref() == Some(lease_id)
                        && current.grant.is_none()
                        && self.boot_lease.is_none(),
                    "unexpected workspace handback"
                );
                // Keep local management locked until the Router commits release.
            }
            Operation::Fence { lease_id } => {
                ensure!(
                    next.grant.as_ref().is_some_and(|g| g.lease_id == *lease_id),
                    "lease changed"
                );
                next.fenced = true;
            }
            Operation::Clean { lease_id } => {
                ensure!(
                    next.fenced
                        && next.grant.as_ref().is_some_and(|g| g.lease_id == *lease_id)
                        && self.boot_lease.as_deref() == Some(lease_id),
                    "cleanup requires the fenced workspace"
                );
                ensure!(current.events.is_empty(), "access events pending delivery");
                if !cleanup_safe {
                    bail!("Share revocation evidence is pending");
                }
                next.purging = true;
            }
        }
        next.last = Some(command);
        write_json_pretty(&self.root.join("market-lease.json"), &next)?;
        *current = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lost_replies_crashes_and_late_cleanup_do_not_cross_rentals() {
        let tmp = std::env::temp_dir().join(format!("cc-market-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("accounts.key"), b"owner-test-key").unwrap();
        let mut w = Workspace::open(tmp.clone()).unwrap();
        let seller = "owner@test.net";
        let first = "ee983484-2e81-499a-b3d4-10708cde2745";
        let second = "ee983484-2e81-499a-b3d4-10708cde2746";
        let grant = |id: &str| Grant {
            buyer_id: None,
            lease_id: id.into(),
            alias: "rent-0123456789abcdef0123".into(),
            tenant_email: "buyer@test.net".into(),
            expires_at: 100,
        };
        let cmd = |sequence, action| Command { sequence, action };
        w.apply(
            cmd(
                1,
                Operation::List {
                    owner_email: seller.into(),
                },
            ),
            seller,
            5,
            true,
        )
        .unwrap();
        let begin = cmd(
            2,
            Operation::Begin {
                grant: grant(first),
            },
        );
        let wire = serde_json::to_vec(&begin).unwrap();
        assert_eq!(serde_json::from_slice::<Command>(&wire).unwrap(), begin);
        w.apply(begin.clone(), seller, 5, true).unwrap();
        assert!(w.switch_required());
        w = Workspace::open(tmp.clone()).unwrap();
        w.apply(begin, seller, 5, true).unwrap();
        let first_dir = w.data_dir().unwrap();
        assert!(!first_dir.join("accounts.key").exists());
        assert!(!tmp.join("accounts.key").exists());
        assert_eq!(
            std::fs::read(tmp.join("workspaces/owner/accounts.key")).unwrap(),
            b"owner-test-key"
        );
        std::fs::write(first_dir.join("evidence"), b"receipt").unwrap();
        w.apply(
            cmd(
                3,
                Operation::Fence {
                    lease_id: first.into(),
                },
            ),
            seller,
            101,
            true,
        )
        .unwrap();
        let clean = cmd(
            4,
            Operation::Clean {
                lease_id: first.into(),
            },
        );
        assert!(w.apply(clean.clone(), seller, 101, false).is_err());
        assert!(first_dir.join("evidence").exists());
        w.apply(clean.clone(), seller, 101, true).unwrap();
        w = Workspace::open(tmp.clone()).unwrap();
        assert!(!first_dir.exists());
        assert_eq!(
            std::fs::read(tmp.join("accounts.key")).unwrap(),
            b"owner-test-key"
        );
        w.apply(clean.clone(), seller, 101, true).unwrap();
        assert!(w.management_locked());
        w.apply(
            cmd(
                5,
                Operation::Return {
                    lease_id: first.into(),
                },
            ),
            seller,
            101,
            true,
        )
        .unwrap();
        assert!(w.management_locked());
        w.acknowledge_released(first).unwrap();
        assert!(!w.management_locked());
        w.apply(
            cmd(
                6,
                Operation::Begin {
                    grant: Grant {
                        tenant_email: seller.into(),
                        ..grant(second)
                    },
                },
            ),
            seller,
            5,
            true,
        )
        .unwrap();
        assert!(w.apply(clean, seller, 5, true).is_err());
        assert!(w
            .apply(
                cmd(
                    7,
                    Operation::Fence {
                        lease_id: first.into()
                    }
                ),
                seller,
                5,
                true
            )
            .is_err());
        assert_eq!(w.view().grant.unwrap().lease_id, second);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn historical_receipts_and_tenant_overflow_survives_lost_acknowledgements() {
        let root = std::env::temp_dir().join(format!("cc-market-events-{}", rand::random::<u64>()));
        std::fs::create_dir(&root).unwrap();
        let workspace = Workspace::open(root.clone()).unwrap();
        let lease = "ee983484-2e81-499a-b3d4-10708cde2747";
        {
            let mut journal = workspace.journal.write().unwrap();
            journal.grant = Some(Grant {
                buyer_id: None,
                lease_id: lease.into(),
                alias: "rent-0123456789abcdef0123".into(),
                tenant_email: "buyer@example.test".into(),
                expires_at: i64::MAX,
            });
            journal.events = (0..1024)
                .map(|n| AccessEvent {
                    id: format!("{n:032x}"),
                    kind: "assistance".into(),
                    at: 1,
                    occurrences: None,
                })
                .collect();
        }
        for _ in 0..2 {
            workspace.record_event(lease, "tenant_upgrade").unwrap();
        }
        workspace.record_event(lease, "tenant_restart").unwrap();
        let published = workspace.view();
        assert_eq!(published.events.len(), 1026);
        assert_eq!(published.events[1024].occurrences, Some(2));
        let ids: Vec<_> = published.events[..1024]
            .iter()
            .map(|e| e.id.clone())
            .collect();
        workspace.acknowledge("stale-lease", &ids).unwrap();
        assert_eq!(workspace.view().events.len(), 1026);
        workspace.acknowledge(lease, &ids).unwrap();
        workspace.acknowledge(lease, &ids).unwrap();
        workspace.record_event(lease, "tenant_upgrade").unwrap();
        let next = workspace.view();
        assert_eq!(next.events.len(), 3);
        assert_eq!(next.events[0].occurrences, Some(2));
        assert_eq!(next.events[2].occurrences, None);
        let disk = std::fs::read(root.join("market-lease.json")).unwrap();
        let saved: Journal = serde_json::from_slice(&disk).unwrap();
        assert_eq!(saved.events[0].id, published.events[1024].id);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn activity_receipts_never_outlive_cleanup() {
        let tmp = std::env::temp_dir().join(format!("cc-market-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&tmp).unwrap();
        let mut w = Workspace::open(tmp.clone()).unwrap();
        let seller = "owner@test.net";
        let lease = "ee983484-2e81-499a-b3d4-10708cde2747";
        let next = "ee983484-2e81-499a-b3d4-10708cde2748";
        let grant = |id: &str| Grant {
            buyer_id: None,
            lease_id: id.into(),
            alias: "rent-0123456789abcdef0123".into(),
            tenant_email: "buyer@test.net".into(),
            expires_at: 100,
        };
        let cmd = |sequence, action| Command { sequence, action };
        w.apply(
            cmd(
                1,
                Operation::List {
                    owner_email: seller.into(),
                },
            ),
            seller,
            5,
            true,
        )
        .unwrap();
        w.apply(
            cmd(
                2,
                Operation::Begin {
                    grant: grant(lease),
                },
            ),
            seller,
            5,
            true,
        )
        .unwrap();
        w = Workspace::open(tmp.clone()).unwrap();
        w.apply(
            cmd(
                3,
                Operation::Fence {
                    lease_id: lease.into(),
                },
            ),
            seller,
            101,
            true,
        )
        .unwrap();
        // Unacknowledged activity must survive fencing.
        w.record_event(lease, "tenant_restart").unwrap();
        let clean = cmd(
            4,
            Operation::Clean {
                lease_id: lease.into(),
            },
        );
        assert!(w.apply(clean.clone(), seller, 101, true).is_err());
        let ids: Vec<String> = w.view().events.iter().map(|e| e.id.clone()).collect();
        w.acknowledge(lease, &ids).unwrap();
        w.apply(clean, seller, 101, true).unwrap();
        // Access during the shutdown after cleanup is dropped, not queued.
        w.record_event(lease, "tenant_upgrade").unwrap();
        assert!(w.view().events.is_empty());
        w = Workspace::open(tmp.clone()).unwrap();
        assert!(w.view().grant.is_none() && w.view().events.is_empty());
        w.apply(
            cmd(
                5,
                Operation::Return {
                    lease_id: lease.into(),
                },
            ),
            seller,
            101,
            true,
        )
        .unwrap();
        w.acknowledge_released(lease).unwrap();
        w.apply(
            cmd(6, Operation::Begin { grant: grant(next) }),
            seller,
            5,
            true,
        )
        .unwrap();
        assert!(w.view().events.is_empty());
        std::fs::remove_dir_all(tmp).unwrap();
    }
}
