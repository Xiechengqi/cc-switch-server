use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use bytes::Bytes;
use fs2::FileExt;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const IMAGE_STORE_DIRECTORY: &str = "image-capabilities";
const IMAGE_STORE_ENTRIES_DIRECTORY: &str = "entries";
const IMAGE_STORE_LOCK_FILE: &str = ".lock";
const IMAGE_STORE_SCHEMA_VERSION: u32 = 1;
const IMAGE_CAPABILITY_TTL_MS: i64 = 60 * 60 * 1000;
const IMAGE_CAPABILITY_MAX_ENTRIES: usize = 128;
const IMAGE_CAPABILITY_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct ImageCapability {
    pub(crate) data: Bytes,
    pub(crate) mime_type: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ImageCapabilityHandle {
    pub(crate) token: String,
}

#[derive(Debug)]
pub(crate) struct ImageCapabilityStore {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageCapabilityMetadata {
    schema_version: u32,
    mime_type: String,
    byte_length: u64,
    sha256: String,
    created_at_ms: i64,
    expires_at_ms: i64,
}

#[derive(Debug, Default)]
struct StoreSnapshot {
    entries: BTreeMap<String, ImageCapabilityMetadata>,
    total_bytes: u64,
    expired: BTreeSet<String>,
    corrupt: BTreeSet<String>,
}

impl ImageCapabilityStore {
    pub(crate) fn from_config_dir(config_dir: &Path) -> anyhow::Result<Self> {
        let root = std::env::var_os("CC_SWITCH_IMAGE_STORE_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    config_dir.join(path)
                }
            })
            .unwrap_or_else(|| config_dir.join(IMAGE_STORE_DIRECTORY));
        Self::at(root)
    }

    fn at(root: PathBuf) -> anyhow::Result<Self> {
        create_private_directory(&root)?;
        create_private_directory(&root.join(IMAGE_STORE_ENTRIES_DIRECTORY))?;
        let store = Self { root };
        let mut lock = store.lock()?;
        let snapshot = store.cleanup_locked(now_ms())?;
        set_store_size_metrics(&snapshot);
        lock.unlock()?;
        Ok(store)
    }

    pub(crate) fn insert(
        &self,
        data: Bytes,
        mime_type: String,
    ) -> anyhow::Result<ImageCapabilityHandle> {
        self.insert_at(data, mime_type, now_ms())
    }

    fn insert_at(
        &self,
        data: Bytes,
        mime_type: String,
        now_ms: i64,
    ) -> anyhow::Result<ImageCapabilityHandle> {
        anyhow::ensure!(
            u64::try_from(data.len()).unwrap_or(u64::MAX) <= IMAGE_CAPABILITY_MAX_TOTAL_BYTES,
            "generated image exceeds the capability store capacity"
        );
        validate_mime_type(&mime_type)?;

        let mut lock = self.lock()?;
        let mut snapshot = self.cleanup_locked(now_ms)?;
        let data_len = u64::try_from(data.len()).context("image length does not fit u64")?;
        while snapshot.entries.len() >= IMAGE_CAPABILITY_MAX_ENTRIES
            || snapshot.total_bytes.saturating_add(data_len) > IMAGE_CAPABILITY_MAX_TOTAL_BYTES
        {
            let Some(oldest) = snapshot
                .entries
                .iter()
                .min_by_key(|(token, metadata)| (metadata.created_at_ms, token.as_str()))
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            if let Some(metadata) = snapshot.entries.remove(&oldest) {
                snapshot.total_bytes = snapshot.total_bytes.saturating_sub(metadata.byte_length);
            }
            self.remove_pair_locked(&oldest)?;
            crate::metrics::record_image_capability_event("evicted");
        }

        let token = loop {
            let mut random = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut random);
            let candidate = hex::encode(random);
            if !self.metadata_path(&candidate).exists() && !self.data_path(&candidate).exists() {
                break candidate;
            }
        };
        let metadata = ImageCapabilityMetadata {
            schema_version: IMAGE_STORE_SCHEMA_VERSION,
            mime_type,
            byte_length: data_len,
            sha256: hex::encode(Sha256::digest(&data)),
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(IMAGE_CAPABILITY_TTL_MS),
        };
        let data_path = self.data_path(&token);
        crate::infra::storage::write_bytes_atomic(&data_path, &data)
            .with_context(|| format!("persist image capability data {}", data_path.display()))?;
        if let Err(error) =
            crate::infra::storage::write_json_pretty(&self.metadata_path(&token), &metadata)
        {
            let _ = fs::remove_file(&data_path);
            return Err(error).context("persist image capability metadata");
        }

        snapshot.total_bytes = snapshot.total_bytes.saturating_add(data_len);
        snapshot.entries.insert(token.clone(), metadata);
        set_store_size_metrics(&snapshot);
        crate::metrics::record_image_capability_event("inserted");
        lock.unlock()?;
        Ok(ImageCapabilityHandle { token })
    }

    pub(crate) fn get(&self, token: &str) -> anyhow::Result<Option<ImageCapability>> {
        self.get_at(token, now_ms())
    }

    fn get_at(&self, token: &str, now_ms: i64) -> anyhow::Result<Option<ImageCapability>> {
        if !valid_token(token) {
            crate::metrics::record_image_capability_event("miss");
            return Ok(None);
        }
        let mut lock = self.lock()?;
        let mut snapshot = self.cleanup_locked(now_ms)?;
        let Some(metadata) = snapshot.entries.get(token).cloned() else {
            if !snapshot.expired.contains(token) && !snapshot.corrupt.contains(token) {
                crate::metrics::record_image_capability_event("miss");
            }
            set_store_size_metrics(&snapshot);
            lock.unlock()?;
            return Ok(None);
        };

        let data_path = self.data_path(token);
        let data = match fs::read(&data_path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read image capability data {}", data_path.display()))
            }
        };
        let valid = u64::try_from(data.len()).ok() == Some(metadata.byte_length)
            && hex::encode(Sha256::digest(&data)) == metadata.sha256;
        if !valid {
            self.remove_pair_locked(token)?;
            snapshot.entries.remove(token);
            snapshot.total_bytes = snapshot.total_bytes.saturating_sub(metadata.byte_length);
            set_store_size_metrics(&snapshot);
            crate::metrics::record_image_capability_event("corrupt");
            lock.unlock()?;
            return Ok(None);
        }
        set_store_size_metrics(&snapshot);
        crate::metrics::record_image_capability_event("hit");
        lock.unlock()?;
        Ok(Some(ImageCapability {
            data: Bytes::from(data),
            mime_type: metadata.mime_type,
        }))
    }

    fn cleanup_locked(&self, now_ms: i64) -> anyhow::Result<StoreSnapshot> {
        let entries_dir = self.entries_dir();
        create_private_directory(&entries_dir)?;
        let mut snapshot = StoreSnapshot::default();
        let mut metadata_tokens = BTreeSet::new();

        for entry in fs::read_dir(&entries_dir)
            .with_context(|| format!("read image capability dir {}", entries_dir.display()))?
        {
            let entry = entry.context("read image capability directory entry")?;
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if file_name.starts_with('.') && file_name.ends_with(".tmp") {
                let _ = fs::remove_file(path);
                continue;
            }
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Some(token) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if !valid_token(token) {
                let _ = fs::remove_file(path);
                crate::metrics::record_image_capability_event("corrupt");
                continue;
            }
            metadata_tokens.insert(token.to_string());
            let metadata = fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<ImageCapabilityMetadata>(&bytes).ok());
            let Some(metadata) = metadata.filter(valid_metadata) else {
                self.remove_pair_locked(token)?;
                snapshot.corrupt.insert(token.to_string());
                continue;
            };
            if metadata.expires_at_ms <= now_ms {
                self.remove_pair_locked(token)?;
                snapshot.expired.insert(token.to_string());
                continue;
            }
            if !self.data_path(token).is_file() {
                self.remove_pair_locked(token)?;
                snapshot.corrupt.insert(token.to_string());
                continue;
            }
            snapshot.total_bytes = snapshot.total_bytes.saturating_add(metadata.byte_length);
            snapshot.entries.insert(token.to_string(), metadata);
        }

        for entry in fs::read_dir(&entries_dir)
            .with_context(|| format!("scan image capability data dir {}", entries_dir.display()))?
        {
            let entry = entry.context("read image capability data entry")?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("bin") {
                continue;
            }
            let Some(token) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if !valid_token(token) || !metadata_tokens.contains(token) {
                fs::remove_file(&path)
                    .with_context(|| format!("remove orphan image data {}", path.display()))?;
                crate::metrics::record_image_capability_event("corrupt");
            }
        }
        for _ in &snapshot.expired {
            crate::metrics::record_image_capability_event("expired");
        }
        for _ in &snapshot.corrupt {
            crate::metrics::record_image_capability_event("corrupt");
        }
        Ok(snapshot)
    }

    fn lock(&self) -> anyhow::Result<ImageStoreLock> {
        let path = self.root.join(IMAGE_STORE_LOCK_FILE);
        let mut options = fs::OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .with_context(|| format!("open image capability lock {}", path.display()))?;
        file.lock_exclusive()
            .with_context(|| format!("lock image capability store {}", self.root.display()))?;
        Ok(ImageStoreLock { file: Some(file) })
    }

    fn remove_pair_locked(&self, token: &str) -> anyhow::Result<()> {
        remove_if_exists(&self.metadata_path(token))?;
        remove_if_exists(&self.data_path(token))?;
        Ok(())
    }

    fn entries_dir(&self) -> PathBuf {
        self.root.join(IMAGE_STORE_ENTRIES_DIRECTORY)
    }

    fn metadata_path(&self, token: &str) -> PathBuf {
        self.entries_dir().join(format!("{token}.json"))
    }

    fn data_path(&self, token: &str) -> PathBuf {
        self.entries_dir().join(format!("{token}.bin"))
    }
}

