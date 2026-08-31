use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::num::Wrapping;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepSeekPowChallenge {
    pub algorithm: String,
    pub challenge: String,
    pub salt: String,
    pub expire_at: i64,
    #[serde(default)]
    pub difficulty: i64,
    #[serde(default)]
    pub expire_after: Option<i64>,
    pub signature: String,
    pub target_path: String,
}

const DEFAULT_DIFFICULTY: i64 = 144_000;
const MAX_DIFFICULTY: i64 = 1_000_000;
const MAX_CLOCK_SKEW_SECS: i64 = 30;
const MAX_FUTURE_EXPIRY_SECS: i64 = 15 * 60;
const COMPLETION_TARGET_PATH: &str = "/api/v0/chat/completion";

pub fn deepseek_hash_v1(data: &[u8]) -> [u8; 32] {
    const RATE: usize = 136;
    let mut state = [0_u64; 25];
    let mut off = 0;
    while off + RATE <= data.len() {
        absorb_block(&mut state, &data[off..off + RATE]);
        keccak_f23(&mut state);
        off += RATE;
    }
    let mut final_block = [0_u8; RATE];
    final_block[..data.len() - off].copy_from_slice(&data[off..]);
    final_block[data.len() - off] = 0x06;
    final_block[RATE - 1] |= 0x80;
    absorb_block(&mut state, &final_block);
    keccak_f23(&mut state);

    let mut out = [0_u8; 32];
    out[0..8].copy_from_slice(&state[0].to_le_bytes());
    out[8..16].copy_from_slice(&state[1].to_le_bytes());
    out[16..24].copy_from_slice(&state[2].to_le_bytes());
    out[24..32].copy_from_slice(&state[3].to_le_bytes());
    out
}

pub async fn solve_and_build_header(challenge: &DeepSeekPowChallenge) -> anyhow::Result<String> {
    let now_secs = chrono::Utc::now().timestamp();
    solve_and_build_header_at(challenge, now_secs).await
}

async fn solve_and_build_header_at(
    challenge: &DeepSeekPowChallenge,
    now_secs: i64,
) -> anyhow::Result<String> {
    let difficulty = validate_challenge(challenge, now_secs)?;
    if challenge.algorithm != "DeepSeekHashV1" {
        anyhow::bail!(
            "unsupported DeepSeek PoW algorithm: {}",
            challenge.algorithm
        );
    }
    let challenge = challenge.clone();
    tokio::task::spawn_blocking(move || {
        let answer = solve_pow(
            &challenge.challenge,
            &challenge.salt,
            challenge.expire_at,
            difficulty,
        )?;
        build_pow_header(&challenge, answer)
    })
    .await?
}

fn validate_challenge(challenge: &DeepSeekPowChallenge, now_secs: i64) -> anyhow::Result<i64> {
    if challenge.algorithm != "DeepSeekHashV1" {
        anyhow::bail!(
            "unsupported DeepSeek PoW algorithm: {}",
            challenge.algorithm
        );
    }
    if challenge.target_path != COMPLETION_TARGET_PATH {
        anyhow::bail!(
            "DeepSeek PoW target path must be {COMPLETION_TARGET_PATH}, got {}",
            challenge.target_path
        );
    }
    if challenge.challenge.len() != 64
        || !challenge
            .challenge
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        anyhow::bail!("DeepSeek PoW challenge must be exactly 64 hexadecimal characters");
    }
    if challenge.salt.is_empty() || challenge.salt.len() > 512 {
        anyhow::bail!("DeepSeek PoW salt is empty or exceeds 512 bytes");
    }
    if challenge.signature.is_empty() || challenge.signature.len() > 16 * 1024 {
        anyhow::bail!("DeepSeek PoW signature is empty or exceeds 16 KiB");
    }
    let expiry_secs = normalize_expiry_secs(challenge.expire_at)?;
    if expiry_secs < now_secs.saturating_sub(MAX_CLOCK_SKEW_SECS) {
        anyhow::bail!("DeepSeek PoW challenge has expired");
    }
    if expiry_secs > now_secs.saturating_add(MAX_FUTURE_EXPIRY_SECS) {
        anyhow::bail!("DeepSeek PoW expiry exceeds the 15 minute horizon");
    }
    if challenge
        .expire_after
        .is_some_and(|seconds| !(1..=MAX_FUTURE_EXPIRY_SECS).contains(&seconds))
    {
        anyhow::bail!("DeepSeek PoW expire_after is outside the reviewed bounds");
    }
    let difficulty = if challenge.difficulty == 0 {
        DEFAULT_DIFFICULTY
    } else {
        challenge.difficulty
    };
    if !(1..=MAX_DIFFICULTY).contains(&difficulty) {
        anyhow::bail!("DeepSeek PoW difficulty is outside the reviewed bounds");
    }
    Ok(difficulty)
}

