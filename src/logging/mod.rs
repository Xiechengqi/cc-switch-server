mod audit;
mod capture;
mod init;

pub use audit::{
    classify_network_error, error_fingerprint, opaque_ref, AuditBatch, AuditCursor, AuditEvent,
    AuditLog, AuditRequestDetails, AuditUploadCursor, AuditWriteError, SharedAuditLog,
    AUDIT_UPLOAD_BATCH_LIMIT,
};
pub use capture::{
    LogCapture, LogTailAccessError, LogTailResponse, LogTailSource, SharedLogCapture,
};
pub use init::{init_tracing, reload_log_level};

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::domain::settings::ui_settings::{self, LOG_API_MAX_TAIL_LINES};

pub const RING_BUFFER_CAPACITY: usize = 5_000;
const LOG_TAIL_FILE_SCAN_MAX_BYTES: u64 = 512 * 1024;
const LOG_DIRECTORY: &str = "log";
const PERSISTENT_LOG_FILENAME: &str = "cc-switch-server.log";
const PROCESS_LOG_FILENAME: &str = "server.log";
const RESTART_HELPER_LOG_FILENAME: &str = "restart-helper.log";

pub(crate) fn log_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(LOG_DIRECTORY)
}

pub(crate) fn persistent_log_path(config_dir: &Path) -> PathBuf {
    log_dir(config_dir).join(PERSISTENT_LOG_FILENAME)
}

pub(crate) fn process_log_path(config_dir: &Path) -> PathBuf {
    log_dir(config_dir).join(PROCESS_LOG_FILENAME)
}

pub(crate) fn restart_helper_log_path(config_dir: &Path) -> PathBuf {
    log_dir(config_dir).join(RESTART_HELPER_LOG_FILENAME)
}

pub(crate) fn ensure_log_dir(config_dir: &Path) -> std::io::Result<PathBuf> {
    let path = log_dir(config_dir);
    fs::create_dir_all(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}

pub(crate) fn open_log_append(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub fn clamp_tail_lines(requested: Option<usize>, configured_default: usize) -> usize {
    let configured = configured_default.clamp(1, LOG_API_MAX_TAIL_LINES);
    let requested = requested.unwrap_or(configured);
    requested.clamp(1, configured.min(LOG_API_MAX_TAIL_LINES))
}

pub fn tail_file_lines(path: &Path, lines: usize) -> std::io::Result<Vec<String>> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let start = file_len.saturating_sub(LOG_TAIL_FILE_SCAN_MAX_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((file_len - start) as usize);
    file.read_to_end(&mut bytes)?;
    if start > 0 {
        if let Some(first_newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=first_newline);
        } else {
            bytes.clear();
        }
    }
    let content = String::from_utf8_lossy(&bytes);
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
    mut file: Vec<String>,
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

    let overlap = (1..=file.len().min(buffer.len()))
        .rev()
        .find(|overlap| file[file.len() - overlap..] == buffer[..*overlap])
        .unwrap_or(0);
    file.extend(buffer.into_iter().skip(overlap));
    let merged = file;
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
        "capability",
        "callback_url",
        "callbackurl",
        "auth_callback_url",
        "cookie",
        "password",
        "secret",
    ];
    let input = mask_kiro_api_keys(input);
    let redacted = input
        .lines()
        .map(|line| redact_sensitive_line(line, KEYS))
        .collect::<Vec<_>>()
        .join("\n");
    mask_email_addresses(&redacted)
}

fn mask_email_addresses(input: &str) -> String {
    fn local_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'%' | b'+' | b'-')
    }

    fn domain_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')
    }

    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut copied_through = 0;
    let mut search_from = 0;
    while let Some(relative_at) = input[search_from..].find('@') {
        let at = search_from + relative_at;
        let mut start = at;
        while start > 0 && local_byte(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = at + 1;
        while end < bytes.len() && domain_byte(bytes[end]) {
            end += 1;
        }
        let domain = &input[at + 1..end];
        let valid = start < at
            && end > at + 1
            && domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.');
        if valid {
            output.push_str(&input[copied_through..start]);
            output.push_str("[REDACTED_EMAIL]");
            copied_through = end;
            search_from = end;
        } else {
            search_from = at + 1;
        }
    }
    output.push_str(&input[copied_through..]);
    output
}

