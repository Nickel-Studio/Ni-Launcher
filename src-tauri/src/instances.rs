//! 实例注册表与本地版本、资源扫描。
//!
//! 注册表按有界 JSON 保存，缺失或损坏时回退空列表。
//! 程序旁目录、便携标记与旧配置优先级统一由 app_paths 决定。
//! 扫描不得发起网络请求；目录身份用于启动与管理，展示名不得替代真实 MC 版本。

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::store;

/// `instances.json` 大小上限（256KB），与 `settings.json` / `game.json` 一致。
const INSTANCES_MAX_BYTES: usize = 256 * 1024;

/// 默认实例目录名（官方启动器约定）。
const DEFAULT_INSTANCE_DIR: &str = crate::app_paths::GAME_DIRECTORY;

/// 注册表缺失 / 为空 / 损坏时的安全回退值。
const EMPTY_REGISTRY: &str = r#"{"roots":[],"active":null}"#;

/// 愚人节版本 id。这些版本的 `type` 与普通版本无异，只能按名字识别，图标应为金块。
const APRIL_FOOLS_VERSIONS: [&str; 7] = [
    "15w14a",
    "3D Shareware v1.34",
    "1.RV-Pre1",
    "20w14infinite",
    "22w13oneBlockAt",
    "23w13a_or_b",
    "24w14potato",
];

// loader / icon / type 的 IPC 取值。

const LOADER_VANILLA: &str = "vanilla";
const LOADER_FORGE: &str = "forge";
const LOADER_NEOFORGE: &str = "neoforge";
const LOADER_FABRIC: &str = "fabric";
const LOADER_OPTIFINE: &str = "optifine";
const LOADER_OPTIFABRIC: &str = "optifabric";
const LOADER_LITELOADER: &str = "liteloader";
const LOADER_LEGACY: &str = "legacy";
const LOADER_UNKNOWN: &str = "unknown";

const ICON_GRASS: &str = "grass";
const ICON_COMMAND_BLOCK: &str = "command_block";
const ICON_COBBLESTONE: &str = "cobblestone";
const ICON_ANVIL: &str = "anvil";
const ICON_NEOFORGE: &str = "neoforge";
const ICON_FABRIC: &str = "fabric";
const ICON_GRASS_PATH: &str = "grass_path";
const ICON_EGG: &str = "egg";
const ICON_GOLD_BLOCK: &str = "gold_block";
const ICON_REDSTONE_BLOCK: &str = "redstone_block";

/// 版本 JSON 读不出来时的 `type` 占位值。
const TYPE_UNKNOWN: &str = "unknown";

/// 单个版本描述 JSON 的大小上限；超过的“业务数据”不参与版本描述回退选择。
const VERSION_JSON_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// `inheritsFrom` 递归上限；与已访问链共同防止异常数据无限递归。
const MAX_INHERIT_DEPTH: usize = 8;

/// 1.5.2 及更早视为 legacy（旧版启动器 / Formatter 时代，没有现代加载器概念）。
const LEGACY_MAX_VERSION: (u32, u32, u32) = (1, 5, 2);

// ─────────────────────────────────────────────────────────────────────────────
// 实例注册表（instances.json）
// ─────────────────────────────────────────────────────────────────────────────

/// 实例注册表。`roots` 为版本根目录列表，`active` 为当前选中的根目录。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Registry {
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub labels: std::collections::BTreeMap<String, String>,
}

/// 解析注册表文本；空 / 非法 JSON → 默认值（返回 `{"roots":[],"active":null}`，不 panic、不 Err）。
pub fn registry_from_text(text: &str) -> Registry {
    if text.trim().is_empty() {
        return Registry::default();
    }
    serde_json::from_str::<Registry>(text).unwrap_or_default()
}

/// 序列化注册表；结构固定、理论不会失败，失败时回退到空注册表文本。
pub fn registry_to_text(registry: &Registry) -> String {
    serde_json::to_string(registry).unwrap_or_else(|_| EMPTY_REGISTRY.to_string())
}

pub(crate) fn reject_links(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        if let Ok(metadata) = std::fs::symlink_metadata(ancestor) {
            if metadata.file_type().is_symlink() { return Err("目录包含符号链接，请选择实际目录".into()); }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                const REPARSE_POINT: u32 = 0x400;
                if metadata.file_attributes() & REPARSE_POINT != 0 { return Err("目录包含重解析点，请选择实际目录".into()); }
            }
        }
    }
    Ok(())
}

/// 只检查路径**自身**是否为符号链接 / Windows 重解析点，不检查祖先。
///
/// 与 `reject_links` 的分工：实例选择 / 删除等需要「整条路径可信」的场景继续用
/// `reject_links`；清理与资源读取只关心目标自身，用本函数。macOS 的 `/var` 是
/// `/private/var` 的符号链接，Windows 的 OneDrive / junction 重定向目录也会让合法
/// 游戏目录带链接祖先，若连祖先一起拒绝，这些位置会被静默跳过。
///
/// 路径不存在时交给后续的 `is_dir` / `canonicalize` 判断，这里不重复报错，保持与
/// `reject_links` 一致的「读不到元数据就不下结论」行为。
pub(crate) fn reject_links_local(path: &Path) -> Result<(), String> {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() {
        return Err("路径不能是符号链接".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & REPARSE_POINT != 0 {
            return Err("路径不能是重解析点".into());
        }
    }
    Ok(())
}

#[tauri::command]
pub fn instance_create(app: AppHandle, parent: String) -> Result<String, String> {
    let parent = PathBuf::from(parent);
    if !parent.is_absolute() || !parent.is_dir() { return Err("请选择已存在的父文件夹".into()); }
    reject_links(&parent)?;
    let path = parent.join(DEFAULT_INSTANCE_DIR);
    std::fs::create_dir(&path).map_err(|e| format!("无法新建 {}：{e}。若目录已存在，请使用添加已有文件夹", path.display()))?;
    let dir = std::fs::canonicalize(&path).map_err(|e| format!("解析新建目录失败：{e}"))?.to_string_lossy().into_owned();
    let _guard = store::lock();
    let registry_path = store::instances_path(&app)?;
    let mut registry = read_registry(&registry_path);
    if !registry.roots.contains(&dir) { registry.roots.push(dir.clone()); }
    write_registry_atomic(&registry_path, &registry_to_text(&registry))?;
    Ok(dir)
}

#[tauri::command]
pub fn instance_root_rename(app: AppHandle, dir: String, label: String) -> Result<(), String> {
    if label.trim().is_empty() || label.len() > 256 || label.chars().any(char::is_control) { return Err("目录显示名称须为 1 至 256 字节，且不能包含控制字符".into()); }
    let _guard = store::lock();
    let path = store::instances_path(&app)?;
    let mut registry = read_registry(&path);
    if !registry.roots.contains(&dir) { return Err("目录不在实例列表中，请刷新后重试".into()); }
    registry.labels.insert(dir, label);
    write_registry_atomic(&path, &registry_to_text(&registry))
}

fn same_canonical_path(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CompareStringOrdinal(left: *const u16, left_len: i32, right: *const u16, right_len: i32, ignore_case: i32) -> i32;
        }
        let left: Vec<u16> = left.as_os_str().encode_wide().chain(Some(0)).collect();
        let right: Vec<u16> = right.as_os_str().encode_wide().chain(Some(0)).collect();
        const EQUAL: i32 = 2;
        // SAFETY: 两侧均为 canonicalize 返回的现存路径，UTF-16 缓冲区以 NUL 终止。
        unsafe { CompareStringOrdinal(left.as_ptr(), -1, right.as_ptr(), -1, 1) == EQUAL }
    }
    #[cfg(not(windows))]
    { left == right }
}

fn deletable_root(path: &Path, protected: &[PathBuf]) -> Result<PathBuf, String> {
    if !path.is_absolute() { return Err("请选择绝对路径的游戏目录".into()); }
    reject_links(path)?;
    let canonical = std::fs::canonicalize(path).map_err(|e| format!("解析目录失败：{e}"))?;
    if canonical.parent().is_none() || canonical.file_name().is_none() {
        return Err("不能删除磁盘根目录".into());
    }
    if !canonical.is_dir() { return Err("请选择游戏文件夹".into()); }
    for directory in protected {
        // 两侧统一 canonical 前缀；Windows 还按系统的 Unicode 不区分大小写规则比较。
        let directory = std::fs::canonicalize(directory)
            .map_err(|e| format!("无法核对启动器、配置或用户主目录，请重新打开启动器后重试：{e}"))?;
        if directory.ancestors().any(|ancestor| same_canonical_path(ancestor, &canonical)) {
            return Err("不能删除启动器、配置或用户主目录及其父级，请只选择游戏文件夹".into());
        }
    }
    Ok(canonical)
}

