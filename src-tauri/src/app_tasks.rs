//! 应用层唯一下载任务生命周期：先登记、真实取消、最终发布与取消互斥。
use crate::{
    network_settings::{self, NetworkSettings},
    store,
};
use minecraft_java_rs_core::net::transfer::{CancellationToken, TransferContext};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const HISTORY_LIMIT: usize = 32;
const FILE_HISTORY_LIMIT: usize = 128;
const SPEED_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskKind {
    #[default]
    Version,
    Modpack,
    Resource,
    Installer,
    Export,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskFile {
    name: String,
    percent: f64,
    done: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStage {
    name: String,
    detail: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    task_id: String,
    kind: TaskKind,
    revision: u64,
    target: String,
    label: String,
    status: String,
    cancel_requested: bool,
    stage: String,
    detail: String,
    percent: Option<f64>,
    files_done: u64,
    files_total: u64,
    transferred_bytes: u64,
    completed_bytes: Option<u64>,
    total_bytes: Option<u64>,
    speed_bps: Option<f64>,
    remaining_ms: Option<u64>,
    started_at: u64,
    ended_at: Option<u64>,
    files: Vec<TaskFile>,
    stages: Vec<TaskStage>,
    message: String,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl TaskSnapshot {
    fn new(id: &str, target: &Path) -> Self {
        Self {
            task_id: id.into(),
            kind: TaskKind::default(),
            revision: 0,
            target: target.to_string_lossy().into_owned(),
            label: target
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            status: "running".into(),
            cancel_requested: false,
            stage: "准备下载".into(),
            detail: String::new(),
            percent: None,
            files_done: 0,
            files_total: 0,
            transferred_bytes: 0,
            completed_bytes: None,
            total_bytes: None,
            speed_bps: None,
            remaining_ms: None,
            started_at: now_ms(),
            ended_at: None,
            files: Vec::new(),
            stages: Vec::new(),
            message: String::new(),
        }
    }
}

struct ProgressState {
    snapshot: TaskSnapshot,
    sampled_at: Instant,
    sampled_bytes: u64,
}

impl ProgressState {
    fn new(id: &str, target: &Path) -> Self {
        Self {
            snapshot: TaskSnapshot::new(id, target),
            sampled_at: Instant::now(),
            sampled_bytes: 0,
        }
    }

    fn sample(&mut self, bytes: u64) {
        let elapsed = self.sampled_at.elapsed();
        if elapsed >= SPEED_SAMPLE_INTERVAL {
            self.snapshot.speed_bps =
                Some(bytes.saturating_sub(self.sampled_bytes) as f64 / elapsed.as_secs_f64());
            self.sampled_at = Instant::now();
            self.sampled_bytes = bytes;
        }
        self.snapshot.transferred_bytes = bytes;
        self.snapshot.revision += 1;
    }
}

#[derive(Default)]
struct Lifecycle {
    terminal: bool,
}
struct TaskRecord {
    cancel: CancellationToken,
    lifecycle: Mutex<Lifecycle>,
    target: Mutex<PathBuf>,
    progress: Mutex<ProgressState>,
    transfer: TransferContext,
}

impl TaskRecord {
    fn cancel(&self) {
        let lifecycle = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        if !lifecycle.terminal {
            self.cancel.cancel();
        }
    }

    fn finalize<T>(
        &self,
        commit: impl FnOnce() -> Result<T, String>,
    ) -> Option<(bool, Result<T, String>)> {
        self.finalize_transaction((), |_| commit(), |_| Ok(()))
    }

    fn finalize_transaction<T, R>(
        &self,
        mut resource: R,
        commit: impl FnOnce(&mut R) -> Result<T, String>,
        cleanup: impl FnOnce(&mut R) -> Result<(), String>,
    ) -> Option<(bool, Result<T, String>)> {
        let mut lifecycle = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        if lifecycle.terminal {
            return None;
        }
        let cancelled = self.cancel.is_cancelled();
        let mut result = if cancelled {
            drop(commit);
            Err("下载已取消".into())
        } else {
            commit(&mut resource)
        };
        if let Err(error) = &mut result {
            if let Err(cleanup_error) = cleanup(&mut resource) {
                *error = format!("{error}；{cleanup_error}");
            }
        }
        drop(resource);
        lifecycle.terminal = true;
        Some((cancelled, result))
    }

    fn overlaps(&self, target: &Path) -> bool {
        let existing = self.target.lock().unwrap_or_else(|e| e.into_inner());
        target.starts_with(&*existing) || existing.starts_with(target)
    }
}
static TASKS: LazyLock<Mutex<HashMap<String, Arc<TaskRecord>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static HISTORY: LazyLock<Mutex<VecDeque<TaskSnapshot>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

/// 活动任务是并行集合，不以列表顺序伪造等待队列。终态只在事务收尾后进入历史。
#[tauri::command]
pub fn app_tasks_snapshot() -> Vec<TaskSnapshot> {
    let tasks = TASKS.lock().unwrap_or_else(|e| e.into_inner());
    let mut snapshots: Vec<_> = tasks
        .values()
        .map(|record| {
            let mut progress = record.progress.lock().unwrap_or_else(|e| e.into_inner());
            if progress.snapshot.status == "running" {
                progress.sample(record.transfer.transferred_bytes());
                progress.snapshot.cancel_requested = record.cancel.is_cancelled();
            }
            progress.snapshot.clone()
        })
        .collect();
    snapshots.extend(
        HISTORY
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned(),
    );
    snapshots.sort_by_key(|snapshot| snapshot.started_at);
    snapshots
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledInstance {
    pub root: String,
    pub version_id: String,
    pub display_name: String,
    pub source: serde_json::Value,
}

#[derive(Clone)]
pub struct TaskContext {
    pub id: String,
    pub network: NetworkSettings,
    pub transfer: TransferContext,
    pub workspace: PathBuf,
    app: AppHandle,
    record: Arc<TaskRecord>,
}

pub fn begin(app: &AppHandle, target: &Path, source: Option<&str>) -> Result<TaskContext, String> {
    begin_with_network(app, target, source, network_settings::snapshot())
}

fn resolve_download_target(target: &Path) -> Result<(PathBuf, PathBuf), String> {
    let absolute = std::path::absolute(target).map_err(|e| format!("解析下载目录失败：{e}"))?;
    for ancestor in absolute.ancestors() {
        match std::fs::canonicalize(ancestor) {
            Ok(existing) => {
                let suffix = absolute
                    .strip_prefix(ancestor)
                    .map_err(|e| format!("解析下载目录失败：{e}"))?;
                let resolved = if suffix.as_os_str().is_empty() {
                    existing.clone()
                } else {
                    existing.join(suffix)
                };
                return Ok((resolved, existing));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("解析下载目录失败：{error}")),
        }
    }
    Err("下载目标缺少可访问的父目录，请重新选择目录".into())
}

pub(crate) fn begin_with_network(
    app: &AppHandle,
    target: &Path,
    source: Option<&str>,
    network: NetworkSettings,
) -> Result<TaskContext, String> {
    let transfer = network.transfer(source)?;
    let id = store::random_token();
    let workspace = crate::app_paths::config_dir(app)?
        .join("cache/downloads")
        .join(&id);
    let mut tasks = TASKS.lock().unwrap_or_else(|e| e.into_inner());
    let (target, existing) = resolve_download_target(target)?;
    if tasks.values().any(|item| item.overlaps(&target)) {
        return Err("此目标已有下载任务，请等待完成或取消后重试".into());
    }
    // 与启动入口保持 TASKS → RUNNING 顺序，检查到登记期间不能插入游戏启动。
    crate::launch_state::with_idle(&existing, || {
        std::fs::create_dir_all(&workspace).map_err(|e| format!("创建下载暂存目录失败：{e}"))?;
        let record = Arc::new(TaskRecord {
            cancel: transfer.cancellation().clone(),
            lifecycle: Mutex::new(Lifecycle::default()),
            progress: Mutex::new(ProgressState::new(&id, &target)),
            target: Mutex::new(target),
            transfer: transfer.clone(),
        });
        tasks.insert(id.clone(), record.clone());
        Ok(TaskContext {
            id,
            network,
            transfer,
            workspace,
            app: app.clone(),
            record,
        })
    })
}

pub fn cancel(task_id: &str) {
    let record = TASKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(task_id)
        .cloned();
    if let Some(record) = record {
        record.cancel();
    }
}

pub fn with_idle_directory<T>(
    directory: &Path,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let target =
        std::fs::canonicalize(directory).map_err(|error| format!("解析实例目录失败：{error}"))?;
    let registry = TASKS.lock().unwrap_or_else(|error| error.into_inner());
    if registry.values().any(|task| task.overlaps(&target)) {
        return Err("该实例仍在安装或取消收尾中，请等待后台任务结束后重试".into());
    }
    action()
}

#[cfg(test)]
pub struct TestReservation(String);
#[cfg(test)]
impl Drop for TestReservation {
    fn drop(&mut self) {
        TASKS.lock().unwrap().remove(&self.0);
    }
}
#[cfg(test)]
pub fn test_reserve(target: &Path) -> TestReservation {
    let id = store::random_token();
    let transfer = network_settings::NetworkSettings::default()
        .transfer(None)
        .unwrap();
    let cancel = transfer.cancellation().clone();
    cancel.cancel();
    TASKS.lock().unwrap().insert(
        id.clone(),
        Arc::new(TaskRecord {
            cancel,
            lifecycle: Mutex::new(Lifecycle::default()),
            target: Mutex::new(std::fs::canonicalize(target).unwrap()),
            progress: Mutex::new(ProgressState::new(&id, target)),
            transfer,
        }),
    );
    TestReservation(id)
}

impl TaskContext {
    pub fn set_target(&self, target: &Path) -> Result<(), String> {
        let target = std::fs::canonicalize(target).map_err(|e| format!("解析下载目录失败：{e}"))?;
        let tasks = TASKS.lock().unwrap_or_else(|e| e.into_inner());
        if tasks
            .iter()
            .any(|(id, record)| id != &self.id && record.overlaps(&target))
        {
            return Err("此目标已有下载任务，请等待完成或取消后重试".into());
        }
        crate::launch_state::with_idle(&target, || {
            self.update_progress(|snapshot| {
                snapshot.target = target.to_string_lossy().into_owned()
            });
            *self.record.target.lock().unwrap_or_else(|e| e.into_inner()) = target.clone();
            Ok(())
        })

    }

    /// 在启动异步工作前登记种类，快照恢复后才能区分串行安装和并行文件任务。
    pub fn describe_kind(&self, kind: TaskKind, label: &str) {
        self.update_progress(|snapshot| {
            snapshot.kind = kind;
            snapshot.label = label.into();
        });
    }

    fn update_progress(&self, update: impl FnOnce(&mut TaskSnapshot)) {
        let lifecycle = self
            .record
            .lifecycle
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !lifecycle.terminal {
            let mut progress = self
                .record
                .progress
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            update(&mut progress.snapshot);
            progress.snapshot.revision += 1;
        }
    }

    pub fn progress_bytes(&self, completed: u64, total: Option<u64>) {
        self.update_progress(|snapshot| {
            snapshot.completed_bytes = Some(completed);
            snapshot.total_bytes = total;
            snapshot.percent = total
                .filter(|n| *n > 0)
                .map(|n| (completed as f64 / n as f64).clamp(0.0, 1.0));
        });
    }

    pub fn check_cancelled(&self) -> Result<(), String> {
        self.transfer.cancellation().check()
    }
    pub fn stage(&self, stage: &str, detail: &str) {
        self.update_progress(|snapshot| {
            if snapshot.stage != stage || snapshot.detail != detail {
                snapshot.percent = None;
                snapshot.files_done = 0;
                snapshot.files_total = 0;
                snapshot.completed_bytes = None;
                snapshot.total_bytes = None;
                snapshot.speed_bps = None;
                snapshot.remaining_ms = None;
                snapshot.stages.push(TaskStage { name: stage.into(), detail: detail.into() });
                if snapshot.stages.len() > FILE_HISTORY_LIMIT { snapshot.stages.remove(0); }
            }
            snapshot.stage = stage.into();
            snapshot.detail = detail.into();
        });
        self.emit(crate::install::InstallEvent::Stage {
            task_id: self.id.clone(), stage: stage.into(), detail: detail.into(),
        });
    }
    pub fn file(&self, name: &str, percent: f64, done: bool) {
        self.update_progress(|snapshot| {
            let file = TaskFile {
                name: name.into(),
                percent: if percent.is_finite() {
                    percent.clamp(0.0, 1.0)
                } else {
                    0.0
                },
                done,
            };
            if let Some(existing) = snapshot.files.iter_mut().find(|file| file.name == name) {
                *existing = file;
            } else {
                if snapshot.files.len() >= FILE_HISTORY_LIMIT {
                    snapshot.files.remove(0);
                }
                snapshot.files.push(file);
            }
        });
        self.emit(crate::install::InstallEvent::File {
            task_id: self.id.clone(),
            name: name.into(),
            percent,
            done,
        });
    }
    pub fn progress_files(&self, files_done: u64, files_total: u64) {
        self.update_progress(|snapshot| {
            snapshot.files_done = files_done;
            snapshot.files_total = files_total;
            if snapshot.total_bytes.is_none() {
                snapshot.percent = (files_total > 0)
                    .then(|| (files_done as f64 / files_total as f64).clamp(0.0, 1.0));
            }
        });
        self.emit_progress_snapshot();
    }

    pub fn transfer_progress(&self, speed: Option<f64>, remaining_ms: Option<u64>) {
        self.update_progress(|snapshot| {
            if let Some(speed) = speed {
                snapshot.speed_bps = (speed.is_finite() && speed >= 0.0).then_some(speed);
            }
            if let Some(remaining) = remaining_ms {
                snapshot.remaining_ms = (remaining > 0).then_some(remaining);
            }
        });
        self.emit_progress_snapshot();
    }

    fn emit_progress_snapshot(&self) {
        let event = {
            let progress = self.record.progress.lock().unwrap_or_else(|e| e.into_inner());
            let snapshot = &progress.snapshot;
            crate::install::progress_event(self.id.clone(), snapshot.files_done, snapshot.files_total,
                snapshot.speed_bps.unwrap_or(0.0), snapshot.remaining_ms.unwrap_or(0))
        };
        self.emit(event);
    }

    pub fn progress(&self, files_done: u64, files_total: u64, speed_bps: f64, remaining_ms: u64) {
        self.update_progress(|snapshot| {
            snapshot.files_done = files_done;
            snapshot.files_total = files_total;
            if snapshot.total_bytes.is_none() {
                snapshot.percent = (files_total > 0)
                    .then(|| (files_done as f64 / files_total as f64).clamp(0.0, 1.0));
            }
            snapshot.remaining_ms = (remaining_ms > 0).then_some(remaining_ms);
        });
        self.emit(crate::install::progress_event(
            self.id.clone(),
            files_done,
            files_total,
            speed_bps,
            remaining_ms,
        ));
    }
    fn emit(&self, event: crate::install::InstallEvent) {
        let lifecycle = self
            .record
            .lifecycle
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !lifecycle.terminal {
            let _ = self.app.emit(crate::install::INSTALL_EVENT, event);
        }
    }
    pub fn finish(&self, result: Result<Option<InstalledInstance>, String>) {
        self.finish_transaction(
            Some(result),
            |result| result.take().unwrap_or(Ok(None)),
            |result| match result.take() {
                Some(Err(error)) if error != "下载已取消" => Err(error),
                _ => Ok(()),
            },
        );
    }
    /// commit 只有在持有生命周期锁且尚未取消时执行。发布赢得竞争后不再接受取消。
    pub fn finish_with<F>(&self, commit: F)
    where
        F: FnOnce() -> Result<Option<InstalledInstance>, String>,
    {
        if let Some((cancelled, result)) = self.record.finalize(commit) {
            self.publish_finish(cancelled, result);
        }
    }

    /// 失败、取消或提交失败均显式清理，清理错误并入同一终态。
    pub fn finish_transaction<R>(
        &self,
        resource: R,
        commit: impl FnOnce(&mut R) -> Result<Option<InstalledInstance>, String>,
        cleanup: impl FnOnce(&mut R) -> Result<(), String>,
    ) {
        if let Some((cancelled, result)) =
            self.record.finalize_transaction(resource, commit, cleanup)
        {
            self.publish_finish(cancelled, result);
        }
    }

    fn publish_finish(&self, cancelled: bool, result: Result<Option<InstalledInstance>, String>) {
        let (ok, message, installed) = match result {
            Ok(installed) => (true, String::new(), installed),
            Err(message) => (false, message, None),
        };
        let _ = std::fs::remove_dir_all(&self.workspace);
        // 与快照读取使用同一锁顺序，避免收尾瞬间任务既不在活动列表也不在历史中。
        let mut tasks = TASKS.lock().unwrap_or_else(|e| e.into_inner());
        let mut progress = self
            .record
            .progress
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        progress.sample(self.transfer.transferred_bytes());
        let snapshot = &mut progress.snapshot;
        snapshot.status = if cancelled {
            "cancelled"
        } else if ok {
            "done"
        } else {
            "error"
        }
        .into();
        snapshot.cancel_requested = cancelled;
        snapshot.ended_at = Some(now_ms());
        snapshot.speed_bps = None;
        snapshot.remaining_ms = None;
        snapshot.message = message.clone();
        if ok {
            snapshot.percent = Some(1.0);
        }
        let mut history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
        history.push_back(snapshot.clone());
        while history.len() > HISTORY_LIMIT {
            history.pop_front();
        }
        tasks.remove(&self.id);
        drop(history);
        drop(progress);
        drop(tasks);
        let _ = self.app.emit(
            crate::install::INSTALL_EVENT,
            crate::install::InstallEvent::Done {
                task_id: self.id.clone(),
                ok,
                message,
                cancelled,
                result: installed,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn record() -> TaskRecord {
        let transfer = network_settings::NetworkSettings::default()
            .transfer(None)
            .unwrap();
        TaskRecord {
            cancel: transfer.cancellation().clone(),
            lifecycle: Mutex::new(Lifecycle::default()),
            target: Mutex::new(PathBuf::new()),
            progress: Mutex::new(ProgressState::new("test", Path::new(""))),
            transfer,
        }
    }

    #[test]
    fn missing_download_target_uses_nearest_existing_ancestor_without_reserving() {
        let root =
            std::env::temp_dir().join(format!("nowhere-task-target-{}", store::random_token()));
        std::fs::create_dir_all(&root).unwrap();
        let suffix = Path::new("pending").join("mods").join("resource.jar");
        let target = root.join(&suffix);
        let (resolved, ancestor) = resolve_download_target(&target).unwrap();
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        assert_eq!(ancestor, canonical_root);
        assert_eq!(resolved, canonical_root.join(suffix));
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        let existing = root.join("existing.jar");
        std::fs::write(&existing, b"existing").unwrap();
        let (resolved, ancestor) = resolve_download_target(&existing).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(&existing).unwrap());
        assert_eq!(resolved, ancestor);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "真实 Wry 任务与启动占用竞争：显式单线程运行"]
    fn live_task_targets_exclude_running_instances() {
        let mut context = tauri::generate_context!();
        context.config_mut().app.windows.clear();
        context.config_mut().identifier =
            format!("com.youtu.nowhere.tasklocktest.{}", store::random_token());
        let app = tauri::Builder::default()
            .any_thread()
            .build(context)
            .unwrap();
        let root =
            std::env::temp_dir().join(format!("nowhere-task-running-{}", store::random_token()));
        let game = root.join("game");
        let other = root.join("other");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let running = crate::launch_state::begin(&game).unwrap();
        let missing = game.join("mods").join("resource.jar");
        let before = app_tasks_snapshot().len();
        assert!(begin(app.handle(), &missing, None).is_err());
        assert_eq!(app_tasks_snapshot().len(), before);
        assert!(!missing.parent().unwrap().exists());
        let task = begin(app.handle(), &other, None).unwrap();
        let original_target = task.record.target.lock().unwrap().clone();
        assert!(task.set_target(&game).is_err());
        assert_eq!(*task.record.target.lock().unwrap(), original_target);
        assert_eq!(
            task.record.progress.lock().unwrap().snapshot.target,
            original_target.to_string_lossy()
        );
        drop(running);
        task.set_target(&game).unwrap();
        assert!(with_idle_directory(&game, || crate::launch_state::begin(&game)).is_err());
        cancel(&task.id);
        assert!(with_idle_directory(&game, || crate::launch_state::begin(&game)).is_err());
        task.finish(Err("下载已取消".into()));
        assert!(!task.workspace.exists());
        let running = with_idle_directory(&game, || crate::launch_state::begin(&game)).unwrap();
        drop(running);

        let barrier = Arc::new(std::sync::Barrier::new(2));
        let start_barrier = barrier.clone();
        let start_game = game.clone();
        let start = std::thread::spawn(move || {
            start_barrier.wait();
            with_idle_directory(&start_game, || crate::launch_state::begin(&start_game))
        });
        barrier.wait();
        let download = begin(app.handle(), &missing, None);
        let launch = start.join().unwrap();
        assert_ne!(download.is_ok(), launch.is_ok());
        if let Ok(task) = download {
            task.finish(Ok(None));
        }
        drop(launch);
        assert!(with_idle_directory(&game, || Ok(())).is_ok());
        assert!(crate::launch_state::with_idle(&game, || Ok(())).is_ok());
        eprintln!(
            "[task-launch-lock] running_rejects_missing_target=true; rejected_retarget_preserves_original=true; cancelled_task_blocks_until_done=true; concurrent_launch_download_one_winner=true"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_speed_uses_network_bytes_and_keeps_zero_distinct_from_unknown() {
        let mut progress = ProgressState::new("sample", Path::new("instance"));
        assert_eq!(progress.snapshot.speed_bps, None);
        assert_eq!(progress.snapshot.percent, None);
        progress.snapshot.completed_bytes = Some(8 * 1024 * 1024);
        progress.sampled_at = Instant::now() - Duration::from_secs(1);
        progress.sample(1024);
        assert_eq!(progress.snapshot.transferred_bytes, 1024);
        let speed = progress.snapshot.speed_bps.unwrap();
        assert!(speed > 0.0 && speed <= 1024.0, "{speed}");
        assert_eq!(progress.snapshot.percent, None);
        progress.sampled_at = Instant::now() - Duration::from_secs(1);
        progress.sample(1024);
        assert_eq!(progress.snapshot.speed_bps, Some(0.0));
        assert_eq!(progress.snapshot.revision, 2);
    }

    #[test]
    fn snapshot_kind_and_unknown_totals_keep_the_frontend_contract() {
        for (kind, name) in [
            (TaskKind::Version, "version"),
            (TaskKind::Modpack, "modpack"),
            (TaskKind::Resource, "resource"),
            (TaskKind::Installer, "installer"),
            (TaskKind::Export, "export"),
        ] {
            let mut snapshot = TaskSnapshot::new("task", Path::new("instance"));
            snapshot.kind = kind;
            let value = serde_json::to_value(snapshot).unwrap();
            assert_eq!(value["kind"], name);
            assert_eq!(value["transferredBytes"], 0);
            assert!(value["completedBytes"].is_null());
            assert!(value["totalBytes"].is_null());
            assert!(value["speedBps"].is_null());
            assert!(value["percent"].is_null());
        }
    }

    #[test]
    fn cancellation_skips_commit_and_terminal_is_unique() {
        let record = record();
        record.cancel();
        let result = record
            .finalize(|| -> Result<(), String> { panic!("取消后不得提交") })
            .unwrap();
        assert!(result.0);
        assert!(result.1.is_err());
        assert!(record.finalize(|| Ok(())).is_none());
    }

    #[test]
    fn successful_commit_wins_later_cancellation() {
        let record = record();
        let result = record.finalize(|| Ok(42)).unwrap();
        assert!(!result.0);
        assert_eq!(result.1.unwrap(), 42);
        record.cancel();
        assert!(!record.cancel.is_cancelled());
        assert!(record.finalize(|| Ok(0)).is_none());
    }

    #[test]
    fn concurrent_cancel_and_commit_have_one_outcome() {
        let record = Arc::new(record());
        let committed = Arc::new(AtomicBool::new(false));
        let other = record.clone();
        let thread = std::thread::spawn(move || other.cancel());
        let result = record
            .finalize(|| {
                committed.store(true, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
        thread.join().unwrap();
        assert_eq!(result.0, result.1.is_err());
        assert_eq!(committed.load(Ordering::SeqCst), !result.0);
        assert!(record.finalize(|| Ok(())).is_none());
    }

    #[test]
    fn cancelled_closure_releases_resources_before_finalization_returns() {
        struct Cleanup(Arc<AtomicBool>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let record = record();
        let cleaned = Arc::new(AtomicBool::new(false));
        let guard = Cleanup(cleaned.clone());
        record.cancel();
        let _ = record.finalize(move || {
            drop(guard);
            Ok(())
        });
        assert!(cleaned.load(Ordering::SeqCst));
    }

    #[test]
    fn transaction_cleanup_errors_are_included_before_terminal() {
        for cancelled in [false, true] {
            let record = record();
            if cancelled {
                record.cancel();
            }
            let cleaned = AtomicBool::new(false);
            let result = record
                .finalize_transaction(
                    (),
                    |_| Err::<(), _>("安装失败".into()),
                    |_| {
                        assert!(record.lifecycle.try_lock().is_err());
                        cleaned.store(true, Ordering::SeqCst);
                        Err("清理失败".into())
                    },
                )
                .unwrap();
            assert_eq!(result.0, cancelled);
            let error = result.1.unwrap_err();
            assert!(error.contains(if cancelled {
                "下载已取消"
            } else {
                "安装失败"
            }));
            assert!(error.contains("清理失败"));
            assert!(cleaned.load(Ordering::SeqCst));
            assert!(record.finalize(|| Ok(())).is_none());
        }
    }

    #[cfg(windows)]
    #[test]
    fn failed_or_cancelled_pack_cleanup_reports_locked_file_and_stays_hidden() {
        use std::os::windows::fs::OpenOptionsExt;
        for cancelled in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "nowhere-cleanup-terminal-{}",
                store::random_token()
            ));
            let reserved =
                crate::named_profiles::reserve_pack(&root, "Pack", "1.20.1", None, "").unwrap();
            let directory = reserved.directory.clone();
            let locked = directory.join("locked.jar");
            std::fs::write(&locked, b"partial").unwrap();
            let handle = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&locked)
                .unwrap();
            let record = record();
            if cancelled {
                record.cancel();
            }
            let (was_cancelled, result) = record
                .finalize_transaction(
                    reserved,
                    |_| Err::<(), _>("安装失败".into()),
                    crate::named_profiles::ReservedProfile::cleanup,
                )
                .unwrap();
            assert_eq!(was_cancelled, cancelled);
            let error = result.unwrap_err();
            assert!(error.contains("清理未完成实例失败"), "{error}");
            assert!(crate::named_profiles::is_pending_pack(&directory));
            assert_eq!(
                crate::instances::instance_scan(root.to_string_lossy().into()),
                "[]"
            );
            assert!(record.finalize(|| Ok(())).is_none());
            drop(handle);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
