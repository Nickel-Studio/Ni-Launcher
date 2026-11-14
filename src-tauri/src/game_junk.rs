//! 清理启动器与游戏自动生成的日志、崩溃报告等可再生成文件。
//!
//! 扫描与删除分离，用户确认后重新收集候选文件再逐项删除。
//! 范围严格限定在：
//! - 每个游戏目录的 `logs/`（保留当前 `nowhere-launcher.log`）与 `crash-reports/`；
//! - 游戏目录顶层的 `hs_err_pid*.log`、`replay_pid*.log` 与 `authlib-injector.log`；
//! - 配置目录的日志归档与原子写残留（`.nowhere-*.part`、`instances.json.tmp-*`）。
//!
//! 不递归 `versions/`、`mods/`、`saves/` 等目录。游戏根、`logs/`、`crash-reports/`
//! 自身是符号链接 / Windows 重解析点时跳过，不跟随到游戏目录之外；但允许它们的**祖先**
//! 是链接（macOS 的 `/var` → `/private/var`、Windows 的 OneDrive / junction 重定向），
//! 否则这些位置下的合法游戏目录会被静默跳过。单个文件删除失败（例如正被运行中的游戏
//! 占用）只计数，不影响其它文件，也不 panic。

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::AppHandle;

const LAUNCHER_LOG_NAME: &str = "nowhere-launcher.log";

/// 同一时间只允许一个清理任务，避免重复点击并发删除。
static JUNK_BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameJunkScan {
    /// 可清理文件的合计字节数。
    pub total_bytes: u64,
    /// 可清理文件数量。
    pub file_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameJunkCleanResult {
    /// 已成功删除的字节数。
    pub freed_bytes: u64,
    /// 已成功删除的文件数量。
    pub removed_count: usize,
    /// 因被占用或无权限而跳过的文件数量。
    pub failed_count: usize,
}

struct BusyGuard;

impl Drop for BusyGuard {
    fn drop(&mut self) {
        JUNK_BUSY.store(false, Ordering::SeqCst);
    }
}

fn begin_clean() -> Result<BusyGuard, String> {
    if JUNK_BUSY
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("上一次清理还没有结束，请稍等一下".into());
    }
    Ok(BusyGuard)
}

/// Windows 路径大小写不敏感；去重键统一小写，避免同一目录被统计两次。
fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

fn push_unique(out: &mut Vec<PathBuf>, seen: &mut BTreeSet<String>, path: PathBuf) {
    if seen.insert(path_key(&path)) {
        out.push(path);
    }
}

fn push_file(files: &mut Vec<PathBuf>, seen: &mut BTreeSet<String>, path: PathBuf) {
    push_unique(files, seen, path);
}

/// 判断路径**自身**是否为符号链接 / Windows 重解析点，不检查祖先。
///
/// 复用 `instances::reject_links_local`，让重解析点判定只有一处实现；读取元数据
/// 失败（路径已不存在等）按「不是链接」处理，后续 `read_dir` 同样会安全失败。
fn is_link(path: &Path) -> bool {
    crate::instances::reject_links_local(path).is_err()
}

/// 只把「像游戏目录」的根纳入清理：`versions/` 是目录，或顶层存在
/// `options.txt` / `level.dat`。普通项目目录即使带 `logs/` 也不能被当成游戏目录。
fn looks_like_game_root(root: &Path) -> bool {
    root.join("versions").is_dir()
        || root.join("options.txt").is_file()
        || root.join("level.dat").is_file()
}

/// 过滤候选根：必须存在、可 canonicalize、根**自身**不是链接，且符合游戏目录特征。
///
/// 只检查根自身：macOS 的 `std::env::temp_dir()` 位于 `/var` → `/private/var` 符号
/// 链接之下，Windows 的 OneDrive / junction 重定向目录也会让合法游戏目录带链接祖先；
/// 若连祖先一起拒绝，这些位置会被静默跳过。根自身是链接时仍必须拒绝，避免
/// canonicalize 把清理目标指到别处。
///
/// 链接检查必须在 canonicalize **之前**、对原始 path 做：canonicalize 会把路径上的
/// 链接全部解析成真实路径，之后再检查 canonical 永远看不到链接本身，防护会失效。
///
/// 已知限制：新建但尚未启动过的实例根没有 `versions/`、`options.txt`、`level.dat`，
/// 因此不会被识别为游戏目录，其 `logs/` 也就不会被清理。这里选择宁漏勿错——
/// 误把普通目录当游戏目录会删掉真实日志，代价远高于留下几个可再生成的日志文件。
fn retain_game_roots(mut candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    candidates.retain(|path| {
        if !path.is_absolute() || !path.is_dir() {
            return false;
        }
        // 先拒绝根自身的符号链接 / Windows 重解析点，再做解析与特征判断。
        if crate::instances::reject_links_local(path).is_err() {
            return false;
        }
        let Ok(canonical) = std::fs::canonicalize(path) else {
            return false;
        };
        canonical.is_dir() && looks_like_game_root(&canonical)
    });
    candidates
}

