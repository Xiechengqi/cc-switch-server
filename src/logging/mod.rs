mod capture;
mod init;

pub use capture::{
    LogCapture, LogTailAccessError, LogTailResponse, LogTailSource, SharedLogCapture,
};
pub use init::{init_tracing, reload_log_level};

use std::path::{Path, PathBuf};

use crate::domain::settings::ui_settings::{self, LOG_API_MAX_TAIL_LINES};
use crate::self_update::version::SERVICE_LOG_PATH;

pub const RING_BUFFER_CAPACITY: usize = 5_000;

pub fn resolve_log_file_path(config_dir: &Path) -> PathBuf {
    let service_log = Path::new(SERVICE_LOG_PATH);
    if service_log.is_file() {
        return service_log.to_path_buf();
    }
    if let Some(parent) = service_log.parent() {
        if parent.exists() || std::fs::create_dir_all(parent).is_ok() {
            return service_log.to_path_buf();
        }
    }
    config_dir.join("cc-switch-server.log")
}

pub fn clamp_tail_lines(requested: Option<usize>, configured_default: usize) -> usize {
    let configured = configured_default.clamp(1, LOG_API_MAX_TAIL_LINES);
    let requested = requested.unwrap_or(configured);
    requested.clamp(1, configured.min(LOG_API_MAX_TAIL_LINES))
}

pub fn tail_file_lines(path: &Path, lines: usize) -> std::io::Result<Vec<String>> {
    let content = std::fs::read_to_string(path)?;
    let mut collected: Vec<String> = content
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if collected.len() > lines {
        collected = collected.split_off(collected.len().saturating_sub(lines));
    }
    Ok(collected)
}

pub fn merge_tail_lines(
    buffer: Vec<String>,
    file: Vec<String>,
    lines: usize,
) -> (Vec<String>, LogTailSource) {
    if buffer.is_empty() {
        let start = file.len().saturating_sub(lines);
        return (file[start..].to_vec(), LogTailSource::File);
    }
    if file.is_empty() {
        let start = buffer.len().saturating_sub(lines);
        return (buffer[start..].to_vec(), LogTailSource::Buffer);
    }

    let mut merged = file;
    merged.extend(buffer);
    let start = merged.len().saturating_sub(lines);
    (merged[start..].to_vec(), LogTailSource::BufferAndFile)
}

pub fn parsed_log_config_from_store(
    store: &ui_settings::UiSettingsStore,
) -> ui_settings::ParsedLogConfig {
    ui_settings::parse_log_config(&ui_settings::log_config_for_frontend(store))
}

pub fn redact_sensitive_text(input: &str) -> String {
    const KEYS: &[&str] = &[
        "authorization",
        "bearer",
        "api_key",
        "apikey",
        "api-key",
        "token",
        "access_token",
        "refresh_token",
        "cookie",
        "password",
        "secret",
    ];
    let input = mask_kiro_api_keys(input);
    input
        .lines()
        .map(|line| redact_sensitive_line(line, KEYS))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn mask_kiro_api_keys(input: &str) -> String {
    const PREFIX: &str = "ksk_";
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(relative_start) = input[cursor..].find(PREFIX) {
        let start = cursor + relative_start;
        output.push_str(&input[cursor..start]);
        let mut end = start + PREFIX.len();
        while end < input.len() {
            let byte = input.as_bytes()[end];
            if !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')) {
                break;
            }
            end += 1;
        }
        let token = &input[start..end];
        if token.len() > 8 {
            output.push_str(&token[..4]);
            output.push_str("...");
            output.push_str(&token[token.len() - 4..]);
        } else {
            output.push_str("[REDACTED_KIRO_API_KEY]");
        }
        cursor = end;
    }
    output.push_str(&input[cursor..]);
    output
}

fn redact_sensitive_line(line: &str, keys: &[&str]) -> String {
    let lower = line.to_ascii_lowercase();
    if let Some(start) = lower.find("bearer ") {
        return format!("{}Bearer [REDACTED]", &line[..start]);
    }
    for key in keys.iter().filter(|key| **key != "bearer") {
        let Some(start) = lower.find(key) else {
            continue;
        };
        let after_key = start + key.len();
        let suffix = &line[after_key..];
        // Only treat a key name as a secret field when its assignment marker
        // immediately follows it. Ordinary product terms such as "token
        // router" remain useful in diagnostics.
        let Some(relative_separator) = suffix
            .char_indices()
            .find(|(index, ch)| *index <= 3 && matches!(ch, ':' | '='))
            .map(|(index, _)| index)
        else {
            continue;
        };
        let end = after_key + relative_separator + 1;
        return format!("{} [REDACTED]", line[..end].trim_end());
    }
    line.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_tail_lines_respects_bounds() {
        assert_eq!(clamp_tail_lines(None, 100), 100);
        assert_eq!(clamp_tail_lines(Some(10), 100), 10);
        assert_eq!(clamp_tail_lines(Some(10_000), 100), 100);
        assert_eq!(clamp_tail_lines(Some(0), 100), 1);
    }

    #[test]
    fn merge_tail_lines_combines_file_and_buffer() {
        let buffer = vec!["b1".into(), "b2".into()];
        let file = vec!["f1".into()];
        let (merged, source) = merge_tail_lines(buffer, file, 3);
        assert_eq!(merged, vec!["f1", "b1", "b2"]);
        assert_eq!(source, LogTailSource::BufferAndFile);
    }

    #[test]
    fn redacts_common_secret_fields() {
        let redacted = redact_sensitive_text("authorization: Bearer abc\nnormal line\napi_key=xyz");
        assert_eq!(
            redacted,
            "authorization: Bearer [REDACTED]\nnormal line\napi_key= [REDACTED]"
        );
        assert!(!redacted.contains("abc"));
        assert!(!redacted.contains("xyz"));
        assert_eq!(
            redact_sensitive_text("token router connected"),
            "token router connected"
        );
    }

    #[test]
    fn masks_kiro_api_keys_without_hiding_surrounding_error() {
        assert_eq!(
            mask_kiro_api_keys("upstream rejected ksk_abcdefghijklmnop; retry denied"),
            "upstream rejected ksk_...mnop; retry denied"
        );
        assert_eq!(
            redact_sensitive_text("error: invalid ksk_1234567890"),
            "error: invalid ksk_...7890"
        );
    }
}
