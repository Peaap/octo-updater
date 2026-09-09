use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

/// A single structured log entry surfaced to the Svelte log viewer, mirroring
/// the legacy Python updater's `log()`/`_LOG_Q` pattern (a plain message plus
/// a semantic tag), but delivered as a typed Tauri event instead of a Tk
/// queue drained on the main thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub timestamp_unix: u64,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Info,
    Success,
    Warning,
    Error,
}

const MAX_BUFFERED_ENTRIES: usize = 500;

/// A small ring buffer of recent log entries, so a log viewer opened after
/// startup can be seeded with history rather than only future events. Kept
/// in `AppState` and guarded by its own mutex (separate from the updater
/// service's) so logging never contends with update/mod operations.
#[derive(Default)]
pub struct LogBuffer(Mutex<Vec<LogEntry>>);

impl LogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<LogEntry> {
        self.0
            .lock()
            .map(|buffer| buffer.clone())
            .unwrap_or_default()
    }

    fn push(&self, entry: LogEntry) {
        if let Ok(mut buffer) = self.0.lock() {
            buffer.push(entry);
            if buffer.len() > MAX_BUFFERED_ENTRIES {
                let overflow = buffer.len() - MAX_BUFFERED_ENTRIES;
                buffer.drain(0..overflow);
            }
        }
    }
}

/// Records `message` at `level` in the buffer and emits it as an
/// "app-log" event for any open window to render immediately. Safe to call
/// from any thread (background update/mod work included); never panics on a
/// closed/missing window.
pub fn log(app: &AppHandle, buffer: &LogBuffer, level: LogLevel, message: impl Into<String>) {
    let entry = LogEntry {
        timestamp_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0),
        level,
        message: message.into(),
    };
    buffer.push(entry.clone());
    let _ = app.emit("app-log", entry);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_caps_at_max_entries_and_keeps_the_most_recent() {
        let buffer = LogBuffer::new();
        for i in 0..(MAX_BUFFERED_ENTRIES + 10) {
            buffer.push(LogEntry {
                timestamp_unix: i as u64,
                level: LogLevel::Info,
                message: format!("entry {i}"),
            });
        }
        let snapshot = buffer.snapshot();
        assert_eq!(snapshot.len(), MAX_BUFFERED_ENTRIES);
        assert_eq!(
            snapshot.last().unwrap().message,
            format!("entry {}", MAX_BUFFERED_ENTRIES + 9)
        );
        assert_eq!(snapshot.first().unwrap().message, "entry 10");
    }

    #[test]
    fn snapshot_of_empty_buffer_is_empty() {
        let buffer = LogBuffer::new();
        assert!(buffer.snapshot().is_empty());
    }
}