fn normalize_expiry_secs(value: i64) -> anyhow::Result<i64> {
    if value <= 0 {
        anyhow::bail!("DeepSeek PoW expiry must be positive");
    }
    Ok(if value > 10_000_000_000 {
        value / 1_000
    } else {
        value
    })
}

fn solve_pow(
    challenge_hex: &str,
    salt: &str,
    expire_at: i64,
    difficulty: i64,
) -> anyhow::Result<i64> {
    if challenge_hex.len() != 64 {
        anyhow::bail!("challenge must be 64 hex chars");
    }
    let target = hex::decode(challenge_hex)?;
    let target_words = [
        u64::from_le_bytes(target[0..8].try_into()?),
        u64::from_le_bytes(target[8..16].try_into()?),
        u64::from_le_bytes(target[16..24].try_into()?),
        u64::from_le_bytes(target[24..32].try_into()?),
    ];
    if !(1..=MAX_DIFFICULTY).contains(&difficulty) {
        anyhow::bail!("PoW difficulty is outside the reviewed bounds");
    }
    let limit = difficulty;
    let prefix = format!("{salt}_{expire_at}_");
    for answer in 0..limit {
        let digest = deepseek_hash_v1(format!("{prefix}{answer}").as_bytes());
        let words = [
            u64::from_le_bytes(digest[0..8].try_into()?),
            u64::from_le_bytes(digest[8..16].try_into()?),
            u64::from_le_bytes(digest[16..24].try_into()?),
            u64::from_le_bytes(digest[24..32].try_into()?),
        ];
        if words == target_words {
            return Ok(answer);
        }
    }
    anyhow::bail!("no PoW solution within difficulty")
}

fn build_pow_header(challenge: &DeepSeekPowChallenge, answer: i64) -> anyhow::Result<String> {
    let payload = serde_json::json!({
        "algorithm": challenge.algorithm,
        "challenge": challenge.challenge,
        "salt": challenge.salt,
        "answer": answer,
        "signature": challenge.signature,
        "target_path": challenge.target_path,
    });
    Ok(base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&payload)?))
}

fn absorb_block(state: &mut [u64; 25], block: &[u8]) {
    for (i, chunk) in block.chunks_exact(8).take(17).enumerate() {
        state[i] ^= u64::from_le_bytes(chunk.try_into().expect("8-byte chunk"));
    }
}

fn rotl64(v: u64, k: u32) -> u64 {
    v.rotate_left(k)
}

#[rustfmt::skip]
const RC: [u64; 24] = [
    0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000,
    0x000000000000808B, 0x0000000080000001, 0x8000000080008081, 0x8000000000008009,
    0x000000000000008A, 0x0000000000000088, 0x0000000080008009, 0x000000008000000A,
    0x000000008000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003,
    0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
    0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008,
];

