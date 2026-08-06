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
    formatted_lines: Vec<String>,
}

impl CaptureWriter {
    fn new(capture: Arc<LogCapture>) -> Self {
        Self {
            capture,
            buffer: Vec::new(),
            formatted_lines: Vec::new(),
        }
    }

    fn flush_line(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        let line = String::from_utf8_lossy(&self.buffer).trim_end().to_string();
        self.buffer.clear();
        if !line.is_empty() {
            self.formatted_lines.push(line.clone());
            crate::logging::remote::remember_remote_log_line(&self.formatted_lines.join("\\n"));
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

    // The capture formatter must run before the remote layer consumes its raw line.
    Registry::default()
        .with(filter_layer)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_span_events(FmtSpan::NONE),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_ansi(false)
                .with_span_events(FmtSpan::NONE)
                .with_writer(CaptureMakeWriter {
                    capture: capture.clone(),
                }),
        )
        .with(crate::logging::remote_log_layer())
        .init();
}

pub fn build_filter(log_level: &str) -> EnvFilter {
    EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(log_level))
        .unwrap_or_else(|_| EnvFilter::new("info"))
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
    fn capture_writer_remembers_the_formatter_output_for_remote_upload() {
        let capture = Arc::new(LogCapture::new(2));
        let mut writer = CaptureWriter::new(capture);
        writer.write_all(b"first line\nsecond line\n").unwrap();

        assert_eq!(
            crate::logging::remote::take_remote_log_line().as_deref(),
            Some("first line\\nsecond line")
        );
    }

    #[test]
    fn capture_layer_preserves_the_process_log_format_without_ansi() {
        let capture = Arc::new(LogCapture::new(2));
        let subscriber = Registry::default().with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_ansi(false)
                .with_span_events(FmtSpan::NONE)
                .with_writer(CaptureMakeWriter { capture }),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(answer = 42, "formatter output");
        });

        let raw_line = crate::logging::remote::take_remote_log_line().unwrap();
        assert!(raw_line.contains(" INFO "));
        assert!(raw_line.contains("formatter output"));
        assert!(raw_line.contains("answer=42"));
        assert!(!raw_line.contains('\u{1b}'));
    }
}