fn delete_registered_root(dir: &str, registry_path: &Path, game_path: &Path, protected: &[PathBuf]) -> Result<(), String> {
    let canonical = deletable_root(Path::new(dir), protected)?;
    let _guard = store::lock();
    let mut registry = read_registry(registry_path);
    if !registry.roots.iter().any(|root| root == dir) { return Err("目录不在实例列表中，请刷新后重试".into()); }
    crate::app_tasks::with_idle_directory(&canonical, || crate::launch_state::with_idle(&canonical, || {
        std::fs::remove_dir_all(&canonical).map_err(|e| format!("删除游戏目录失败 {}：{e}", canonical.display()))
    }))?;
    registry.roots.retain(|root| root != dir);
    registry.labels.remove(dir);
    if registry.active.as_deref() == Some(dir) { registry.active = registry.roots.first().cloned(); }
    write_registry_atomic(registry_path, &registry_to_text(&registry))?;
    let mut game: serde_json::Value = std::fs::read_to_string(game_path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_else(|| serde_json::json!({}));
    if game.get("gameDir").and_then(|v| v.as_str()) == Some(dir) {
        game["gameDir"] = registry.active.unwrap_or_default().into();
        crate::named_profiles::atomic_write(game_path, game.to_string().as_bytes())?;
    }
    Ok(())
}

#[tauri::command]
pub fn instance_root_delete(app: AppHandle, dir: String) -> Result<(), String> {
    let mut protected = vec![crate::app_paths::executable_directory_for(&app)?, crate::app_paths::config_dir(&app)?];
    #[cfg(target_os = "macos")]
    {
        // 数据根在 .app 旁；删除保护还需覆盖包内可执行目录，防止注册 .app 后删除自身。
        let executable = std::env::current_exe().map_err(|e| format!("无法定位启动器，请重新打开后重试：{e}"))?;
        protected.push(executable.parent().ok_or("无法定位启动器目录，请重新打开后重试")?.to_path_buf());
    }
    if let Some(home) = user_home_dir() { protected.push(home); }
    delete_registered_root(&dir, &store::instances_path(&app)?, &store::game_path(&app)?, &protected)
}

/// 读取注册表文件；IO 失败一律回退默认值。
fn read_registry(path: &Path) -> Registry {
    match std::fs::read_to_string(path) {
        Ok(text) => registry_from_text(&text),
        Err(_) => Registry::default(),
    }
}

/// 原子写入注册表：先写同目录临时文件，再 rename 覆盖目标。
///
/// 这样做是为了避免「写到一半被中断」留下半截 JSON——注册表一旦损坏虽然会安全回退，
/// 但用户配置的实例列表就丢了，代价比多一次 rename 高得多。
fn write_registry_atomic(path: &Path, text: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "无法定位实例注册表所在目录".to_string())?;
    let tmp = parent.join(format!("instances.json.tmp-{}", store::random_token()));

    std::fs::write(&tmp, text).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("写入实例注册表失败: {e}")
    })?;

    if std::fs::rename(&tmp, path).is_err() {
        // 极端情况（目标被占用等）下 rename 失败：清理临时文件后退化为直接写入，
        // 宁可牺牲原子性，也不要把「保存实例列表」整个卡死。
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(path, text).map_err(|e| format!("写入实例注册表失败: {e}"))?;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 命令
// ─────────────────────────────────────────────────────────────────────────────

/// 用户主目录：用于目录删除保护与已有 Minecraft 路径探测，不作为启动器自身目录。
pub fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// 启动器旁目录：普通可执行文件的父目录；macOS 应用包使用 `.app` 的父目录。
#[tauri::command]
pub fn launcher_self_dir() -> Result<String, String> {
    Ok(crate::app_paths::executable_directory()?.to_string_lossy().into_owned())
}

/// 创建 app_paths 选定的默认游戏目录并注册；已有 active 保持不变。
#[tauri::command]
pub fn instance_ensure_default(app: AppHandle) -> Result<String, String> {
    let _guard = store::lock();
    let dir = crate::app_paths::default_game_dir(&app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建游戏目录 {}：{e}。请将启动器放入有写入权限的文件夹后重试", dir.display()))?;
    let dir_str = dir.to_string_lossy().into_owned();
    let path = store::instances_path(&app)?;
    let mut registry = read_registry(&path);
    if !registry.roots.contains(&dir_str) {
        registry.roots.insert(0, dir_str.clone());
    }
    if registry.active.as_deref().is_none_or(str::is_empty) {
        registry.active = Some(dir_str.clone());
    }
    write_registry_atomic(&path, &registry_to_text(&registry))?;
    Ok(dir_str)
}

/// 启动器自带 / 便携包定义的游戏目录（不读取 `game.json`，也不创建目录）。
/// 前端用它识别「默认实例目录」并替换展示名；物理目录名保持 `.minecraft` 不变。
#[tauri::command]
pub fn instance_default_dir(app: AppHandle) -> Result<String, String> {
    Ok(crate::app_paths::bundled_game_dir(&app)?.to_string_lossy().into_owned())
}

/// 实例取名优化：只有「孤立的 `.minecraft`」才建议使用上级目录名。
///
/// 约束（与用户侧说明一致）：
/// 1. 当前目录名必须完全是 `.minecraft`；
/// 2. 必须存在上级目录，且上级不能再是磁盘根（避免取名成 `D:` 或 `/`）；
/// 3. 上级不能是用户主目录、用户目录容器或系统目录；
/// 4. 同一上级下只能有一个可识别的实例目录（含 `versions` 子目录的文件夹）。
/// 任一条不满足就返回 `None`，调用方保持现有默认取名方式。
fn suggest_instance_label(
    dir: &Path,
    home: Option<&Path>,
    system: &[PathBuf],
    home_roots: &[PathBuf],
) -> Option<String> {
    let canonical = std::fs::canonicalize(dir).ok()?;
    let name = canonical.file_name()?.to_str()?;
    #[cfg(windows)]
    let matches_minecraft = name.eq_ignore_ascii_case(crate::app_paths::GAME_DIRECTORY);
    #[cfg(not(windows))]
    let matches_minecraft = name == crate::app_paths::GAME_DIRECTORY;
    if !matches_minecraft {
        return None;
    }

    let parent = canonical.parent()?;
    // 上级的上级必须存在：排除 `D:\.minecraft`、`/.minecraft` 这类根目录。
    parent.parent()?;

    if let Some(home) = home.filter(|home| home.is_dir()) {
        let home = std::fs::canonicalize(home).ok()?;
        if same_canonical_path(parent, &home) {
            return None;
        }
    }
    for container in home_roots {
        let Ok(container) = std::fs::canonicalize(container) else { continue };
        if same_canonical_path(parent, &container) {
            return None;
        }
    }
    for directory in system {
        let Ok(directory) = std::fs::canonicalize(directory) else { continue };
        // 只拒绝系统目录本身及其内部路径；用户主目录里的游戏目录不受影响。
        if parent.ancestors().any(|ancestor| same_canonical_path(ancestor, &directory)) {
            return None;
        }
    }

    let label = parent.file_name()?.to_string_lossy().trim().to_string();
    if label.is_empty() || label.starts_with('.') || label == "." || label == ".." {
        return None;
    }

    // 「可识别」保持与扫描器同一判据：目录下存在 versions 文件夹。
    // 同级已有多个可读实例时，说明用户有意把它们放在一起，不再擅自改名。
    let mut readable_instances = 0usize;
    for entry in std::fs::read_dir(parent).ok()?.flatten() {
        let is_dir = entry.file_type().map(|file_type| file_type.is_dir()).unwrap_or(false);
        if is_dir && entry.path().join("versions").is_dir() {
            readable_instances += 1;
            if readable_instances > 1 {
                return None;
            }
        }
    }

    Some(label)
}

/// 平台系统目录：这些位置本身及其子目录都不应被用作实例显示名。
fn system_directories() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut out = Vec::new();
        // AppData 是官方启动器 / 各应用的配置根，不应被取成「Roaming」这类名字。
        for key in [
            "WINDIR",
            "SystemRoot",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramData",
            "APPDATA",
            "LOCALAPPDATA",
        ] {
            if let Some(value) = std::env::var_os(key).filter(|value| !value.is_empty()) {
                out.push(PathBuf::from(value));
            }
        }
        out
    }
    #[cfg(not(windows))]
    {
        ["/etc", "/usr", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/dev", "/proc", "/sys", "/var", "/opt", "/tmp", "/run", "/srv", "/mnt", "/media", "/System", "/Library", "/Applications"]
            .iter()
            .map(PathBuf::from)
            .collect()
    }
}

/// 用户目录容器：只拒绝容器本身（`C:\Users`、`/home`），容器内的游戏目录仍然允许。
fn home_container_directories() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut out = Vec::new();
        if let Some(drive) = std::env::var_os("SystemDrive").filter(|value| !value.is_empty()) {
            let drive = drive.to_string_lossy();
            let root = PathBuf::from(format!(
                "{}\\",
                drive.trim_end_matches(|ch| ch == '\\' || ch == '/')
            ));
            out.push(root.join("Users"));
        }
        out
    }
    #[cfg(not(windows))]
    {
        ["/home", "/Users"].iter().map(PathBuf::from).collect()
    }
}

/// 建议的实例显示名；不满足「孤立 `.minecraft`」条件时返回 `null`。
#[tauri::command]
pub fn instance_suggest_label(dir: String) -> Option<String> {
    let path = Path::new(dir.trim());
    if !path.is_absolute() {
        return None;
    }
    suggest_instance_label(
        path,
        user_home_dir().as_deref(),
        &system_directories(),
        &home_container_directories(),
    )
}

/// 读取实例注册表。文件缺失 / 为空 / 非法 JSON 一律回退为 `{"roots":[],"active":null}`，绝不 `Err`。
#[tauri::command]
pub fn instances_get(app: AppHandle) -> String {
    let path = match store::instances_path(&app) {
        Ok(path) => path,
        Err(_) => return EMPTY_REGISTRY.to_string(),
    };
    let mut registry = read_registry(&path);
    if let Ok(Some((root, manifest))) = crate::app_paths::portable_root(&app) {
        let portable = root.join(manifest.game_directory).to_string_lossy().into_owned();
        registry.roots.retain(|path| Path::new(path).is_dir() && path != &portable);
        registry.roots.insert(0, portable.clone());
        registry.active = Some(portable);
    }
    registry_to_text(&registry)
}

/// 写入实例注册表：>256KB 或非法 JSON 拒绝；合法则原子写入。
#[tauri::command]
pub fn instances_set(app: AppHandle, json: String) -> Result<(), String> {
    if json.len() > INSTANCES_MAX_BYTES {
        return Err(format!(
            "实例注册表内容过大（{} 字节，上限 {} 字节）",
            json.len(),
            INSTANCES_MAX_BYTES
        ));
    }
    // 校验合法性后原样透传（后端不解析业务字段）。
    serde_json::from_str::<serde_json::Value>(&json).map_err(|e| format!("非法 JSON: {e}"))?;
    let path = store::instances_path(&app)?;
    write_registry_atomic(&path, &json)
}

