use std::io::{self, Write};
use std::sync::Arc;

use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{reload, EnvFilter, Registry};

use crate::logging::capture::LogCapture;

struct CaptureWriter {
    capture: Arc<LogCapture>,
    buffer: Vec<u8>,
}

impl CaptureWriter {
    fn new(capture: Arc<LogCapture>) -> Self {
        Self {
            capture,
            buffer: Vec::new(),
        }
    }

    fn flush_line(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        let line = String::from_utf8_lossy(&self.buffer).trim_end().to_string();
        self.buffer.clear();
        if !line.is_empty() {
            self.capture.push_line(line);
        }
    }
}

impl Drop for CaptureWriter {
    fn drop(&mut self) {
        self.flush_line();
    }
}

impl Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for byte in buf {
            if *byte == b'\n' {
                self.flush_line();
            } else {
                self.buffer.push(*byte);
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_line();
        Ok(())
    }
}

struct CaptureMakeWriter {
    capture: Arc<LogCapture>,
}

impl<'a> MakeWriter<'a> for CaptureMakeWriter {
    type Writer = CaptureWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CaptureWriter::new(self.capture.clone())
    }
}

pub fn reload_log_level(level: &str) {
    if let Some(handle) = FILTER_HANDLE.get() {
        reload_filter(handle, level);
    }
}

static FILTER_HANDLE: std::sync::OnceLock<reload::Handle<EnvFilter, Registry>> =
    std::sync::OnceLock::new();

pub fn init_tracing(log_level: &str, capture: Arc<LogCapture>) {
    let filter = build_filter(log_level);
    let (filter_layer, filter_handle) = reload::Layer::new(filter);
    let _ = FILTER_HANDLE.set(filter_handle);

    Registry::default()
        .with(filter_layer)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_span_events(FmtSpan::NONE),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_ansi(false)
                .with_span_events(FmtSpan::NONE)
                .with_writer(CaptureMakeWriter {
                    capture: capture.clone(),
                }),
        )
        .init();
}

pub fn build_filter(log_level: &str) -> EnvFilter {
    if let Ok(directives) = std::env::var("RUST_LOG") {
        if let Ok(filter) = EnvFilter::try_new(with_dependency_noise_floor(&directives)) {
            return filter;
        }
    }
    EnvFilter::try_new(default_filter_directives(log_level))
        .unwrap_or_else(|_| EnvFilter::new(default_filter_directives("info")))
}

fn default_filter_directives(log_level: &str) -> String {
    let level = log_level.trim().to_ascii_lowercase();
    with_dependency_noise_floor(&level)
}

fn with_dependency_noise_floor(directives: &str) -> String {
    let directives = directives.trim();
    if explicitly_configures_russh(directives) {
        return directives.to_string();
    }
    if directives.is_empty() {
        "russh::client=warn".to_string()
    } else {
        format!("{directives},russh::client=warn")
    }
}

fn explicitly_configures_russh(directives: &str) -> bool {
    directives.split(',').any(|directive| {
        let selector = directive
            .trim()
            .split(['[', '{', '='])
            .next()
            .unwrap_or_default()
            .trim();
        selector == "russh" || selector.starts_with("russh::")
    })
}

pub fn reload_filter(handle: &reload::Handle<EnvFilter, Registry>, level: &str) {
    let _ = handle.modify(|filter| {
        *filter = build_filter(level);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_writer_records_each_formatted_line() {
        let capture = Arc::new(LogCapture::new(2));
        let mut writer = CaptureWriter::new(capture.clone());
        writer.write_all(b"first line\nsecond line\n").unwrap();
        assert_eq!(capture.tail_lines(2), vec!["first line", "second line"]);
    }

    #[test]
    fn capture_layer_preserves_the_process_log_format_without_ansi() {
        let capture = Arc::new(LogCapture::new(2));
        let subscriber = Registry::default().with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_ansi(false)
                .with_span_events(FmtSpan::NONE)
                .with_writer(CaptureMakeWriter {
                    capture: capture.clone(),
                }),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(answer = 42, "formatter output");
        });

        let raw_line = capture.tail_lines(1).pop().unwrap();
        assert!(raw_line.contains(" INFO "));
        assert!(raw_line.contains("formatter output"));
        assert!(raw_line.contains("answer=42"));
        assert!(raw_line.contains("cc_switch_server::logging::init::tests"));
        assert!(!raw_line.contains('\u{1b}'));
    }

    #[test]
    fn info_defaults_suppress_periodic_russh_rekey_noise() {
        assert_eq!(default_filter_directives("info"), "info,russh::client=warn");
        assert_eq!(
            default_filter_directives("debug"),
            "debug,russh::client=warn"
        );
        assert_eq!(
            with_dependency_noise_floor("info,cc_switch_server=debug"),
            "info,cc_switch_server=debug,russh::client=warn"
        );
        assert_eq!(
            with_dependency_noise_floor("info,russh::client=trace"),
            "info,russh::client=trace"
        );
        assert_eq!(
            with_dependency_noise_floor("info,russh=debug"),
            "info,russh=debug"
        );
    }
}