#[allow(clippy::many_single_char_names)]
fn keccak_f23(s: &mut [u64; 25]) {
    let mut a0 = Wrapping(s[0]);
    let mut a1 = Wrapping(s[1]);
    let mut a2 = Wrapping(s[2]);
    let mut a3 = Wrapping(s[3]);
    let mut a4 = Wrapping(s[4]);
    let mut a5 = Wrapping(s[5]);
    let mut a6 = Wrapping(s[6]);
    let mut a7 = Wrapping(s[7]);
    let mut a8 = Wrapping(s[8]);
    let mut a9 = Wrapping(s[9]);
    let mut a10 = Wrapping(s[10]);
    let mut a11 = Wrapping(s[11]);
    let mut a12 = Wrapping(s[12]);
    let mut a13 = Wrapping(s[13]);
    let mut a14 = Wrapping(s[14]);
    let mut a15 = Wrapping(s[15]);
    let mut a16 = Wrapping(s[16]);
    let mut a17 = Wrapping(s[17]);
    let mut a18 = Wrapping(s[18]);
    let mut a19 = Wrapping(s[19]);
    let mut a20 = Wrapping(s[20]);
    let mut a21 = Wrapping(s[21]);
    let mut a22 = Wrapping(s[22]);
    let mut a23 = Wrapping(s[23]);
    let mut a24 = Wrapping(s[24]);

    for &rc in RC.iter().skip(1) {
        let c0 = a0 ^ a5 ^ a10 ^ a15 ^ a20;
        let c1 = a1 ^ a6 ^ a11 ^ a16 ^ a21;
        let c2 = a2 ^ a7 ^ a12 ^ a17 ^ a22;
        let c3 = a3 ^ a8 ^ a13 ^ a18 ^ a23;
        let c4 = a4 ^ a9 ^ a14 ^ a19 ^ a24;
        let d0 = c4 ^ Wrapping(rotl64(c1.0, 1));
        let d1 = c0 ^ Wrapping(rotl64(c2.0, 1));
        let d2 = c1 ^ Wrapping(rotl64(c3.0, 1));
        let d3 = c2 ^ Wrapping(rotl64(c4.0, 1));
        let d4 = c3 ^ Wrapping(rotl64(c0.0, 1));

        a0 ^= d0;
        a5 ^= d0;
        a10 ^= d0;
        a15 ^= d0;
        a20 ^= d0;
        a1 ^= d1;
        a6 ^= d1;
        a11 ^= d1;
        a16 ^= d1;
        a21 ^= d1;
        a2 ^= d2;
        a7 ^= d2;
        a12 ^= d2;
        a17 ^= d2;
        a22 ^= d2;
        a3 ^= d3;
        a8 ^= d3;
        a13 ^= d3;
        a18 ^= d3;
        a23 ^= d3;
        a4 ^= d4;
        a9 ^= d4;
        a14 ^= d4;
        a19 ^= d4;
        a24 ^= d4;

        let b0 = a0;
        let b10 = Wrapping(rotl64(a1.0, 1));
        let b20 = Wrapping(rotl64(a2.0, 62));
        let b5 = Wrapping(rotl64(a3.0, 28));
        let b15 = Wrapping(rotl64(a4.0, 27));
        let b16 = Wrapping(rotl64(a5.0, 36));
        let b1 = Wrapping(rotl64(a6.0, 44));
        let b11 = Wrapping(rotl64(a7.0, 6));
        let b21 = Wrapping(rotl64(a8.0, 55));
        let b6 = Wrapping(rotl64(a9.0, 20));
        let b7 = Wrapping(rotl64(a10.0, 3));
        let b17 = Wrapping(rotl64(a11.0, 10));
        let b2 = Wrapping(rotl64(a12.0, 43));
        let b12 = Wrapping(rotl64(a13.0, 25));
        let b22 = Wrapping(rotl64(a14.0, 39));
        let b23 = Wrapping(rotl64(a15.0, 41));
        let b8 = Wrapping(rotl64(a16.0, 45));
        let b18 = Wrapping(rotl64(a17.0, 15));
        let b3 = Wrapping(rotl64(a18.0, 21));
        let b13 = Wrapping(rotl64(a19.0, 8));
        let b14 = Wrapping(rotl64(a20.0, 18));
        let b24 = Wrapping(rotl64(a21.0, 2));
        let b9 = Wrapping(rotl64(a22.0, 61));
        let b19 = Wrapping(rotl64(a23.0, 56));
        let b4 = Wrapping(rotl64(a24.0, 14));

        a0 = b0 ^ ((!b1) & b2);
        a1 = b1 ^ ((!b2) & b3);
        a2 = b2 ^ ((!b3) & b4);
        a3 = b3 ^ ((!b4) & b0);
        a4 = b4 ^ ((!b0) & b1);
        a5 = b5 ^ ((!b6) & b7);
        a6 = b6 ^ ((!b7) & b8);
        a7 = b7 ^ ((!b8) & b9);
        a8 = b8 ^ ((!b9) & b5);
        a9 = b9 ^ ((!b5) & b6);
        a10 = b10 ^ ((!b11) & b12);
        a11 = b11 ^ ((!b12) & b13);
        a12 = b12 ^ ((!b13) & b14);
        a13 = b13 ^ ((!b14) & b10);
        a14 = b14 ^ ((!b10) & b11);
        a15 = b15 ^ ((!b16) & b17);
        a16 = b16 ^ ((!b17) & b18);
        a17 = b17 ^ ((!b18) & b19);
        a18 = b18 ^ ((!b19) & b15);
        a19 = b19 ^ ((!b15) & b16);
        a20 = b20 ^ ((!b21) & b22);
        a21 = b21 ^ ((!b22) & b23);
        a22 = b22 ^ ((!b23) & b24);
        a23 = b23 ^ ((!b24) & b20);
        a24 = b24 ^ ((!b20) & b21);

        a0 ^= Wrapping(rc);
    }
    s[0] = a0.0;
    s[1] = a1.0;
    s[2] = a2.0;
    s[3] = a3.0;
    s[4] = a4.0;
    s[5] = a5.0;
    s[6] = a6.0;
    s[7] = a7.0;
    s[8] = a8.0;
    s[9] = a9.0;
    s[10] = a10.0;
    s[11] = a11.0;
    s[12] = a12.0;
    s[13] = a13.0;
    s[14] = a14.0;
    s[15] = a15.0;
    s[16] = a16.0;
    s[17] = a17.0;
    s[18] = a18.0;
    s[19] = a19.0;
    s[20] = a20.0;
    s[21] = a21.0;
    s[22] = a22.0;
    s[23] = a23.0;
    s[24] = a24.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge(now_secs: i64, answer: i64) -> DeepSeekPowChallenge {
        let salt = "strict-pow";
        let expire_at = now_secs + 60;
        let digest = deepseek_hash_v1(format!("{salt}_{expire_at}_{answer}").as_bytes());
        DeepSeekPowChallenge {
            algorithm: "DeepSeekHashV1".to_string(),
            challenge: hex::encode(digest),
            salt: salt.to_string(),
            expire_at,
            difficulty: answer + 1,
            expire_after: Some(60),
            signature: "fixture-signature".to_string(),
            target_path: COMPLETION_TARGET_PATH.to_string(),
        }
    }

    #[tokio::test]
    async fn pow_accepts_exact_current_completion_challenge() {
        let encoded = solve_and_build_header_at(&challenge(1_700_000_000, 42), 1_700_000_000)
            .await
            .unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(value["answer"], 42);
        assert_eq!(value["target_path"], COMPLETION_TARGET_PATH);
    }

    #[tokio::test]
    async fn pow_rejects_expiry_target_algorithm_and_difficulty_drift() {
        let now = 1_700_000_000;
        let mut expired = challenge(now, 1);
        expired.expire_at = now - 31;
        assert!(solve_and_build_header_at(&expired, now).await.is_err());

        let mut future = challenge(now, 1);
        future.expire_at = now + MAX_FUTURE_EXPIRY_SECS + 1;
        assert!(solve_and_build_header_at(&future, now).await.is_err());

        let mut target = challenge(now, 1);
        target.target_path = "/api/v0/other".to_string();
        assert!(solve_and_build_header_at(&target, now).await.is_err());

        let mut algorithm = challenge(now, 1);
        algorithm.algorithm = "FutureHash".to_string();
        assert!(solve_and_build_header_at(&algorithm, now).await.is_err());

        let mut difficulty = challenge(now, 1);
        difficulty.difficulty = MAX_DIFFICULTY + 1;
        assert!(solve_and_build_header_at(&difficulty, now).await.is_err());
    }
}
