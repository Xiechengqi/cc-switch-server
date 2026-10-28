use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use bytes::Bytes;
use fs2::FileExt;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::infra::resident_memory::{
    process_resident_memory, ResidentMemoryBudget, ResidentMemoryReservation,
};

const IMAGE_STORE_DIRECTORY: &str = "image-capabilities";
const IMAGE_STORE_ENTRIES_DIRECTORY: &str = "entries";
const IMAGE_STORE_LOCK_FILE: &str = ".lock";
const IMAGE_STORE_SCHEMA_VERSION: u32 = 1;
const IMAGE_CAPABILITY_TTL_MS: i64 = 60 * 60 * 1000;
const IMAGE_CAPABILITY_MAX_ENTRIES: usize = 128;
const IMAGE_CAPABILITY_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const IMAGE_HEAD_VERIFY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageCapabilityRead {
    Body,
    Head,
}

#[derive(Debug, thiserror::Error)]
#[error("image capability memory capacity exhausted")]
pub(crate) struct ImageCapabilityCapacityError;

#[derive(Debug, Clone)]
pub(crate) struct ImageCapability {
    pub(crate) data: Bytes,
    pub(crate) mime_type: String,
    pub(crate) byte_length: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct ImageCapabilityHandle {
    pub(crate) token: String,
}

#[derive(Debug)]
pub(crate) struct ImageCapabilityStore {
    root: PathBuf,
    parent: ResidentMemoryBudget,
}

struct ImagePayload {
    data: Vec<u8>,
    _memory: ResidentMemoryReservation,
    _capacity_memory: Option<ResidentMemoryReservation>,
}

impl AsRef<[u8]> for ImagePayload {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
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
        let root =
            resolve_image_store_dir(config_dir, std::env::var_os("CC_SWITCH_IMAGE_STORE_DIR"));
        Self::at(root)
    }

    fn at(root: PathBuf) -> anyhow::Result<Self> {
        Self::with_parent(root, process_resident_memory())
    }

    fn with_parent(root: PathBuf, parent: ResidentMemoryBudget) -> anyhow::Result<Self> {
        create_private_directory(&root)?;
        create_private_directory(&root.join(IMAGE_STORE_ENTRIES_DIRECTORY))?;
        let store = Self { root, parent };
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

    pub(crate) fn get(
        &self,
        token: &str,
        mode: ImageCapabilityRead,
    ) -> anyhow::Result<Option<ImageCapability>> {
        self.get_at_mode(token, now_ms(), mode)
    }

    #[cfg(test)]
    fn get_at(&self, token: &str, now_ms: i64) -> anyhow::Result<Option<ImageCapability>> {
        self.get_at_mode(token, now_ms, ImageCapabilityRead::Body)
    }

    fn get_at_mode(
        &self,
        token: &str,
        now_ms: i64,
        mode: ImageCapabilityRead,
    ) -> anyhow::Result<Option<ImageCapability>> {
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

        let data = self.read_verified_payload(token, &metadata, mode)?;
        let Some(data) = data else {
            self.remove_pair_locked(token)?;
            snapshot.entries.remove(token);
            snapshot.total_bytes = snapshot.total_bytes.saturating_sub(metadata.byte_length);
            set_store_size_metrics(&snapshot);
            crate::metrics::record_image_capability_event("corrupt");
            lock.unlock()?;
            return Ok(None);
        };
        set_store_size_metrics(&snapshot);
        crate::metrics::record_image_capability_event("hit");
        lock.unlock()?;
        Ok(Some(ImageCapability {
            data,
            mime_type: metadata.mime_type,
            byte_length: metadata.byte_length,
        }))
    }

    /// Validate both GET and HEAD under the cross-process lock. File length
    /// never controls an unbounded allocation, including a racing file growth.
    fn read_verified_payload(
        &self,
        token: &str,
        metadata: &ImageCapabilityMetadata,
        mode: ImageCapabilityRead,
    ) -> anyhow::Result<Option<Bytes>> {
        let data_path = self.data_path(token);
        let mut file = match fs::File::open(&data_path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("open image capability payload"),
        };
        if file.metadata()?.len() != metadata.byte_length {
            return Ok(None);
        }
        let length = usize::try_from(metadata.byte_length)?;
        let buffer_length = match mode {
            ImageCapabilityRead::Body => length,
            ImageCapabilityRead::Head => length.min(IMAGE_HEAD_VERIFY_BUFFER_BYTES),
        };
        let memory = self
            .parent
            .reserve(buffer_length.saturating_add(std::mem::size_of::<ImagePayload>()))
            .ok_or(ImageCapabilityCapacityError)?;
        let mut data = Vec::new();
        data.try_reserve_exact(buffer_length)
            .map_err(|_| ImageCapabilityCapacityError)?;
        let capacity_memory = (data.capacity() > buffer_length)
            .then(|| {
                self.parent
                    .reserve(data.capacity() - buffer_length)
                    .ok_or(ImageCapabilityCapacityError)
            })
            .transpose()?;
        data.resize(buffer_length, 0);
        let mut hasher = Sha256::new();
        let mut remaining = length;
        while remaining > 0 {
            let chunk_length = remaining.min(buffer_length);
            let target = match mode {
                ImageCapabilityRead::Body => &mut data[length - remaining..][..chunk_length],
                ImageCapabilityRead::Head => &mut data[..chunk_length],
            };
            if let Err(error) = file.read_exact(target) {
                if error.kind() == std::io::ErrorKind::UnexpectedEof {
                    return Ok(None);
                }
                return Err(error).context("read image capability payload");
            }
            hasher.update(target);
            remaining -= chunk_length;
        }
        // Probe one extra byte without growing the buffer, even if another
        // process ignored the store lock and appended after the length check.
        if file.read(&mut [0u8; 1])? != 0 || hex::encode(hasher.finalize()) != metadata.sha256 {
            return Ok(None);
        }
        if mode == ImageCapabilityRead::Head {
            return Ok(Some(Bytes::new()));
        }
        Ok(Some(Bytes::from_owner(ImagePayload {
            data,
            _memory: memory,
            _capacity_memory: capacity_memory,
        })))
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

fn resolve_image_store_dir(config_dir: &Path, configured: Option<OsString>) -> PathBuf {
    configured
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                config_dir.join(path)
            }
        })
        .unwrap_or_else(|| config_dir.join(IMAGE_STORE_DIRECTORY))
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