struct ImageStoreLock {
    file: Option<fs::File>,
}

impl ImageStoreLock {
    fn unlock(&mut self) -> anyhow::Result<()> {
        if let Some(file) = self.file.take() {
            FileExt::unlock(&file).context("unlock image capability store")?;
        }
        Ok(())
    }
}

impl Drop for ImageStoreLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = FileExt::unlock(&file);
        }
    }
}

fn valid_metadata(metadata: &ImageCapabilityMetadata) -> bool {
    metadata.schema_version == IMAGE_STORE_SCHEMA_VERSION
        && metadata.byte_length <= IMAGE_CAPABILITY_MAX_TOTAL_BYTES
        && metadata.expires_at_ms > metadata.created_at_ms
        && metadata.sha256.len() == 64
        && metadata.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        && validate_mime_type(&metadata.mime_type).is_ok()
}

fn validate_mime_type(mime_type: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        matches!(mime_type, "image/png" | "image/jpeg" | "image/webp"),
        "unsupported image capability MIME type"
    );
    Ok(())
}

fn valid_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn remove_if_exists(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn create_private_directory(path: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(path).with_context(|| format!("create dir {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod 0700 {}", path.display()))?;
    }
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn set_store_size_metrics(snapshot: &StoreSnapshot) {
    crate::metrics::set_image_capability_store_size(snapshot.entries.len(), snapshot.total_bytes);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::*;

    fn test_store(name: &str) -> (PathBuf, ImageCapabilityStore) {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-image-store-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = ImageCapabilityStore::at(root.clone()).unwrap();
        (root, store)
    }

    #[test]
    fn capability_survives_store_recreation_and_is_visible_cross_instance() {
        let (root, first) = test_store("restart");
        let inserted_at = now_ms();
        let handle = first
            .insert_at(
                Bytes::from_static(b"\x89PNG\r\n\x1a\nfixture"),
                "image/png".to_string(),
                inserted_at,
            )
            .unwrap();
        let second = ImageCapabilityStore::at(root.clone()).unwrap();
        let image = second
            .get_at(&handle.token, inserted_at + 1)
            .unwrap()
            .unwrap();
        assert_eq!(image.data, Bytes::from_static(b"\x89PNG\r\n\x1a\nfixture"));
        assert_eq!(image.mime_type, "image/png");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capability_expiry_capacity_and_integrity_are_enforced() {
        let (root, store) = test_store("bounds");
        let expiring = store
            .insert_at(Bytes::from_static(b"x"), "image/png".to_string(), 10)
            .unwrap();
        assert!(store
            .get_at(&expiring.token, 10 + IMAGE_CAPABILITY_TTL_MS - 1)
            .unwrap()
            .is_some());
        assert!(store
            .get_at(&expiring.token, 10 + IMAGE_CAPABILITY_TTL_MS)
            .unwrap()
            .is_none());

        let mut handles = Vec::new();
        for index in 0..=IMAGE_CAPABILITY_MAX_ENTRIES {
            handles.push(
                store
                    .insert_at(
                        Bytes::from_static(b"x"),
                        "image/png".to_string(),
                        1_000 + index as i64,
                    )
                    .unwrap(),
            );
        }
        assert!(store.get_at(&handles[0].token, 2_000).unwrap().is_none());
        let newest = handles.last().unwrap();
        assert!(store.get_at(&newest.token, 2_000).unwrap().is_some());
        fs::write(store.data_path(&newest.token), b"corrupt").unwrap();
        assert!(store.get_at(&newest.token, 2_000).unwrap().is_none());
        assert!(!store.metadata_path(&newest.token).exists());
        assert!(!store.data_path(&newest.token).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_instances_serialize_writes_with_the_shared_lock() {
        let (root, _) = test_store("concurrent");
        let root = Arc::new(root);
        let inserted_at = now_ms();
        let mut workers = Vec::new();
        for index in 0..16 {
            let root = Arc::clone(&root);
            workers.push(thread::spawn(move || {
                ImageCapabilityStore::at((*root).clone())
                    .unwrap()
                    .insert_at(
                        Bytes::from(vec![index as u8]),
                        "image/png".to_string(),
                        inserted_at + index,
                    )
                    .unwrap()
            }));
        }
        let handles = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        let reader = ImageCapabilityStore::at((*root).clone()).unwrap();
        for handle in handles {
            assert!(reader
                .get_at(&handle.token, inserted_at + 1_000)
                .unwrap()
                .is_some());
        }
        fs::remove_dir_all(root.as_ref()).unwrap();
    }
}
