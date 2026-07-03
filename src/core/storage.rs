use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Context;
use rand::RngCore;
use serde::Serialize;

pub fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let mut content = serde_json::to_vec_pretty(value).context("serialize json")?;
    content.push(b'\n');
    write_bytes_atomic(path, &content)
}

pub fn write_bytes_atomic(path: &Path, content: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    let tmp_path = temp_path(path);
    {
        let mut file = fs::File::create(&tmp_path)
            .with_context(|| format!("create temp json {}", tmp_path.display()))?;
        file.write_all(content)
            .with_context(|| format!("write temp json {}", tmp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("sync temp json {}", tmp_path.display()))?;
    }
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "replace json {} with {}",
            path.display(),
            tmp_path.display()
        )
    })?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)
            .with_context(|| format!("open dir {} for sync", parent.display()))?
            .sync_all()
            .with_context(|| format!("sync dir {}", parent.display()))?;
    }
    Ok(())
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
}