/// 注册表里的实例根、当前默认游戏目录与启动器自带目录。
fn game_roots(app: &AppHandle) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();

    let registry = crate::instances::registry_from_text(&crate::instances::instances_get(
        app.clone(),
    ));
    for root in registry.roots {
        push_unique(&mut out, &mut seen, PathBuf::from(root));
    }
    if let Ok(path) = crate::app_paths::default_game_dir(app) {
        push_unique(&mut out, &mut seen, path);
    }
    if let Ok(path) = crate::app_paths::bundled_game_dir(app) {
        push_unique(&mut out, &mut seen, path);
    }
    retain_game_roots(out)
}

/// 所有实际游戏目录：实例根（非隔离）与 `versions/<id>/`（隔离，含命名实例）。
fn game_directories(app: &AppHandle) -> Vec<PathBuf> {
    game_directories_with(game_roots(app), |root, version| {
        // 清理路径不能复用启动路径的祖先链接校验：macOS /var、OneDrive 等
        // 合法重定向会让实例被静默跳过（见 `resource_directory_for_cleanup`）。
        crate::runtime_settings::resource_directory_for_cleanup(app, root, version).ok()
    })
}

/// 用调用方提供的解析器遍历实例根下的 `versions/`。
///
/// 抽成闭包参数，是为了让「祖先链接」「版本目录自身链接」等场景能在单元测试里
/// 不构造 `AppHandle` 直接覆盖。
fn game_directories_with(
    roots: impl IntoIterator<Item = PathBuf>,
    resolve: impl Fn(&Path, &str) -> Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();

    for root in roots {
        push_unique(&mut out, &mut seen, root.clone());

        let Ok(entries) = fs::read_dir(root.join("versions")) else {
            continue;
        };
        for entry in entries.flatten() {
            let directory = entry.path();
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            if crate::named_profiles::is_pending_pack(&directory) {
                continue;
            }
            let version = entry.file_name().to_string_lossy().into_owned();
            // 复用启动时的隔离解析，保证清理目标与游戏实际写入位置一致。
            if let Some(game) = resolve(&root, &version) {
                push_unique(&mut out, &mut seen, game);
            }
        }
    }

    out
}

/// `logs/` 里的普通文件全部视为可再生成日志；当前启动会话日志保留。
fn collect_log_directory(files: &mut Vec<PathBuf>, seen: &mut BTreeSet<String>, directory: &Path) {
    // 目录本身是链接时直接跳过：否则会跟随到游戏目录之外，删除目标目录里的文件。
    if is_link(directory) {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        if entry.file_name().to_string_lossy() == LAUNCHER_LOG_NAME {
            continue;
        }
        push_file(files, seen, entry.path());
    }
}

/// 崩溃报告目录只处理顶层普通文件，不进入子目录。
fn collect_directory_files(
    files: &mut Vec<PathBuf>,
    seen: &mut BTreeSet<String>,
    directory: &Path,
) {
    // 目录本身是链接时直接跳过：不能跟随到游戏目录之外删除目标目录里的文件。
    if is_link(directory) {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        push_file(files, seen, entry.path());
    }
}

fn is_game_root_junk(name: &str) -> bool {
    name == "authlib-injector.log"
        || ((name.starts_with("hs_err_pid") || name.starts_with("replay_pid"))
            && name.ends_with(".log"))
}

/// JVM 崩溃日志与 authlib-injector 日志落在游戏目录顶层，单独匹配。
fn collect_root_files(files: &mut Vec<PathBuf>, seen: &mut BTreeSet<String>, directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        if is_game_root_junk(&entry.file_name().to_string_lossy()) {
            push_file(files, seen, entry.path());
        }
    }
}

