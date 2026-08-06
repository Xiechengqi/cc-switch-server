use std::borrow::Cow;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::domain::settings::ui_settings::ParsedLogConfig;
use crate::logging::{
    merge_tail_lines, open_log_append, persistent_log_path, tail_file_lines, RING_BUFFER_CAPACITY,
};

const PERSISTENT_LOG_MAX_BYTES: u64 = 8 * 1024 * 1024;
const PERSISTENT_LOG_BACKUP_COUNT: usize = 2;

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
    file_error_reported: AtomicBool,
    buffer: Mutex<VecDeque<String>>,
    file_path: Mutex<PathBuf>,
    capacity: usize,
}

impl LogCapture {
    pub fn new(capacity: usize) -> Self {
        Self {
            enabled: AtomicBool::new(true),
            file_error_reported: AtomicBool::new(false),
            buffer: Mutex::new(VecDeque::with_capacity(capacity.min(RING_BUFFER_CAPACITY))),
            file_path: Mutex::new(PathBuf::new()),
            capacity: capacity.min(RING_BUFFER_CAPACITY),
        }
    }

    pub fn apply_config(&self, config: &ParsedLogConfig, config_dir: &Path) {
        self.enabled.store(config.enabled, Ordering::Relaxed);
        if config.enabled {
            *self.file_path.lock().expect("log file path lock") = persistent_log_path(config_dir);
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
            let file_path = self.file_path.lock().expect("log file path lock");
            if !file_path.as_os_str().is_empty() {
                match append_rotating_line(
                    &file_path,
                    &stored,
                    PERSISTENT_LOG_MAX_BYTES,
                    PERSISTENT_LOG_BACKUP_COUNT,
                ) {
                    Ok(()) => self.file_error_reported.store(false, Ordering::Relaxed),
                    Err(error) => {
                        if !self.file_error_reported.swap(true, Ordering::Relaxed) {
                            eprintln!(
                                "cc-switch-server persistent log write failed for {}: {error}",
                                file_path.display()
                            );
                        }
                    }
                }
            }
        }
        let mut buffer = self.buffer.lock().expect("log buffer lock");
        if buffer.len() >= self.capacity {
            buffer.pop_front();
        }
        buffer.push_back(stored);
    }

    pub fn tail_lines(&self, lines: usize) -> Vec<String> {
        let buffer = self.buffer.lock().expect("log buffer lock");
        let start = buffer.len().saturating_sub(lines);
        buffer.iter().skip(start).cloned().collect()
    }

    pub fn read_current_process_tail(&self, requested_lines: usize) -> LogTailResponse {
        let buffer = self.buffer.lock().expect("log buffer lock");
        let total_available = buffer.len();
        let start = total_available.saturating_sub(requested_lines);
        let lines = buffer.iter().skip(start).cloned().collect::<Vec<_>>();
        LogTailResponse {
            lines: lines.len(),
            truncated: total_available > requested_lines,
            source: LogTailSource::Buffer,
            path: String::new(),
            content: lines.join("\n"),
        }
    }

    pub fn read_tail(
        &self,
        _config: &ParsedLogConfig,
        config_dir: &Path,
        requested_lines: usize,
    ) -> LogTailResponse {
        let configured_path = self.file_path.lock().expect("log file path lock");
        let path = if configured_path.as_os_str().is_empty() {
            persistent_log_path(config_dir)
        } else {
            configured_path.clone()
        };
        let buffer = self.tail_lines(requested_lines);
        let file = tail_rotating_files(&path, requested_lines, PERSISTENT_LOG_BACKUP_COUNT)
            .unwrap_or_default();
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

    #[cfg(test)]
    fn set_file_path_for_test(&self, path: PathBuf) {
        *self.file_path.lock().expect("log file path lock") = path;
    }
}

fn append_rotating_line(
    path: &Path,
    line: &str,
    max_bytes: u64,
    backup_count: usize,
) -> io::Result<()> {
    let line = bounded_persistent_line(line, max_bytes);
    let additional_bytes = u64::try_from(line.len())
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let current_bytes = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    if current_bytes > 0 && current_bytes.saturating_add(additional_bytes) > max_bytes {
        rotate_files(path, backup_count, max_bytes)?;
    }
    let mut file = open_log_append(path)?;
    writeln!(file, "{line}")
}

fn bounded_persistent_line(line: &str, max_bytes: u64) -> Cow<'_, str> {
    const TRUNCATED_SUFFIX: &str = " ... [persistent log line truncated]";

    let max_line_bytes = usize::try_from(max_bytes.saturating_sub(1)).unwrap_or(usize::MAX);
    if line.len() <= max_line_bytes {
        return Cow::Borrowed(line);
    }
    let suffix = if TRUNCATED_SUFFIX.len() <= max_line_bytes {
        TRUNCATED_SUFFIX
    } else {
        ""
    };
    let mut end = max_line_bytes.saturating_sub(suffix.len());
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!("{}{suffix}", &line[..end]))
}

