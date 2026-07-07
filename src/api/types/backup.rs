use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct CreateBackupRequest {
    #[serde(default)]
    pub(in crate::api) reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct BackupListResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) backups: Vec<crate::infra::backup::BackupManifest>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct BackupCreateResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) backup: crate::infra::backup::BackupManifest,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct BackupRestoreResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) result: crate::infra::backup::BackupRestoreResult,
}