fn is_config_temp(name: &str) -> bool {
    (name.starts_with(".nowhere-") && name.ends_with(".part"))
        || name.starts_with("instances.json.tmp-")
}

/// 配置目录的原子写残留（实例注册表临时文件、导出 / 凭据写入暂存）。
fn collect_config_temp(files: &mut Vec<PathBuf>, seen: &mut BTreeSet<String>, directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        if is_config_temp(&entry.file_name().to_string_lossy()) {
            push_file(files, seen, entry.path());
        }
    }
}

/// 汇总给定游戏目录与配置目录下的待清理文件（纯文件系统遍历）。
///
/// 拆成纯函数是为了让「隔离实例 / 非隔离实例 / 配置目录残留」的组合
/// 能在单元测试里用临时目录直接覆盖，而不必构造 AppHandle。
fn collect_files_in(
    games: impl IntoIterator<Item = PathBuf>,
    config: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();

    for game in games {
        collect_log_directory(&mut files, &mut seen, &game.join("logs"));
        collect_directory_files(&mut files, &mut seen, &game.join("crash-reports"));
        collect_root_files(&mut files, &mut seen, &game);
    }

    if let Some(config) = config {
        collect_log_directory(&mut files, &mut seen, &config.join("logs"));
        collect_config_temp(&mut files, &mut seen, &config);
    }

    files
}

/// 解析应用的实际游戏目录与配置目录，再复用纯遍历逻辑。
fn collect_files(app: &AppHandle) -> Vec<PathBuf> {
    collect_files_in(
        game_directories(app),
        crate::app_paths::config_dir(app).ok(),
    )
}

fn scan_paths(paths: impl IntoIterator<Item = PathBuf>) -> GameJunkScan {
    let mut report = GameJunkScan::default();
    for path in paths {
        if let Ok(metadata) = fs::metadata(&path) {
            if metadata.is_file() {
                report.total_bytes += metadata.len();
                report.file_count += 1;
            }
        }
    }
    report
}

