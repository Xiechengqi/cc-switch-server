pub(crate) fn deepseek_hash_v1(data: &[u8]) -> [u8; 32] {
    crate::clients::deepseek::pow::deepseek_hash_v1(data)
}

#[cfg(target_os = "linux")]
mod codex_server_probe;
