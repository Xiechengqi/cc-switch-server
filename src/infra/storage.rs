use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Context;
use fs2::FileExt;
use rand::RngCore;
use serde::Serialize;

const DATA_DIRECTORY_LOCK_FILE: &str = ".cc-switch-server.lock";

#[derive(Debug)]
pub struct DataDirectoryLock {
    file: fs::File,
}

impl Drop for DataDirectoryLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub fn acquire_data_directory_lock(config_dir: &Path) -> anyhow::Result<DataDirectoryLock> {
    fs::create_dir_all(config_dir)
        .with_context(|| format!("create config dir {}", config_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(config_dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod 0700 {}", config_dir.display()))?;
    }
    let path = config_dir.join(DATA_DIRECTORY_LOCK_FILE);
    let mut options = fs::OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .with_context(|| format!("open data directory lock {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", path.display()))?;
    }
    file.try_lock_exclusive().with_context(|| {
        format!(
            "data directory {} is already in use; stop cc-switch-server before running offline migration commands",
            config_dir.display()
        )
    })?;
    Ok(DataDirectoryLock { file })
}

pub fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    write_json_pretty_with_hook(path, value, |_| Ok(()))
}

pub(crate) fn write_json_pretty_with_hook<T: Serialize>(
    path: &Path,
    value: &T,
    mut before_stage: impl FnMut(AtomicWriteStage) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    before_stage(AtomicWriteStage::Serialize)?;
    let mut content = serde_json::to_vec_pretty(value).context("serialize json")?;
    content.push(b'\n');
    write_bytes_atomic_with_hook(path, &content, before_stage)
}

pub fn write_bytes_atomic(path: &Path, content: &[u8]) -> anyhow::Result<()> {
    write_bytes_atomic_with_hook(path, content, |_| Ok(()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicWriteStage {
    Serialize,
    CreateDirectory,
    CreateTemp,
    WriteTemp,
    SyncTemp,
    Rename,
    SyncDirectory,
}

pub(crate) fn write_bytes_atomic_with_hook(
    path: &Path,
    content: &[u8],
    mut before_stage: impl FnMut(AtomicWriteStage) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    before_stage(AtomicWriteStage::CreateDirectory)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    let tmp_path = temp_path(path);
    let result = (|| {
        before_stage(AtomicWriteStage::CreateTemp)?;
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp_path)
            .with_context(|| format!("create temp json {}", tmp_path.display()))?;
        before_stage(AtomicWriteStage::WriteTemp)?;
        file.write_all(content)
            .with_context(|| format!("write temp json {}", tmp_path.display()))?;
        before_stage(AtomicWriteStage::SyncTemp)?;
        file.sync_all()
            .with_context(|| format!("sync temp json {}", tmp_path.display()))?;
        before_stage(AtomicWriteStage::Rename)?;
        fs::rename(&tmp_path, path).with_context(|| {
            format!(
                "replace json {} with {}",
                path.display(),
                tmp_path.display()
            )
        })?;
        before_stage(AtomicWriteStage::SyncDirectory)?;
        if let Some(parent) = path.parent() {
            fs::File::open(parent)
                .with_context(|| format!("open dir {} for sync", parent.display()))?
                .sync_all()
                .with_context(|| format!("sync dir {}", parent.display()))?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn temp_path(path: &Path) -> std::path::PathBuf {
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("store.json");
    path.with_file_name(format!(".{file_name}.{suffix}.tmp"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    #[test]
    fn concurrent_json_writes_leave_valid_json() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-storage-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = Arc::new(dir.join("store.json"));
        let mut handles = Vec::new();
        for index in 0..16 {
            let path = path.clone();
            handles.push(thread::spawn(move || {
                write_json_pretty(&path, &json!({"index": index, "items": [1, 2, 3]})).unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let content = fs::read_to_string(path.as_ref()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        fs::remove_dir_all(&dir).unwrap();

        assert!(value
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .is_some());
    }

    #[test]
    fn atomic_write_failure_before_rename_preserves_destination() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-storage-failure-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.json");
        fs::write(&path, b"old").unwrap();

        let error = write_bytes_atomic_with_hook(&path, b"new", |stage| {
            if stage == AtomicWriteStage::Rename {
                anyhow::bail!("injected rename failure");
            }
            Ok(())
        })
        .unwrap_err();

        assert!(error.to_string().contains("injected rename failure"));
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn every_atomic_write_stage_is_observable_and_leaves_no_temp_file() {
        let stages = [
            AtomicWriteStage::Serialize,
            AtomicWriteStage::CreateDirectory,
            AtomicWriteStage::CreateTemp,
            AtomicWriteStage::WriteTemp,
            AtomicWriteStage::SyncTemp,
            AtomicWriteStage::Rename,
            AtomicWriteStage::SyncDirectory,
        ];
        for injected in stages {
            let dir = std::env::temp_dir().join(format!(
                "cc-switch-server-storage-stage-test-{injected:?}-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join("store.json");
            fs::write(&path, b"old").unwrap();

            let error = write_json_pretty_with_hook(&path, &json!({"next": true}), |stage| {
                if stage == injected {
                    anyhow::bail!("injected {stage:?}");
                }
                Ok(())
            })
            .unwrap_err();

            assert!(error.to_string().contains("injected"));
            let expected = if injected == AtomicWriteStage::SyncDirectory {
                serde_json::to_vec_pretty(&json!({"next": true})).unwrap()
            } else {
                b"old".to_vec()
            };
            let actual = fs::read(&path).unwrap();
            if injected == AtomicWriteStage::SyncDirectory {
                assert_eq!(actual.strip_suffix(b"\n"), Some(expected.as_slice()));
            } else {
                assert_eq!(actual, expected);
            }
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
            fs::remove_dir_all(dir).unwrap();
        }
    }
}
