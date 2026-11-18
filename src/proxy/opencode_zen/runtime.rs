//! Fixed, checksum-verified fetch runtime. Provisioning is single-flight and
//! independent of inference; neither credentials nor prompts are written here.
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::proxy::ProxyError;

const VERSION: &str = "1.4.4";
const MAX_ARCHIVE: usize = 64 * 1024 * 1024;
static VERIFIED: Mutex<BTreeSet<PathBuf>> = Mutex::const_new(BTreeSet::new());

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Artifact {
    asset: String,
    archive_sha256: String,
    binary_sha256: String,
    binary_bytes: u64,
}

#[derive(Deserialize)]
struct Manifest {
    artifacts: Vec<Artifact>,
}

fn artifact() -> Result<Artifact, ProxyError> {
    if !cfg!(target_os = "linux") {
        return Err(runtime_error("unsupported_platform"));
    }
    let (arch, loader) = match std::env::consts::ARCH {
        "x86_64" => ("x64", "/lib/ld-musl-x86_64.so.1"),
        "aarch64" => ("aarch64", "/lib/ld-musl-aarch64.so.1"),
        _ => return Err(runtime_error("unsupported_architecture")),
    };
    let musl = if Path::new(loader).exists() {
        "-musl"
    } else {
        ""
    };
    let baseline = if arch == "x64" { "-baseline" } else { "" };
    let asset = format!("bun-linux-{arch}{musl}{baseline}");
    let manifest: Manifest = serde_json::from_str(include_str!(
        "../../../assets/contract/opencode-zen-runtime.json"
    ))
    .map_err(|_| runtime_error("invalid_manifest"))?;
    manifest
        .artifacts
        .into_iter()
        .find(|row| row.asset == asset)
        .ok_or_else(|| runtime_error("unsupported_platform"))
}

pub(crate) async fn ensure(
    config_dir: &Path,
    deadline: tokio::time::Instant,
) -> Result<PathBuf, ProxyError> {
    tokio::time::timeout_at(deadline, ensure_inner(config_dir)).await.map_err(|_| ProxyError {
        status: http::StatusCode::GATEWAY_TIMEOUT,
        message: "OpenCode Zen fetch runtime preparation deadline exceeded; check download connectivity and retry".into(),
    })?
}

async fn ensure_inner(config_dir: &Path) -> Result<PathBuf, ProxyError> {
    let artifact = artifact()?;
    let override_path = std::env::var_os("CC_SWITCH_ZEN_BUN_PATH")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let root = config_dir
        .join("runtime/opencode-zen")
        .join(format!("bun-{VERSION}"))
        .join(&artifact.asset);
    let executable = override_path.clone().unwrap_or_else(|| root.join("bun"));
    let mut verified = VERIFIED.lock().await;
    if verified.contains(&executable) && executable.is_file() {
        return Ok(executable);
    }
    let path = executable.clone();
    let row = artifact.clone();
    if tokio::task::spawn_blocking(move || verify_binary(&path, &row))
        .await
        .unwrap_or(false)
    {
        verified.insert(executable.clone());
        return Ok(executable);
    }
    if override_path.is_some() {
        return Err(runtime_error("configured_binary_checksum_mismatch"));
    }
    std::fs::create_dir_all(&root).map_err(|_| runtime_error("runtime_directory_unavailable"))?;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| runtime_error("runtime_directory_unavailable"))?;
    let archive = TemporaryFile(root.join(format!(
        "download-{}-{}.zip",
        std::process::id(),
        rand::random::<u64>()
    )));
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&archive.0)
        .map_err(|_| runtime_error("runtime_directory_unavailable"))?;
    let mut file = tokio::fs::File::from_std(file);
    let client = crate::infra::http::outbound_client_builder()
        .map_err(|_| runtime_error("download_proxy_invalid"))?
        .redirect(crate::infra::http::self_update_redirect_policy())
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|_| runtime_error("download_client_unavailable"))?;
    let url = format!(
        "https://github.com/oven-sh/bun/releases/download/bun-v{VERSION}/{}.zip",
        artifact.asset
    );
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| runtime_error("download_unavailable"))?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|n| n > MAX_ARCHIVE as u64)
    {
        return Err(runtime_error("download_rejected"));
    }
    let mut stream = response.bytes_stream();
    let mut hash = Sha256::new();
    let mut size = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| runtime_error("download_interrupted"))?;
        if chunk.len() > MAX_ARCHIVE.saturating_sub(size) {
            return Err(runtime_error("download_too_large"));
        }
        size += chunk.len();
        hash.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|_| runtime_error("runtime_directory_unavailable"))?;
    }
    file.sync_all()
        .await
        .map_err(|_| runtime_error("runtime_directory_unavailable"))?;
    drop(file);
    if hex::encode(hash.finalize()) != artifact.archive_sha256 {
        return Err(runtime_error("archive_checksum_mismatch"));
    }
    let target = executable.clone();
    tokio::task::spawn_blocking(move || install_archive(archive, &target, &artifact))
        .await
        .map_err(|_| runtime_error("extraction_failed"))??;
    if verified.len() >= 16 {
        verified.clear();
    }
    verified.insert(executable.clone());
    Ok(executable)
}