/// 扫描 `<dir>/versions/*/` 下的版本实例，返回 JSON 数组文本；目录不存在 → `"[]"`。
#[tauri::command]
pub fn instance_scan(dir: String) -> String {
    let entries = scan_instances(Path::new(dir.trim()));
    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceInstanceContext {
    resource_dir: String,
    display_name: String,
    minecraft_version: Option<String>,
    loader: String,
}

/// 社区安装与本地资源浏览复用启动时的隔离设置，不按实例类型猜测目录。
#[tauri::command]
pub fn resource_instance_context(
    app: AppHandle,
    dir: String,
    version_id: String,
) -> Result<Option<ResourceInstanceContext>, String> {
    crate::named_profiles::validate_name(&version_id)?;
    let root = Path::new(&dir);
    if !root.is_absolute() {
        return Err("请选择绝对路径的实例目录".into());
    }
    reject_links(&root.join("versions").join(&version_id))?;
    let Some(entry) = scan_instances(root).into_iter().find(|entry| entry.id == version_id && entry.has_json) else {
        return Ok(None);
    };
    let resource = crate::runtime_settings::resource_directory(&app, root, &version_id)?;
    Ok(Some(ResourceInstanceContext {
        resource_dir: resource.to_string_lossy().into_owned(),
        display_name: entry.display_name,
        minecraft_version: entry.minecraft_version,
        loader: entry.loader,
    }))
}

/// 扫描本地资源（`kind` ∈ mods | resourcepacks | shaderpacks | saves），返回 JSON 数组文本。
///
/// 对应文件夹不存在 / `kind` 不认识 → `"[]"`（不 `Err`，让前端资源页永远有结果可渲染）。
#[tauri::command]
pub fn resources_scan(dir: String, kind: String) -> String {
    let entries = scan_resources(Path::new(dir.trim()), &kind);
    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
}

// ─────────────────────────────────────────────────────────────────────────────
// 数据结构
// ─────────────────────────────────────────────────────────────────────────────

/// 实例扫描结果；目录身份与展示名分离。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceEntry {
    pub id: String,
    pub display_name: String,
    pub favorite: bool,
    pub favorite_at: Option<u64>,
    pub category: String,
    #[serde(rename = "type")]
    pub version_type: String,
    pub loader: String,
    pub loader_version: Option<String>,
    pub icon: String,
    /// 该实例的版本目录（`<dir>/versions/<name>`）。
    pub path: String,
    pub release_time: Option<String>,
    pub has_json: bool,
    /// 真实 Minecraft 版本；命名实例的 id 只表示目录身份。
    pub minecraft_version: Option<String>,
    pub named_profile: bool,
}

/// 一条本地资源（文件或文件夹）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceEntry {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    /// 修改时间（Unix 秒；取不到时为 0）。
    pub modified: u64,
    pub is_dir: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// 实例扫描
// ─────────────────────────────────────────────────────────────────────────────

fn scan_instances(dir: &Path) -> Vec<InstanceEntry> {
    let _reservation = crate::named_profiles::reservation_guard();
    let versions_dir = dir.join("versions");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&versions_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let version_dir = entry.path();
        if !version_dir.is_dir() || crate::named_profiles::is_pending_pack(&version_dir) {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let json_path = pick_version_json(&version_dir, &dir_name);
        let text = json_path
            .as_deref()
            .and_then(|path| std::fs::read_to_string(path).ok());
        if crate::named_profiles::is_pending_pack(&version_dir) {
            continue;
        }
        out.push(classify_version(&version_dir, &dir_name, text.as_deref()));
    }
    out.sort_by(|a, b| {
        b.favorite.cmp(&a.favorite)
            .then_with(|| a.favorite_at.unwrap_or(0).cmp(&b.favorite_at.unwrap_or(0)))
            .then_with(|| b.release_time.cmp(&a.release_time))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// 清理 BOM、字符串外注释与尾随逗号；字符串里的 URL、转义引号与逗号须原样保留。
pub(crate) fn sanitize_json_text(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut without_comments = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            without_comments.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                without_comments.push(ch);
            }
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                // 保留换行，避免把前后两行 token 拼在一起。
                for next in chars.by_ref() {
                    if next == '\n' {
                        without_comments.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = '\0';
                for next in chars.by_ref() {
                    if next == '\n' {
                        without_comments.push('\n');
                    }
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
            }
            _ => without_comments.push(ch),
        }
    }

    // 第二遍只处理字符串外的逗号：逗号后（跳过空白）是 `}` / `]` 时删除。
    let mut out = String::with_capacity(without_comments.len());
    let mut chars = without_comments.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }
        if ch == ',' {
            let next_non_whitespace = chars.clone().find(|next| !next.is_whitespace());
            if !matches!(next_non_whitespace, Some('}') | Some(']')) {
                out.push(',');
            }
            continue;
        }
        out.push(ch);
    }
    out
}

/// 标准 JSON 解析失败后，才尝试清理 BOM、注释与尾随逗号。
pub(crate) fn parse_json_lenient(text: &str) -> Option<serde_json::Value> {
    serde_json::from_str(text)
        .ok()
        .or_else(|| serde_json::from_str(&sanitize_json_text(text)).ok())
}

/// 回退描述须同时含 id / mainClass / type，避免选中校验报告等业务 JSON。
fn version_descriptor_has_identity(value: &serde_json::Value) -> bool {
    let has_id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| !id.trim().is_empty());
    has_id
        && value.get("mainClass").and_then(serde_json::Value::as_str).is_some()
        && value.get("type").and_then(serde_json::Value::as_str).is_some()
}

/// 同名 JSON 优先；外部实例文件名不一致时，按文件名排序选首个有效版本描述。
pub(crate) fn pick_version_json(version_dir: &Path, dir_name: &str) -> Option<PathBuf> {
    let exact = version_dir.join(format!("{dir_name}.json"));
    // 精确同名 JSON 永远优先；超过上限的文件按损坏处理，避免扫描时整份读入内存。
    if exact.is_file()
        && exact
            .metadata()
            .map(|meta| meta.len() <= VERSION_JSON_MAX_BYTES)
            .unwrap_or(false)
    {
        return Some(exact);
    }
    if crate::named_profiles::has_profile(version_dir) { return None; }
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(version_dir).ok()?.flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("json")))
        .filter(|path| {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
            if name.eq_ignore_ascii_case("nowhere.settings.json") || name.eq_ignore_ascii_case("gameData.json") { return false; }
            let Ok(metadata) = path.metadata() else { return false; };
            if metadata.len() > VERSION_JSON_MAX_BYTES { return false; }
            std::fs::read_to_string(path).ok()
                .and_then(|text| parse_json_lenient(&text))
                .is_some_and(|value| version_descriptor_has_identity(&value))
        }).collect();
    candidates.sort();
    candidates.into_iter().next()
}

/// 由版本目录 + 版本 JSON 文本归纳出实例信息。
fn classify_version(version_dir: &Path, dir_name: &str, json_text: Option<&str>) -> InstanceEntry {
    let path = version_dir.to_string_lossy().to_string();
    let parsed = json_text.and_then(parse_json_lenient);
    let Some(json) = parsed else {
        return InstanceEntry {
            id: dir_name.into(),
            display_name: dir_name.into(),
            favorite: false,
            favorite_at: None,
            category: String::new(),
            version_type: TYPE_UNKNOWN.into(),
            loader: LOADER_UNKNOWN.into(),
            loader_version: None,
            icon: ICON_REDSTONE_BLOCK.into(),
            path,
            release_time: None,
            has_json: false,
            minecraft_version: None,
            named_profile: crate::named_profiles::has_profile(version_dir),
        };
    };
    // 目录身份用于启动/删除/设置；不能把描述中的自定义 id 当成游戏版本。
    let metadata_id = json
        .get("id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(dir_name);
    let display_name = json
        .pointer("/nowhereCustom/name")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(dir_name)
        .to_string();
    let version_type = json
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("release")
        .to_string();
    let release_time = json
        .get("releaseTime")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let main_class = json.get("mainClass").and_then(|value| value.as_str());
    let inherits_from = json.get("inheritsFrom").and_then(|value| value.as_str());
    let libraries = library_names(&json);
    let root = version_dir.parent().and_then(Path::parent);
    // 外部配置只补齐缺失的版本 / 加载器信息，不能覆盖 JSON 中的证据。
    let external_setup = crate::version_settings::read_legacy_launcher_setup(version_dir);
    let external_minecraft = external_setup
        .as_ref()
        .and_then(|setup| setup.get("VersionVanillaName"))
        .filter(|value| minecraft_metadata_id(value).is_some())
        .map(str::to_owned);
    let profile = json.get("nowhereProfile");
    // 命名实例的运行档案优先于外层扫描描述，兼容旧描述缺失或残留的版本字段。
    let minecraft_version = version_dir.parent().and_then(Path::parent)
        .and_then(|root| crate::named_profiles::load(root, dir_name).ok().flatten())
        .map(|profile| profile.minecraft_version)
        .or_else(|| minecraft_version_from_metadata_in(
            root,
            &json,
            &libraries,
            external_minecraft.as_deref(),
            0,
            &mut Vec::new(),
        ));
    let mut loader = if let Some(profile) = profile {
        profile
            .get("loader")
            .and_then(|value| value.as_str())
            .unwrap_or(LOADER_VANILLA)
    } else {
        detect_loader(metadata_id, &version_type, main_class, inherits_from, &libraries)
    };
    // Setup.ini 是扫描缓存，只在 JSON 未识别加载器时补位。
    if matches!(loader, LOADER_VANILLA | LOADER_UNKNOWN | LOADER_LEGACY)
        && let Some(hint) = legacy_launcher_loader_hint(external_setup.as_ref())
    {
        loader = hint;
    }
    let loader_version = if let Some(profile) = profile {
        profile
            .get("loaderVersion")
            .and_then(|value| value.as_str())
            .filter(|_| loader != LOADER_VANILLA)
            .map(str::to_string)
    } else {
        extract_loader_version(loader, &libraries, &json, external_setup.as_ref())
    };
    let icon = pick_icon(loader, minecraft_version.as_deref().unwrap_or(metadata_id), &version_type, true);
    InstanceEntry {
        id: dir_name.into(),
        display_name,
        favorite: json.pointer("/nowhereCustom/favorite").and_then(|v| v.as_bool()).unwrap_or(false),
        favorite_at: json.pointer("/nowhereCustom/favoriteAt").and_then(|v| v.as_u64()),
        category: json.pointer("/nowhereCustom/category").and_then(|v| v.as_str()).unwrap_or("").into(),
        version_type,
        loader: loader.into(),
        loader_version,
        icon: crate::runtime_settings::read_local(version_dir.parent().and_then(Path::parent).unwrap_or(version_dir), dir_name).get("icon").and_then(|v| v.as_str()).filter(|v| !v.is_empty() && *v != "auto").unwrap_or(icon).into(),
        path,
        release_time,
        has_json: true,
        minecraft_version,
        named_profile: profile.is_some(),
    }
}

/// 只在通用 id / 继承引用中识别完整游戏版本，不从实例名称截取数字或补版本前缀。
fn minecraft_metadata_id(value: &str) -> Option<&str> {
    let value = value.trim();
    if is_april_fools(value) { return Some(value); }
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let bytes = value.as_bytes();
    if bytes.len() == 6 && bytes[2] == b'w'
        && bytes[..2].iter().all(u8::is_ascii_digit)
        && bytes[3..5].iter().all(u8::is_ascii_digit)
        && bytes[5].is_ascii_lowercase()
    {
        return Some(value);
    }
    if ["rd-", "inf-"].iter().any(|prefix| value.strip_prefix(prefix).is_some_and(digits)) {
        return Some(value);
    }
    let numeric = ["a", "b", "c"].iter().find_map(|prefix| value.strip_prefix(prefix)).unwrap_or(value);
    let (base, suffix) = numeric.split_once('-').unwrap_or((numeric, ""));
    let (base, revision) = base.split_once('_').unwrap_or((base, ""));
    let parts: Vec<_> = base.split('.').collect();
    let valid_suffix = suffix.is_empty() || ["pre", "rc", "snapshot-"].iter().any(|prefix| {
        suffix.strip_prefix(prefix).is_some_and(|tail| digits(tail.strip_prefix('-').unwrap_or(tail)))
    });
    (matches!(parts.len(), 2 | 3) && parts.iter().all(|part| digits(part))
        && (revision.is_empty() || digits(revision)) && valid_suffix).then_some(value)
}
#[cfg(test)]
fn minecraft_version_from_metadata(json: &serde_json::Value, libraries: &[String]) -> Option<String> {
    minecraft_version_from_metadata_in(None, json, libraries, None, 0, &mut Vec::new())
}

/// MC 版本优先取运行档案、clientVersion 与继承链，再回退外部 VersionVanillaName。
/// VersionVanilla 是内部编码，不得当作 MC 版本；jar / id 须通过完整版本号校验。
fn minecraft_version_from_metadata_in(
    root: Option<&Path>,
    json: &serde_json::Value,
    libraries: &[String],
    external_version: Option<&str>,
    depth: usize,
    chain: &mut Vec<String>,
) -> Option<String> {
    json.pointer("/nowhereProfile/minecraftVersion")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| json.get("clientVersion").and_then(serde_json::Value::as_str)
            .and_then(minecraft_metadata_id).map(str::to_owned))
        .or_else(|| json.get("inheritsFrom").and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| inherited_minecraft_version(root, value, depth, chain)))
        .or_else(|| external_version
            .and_then(minecraft_metadata_id)
            .map(str::to_owned))
        .or_else(|| game_argument_value(json, "--fml.mcVersion")
            .and_then(|value| minecraft_metadata_id(&value).map(str::to_owned)))
        .or_else(|| json.get("jar").and_then(serde_json::Value::as_str)
            .and_then(minecraft_metadata_id).map(str::to_owned))
        .or_else(|| library_artifact_version(libraries, "net.fabricmc:intermediary"))
        .or_else(|| library_artifact_version(libraries, "org.quiltmc:intermediary"))
        .or_else(|| library_artifact_version(libraries, "net.minecraftforge:forge")
            .and_then(|value| value.split_once('-').map(|(minecraft, _)| minecraft.to_owned())))
        .or_else(|| json.get("id").and_then(serde_json::Value::as_str)
            .and_then(minecraft_metadata_id).map(str::to_owned))
}