    #[test]
    fn image_store_defaults_to_the_config_directory() {
        let config_dir = Path::new("/srv/cc-switch-server-data");
        assert_eq!(
            resolve_image_store_dir(config_dir, None),
            config_dir.join("image-capabilities")
        );
        assert_eq!(
            resolve_image_store_dir(config_dir, Some(OsString::new())),
            config_dir.join("image-capabilities")
        );
        assert_eq!(
            resolve_image_store_dir(config_dir, Some(OsString::from("shared-images"))),
            config_dir.join("shared-images")
        );
        assert_eq!(
            resolve_image_store_dir(
                config_dir,
                Some(OsString::from("/mnt/cc-switch-image-capabilities")),
            ),
            PathBuf::from("/mnt/cc-switch-image-capabilities")
        );
    }

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

    #[test]
    fn codex_r4_image_download_owner_follows_last_bytes_view() {
        let (root, _) = test_store("resident-owner");
        let parent = ResidentMemoryBudget::new(None);
        let store = ImageCapabilityStore::with_parent(root.clone(), parent.clone()).unwrap();
        let handle = store
            .insert_at(Bytes::from(vec![7; 1024]), "image/png".into(), 10)
            .unwrap();
        let image = store.get_at(&handle.token, 11).unwrap().unwrap();
        let charged = 1024 + std::mem::size_of::<ImagePayload>();
        assert_eq!(parent.snapshot().used_bytes, charged);
        let clone = image.data.clone();
        let slice = image.data.slice(100..101);
        drop(image);
        drop(store);
        assert_eq!(parent.snapshot().used_bytes, charged);
        drop(clone);
        assert_eq!(slice.as_ref(), &[7]);
        assert_eq!(parent.snapshot().used_bytes, charged);
        drop(slice);
        assert_eq!(parent.snapshot().used_bytes, 0);
        assert_eq!(parent.snapshot().active_reservations, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_r4_image_head_verifies_with_bounded_memory_and_capacity_is_not_corruption() {
        let (root, _) = test_store("resident-head");
        let parent = ResidentMemoryBudget::new(Some(
            IMAGE_HEAD_VERIFY_BUFFER_BYTES + std::mem::size_of::<ImagePayload>(),
        ));
        let store = ImageCapabilityStore::with_parent(root.clone(), parent.clone()).unwrap();
        let length = IMAGE_HEAD_VERIFY_BUFFER_BYTES * 3;
        let handle = store
            .insert_at(Bytes::from(vec![5; length]), "image/png".into(), 10)
            .unwrap();
        let error = store.get_at(&handle.token, 11).unwrap_err();
        assert!(error.is::<ImageCapabilityCapacityError>());
        assert!(store.data_path(&handle.token).exists());
        assert_eq!(parent.snapshot().used_bytes, 0);
        let head = store
            .get_at_mode(&handle.token, 12, ImageCapabilityRead::Head)
            .unwrap()
            .unwrap();
        assert_eq!(head.byte_length, length as u64);
        assert!(head.data.is_empty());
        assert_eq!(parent.snapshot().used_bytes, 0);
        assert_eq!(
            parent.snapshot().high_water_bytes,
            IMAGE_HEAD_VERIFY_BUFFER_BYTES + std::mem::size_of::<ImagePayload>()
        );

        let mut corrupt = vec![5; length];
        corrupt[length - 1] = 6;
        fs::write(store.data_path(&handle.token), corrupt).unwrap();
        assert!(store
            .get_at_mode(&handle.token, 13, ImageCapabilityRead::Head)
            .unwrap()
            .is_none());
        assert_eq!(parent.snapshot().used_bytes, 0);
        assert!(!store.data_path(&handle.token).exists());

        let handle = store
            .insert_at(Bytes::from_static(b"small"), "image/png".into(), 20)
            .unwrap();
        fs::write(store.data_path(&handle.token), vec![0; length]).unwrap();
        parent.configure_limit(Some(0));
        let rejections = parent.snapshot().rejections;
        assert!(store.get_at(&handle.token, 21).unwrap().is_none());
        assert_eq!(parent.snapshot().rejections, rejections);
        assert_eq!(parent.snapshot().used_bytes, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn codex_r4_abandoned_blocking_image_read_releases_completed_payload() {
        let (root, _) = test_store("resident-cancel");
        let parent = ResidentMemoryBudget::new(None);
        let store = ImageCapabilityStore::with_parent(root.clone(), parent.clone()).unwrap();
        let handle = store
            .insert_at(Bytes::from_static(b"image"), "image/png".into(), 10)
            .unwrap();
        let (ready, started) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let task = tokio::task::spawn_blocking(move || {
            let result = store.get_at(&handle.token, 11);
            ready.send(()).unwrap();
            wait.recv().unwrap();
            result
        });
        started.await.unwrap();
        assert!(parent.snapshot().used_bytes > 0);
        task.abort();
        drop(task);
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while parent.snapshot().used_bytes != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(parent.snapshot().active_reservations, 0);
        fs::remove_dir_all(root).unwrap();
    }
}
