use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::router::{
    BaseDomain, ClientKey, NamespaceError, PublicHost, PublicHostClaim, PublicHostKind,
    PROTOCOL_EPOCH,
};
use crate::domain::settings::config::router_control_db_path;

const SCHEMA_VERSION: i64 = 1;
const MAX_OUTBOX_CLAIM: usize = 1_000;

#[derive(Debug, thiserror::Error)]
pub enum RouterControlStoreError {
    #[error("open Router control database: {0}")]
    Io(#[from] std::io::Error),
    #[error("Router control database operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Router control database lock is poisoned")]
    LockPoisoned,
    #[error("Router control database schema {found} is unsupported (expected {expected})")]
    UnsupportedSchema { found: i64, expected: i64 },
    #[error("Router control protocol epoch mismatch: found {found}, expected {expected}")]
    ProtocolEpochMismatch {
        found: String,
        expected: &'static str,
    },
    #[error(transparent)]
    Namespace(#[from] NamespaceError),
    #[error("invalid global device identity: {0}")]
    InvalidDeviceIdentity(String),
    #[error("global device identity has not been initialized")]
    DeviceIdentityMissing,
    #[error("global device identity already uses client prefix {existing}")]
    ClientPrefixMismatch { existing: String },
    #[error("invalid Router profile: {0}")]
    InvalidRouterProfile(String),
    #[error("Router profile {0} already exists")]
    RouterAlreadyExists(String),
    #[error("Router profile {0} does not exist")]
    RouterNotFound(String),
    #[error("the first Router profile must be Primary")]
    FirstRouterMustBePrimary,
    #[error("Primary Router {0} already exists")]
    PrimaryRouterExists(String),
    #[error("cannot remove Primary Router while Auxiliary Routers remain")]
    CannotRemovePrimary,
    #[error("Router installation id {0} is already assigned to another profile")]
    InstallationConflict(String),
    #[error("serialize Router control payload: {0}")]
    SerializePayload(#[source] serde_json::Error),
    #[error("decode Router control payload: {0}")]
    DecodePayload(#[source] serde_json::Error),
    #[error("invalid outbox input: {0}")]
    InvalidOutbox(String),
    #[error("outbox message {0} does not exist")]
    OutboxNotFound(String),
    #[error("outbox lease for {0} is not owned by this worker")]
    OutboxLeaseMismatch(String),
    #[error("invalid sync cursor: {0}")]
    InvalidCursor(String),
    #[error("sync cursor compare-and-set failed: expected {expected:?}, current {current:?}")]
    CursorConflict {
        expected: Option<i64>,
        current: Option<i64>,
    },
    #[error("public host {host} is already claimed by {existing_kind}:{existing_subject}")]
    HostConflict {
        host: String,
        existing_kind: PublicHostKind,
        existing_subject: String,
    },
    #[error("{kind} subject {subject_id} already has a public host on Router {router_id}")]
    SubjectAlreadyClaimed {
        router_id: String,
        kind: PublicHostKind,
        subject_id: String,
    },
    #[error("public host claim uses a client key different from the global DeviceIdentity")]
    ClientKeyMismatch,
    #[error("Router control database invariant failed: {0}")]
    DatabaseInvariant(String),
}

pub type Result<T> = std::result::Result<T, RouterControlStoreError>;

pub struct RouterControlStore {
    path: Option<PathBuf>,
    connection: Mutex<Connection>,
}

impl fmt::Debug for RouterControlStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RouterControlStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdentity {
    pub protocol_epoch: String,
    pub client_prefix: String,
    pub client_key: ClientKey,
    pub public_key: String,
    pub private_key: String,
    pub created_at_ms: i64,
}

impl fmt::Debug for DeviceIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceIdentity")
            .field("protocol_epoch", &self.protocol_epoch)
            .field("client_prefix", &self.client_prefix)
            .field("client_key", &self.client_key)
            .field("public_key", &self.public_key)
            .field("private_key", &"[REDACTED]")
            .field("created_at_ms", &self.created_at_ms)
            .finish()
    }
}

impl DeviceIdentity {
    fn generate(client_prefix: &str, created_at_ms: i64) -> Result<Self> {
        let signing_key = SigningKey::generate(&mut OsRng);
        let public_key_bytes = signing_key.verifying_key().to_bytes();
        let client_key = ClientKey::derive(client_prefix, &public_key_bytes)?;
        Ok(Self {
            protocol_epoch: PROTOCOL_EPOCH.to_string(),
            client_prefix: client_prefix.to_string(),
            client_key,
            public_key: STANDARD.encode(public_key_bytes),
            private_key: STANDARD.encode(signing_key.to_bytes()),
            created_at_ms,
        })
    }

    fn validate(&self) -> Result<()> {
        if self.protocol_epoch != PROTOCOL_EPOCH {
            return Err(RouterControlStoreError::ProtocolEpochMismatch {
                found: self.protocol_epoch.clone(),
                expected: PROTOCOL_EPOCH,
            });
        }
        let private_key = STANDARD.decode(&self.private_key).map_err(|_| {
            RouterControlStoreError::InvalidDeviceIdentity(
                "private key is not valid base64".to_string(),
            )
        })?;
        let private_key: [u8; 32] = private_key.try_into().map_err(|_| {
            RouterControlStoreError::InvalidDeviceIdentity(
                "private key must contain 32 bytes".to_string(),
            )
        })?;
        let signing_key = SigningKey::from_bytes(&private_key);
        let expected_public_key = signing_key.verifying_key().to_bytes();
        let public_key = STANDARD.decode(&self.public_key).map_err(|_| {
            RouterControlStoreError::InvalidDeviceIdentity(
                "public key is not valid base64".to_string(),
            )
        })?;
        if public_key.as_slice() != expected_public_key {
            return Err(RouterControlStoreError::InvalidDeviceIdentity(
                "public and private keys do not match".to_string(),
            ));
        }
        let expected_client_key = ClientKey::derive(&self.client_prefix, &expected_public_key)?;
        if self.client_key != expected_client_key {
            return Err(RouterControlStoreError::InvalidDeviceIdentity(
                "client key does not match the public key fingerprint".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouterRole {
    Primary,
    Auxiliary,
}

impl RouterRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Auxiliary => "auxiliary",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "primary" => Ok(Self::Primary),
            "auxiliary" => Ok(Self::Auxiliary),
            _ => Err(RouterControlStoreError::DatabaseInvariant(format!(
                "unknown Router role {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRouterProfile {
    pub router_id: String,
    pub api_base: String,
    pub base_domain: BaseDomain,
    pub role: RouterRole,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterProfile {
    pub protocol_epoch: String,
    pub router_id: String,
    pub api_base: String,
    pub base_domain: BaseDomain,
    pub installation_id: Option<String>,
    pub control_secret: Option<String>,
    pub role: RouterRole,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl fmt::Debug for RouterProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RouterProfile")
            .field("protocol_epoch", &self.protocol_epoch)
            .field("router_id", &self.router_id)
            .field("api_base", &self.api_base)
            .field("base_domain", &self.base_domain)
            .field("installation_id", &self.installation_id)
            .field(
                "control_secret",
                &self.control_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("role", &self.role)
            .field("created_at_ms", &self.created_at_ms)
            .field("updated_at_ms", &self.updated_at_ms)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrimaryRouterState {
    pub router_id: Option<String>,
    pub controller_epoch: i64,
}

impl RouterControlStore {
    pub fn open(config_dir: &Path) -> Result<Self> {
        Self::open_path(router_control_db_path(config_dir))
    }

    pub fn open_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        configure_connection(&connection)?;
        initialize_schema(&connection)?;
        harden_database_permissions(path)?;
        Ok(Self {
            path: Some(path.to_path_buf()),
            connection: Mutex::new(connection),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        configure_connection(&connection)?;
        initialize_schema(&connection)?;
        Ok(Self {
            path: None,
            connection: Mutex::new(connection),
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| RouterControlStoreError::LockPoisoned)
    }

    pub fn load_device_identity(&self) -> Result<Option<DeviceIdentity>> {
        let connection = self.connection()?;
        load_device_identity(&connection)
    }

    pub fn load_or_create_device_identity(
        &self,
        client_prefix: &str,
        now_ms: i64,
    ) -> Result<DeviceIdentity> {
        validate_timestamp(now_ms)?;
        // Validate the prefix before starting the write transaction.
        let placeholder_key = [0_u8; 32];
        ClientKey::derive(client_prefix, &placeholder_key)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(identity) = load_device_identity(&transaction)? {
            if identity.client_prefix != client_prefix {
                return Err(RouterControlStoreError::ClientPrefixMismatch {
                    existing: identity.client_prefix,
                });
            }
            transaction.commit()?;
            return Ok(identity);
        }
        let identity = DeviceIdentity::generate(client_prefix, now_ms)?;
        transaction.execute(
            "INSERT INTO device_identity (
                 singleton, protocol_epoch, client_prefix, client_key,
                 public_key, private_key, created_at_ms
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                PROTOCOL_EPOCH,
                identity.client_prefix,
                identity.client_key.as_str(),
                identity.public_key,
                identity.private_key,
                identity.created_at_ms,
            ],
        )?;
        transaction.commit()?;
        Ok(identity)
    }

    pub fn insert_router_profile(
        &self,
        profile: NewRouterProfile,
        now_ms: i64,
    ) -> Result<RouterProfile> {
        validate_timestamp(now_ms)?;
        let router_id = validate_router_id(&profile.router_id)?;
        let api_base = normalize_api_base(&profile.api_base, &profile.base_domain)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if router_profile_exists(&transaction, &router_id)? {
            return Err(RouterControlStoreError::RouterAlreadyExists(router_id));
        }
        let profile_count: i64 =
            transaction.query_row("SELECT COUNT(*) FROM router_profiles", [], |row| row.get(0))?;
        if profile_count == 0 && profile.role != RouterRole::Primary {
            return Err(RouterControlStoreError::FirstRouterMustBePrimary);
        }
        if profile.role == RouterRole::Primary {
            if let Some(primary) = current_primary_router_id(&transaction)? {
                return Err(RouterControlStoreError::PrimaryRouterExists(primary));
            }
        }
        transaction.execute(
            "INSERT INTO router_profiles (
                 router_id, protocol_epoch, api_base, base_domain, role,
                 created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                router_id,
                PROTOCOL_EPOCH,
                api_base,
                profile.base_domain.as_str(),
                profile.role.as_str(),
                now_ms,
            ],
        )?;
        if profile.role == RouterRole::Primary {
            transaction.execute(
                "UPDATE control_state
                 SET primary_router_id = ?1,
                     controller_epoch = controller_epoch + 1
                 WHERE singleton = 1",
                params![router_id],
            )?;
        }
        let stored = load_router_profile(&transaction, &router_id)?
            .ok_or_else(|| RouterControlStoreError::RouterNotFound(router_id.clone()))?;
        transaction.commit()?;
        Ok(stored)
    }

    pub fn router_profile(&self, router_id: &str) -> Result<Option<RouterProfile>> {
        let router_id = validate_router_id(router_id)?;
        let connection = self.connection()?;
        load_router_profile(&connection, &router_id)
    }

    pub fn router_profiles(&self) -> Result<Vec<RouterProfile>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT protocol_epoch, router_id, api_base, base_domain,
                    installation_id, control_secret, role, created_at_ms, updated_at_ms
             FROM router_profiles
             ORDER BY CASE role WHEN 'primary' THEN 0 ELSE 1 END, router_id",
        )?;
        let rows = statement.query_map([], router_profile_from_row)?;
        rows.map(|row| row.map_err(RouterControlStoreError::from))
            .collect()
    }

    pub fn set_router_installation(
        &self,
        router_id: &str,
        installation_id: &str,
        control_secret: &str,
        now_ms: i64,
    ) -> Result<RouterProfile> {
        let router_id = validate_router_id(router_id)?;
        let installation_id = validate_opaque_id("installation id", installation_id, 255)?;
        let control_secret = validate_secret(control_secret)?;
        validate_timestamp(now_ms)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !router_profile_exists(&transaction, &router_id)? {
            return Err(RouterControlStoreError::RouterNotFound(router_id));
        }
        let conflict: Option<String> = transaction
            .query_row(
                "SELECT router_id FROM router_profiles
                 WHERE installation_id = ?1 AND router_id <> ?2",
                params![installation_id, router_id],
                |row| row.get(0),
            )
            .optional()?;
        if conflict.is_some() {
            return Err(RouterControlStoreError::InstallationConflict(
                installation_id,
            ));
        }
        transaction.execute(
            "UPDATE router_profiles
             SET installation_id = ?2, control_secret = ?3, updated_at_ms = ?4
             WHERE router_id = ?1",
            params![router_id, installation_id, control_secret, now_ms],
        )?;
        let profile = load_router_profile(&transaction, &router_id)?
            .ok_or_else(|| RouterControlStoreError::RouterNotFound(router_id.clone()))?;
        transaction.commit()?;
        Ok(profile)
    }

    pub fn primary_state(&self) -> Result<PrimaryRouterState> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT primary_router_id, controller_epoch
                 FROM control_state WHERE singleton = 1",
                [],
                |row| {
                    Ok(PrimaryRouterState {
                        router_id: row.get(0)?,
                        controller_epoch: row.get(1)?,
                    })
                },
            )
            .map_err(RouterControlStoreError::from)
    }

    pub fn set_primary_router(&self, router_id: &str, now_ms: i64) -> Result<PrimaryRouterState> {
        let router_id = validate_router_id(router_id)?;
        validate_timestamp(now_ms)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !router_profile_exists(&transaction, &router_id)? {
            return Err(RouterControlStoreError::RouterNotFound(router_id));
        }
        let current = current_primary_router_id(&transaction)?;
        if current.as_deref() != Some(router_id.as_str()) {
            transaction.execute(
                "UPDATE router_profiles
                 SET role = 'auxiliary', updated_at_ms = ?1
                 WHERE role = 'primary'",
                params![now_ms],
            )?;
            transaction.execute(
                "UPDATE router_profiles
                 SET role = 'primary', updated_at_ms = ?2
                 WHERE router_id = ?1",
                params![router_id, now_ms],
            )?;
            transaction.execute(
                "UPDATE control_state
                 SET primary_router_id = ?1,
                     controller_epoch = controller_epoch + 1
                 WHERE singleton = 1",
                params![router_id],
            )?;
        }
        let state = transaction.query_row(
            "SELECT primary_router_id, controller_epoch
             FROM control_state WHERE singleton = 1",
            [],
            |row| {
                Ok(PrimaryRouterState {
                    router_id: row.get(0)?,
                    controller_epoch: row.get(1)?,
                })
            },
        )?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn remove_router_profile(&self, router_id: &str) -> Result<()> {
        let router_id = validate_router_id(router_id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let profile = load_router_profile(&transaction, &router_id)?
            .ok_or_else(|| RouterControlStoreError::RouterNotFound(router_id.clone()))?;
        if profile.role == RouterRole::Primary {
            let count: i64 =
                transaction
                    .query_row("SELECT COUNT(*) FROM router_profiles", [], |row| row.get(0))?;
            if count > 1 {
                return Err(RouterControlStoreError::CannotRemovePrimary);
            }
            transaction.execute(
                "UPDATE control_state
                 SET primary_router_id = NULL,
                     controller_epoch = controller_epoch + 1
                 WHERE singleton = 1",
                [],
            )?;
        }
        transaction.execute(
            "DELETE FROM router_profiles WHERE router_id = ?1",
            params![router_id],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewOutboxMessage {
    pub router_id: String,
    pub operation: String,
    pub payload: Value,
    pub dedupe_key: Option<String>,
    pub available_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueResult {
    pub message_id: String,
    pub inserted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxStatus {
    Pending,
    InFlight,
    DeadLetter,
}

impl OutboxStatus {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "in_flight" => Ok(Self::InFlight),
            "dead_letter" => Ok(Self::DeadLetter),
            _ => Err(RouterControlStoreError::DatabaseInvariant(format!(
                "unknown outbox status {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboxMessage {
    pub message_id: String,
    pub protocol_epoch: String,
    pub router_id: String,
    pub operation: String,
    pub payload: Value,
    pub dedupe_key: Option<String>,
    pub status: OutboxStatus,
    pub attempt_count: i64,
    pub next_attempt_at_ms: i64,
    pub lease_owner: Option<String>,
    pub lease_until_ms: Option<i64>,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl RouterControlStore {
    pub fn enqueue_outbox(&self, message: NewOutboxMessage, now_ms: i64) -> Result<EnqueueResult> {
        let router_id = validate_router_id(&message.router_id)?;
        let operation = validate_operation(&message.operation)?;
        let dedupe_key = message
            .dedupe_key
            .as_deref()
            .map(|value| validate_opaque_id("outbox dedupe key", value, 255))
            .transpose()?;
        validate_timestamp(message.available_at_ms)?;
        validate_timestamp(now_ms)?;
        let payload_json = serde_json::to_string(&message.payload)
            .map_err(RouterControlStoreError::SerializePayload)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !router_profile_exists(&transaction, &router_id)? {
            return Err(RouterControlStoreError::RouterNotFound(router_id));
        }
        if let Some(dedupe_key) = dedupe_key.as_deref() {
            if let Some(message_id) = transaction
                .query_row(
                    "SELECT message_id FROM control_outbox
                     WHERE router_id = ?1 AND dedupe_key = ?2",
                    params![router_id, dedupe_key],
                    |row| row.get(0),
                )
                .optional()?
            {
                transaction.commit()?;
                return Ok(EnqueueResult {
                    message_id,
                    inserted: false,
                });
            }
        }
        let message_id = random_id("outbox");
        transaction.execute(
            "INSERT INTO control_outbox (
                 message_id, protocol_epoch, router_id, operation, payload_json,
                 dedupe_key, status, attempt_count, next_attempt_at_ms,
                 created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', 0, ?7, ?8, ?8)",
            params![
                message_id,
                PROTOCOL_EPOCH,
                router_id,
                operation,
                payload_json,
                dedupe_key,
                message.available_at_ms,
                now_ms,
            ],
        )?;
        transaction.commit()?;
        Ok(EnqueueResult {
            message_id,
            inserted: true,
        })
    }

    pub fn claim_due_outbox(
        &self,
        router_id: &str,
        worker_id: &str,
        now_ms: i64,
        lease_duration_ms: i64,
        limit: usize,
    ) -> Result<Vec<OutboxMessage>> {
        let router_id = validate_router_id(router_id)?;
        let worker_id = validate_opaque_id("outbox worker id", worker_id, 255)?;
        validate_timestamp(now_ms)?;
        if lease_duration_ms <= 0 {
            return Err(RouterControlStoreError::InvalidOutbox(
                "lease duration must be positive".to_string(),
            ));
        }
        let limit = limit.min(MAX_OUTBOX_CLAIM);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let lease_until_ms = now_ms.checked_add(lease_duration_ms).ok_or_else(|| {
            RouterControlStoreError::InvalidOutbox("lease deadline overflow".to_string())
        })?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !router_profile_exists(&transaction, &router_id)? {
            return Err(RouterControlStoreError::RouterNotFound(router_id));
        }
        let mut statement = transaction.prepare(
            "SELECT message_id FROM control_outbox
             WHERE router_id = ?1
               AND status <> 'dead_letter'
               AND next_attempt_at_ms <= ?2
               AND (status = 'pending'
                    OR (status = 'in_flight' AND lease_until_ms <= ?2))
             ORDER BY next_attempt_at_ms, created_at_ms, message_id
             LIMIT ?3",
        )?;
        let message_ids = statement
            .query_map(params![router_id, now_ms, limit as i64], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        for message_id in &message_ids {
            transaction.execute(
                "UPDATE control_outbox
                 SET status = 'in_flight',
                     attempt_count = attempt_count + 1,
                     lease_owner = ?2,
                     lease_until_ms = ?3,
                     updated_at_ms = ?4
                 WHERE message_id = ?1",
                params![message_id, worker_id, lease_until_ms, now_ms],
            )?;
        }
        let mut messages = Vec::with_capacity(message_ids.len());
        for message_id in message_ids {
            messages.push(
                load_outbox_message(&transaction, &message_id)?
                    .ok_or_else(|| RouterControlStoreError::OutboxNotFound(message_id.clone()))?,
            );
        }
        transaction.commit()?;
        Ok(messages)
    }

    pub fn acknowledge_outbox(&self, message_id: &str, worker_id: &str) -> Result<()> {
        let message_id = validate_opaque_id("outbox message id", message_id, 255)?;
        let worker_id = validate_opaque_id("outbox worker id", worker_id, 255)?;
        let connection = self.connection()?;
        let changed = connection.execute(
            "DELETE FROM control_outbox
             WHERE message_id = ?1 AND status = 'in_flight' AND lease_owner = ?2",
            params![message_id, worker_id],
        )?;
        if changed == 0 {
            return classify_outbox_lease_failure(&connection, &message_id);
        }
        Ok(())
    }

    pub fn reschedule_outbox(
        &self,
        message_id: &str,
        worker_id: &str,
        next_attempt_at_ms: i64,
        error: &str,
        now_ms: i64,
    ) -> Result<()> {
        let message_id = validate_opaque_id("outbox message id", message_id, 255)?;
        let worker_id = validate_opaque_id("outbox worker id", worker_id, 255)?;
        let error = validate_error_message(error)?;
        validate_timestamp(next_attempt_at_ms)?;
        validate_timestamp(now_ms)?;
        let connection = self.connection()?;
        let changed = connection.execute(
            "UPDATE control_outbox
             SET status = 'pending', next_attempt_at_ms = ?3,
                 lease_owner = NULL, lease_until_ms = NULL,
                 last_error = ?4, updated_at_ms = ?5
             WHERE message_id = ?1 AND status = 'in_flight' AND lease_owner = ?2",
            params![message_id, worker_id, next_attempt_at_ms, error, now_ms],
        )?;
        if changed == 0 {
            return classify_outbox_lease_failure(&connection, &message_id);
        }
        Ok(())
    }

    pub fn dead_letter_outbox(
        &self,
        message_id: &str,
        worker_id: &str,
        error: &str,
        now_ms: i64,
    ) -> Result<()> {
        let message_id = validate_opaque_id("outbox message id", message_id, 255)?;
        let worker_id = validate_opaque_id("outbox worker id", worker_id, 255)?;
        let error = validate_error_message(error)?;
        validate_timestamp(now_ms)?;
        let connection = self.connection()?;
        let changed = connection.execute(
            "UPDATE control_outbox
             SET status = 'dead_letter', lease_owner = NULL, lease_until_ms = NULL,
                 last_error = ?3, updated_at_ms = ?4
             WHERE message_id = ?1 AND status = 'in_flight' AND lease_owner = ?2",
            params![message_id, worker_id, error, now_ms],
        )?;
        if changed == 0 {
            return classify_outbox_lease_failure(&connection, &message_id);
        }
        Ok(())
    }

    pub fn outbox_message(&self, message_id: &str) -> Result<Option<OutboxMessage>> {
        let message_id = validate_opaque_id("outbox message id", message_id, 255)?;
        let connection = self.connection()?;
        load_outbox_message(&connection, &message_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncCursor {
    pub protocol_epoch: String,
    pub router_id: String,
    pub stream: String,
    pub position: String,
    pub sequence: i64,
    pub updated_at_ms: i64,
}

impl RouterControlStore {
    pub fn sync_cursor(&self, router_id: &str, stream: &str) -> Result<Option<SyncCursor>> {
        let router_id = validate_router_id(router_id)?;
        let stream = validate_operation(stream)?;
        let connection = self.connection()?;
        load_sync_cursor(&connection, &router_id, &stream)
    }

    pub fn compare_and_set_sync_cursor(
        &self,
        router_id: &str,
        stream: &str,
        expected_sequence: Option<i64>,
        next_sequence: i64,
        position: &str,
        now_ms: i64,
    ) -> Result<SyncCursor> {
        let router_id = validate_router_id(router_id)?;
        let stream = validate_operation(stream)?;
        let position = validate_opaque_id("sync cursor position", position, 2_048)?;
        if next_sequence < 0 || expected_sequence.is_some_and(|value| value < 0) {
            return Err(RouterControlStoreError::InvalidCursor(
                "cursor sequences must be non-negative".to_string(),
            ));
        }
        validate_timestamp(now_ms)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !router_profile_exists(&transaction, &router_id)? {
            return Err(RouterControlStoreError::RouterNotFound(router_id));
        }
        let current = load_sync_cursor(&transaction, &router_id, &stream)?;
        let current_sequence = current.as_ref().map(|cursor| cursor.sequence);
        if current_sequence != expected_sequence {
            return Err(RouterControlStoreError::CursorConflict {
                expected: expected_sequence,
                current: current_sequence,
            });
        }
        if let Some(current) = current.as_ref() {
            if next_sequence <= current.sequence {
                return Err(RouterControlStoreError::InvalidCursor(
                    "next cursor sequence must advance".to_string(),
                ));
            }
            transaction.execute(
                "UPDATE sync_cursors
                 SET position = ?3, sequence = ?4, updated_at_ms = ?5
                 WHERE router_id = ?1 AND stream = ?2",
                params![router_id, stream, position, next_sequence, now_ms],
            )?;
        } else {
            transaction.execute(
                "INSERT INTO sync_cursors (
                     router_id, stream, protocol_epoch, position, sequence, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    router_id,
                    stream,
                    PROTOCOL_EPOCH,
                    position,
                    next_sequence,
                    now_ms
                ],
            )?;
        }
        let cursor = load_sync_cursor(&transaction, &router_id, &stream)?.ok_or_else(|| {
            RouterControlStoreError::DatabaseInvariant(
                "cursor disappeared during transaction".to_string(),
            )
        })?;
        transaction.commit()?;
        Ok(cursor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredHostClaim {
    pub protocol_epoch: String,
    pub router_id: String,
    pub host: PublicHost,
    pub kind: PublicHostKind,
    pub subject_id: String,
    pub client_key: Option<ClientKey>,
    pub slug: Option<String>,
    pub created_at_ms: i64,
}

impl RouterControlStore {
    pub fn claim_public_host(
        &self,
        router_id: &str,
        claim: &PublicHostClaim,
        now_ms: i64,
    ) -> Result<bool> {
        let router_id = validate_router_id(router_id)?;
        validate_timestamp(now_ms)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let profile = load_router_profile(&transaction, &router_id)?
            .ok_or_else(|| RouterControlStoreError::RouterNotFound(router_id.clone()))?;
        claim.validate_for(&profile.base_domain)?;
        if claim.kind() != PublicHostKind::Market {
            let identity = load_device_identity(&transaction)?
                .ok_or(RouterControlStoreError::DeviceIdentityMissing)?;
            if claim.client_key() != Some(&identity.client_key) {
                return Err(RouterControlStoreError::ClientKeyMismatch);
            }
        }
        if let Some(existing) = load_host_claim(&transaction, claim.host())? {
            if existing.router_id == router_id
                && existing.kind == claim.kind()
                && existing.subject_id == claim.subject_id()
                && existing.client_key.as_ref() == claim.client_key()
                && existing.slug.as_deref() == claim.slug()
            {
                transaction.commit()?;
                return Ok(false);
            }
            return Err(RouterControlStoreError::HostConflict {
                host: claim.host().to_string(),
                existing_kind: existing.kind,
                existing_subject: existing.subject_id,
            });
        }
        let subject_conflict: Option<String> = transaction
            .query_row(
                "SELECT host FROM public_hosts
                 WHERE router_id = ?1 AND kind = ?2 AND subject_id = ?3",
                params![router_id, claim.kind().as_str(), claim.subject_id()],
                |row| row.get(0),
            )
            .optional()?;
        if subject_conflict.is_some() {
            return Err(RouterControlStoreError::SubjectAlreadyClaimed {
                router_id,
                kind: claim.kind(),
                subject_id: claim.subject_id().to_string(),
            });
        }
        transaction.execute(
            "INSERT INTO public_hosts (
                 host, protocol_epoch, router_id, kind, subject_id,
                 client_key, slug, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                claim.host().as_str(),
                PROTOCOL_EPOCH,
                router_id,
                claim.kind().as_str(),
                claim.subject_id(),
                claim.client_key().map(ClientKey::as_str),
                claim.slug(),
                now_ms,
            ],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn resolve_public_host(&self, host: &str) -> Result<Option<StoredHostClaim>> {
        let host = PublicHost::parse(host)?;
        let connection = self.connection()?;
        load_host_claim(&connection, &host)
    }

    pub fn release_public_host(
        &self,
        router_id: &str,
        host: &str,
        subject_id: &str,
    ) -> Result<bool> {
        let router_id = validate_router_id(router_id)?;
        let host = PublicHost::parse(host)?;
        let subject_id = validate_opaque_id("host subject id", subject_id, 255)?;
        let connection = self.connection()?;
        let changed = connection.execute(
            "DELETE FROM public_hosts
             WHERE host = ?1 AND router_id = ?2 AND subject_id = ?3",
            params![host.as_str(), router_id, subject_id],
        )?;
        Ok(changed == 1)
    }

    pub fn public_hosts(&self, router_id: &str) -> Result<Vec<StoredHostClaim>> {
        let router_id = validate_router_id(router_id)?;
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT protocol_epoch, router_id, host, kind, subject_id,
                    client_key, slug, created_at_ms
             FROM public_hosts WHERE router_id = ?1 ORDER BY host",
        )?;
        let rows = statement.query_map(params![router_id], host_claim_from_row)?;
        rows.map(|row| row.map_err(RouterControlStoreError::from))
            .collect()
    }
}

fn configure_connection(connection: &Connection) -> Result<()> {
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;
         PRAGMA trusted_schema = OFF;",
    )?;
    Ok(())
}

fn initialize_schema(connection: &Connection) -> Result<()> {
    let found: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if found != 0 && found != SCHEMA_VERSION {
        return Err(RouterControlStoreError::UnsupportedSchema {
            found,
            expected: SCHEMA_VERSION,
        });
    }
    if found == 0 {
        let existing_tables: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;
        if existing_tables != 0 {
            return Err(RouterControlStoreError::UnsupportedSchema {
                found,
                expected: SCHEMA_VERSION,
            });
        }
    }
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_meta (
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL
         ) STRICT;

         CREATE TABLE IF NOT EXISTS device_identity (
             singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
             protocol_epoch TEXT NOT NULL CHECK (protocol_epoch = 'namespace-flat-1'),
             client_prefix TEXT NOT NULL,
             client_key TEXT NOT NULL UNIQUE,
             public_key TEXT NOT NULL UNIQUE,
             private_key TEXT NOT NULL,
             created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
         ) STRICT;

         CREATE TABLE IF NOT EXISTS router_profiles (
             router_id TEXT PRIMARY KEY,
             protocol_epoch TEXT NOT NULL CHECK (protocol_epoch = 'namespace-flat-1'),
             api_base TEXT NOT NULL UNIQUE,
             base_domain TEXT NOT NULL UNIQUE,
             installation_id TEXT UNIQUE,
             control_secret TEXT,
             role TEXT NOT NULL CHECK (role IN ('primary', 'auxiliary')),
             created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
             updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
             CHECK ((installation_id IS NULL) = (control_secret IS NULL))
         ) STRICT;

         CREATE UNIQUE INDEX IF NOT EXISTS one_primary_router
         ON router_profiles(role) WHERE role = 'primary';

         CREATE TABLE IF NOT EXISTS control_state (
             singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
             primary_router_id TEXT REFERENCES router_profiles(router_id) ON DELETE SET NULL,
             controller_epoch INTEGER NOT NULL CHECK (controller_epoch >= 0)
         ) STRICT;

         CREATE TABLE IF NOT EXISTS control_outbox (
             message_id TEXT PRIMARY KEY,
             protocol_epoch TEXT NOT NULL CHECK (protocol_epoch = 'namespace-flat-1'),
             router_id TEXT NOT NULL REFERENCES router_profiles(router_id) ON DELETE CASCADE,
             operation TEXT NOT NULL,
             payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
             dedupe_key TEXT,
             status TEXT NOT NULL CHECK (status IN ('pending', 'in_flight', 'dead_letter')),
             attempt_count INTEGER NOT NULL CHECK (attempt_count >= 0),
             next_attempt_at_ms INTEGER NOT NULL CHECK (next_attempt_at_ms >= 0),
             lease_owner TEXT,
             lease_until_ms INTEGER,
             last_error TEXT,
             created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
             updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
             CHECK ((status = 'in_flight') = (lease_owner IS NOT NULL)),
             CHECK ((status = 'in_flight') = (lease_until_ms IS NOT NULL))
         ) STRICT;

         CREATE UNIQUE INDEX IF NOT EXISTS control_outbox_dedupe
         ON control_outbox(router_id, dedupe_key) WHERE dedupe_key IS NOT NULL;
         CREATE INDEX IF NOT EXISTS control_outbox_due
         ON control_outbox(router_id, status, next_attempt_at_ms, lease_until_ms);

         CREATE TABLE IF NOT EXISTS sync_cursors (
             router_id TEXT NOT NULL REFERENCES router_profiles(router_id) ON DELETE CASCADE,
             stream TEXT NOT NULL,
             protocol_epoch TEXT NOT NULL CHECK (protocol_epoch = 'namespace-flat-1'),
             position TEXT NOT NULL,
             sequence INTEGER NOT NULL CHECK (sequence >= 0),
             updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
             PRIMARY KEY (router_id, stream)
         ) STRICT;

         CREATE TABLE IF NOT EXISTS public_hosts (
             host TEXT PRIMARY KEY COLLATE NOCASE,
             protocol_epoch TEXT NOT NULL CHECK (protocol_epoch = 'namespace-flat-1'),
             router_id TEXT NOT NULL REFERENCES router_profiles(router_id) ON DELETE CASCADE,
             kind TEXT NOT NULL CHECK (kind IN ('client', 'share', 'market')),
             subject_id TEXT NOT NULL,
             client_key TEXT,
             slug TEXT,
             created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
             UNIQUE (router_id, kind, subject_id),
             CHECK (
                 (kind = 'client' AND client_key IS NOT NULL AND slug IS NULL)
                 OR (kind = 'share' AND client_key IS NOT NULL AND slug IS NOT NULL)
                 OR (kind = 'market' AND client_key IS NULL AND slug IS NOT NULL)
             )
         ) STRICT;

         INSERT OR IGNORE INTO schema_meta(key, value)
         VALUES ('protocol_epoch', 'namespace-flat-1');
         INSERT OR IGNORE INTO control_state(singleton, primary_router_id, controller_epoch)
         VALUES (1, NULL, 0);",
    )?;
    let epoch: String = connection.query_row(
        "SELECT value FROM schema_meta WHERE key = 'protocol_epoch'",
        [],
        |row| row.get(0),
    )?;
    if epoch != PROTOCOL_EPOCH {
        return Err(RouterControlStoreError::ProtocolEpochMismatch {
            found: epoch,
            expected: PROTOCOL_EPOCH,
        });
    }
    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

#[cfg(unix)]
fn harden_database_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn harden_database_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn load_device_identity(connection: &Connection) -> Result<Option<DeviceIdentity>> {
    let raw: Option<(String, String, String, String, String, i64)> = connection
        .query_row(
            "SELECT protocol_epoch, client_prefix, client_key,
                    public_key, private_key, created_at_ms
             FROM device_identity WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;
    let Some((protocol_epoch, client_prefix, client_key, public_key, private_key, created_at_ms)) =
        raw
    else {
        return Ok(None);
    };
    let identity = DeviceIdentity {
        protocol_epoch,
        client_prefix,
        client_key: ClientKey::parse(&client_key)?,
        public_key,
        private_key,
        created_at_ms,
    };
    identity.validate()?;
    Ok(Some(identity))
}

fn router_profile_exists(connection: &Connection, router_id: &str) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM router_profiles WHERE router_id = ?1)",
            params![router_id],
            |row| row.get(0),
        )
        .map_err(RouterControlStoreError::from)
}

fn current_primary_router_id(connection: &Connection) -> Result<Option<String>> {
    connection
        .query_row(
            "SELECT primary_router_id FROM control_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(RouterControlStoreError::from)
}

fn load_router_profile(connection: &Connection, router_id: &str) -> Result<Option<RouterProfile>> {
    connection
        .query_row(
            "SELECT protocol_epoch, router_id, api_base, base_domain,
                    installation_id, control_secret, role, created_at_ms, updated_at_ms
             FROM router_profiles WHERE router_id = ?1",
            params![router_id],
            router_profile_from_row,
        )
        .optional()
        .map_err(RouterControlStoreError::from)
}

fn router_profile_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RouterProfile> {
    let base_domain: String = row.get(3)?;
    let role: String = row.get(6)?;
    Ok(RouterProfile {
        protocol_epoch: row.get(0)?,
        router_id: row.get(1)?,
        api_base: row.get(2)?,
        base_domain: BaseDomain::parse(&base_domain).map_err(|error| conversion_error(3, error))?,
        installation_id: row.get(4)?,
        control_secret: row.get(5)?,
        role: RouterRole::parse(&role).map_err(|error| conversion_error(6, error))?,
        created_at_ms: row.get(7)?,
        updated_at_ms: row.get(8)?,
    })
}

#[derive(Debug)]
struct RawOutboxMessage {
    message_id: String,
    protocol_epoch: String,
    router_id: String,
    operation: String,
    payload_json: String,
    dedupe_key: Option<String>,
    status: String,
    attempt_count: i64,
    next_attempt_at_ms: i64,
    lease_owner: Option<String>,
    lease_until_ms: Option<i64>,
    last_error: Option<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
}

fn load_outbox_message(connection: &Connection, message_id: &str) -> Result<Option<OutboxMessage>> {
    let raw = connection
        .query_row(
            "SELECT message_id, protocol_epoch, router_id, operation, payload_json,
                    dedupe_key, status, attempt_count, next_attempt_at_ms,
                    lease_owner, lease_until_ms, last_error, created_at_ms, updated_at_ms
             FROM control_outbox WHERE message_id = ?1",
            params![message_id],
            |row| {
                Ok(RawOutboxMessage {
                    message_id: row.get(0)?,
                    protocol_epoch: row.get(1)?,
                    router_id: row.get(2)?,
                    operation: row.get(3)?,
                    payload_json: row.get(4)?,
                    dedupe_key: row.get(5)?,
                    status: row.get(6)?,
                    attempt_count: row.get(7)?,
                    next_attempt_at_ms: row.get(8)?,
                    lease_owner: row.get(9)?,
                    lease_until_ms: row.get(10)?,
                    last_error: row.get(11)?,
                    created_at_ms: row.get(12)?,
                    updated_at_ms: row.get(13)?,
                })
            },
        )
        .optional()?;
    raw.map(|raw| {
        Ok(OutboxMessage {
            message_id: raw.message_id,
            protocol_epoch: raw.protocol_epoch,
            router_id: raw.router_id,
            operation: raw.operation,
            payload: serde_json::from_str(&raw.payload_json)
                .map_err(RouterControlStoreError::DecodePayload)?,
            dedupe_key: raw.dedupe_key,
            status: OutboxStatus::parse(&raw.status)?,
            attempt_count: raw.attempt_count,
            next_attempt_at_ms: raw.next_attempt_at_ms,
            lease_owner: raw.lease_owner,
            lease_until_ms: raw.lease_until_ms,
            last_error: raw.last_error,
            created_at_ms: raw.created_at_ms,
            updated_at_ms: raw.updated_at_ms,
        })
    })
    .transpose()
}

fn classify_outbox_lease_failure(connection: &Connection, message_id: &str) -> Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM control_outbox WHERE message_id = ?1)",
        params![message_id],
        |row| row.get(0),
    )?;
    if exists {
        Err(RouterControlStoreError::OutboxLeaseMismatch(
            message_id.to_string(),
        ))
    } else {
        Err(RouterControlStoreError::OutboxNotFound(
            message_id.to_string(),
        ))
    }
}

fn load_sync_cursor(
    connection: &Connection,
    router_id: &str,
    stream: &str,
) -> Result<Option<SyncCursor>> {
    connection
        .query_row(
            "SELECT protocol_epoch, router_id, stream, position, sequence, updated_at_ms
             FROM sync_cursors WHERE router_id = ?1 AND stream = ?2",
            params![router_id, stream],
            |row| {
                Ok(SyncCursor {
                    protocol_epoch: row.get(0)?,
                    router_id: row.get(1)?,
                    stream: row.get(2)?,
                    position: row.get(3)?,
                    sequence: row.get(4)?,
                    updated_at_ms: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(RouterControlStoreError::from)
}

fn load_host_claim(connection: &Connection, host: &PublicHost) -> Result<Option<StoredHostClaim>> {
    connection
        .query_row(
            "SELECT protocol_epoch, router_id, host, kind, subject_id,
                    client_key, slug, created_at_ms
             FROM public_hosts WHERE host = ?1",
            params![host.as_str()],
            host_claim_from_row,
        )
        .optional()
        .map_err(RouterControlStoreError::from)
}

fn host_claim_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredHostClaim> {
    let host: String = row.get(2)?;
    let kind: String = row.get(3)?;
    let client_key: Option<String> = row.get(5)?;
    Ok(StoredHostClaim {
        protocol_epoch: row.get(0)?,
        router_id: row.get(1)?,
        host: PublicHost::parse(&host).map_err(|error| conversion_error(2, error))?,
        kind: PublicHostKind::parse(&kind).map_err(|error| conversion_error(3, error))?,
        subject_id: row.get(4)?,
        client_key: client_key
            .map(|value| ClientKey::parse(&value))
            .transpose()
            .map_err(|error| conversion_error(5, error))?,
        slug: row.get(6)?,
        created_at_ms: row.get(7)?,
    })
}

fn conversion_error(
    column: usize,
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(error))
}

fn validate_router_id(value: &str) -> Result<String> {
    let value = validate_opaque_id("Router id", value, 128)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(RouterControlStoreError::InvalidRouterProfile(
            "Router id contains unsupported characters".to_string(),
        ));
    }
    Ok(value)
}

fn normalize_api_base(value: &str, base_domain: &BaseDomain) -> Result<String> {
    let mut url = reqwest::Url::parse(value).map_err(|error| {
        RouterControlStoreError::InvalidRouterProfile(format!("invalid API base URL: {error}"))
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.host_str() != Some(base_domain.as_str())
    {
        return Err(RouterControlStoreError::InvalidRouterProfile(
            "API base must be an http(s) Router origin matching base_domain".to_string(),
        ));
    }
    url.set_path("");
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn validate_operation(value: &str) -> Result<String> {
    let value = validate_opaque_id("operation", value, 128)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(RouterControlStoreError::InvalidOutbox(
            "operation must use lowercase letters, digits, and underscores".to_string(),
        ));
    }
    Ok(value)
}

fn validate_opaque_id(label: &str, value: &str, max_len: usize) -> Result<String> {
    if value.trim() != value
        || value.is_empty()
        || value.len() > max_len
        || value.chars().any(char::is_control)
    {
        return Err(RouterControlStoreError::InvalidRouterProfile(format!(
            "{label} is invalid"
        )));
    }
    Ok(value.to_string())
}

fn validate_secret(value: &str) -> Result<String> {
    if value.trim() != value
        || value.len() < 16
        || value.len() > 4_096
        || value.chars().any(char::is_control)
    {
        return Err(RouterControlStoreError::InvalidRouterProfile(
            "Router control secret is invalid".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn validate_error_message(value: &str) -> Result<String> {
    if value.trim().is_empty() {
        return Err(RouterControlStoreError::InvalidOutbox(
            "outbox error must not be empty".to_string(),
        ));
    }
    Ok(value.chars().take(2_048).collect())
}

fn validate_timestamp(value: i64) -> Result<()> {
    if value < 0 {
        return Err(RouterControlStoreError::InvalidRouterProfile(
            "timestamp must be non-negative".to_string(),
        ));
    }
    Ok(())
}

fn random_id(prefix: &str) -> String {
    let mut bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{prefix}-{}", hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;
    use crate::domain::router::{MarketSlug, ShareSlug};

    fn primary(router_id: &str, domain: &str) -> NewRouterProfile {
        NewRouterProfile {
            router_id: router_id.to_string(),
            api_base: format!("https://{domain}"),
            base_domain: BaseDomain::parse(domain).unwrap(),
            role: RouterRole::Primary,
        }
    }

    fn auxiliary(router_id: &str, domain: &str) -> NewRouterProfile {
        NewRouterProfile {
            role: RouterRole::Auxiliary,
            ..primary(router_id, domain)
        }
    }

    #[test]
    fn global_device_identity_is_stable_and_private_key_is_redacted() {
        let store = RouterControlStore::open_in_memory().unwrap();
        let first = store.load_or_create_device_identity("edge", 100).unwrap();
        let second = store.load_or_create_device_identity("edge", 200).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.protocol_epoch, PROTOCOL_EPOCH);
        assert!(!format!("{first:?}").contains(&first.private_key));
        assert!(matches!(
            store.load_or_create_device_identity("other", 300),
            Err(RouterControlStoreError::ClientPrefixMismatch { .. })
        ));
    }

    #[test]
    fn exactly_one_primary_is_enforced_and_switch_is_atomic() {
        let store = RouterControlStore::open_in_memory().unwrap();
        assert!(matches!(
            store.insert_router_profile(auxiliary("r1", "one.example.com"), 1),
            Err(RouterControlStoreError::FirstRouterMustBePrimary)
        ));
        store
            .insert_router_profile(primary("r1", "one.example.com"), 1)
            .unwrap();
        store
            .insert_router_profile(auxiliary("r2", "two.example.com"), 2)
            .unwrap();
        assert!(matches!(
            store.insert_router_profile(primary("r3", "three.example.com"), 3),
            Err(RouterControlStoreError::PrimaryRouterExists(_))
        ));

        let before = store.primary_state().unwrap();
        let after = store.set_primary_router("r2", 4).unwrap();
        assert_eq!(before.router_id.as_deref(), Some("r1"));
        assert_eq!(after.router_id.as_deref(), Some("r2"));
        assert_eq!(after.controller_epoch, before.controller_epoch + 1);
        assert_eq!(
            store.router_profile("r1").unwrap().unwrap().role,
            RouterRole::Auxiliary
        );
        assert_eq!(
            store.router_profile("r2").unwrap().unwrap().role,
            RouterRole::Primary
        );
        assert!(matches!(
            store.remove_router_profile("r2"),
            Err(RouterControlStoreError::CannotRemovePrimary)
        ));
    }

    #[test]
    fn installation_credentials_are_scoped_per_router() {
        let store = RouterControlStore::open_in_memory().unwrap();
        store
            .insert_router_profile(primary("r1", "one.example.com"), 1)
            .unwrap();
        store
            .insert_router_profile(auxiliary("r2", "two.example.com"), 2)
            .unwrap();
        let profile = store
            .set_router_installation("r1", "installation-1", "secret-secret-secret", 3)
            .unwrap();
        assert_eq!(profile.installation_id.as_deref(), Some("installation-1"));
        assert_eq!(
            profile.control_secret.as_deref(),
            Some("secret-secret-secret")
        );
        assert!(matches!(
            store.set_router_installation("r2", "installation-1", "another-secret-secret", 4),
            Err(RouterControlStoreError::InstallationConflict(_))
        ));
    }

    #[test]
    fn outbox_deduplicates_claims_recovers_expired_leases_and_acks() {
        let store = RouterControlStore::open_in_memory().unwrap();
        store
            .insert_router_profile(primary("r1", "one.example.com"), 1)
            .unwrap();
        let message = NewOutboxMessage {
            router_id: "r1".to_string(),
            operation: "share_upsert".to_string(),
            payload: json!({"shareId": "share-1", "revision": 2}),
            dedupe_key: Some("share-1:2".to_string()),
            available_at_ms: 10,
        };
        let first = store.enqueue_outbox(message.clone(), 2).unwrap();
        let duplicate = store.enqueue_outbox(message, 3).unwrap();
        assert!(first.inserted);
        assert!(!duplicate.inserted);
        assert_eq!(first.message_id, duplicate.message_id);

        assert!(store
            .claim_due_outbox("r1", "worker-a", 9, 10, 10)
            .unwrap()
            .is_empty());
        let claimed = store
            .claim_due_outbox("r1", "worker-a", 10, 10, 10)
            .unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].attempt_count, 1);
        assert!(store
            .claim_due_outbox("r1", "worker-b", 19, 10, 10)
            .unwrap()
            .is_empty());
        let reclaimed = store
            .claim_due_outbox("r1", "worker-b", 20, 10, 10)
            .unwrap();
        assert_eq!(reclaimed[0].attempt_count, 2);
        assert!(matches!(
            store.acknowledge_outbox(&first.message_id, "worker-a"),
            Err(RouterControlStoreError::OutboxLeaseMismatch(_))
        ));
        store
            .acknowledge_outbox(&first.message_id, "worker-b")
            .unwrap();
        assert!(store.outbox_message(&first.message_id).unwrap().is_none());
    }

    #[test]
    fn cursor_update_requires_monotonic_compare_and_set() {
        let store = RouterControlStore::open_in_memory().unwrap();
        store
            .insert_router_profile(primary("r1", "one.example.com"), 1)
            .unwrap();
        let first = store
            .compare_and_set_sync_cursor("r1", "shares", None, 4, "cursor-4", 2)
            .unwrap();
        assert_eq!(first.sequence, 4);
        assert!(matches!(
            store.compare_and_set_sync_cursor("r1", "shares", Some(3), 5, "cursor-5", 3),
            Err(RouterControlStoreError::CursorConflict {
                current: Some(4),
                ..
            })
        ));
        assert!(matches!(
            store.compare_and_set_sync_cursor("r1", "shares", Some(4), 4, "cursor-4b", 3),
            Err(RouterControlStoreError::InvalidCursor(_))
        ));
        let next = store
            .compare_and_set_sync_cursor("r1", "shares", Some(4), 5, "cursor-5", 3)
            .unwrap();
        assert_eq!(next.position, "cursor-5");
    }

    #[test]
    fn exact_public_host_claims_use_global_client_key_and_typed_grammar() {
        let store = RouterControlStore::open_in_memory().unwrap();
        let identity = store.load_or_create_device_identity("edge", 1).unwrap();
        let base = BaseDomain::parse("one.example.com").unwrap();
        store
            .insert_router_profile(primary("r1", base.as_str()), 2)
            .unwrap();

        let client = PublicHostClaim::client(&base, identity.client_key.clone(), "device").unwrap();
        let share = PublicHostClaim::share(
            &base,
            ShareSlug::parse("shared").unwrap(),
            identity.client_key.clone(),
            "share-1",
        )
        .unwrap();
        let market =
            PublicHostClaim::market(&base, MarketSlug::parse("official").unwrap(), "market-1")
                .unwrap();
        assert!(store.claim_public_host("r1", &client, 3).unwrap());
        assert!(!store.claim_public_host("r1", &client, 4).unwrap());
        assert!(store.claim_public_host("r1", &share, 5).unwrap());
        assert!(store.claim_public_host("r1", &market, 6).unwrap());
        assert_eq!(store.public_hosts("r1").unwrap().len(), 3);
        let resolved = store
            .resolve_public_host(&share.host().to_string().to_ascii_uppercase())
            .unwrap()
            .unwrap();
        assert_eq!(resolved.kind, PublicHostKind::Share);
        assert_eq!(resolved.subject_id, "share-1");
    }

    #[test]
    fn database_rejects_a_different_protocol_epoch_without_fallback() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "cc-switch-router-control-{}-{suffix}.sqlite",
            std::process::id()
        ));
        {
            let store = RouterControlStore::open_path(&path).unwrap();
            drop(store);
        }
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "PRAGMA ignore_check_constraints = ON;
                     UPDATE schema_meta SET value = 'legacy' WHERE key = 'protocol_epoch';",
                )
                .unwrap();
        }
        assert!(matches!(
            RouterControlStore::open_path(&path),
            Err(RouterControlStoreError::ProtocolEpochMismatch { .. })
        ));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
    }
}
