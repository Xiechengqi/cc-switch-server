use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::domain::settings::ui_settings::ParsedLogConfig;
use crate::logging::{
    merge_tail_lines, resolve_log_file_path, tail_file_lines, RING_BUFFER_CAPACITY,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTailAccessError {
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LogTailSource {
    Buffer,
    File,
    BufferAndFile,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogTailResponse {
    pub lines: usize,
    pub truncated: bool,
    pub source: LogTailSource,
    pub path: String,
    pub content: String,
}

#[derive(Debug)]
pub struct LogCapture {
    enabled: AtomicBool,
    buffer: Mutex<VecDeque<String>>,
    file_path: Mutex<PathBuf>,
    capacity: usize,
}

impl LogCapture {
    pub fn new(capacity: usize) -> Self {
        Self {
            enabled: AtomicBool::new(true),
            buffer: Mutex::new(VecDeque::with_capacity(capacity.min(RING_BUFFER_CAPACITY))),
            file_path: Mutex::new(PathBuf::new()),
            capacity: capacity.min(RING_BUFFER_CAPACITY),
        }
    }

    pub fn apply_config(&self, config: &ParsedLogConfig, config_dir: &Path) {
        self.enabled.store(config.enabled, Ordering::Relaxed);
        if config.enabled {
            *self.file_path.lock().expect("log file path lock") = resolve_log_file_path(config_dir);
        }
    }

    pub fn push_line(&self, line: String) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        let stored = trimmed.to_string();
        {
            let mut buffer = self.buffer.lock().expect("log buffer lock");
            if buffer.len() >= self.capacity {
                buffer.pop_front();
            }
            buffer.push_back(stored.clone());
        }
        let file_path = self.file_path.lock().expect("log file path lock").clone();
        if file_path.as_os_str().is_empty() {
            return;
        }
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
        {
            let _ = writeln!(file, "{stored}");
        }
    }

    pub fn tail_lines(&self, lines: usize) -> Vec<String> {
        let buffer = self.buffer.lock().expect("log buffer lock");
        let start = buffer.len().saturating_sub(lines);
        buffer.iter().skip(start).cloned().collect()
    }

    pub fn read_tail(
        &self,
        _config: &ParsedLogConfig,
        config_dir: &Path,
        requested_lines: usize,
    ) -> LogTailResponse {
        let path = resolve_log_file_path(config_dir);
        let buffer = self.tail_lines(requested_lines);
        let file = tail_file_lines(&path, requested_lines).unwrap_or_default();
        let total_available = buffer.len() + file.len();
        let (merged, source) = merge_tail_lines(buffer, file, requested_lines);
        LogTailResponse {
            lines: merged.len(),
            truncated: total_available > requested_lines,
            source,
            path: path.display().to_string(),
            content: merged.join("\n"),
        }
    }
}

pub type SharedLogCapture = Arc<LogCapture>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::settings::ui_settings::ParsedLogConfig;

    #[test]
    fn push_line_respects_capacity() {
        let capture = LogCapture::new(2);
        capture.apply_config(
            &ParsedLogConfig {
                enabled: true,
                level: "info".into(),
                api_enabled: true,
                api_tail_lines: 100,
            },
            Path::new("/tmp"),
        );
        capture.push_line("one".into());
        capture.push_line("two".into());
        capture.push_line("three".into());
        let lines = capture.tail_lines(10);
        assert_eq!(lines, vec!["two", "three"]);
    }
}
