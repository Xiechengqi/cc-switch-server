use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hkdf::Hkdf;
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const ROOT_KEY_FILE_NAME: &str = "accounts.key";
pub const ROOT_KEY_ENV: &str = "CC_SWITCH_SERVER_ACCOUNTS_ENCRYPTION_KEY";
const PROVIDER_KEY_INFO: &[u8] = b"cc-switch-server/provider-credentials/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKeySource {
    Environment,
    File,
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ResolvedCredentialKey {
    pub key: [u8; 32],
    #[zeroize(skip)]
    pub source: CredentialKeySource,
}

impl std::fmt::Debug for ResolvedCredentialKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedCredentialKey")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

pub fn root_key_path(config_dir: &Path) -> PathBuf {
    config_dir.join(ROOT_KEY_FILE_NAME)
}

pub fn load_root_key(config_dir: &Path) -> anyhow::Result<ResolvedCredentialKey> {
    load_root_key_if_present(config_dir)?.ok_or_else(|| {
        anyhow::anyhow!(
            "credential encryption key is required: {}",
            root_key_path(config_dir).display()
        )
    })
}

pub fn load_or_create_root_key(config_dir: &Path) -> anyhow::Result<ResolvedCredentialKey> {
    if let Some(resolved) = load_root_key_if_present(config_dir)? {
        return Ok(resolved);
    }

    fs::create_dir_all(config_dir)
        .with_context(|| format!("create config dir {}", config_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(config_dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("secure config dir {}", config_dir.display()))?;
    }

    let path = root_key_path(config_dir);
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    let encoded = URL_SAFE_NO_PAD.encode(key);
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(encoded.as_bytes())
        .with_context(|| format!("write {}", path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync {}", path.display()))?;

    Ok(ResolvedCredentialKey {
        key,
        source: CredentialKeySource::File,
    })
}

pub fn load_root_key_if_present(
    config_dir: &Path,
) -> anyhow::Result<Option<ResolvedCredentialKey>> {
    if let Ok(value) = std::env::var(ROOT_KEY_ENV) {
        return decode_root_key(value.trim())
            .context("decode accounts encryption env key")
            .map(|key| {
                Some(ResolvedCredentialKey {
                    key,
                    source: CredentialKeySource::Environment,
                })
            });
    }

    let path = root_key_path(config_dir);
    if !path.exists() {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", path.display()))?;
    }
    let content = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    decode_root_key(content.trim())
        .with_context(|| format!("decode {}", path.display()))
        .map(|key| {
            Some(ResolvedCredentialKey {
                key,
                source: CredentialKeySource::File,
            })
        })
}

pub fn decode_root_key(value: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(value))
        .context("base64 decode key")?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("accounts encryption key must be 32 bytes"))
}

pub fn derive_provider_key(root_key: &[u8; 32]) -> anyhow::Result<[u8; 32]> {
    let hkdf = Hkdf::<Sha256>::new(Some(b"cc-switch-server/provider-key-salt/v1"), root_key);
    let mut key = [0u8; 32];
    hkdf.expand(PROVIDER_KEY_INFO, &mut key)
        .map_err(|_| anyhow::anyhow!("derive Provider credential key"))?;
    Ok(key)
}

pub fn provider_key_id(provider_key: &[u8; 32]) -> String {
    let digest = Sha256::digest(provider_key);
    format!("provider-v1-{}", hex::encode(&digest[..8]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_key_is_purpose_separated_and_has_stable_id() {
        let root = [7u8; 32];
        let first = derive_provider_key(&root).unwrap();
        let second = derive_provider_key(&root).unwrap();
        assert_eq!(first, second);
        assert_ne!(first, root);
        assert_eq!(provider_key_id(&first), provider_key_id(&second));
    }

    #[test]
    fn decode_accepts_url_safe_and_standard_base64() {
        let key = [9u8; 32];
        assert_eq!(decode_root_key(&URL_SAFE_NO_PAD.encode(key)).unwrap(), key);
        assert_eq!(
            decode_root_key(&base64::engine::general_purpose::STANDARD.encode(key)).unwrap(),
            key
        );
    }
}