/// 解析 `inheritsFrom`：值本身是 MC 版本时直接使用；否则读取父版本 JSON 继续归纳。
/// 父版本缺失只代表依赖不完整，不会把子实例判为版本损坏。
fn inherited_minecraft_version(
    root: Option<&Path>,
    inherits_from: &str,
    depth: usize,
    chain: &mut Vec<String>,
) -> Option<String> {
    // 父版本名本身就是 MC 版本时直接采用；这一步不受递归深度限制。
    if let Some(version) = minecraft_metadata_id(inherits_from) {
        return Some(version.to_owned());
    }
    // 深度上限与已访问链共同兜底：损坏数据里的 A→B→A 循环不会无限递归。
    if depth >= MAX_INHERIT_DEPTH || chain.iter().any(|seen| seen == inherits_from) {
        return None;
    }
    let root = root?;
    let json_path = root
        .join("versions")
        .join(inherits_from)
        .join(format!("{inherits_from}.json"));
    let text = std::fs::read_to_string(json_path).ok()?;
    let json = parse_json_lenient(&text)?;
    let libraries = library_names(&json);
    chain.push(inherits_from.to_owned());
    let resolved = minecraft_version_from_metadata_in(
        Some(root),
        &json,
        &libraries,
        None,
        depth + 1,
        chain,
    );
    chain.pop();
    resolved
}

/// 取 `arguments.game` 中 `--fml.*` 这类“参数名 + 值”的下一项。
fn game_argument_value(json: &serde_json::Value, name: &str) -> Option<String> {
    json.pointer("/arguments/game")?
        .as_array()?
        .windows(2)
        .find_map(|pair| {
            (pair[0].as_str() == Some(name))
                .then(|| pair[1].as_str().map(str::to_owned))
                .flatten()
        })
}

/// 外部 Setup.ini 的加载器提示仅用于 JSON 缺少证据时。
fn legacy_launcher_loader_hint(setup: Option<&crate::version_settings::LegacyLauncherSetup>) -> Option<&'static str> {
    let setup = setup?;
    if setup.get("VersionNeoForge").is_some() {
        Some(LOADER_NEOFORGE)
    } else if setup.get("VersionFabric").is_some() {
        Some(LOADER_FABRIC)
    } else if setup.get("VersionForge").is_some() {
        Some(LOADER_FORGE)
    } else if setup.get("VersionOptiFine").is_some() {
        Some(LOADER_OPTIFINE)
    } else if setup.bool_value("VersionLiteLoader") == Some(true) {
        Some(LOADER_LITELOADER)
    } else {
        None
    }
}

fn library_names(json: &serde_json::Value) -> Vec<String> {
    json.get("libraries")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l.get("name").and_then(|n| n.as_str()))
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// 判定加载器类型。
///
/// 线索来源：`id` / `mainClass` / `inheritsFrom` 的文本特征 + `libraries[].name` 的坐标证据。
/// 顺序很关键：`neoforge` 名字里含 `forge`、`optifabric` 名字里含 `fabric`，
/// 必须先判定更具体的那个，否则会误判。
fn detect_loader(
    id: &str,
    version_type: &str,
    main_class: Option<&str>,
    inherits_from: Option<&str>,
    libraries: &[String],
) -> &'static str {
    let hint = format!(
        "{id} {} {}",
        main_class.unwrap_or(""),
        inherits_from.unwrap_or("")
    )
    .to_lowercase();
    let has_library = |prefix: &str| libraries.iter().any(|n| n.starts_with(prefix));
    let has_forge_library = libraries.iter().any(|n| n.starts_with("net.minecraftforge:"));
    let has_neoforge_library = libraries
        .iter()
        .any(|n| n.starts_with("net.neoforged:") || n.starts_with("net.neoforge:"));

    if hint.contains("neoforge") || has_neoforge_library {
        return LOADER_NEOFORGE;
    }
    if hint.contains("optifabric") || has_library("net.optifabric:") {
        return LOADER_OPTIFABRIC;
    }
    // 实例列表的现有分类把 Quilt 归为 fabric；实际库坐标仍保留 Quilt。
    if has_library("net.fabricmc:fabric-loader")
        || has_library("org.quiltmc:quilt-loader")
        || hint.contains("fabric")
        || hint.contains("fabricmc")
        || hint.contains("quilt")
    {
        return LOADER_FABRIC;
    }
    // 现代 Forge 可能没有 forge artifact，须同时识别 net.minecraftforge 命名空间的依赖。
    if has_forge_library || hint.contains("forge") {
        return LOADER_FORGE;
    }
    if hint.contains("optifine") || has_library("optifine:") {
        return LOADER_OPTIFINE;
    }
    if hint.contains("liteloader") || has_library("com.mumfrey:") {
        return LOADER_LITELOADER;
    }
    // 兜底：1.5.2 及更早的老版本。
    if let Some(v) = parse_version_number(id) {
        if v <= LEGACY_MAX_VERSION {
            return LOADER_LEGACY;
        }
    }
    // 没有加载器特征：能识别出版本号、或 type 属于原版取值，即 vanilla，否则 unknown。
    if parse_version_number(id).is_some()
        || matches!(
            version_type,
            "release" | "snapshot" | "old_beta" | "old_alpha"
        )
    {
        return LOADER_VANILLA;
    }
    LOADER_UNKNOWN
}

/// 图标映射。
///
/// 1. loader 优先（装了加载器的实例一眼可辨）；
/// 2. 愚人节版本名字命中即金块（优先于 type，但低于 loader）；
/// 3. 其余按 type 区分原版；读不出 JSON 则红石块兜底。
///
/// `optifabric` 不在契约的图标映射表内，与 vanilla 走同一条按 type 的路径。
fn pick_icon(loader: &str, id: &str, version_type: &str, has_json: bool) -> &'static str {
    match loader {
        LOADER_FORGE => return ICON_ANVIL,
        LOADER_NEOFORGE => return ICON_NEOFORGE,
        LOADER_FABRIC => return ICON_FABRIC,
        LOADER_OPTIFINE => return ICON_GRASS_PATH,
        LOADER_LITELOADER => return ICON_EGG,
        _ => {}
    }
    if !has_json {
        return ICON_REDSTONE_BLOCK;
    }
    if is_april_fools(id) {
        return ICON_GOLD_BLOCK;
    }
    match version_type {
        "release" => ICON_GRASS,
        "snapshot" => ICON_COMMAND_BLOCK,
        "old_beta" | "old_alpha" => ICON_COBBLESTONE,
        _ => ICON_REDSTONE_BLOCK,
    }
}

fn is_april_fools(id: &str) -> bool {
    let id = id.trim().to_lowercase();
    APRIL_FOOLS_VERSIONS.iter().any(|v| v.to_lowercase() == id)
}

