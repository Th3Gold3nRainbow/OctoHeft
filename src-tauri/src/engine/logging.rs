use chrono::Local;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;
use std::path::Path;
use std::sync::OnceLock;
use tracing::{field::Visit, Event, Subscriber};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

const RING_CAPACITY: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub target: String,
    pub message: String,
}

static RING_BUFFER: OnceLock<Mutex<VecDeque<LogEntry>>> = OnceLock::new();
static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

fn ring() -> &'static Mutex<VecDeque<LogEntry>> {
    RING_BUFFER.get_or_init(|| Mutex::new(VecDeque::with_capacity(RING_CAPACITY)))
}

struct MessageVisitor {
    message: String,
    extra: String,
}

impl MessageVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
            extra: String::new(),
        }
    }
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
            // Strip surrounding quotes if present from debug format
            if self.message.starts_with('"') && self.message.ends_with('"') && self.message.len() >= 2 {
                self.message = self.message[1..self.message.len() - 1].to_string();
            }
        } else {
            if !self.extra.is_empty() {
                self.extra.push_str(", ");
            }
            self.extra.push_str(&format!("{}={:?}", field.name(), value));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            if !self.extra.is_empty() {
                self.extra.push_str(", ");
            }
            self.extra.push_str(&format!("{}={}", field.name(), value));
        }
    }
}

pub struct RingBufferLayer;

impl<S: Subscriber> Layer<S> for RingBufferLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::new();
        event.record(&mut visitor);

        let full_msg = if visitor.extra.is_empty() {
            visitor.message
        } else if visitor.message.is_empty() {
            visitor.extra
        } else {
            format!("{} ({})", visitor.message, visitor.extra)
        };

        let meta = event.metadata();
        let entry = LogEntry {
            timestamp: Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
            level: meta.level().as_str().to_string(),
            target: meta.target().to_string(),
            message: full_msg,
        };

        let mut buf = ring().lock();
        if buf.len() >= RING_CAPACITY {
            buf.pop_front();
        }
        buf.push_back(entry);
    }
}

/// Initialize global tracing with rolling file appender & in-memory ring buffer
pub fn init_logging(logs_dir: &Path) {
    let _ = std::fs::create_dir_all(logs_dir);

    let file_appender = tracing_appender::rolling::daily(logs_dir, "octoheft.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let _ = LOG_GUARD.set(guard);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true);

    let _ = tracing_subscriber::registry()
        .with(file_layer)
        .with(RingBufferLayer)
        .try_init();

    tracing::info!("OctoHeft structured diagnostics initialized at {:?}", logs_dir);
}

/// Read snapshot of recent events from the in-memory ring buffer
pub fn get_logs() -> Vec<LogEntry> {
    ring().lock().iter().cloned().collect()
}

/// Clear the in-memory ring buffer
pub fn clear_logs() {
    ring().lock().clear();
    tracing::info!("Diagnostic ring buffer cleared by user");
}

/// Format logs as plain text for export
pub fn export_logs_text() -> String {
    let entries = get_logs();
    let mut out = String::with_capacity(entries.len() * 80);
    for e in entries {
        out.push_str(&format!("[{}] [{}] [{}] {}\n", e.timestamp, e.level, e.target, e.message));
    }
    out
}

