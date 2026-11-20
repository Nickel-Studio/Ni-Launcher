//! Session-owned process output redaction before storage, capture or frontend emission.
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

static ACTIVE_LOGS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

pub(crate) struct SessionLease(PathBuf);

fn log_identity(path: &Path) -> PathBuf {
    let mut ancestor = path;
    let mut suffix = Vec::new();
    loop {
        if let Ok(mut resolved) = std::fs::canonicalize(ancestor) {
            for name in suffix.iter().rev() {
                resolved.push(name);
            }
            return resolved;
        }
        match (ancestor.file_name(), ancestor.parent()) {
            (Some(name), Some(parent)) => {
                suffix.push(name.to_os_string());
                ancestor = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

impl SessionLease {
    fn begin(path: &Path) -> Result<Self, String> {
        let path = log_identity(path);
        let mut active = ACTIVE_LOGS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !active.insert(path.clone()) {
            return Err("该实例的日志会话尚未结束，请稍后重试".into());
        }
        Ok(Self(path))
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        ACTIVE_LOGS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.0);
    }
}

#[cfg(test)]
pub(crate) fn ensure_finished(path: &Path) -> Result<(), String> {
    if ACTIVE_LOGS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains(&log_identity(path))
    {
        Err("游戏输出仍在处理，请等待日志会话结束后重试完整报告".into())
    } else {
        Ok(())
    }
}

/// Reserve the same identity through the entire export so a new output pump
/// cannot start between an idle check and the report's file snapshots.
pub(crate) fn reserve_finished(path: &Path) -> Result<SessionLease, String> {
    SessionLease::begin(path)
}

use minecraft_java_rs_core::launcher::events::{LaunchEvent, ProcessStream};
use tokio::sync::mpsc;

use crate::launch_log_store::{LaunchLogStore, StreamRedactor};
use crate::runtime_settings;
use crate::{FrontendEvent, redact_frontend_event};

const CAPTURE_LINES: usize = 4000;
const CAPTURE_BYTES: usize = 4 * 1024 * 1024;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct OutputCapture {
    lines: VecDeque<String>,
    bytes: usize,
}

impl OutputCapture {
    fn push(&mut self, line: String) {
        while !self.lines.is_empty()
            && (self.lines.len() >= CAPTURE_LINES || self.bytes + line.len() > CAPTURE_BYTES)
        {
            self.bytes -= self.lines.pop_front().unwrap().len();
        }
        self.bytes += line.len();
        self.lines.push_back(line);
    }

    pub(crate) fn text(&self) -> String {
        self.lines
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The guard requests draining on both early returns and normal process termination.
/// Acknowledgment is sent only after final state and log writes are complete.
pub(crate) struct OutputPump {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

struct OutputSession {
    store: LaunchLogStore,
    _lease: SessionLease,
    finished: bool,
}

impl OutputSession {
    fn complete(mut self) {
        self.finished = true;
    }
}

impl Drop for OutputSession {
    fn drop(&mut self) {
        // Task cancellation can drop the future without executing its terminal flush.
        if !self.finished {
            let _ = self
                .store
                .append_safe_text(crate::launch_log_store::REDACTION_FAILURE);
        }
    }
}

impl OutputPump {
    pub(crate) fn start(
        store: LaunchLogStore,
        rx: mpsc::Receiver<LaunchEvent>,
        capture: Arc<Mutex<OutputCapture>>,
        mut emit: impl FnMut(FrontendEvent) + Send + 'static,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(&store.paths().directory).map_err(|_| "无法创建启动日志目录")?;
        let lease = SessionLease::begin(store.log_path())?;
        let session = OutputSession {
            store: store.clone(),
            _lease: lease,
            finished: false,
        };
        let (stop, finish) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            run(store, rx, finish, capture, &mut emit).await;
            session.complete();
        });
        Ok(Self {
            stop: Some(stop),
            task: Some(task),
        })
    }

    pub(crate) async fn finish(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for OutputPump {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

fn stream_index(stream: ProcessStream) -> usize {
    match stream {
        ProcessStream::Stdout => 0,
        ProcessStream::Stderr => 1,
    }
}

fn publish(
    store: &LaunchLogStore,
    capture: &Arc<Mutex<OutputCapture>>,
    text: String,
    emit: &mut impl FnMut(FrontendEvent),
) {
    if text.is_empty() {
        return;
    }
    if store.append_safe_text(&text).is_err() {
        emit(FrontendEvent::Warning {
            message: "无法保存启动日志，请检查日志目录权限。".into(),
        });
    }
    if let Ok(mut capture) = capture.lock() {
        for line in text.lines() {
            capture.push(runtime_settings::bounded_safe_log_line(line));
        }
    }
    for line in text.lines() {
        emit(FrontendEvent::Log {
            line: runtime_settings::bounded_safe_log_line(line),
        });
    }
}

async fn run(
    store: LaunchLogStore,
    mut rx: mpsc::Receiver<LaunchEvent>,
    mut finish: tokio::sync::oneshot::Receiver<()>,
    capture: Arc<Mutex<OutputCapture>>,
    emit: &mut impl FnMut(FrontendEvent),
) {
    let mut streams = [StreamRedactor::default(), StreamRedactor::default()];
    let mut observed = [false; 2];
    let mut closed = [false; 2];
    let mut deadline = None;
    let mut timer = tokio::time::interval(Duration::from_millis(100));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            event = rx.recv() => {
                let Some(event) = event else { break; };
                match event {
                    LaunchEvent::ProcessOutput { stream, line, discarded } => {
                        let index = stream_index(stream);
                        observed[index] = true;
                        let safe = streams[index].push_line(&line, discarded);
                        publish(&store, &capture, safe, emit);
                    }
                    LaunchEvent::OutputClosed { stream, failed } => {
                        let index = stream_index(stream);
                        observed[index] = true;
                        closed[index] = true;
                        let safe = streams[index].finish(failed);
                        publish(&store, &capture, safe, emit);
                    }
                    event => {
                        let mut frontend = FrontendEvent::from(&event);
                        redact_frontend_event(&mut frontend);
                        let text = match &frontend {
                            FrontendEvent::Log { line } => Some(line.clone()),
                            FrontendEvent::Error { message } => Some(format!("[错误] {message}")),
                            FrontendEvent::Close { code } => Some(format!("--- Game exited ({code}) ---")),
                            _ => None,
                        };
                        if let Some(text) = text {
                            if store.append_safe_text(&text).is_err() {
                                emit(FrontendEvent::Warning {
                                    message: "无法保存启动日志，请检查日志目录权限。".into(),
                                });
                            }
                            if let Ok(mut capture) = capture.lock() {
                                capture.push(runtime_settings::bounded_safe_log_line(&text));
                            }
                        }
                        emit(frontend);
                    }
                }
                if deadline.is_some() && closed.iter().all(|closed| *closed) { break; }
            }
            _ = &mut finish, if deadline.is_none() => {
                deadline = Some(tokio::time::Instant::now() + DRAIN_TIMEOUT);
                if closed.iter().all(|closed| *closed) { break; }
            }
            _ = timer.tick() => {
                for stream in &mut streams {
                    let safe = stream.expire();
                    publish(&store, &capture, safe, emit);
                }
                if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                    for index in 0..streams.len() {
                        let safe = streams[index].finish(!closed[index]);
                        publish(&store, &capture, safe, emit);
                    }
                    break;
                }
            }
        }
    }
    for index in 0..streams.len() {
        // A producer disappearing without its ordered EOF loses stream context.
        let safe = streams[index].finish(observed[index] && !closed[index]);
        publish(&store, &capture, safe, emit);
    }
    if streams.iter().any(StreamRedactor::failed) {
        publish(
            &store,
            &capture,
            crate::launch_log_store::REDACTION_FAILURE.into(),
            emit,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TemporaryRoot(PathBuf);
    impl Drop for TemporaryRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn aborted_output_tasks_mark_failure_before_releasing_the_log_lease() {
        for pending in [false, true] {
            let root = TemporaryRoot(std::env::temp_dir().join(format!(
                "nickel-aborted-output-{}",
                crate::store::random_token()
            )));
            let store = LaunchLogStore::instance(&root.0);
            let (tx, rx) = mpsc::channel(4);
            let capture = Arc::new(Mutex::new(OutputCapture::default()));
            let pump = OutputPump::start(store.clone(), rx, capture.clone(), |_| {}).unwrap();
            if pending {
                for line in ["password=[", "stream-secret"] {
                    tx.send(LaunchEvent::ProcessOutput {
                        stream: ProcessStream::Stdout,
                        line: line.into(),
                        discarded: false,
                    })
                    .await
                    .unwrap();
                }
                tx.send(LaunchEvent::ProcessOutput {
                    stream: ProcessStream::Stderr,
                    line: "visible-stderr".into(),
                    discarded: false,
                })
                .await
                .unwrap();
                tokio::time::timeout(Duration::from_secs(2), async {
                    while !capture.lock().unwrap().text().contains("visible-stderr") {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
            }
            pump.task.as_ref().unwrap().abort();
            pump.finish().await;
            drop(tx);
            assert!(ensure_finished(store.log_path()).is_ok());
            let stored = std::fs::read_to_string(store.log_path()).unwrap();
            assert!(stored.contains(crate::launch_log_store::REDACTION_FAILURE));
            assert!(!stored.contains("stream-secret"));
            assert!(crate::launch_log_store::redact_sensitive_text_checked(&stored).is_err());
        }
    }

    #[test]
    fn safe_output_capture_has_both_byte_and_line_budgets() {
        let mut capture = OutputCapture::default();
        for _ in 0..CAPTURE_LINES + 1 {
            capture.push("line".into());
        }
        assert_eq!(capture.lines.len(), CAPTURE_LINES);
        let line = runtime_settings::bounded_safe_log_line(&"x".repeat(100_000));
        for _ in 0..CAPTURE_LINES {
            capture.push(line.clone());
        }
        assert!(capture.lines.len() <= CAPTURE_LINES);
        assert!(capture.bytes <= CAPTURE_BYTES);
        assert_eq!(
            capture.bytes,
            capture.lines.iter().map(String::len).sum::<usize>()
        );
    }
}