/// 从 `id` 中提取首个形如 `1.20.1` 的版本号（`b1.7.3` / `1.5.2` 均可）。
///
/// 要求至少两段（含 `.`），避免把 `1.RV-Pre1`、`15w14a` 这类快照名误判成老版本。
fn parse_version_number(raw: &str) -> Option<(u32, u32, u32)> {
    let s = raw.trim().to_ascii_lowercase();
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let token: String = s[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let token = token.trim_end_matches('.');
    if !token.contains('.') {
        return None;
    }
    let mut parts = token.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

/// 现代 Forge / NeoForge 优先使用启动参数；库坐标次之，外部扫描缓存最后回退。
fn extract_loader_version(
    loader: &str,
    libraries: &[String],
    json: &serde_json::Value,
    external_setup: Option<&crate::version_settings::LegacyLauncherSetup>,
) -> Option<String> {
    let from_arguments = |name: &str| game_argument_value(json, name);
    let from_external = |key: &str| external_setup.and_then(|setup| setup.get(key)).map(str::to_owned);
    match loader {
        LOADER_FORGE => from_arguments("--fml.forgeVersion")
            .or_else(|| library_artifact_version(libraries, "net.minecraftforge:forge")
                .map(|value| strip_mc_prefix(&value)))
            .or_else(|| library_artifact_version(libraries, "net.minecraftforge:fmlloader")
                .map(|value| strip_mc_prefix(&value)))
            .or_else(|| from_external("VersionForge")),
        LOADER_NEOFORGE => from_arguments("--fml.neoForgeVersion")
            .or_else(|| library_artifact_version(libraries, "net.neoforged:neoforge")
                .map(|value| strip_mc_prefix(&value)))
            .or_else(|| from_external("VersionNeoForge")),
        LOADER_FABRIC => library_artifact_version(libraries, "net.fabricmc:fabric-loader")
            .or_else(|| library_artifact_version(libraries, "org.quiltmc:quilt-loader"))
            .or_else(|| from_external("VersionFabric")),
        LOADER_OPTIFABRIC => library_artifact_version(libraries, "net.optifabric:optifabric"),
        LOADER_OPTIFINE => library_artifact_version(libraries, "optifine:OptiFine")
            .or_else(|| from_external("VersionOptiFine")),
        LOADER_LITELOADER => library_artifact_version(libraries, "com.mumfrey:liteloader"),
        _ => None,
    }
}

/// 取 `<group>:<artifact>:<version>` 里的 `<version>`（去掉可能的 `@jar` 后缀）。
fn library_artifact_version(libraries: &[String], coord: &str) -> Option<String> {
    let needle = format!("{coord}:");
    libraries
        .iter()
        .find(|n| n.starts_with(&needle))
        .map(|n| {
            n[needle.len()..]
                .split(':')
                .next()
                .unwrap_or("")
                .split('@')
                .next()
                .unwrap_or("")
                .to_string()
        })
        .filter(|v| !v.is_empty())
}

/// 去掉 `1.20.1-` 这样的 MC 版本前缀，保留加载器自身版本。
fn strip_mc_prefix(artifact: &str) -> String {
    if let Some((head, tail)) = artifact.split_once('-') {
        if !tail.is_empty() && parse_version_number(head).is_some() {
            return tail.to_string();
        }
    }
    artifact.to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// 资源扫描
// ─────────────────────────────────────────────────────────────────────────────

/// `kind` → 目录名（`mods` | `resourcepacks` | `shaderpacks` | `saves`）。
fn resource_folder(kind: &str) -> Option<&'static str> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "mods" => Some("mods"),
        "resourcepacks" => Some("resourcepacks"),
        "shaderpacks" => Some("shaderpacks"),
        "saves" => Some("saves"),
        _ => None,
    }
}

fn scan_resources(dir: &Path, kind: &str) -> Vec<ResourceEntry> {
    let Some(folder) = resource_folder(kind) else {
        return Vec::new();
    };
    let target = dir.join(folder);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&target) else {
        return out;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // 跳过隐藏文件（macOS 的 .DS_Store、编辑器临时文件等），避免污染资源列表。
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let is_dir = meta.is_dir();
        out.push(ResourceEntry {
            name,
            path: entry.path().to_string_lossy().to_string(),
            // 目录的 size 在多数文件系统上没有意义，统一给 0。
            size_bytes: if is_dir { 0 } else { meta.len() },
            modified: modified_secs(&meta),
            is_dir,
        });
    }

    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn modified_secs(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn libs(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// 测试用临时目录（唯一名，避免并行测试互相干扰）。
    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nowhere-test-{tag}-{}", store::random_token()))
    }

    #[test]
    fn detects_loader_table() {
        // forge：库坐标 + id 文本；仅有 id 时也要能识别
        let forge = libs(&["net.minecraftforge:forge:1.20.1-47.2.0"]);
        assert_eq!(
            detect_loader(
                "1.20.1-forge-47.2.0",
                "release",
                None,
                Some("1.20.1"),
                &forge
            ),
            LOADER_FORGE
        );
        assert_eq!(
            detect_loader("1.20.1-forge-47.2.0", "release", None, None, &[]),
            LOADER_FORGE
        );

        // neoforge：不能被 forge 抢走
        let neo = libs(&["net.neoforged:neoforge:20.4.237"]);
        assert_eq!(
            detect_loader("1.20.1-neoforge-20.4.237", "release", None, None, &neo),
            LOADER_NEOFORGE
        );
        assert_eq!(
            detect_loader("1.20.1-neoforge-20.4.237", "release", None, None, &[]),
            LOADER_NEOFORGE
        );

        // fabric：库坐标 / id 文本
        let fabric = libs(&["net.fabricmc:fabric-loader:0.15.11"]);
        assert_eq!(
            detect_loader(
                "fabric-loader-0.15.11-1.20.1",
                "release",
                None,
                None,
                &fabric
            ),
            LOADER_FABRIC
        );
        assert_eq!(
            detect_loader("fabric-loader-0.15.11-1.20.1", "release", None, None, &[]),
            LOADER_FABRIC
        );

        // optifabric：不能被 fabric 抢走
        assert_eq!(
            detect_loader("1.20.1-optifabric-1.5.0", "release", None, None, &[]),
            LOADER_OPTIFABRIC
        );
        assert_eq!(
            detect_loader(
                "1.20.1-optifabric-1.5.0",
                "release",
                None,
                None,
                &libs(&["net.optifabric:optifabric:1.5.0"])
            ),
            LOADER_OPTIFABRIC
        );

        // optifine / liteloader
        assert_eq!(
            detect_loader("OptiFine_1.20.1_HD_U_I6", "release", None, None, &[]),
            LOADER_OPTIFINE
        );
        assert_eq!(
            detect_loader("1.12.2-LiteLoader1.12.2", "release", None, None, &[]),
            LOADER_LITELOADER
        );

        // vanilla / legacy / unknown
        assert_eq!(
            detect_loader("1.20.1", "release", None, None, &[]),
            LOADER_VANILLA
        );
        assert_eq!(
            detect_loader("1.5.2", "release", None, None, &[]),
            LOADER_LEGACY
        );
        assert_eq!(
            detect_loader("1.4.7", "release", None, None, &[]),
            LOADER_LEGACY
        );
        assert_eq!(
            detect_loader("my-custom-instance", "custom", None, None, &[]),
            LOADER_UNKNOWN
        );
    }

    #[test]
    fn picks_icon_with_loader_priority() {
        // loader 优先于 type（含愚人节 id）
        assert_eq!(
            pick_icon(LOADER_FORGE, "20w14infinite", "snapshot", true),
            ICON_ANVIL
        );
        assert_eq!(
            pick_icon(LOADER_NEOFORGE, "1.21.1", "release", true),
            ICON_NEOFORGE
        );
        assert_eq!(
            pick_icon(LOADER_FABRIC, "24w14potato", "snapshot", true),
            ICON_FABRIC
        );
        assert_eq!(
            pick_icon(LOADER_OPTIFINE, "1.20.1", "release", true),
            ICON_GRASS_PATH
        );
        assert_eq!(
            pick_icon(LOADER_LITELOADER, "1.12.2", "release", true),
            ICON_EGG
        );

        // 愚人节集合（优先于 type）
        assert_eq!(
            pick_icon(LOADER_VANILLA, "20w14infinite", "snapshot", true),
            ICON_GOLD_BLOCK
        );
        assert_eq!(
            pick_icon(LOADER_VANILLA, "23w13a_or_b", "snapshot", true),
            ICON_GOLD_BLOCK
        );
        assert_eq!(
            pick_icon(LOADER_VANILLA, "3D Shareware v1.34", "release", true),
            ICON_GOLD_BLOCK
        );
        assert_eq!(
            pick_icon(LOADER_LEGACY, "1.RV-Pre1", "release", true),
            ICON_GOLD_BLOCK
        );

        // 按 type
        assert_eq!(
            pick_icon(LOADER_VANILLA, "1.21.4", "release", true),
            ICON_GRASS
        );
        assert_eq!(
            pick_icon(LOADER_VANILLA, "1.21.5-pre1", "snapshot", true),
            ICON_COMMAND_BLOCK
        );
        assert_eq!(
            pick_icon(LOADER_VANILLA, "b1.7.3", "old_beta", true),
            ICON_COBBLESTONE
        );
        assert_eq!(
            pick_icon(LOADER_VANILLA, "a1.2.6", "old_alpha", true),
            ICON_COBBLESTONE
        );
        assert_eq!(
            pick_icon(LOADER_LEGACY, "1.5.2", "release", true),
            ICON_GRASS
        );

        // 解析失败兜底
        assert_eq!(
            pick_icon(LOADER_UNKNOWN, "corrupted", TYPE_UNKNOWN, false),
            ICON_REDSTONE_BLOCK
        );
        // 无法识别的 type
        assert_eq!(
            pick_icon(LOADER_VANILLA, "weird", "custom", true),
            ICON_REDSTONE_BLOCK
        );
    }

    #[test]
    fn registry_falls_back_on_corruption() {
        for text in [
            "",
            "   ",
            "{not json",
            "{\"roots\":\"nope\"}",
            "[1,2,3]",
            "null",
        ] {
            let registry = registry_from_text(text);
            assert!(registry.roots.is_empty(), "roots 应为空: {text}");
            assert!(registry.active.is_none(), "active 应为 None: {text}");
            assert_eq!(registry_to_text(&registry), EMPTY_REGISTRY);
        }

        let registry = registry_from_text(r#"{"roots":["C:/a","C:/b"],"active":"C:/b"}"#);
        assert_eq!(registry.roots, vec!["C:/a".to_string(), "C:/b".to_string()]);
        assert_eq!(registry.active.as_deref(), Some("C:/b"));
    }

    #[test]
    fn classify_version_handles_missing_and_corrupt_json() {
        let root = Path::new("C:/mc/versions/1.20.1");

        let missing = classify_version(root, "1.20.1", None);
        assert_eq!(missing.id, "1.20.1");
        assert!(!missing.has_json);
        assert_eq!(missing.loader, LOADER_UNKNOWN);
        assert_eq!(missing.version_type, TYPE_UNKNOWN);
        assert_eq!(missing.icon, ICON_REDSTONE_BLOCK);
        assert!(missing.release_time.is_none());

        let corrupt = classify_version(root, "1.20.1", Some("{oops"));
        assert!(!corrupt.has_json);
        assert_eq!(corrupt.loader, LOADER_UNKNOWN);
        assert_eq!(corrupt.icon, ICON_REDSTONE_BLOCK);
    }

    #[test]
    fn classify_version_extracts_metadata() {
        let root = Path::new("C:/mc/versions/1.20.1-forge-47.2.0");
        let json = r#"{
            "id": "1.20.1-forge-47.2.0",
            "type": "release",
            "releaseTime": "2023-06-12T13:25:51+00:00",
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "inheritsFrom": "1.20.1",
            "libraries": [{"name": "net.minecraftforge:forge:1.20.1-47.2.0"}]
        }"#;
        let entry = classify_version(root, "1.20.1-forge-47.2.0", Some(json));
        assert_eq!(entry.id, "1.20.1-forge-47.2.0");
        assert_eq!(entry.version_type, "release");
        assert_eq!(entry.loader, LOADER_FORGE);
        assert_eq!(entry.loader_version.as_deref(), Some("47.2.0"));
        assert_eq!(entry.icon, ICON_ANVIL);
        assert_eq!(
            entry.release_time.as_deref(),
            Some("2023-06-12T13:25:51+00:00")
        );
        assert!(entry.has_json);
        assert_eq!(entry.path, root.to_string_lossy());

        // id 缺失时退化为目录名
        let entry = classify_version(
            Path::new("C:/mc/versions/1.21.4"),
            "1.21.4",
            Some("{\"type\":\"release\"}"),
        );
        assert_eq!(entry.id, "1.21.4");
        assert_eq!(entry.loader, LOADER_VANILLA);
        assert_eq!(entry.icon, ICON_GRASS);
    }

    #[test]
    fn renamed_vanilla_keeps_metadata_version_and_directory_identity() {
        let entry = classify_version(
            Path::new("C:/mc/versions/My survival"),
            "My survival",
            Some(r#"{"id":"1.19.2","type":"release"}"#),
        );
        assert_eq!(entry.id, "My survival");
        assert_eq!(entry.minecraft_version.as_deref(), Some("1.19.2"));
        assert!(!entry.named_profile);
    }

    #[test]
    fn community_version_uses_explicit_metadata_not_display_or_directory_name() {
        for minecraft in ["1.26.3", "26.3", "26.3-snapshot-1", "3D Shareware v1.34"] {
            let json = serde_json::json!({
                "id": "arbitrary-directory",
                "nowhereCustom": {"name": "Foo 26.3"},
                "nowhereProfile": {"minecraftVersion": minecraft, "loader": "fabric"},
            });
            let entry = classify_version(Path::new("community-fixture/versions/arbitrary-directory"),
                "arbitrary-directory", Some(&json.to_string()));
            assert_eq!(entry.display_name, "Foo 26.3");
            assert_eq!(entry.id, "arbitrary-directory");
            assert_eq!(entry.minecraft_version.as_deref(), Some(minecraft));
        }
    }

    #[test]
    fn community_version_unknown_metadata_never_falls_back_to_instance_name() {
        for json in [
            serde_json::json!({"id": "Foo 26.3"}),
            serde_json::json!({"nowhereCustom": {"name": "Foo 26.3"}}),
            serde_json::json!({"id": "Foo 26.3", "inheritsFrom": "Other 1.26.3"}),
            serde_json::json!({"id": "Foo 26.3", "nowhereProfile": {"minecraftVersion": "  "}}),
        ] {
            let entry = classify_version(Path::new("community-fixture/versions/26.3"),
                "26.3", Some(&json.to_string()));
            assert_eq!(entry.minecraft_version, None, "{json}");
        }
    }

    #[test]
    fn community_version_preserves_metadata_ids_and_legacy_loader_evidence() {
        for minecraft in ["1.26.3", "26.3", "25w45a", "1.21.1-pre1", "26.3-rc-1", "26.3-snapshot-1", "b1.7.3", "a1.2.6", "c0.30_01", "rd-132211", "3D Shareware v1.34"] {
            let json = serde_json::json!({"id": minecraft});
            assert_eq!(minecraft_version_from_metadata(&json, &[]).as_deref(), Some(minecraft));
        }
        let json = serde_json::json!({
            "id": "Foo 26.3", "inheritsFrom": "26.3",
            "nowhereProfile": {"minecraftVersion": ""},
        });
        assert_eq!(minecraft_version_from_metadata(&json, &[]).as_deref(), Some("26.3"));
        let json = serde_json::json!({"id": "Foo 26.3", "inheritsFrom": "base-instance"});
        let libraries = libs(&["net.fabricmc:intermediary:1.26.3"]);
        assert_eq!(minecraft_version_from_metadata(&json, &libraries).as_deref(), Some("1.26.3"));
    }

    #[test]
    fn community_version_reads_named_profile_when_outer_descriptor_is_stale() -> Result<(), String> {
        let root = temp_dir("community-version");
        let (_, directory) = crate::named_profiles::reserve(&root, "Foo 26.3", "1.26.3", None, "")?;
        for descriptor in [
            serde_json::json!({"id": "Foo 26.3", "nowhereCustom": {"name": "My pack"}}),
            serde_json::json!({"id": "Foo 26.3", "inheritsFrom": "Foo 26.3", "nowhereProfile": {"minecraftVersion": "Foo 26.3"}}),
        ] {
            std::fs::write(directory.join("Foo 26.3.json"), descriptor.to_string()).map_err(|error| error.to_string())?;
            let entries = scan_instances(&root);
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].minecraft_version.as_deref(), Some("1.26.3"));
            assert_eq!(entries[0].id, "Foo 26.3");
        }
        std::fs::write(directory.join("nowhere.profile.json"), "{broken").map_err(|error| error.to_string())?;
        std::fs::write(directory.join("Foo 26.3.json"), r#"{"id":"Foo 26.3"}"#).map_err(|error| error.to_string())?;
        assert_eq!(scan_instances(&root)[0].minecraft_version, None);
        std::fs::remove_dir_all(root).map_err(|error| error.to_string())
    }

    #[test]
    fn registry_labels_roundtrip_without_changing_legacy_empty_shape() {
        let mut registry = Registry::default();
        assert_eq!(registry_to_text(&registry), EMPTY_REGISTRY);
        registry.labels.insert("C:/mc".into(), "Survival".into());
        assert_eq!(registry_from_text(&registry_to_text(&registry)), registry);
    }

    #[test]
    fn suggests_parent_name_only_for_lone_minecraft_directory() {
        let root = temp_dir("label-suggest");
        let game = root.join("MyMC").join(".minecraft");
        std::fs::create_dir_all(game.join("versions")).unwrap();
        assert_eq!(
            suggest_instance_label(&game, None, &[], &[]),
            Some("MyMC".to_string())
        );

        // 非 `.minecraft` 目录不参与自动取名。
        let custom = root.join("MyMC").join("custom");
        std::fs::create_dir_all(&custom).unwrap();
        assert_eq!(suggest_instance_label(&custom, None, &[], &[]), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_naming_when_parent_already_holds_multiple_instances() {
        let root = temp_dir("label-siblings");
        let parent = root.join("MyMC");
        for name in [".minecraft", ".mc", ".mygames"] {
            std::fs::create_dir_all(parent.join(name).join("versions")).unwrap();
        }
        std::fs::create_dir_all(parent.join(".other")).unwrap();
        assert_eq!(
            suggest_instance_label(&parent.join(".minecraft"), None, &[], &[]),
            None,
            "同一上级下存在多个可读实例时保持默认取名"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_home_and_system_parents() {
        let root = temp_dir("label-protected");
        let home = root.join("home");
        let system = root.join("system");
        let home_game = home.join(".minecraft");
        let system_game = system.join("games").join(".minecraft");
        std::fs::create_dir_all(&home_game).unwrap();
        std::fs::create_dir_all(&system_game).unwrap();

        assert_eq!(
            suggest_instance_label(&home_game, Some(&home), &[], &[]),
            None,
            "用户主目录本身不能作为显示名"
        );
        assert_eq!(
            suggest_instance_label(&system_game, None, &[system.clone()], &[]),
            None,
            "系统目录及其子目录不能作为显示名"
        );
        std::fs::remove_dir_all(root).unwrap();
    }


    #[test]
    fn scans_instances_from_disk() {
        let root = temp_dir("instances");

        let forge_dir = root.join("versions").join("1.20.1-forge-47.2.0");
        std::fs::create_dir_all(&forge_dir).unwrap();
        std::fs::write(
            forge_dir.join("1.20.1-forge-47.2.0.json"),
            r#"{"id":"1.20.1-forge-47.2.0","type":"release","releaseTime":"2023-06-12T13:25:51+00:00",
                "libraries":[{"name":"net.minecraftforge:forge:1.20.1-47.2.0"}]}"#,
        )
        .unwrap();

        // 文件名与目录名不一致
        let odd_dir = root.join("versions").join("1.21.4");
        std::fs::create_dir_all(&odd_dir).unwrap();
        std::fs::write(
            odd_dir.join("client.json"),
            r#"{"id":"1.21.4","type":"release","mainClass":"net.minecraft.client.main.Main","releaseTime":"2024-12-03T10:00:00+00:00"}"#,
        )
        .unwrap();

        // 损坏的版本 JSON
        let broken_dir = root.join("versions").join("broken");
        std::fs::create_dir_all(&broken_dir).unwrap();
        std::fs::write(broken_dir.join("broken.json"), "{oops").unwrap();

        // versions 下的普通文件应被忽略
        std::fs::write(root.join("versions").join("README.txt"), "x").unwrap();

        let entries = scan_instances(&root);
        assert_eq!(entries.len(), 3, "只统计目录，忽略散落文件");

        // 排序：有发布时间的新在前
        assert_eq!(entries[0].id, "1.21.4");

        let forge = entries.iter().find(|e| e.loader == LOADER_FORGE).unwrap();
        assert_eq!(forge.loader_version.as_deref(), Some("47.2.0"));
        assert_eq!(forge.icon, ICON_ANVIL);
        assert!(forge.has_json);
        assert!(forge.path.ends_with("1.20.1-forge-47.2.0"));

        let broken = entries.iter().find(|e| e.id == "broken").unwrap();
        assert!(!broken.has_json);
        assert_eq!(broken.loader, LOADER_UNKNOWN);
        assert_eq!(broken.icon, ICON_REDSTONE_BLOCK);
        assert!(broken.release_time.is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_commands_never_fail() {
        let missing = temp_dir("missing");
        assert_eq!(instance_scan(missing.to_string_lossy().to_string()), "[]");
        assert_eq!(
            resources_scan(missing.to_string_lossy().to_string(), "mods".into()),
            "[]"
        );

        let root = temp_dir("resources");
        std::fs::create_dir_all(root.join("mods")).unwrap();
        std::fs::write(root.join("mods").join("a.jar"), b"xy").unwrap();
        std::fs::write(root.join("mods").join(".DS_Store"), b"x").unwrap();
        std::fs::create_dir_all(root.join("mods").join("disabled")).unwrap();

        let mods = scan_resources(&root, "mods");
        assert_eq!(mods.len(), 2, "隐藏文件应被跳过");
        assert_eq!(mods[0].name, "a.jar");
        assert_eq!(mods[0].size_bytes, 2);
        assert!(!mods[0].is_dir);
        assert_eq!(mods[1].name, "disabled");
        assert!(mods[1].is_dir);
        assert_eq!(mods[1].size_bytes, 0);

        // 未知 kind / 缺文件夹 → 空数组而不是错误
        assert!(scan_resources(&root, "unknown").is_empty());
        assert!(scan_resources(&root, "saves").is_empty());
        assert_eq!(
            resources_scan(root.to_string_lossy().to_string(), "shaderpacks".into()),
            "[]"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn root_deletion_canonicalizes_protected_paths_and_their_parents() {
        let root = temp_dir("delete-protection");
        let protected = [root.join("Launcher/bin"), root.join("Config/data"), root.join("Users/TestHome")];
        for directory in &protected { std::fs::create_dir_all(directory).unwrap(); }
        let game = root.join("Launcher/bin/.minecraft");
        std::fs::create_dir(&game).unwrap();
        for directory in &protected {
            for target in [directory.as_path(), directory.parent().unwrap(), root.as_path()] {
                assert!(deletable_root(target, &protected).unwrap_err().contains("不能删除"));
                let canonical = std::fs::canonicalize(target).unwrap();
                assert!(deletable_root(&canonical, &protected).unwrap_err().contains("不能删除"));
                #[cfg(windows)]
                {
                    let case_changed = PathBuf::from(target.to_string_lossy().to_uppercase());
                    assert!(deletable_root(&case_changed, &protected).unwrap_err().contains("不能删除"));
                    let case_protected: Vec<_> = protected.iter().map(|path| PathBuf::from(path.to_string_lossy().to_lowercase())).collect();
                    assert!(deletable_root(&canonical, &case_protected).unwrap_err().contains("不能删除"));
                }
                assert!(target.is_dir());
            }
        }
        assert_eq!(deletable_root(&game, &protected).unwrap(), std::fs::canonicalize(&game).unwrap());
        assert!(deletable_root(&game, &[root.join("missing-protected")]).is_err());
        let volume = std::fs::canonicalize(&root).unwrap().ancestors().last().unwrap().to_path_buf();
        assert!(deletable_root(&volume, &protected).unwrap_err().contains("根目录"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registered_root_delete_preserves_protected_and_busy_directories() {
        let root = temp_dir("delete-idle");
        let launcher = root.join("launcher");
        let configuration = root.join("configuration");
        let home = root.join("home");
        let game = launcher.join(".minecraft");
        let child = game.join("versions/instance");
        for directory in [&launcher, &configuration, &home, &child] { std::fs::create_dir_all(directory).unwrap(); }
        let protected = [launcher.clone(), configuration.clone(), home.clone()];
        let registry_path = configuration.join("instances.json");
        let game_path = configuration.join("game.json");
        let sentinel = child.join("keep.txt");
        std::fs::write(&sentinel, b"keep until authorized idle delete").unwrap();
        let game_text = game.to_string_lossy().into_owned();
        let mut registry = Registry { roots: vec![game_text.clone()], active: Some(game_text.clone()), ..Default::default() };
        for target in [&launcher, &configuration, &home, &root] { registry.roots.push(target.to_string_lossy().into_owned()); }
        let original = registry_to_text(&registry);
        write_registry_atomic(&registry_path, &original).unwrap();
        std::fs::write(&game_path, serde_json::json!({"gameDir":game_text,"extra":"keep"}).to_string()).unwrap();
        for target in [&launcher, &configuration, &home, &root] {
            assert!(delete_registered_root(&target.to_string_lossy(), &registry_path, &game_path, &protected).unwrap_err().contains("不能删除"));
        }
        {
            let _task = crate::app_tasks::test_reserve(&child);
            assert!(delete_registered_root(&game_text, &registry_path, &game_path, &protected).unwrap_err().contains("安装或取消"));
        }
        {
            let _running = crate::launch_state::begin(&child).unwrap();
            assert!(delete_registered_root(&game_text, &registry_path, &game_path, &protected).unwrap_err().contains("正在启动或运行"));
        }
        assert!(sentinel.exists());
        assert_eq!(std::fs::read_to_string(&registry_path).unwrap(), original);
        delete_registered_root(&game_text, &registry_path, &game_path, &protected).unwrap();
        assert!(!game.exists());
        for path in protected { assert!(path.is_dir()); }
        assert!(!read_registry(&registry_path).roots.contains(&game_text));
        let game_settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&game_path).unwrap()).unwrap();
        assert_eq!(game_settings["extra"], "keep");
        assert_ne!(game_settings["gameDir"], game_text);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lenient_json_accepts_bom_comments_trailing_commas_and_keeps_strings() {
        let text = "\u{feff}{\r\n  // line comment\r\n  \"url\": \"https://example.invalid/a//b\",\r\n  \"list\": [1, 2,],\r\n  /* block comment */\r\n  \"text\": \"a,b] // not a comment\",\r\n}";
        let value = parse_json_lenient(text).expect("PCL 宽容 JSON 应可解析");
        assert_eq!(value["url"], "https://example.invalid/a//b");
        assert_eq!(value["list"], serde_json::json!([1, 2]));
        assert_eq!(value["text"], "a,b] // not a comment");

        let value = parse_json_lenient(r#"{"quote":"say \"//\" and ,","ok":true,}"#).unwrap();
        assert_eq!(value["quote"], "say \"//\" and ,");
        assert_eq!(value["ok"], true);

        assert!(parse_json_lenient("{ /* unterminated").is_none());
        assert!(parse_json_lenient("{oops").is_none());
    }

    #[test]
    fn version_json_fallback_prefers_exact_name_and_requires_version_descriptor() {
        let root = temp_dir("pick-json");
        let versions = root.join("versions");

        let exact = versions.join("exact");
        std::fs::create_dir_all(&exact).unwrap();
        std::fs::write(exact.join("exact.json"), r#"{"not":"a version"}"#).unwrap();
        std::fs::write(
            exact.join("version.json"),
            r#"{"id":"1.20.1","type":"release","mainClass":"net.minecraft.client.main.Main"}"#,
        )
        .unwrap();
        assert_eq!(
            pick_version_json(&exact, "exact").unwrap(),
            exact.join("exact.json"),
            "精确同名 JSON 永远优先"
        );

        let renamed = versions.join("renamed");
        std::fs::create_dir_all(&renamed).unwrap();
        std::fs::write(
            renamed.join("verification.json"),
            r#"{"version":"1.20.1","report":true}"#,
        )
        .unwrap();
        std::fs::write(
            renamed.join("version.json"),
            "\u{feff}{\n // pcl\n \"id\":\"1.21.4\", \"type\":\"release\", \"mainClass\":\"net.minecraft.client.main.Main\",\n}",
        )
        .unwrap();
        assert_eq!(
            pick_version_json(&renamed, "renamed").unwrap(),
            renamed.join("version.json"),
            "业务 JSON 不能被当成版本描述"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn version_json_fallback_skips_oversized_files() {
        let root = temp_dir("pick-json-size");
        let version = root.join("versions").join("renamed");
        std::fs::create_dir_all(&version).unwrap();
        for name in ["huge.json", "renamed.json"] {
            let file = std::fs::File::create(version.join(name)).unwrap();
            file.set_len(VERSION_JSON_MAX_BYTES + 1).unwrap();
        }
        std::fs::write(
            version.join("version.json"),
            r#"{"id":"1.21.4","type":"release","mainClass":"net.minecraft.client.main.Main"}"#,
        )
        .unwrap();

        // 超过 8 MiB 的精确同名 / 业务文件不能整份读入，也不能挡住合法版本描述。
        assert_eq!(
            pick_version_json(&version, "renamed").unwrap(),
            version.join("version.json")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_launcher_versions_use_client_version_and_ignore_version_vanilla_encoding() -> Result<(), String> {
        let root = temp_dir("pcl-version");
        let directory = root.join("versions").join("BACraft");
        std::fs::create_dir_all(directory.join("PCL")).map_err(|e| e.to_string())?;
        std::fs::write(
            directory.join("BACraft.json"),
            serde_json::json!({
                "id": "BACraft",
                "type": "release",
                "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
                "clientVersion": "1.20.1",
                "jar": null,
                "libraries": [
                    {"name": "net.minecraftforge:eventbus:6.0.5"},
                    {"name": "net.minecraftforge:fmlloader:1.20.1-47.4.10"}
                ],
                "arguments": {"game": ["--launchTarget", "forgeclient", "--fml.forgeVersion", "47.4.10", "--fml.mcVersion", "1.20.1"]}
            })
            .to_string(),
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(
            directory.join("PCL/Setup.ini"),
            "VersionVanillaName:1.20.1\r\nVersionVanilla:20.0.1\r\nVersionForge:47.4.10\r\nVersionArgumentIndieV2:True\r\n",
        )
        .map_err(|e| e.to_string())?;

        let entries = scan_instances(&root);
        let entry = entries.first().ok_or("没有扫描到实例")?;
        assert!(entry.has_json, "jar:null 不应判为版本损坏");
        assert_eq!(entry.minecraft_version.as_deref(), Some("1.20.1"));
        assert_eq!(entry.loader, LOADER_FORGE);
        assert_eq!(entry.loader_version.as_deref(), Some("47.4.10"));

        // 没有继承与 JAR 时仍识别 clientVersion，加载器可从 Setup.ini 补齐。
        std::fs::write(
            directory.join("BACraft.json"),
            serde_json::json!({
                "id": "BACraft",
                "type": "release",
                "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
                "clientVersion": "1.20.1"
            })
            .to_string(),
        )
        .map_err(|e| e.to_string())?;
        let entries = scan_instances(&root);
        assert_eq!(entries[0].minecraft_version.as_deref(), Some("1.20.1"));
        assert_eq!(entries[0].loader, LOADER_FORGE);
        assert_eq!(entries[0].loader_version.as_deref(), Some("47.4.10"));

        // VersionVanillaName 可补齐版本，VersionVanilla 的内部编码不能参与判断。
        std::fs::write(
            directory.join("BACraft.json"),
            r#"{"id":"BACraft","type":"release","mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher"}"#,
        )
        .map_err(|e| e.to_string())?;
        let entries = scan_instances(&root);
        assert_eq!(entries[0].minecraft_version.as_deref(), Some("1.20.1"));
        assert_eq!(entries[0].loader, LOADER_FORGE);

        std::fs::remove_dir_all(root).map_err(|e| e.to_string())?;
        Ok(())
    }

    #[test]
    fn inherits_chain_resolves_parent_and_missing_parent_is_not_corrupt() -> Result<(), String> {
        let root = temp_dir("inherits");
        let versions = root.join("versions");
        let base = versions.join("base");
        std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
        std::fs::write(
            base.join("base.json"),
            r#"{"id":"base","type":"release","mainClass":"net.minecraft.client.main.Main","clientVersion":"1.19.4"}"#,
        )
        .map_err(|e| e.to_string())?;

        let child = versions.join("child");
        std::fs::create_dir_all(&child).map_err(|e| e.to_string())?;
        std::fs::write(
            child.join("child.json"),
            r#"{"id":"child","type":"release","mainClass":"net.minecraft.client.main.Main","inheritsFrom":"base"}"#,
        )
        .map_err(|e| e.to_string())?;
        let entries = scan_instances(&root);
        let child_entry = entries.iter().find(|entry| entry.id == "child").unwrap();
        assert_eq!(child_entry.minecraft_version.as_deref(), Some("1.19.4"));

        std::fs::write(
            child.join("child.json"),
            r#"{"id":"child","type":"release","mainClass":"net.minecraft.client.main.Main","inheritsFrom":"missing-parent"}"#,
        )
        .map_err(|e| e.to_string())?;
        let entries = scan_instances(&root);
        let child_entry = entries.iter().find(|entry| entry.id == "child").unwrap();
        assert_eq!(child_entry.minecraft_version, None, "父版本缺失时不应伪造版本号");
        assert!(child_entry.has_json, "父版本缺失不等于子版本 JSON 损坏");

        std::fs::remove_dir_all(root).map_err(|e| e.to_string())?;
        Ok(())
    }

    #[test]
    fn inherits_chain_stops_at_cycles_and_depth_limit() -> Result<(), String> {
        let root = temp_dir("inherits-bounds");
        let versions = root.join("versions");
        let write = |name: &str, json: serde_json::Value| -> Result<(), String> {
            let directory = versions.join(name);
            std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
            std::fs::write(directory.join(format!("{name}.json")), json.to_string())
                .map_err(|e| e.to_string())
        };

        // A→B→A 循环：不能无限递归，也不能把实例判为版本损坏。
        write("cycle-a", serde_json::json!({
            "id": "cycle-a", "type": "release",
            "mainClass": "net.minecraft.client.main.Main", "inheritsFrom": "cycle-b"
        }))?;
        write("cycle-b", serde_json::json!({
            "id": "cycle-b", "type": "release",
            "mainClass": "net.minecraft.client.main.Main", "inheritsFrom": "cycle-a"
        }))?;
        let entries = scan_instances(&root);
        for id in ["cycle-a", "cycle-b"] {
            let entry = entries.iter().find(|entry| entry.id == id).ok_or("缺少循环实例")?;
            assert_eq!(entry.minecraft_version, None);
            assert!(entry.has_json, "循环引用不等于版本 JSON 损坏");
        }

        // 10 层链超过上限：deep-0 不应继续追到 deep-9 的版本号。
        for depth in 0..10 {
            let name = format!("deep-{depth}");
            let mut json = serde_json::json!({
                "id": name, "type": "release",
                "mainClass": "net.minecraft.client.main.Main"
            });
            if depth == 9 {
                json["clientVersion"] = serde_json::json!("1.20.1");
            } else {
                json["inheritsFrom"] = serde_json::json!(format!("deep-{}", depth + 1));
            }
            write(&name, json)?;
        }
        let entries = scan_instances(&root);
        let deepest = entries.iter().find(|entry| entry.id == "deep-0").ok_or("缺少深层实例")?;
        assert_eq!(deepest.minecraft_version, None, "超过递归上限时应安全停止");
        // 上限内的 8 次跳转仍能解析到真实版本号。
        let near = entries.iter().find(|entry| entry.id == "deep-1").ok_or("缺少近层实例")?;
        assert_eq!(near.minecraft_version.as_deref(), Some("1.20.1"));

        std::fs::remove_dir_all(root).map_err(|e| e.to_string())
    }

    #[test]
    fn forge_and_neoforge_are_detected_from_any_library_coordinate() {
        let forge = libs(&[
            "net.minecraftforge:eventbus:6.0.5",
            "net.minecraftforge:fmlloader:1.20.1-47.4.10",
        ]);
        assert_eq!(
            detect_loader(
                "BACraft",
                "release",
                Some("cpw.mods.bootstraplauncher.BootstrapLauncher"),
                None,
                &forge,
            ),
            LOADER_FORGE
        );

        let neo = libs(&[
            "net.neoforged:accesstransformers:10.0.1",
            "net.neoforged.fancymodloader:loader:4.0.43",
        ]);
        assert_eq!(
            detect_loader(
                "设身处地-1.1.2（空军总部更新）",
                "release",
                Some("cpw.mods.bootstraplauncher.BootstrapLauncher"),
                None,
                &neo,
            ),
            LOADER_NEOFORGE
        );

        let quilt = libs(&["org.quiltmc:quilt-loader:0.24.0"]);
        assert_eq!(
            detect_loader("quilt pack", "release", None, None, &quilt),
            LOADER_FABRIC
        );
    }

    #[test]
    #[ignore]
    fn legacy_launcher_instances_are_detected() {
        let Some(root) = std::env::var_os("NOWHERE_PCL_ROOT").map(PathBuf::from) else {
            eprintln!("[pcl-compat] 未设置 NOWHERE_PCL_ROOT，跳过真实 PCL 根目录验收");
            return;
        };
        assert!(root.is_absolute(), "NOWHERE_PCL_ROOT 必须是绝对路径");
        let entries = scan_instances(&root);
        assert!(!entries.is_empty(), "PCL 根目录下没有实例");

        let mut failures = Vec::new();
        let mut unparsable = Vec::new();
        eprintln!("dir\tmcVersion\tloader\tloaderVersion\tdisplayName\tsettingsSource\tmemoryMode\tmemoryMb");
        for entry in &entries {
            let source = crate::version_settings::settings_source(&root, &entry.id);
            let settings = crate::version_settings::version_settings_get(
                root.to_string_lossy().into_owned(),
                entry.id.clone(),
            )
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
            // VersionRamCustom 是滑条档位，读取结果须完成容量换算。
            if source == "pcl"
                && let Some(setup) = crate::version_settings::read_legacy_launcher_setup(
                    &root.join("versions").join(&entry.id),
                )
                && setup.int_value("VersionRamType") == Some(1)
            {
                let slider = setup.int_value("VersionRamCustom").unwrap_or(15).max(0) as u64;
                let mode = settings.as_ref().and_then(|value| value["memoryMode"].as_str());
                let memory_mb = settings.as_ref().and_then(|value| value["memoryMb"].as_u64());
                if mode != Some("custom") || memory_mb.is_none() || memory_mb == Some(slider) {
                    failures.push(format!("{}: PCL VersionRamCustom 未按滑条换算为 MB", entry.id));
                }
            }
            let memory_mode = settings
                .as_ref()
                .and_then(|value| value["memoryMode"].as_str())
                .unwrap_or("<none>");
            let memory_mb = settings
                .as_ref()
                .and_then(|value| value["memoryMb"].as_u64())
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<none>".to_string());
            eprintln!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                entry.id,
                entry.minecraft_version.as_deref().unwrap_or("<unknown>"),
                entry.loader,
                entry.loader_version.as_deref().unwrap_or("<none>"),
                entry.display_name,
                source,
                memory_mode,
                memory_mb
            );
            if !entry.has_json {
                let version_dir = root.join("versions").join(&entry.id);
                if pick_version_json(&version_dir, &entry.id).is_some() {
                    // 描述存在但不可解析时单独列出，继续检查其余实例。
                    unparsable.push(entry.id.clone());
                } else {
                    failures.push(format!("{}: 缺少版本 JSON", entry.id));
                }
                continue;
            }
            if entry.minecraft_version.is_none() {
                failures.push(format!("{}: 未识别 MC 版本", entry.id));
            }
            if entry.loader == LOADER_UNKNOWN {
                failures.push(format!("{}: 未识别加载器", entry.id));
            }
            // 详情页与实例列表必须给出同一份真实版本 / 来源；缺失 JAR 只记录不报错。
            match crate::version_settings::version_detail(
                root.to_string_lossy().into_owned(),
                entry.id.clone(),
            )
            .and_then(|text| {
                serde_json::from_str::<serde_json::Value>(&text)
                    .map_err(|error| error.to_string())
            }) {
                Ok(detail) => {
                    if detail["minecraftVersion"] != entry.minecraft_version.clone().map_or(
                        serde_json::Value::Null,
                        serde_json::Value::String,
                    ) {
                        failures.push(format!("{}: 详情与列表的 MC 版本不一致", entry.id));
                    }
                    if detail["settingsSource"].as_str() != Some(source) {
                        failures.push(format!("{}: 详情与列表的设置来源不一致", entry.id));
                    }
                    if matches!(
                        entry.loader.as_str(),
                        LOADER_FORGE | LOADER_NEOFORGE | LOADER_FABRIC | LOADER_OPTIFABRIC
                    ) && detail["loaderVersion"].as_str().is_none()
                    {
                        failures.push(format!("{}: 详情缺少加载器版本", entry.id));
                    }
                }
                Err(error) => failures.push(format!("{}: 读取版本详情失败（{error}）", entry.id)),
            }
        }
        if !unparsable.is_empty() {
            eprintln!(
                "[pcl-compat] 以下实例的版本 JSON 确实不可解析，已按需求单独列出：{}",
                unparsable.join(", ")
            );
        }
        assert!(
            failures.is_empty(),
            "PCL 实例识别失败：\n{}",
            failures.join("\n")
        );
    }


}