fn clean_paths(paths: impl IntoIterator<Item = PathBuf>) -> GameJunkCleanResult {
    let mut report = GameJunkCleanResult::default();
    for path in paths {
        let size = fs::metadata(&path)
            .map(|metadata| if metadata.is_file() { metadata.len() } else { 0 })
            .unwrap_or(0);
        match fs::remove_file(&path) {
            Ok(()) => {
                report.freed_bytes += size;
                report.removed_count += 1;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => report.failed_count += 1,
        }
    }
    report
}

/// 统计可清理的日志、崩溃报告与临时文件，不修改磁盘。
#[tauri::command]
pub async fn game_junk_scan(app: AppHandle) -> Result<GameJunkScan, String> {
    tokio::task::spawn_blocking(move || scan_paths(collect_files(&app)))
        .await
        .map_err(|error| format!("统计游戏垃圾失败：{error}"))
}

/// 删除扫描到的文件；被占用 / 无权限的文件跳过并计数。
#[tauri::command]
pub async fn game_junk_clean(app: AppHandle) -> Result<GameJunkCleanResult, String> {
    let guard = begin_clean()?;
    tokio::task::spawn_blocking(move || {
        let report = clean_paths(collect_files(&app));
        drop(guard);
        report
    })
    .await
    .map_err(|error| format!("清理游戏垃圾失败：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nowhere-junk-{tag}-{}", crate::store::random_token()))
    }

    #[test]
    fn log_directory_keeps_current_launcher_log() {
        let root = temp_root("logs");
        let logs = root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(logs.join(LAUNCHER_LOG_NAME), "current").unwrap();
        fs::write(logs.join("latest.log"), "latest").unwrap();
        fs::write(logs.join("2026-01-01-1.log.gz"), vec![0_u8; 32]).unwrap();
        fs::create_dir_all(logs.join("nested")).unwrap();

        let mut files = Vec::new();
        let mut seen = BTreeSet::new();
        collect_log_directory(&mut files, &mut seen, &logs);

        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|path| path.ends_with("latest.log")));
        assert!(files.iter().any(|path| path.ends_with("2026-01-01-1.log.gz")));
        assert!(!files.iter().any(|path| path.ends_with(LAUNCHER_LOG_NAME)));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn crash_reports_only_include_top_level_files() {
        let root = temp_root("crash");
        let crash = root.join("crash-reports");
        fs::create_dir_all(crash.join("nested")).unwrap();
        fs::write(crash.join("crash-2026-01-01.txt"), "boom").unwrap();
        fs::write(crash.join("nested/keep.txt"), "keep").unwrap();

        let mut files = Vec::new();
        let mut seen = BTreeSet::new();
        collect_directory_files(&mut files, &mut seen, &crash);

        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("crash-2026-01-01.txt"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn game_root_junk_matches_known_logs_only() {
        assert!(is_game_root_junk("hs_err_pid123.log"));
        assert!(is_game_root_junk("replay_pid123.log"));
        assert!(is_game_root_junk("authlib-injector.log"));
        assert!(!is_game_root_junk("hs_err_pid123.txt"));
        assert!(!is_game_root_junk("options.txt"));
        assert!(!is_game_root_junk("latest.log"));
    }

    #[test]
    fn config_temp_matches_atomic_write_leftovers() {
        assert!(is_config_temp(".nowhere-abc.part"));
        assert!(is_config_temp("instances.json.tmp-abc"));
        assert!(!is_config_temp("settings.json"));
        assert!(!is_config_temp("nowhere.settings.json"));
        assert!(!is_config_temp(".nowhere-abc.tmp"));
    }

    #[test]
    fn scan_and_clean_remove_only_collected_files() {
        let root = temp_root("scan");
        let logs = root.join("logs");
        let crash = root.join("crash-reports");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(&crash).unwrap();
        fs::write(logs.join(LAUNCHER_LOG_NAME), "current").unwrap();
        fs::write(logs.join("latest.log"), "latest").unwrap();
        fs::write(logs.join("old.log.gz"), vec![0_u8; 32]).unwrap();
        fs::write(crash.join("crash-1.txt"), "boom").unwrap();

        let mut files = Vec::new();
        let mut seen = BTreeSet::new();
        collect_log_directory(&mut files, &mut seen, &logs);
        collect_directory_files(&mut files, &mut seen, &crash);

        let scanned = scan_paths(files.clone());
        assert_eq!(scanned.file_count, 3);
        assert_eq!(scanned.total_bytes, 6 + 32 + 4);

        let cleaned = clean_paths(files);
        assert_eq!(cleaned.removed_count, 3);
        assert_eq!(cleaned.failed_count, 0);
        assert_eq!(cleaned.freed_bytes, scanned.total_bytes);
        assert!(logs.join(LAUNCHER_LOG_NAME).exists());
        assert!(!logs.join("latest.log").exists());
        assert!(!crash.join("crash-1.txt").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_game_like_roots_enter_cleanup_list() {
        let root = temp_root("roots");
        let project = root.join("project");
        let world = root.join("world");
        let versioned = root.join("versioned");
        let options_only = root.join("options-only");

        // 普通项目目录：只有 logs/，没有 versions/options.txt/level.dat。
        fs::create_dir_all(project.join("logs")).unwrap();
        fs::write(project.join("logs/latest.log"), "log").unwrap();
        // 存档目录：level.dat 是游戏目录特征。
        fs::create_dir_all(&world).unwrap();
        fs::write(world.join("level.dat"), "level").unwrap();
        // 标准 .minecraft：versions/ 是目录。
        fs::create_dir_all(versioned.join("versions")).unwrap();
        // 非隔离实例：顶层 options.txt。
        fs::create_dir_all(&options_only).unwrap();
        fs::write(options_only.join("options.txt"), "options").unwrap();

        let roots = retain_game_roots(vec![
            project.clone(),
            world.clone(),
            versioned.clone(),
            options_only.clone(),
            root.join("missing"),
        ]);
        assert_eq!(roots.len(), 3, "{roots:?}");
        let names: BTreeSet<String> = roots
            .iter()
            .filter_map(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        assert!(!names.contains("project"), "普通项目目录不应进入清理列表");
        assert!(names.contains("world"));
        assert!(names.contains("versioned"));
        assert!(names.contains("options-only"));

        // 对照：project 本身确实含可清理日志，说明下面的空结果来自过滤而不是路径无效。
        let unfiltered = collect_files_in(vec![project.clone()], None);
        assert!(
            unfiltered.iter().any(|path| path.ends_with("logs/latest.log")),
            "{unfiltered:?}"
        );
        // 过滤后的 roots 不含 project，因此同一份日志不会再进入清理列表。
        let filtered = collect_files_in(roots, None);
        assert!(filtered.is_empty(), "{filtered:?}");

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_ancestor_does_not_reject_real_game_root() {
        use std::os::unix::fs::symlink;

        let root = temp_root("ancestor-link");
        let real_parent = root.join("real");
        let game = real_parent.join("game");
        fs::create_dir_all(game.join("versions")).unwrap();
        fs::write(game.join("options.txt"), "options").unwrap();
        // 候选根经符号链接祖先定位，但自身是普通目录：macOS 的 /var → /private/var
        // 就是这种结构，不能因此把合法游戏目录过滤掉。
        let linked_parent = root.join("linked");
        symlink(&real_parent, &linked_parent).unwrap();

        let roots = retain_game_roots(vec![linked_parent.join("game")]);
        assert_eq!(roots.len(), 1, "{roots:?}");

        // 对照：根本身是符号链接时仍必须拒绝，避免清理目标被解析到别处。
        let linked_game = root.join("linked-game");
        symlink(&game, &linked_game).unwrap();
        assert!(retain_game_roots(vec![linked_game.clone()]).is_empty());

        std::fs::remove_file(&linked_game).unwrap();
        std::fs::remove_file(&linked_parent).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    /// 祖先链接不应让隔离实例的 logs 从清理列表中消失（M3 回归）。
    #[cfg(unix)]
    #[test]
    fn linked_ancestor_still_collects_isolated_version_logs() {
        use std::os::unix::fs::symlink;

        let base = temp_root("ancestor-cleanup");
        let real_root = base.join("real/instances/game");
        let version_dir = real_root.join("versions/1.20.1");
        fs::create_dir_all(version_dir.join("logs")).unwrap();
        fs::write(version_dir.join("logs/latest.log"), "latest").unwrap();
        fs::create_dir_all(base.join("real/instances")).unwrap();
        let linked_parent = base.join("linked");
        symlink(base.join("real/instances"), &linked_parent).unwrap();
        let linked_root = linked_parent.join("game");

        // 测试不构造 AppHandle：用清理专用的纯逻辑解析器替代设置读取。
        let games = game_directories_with([linked_root.clone()], |root, version| {
            crate::runtime_settings::cleanup_game_directory(root, version, true).ok()
        });
        assert_eq!(
            games,
            vec![linked_root.clone(), linked_root.join("versions/1.20.1")]
        );

        let files = collect_files_in(games, None);
        assert!(
            files
                .iter()
                .any(|path| path.ends_with("versions/1.20.1/logs/latest.log")),
            "{files:?}"
        );
        assert_eq!(clean_paths(files).removed_count, 1);
        assert!(!version_dir.join("logs/latest.log").exists());

        fs::remove_file(&linked_parent).unwrap();
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn linked_log_and_crash_directories_are_not_followed() {
        use std::os::unix::fs::symlink;

        let root = temp_root("linked-dirs");
        let outside = root.join("outside");
        let game = root.join("game");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(game.join("logs")).unwrap();
        fs::create_dir_all(game.join("crash-reports")).unwrap();
        fs::write(outside.join("outside.log"), "outside").unwrap();
        fs::write(outside.join("crash-outside.txt"), "outside").unwrap();
        // logs / crash-reports 换成指向游戏目录之外的链接：既不能被收集，也不能被删除。
        fs::remove_dir(game.join("logs")).unwrap();
        fs::remove_dir(game.join("crash-reports")).unwrap();
        symlink(&outside, game.join("logs")).unwrap();
        symlink(&outside, game.join("crash-reports")).unwrap();

        let files = collect_files_in([game.clone()], None);
        assert!(files.is_empty(), "{files:?}");
        let cleaned = clean_paths(files);
        assert_eq!(cleaned.removed_count, 0);
        assert_eq!(cleaned.failed_count, 0);
        assert!(outside.join("outside.log").exists());
        assert!(outside.join("crash-outside.txt").exists());

        fs::remove_file(game.join("logs")).unwrap();
        fs::remove_file(game.join("crash-reports")).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn linked_log_directory_is_not_followed_on_windows() {
        let root = temp_root("linked-dirs-win");
        let outside = root.join("outside");
        let game = root.join("game");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(game.join("logs")).unwrap();
        fs::write(outside.join("outside.log"), "outside").unwrap();
        fs::remove_dir(game.join("logs")).unwrap();
        // 目录符号链接需要开发者模式或管理员权限；环境不允许时跳过，不把测试变成
        // 权限探测。junction 同属重解析点，由同一处 `is_link` 判定拦截。
        if std::os::windows::fs::symlink_dir(&outside, game.join("logs")).is_err() {
            let _ = fs::remove_dir_all(&root);
            return;
        }

        let files = collect_files_in([game.clone()], None);
        assert!(files.is_empty(), "{files:?}");
        assert_eq!(clean_paths(files).removed_count, 0);
        assert!(outside.join("outside.log").exists());

        // Windows 的目录符号链接必须用 remove_dir 删除，remove_file 会报拒绝访问。
        fs::remove_dir(game.join("logs")).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn collect_files_combines_instances_and_config_without_touching_game_data() {
        let root = temp_root("tree");
        let isolated = root.join("versions/1.20.1");
        let config = root.join("config");

        // 隔离实例：日志、崩溃报告、JVM / authlib 日志各一份可清理文件。
        fs::create_dir_all(isolated.join("logs")).unwrap();
        fs::create_dir_all(isolated.join("crash-reports")).unwrap();
        fs::create_dir_all(isolated.join("mods")).unwrap();
        fs::create_dir_all(isolated.join("saves")).unwrap();
        fs::write(isolated.join("logs/latest.log"), "latest").unwrap();
        fs::write(isolated.join("logs").join(LAUNCHER_LOG_NAME), "current").unwrap();
        fs::write(isolated.join("crash-reports/crash-1.txt"), "boom").unwrap();
        fs::write(isolated.join("hs_err_pid123.log"), "jvm").unwrap();
        fs::write(isolated.join("authlib-injector.log"), "authlib").unwrap();
        fs::write(isolated.join("mods/keep.jar"), "jar").unwrap();
        fs::write(isolated.join("saves/keep.dat"), "save").unwrap();
        fs::write(isolated.join("options.txt"), "options").unwrap();

        // 配置目录：启动器日志与原子写残留。
        fs::create_dir_all(config.join("logs")).unwrap();
        fs::write(config.join("logs").join(LAUNCHER_LOG_NAME), "current").unwrap();
        fs::write(config.join("logs/nowhere-launcher.log.1"), "old").unwrap();
        fs::write(config.join("instances.json.tmp-abc"), "{}").unwrap();
        fs::write(config.join(".nowhere-export-x.part"), "zip").unwrap();
        fs::write(config.join("settings.json"), "{}").unwrap();

        let files = collect_files_in([isolated.clone()], Some(config.clone()));
        let names: Vec<String> = files
            .iter()
            .filter_map(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();

        assert_eq!(files.len(), 7, "{names:?}");
        assert!(names.contains(&"latest.log".to_string()));
        assert!(names.contains(&"crash-1.txt".to_string()));
        assert!(names.contains(&"hs_err_pid123.log".to_string()));
        assert!(names.contains(&"authlib-injector.log".to_string()));
        assert!(names.contains(&"nowhere-launcher.log.1".to_string()));
        assert!(names.contains(&"instances.json.tmp-abc".to_string()));
        assert!(names.contains(&".nowhere-export-x.part".to_string()));
        assert!(!names.iter().any(|name| name == "keep.jar"));
        assert!(!names.iter().any(|name| name == "keep.dat"));
        assert!(!names.iter().any(|name| name == "options.txt"));
        assert!(!names.iter().any(|name| name == "settings.json"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_directory_registered_twice_is_only_counted_once() {
        let root = temp_root("dedup");
        fs::create_dir_all(root.join("logs")).unwrap();
        fs::write(root.join("logs/latest.log"), "latest").unwrap();

        // 非隔离实例：多个版本共用同一资源根，必须去重而不是重复统计。
        let files = collect_files_in([root.clone(), root.clone()], None);
        assert_eq!(files.len(), 1);

        fs::remove_dir_all(root).unwrap();
    }
}