fn verify_binary(path: &Path, artifact: &Artifact) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file()
        || metadata.len() != artifact.binary_bytes
        || metadata.permissions().mode() & 0o111 == 0
    {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hash.update(&buffer[..n]),
            Err(_) => return false,
        }
    }
    hex::encode(hash.finalize()) == artifact.binary_sha256
}

fn install_archive(
    archive: TemporaryFile,
    target: &Path,
    artifact: &Artifact,
) -> Result<(), ProxyError> {
    let file = std::fs::File::open(&archive.0).map_err(|_| runtime_error("extraction_failed"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| runtime_error("invalid_archive"))?;
    let mut entry = zip
        .by_name(&format!("{}/bun", artifact.asset))
        .map_err(|_| runtime_error("missing_binary"))?;
    if entry.size() != artifact.binary_bytes {
        return Err(runtime_error("binary_size_mismatch"));
    }
    let staged = TemporaryFile(target.with_extension(format!("stage-{}", rand::random::<u64>())));
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o700)
        .open(&staged.0)
        .map_err(|_| runtime_error("runtime_directory_unavailable"))?;
    let copied = std::io::copy(
        &mut entry.by_ref().take(artifact.binary_bytes + 1),
        &mut output,
    )
    .map_err(|_| runtime_error("extraction_failed"))?;
    if copied != artifact.binary_bytes {
        return Err(runtime_error("binary_size_mismatch"));
    }
    output
        .flush()
        .and_then(|_| output.sync_all())
        .map_err(|_| runtime_error("runtime_directory_unavailable"))?;
    drop(output);
    if !verify_binary(&staged.0, artifact) {
        return Err(runtime_error("binary_checksum_mismatch"));
    }
    std::fs::rename(&staged.0, target)
        .map_err(|_| runtime_error("runtime_directory_unavailable"))?;
    Ok(())
}

struct TemporaryFile(PathBuf);
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn runtime_error(code: &str) -> ProxyError {
    ProxyError::bad_gateway(format!("OpenCode Zen fetch runtime {VERSION}: {code}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zen_runtime_archive_is_verified_before_atomic_install() {
        let root = std::env::temp_dir().join(format!("zen-runtime-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&root).unwrap();
        let data = b"verified-test-runtime";
        let mut row = Artifact {
            asset: "fixture".into(),
            archive_sha256: String::new(),
            binary_sha256: hex::encode(Sha256::digest(data)),
            binary_bytes: data.len() as u64,
        };
        let archive = root.join("test.zip");
        let make_archive = || {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
            zip.start_file("fixture/bun", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
            zip.finish().unwrap();
        };
        make_archive();
        let executable = root.join("bun");
        install_archive(TemporaryFile(archive.clone()), &executable, &row).unwrap();
        assert!(verify_binary(&executable, &row));
        assert!(!archive.exists());
        make_archive();
        row.binary_sha256 = "0".repeat(64);
        assert!(install_archive(TemporaryFile(archive.clone()), &executable, &row).is_err());
        assert_eq!(std::fs::read(&executable).unwrap(), data);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