fn rotate_files(path: &Path, backup_count: usize, max_bytes: u64) -> io::Result<()> {
    if backup_count == 0 {
        return open_private_truncate(path).map(|_| ());
    }
    for index in (1..=backup_count).rev() {
        let source = if index == 1 {
            path.to_path_buf()
        } else {
            rotated_path(path, index - 1)
        };
        let target = rotated_path(path, index);
        match std::fs::metadata(&source) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        }
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        match std::fs::rename(&source, &target) {
            Ok(()) => trim_file_to_tail(&target, max_bytes)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn trim_file_to_tail(path: &Path, max_bytes: u64) -> io::Result<()> {
    let mut source = File::open(path)?;
    let file_bytes = source.metadata()?.len();
    if file_bytes <= max_bytes {
        return Ok(());
    }
    let start = file_bytes - max_bytes;
    source.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::with_capacity(usize::try_from(max_bytes).unwrap_or(usize::MAX));
    source.read_to_end(&mut tail)?;
    if start > 0 {
        if let Some(first_newline) = tail.iter().position(|byte| *byte == b'\n') {
            tail.drain(..=first_newline);
        }
    }
    let temporary = rotated_temporary_path(path);
    let mut output = open_private_truncate(&temporary)?;
    output.write_all(&tail)?;
    output.sync_all()?;
    std::fs::rename(temporary, path)
}

fn tail_rotating_files(path: &Path, lines: usize, backup_count: usize) -> io::Result<Vec<String>> {
    let mut collected = Vec::new();
    for candidate in (1..=backup_count)
        .rev()
        .map(|index| rotated_path(path, index))
        .chain(std::iter::once(path.to_path_buf()))
    {
        match tail_file_lines(&candidate, lines) {
            Ok(mut candidate_lines) => collected.append(&mut candidate_lines),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        }
        if collected.len() > lines {
            collected.drain(..collected.len() - lines);
        }
    }
    Ok(collected)
}

fn rotated_path(path: &Path, index: usize) -> PathBuf {
    let mut rotated = path.as_os_str().to_os_string();
    rotated.push(format!(".{index}"));
    PathBuf::from(rotated)
}

fn rotated_temporary_path(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(".rotate-tmp");
    PathBuf::from(temporary)
}

fn open_private_truncate(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub type SharedLogCapture = Arc<LogCapture>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_log_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-log-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create log test directory");
        dir.join("server.log")
    }

    #[test]
    fn push_line_respects_capacity() {
        let path = test_log_path("capacity");
        let capture = LogCapture::new(2);
        capture.set_file_path_for_test(path.clone());
        capture.push_line("one".into());
        capture.push_line("two".into());
        capture.push_line("three".into());
        let lines = capture.tail_lines(10);
        assert_eq!(lines, vec!["two", "three"]);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn current_process_tail_does_not_include_persisted_history() {
        let path = test_log_path("current-process");
        std::fs::write(&path, "previous process\n").unwrap();
        let capture = LogCapture::new(10);
        capture.set_file_path_for_test(path.clone());
        capture.push_line("current process".into());

        let tail = capture.read_current_process_tail(10);
        assert_eq!(tail.source, LogTailSource::Buffer);
        assert_eq!(tail.content, "current process");
        assert_eq!(tail.lines, 1);
        assert!(!tail.truncated);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn persistent_log_rotation_is_size_bounded_and_keeps_recent_backups() {
        let path = test_log_path("rotation");
        append_rotating_line(&path, "12345678", 10, 2).unwrap();
        append_rotating_line(&path, "two", 10, 2).unwrap();
        append_rotating_line(&path, "abcdef", 10, 2).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abcdef\n");
        assert_eq!(
            std::fs::read_to_string(rotated_path(&path, 1)).unwrap(),
            "two\n"
        );
        assert_eq!(
            std::fs::read_to_string(rotated_path(&path, 2)).unwrap(),
            "12345678\n"
        );
        assert_eq!(
            tail_rotating_files(&path, 10, 2).unwrap(),
            vec!["12345678", "two", "abcdef"]
        );

        append_rotating_line(&path, "last", 10, 2).unwrap();
        assert_eq!(
            tail_rotating_files(&path, 10, 2).unwrap(),
            vec!["two", "abcdef", "last"]
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn persistent_log_rotation_bounds_legacy_files_and_oversized_lines() {
        let path = test_log_path("hard-limit");
        std::fs::write(&path, "old\nold\nold\nold\n").unwrap();

        append_rotating_line(&path, "0123456789abcdef", 10, 2).unwrap();

        assert!(std::fs::metadata(&path).unwrap().len() <= 10);
        assert!(std::fs::metadata(rotated_path(&path, 1)).unwrap().len() <= 10);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
