use super::*;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding};

pub(super) fn generate_key_pem() -> Result<Zeroizing<String>, TraeClientError> {
    p256::SecretKey::random(&mut OsRng)
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|_| TraeClientError::protocol("could not create Trae device key"))
}

fn parse_key(pem: &str) -> Result<p256::SecretKey, TraeClientError> {
    if pem.len() > 16 * 1024 {
        return Err(TraeClientError::bad_request(
            "Trae device key exceeds 16 KiB",
        ));
    }
    p256::SecretKey::from_sec1_pem(pem)
        .or_else(|_| p256::SecretKey::from_pkcs8_pem(pem))
        .map_err(|_| TraeClientError::bad_request("Trae device key must be valid P-256 PEM"))
}

pub(super) fn public_key_pem(pem: &str) -> Result<String, TraeClientError> {
    parse_key(pem)?
        .public_key()
        .to_public_key_pem(LineEnding::LF)
        .map_err(|_| TraeClientError::protocol("could not encode Trae device public key"))
}

/// Optional imported device proof, bound to the same stored machine/device.
/// Invalid explicitly configured keys fail closed rather than silently retrying
/// ExchangeToken without the proof. Private material never enters diagnostics.
pub(super) fn device_proof(
    account: &Account,
    uid: &str,
    client_id: &str,
    refresh_token: &str,
    now_ms: i64,
) -> Result<Option<Value>, TraeClientError> {
    // The audited cloudide refresh proof is specific to the Intl contract.
    // A v3 CN device registration does not establish a CN refresh proof format.
    if account.provider_type != ProviderType::TraeIntl {
        return Ok(None);
    }
    let Some(pem) = account
        .raw
        .as_ref()
        .and_then(|raw| raw.get("devicePrivateKeyPem"))
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    let pem = pem
        .as_str()
        .filter(|pem| pem.len() <= 16 * 1024)
        .ok_or_else(|| {
            TraeClientError::bad_request(
                "Trae device private key must be PEM text of at most 16 KiB",
            )
        })?;
    let secret = parse_key(pem)?;
    let signing = SigningKey::from(secret);
    let nonce = random_hex(16);
    let message = format!(
        "POST {} {} {} {} {}",
        TRAE_EXCHANGE_TOKEN_PATH, client_id, refresh_token, now_ms, nonce
    );
    let signature: Signature = signing.sign(message.as_bytes());
    let profile = TraeAccountProfile::parse(account.profile.as_ref())
        .map_err(TraeClientError::bad_request)?;
    if profile.uid != uid {
        return Err(TraeClientError::conflict(
            "Trae device proof account identity changed",
        ));
    }
    Ok(Some(json!({
        "DeviceInfo":{"DeviceId":profile.device_id,"MachineId":profile.machine_id,"DeviceType":"windows","OsVersion":TRAE_OS_VERSION,"DeviceBrand":TRAE_DEVICE_BRAND},
        "DeviceProof":{"Signature":base64::engine::general_purpose::STANDARD.encode(signature.to_der().as_bytes()),"Timestamp":now_ms,"Nonce":nonce}
    })))
}