pub fn redact_sensitive_text_with_values<'a>(
    input: &str,
    sensitive_values: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut redacted = redact_sensitive_text(input);
    for sensitive_value in sensitive_values
        .into_iter()
        .filter(|value| !value.is_empty())
    {
        if redacted == sensitive_value {
            redacted = "[REDACTED]".to_string();
        } else if sensitive_value.len() >= 8 && redacted.contains(sensitive_value) {
            redacted = redacted.replace(sensitive_value, "[REDACTED]");
        }
    }
    redacted
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
        output.push_str("[REDACTED_KIRO_API_KEY]");
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

    fn test_config_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "cc-switch-server-log-path-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn application_logs_resolve_under_config_log_directory() {
        let config_dir = Path::new("/srv/cc-switch-server-data");
        assert_eq!(log_dir(config_dir), config_dir.join("log"));
        assert_eq!(
            persistent_log_path(config_dir),
            config_dir.join("log/cc-switch-server.log")
        );
        assert_eq!(
            process_log_path(config_dir),
            config_dir.join("log/server.log")
        );
        assert_eq!(
            restart_helper_log_path(config_dir),
            config_dir.join("log/restart-helper.log")
        );
    }

    #[cfg(unix)]
    #[test]
    fn log_directory_and_new_log_files_are_private() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let config_dir = test_config_dir("permissions");
        let directory = ensure_log_dir(&config_dir).unwrap();
        let path = persistent_log_path(&config_dir);
        let mut file = open_log_append(&path).unwrap();
        writeln!(file, "private log").unwrap();
        drop(file);

        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
            0
        );
        std::fs::remove_dir_all(config_dir).unwrap();
    }

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
    fn merge_tail_lines_deduplicates_file_buffer_overlap() {
        let buffer = vec!["two".into(), "three".into()];
        let file = vec!["one".into(), "two".into(), "three".into()];
        let (merged, source) = merge_tail_lines(buffer, file, 10);
        assert_eq!(merged, vec!["one", "two", "three"]);
        assert_eq!(source, LogTailSource::BufferAndFile);
    }

    #[test]
    fn redacts_common_secret_fields() {
        let redacted = redact_sensitive_text(
            "authorization: Bearer abc\nnormal line\napi_key=xyz\nowner=user@example.com",
        );
        assert_eq!(
            redacted,
            "authorization: Bearer [REDACTED]\nnormal line\napi_key= [REDACTED]\nowner=[REDACTED_EMAIL]"
        );
        assert!(!redacted.contains("abc"));
        assert!(!redacted.contains("xyz"));
        assert!(!redacted.contains("user@example.com"));
        assert_eq!(
            redact_sensitive_text("token router connected"),
            "token router connected"
        );
        let callback = redact_sensitive_text(
            "auth_callback_url=http://localhost/callback?flowId=f&capability=callback-secret",
        );
        assert!(!callback.contains("callback-secret"));
        assert!(callback.contains("[REDACTED]"));
    }

    #[test]
    fn masks_kiro_api_keys_without_hiding_surrounding_error() {
        assert_eq!(
            mask_kiro_api_keys("upstream rejected ksk_abcdefghijklmnop; retry denied"),
            "upstream rejected [REDACTED_KIRO_API_KEY]; retry denied"
        );
        assert_eq!(
            redact_sensitive_text("error: invalid ksk_1234567890"),
            "error: invalid [REDACTED_KIRO_API_KEY]"
        );
    }

    #[test]
    fn redacts_known_sensitive_values_reflected_without_a_field_name() {
        let redacted = redact_sensitive_text_with_values(
            "upstream rejected refresh-secret-value; retry denied",
            ["refresh-secret-value"],
        );
        assert_eq!(redacted, "upstream rejected [REDACTED]; retry denied");
    }
}
