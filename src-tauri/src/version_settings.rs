//! 实例设置与版本管理命令。
//!
//! 设置文件 `nickel.settings.json` 随版本目录保存；读写补齐默认值并保留未知键。
//! 版本 JSON 的 `nickelCustom` 只保存展示信息，改名不得修改目录身份或 `id`。
//! 外部启动器设置只作为缺失本地设置时的只读来源，首次保存才写本地文件。
//! 写入和删除前须校验版本 ID，拒绝路径分隔符、`..`、盘符、备用数据流与 NUL。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::store;

/// 单个版本设置文件大小上限（256KB），与 settings.json / game.json 一致。
const SETTINGS_MAX_BYTES: usize = 256 * 1024;

/// 保留已有实例的设置文件名，复制与迁移时随版本目录携带。
const SETTINGS_FILE_NAME: &str = "nickel.settings.json";

const ICON_MAX_BYTES: u64 = 8 * 1024 * 1024;
const ICON_MAX_DIMENSION: u32 = 4096;
const ICON_DIRECTORY: &str = "nickel-icons";
const ICON_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];


/// 版本 JSON 里的自定义信息字段。
const CUSTOM_FIELD: &str = "nickelCustom";

// memoryMode 的持久化取值。
const MEMORY_GLOBAL: &str = "global";
const MEMORY_AUTO: &str = "auto";
const MEMORY_CUSTOM: &str = "custom";

// ─────────────────────────────────────────────────────────────────────────────
// 设置结构 / 归一化
// ─────────────────────────────────────────────────────────────────────────────

/// 实例设置的默认值与序列化字段名；实际读写须用 Value 合并保留未知键。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VersionSettings {
    pub version_isolation: Option<bool>,
    pub window_title: Option<String>,
    pub custom_info: Option<String>,
    pub java_path: Option<String>,
    pub launcher_visibility: Option<String>,
    pub process_priority: Option<String>,
    pub window_mode: Option<String>,
    pub window_width: Option<u64>,
    pub window_height: Option<u64>,
    /// `global` | `auto` | `custom`。
    pub memory_mode: String,
    pub memory_mb: Option<u64>,
    pub memory_management: Option<String>,
    pub high_performance_gpu: Option<bool>,
    pub offline_skin: Option<String>,
    pub offline_skin_name: Option<String>,
    pub offline_skin_path: Option<String>,
    pub offline_skin_slim: Option<bool>,
    pub server_login: Option<String>,
    pub auth_server: Option<String>,
    pub server_registration: Option<String>,
    pub server_name: Option<String>,
    pub server_address: Option<String>,
    pub jvm_args: Option<String>,
    pub game_args: Option<String>,
    pub pre_launch_command: Option<String>,
    pub disable_mod_updates: Option<bool>,
    pub disable_file_check: Option<bool>,
    pub disable_wrapper: Option<bool>,
    pub disable_lwjgl_agent: Option<bool>,
    pub icon: Option<String>,
}

impl Default for VersionSettings {
    fn default() -> Self {
        Self {
            version_isolation: None,
            window_title: None,
            custom_info: None,
            java_path: None,
            launcher_visibility: None,
            process_priority: None,
            window_mode: None,
            window_width: None,
            window_height: None,
            memory_mode: MEMORY_GLOBAL.to_string(),
            memory_mb: None,
            memory_management: None,
            high_performance_gpu: None,
            offline_skin: None,
            offline_skin_name: None,
            offline_skin_path: None,
            offline_skin_slim: None,
            server_login: None,
            auth_server: None,
            server_registration: None,
            server_name: None,
            server_address: None,
            jvm_args: None,
            game_args: None,
            pre_launch_command: None,
            disable_mod_updates: None,
            disable_file_check: None,
            disable_wrapper: None,
            disable_lwjgl_agent: None,
            icon: None,
        }
    }
}

/// 默认设置序列化失败时回退空对象。
fn default_settings_value() -> serde_json::Value {
    serde_json::to_value(VersionSettings::default()).unwrap_or_else(|_| serde_json::json!({}))
}

/// 合并时保留未知键；已知键类型不符时使用默认值，继承标记单独接受。
fn merge_settings(defaults: serde_json::Value, stored: serde_json::Value) -> serde_json::Value {
    let Some(mut out) = defaults.as_object().cloned() else {
        return default_settings_value();
    };
    let Some(src) = stored.as_object().cloned() else {
        return serde_json::Value::Object(out);
    };
    for (key, value) in src {
        let inherits = value.is_null() || value.as_str() == Some(MEMORY_GLOBAL);
        let accepted = match key.as_str() {
            "versionIsolation" | "disableModUpdates" | "disableFileCheck"
            | "disableWrapper" | "disableLwjglAgent"
            | "highPerformanceGpu" | "offlineSkinSlim" => inherits || value.is_boolean(),
            "windowTitle" | "customInfo" | "javaPath" | "launcherVisibility"
            | "processPriority" | "windowMode" | "memoryManagement" | "offlineSkin"
            | "offlineSkinName" | "offlineSkinPath" | "serverLogin" | "authServer"
            | "serverRegistration" | "serverName" | "serverAddress" | "jvmArgs"
            | "gameArgs" | "preLaunchCommand" | "icon" => inherits || value.is_string(),
            "memoryMb" | "windowWidth" | "windowHeight" => inherits || value.as_u64().is_some(),
            _ => match out.get(&key) {
                None => true,
                Some(serde_json::Value::Null) => !value.is_object() && !value.is_array(),
                Some(serde_json::Value::Bool(_)) => value.is_boolean(),
                Some(serde_json::Value::Number(_)) => value.is_number(),
                Some(serde_json::Value::String(_)) => value.is_string(),
                Some(serde_json::Value::Object(_) | serde_json::Value::Array(_)) => false,
            },
        };
        if accepted {
            out.insert(key, value);
        }
    }
    serde_json::Value::Object(out)
}

/// 非法内存模式或缺少数值的 custom 模式回退全局设置。
fn normalize_memory(value: serde_json::Value) -> serde_json::Value {
    let mut value = value;
    let mode = match value.get("memoryMode").and_then(|v| v.as_str()) {
        Some(MEMORY_GLOBAL) | Some(MEMORY_AUTO) | Some(MEMORY_CUSTOM) => value
            .get("memoryMode")
            .and_then(|v| v.as_str())
            .unwrap_or(MEMORY_GLOBAL)
            .to_string(),
        _ => MEMORY_GLOBAL.to_string(),
    };
    let effective = if mode == MEMORY_CUSTOM
        && value.get("memoryMb").and_then(|v| v.as_u64()).is_none()
    {
        MEMORY_GLOBAL.to_string()
    } else {
        mode
    };
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "memoryMode".into(),
            serde_json::Value::String(effective),
        );
        if let Some(management) = obj
            .get("memoryManagement")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
        {
            let normalized = crate::runtime_settings::normalize_memory_management(&management)
                .unwrap_or_default();
            obj.insert(
                "memoryManagement".into(),
                if normalized.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(normalized)
                },
            );
        }
    }
    value
}

/// 文件缺失、超限、不可读或不是 JSON 对象时返回 None，交给调用方选用兼容来源。
fn read_own_settings(path: &Path) -> Option<serde_json::Value> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > SETTINGS_MAX_BYTES as u64 {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    let value = crate::instances::parse_json_lenient(&text)?;
    value.is_object().then_some(value)
}

/// 本地设置不可用时返回默认值。
#[cfg(test)]
fn read_settings_file(path: &Path) -> String {
    match read_own_settings(path) {
        Some(stored) => normalize_memory(merge_settings(default_settings_value(), stored))
            .to_string(),
        None => serde_json::to_string(&VersionSettings::default())
            .unwrap_or_else(|_| "{}".to_string()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 外部启动器配置兼容：versions/<id>/PCL/Setup.ini
// ─────────────────────────────────────────────────────────────────────────────

const LEGACY_LAUNCHER_SETUP_FILE_NAME: &str = "Setup.ini";
/// 外部设置与本地 JSON 使用相同的大小上限。
const LEGACY_LAUNCHER_SETUP_MAX_BYTES: u64 = 256 * 1024;

/// Setup.ini 使用 Key:Value 行格式；只拆第一个冒号，重复键以最后一项为准。
/// URL 中的冒号和斜杠属于值，不能按 INI 注释或分段语法处理。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LegacyLauncherSetup {
    values: std::collections::BTreeMap<String, String>,
}

impl LegacyLauncherSetup {
    /// 空值等同未设置，允许调用方继承根级设置。
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.values
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
    }

    pub(crate) fn bool_value(&self, key: &str) -> Option<bool> {
        match self.values.get(key)?.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        }
    }

    pub(crate) fn int_value(&self, key: &str) -> Option<i64> {
        self.values.get(key)?.trim().parse().ok()
    }

    fn keys(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }
}

fn read_legacy_launcher_setup_file(path: &Path) -> Option<LegacyLauncherSetup> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > LEGACY_LAUNCHER_SETUP_MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut values = std::collections::BTreeMap::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let line = line.strip_prefix('\u{feff}').unwrap_or(line);
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.is_empty() {
            continue;
        }
        values.insert(key.to_string(), value.to_string());
    }
    Some(LegacyLauncherSetup { values })
}

/// 读取 `<版本目录>/PCL/Setup.ini`；文件不存在 / 超限 / 不可读 → None。
pub(crate) fn read_legacy_launcher_setup(version_dir: &Path) -> Option<LegacyLauncherSetup> {
    read_legacy_launcher_setup_file(&version_dir.join("PCL").join(LEGACY_LAUNCHER_SETUP_FILE_NAME))
}

/// 根级 Setup.ini 补齐实例中未设置的 JVM / 游戏参数与标题。
/// 根级卡片缓存和注册表源不参与设置映射。
fn read_legacy_launcher_global_setup(root: &Path) -> Option<LegacyLauncherSetup> {
    read_legacy_launcher_setup_file(&root.join("PCL").join(LEGACY_LAUNCHER_SETUP_FILE_NAME))
}

/// 已消费的设置键与版本 / 加载器识别键。
const LEGACY_LAUNCHER_MAPPED_KEYS: &[&str] = &[
    "VersionVanillaName",
    "VersionForge",
    "VersionFabric",
    "VersionNeoForge",
    "VersionOptiFine",
    "VersionLiteLoader",
    "VersionArgumentIndieV2",
    "VersionArgumentTitle",
    "CustomInfo",
    "VersionAdvanceJvm",
    "VersionAdvanceGame",
    "VersionAdvanceRun",
    "VersionAdvanceDisableJLW",
    "VersionAdvanceDisableLUA",
    "VersionAdvanceDisableModUpdate",
    "VersionAdvanceAssetsV2",
    "VersionAdvanceAssets",
    "VersionAdvanceGC",
    "VersionRamType",
    "VersionRamCustom",
    "VersionArgumentJavaSelect",
    "VersionServerLogin",
    "VersionServerAuthServer",
    "VersionServerAuthRegister",
    "VersionServerAuthName",
    "VersionServerEnter",
];

/// 展示缓存和内部编码不作为用户设置；VersionVanilla（如 20.0.1）不得用于 MC 版本识别。
const LEGACY_LAUNCHER_CACHE_KEYS: &[&str] = &[
    "State",
    "Info",
    "Logo",
    "LogoCustom",
    "ReleaseTime",
    "IsStar",
    "DisplayType",
    "VersionVanilla",
    "VersionOriginal",
    "VersionOriginalMain",
    "VersionOriginalSub",
    "VersionApiCode",
];

fn legacy_launcher_text_with_global(
    setup: &LegacyLauncherSetup,
    global: Option<&LegacyLauncherSetup>,
    local_key: &str,
    global_key: &str,
) -> Option<String> {
    setup
        .get(local_key)
        .or_else(|| global.and_then(|global| global.get(global_key)))
        .map(str::to_owned)
}

fn legacy_launcher_bool_with_global(
    setup: &LegacyLauncherSetup,
    global: Option<&LegacyLauncherSetup>,
    local_key: &str,
    global_key: &str,
) -> Option<bool> {
    setup
        .bool_value(local_key)
        .or_else(|| global.and_then(|global| global.bool_value(global_key)))
}

/// VersionRamCustom 保存非线性滑条档位而非 MB，须先还原容量再换算单位。
fn legacy_launcher_memory_mb(slider: i64) -> u64 {
    let value = if slider <= 12 {
        slider as f64 * 0.1 + 0.3
    } else if slider <= 25 {
        (slider - 12) as f64 * 0.5 + 1.5
    } else if slider <= 33 {
        (slider - 25) as f64 + 8.0
    } else {
        (slider - 33) as f64 * 2.0 + 16.0
    };
    ((value * 1024.0).round() as u64).max(crate::memory_plan::MIN_MEMORY_MB)
}

/// 实例级 GC 的 0 表示继承，1..5 比根级编码偏移一档。
fn legacy_launcher_instance_gc_mode(value: i64) -> Option<&'static str> {
    match value {
        1 => Some("auto-zgc"),
        2 => Some("generational-zgc"),
        3 => Some("g1"),
        4 => Some("custom"),
        5 => Some("g1-tuned"),
        _ => None,
    }
}

/// 根级 GC 编码为 0..4，不能复用实例级偏移。
fn legacy_launcher_global_gc_mode(value: i64) -> Option<&'static str> {
    match value {
        0 => Some("auto-zgc"),
        1 => Some("generational-zgc"),
        2 => Some("g1"),
        3 => Some("custom"),
        4 => Some("g1-tuned"),
        _ => None,
    }
}

/// Java 选择值兼容 JSON 的 Path 字段与直接路径；自动 / 全局 / 目录选项不写入 javaPath。
fn legacy_launcher_java_path(setup: &LegacyLauncherSetup) -> Option<String> {
    let raw = setup.get("VersionArgumentJavaSelect")?;
    let json_path = crate::instances::parse_json_lenient(raw)
        .and_then(|value| {
            value
                .get("Path")
                .or_else(|| value.get("path"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
        });
    if let Some(path) = json_path {
        return Some(path);
    }
    let path = raw.trim();
    let lower = path.to_ascii_lowercase();
    ((path.contains('/') || path.contains('\\'))
        && (lower.ends_with(".exe") || lower.ends_with("/java") || lower.ends_with("\\java")))
    .then(|| path.to_string())
}

/// VersionServerLogin 的索引 3 为统一通行证，缺少对应认证流程时不生成覆盖值。
fn legacy_launcher_server_login(value: i64) -> Option<&'static str> {
    match value {
        0 => Some("any"),
        1 => Some("microsoft"),
        2 => Some("offline"),
        4 => Some("thirdParty"),
        _ => None,
    }
}

/// 只映射语义明确的外部设置键，未映射键单独列出；缺少配置时返回 None。
pub(crate) fn legacy_launcher_effective_settings(
    root: &Path,
    version_id: &str,
) -> Option<serde_json::Value> {
    let setup = read_legacy_launcher_setup(&root.join("versions").join(version_id))?;
    let global = read_legacy_launcher_global_setup(root);
    let global = global.as_ref();
    let mut out = serde_json::Map::new();
    let mut insert = |key: &str, value: serde_json::Value| {
        out.insert(key.to_string(), value);
    };

    if let Some(value) = setup.bool_value("VersionArgumentIndieV2") {
        insert("versionIsolation", value.into());
    }
    if let Some(value) =
        legacy_launcher_text_with_global(&setup, global, "VersionArgumentTitle", "LaunchArgumentTitle")
    {
        insert("windowTitle", value.into());
    }
    if let Some(value) = setup.get("CustomInfo") {
        insert("customInfo", value.to_string().into());
    }
    if let Some(value) = legacy_launcher_java_path(&setup) {
        insert("javaPath", value.into());
    }

    // VersionRamType：0 自动 / 1 自定义 / 2 继承，缺省继承根级设置。
    match setup.int_value("VersionRamType").unwrap_or(2) {
        0 => {
            insert("memoryMode", "auto".into());
        }
        1 => {
            insert("memoryMode", "custom".into());
            insert(
                "memoryMb",
                legacy_launcher_memory_mb(setup.int_value("VersionRamCustom").unwrap_or(15)).into(),
            );
        }
        _ => match global.and_then(|global| global.int_value("LaunchRamType")) {
            Some(0) => {
                insert("memoryMode", "auto".into());
            }
            Some(1) => {
                insert("memoryMode", "custom".into());
                let slider = global
                    .and_then(|global| global.int_value("LaunchRamCustom"))
                    .unwrap_or(15);
                insert("memoryMb", legacy_launcher_memory_mb(slider).into());
            }
            _ => {
                insert("memoryMode", "global".into());
            }
        },
    }

    // 实例级 GC 的 0 继承根级设置；显式继承而根级缺失时采用优化 G1。
    let gc_mode = match setup.int_value("VersionAdvanceGC") {
        Some(value) if value > 0 => legacy_launcher_instance_gc_mode(value),
        Some(_) => global
            .and_then(|global| global.int_value("LaunchAdvanceGC"))
            .and_then(legacy_launcher_global_gc_mode)
            .or(Some("g1-tuned")),
        None => global
            .and_then(|global| global.int_value("LaunchAdvanceGC"))
            .and_then(legacy_launcher_global_gc_mode),
    };
    if let Some(mode) = gc_mode {
        insert("memoryManagement", mode.into());
    }

    if let Some(value) =
        legacy_launcher_text_with_global(&setup, global, "VersionAdvanceJvm", "LaunchAdvanceJvm")
    {
        insert("jvmArgs", value.into());
    }
    if let Some(value) =
        legacy_launcher_text_with_global(&setup, global, "VersionAdvanceGame", "LaunchAdvanceGame")
    {
        insert("gameArgs", value.into());
    }
    if let Some(value) =
        legacy_launcher_text_with_global(&setup, global, "VersionAdvanceRun", "LaunchAdvanceRun")
    {
        insert("preLaunchCommand", value.into());
    }
    if let Some(value) = setup.bool_value("VersionAdvanceDisableModUpdate") {
        insert("disableModUpdates", value.into());
    }
    // 新字段存在时优先读取；旧 VersionAdvanceAssets 只有值 2 表示跳过校验。
    let disable_file_check = setup
        .bool_value("VersionAdvanceAssetsV2")
        .or_else(|| setup.int_value("VersionAdvanceAssets").and_then(|value| (value == 2).then_some(true)));
    if let Some(value) = disable_file_check {
        insert("disableFileCheck", value.into());
    }
    if let Some(value) = legacy_launcher_bool_with_global(
        &setup,
        global,
        "VersionAdvanceDisableJLW",
        "LaunchAdvanceDisableJLW",
    ) {
        insert("disableWrapper", value.into());
    }
    if let Some(value) = legacy_launcher_bool_with_global(
        &setup,
        global,
        "VersionAdvanceDisableLUA",
        "LaunchAdvanceDisableLUA",
    ) {
        insert("disableLwjglAgent", value.into());
    }

    if let Some(value) = setup
        .int_value("VersionServerLogin")
        .and_then(legacy_launcher_server_login)
    {
        insert("serverLogin", value.into());
    }
    for (external_key, local_key) in [
        ("VersionServerAuthServer", "authServer"),
        ("VersionServerAuthRegister", "serverRegistration"),
        ("VersionServerAuthName", "serverName"),
        ("VersionServerEnter", "serverAddress"),
    ] {
        if let Some(value) = setup.get(external_key) {
            insert(local_key, value.to_string().into());
        }
    }

    Some(serde_json::Value::Object(out))
}

/// 列出未消费且不是展示缓存的外部设置键。
pub(crate) fn legacy_launcher_unmapped_keys(version_dir: &Path) -> Vec<String> {
    let Some(setup) = read_legacy_launcher_setup(version_dir) else {
        return Vec::new();
    };
    setup
        .keys()
        .filter(|key| !LEGACY_LAUNCHER_MAPPED_KEYS.contains(key) && !LEGACY_LAUNCHER_CACHE_KEYS.contains(key))
        .map(str::to_owned)
        .collect()
}

/// 本地设置文件整体优先，不与外部配置混合；来源标识保持已有 IPC 取值。
pub(crate) fn effective_local(root: &Path, version: &str) -> (serde_json::Value, &'static str) {
    let version_dir = root.join("versions").join(version);
    if let Some(value) = read_own_settings(&version_dir.join(SETTINGS_FILE_NAME)) {
        return (value, "nickel");
    }
    if let Some(value) = legacy_launcher_effective_settings(root, version) {
        return (value, "pcl");
    }
    (serde_json::Value::Null, "nickel")
}

pub(crate) fn settings_source(root: &Path, version: &str) -> &'static str {
    effective_local(root, version).1
}

// ─────────────────────────────────────────────────────────────────────────────
// 纯工具：路径安全 / 原子写 / 版本 JSON 与 jar 定位
// ─────────────────────────────────────────────────────────────────────────────

/// 版本 id 安全校验：空 / 路径分隔符 / 上级目录 / 盘符与 NTFS 备用数据流 / NUL 一律拒绝。
fn valid_version_id(version_id: &str) -> Result<&str, String> {
    let id = version_id.trim();
    if id.is_empty() {
        return Err("缺少版本 id（versionId）".to_string());
    }
    if id == "." || id.contains("..") {
        return Err(format!("非法的版本 id：{id}（不允许包含 ..）"));
    }
    if id.contains('/') || id.contains('\\') {
        return Err(format!("非法的版本 id：{id}（不允许包含路径分隔符）"));
    }
    if id.contains(':') || id.contains('\0') {
        return Err(format!("非法的版本 id：{id}（不允许包含 : 或控制字符）"));
    }
    Ok(id)
}

/// 实例根目录 + 版本 id → 版本目录。
fn version_dir_of(dir: &str, version_id: &str) -> Result<PathBuf, String> {
    if dir.trim().is_empty() {
        return Err("缺少实例目录（dir）".to_string());
    }
    let path = PathBuf::from(dir.trim()).join("versions").join(version_id);
    crate::instances::reject_links(&path)?;
    Ok(path)
}

/// 原子写：先写同目录临时文件再 rename；rename 失败（目标被占用等极端情况）
/// 退化为直接写 —— 宁可牺牲原子性也不让保存卡死（与 instances.rs 同一策略）。
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败（{}）：{e}", parent.display()))?;
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("nickel.tmp");
    let tmp = path.with_file_name(format!("{file_name}.tmp-{}", store::random_token()));
    std::fs::write(&tmp, text)
        .map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    }
    Ok(())
}

/// 与实例扫描共用版本描述选择规则，避免命名实例元数据被误当作 Mojang JSON。
fn pick_version_json(version_dir: &Path, dir_name: &str) -> Option<PathBuf> {
    crate::instances::pick_version_json(version_dir, dir_name)
}

/// 找版本主 jar：优先 `<version_id>.jar`；找不到时退化为目录下按名称排序的
/// 第一个 `.jar`（与 [`pick_version_json`] 同构）。返回 jar 大小（字节）。
fn find_version_jar(version_dir: &Path, version_id: &str) -> Option<u64> {
    if crate::named_profiles::has_profile(version_dir) {
        let root = version_dir.parent()?.parent()?;
        let profile = crate::named_profiles::load(root, version_id).ok()??;
        let mc = &profile.minecraft_version;
        let jar = version_dir.join("versions").join(mc).join(format!("{mc}.jar"));
        return std::fs::metadata(jar).ok().filter(|m| m.is_file()).map(|m| m.len());
    }
    let exact = version_dir.join(format!("{version_id}.jar"));
    if exact.is_file() {
        return std::fs::metadata(&exact).map(|m| m.len()).ok();
    }
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(version_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .map(|x| x.eq_ignore_ascii_case("jar"))
                    .unwrap_or(false)
        })
        .collect();
    candidates.sort();
    let jar = candidates.into_iter().next()?;
    std::fs::metadata(&jar).map(|m| m.len()).ok()
}

/// 仅更新 nickelCustom 的展示字段，保留其余键与顶层 id；非对象输入返回 false。
fn patch_custom_fields(
    value: &mut serde_json::Value,
    name: &str,
    description: &str,
    category: &str,
    favorite: bool,
) -> bool {
    let Some(obj) = value.as_object_mut() else {
        return false;
    };
    let mut custom = obj
        .get(CUSTOM_FIELD)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    custom.insert("name".into(), serde_json::Value::String(name.to_string()));
    custom.insert(
        "description".into(),
        serde_json::Value::String(description.to_string()),
    );
    custom.insert(
        "category".into(),
        serde_json::Value::String(category.to_string()),
    );
    let previous = custom.get("favorite").and_then(|v| v.as_bool()).unwrap_or(false);
    if favorite && (!previous || custom.get("favoriteAt").and_then(|v| v.as_u64()).is_none()) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|value| value.as_millis() as u64).unwrap_or(0);
        custom.insert("favoriteAt".into(), now.into());
    } else if !favorite { custom.insert("favoriteAt".into(), serde_json::Value::Null); }
    custom.insert("favorite".into(), serde_json::Value::Bool(favorite));
    obj.insert(CUSTOM_FIELD.into(), serde_json::Value::Object(custom));
    true
}

// ─────────────────────────────────────────────────────────────────────────────
// 命令：per-version 设置
// ─────────────────────────────────────────────────────────────────────────────

/// 本地设置不可用时读取外部配置并附来源标识；两者缺失时使用默认值。
/// 路径与版本 ID 仍须通过校验。
#[tauri::command]
pub fn version_settings_get(dir: String, version_id: String) -> Result<String, String> {
    let id = valid_version_id(&version_id)?;
    let version_dir = version_dir_of(&dir, id)?;
    let root = version_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(Path::new(dir.trim()));
    let (raw, source) = effective_local(root, id);
    let mut merged = if raw.is_null() {
        default_settings_value()
    } else {
        normalize_memory(merge_settings(default_settings_value(), raw))
    };
    if let Some(object) = merged.as_object_mut() {
        object.insert(
            "settingsSource".into(),
            serde_json::Value::String(source.into()),
        );
    }
    Ok(merged.to_string())
}

/// 写入 per-version 设置：≤256KB + JSON 校验，归一化（补默认、保未知字段）后落盘。
#[tauri::command]
pub fn version_settings_set(dir: String, version_id: String, json: String) -> Result<(), String> {
    let id = valid_version_id(&version_id)?;
    if json.len() > SETTINGS_MAX_BYTES {
        return Err(format!(
            "版本设置内容过大（{} 字节，上限 {} 字节）",
            json.len(),
            SETTINGS_MAX_BYTES
        ));
    }
    let mut stored: serde_json::Value = crate::instances::parse_json_lenient(&json)
        .ok_or_else(|| "非法 JSON：版本设置无法解析".to_string())?;
    // 来源在读取时计算，不能写入设置文件固化旧来源。
    if let Some(object) = stored.as_object_mut() {
        object.remove("settingsSource");
    }
    let merged = normalize_memory(merge_settings(default_settings_value(), stored));
    let text =
        serde_json::to_string(&merged).map_err(|e| format!("序列化版本设置失败: {e}"))?;
    let version_dir = version_dir_of(&dir, id)?;
    if !version_dir.is_dir() {
        return Err(format!("版本目录不存在：{}", version_dir.display()));
    }
    write_atomic(&version_dir.join(SETTINGS_FILE_NAME), &text)
}

fn icon_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || width > ICON_MAX_DIMENSION || height > ICON_MAX_DIMENSION {
        return Err(format!("图标尺寸无效，请选择宽高均为 1–{ICON_MAX_DIMENSION} 像素的图片"));
    }
    Ok(())
}

// 后端限制文件大小、容器边界和像素尺寸；前端在提交 icon 元数据前用 WebView 解码像素流。
fn validate_icon(bytes: &[u8], extension: &str) -> Result<(), String> {
    let invalid = || "图标格式无效或文件不完整，请重新导出 PNG、JPEG 或 WebP 图片".to_string();
    if bytes.len() as u64 > ICON_MAX_BYTES { return Err("图标超过 8 MiB，请缩小图片后重试".into()); }
    match extension {
        "png" => {
            if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") { return Err(invalid()); }
            let mut offset = 8;
            let mut header = false;
            let mut pixels = false;
            while offset + 12 <= bytes.len() {
                let size = u32::from_be_bytes(bytes[offset..offset + 4].try_into().map_err(|_| invalid())?) as usize;
                let end = offset.checked_add(12).and_then(|value| value.checked_add(size)).filter(|end| *end <= bytes.len()).ok_or_else(invalid)?;
                let kind = &bytes[offset + 4..offset + 8];
                let data = &bytes[offset + 8..end - 4];
                if !header && kind != b"IHDR" { return Err(invalid()); }
                match kind {
                    b"IHDR" if !header && size == 13 => {
                        icon_dimensions(u32::from_be_bytes(data[..4].try_into().map_err(|_| invalid())?),
                            u32::from_be_bytes(data[4..8].try_into().map_err(|_| invalid())?))?;
                        let depth_valid = match data[9] {
                            0 => matches!(data[8], 1 | 2 | 4 | 8 | 16),
                            2 | 4 | 6 => matches!(data[8], 8 | 16),
                            3 => matches!(data[8], 1 | 2 | 4 | 8),
                            _ => false,
                        };
                        if !depth_valid || data[10] != 0 || data[11] != 0 || data[12] > 1 { return Err(invalid()); }
                        header = true;
                    }
                    b"IHDR" => return Err(invalid()),
                    b"IDAT" => pixels |= !data.is_empty(),
                    b"IEND" if size == 0 && pixels && end == bytes.len() => return Ok(()),
                    b"IEND" => return Err(invalid()),
                    _ => {}
                }
                offset = end;
            }
            Err(invalid())
        }
        "jpg" | "jpeg" => {
            if !bytes.starts_with(&[0xff, 0xd8]) { return Err(invalid()); }
            let mut offset = 2;
            let mut frame = false;
            let mut pixels = false;
            while offset + 2 <= bytes.len() {
                if bytes[offset] != 0xff { return Err(invalid()); }
                while bytes.get(offset) == Some(&0xff) { offset += 1; }
                let marker = *bytes.get(offset).ok_or_else(invalid)?;
                offset += 1;
                if marker == 0xd9 { return if frame && pixels && offset == bytes.len() { Ok(()) } else { Err(invalid()) }; }
                if marker == 0 || marker == 0xd8 { return Err(invalid()); }
                if marker == 1 || (0xd0..=0xd7).contains(&marker) { continue; }
                let size_bytes = bytes.get(offset..offset + 2).ok_or_else(invalid)?;
                let size = u16::from_be_bytes([size_bytes[0], size_bytes[1]]) as usize;
                let end = offset.checked_add(size).filter(|end| size >= 2 && *end <= bytes.len()).ok_or_else(invalid)?;
                if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                    if size < 8 || size != 8 + 3 * bytes[offset + 7] as usize { return Err(invalid()); }
                    icon_dimensions(u16::from_be_bytes([bytes[offset + 5], bytes[offset + 6]]) as u32,
                        u16::from_be_bytes([bytes[offset + 3], bytes[offset + 4]]) as u32)?;
                    frame = true;
                }
                if marker == 0xda {
                    if !frame || size < 6 || size != 6 + 2 * bytes[offset + 2] as usize { return Err(invalid()); }
                    offset = end;
                    while offset + 1 < bytes.len() {
                        if bytes[offset] == 0xff {
                            if bytes[offset + 1] == 0 || (0xd0..=0xd7).contains(&bytes[offset + 1]) { offset += 2; }
                            else { break; }
                        } else { offset += 1; }
                    }
                    pixels |= offset > end;
                } else { offset = end; }
            }
            Err(invalid())
        }
        "webp" => {
            if bytes.len() < 20 || !bytes.starts_with(b"RIFF") || &bytes[8..12] != b"WEBP" { return Err(invalid()); }
            let size = u32::from_le_bytes(bytes[4..8].try_into().map_err(|_| invalid())?) as usize;
            if size.checked_add(8) != Some(bytes.len()) { return Err(invalid()); }
            let mut offset = 12;
            let mut pixels = false;
            while offset + 8 <= bytes.len() {
                let kind = &bytes[offset..offset + 4];
                let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().map_err(|_| invalid())?) as usize;
                let end = offset.checked_add(8).and_then(|value| value.checked_add(size)).filter(|end| *end <= bytes.len()).ok_or_else(invalid)?;
                let data = &bytes[offset + 8..end];
                match kind {
                    b"VP8 " => {
                        if pixels || data.len() <= 10 || data[0] & 1 != 0 || data[3..6] != [0x9d, 1, 0x2a] { return Err(invalid()); }
                        icon_dimensions((u16::from_le_bytes([data[6], data[7]]) & 0x3fff) as u32,
                            (u16::from_le_bytes([data[8], data[9]]) & 0x3fff) as u32)?;
                        pixels = true;
                    }
                    b"VP8L" => {
                        if pixels || data.len() <= 5 || data[0] != 0x2f { return Err(invalid()); }
                        let bits = u32::from_le_bytes(data[1..5].try_into().map_err(|_| invalid())?);
                        if bits >> 29 != 0 { return Err(invalid()); }
                        icon_dimensions((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)?;
                        pixels = true;
                    }
                    b"VP8X" => {
                        if data.len() != 10 || data[0] & 2 != 0 { return Err("请选择静态 WebP 图标".into()); }
                        icon_dimensions(u32::from_le_bytes([data[4], data[5], data[6], 0]) + 1,
                            u32::from_le_bytes([data[7], data[8], data[9], 0]) + 1)?;
                    }
                    _ => {}
                }
                offset = end + size % 2;
            }
            if pixels && offset == bytes.len() { Ok(()) } else { Err(invalid()) }
        }
        _ => Err("请选择 PNG、JPEG 或 WebP 图标".into()),
    }
}

fn persist_icon_selection(dir: &str, version_id: &str, selected: Option<&Path>) -> Result<Option<String>, String> {
    use std::io::Read;
    let Some(source) = selected else { return Ok(None); };
    crate::named_profiles::validate_name(version_id)?;
    if !Path::new(dir).is_absolute() { return Err("请选择绝对路径的实例目录".into()); }
    let version = version_dir_of(dir, version_id)?;
    if !version.is_dir() || crate::named_profiles::is_pending_pack(&version)
        || pick_version_json(&version, version_id).is_none() {
        return Err("实例尚未安装完成，请完成安装后选择图标".into());
    }
    crate::instances::reject_links(source)?;
    let file = std::fs::File::open(source).map_err(|e| format!("读取图标失败：{e}"))?;
    let metadata = file.metadata().map_err(|e| format!("读取图标信息失败：{e}"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > ICON_MAX_BYTES {
        return Err("请选择不超过 8 MiB 的 PNG、JPEG 或 WebP 图片".into());
    }
    let extension = source.extension().and_then(|value| value.to_str()).unwrap_or_default().to_ascii_lowercase();
    let mut bytes = Vec::new();
    file.take(ICON_MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("读取图标失败：{e}"))?;
    if bytes.len() as u64 > ICON_MAX_BYTES { return Err("图标超过 8 MiB，请缩小图片后重试".into()); }
    validate_icon(&bytes, &extension)?;
    crate::app_tasks::with_idle_directory(&version, || crate::launch_state::with_idle(Path::new(dir), || {
        crate::instances::reject_links(&version)?;
        let folder = version.join(ICON_DIRECTORY);
        crate::instances::reject_links(&folder)?;
        std::fs::create_dir_all(&folder).map_err(|e| format!("创建实例图标目录失败：{e}"))?;
        let target = folder.join(format!("{}.{}", store::random_token(), extension));
        crate::named_profiles::atomic_write(&target, &bytes)?;
        Ok(Some(target.to_string_lossy().into_owned()))
    }))
}

/// 只复制所选图片；前端成功保存 icon 扩展字段后切换展示，不改版本描述或旧图标。
#[tauri::command]
pub async fn version_icon_pick(app: AppHandle, dir: String, version_id: String) -> Result<Option<String>, String> {
    crate::named_profiles::validate_name(&version_id)?;
    let version = version_dir_of(&dir, &version_id)?;
    if !Path::new(&dir).is_absolute() || !version.is_dir() { return Err("实例目录已不存在，请刷新实例列表".into()); }
    let selected = crate::pack_export::choose_file(&app, false, "选择实例图标", "", ICON_EXTENSIONS).await?;
    tokio::task::spawn_blocking(move || persist_icon_selection(&dir, &version_id, selected.as_deref()))
        .await.map_err(|e| format!("保存实例图标失败：{e}"))?
}


/// 修改版本的自定义信息（名称 / 描述 / 分类 / 收藏）：对版本 JSON 做最小侵入
/// patch，仅替换 `nickelCustom` 的四个键，其余字段原样保留，`id` 不动。
#[tauri::command]
pub fn version_rename(
    dir: String,
    version_id: String,
    name: String,
    description: String,
    category: String,
    favorite: bool,
) -> Result<(), String> {
    let _guard = store::lock();
    let id = valid_version_id(&version_id)?;
    let version_dir = version_dir_of(&dir, id)?;
    let Some(json_path) = pick_version_json(&version_dir, id) else {
        return Err(format!(
            "版本目录下没有版本 JSON 文件：{}",
            version_dir.display()
        ));
    };
    let text = std::fs::read_to_string(&json_path)
        .map_err(|e| format!("读取版本 JSON 失败（{}）：{e}", json_path.display()))?;
    let mut value: serde_json::Value = crate::instances::parse_json_lenient(&text)
        .ok_or_else(|| format!("版本 JSON 已损坏，无法修改（{}）", json_path.display()))?;
    if !patch_custom_fields(&mut value, &name, &description, &category, favorite) {
        return Err(format!("版本 JSON 顶层必须是对象：{}", json_path.display()));
    }
    let out = serde_json::to_string_pretty(&value)
        .map_err(|e| format!("序列化版本 JSON 失败: {e}"))?;
    write_atomic(&json_path, &out)
}

/// 删除 `<dir>/versions/<version_id>` 整个目录（含 jar 与版本 JSON）。
/// 二次确认由前端负责；路径穿越在 [`valid_version_id`] 拒绝。
#[tauri::command]
pub fn version_delete(dir: String, version_id: String) -> Result<(), String> {
    let id = valid_version_id(&version_id)?;
    let version_dir = version_dir_of(&dir, id)?;
    if !version_dir.is_dir() {
        return Err(format!("版本目录不存在：{}", version_dir.display()));
    }
    crate::install::with_idle_directory(&version_dir, || {
        crate::launch_state::with_idle(&version_dir, || {
            std::fs::remove_dir_all(&version_dir)
                .map_err(|e| format!("删除版本目录失败（{}）：{e}", version_dir.display()))
        })
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// 命令：版本详情 / 补全文件
// ─────────────────────────────────────────────────────────────────────────────

/// 真实版本与加载器来自扫描结果，展示字段来自 nickelCustom；缺失值按 IPC 契约回退。
fn build_detail(
    version_dir: &Path,
    version_id: &str,
    entry: Option<&serde_json::Value>,
    version_json: Option<&serde_json::Value>,
    jar_size: Option<u64>,
) -> serde_json::Value {
    let str_of = |v: Option<&serde_json::Value>| -> String {
        v.and_then(|v| v.as_str()).unwrap_or("").to_string()
    };
    let custom = version_json.and_then(|j| j.get(CUSTOM_FIELD));

    serde_json::json!({
        "id": version_id,
        "minecraftVersion": entry.and_then(|e| e.get("minecraftVersion")).cloned().unwrap_or(serde_json::Value::Null),
        "favoriteAt": custom.and_then(|c| c.get("favoriteAt")).cloned().unwrap_or(serde_json::Value::Null),
        "dir": version_dir.to_string_lossy(),
        "type": entry
            .and_then(|e| e.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
        "loader": entry
            .and_then(|e| e.get("loader"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
        "loaderVersion": entry.and_then(|e| e.get("loaderVersion")).cloned().unwrap_or(serde_json::Value::Null),
        "icon": entry
            .and_then(|e| e.get("icon"))
            .and_then(|v| v.as_str())
            .unwrap_or("redstone_block"),
        "name": str_of(custom.and_then(|c| c.get("name"))),
        "description": str_of(custom.and_then(|c| c.get("description"))),
        "favorite": custom
            .and_then(|c| c.get("favorite"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "category": str_of(custom.and_then(|c| c.get("category"))),
        "hasJar": jar_size.is_some(),
        "jarSizeBytes": jar_size.unwrap_or(0),
        "releaseTime": entry.and_then(|e| e.get("releaseTime")).cloned().unwrap_or(serde_json::Value::Null),
    })
}

/// 用版本目录匹配扫描条目，不能用可重复的展示名匹配。
fn scan_entry_for(scan_text: &str, version_dir: &Path) -> Option<serde_json::Value> {
    let arr = serde_json::from_str::<serde_json::Value>(scan_text).ok()?;
    let want = version_dir.to_string_lossy().to_string();
    arr.as_array()?
        .iter()
        .find(|e| {
            e.get("path")
                .and_then(|p| p.as_str())
                .map(|p| p.eq_ignore_ascii_case(&want))
                .unwrap_or(false)
        })
        .cloned()
}

/// 父版本 JSON 缺失时返回依赖名供详情提示，不据此将子版本判为损坏。
fn missing_dependency(
    root: &Path,
    version_json: Option<&serde_json::Value>,
    version_id: &str,
) -> Option<String> {
    let parent = version_json?
        .get("inheritsFrom")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != version_id)?;
    let path = root
        .join("versions")
        .join(parent)
        .join(format!("{parent}.json"));
    (!path.is_file()).then(|| parent.to_string())
}

/// 详情复用实例扫描的类型、加载器、图标与发布日期，避免维护两套识别规则。
/// settingsSource 与 pclUnmappedKeys 保留既有 IPC 字段；缺失依赖只作为详情提示。
#[tauri::command]
pub fn version_detail(dir: String, version_id: String) -> Result<String, String> {
    let id = valid_version_id(&version_id)?;
    let version_dir = version_dir_of(&dir, id)?;
    if !version_dir.is_dir() {
        return Err(format!("版本目录不存在：{}", version_dir.display()));
    }

    let root = version_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(Path::new(""));
    let scan_text = crate::instances::instance_scan(root.to_string_lossy().to_string());
    let entry = scan_entry_for(&scan_text, &version_dir);

    let json_text = pick_version_json(&version_dir, id)
        .and_then(|p| std::fs::read_to_string(p).ok());
    let version_json = json_text
        .as_deref()
        .and_then(crate::instances::parse_json_lenient);

    let mut detail = build_detail(
        &version_dir,
        id,
        entry.as_ref(),
        version_json.as_ref(),
        find_version_jar(&version_dir, id),
    );
    if let Some(object) = detail.as_object_mut() {
        object.insert(
            "settingsSource".into(),
            serde_json::Value::String(settings_source(root, id).into()),
        );
        object.insert(
            "missingDependency".into(),
            missing_dependency(root, version_json.as_ref(), id)
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
        object.insert(
            "pclUnmappedKeys".into(),
            serde_json::json!(legacy_launcher_unmapped_keys(&version_dir)),
        );
    }
    serde_json::to_string(&detail).map_err(|e| format!("序列化版本详情失败: {e}"))
}

/// 复用 install.rs 的安装管线「补全」版本文件：缺失 / 校验不符的 jar、libraries、
/// assets 会重新下载，已就绪的文件直接跳过（不会覆盖本地版本 JSON）。
/// 返回 taskId，进度 / 结果通过与安装一致的 `install-event` 推送。
#[tauri::command]
pub async fn version_complete_files(
    app: AppHandle,
    dir: String,
    version_id: String,
) -> Result<String, String> {
    let id = valid_version_id(&version_id)?.to_string();
    crate::install::repair_version(app, dir, id).await
}

pub(crate) const LAUNCH_INSTANCE_ARG: &str = "--launch-instance";
pub(crate) const GAME_ROOT_ARG: &str = "--game-root";

fn script_argument(value: &str, windows: bool) -> Result<String, String> {
    if value.chars().any(char::is_control) || (windows && value.contains('"')) {
        return Err("路径或实例名包含脚本不支持的控制字符，请更换保存位置或实例名称".into());
    }
    if windows {
        Ok(format!("\"{}\"", value.replace('%', "%%")))
    } else {
        Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
    }
}

fn launch_script(executable: &Path, root: &Path, version_id: &str, windows: bool) -> Result<String, String> {
    crate::named_profiles::validate_name(version_id)?;
    let executable = executable.to_str().ok_or("启动器路径包含无法编码的字符")?;
    let root = root.to_str().ok_or("游戏目录包含无法编码的字符")?;
    let command = format!(
        "{} {LAUNCH_INSTANCE_ARG} {} {GAME_ROOT_ARG} {}",
        script_argument(executable, windows)?,
        script_argument(version_id, windows)?,
        script_argument(root, windows)?
    );
    if windows {
        Ok(format!("@echo off\r\nsetlocal DisableDelayedExpansion\r\nchcp 65001 >nul\r\n{command}\r\nexit /b %errorlevel%\r\n"))
    } else {
        Ok(format!("#!/bin/sh\nexec {command}\n"))
    }
}

/// 脚本只交付实例定位参数；账号与运行设置仍由启动器在每次启动时读取。
#[tauri::command]
pub async fn version_export_script(app: AppHandle, dir: String, version_id: String) -> Result<Option<String>, String> {
    crate::named_profiles::validate_name(&version_id)?;
    let root = PathBuf::from(&dir);
    if !root.is_absolute() { return Err("请选择绝对路径的实例目录".into()); }
    let version_dir = version_dir_of(&dir, &version_id)?;
    if !version_dir.is_dir() || crate::named_profiles::is_pending_pack(&version_dir) {
        return Err("实例尚未安装完成，请完成安装后导出启动脚本".into());
    }
    let executable = std::env::current_exe().map_err(|e| format!("无法定位启动器：{e}"))?;
    let text = launch_script(&executable, &root, &version_id, cfg!(windows))?;
    let extension = if cfg!(windows) { "cmd" } else { "sh" };
    let Some(path) = crate::pack_export::choose_file(
        &app, true, "导出启动脚本", &format!("{version_id}.{extension}"), &[extension]
    ).await? else { return Ok(None); };
    crate::instances::reject_links(&path)?;
    if !path.extension().is_some_and(|value| value.eq_ignore_ascii_case(extension)) {
        return Err(format!("启动脚本扩展名必须为 .{extension}"));
    }
    let result = path.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || {
        crate::named_profiles::atomic_write(&path, text.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&path).map_err(|e| e.to_string())?.permissions();
            permissions.set_mode(permissions.mode() | 0o100);
            std::fs::set_permissions(&path, permissions).map_err(|e| format!("设置脚本执行权限失败：{e}"))?;
        }
        Ok::<_, String>(())
    }).await.map_err(|e| format!("保存启动脚本失败：{e}"))??;
    Ok(Some(result))
}

pub(crate) fn startup_launch_arguments(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Option<(String, String)>, String> {
    let args: Vec<_> = args.into_iter().collect();
    if !args.iter().any(|arg| arg == LAUNCH_INSTANCE_ARG || arg == GAME_ROOT_ARG) {
        return Ok(None);
    }
    let mut version = None;
    let mut root = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let slot = if arg == LAUNCH_INSTANCE_ARG { &mut version }
            else if arg == GAME_ROOT_ARG { &mut root }
            else { return Err("启动脚本参数无效，请在实例设置中重新导出".into()); };
        if slot.is_some() { return Err("启动脚本包含重复参数，请重新导出".into()); }
        *slot = Some(args.next().and_then(|value| value.into_string().ok()).ok_or("启动脚本参数不完整，请重新导出")?);
    }
    let version = version.ok_or("启动脚本缺少实例身份，请重新导出")?;
    let root = root.ok_or("启动脚本缺少游戏目录，请重新导出")?;
    crate::named_profiles::validate_name(&version)?;
    if !Path::new(&root).is_absolute() { return Err("启动脚本必须使用绝对游戏目录".into()); }
    Ok(Some((root, version)))
}

// ─────────────────────────────────────────────────────────────────────────────
// 单元测试（纯逻辑 / 本地，不联网）
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用临时目录（唯一名，避免并行测试互相干扰）。
    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nickel-vsettings-test-{tag}-{}",
            store::random_token()
        ))
    }

    // ── 设置默认值与损坏回退 ─────────────────────────────────────────────────

    #[test]
    fn settings_fall_back_to_defaults_on_missing_and_corruption() {
        let dir = temp_dir("fallback");
        let path = dir.join("nickel.settings.json");
        let text = read_settings_file(&path);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(value["versionIsolation"].is_null());
        assert_eq!(value["memoryMode"], MEMORY_GLOBAL);
        assert!(value["jvmArgs"].is_null());
        assert!(value["windowTitle"].is_null());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "{oops").unwrap();
        let value: serde_json::Value = serde_json::from_str(&read_settings_file(&path)).unwrap();
        assert!(value["versionIsolation"].is_null());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn settings_merge_keeps_unknown_fields_and_fixes_types() {
        let stored = serde_json::json!({
            "memoryMode": "custom", "memoryMb": 8192, "jvmArgs": "-XX:+UseG1GC",
            "icon": "anvil", "customExtra": {"a": 1}, "versionIsolation": "not-a-bool"
        });
        let merged = normalize_memory(merge_settings(default_settings_value(), stored));
        assert_eq!(merged["memoryMode"], "custom");
        assert_eq!(merged["memoryMb"], 8192);
        assert_eq!(merged["jvmArgs"], "-XX:+UseG1GC");
        assert_eq!(merged["icon"], "anvil");
        assert!(merged.get("customExtra").is_some());
        assert!(merged["versionIsolation"].is_null());
    }

    #[test]
    fn all_known_launch_fields_are_typed_and_unknown_fields_survive() {
        let stored = serde_json::json!({
            "launcherVisibility": "hide-close",
            "processPriority": "high",
            "windowMode": "same-as-launcher",
            "windowWidth": 1100,
            "windowHeight": 720,
            "memoryManagement": "auto-zgc",
            "highPerformanceGpu": false,
            "offlineSkin": "custom",
            "offlineSkinName": "skin",
            "offlineSkinPath": "C:\\skins\\skin.png",
            "offlineSkinSlim": true,
            "serverLogin": "thirdParty",
            "authServer": "https://example.invalid/api/yggdrasil",
            "serverRegistration": "https://example.invalid/auth/register",
            "serverName": "Example",
            "serverAddress": "mc.example.invalid:25565",
            "disableModUpdates": true,
            "icon": "anvil",
            "futureField": {"nested": true}
        });
        let merged = normalize_memory(merge_settings(default_settings_value(), stored));
        for (key, expected) in [
            ("launcherVisibility", serde_json::json!("hide-close")),
            ("processPriority", serde_json::json!("high")),
            ("windowMode", serde_json::json!("same-as-launcher")),
            ("windowWidth", serde_json::json!(1100)),
            ("windowHeight", serde_json::json!(720)),
            ("memoryManagement", serde_json::json!("auto-zgc")),
            ("highPerformanceGpu", serde_json::json!(false)),
            ("offlineSkin", serde_json::json!("custom")),
            ("offlineSkinName", serde_json::json!("skin")),
            ("offlineSkinPath", serde_json::json!("C:\\skins\\skin.png")),
            ("offlineSkinSlim", serde_json::json!(true)),
            ("serverLogin", serde_json::json!("thirdParty")),
            ("authServer", serde_json::json!("https://example.invalid/api/yggdrasil")),
            ("serverRegistration", serde_json::json!("https://example.invalid/auth/register")),
            ("serverName", serde_json::json!("Example")),
            ("serverAddress", serde_json::json!("mc.example.invalid:25565")),
            ("disableModUpdates", serde_json::json!(true)),
            ("icon", serde_json::json!("anvil")),
        ] {
            assert_eq!(merged[key], expected, "{key}");
        }
        assert_eq!(merged["futureField"]["nested"], true);
        let roundtrip: VersionSettings = serde_json::from_value(merged).unwrap();
        assert_eq!(roundtrip.window_width, Some(1100));
        assert_eq!(roundtrip.memory_management.as_deref(), Some("auto-zgc"));
        assert_eq!(roundtrip.server_address.as_deref(), Some("mc.example.invalid:25565"));
    }

    #[test]
    fn invalid_setting_types_are_dropped() {
        let stored = serde_json::json!({
            "windowWidth": "wide",
            "serverLogin": false,
        });
        let merged = normalize_memory(merge_settings(default_settings_value(), stored));
        assert!(merged["windowWidth"].is_null());
        assert!(merged["serverLogin"].is_null());
    }

    #[test]
    fn settings_roundtrip_keeps_inheritance_and_explicit_empty() {
        for inherited in [serde_json::Value::Null, serde_json::json!("global")] {
            let stored = serde_json::json!({"versionIsolation": inherited, "jvmArgs": inherited, "gameArgs": "", "preLaunchCommand": ""});
            let merged = normalize_memory(merge_settings(default_settings_value(), stored));
            let result = crate::runtime_settings::resolve(
                &serde_json::json!({"versionIsolation":false,"jvmArgs":"-Dglobal=1","gameArgs":"--demo","preLaunchCommand":"echo hello"}),
                &merged, None
            ).unwrap();
            assert!(!result.isolation);
            assert_eq!(result.jvm_args, ["-Dglobal=1"]);
            assert!(result.game_args.is_empty());
            assert!(result.pre_launch_command.is_empty());
        }
    }

    #[test]
    fn scripts_only_forward_identity_and_quote_special_paths() {
        let windows = launch_script(Path::new(r"C:\Launch 100%\Nickel.exe"), Path::new(r"D:\Games & Mods!"), "我的实例", true).unwrap();
        assert!(windows.contains("DisableDelayedExpansion"));
        assert!(windows.contains(r#""C:\Launch 100%%\Nickel.exe" --launch-instance "我的实例" --game-root "D:\Games & Mods!""#));
        assert!(!windows.contains("accessToken"));
        assert!(!windows.contains("username"));
        assert!(script_argument("bad\npath", true).is_err());
        assert_eq!(script_argument("a'b", false).unwrap(), "'a'\"'\"'b'");
        let root = std::env::temp_dir().to_string_lossy().into_owned();
        let args = [LAUNCH_INSTANCE_ARG, "我的实例", GAME_ROOT_ARG, &root].map(std::ffi::OsString::from);
        assert_eq!(startup_launch_arguments(args).unwrap(), Some((root, "我的实例".into())));
        for args in [vec![LAUNCH_INSTANCE_ARG], vec![GAME_ROOT_ARG, "relative"], vec![LAUNCH_INSTANCE_ARG, "a", LAUNCH_INSTANCE_ARG, "b"]] {
            assert!(startup_launch_arguments(args.into_iter().map(std::ffi::OsString::from)).is_err());
        }
    }

    #[test]
    fn memory_mode_is_normalized() {
        // 非白名单取值 → global
        let value = normalize_memory(serde_json::json!({"memoryMode": "turbo"}));
        assert_eq!(value["memoryMode"], MEMORY_GLOBAL);

        // custom 但没有 memoryMb → global
        let value = normalize_memory(serde_json::json!({"memoryMode": "custom"}));
        assert_eq!(value["memoryMode"], MEMORY_GLOBAL);

        // custom 且有 memoryMb → 保留
        let value = normalize_memory(serde_json::json!({"memoryMode": "custom", "memoryMb": 2048}));
        assert_eq!(value["memoryMode"], MEMORY_CUSTOM);
        assert_eq!(value["memoryMb"], 2048);

        // auto / global 原样保留
        for mode in [MEMORY_AUTO, MEMORY_GLOBAL] {
            let value = normalize_memory(serde_json::json!({"memoryMode": mode}));
            assert_eq!(value["memoryMode"], mode);
        }
    }

    // ── rename patch 保留未知字段 ────────────────────────────────────────────

    #[test]
    fn rename_patch_preserves_unknown_fields_and_id() {
        let mut value: serde_json::Value = serde_json::from_str(
            r#"{
                "id": "1.20.1-forge-47.2.0",
                "type": "release",
                "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
                "libraries": [{"name": "net.minecraftforge:forge:1.20.1-47.2.0"}]
            }"#,
        )
        .unwrap();

        assert!(patch_custom_fields(&mut value, "我的整合包", "主玩", "生存", true));

        // 四个自定义键写入
        let custom = value.get(CUSTOM_FIELD).unwrap();
        assert_eq!(custom["name"], "我的整合包");
        assert_eq!(custom["description"], "主玩");
        assert_eq!(custom["category"], "生存");
        assert_eq!(custom["favorite"], true);

        // 其余字段与 id 原样保留
        assert_eq!(value["id"], "1.20.1-forge-47.2.0");
        assert_eq!(value["mainClass"], "cpw.mods.bootstraplauncher.BootstrapLauncher");
        assert_eq!(value["libraries"][0]["name"], "net.minecraftforge:forge:1.20.1-47.2.0");

        // 二次 patch：保留上次写入之外的自定义键（如图标）
        value[CUSTOM_FIELD]["icon"] = serde_json::json!("anvil");
        assert!(patch_custom_fields(&mut value, "改名", "", "", false));
        let custom = value.get(CUSTOM_FIELD).unwrap();
        assert_eq!(custom["name"], "改名");
        assert_eq!(custom["favorite"], false);
        assert_eq!(custom["icon"], "anvil", "rename 不得丢掉 nickelCustom 的其它键");
    }

    #[test]
    fn rename_patch_rejects_non_object_root() {
        let mut value = serde_json::json!([1, 2, 3]);
        assert!(!patch_custom_fields(&mut value, "n", "d", "c", false));
    }

    // ── 路径穿越拒绝 ─────────────────────────────────────────────────────────

    #[test]
    fn version_id_validation_rejects_traversal_and_injection() {
        assert!(valid_version_id("1.20.1").is_ok());
        assert!(valid_version_id(" 1.20.1 ").is_ok(), "首尾空白应被裁剪");

        for bad in [
            "",
            "   ",
            ".",
            "..",
            "../escape",
            "a/b",
            "a\\b",
            "a..b",
            "C:evil",
            "stream:hidden",
            "nul\0byte",
        ] {
            let err = valid_version_id(bad).unwrap_err();
            assert!(!err.is_empty(), "必须给出原因: {bad:?}");
        }
    }

    #[test]
    fn version_dir_requires_root_dir() {
        assert!(version_dir_of("", "1.20.1").is_err());
        assert!(version_dir_of("   ", "1.20.1").is_err());
        let dir = version_dir_of("C:/mc", "1.20.1").unwrap();
        assert_eq!(dir, PathBuf::from("C:/mc").join("versions").join("1.20.1"));
    }

    // ── detail 归一化 ────────────────────────────────────────────────────────

    const SCAN_ENTRY: &str = r#"{
        "id": "1.20.1-forge",
        "type": "release",
        "loader": "forge",
        "loaderVersion": "47.2.0",
        "icon": "anvil",
        "path": "C:/mc/versions/1.20.1-forge-47.2.0",
        "releaseTime": "2023-06-12T13:25:51+00:00",
        "hasJson": true
    }"#;

    #[test]
    fn build_detail_normalizes_contract_fields() {
        let version_dir = PathBuf::from("C:/mc/versions/1.20.1-forge-47.2.0");
        let entry: serde_json::Value = serde_json::from_str(SCAN_ENTRY).unwrap();
        let version_json: serde_json::Value = serde_json::from_str(
            r#"{
                "id": "1.20.1-forge",
                "nickelCustom": {
                    "name": "我的整合包",
                    "description": "主玩",
                    "category": "生存",
                    "favorite": true,
                    "icon": "anvil"
                }
            }"#,
        )
        .unwrap();

        let detail = build_detail(
            &version_dir,
            "1.20.1-forge-47.2.0",
            Some(&entry),
            Some(&version_json),
            Some(24_632_320),
        );

        assert_eq!(detail["id"], "1.20.1-forge-47.2.0");
        assert_eq!(
            detail["dir"],
            serde_json::Value::String(version_dir.to_string_lossy().to_string())
        );
        assert_eq!(detail["type"], "release");
        assert_eq!(detail["loader"], "forge");
        assert_eq!(detail["loaderVersion"], "47.2.0");
        assert_eq!(detail["icon"], "anvil");
        assert_eq!(detail["name"], "我的整合包");
        assert_eq!(detail["description"], "主玩");
        assert_eq!(detail["category"], "生存");
        assert_eq!(detail["favorite"], true);
        assert_eq!(detail["hasJar"], true);
        assert_eq!(detail["jarSizeBytes"], 24_632_320);
        assert_eq!(detail["releaseTime"], "2023-06-12T13:25:51+00:00");
    }

    #[test]
    fn build_detail_falls_back_when_scan_and_json_missing() {
        let version_dir = PathBuf::from("C:/mc/versions/broken");
        let detail = build_detail(&version_dir, "broken", None, None, None);

        assert_eq!(detail["id"], "broken");
        assert_eq!(detail["type"], "unknown");
        assert_eq!(detail["loader"], "unknown");
        assert_eq!(detail["icon"], "redstone_block");
        assert_eq!(detail["name"], "");
        assert_eq!(detail["favorite"], false);
        assert_eq!(detail["hasJar"], false);
        assert_eq!(detail["jarSizeBytes"], 0);
        assert_eq!(detail["releaseTime"], serde_json::Value::Null);
        assert_eq!(detail["loaderVersion"], serde_json::Value::Null);
    }

    #[test]
    fn version_detail_recognizes_optifine_and_liteloader_profiles() {
        for (name, loader, loader_version, minecraft, library) in [
            (
                "OptiFine world",
                "optifine",
                "1.20.1_HD_U_I6",
                "1.20.1",
                "optifine:OptiFine:1.20.1_HD_U_I6",
            ),
            (
                "LiteLoader world",
                "liteloader",
                "1.12.2-SNAPSHOT-r0",
                "1.12.2",
                "com.mumfrey:liteloader:1.12.2-SNAPSHOT-r0",
            ),
        ] {
            let root = temp_dir(&format!("detail-{loader}"));
            let version = root.join("versions").join(name);
            std::fs::create_dir_all(&version).unwrap();
            std::fs::write(
                version.join(format!("{name}.json")),
                serde_json::to_vec(&serde_json::json!({
                    "id": name,
                    "type": "release",
                    "mainClass": "net.minecraft.launchwrapper.Launch",
                    "libraries": [{"name": library}],
                    "nickelProfile": {
                        "minecraftVersion": minecraft,
                        "loader": loader,
                        "loaderVersion": loader_version
                    }
                }))
                .unwrap(),
            )
            .unwrap();
            let detail = version_detail(root.to_string_lossy().into_owned(), name.into()).unwrap();
            let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
            assert_eq!(detail["loader"], loader);
            assert_eq!(detail["loaderVersion"], loader_version);
            assert_eq!(detail["minecraftVersion"], minecraft);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn scan_entry_finds_by_path() {
        let scan = format!("[{}, {{}}]", SCAN_ENTRY.trim());
        let found = scan_entry_for(&scan, Path::new("C:/mc/versions/1.20.1-forge-47.2.0"));
        assert!(found.is_some());
        assert_eq!(found.unwrap()["loader"], "forge");

        // 大小写不敏感（Windows 路径习惯）
        let found = scan_entry_for(&scan, Path::new("c:/MC/VERSIONS/1.20.1-forge-47.2.0"));
        assert!(found.is_some());

        assert!(scan_entry_for(&scan, Path::new("C:/mc/versions/other")).is_none());
        assert!(scan_entry_for("[]", Path::new("C:/mc/versions/other")).is_none());
        assert!(scan_entry_for("{oops", Path::new("C:/mc/versions/other")).is_none());
    }

    #[test]
    fn icon_selection_is_bounded_persistent_and_cancel_is_read_only() {
        let root = temp_dir("icon-pick");
        let version = root.join("versions/icon-test");
        std::fs::create_dir_all(&version).unwrap();
        let descriptor = version.join("icon-test.json");
        std::fs::write(&descriptor, br#"{"id":"1.16.5","nickelCustom":{"name":"Keep"}}"#).unwrap();
        let dir = root.to_string_lossy().into_owned();
        let source = root.join("selected.PNG");
        let png = include_bytes!("../../src/assets/blocks/Grass.png");
        std::fs::write(&source, png).unwrap();
        let before = std::fs::read(&descriptor).unwrap();
        assert_eq!(persist_icon_selection(&dir, "icon-test", None).unwrap(), None);
        assert!(!version.join(ICON_DIRECTORY).exists());
        assert!(persist_icon_selection(&dir, "../escape", Some(&source)).is_err());
        assert!(persist_icon_selection(&dir, "missing", Some(&source)).is_err());
        {
            let _install = crate::app_tasks::test_reserve(&version);
            assert!(persist_icon_selection(&dir, "icon-test", Some(&source)).is_err());
        }
        {
            let _running = crate::launch_state::begin(&root).unwrap();
            assert!(persist_icon_selection(&dir, "icon-test", Some(&source)).is_err());
        }
        assert!(!version.join(ICON_DIRECTORY).exists());
        let icon = persist_icon_selection(&dir, "icon-test", Some(&source)).unwrap().unwrap();
        assert!(Path::new(&icon).starts_with(version.join(ICON_DIRECTORY)));
        assert_eq!(Path::new(&icon).extension().unwrap(), "png");
        std::fs::remove_file(&source).unwrap();
        assert_eq!(std::fs::read(&icon).unwrap(), png);
        assert_eq!(std::fs::read(&descriptor).unwrap(), before);
        assert!(!version.join(SETTINGS_FILE_NAME).exists());
        version_settings_set(dir.clone(), "icon-test".into(), serde_json::json!({"icon":icon}).to_string()).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&version_settings_get(dir.clone(), "icon-test".into()).unwrap()).unwrap();
        assert_eq!(settings["icon"], icon);
        let scan: serde_json::Value = serde_json::from_str(&crate::instances::instance_scan(dir.clone())).unwrap();
        assert_eq!(scan[0]["icon"], icon);
        assert_eq!(persist_icon_selection(&dir, "icon-test", None).unwrap(), None);
        for bytes in [b"<html>fake</html>".to_vec(), png[..20].to_vec(), vec![0; ICON_MAX_BYTES as usize + 1]] {
            std::fs::write(&source, bytes).unwrap();
            assert!(persist_icon_selection(&dir, "icon-test", Some(&source)).is_err());
            assert_eq!(std::fs::read(&icon).unwrap(), png);
            assert_eq!(std::fs::read_dir(version.join(ICON_DIRECTORY)).unwrap().count(), 1);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn icon_formats_accept_real_images_and_reject_truncation_and_oversize() {
        use base64::Engine;
        // 2×3 纯色 Canvas 编码样本，已由浏览器 Image.decode 验证，不含外部图像资产。
        let jpeg = base64::engine::general_purpose::STANDARD.decode("/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgICAgMCAgIDAwMDBAYEBAQEBAgGBgUGCQgKCgkICQkKDA8MCgsOCwkJDRENDg8QEBEQCgwSExIQEw8QEBD/2wBDAQMDAwQDBAgEBAgQCwkLEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBD/wAARCAADAAIDASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAf/xAAUEAEAAAAAAAAAAAAAAAAAAAAA/8QAFQEBAQAAAAAAAAAAAAAAAAAABQf/xAAUEQEAAAAAAAAAAAAAAAAAAAAA/9oADAMBAAIRAxEAPwCTgCkqf//Z").unwrap();
        let webp = base64::engine::general_purpose::STANDARD.decode("UklGRjQAAABXRUJQVlA4ICgAAABwAQCdASoCAAMAAUAmJaACdAFAAAD+73bUYTSTo/+LQ/t5/wa2lAAA").unwrap();
        let png = include_bytes!("../../src/assets/blocks/Grass.png");
        for (bytes, format) in [(png.as_slice(), "png"), (jpeg.as_slice(), "jpeg"), (jpeg.as_slice(), "jpg"), (webp.as_slice(), "webp")] {
            validate_icon(bytes, format).unwrap();
            for length in 0..bytes.len() { assert!(validate_icon(&bytes[..length], format).is_err(), "{format}: {length}"); }
        }
        let mut huge_png = png.to_vec();
        huge_png[16..20].copy_from_slice(&(ICON_MAX_DIMENSION + 1).to_be_bytes());
        assert!(validate_icon(&huge_png, "png").is_err());
        let mut huge_jpeg = jpeg.clone();
        let frame = huge_jpeg.windows(2).position(|bytes| bytes == [0xff, 0xc0]).unwrap();
        huge_jpeg[frame + 7..frame + 9].copy_from_slice(&((ICON_MAX_DIMENSION + 1) as u16).to_be_bytes());
        assert!(validate_icon(&huge_jpeg, "jpeg").is_err());
        let mut huge_webp = webp.clone();
        huge_webp[26..28].copy_from_slice(&((ICON_MAX_DIMENSION + 1) as u16).to_le_bytes());
        assert!(validate_icon(&huge_webp, "webp").is_err());
        assert!(validate_icon(png, "jpeg").is_err());
        assert!(validate_icon(b"RIFF\x04\0\0\0WEBP", "webp").is_err());
        assert!(validate_icon(png, "svg").is_err());
    }

    #[test]
    fn legacy_launcher_setup_maps_real_keys_and_lists_unmapped() -> Result<(), String> {
        let root = temp_dir("pcl-map");
        let directory = root.join("versions").join("灾厄巫咒");
        std::fs::create_dir_all(directory.join("PCL")).map_err(|e| e.to_string())?;
        let setup = "\u{feff}VersionVanillaName:1.20.1\r\n\
VersionForge:47.4.10\r\n\
VersionArgumentIndieV2:True\r\n\
VersionRamType:1\r\n\
VersionRamCustom:25\r\n\
VersionAdvanceGC:5\r\n\
VersionAdvanceJvm:-XX:+UseStringDeduplication -XX:MaxMetaspaceSize=1024M\r\n\
VersionAdvanceGame:--width 1280\r\n\
VersionAdvanceRun:echo ready\r\n\
VersionAdvanceDisableModUpdate:True\r\n\
VersionAdvanceAssetsV2:True\r\n\
VersionAdvanceDisableJLW:True\r\n\
VersionAdvanceDisableLUA:True\r\n\
VersionServerLogin:4\r\n\
VersionServerAuthServer:https://littleskin.cn/api/yggdrasil\r\n\
VersionServerAuthRegister:https://littleskin.cn/auth/register\r\n\
VersionServerAuthName:LittleSkin 登录\r\n\
VersionServerEnter:mc.example.invalid:25565\r\n\
VersionArgumentTitle:My Title\r\n\
CustomInfo:My Info\r\n\
VersionArgumentJavaSelect:{\"Path\":\"C:\\\\Java\\\\bin\\\\java.exe\"}\r\n\
VersionVanilla:20.0.1\r\n\
Info:正式版 1.20.1, Forge 47.4.10\r\n\
VersionArgumentIndie:1\r\n";
        std::fs::write(directory.join("PCL/Setup.ini"), setup).map_err(|e| e.to_string())?;

        let settings = legacy_launcher_effective_settings(&root, "灾厄巫咒").expect("应识别 PCL 配置");
        assert_eq!(settings["versionIsolation"], true);
        assert_eq!(settings["memoryMode"], "custom");
        assert_eq!(settings["memoryMb"], 8192);
        assert_eq!(settings["memoryManagement"], "g1-tuned");
        assert_eq!(
            settings["jvmArgs"],
            "-XX:+UseStringDeduplication -XX:MaxMetaspaceSize=1024M"
        );
        assert_eq!(settings["gameArgs"], "--width 1280");
        assert_eq!(settings["preLaunchCommand"], "echo ready");
        assert_eq!(settings["disableModUpdates"], true);
        assert_eq!(settings["disableFileCheck"], true);
        assert_eq!(settings["disableWrapper"], true);
        assert_eq!(settings["disableLwjglAgent"], true);
        assert_eq!(settings["serverLogin"], "thirdParty");
        assert_eq!(settings["authServer"], "https://littleskin.cn/api/yggdrasil");
        assert_eq!(
            settings["serverRegistration"],
            "https://littleskin.cn/auth/register"
        );
        assert_eq!(settings["serverName"], "LittleSkin 登录");
        assert_eq!(settings["serverAddress"], "mc.example.invalid:25565");
        assert_eq!(settings["windowTitle"], "My Title");
        // CustomInfo 是用户输入，Info 是扫描展示缓存，两者不能混用。
        assert_eq!(settings["customInfo"], "My Info");
        assert_eq!(settings["javaPath"], r"C:\Java\bin\java.exe");

        let audit = legacy_launcher_unmapped_keys(&directory);
        assert!(audit.contains(&"VersionArgumentIndie".to_string()));
        assert!(!audit.contains(&"VersionForge".to_string()));
        assert!(!audit.contains(&"VersionVanilla".to_string()), "内部编码不应进入审计");

        std::fs::remove_dir_all(root).map_err(|e| e.to_string())?;
        Ok(())
    }

    #[test]
    fn legacy_launcher_setup_uses_last_duplicate_and_preserves_colons() -> Result<(), String> {
        let root = temp_dir("pcl-ini");
        let directory = root.join("versions").join("id");
        std::fs::create_dir_all(directory.join("PCL")).map_err(|e| e.to_string())?;
        std::fs::write(
            directory.join("PCL/Setup.ini"),
            "\u{feff}VersionServerAuthServer:https://first.invalid\r\n\
VersionServerAuthServer:https://second.invalid\r\n\
:ignored-key\r\n\
VersionVanillaName:1.20.1\r\n",
        )
        .map_err(|e| e.to_string())?;
        let setup = read_legacy_launcher_setup(&directory).expect("应读取 Setup.ini");
        assert_eq!(setup.get("VersionServerAuthServer"), Some("https://second.invalid"));
        assert_eq!(setup.get("VersionVanillaName"), Some("1.20.1"));
        assert_eq!(setup.get(""), None);
        std::fs::remove_dir_all(root).map_err(|e| e.to_string())?;
        Ok(())
    }

    #[test]
    fn settings_source_prefers_local_and_first_save_materializes_own_file() -> Result<(), String> {
        let root = temp_dir("pcl-source");
        let directory = root.join("versions").join("id");
        std::fs::create_dir_all(directory.join("PCL")).map_err(|e| e.to_string())?;
        std::fs::write(
            directory.join("PCL/Setup.ini"),
            "VersionArgumentIndieV2:True\r\nVersionRamType:1\r\nVersionRamCustom:25\r\n",
        )
        .map_err(|e| e.to_string())?;
        let dir = root.to_string_lossy().into_owned();

        assert_eq!(settings_source(&root, "id"), "pcl");
        assert_eq!(
            crate::runtime_settings::read_local(&root, "id")["versionIsolation"],
            true
        );
        let loaded: serde_json::Value =
            serde_json::from_str(&version_settings_get(dir.clone(), "id".into())?).unwrap();
        assert_eq!(loaded["settingsSource"], "pcl");
        assert_eq!(loaded["memoryMode"], "custom");
        assert_eq!(loaded["memoryMb"], 8192);

        // 首次保存创建本地设置文件，来源元数据不落盘。
        version_settings_set(
            dir.clone(),
            "id".into(),
            serde_json::json!({"windowTitle":"Saved","settingsSource":"pcl"}).to_string(),
        )?;
        let own = std::fs::read_to_string(directory.join(SETTINGS_FILE_NAME)).unwrap();
        assert!(!own.contains("settingsSource"));
        let loaded: serde_json::Value =
            serde_json::from_str(&version_settings_get(dir.clone(), "id".into())?).unwrap();
        assert_eq!(loaded["settingsSource"], "nickel");
        assert_eq!(loaded["windowTitle"], "Saved");
        assert_eq!(loaded["memoryMode"], "global", "Nickel 配置优先后不再套用 PCL");

        // 无外部设置时仍返回默认来源。
        let plain = root.join("versions").join("plain");
        std::fs::create_dir_all(&plain).map_err(|e| e.to_string())?;
        assert!(legacy_launcher_effective_settings(&root, "plain").is_none());
        assert_eq!(settings_source(&root, "plain"), "nickel");

        std::fs::remove_dir_all(root).map_err(|e| e.to_string())?;
        Ok(())
    }

    #[test]
    fn global_legacy_launcher_setup_fills_empty_instance_values() -> Result<(), String> {
        let root = temp_dir("pcl-global");
        std::fs::create_dir_all(root.join("PCL")).map_err(|e| e.to_string())?;
        std::fs::write(
            root.join("PCL/Setup.ini"),
            "LaunchAdvanceJvm:-Dglobal=1\r\n\
LaunchAdvanceGame:--demo\r\n\
LaunchArgumentTitle:Global Title\r\n\
LaunchAdvanceDisableJLW:True\r\n\
LaunchRamType:1\r\n\
LaunchRamCustom:25\r\n\
LaunchAdvanceGC:0\r\n",
        )
        .map_err(|e| e.to_string())?;
        let directory = root.join("versions").join("id");
        std::fs::create_dir_all(directory.join("PCL")).map_err(|e| e.to_string())?;
        std::fs::write(
            directory.join("PCL/Setup.ini"),
            "VersionRamType:2\r\nVersionAdvanceJvm:\r\nVersionArgumentTitle:\r\nVersionAdvanceGC:0\r\n",
        )
        .map_err(|e| e.to_string())?;

        let settings = legacy_launcher_effective_settings(&root, "id").expect("应识别 PCL 配置");
        assert_eq!(settings["jvmArgs"], "-Dglobal=1");
        assert_eq!(settings["gameArgs"], "--demo");
        assert_eq!(settings["windowTitle"], "Global Title");
        assert_eq!(settings["disableWrapper"], true);
        assert_eq!(settings["memoryMode"], "custom");
        assert_eq!(settings["memoryMb"], 8192);
        assert_eq!(settings["memoryManagement"], "auto-zgc");

        std::fs::remove_dir_all(root).map_err(|e| e.to_string())?;
        Ok(())
    }

    #[test]
    fn version_detail_reports_external_source_missing_dependency_and_unmapped_keys() -> Result<(), String> {
        let root = temp_dir("pcl-detail");
        let directory = root.join("versions").join("child");
        std::fs::create_dir_all(directory.join("PCL")).map_err(|e| e.to_string())?;
        std::fs::write(
            directory.join("child.json"),
            r#"{"id":"child","type":"release","mainClass":"net.minecraft.client.main.Main","inheritsFrom":"missing-parent"}"#,
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(
            directory.join("PCL/Setup.ini"),
            "VersionVanillaName:1.20.1\r\nVersionArgumentIndie:1\r\n",
        )
        .map_err(|e| e.to_string())?;

        let dir = root.to_string_lossy().into_owned();
        let detail: serde_json::Value =
            serde_json::from_str(&version_detail(dir.clone(), "child".into())?).unwrap();
        assert_eq!(detail["minecraftVersion"], "1.20.1");
        assert_eq!(detail["settingsSource"], "pcl");
        assert_eq!(detail["missingDependency"], "missing-parent");
        assert_eq!(detail["hasJar"], false, "缺少版本 JAR 只提示，不判损坏");
        assert!(detail["pclUnmappedKeys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "VersionArgumentIndie"));

        // 父版本恢复后，继承链优先于外部扫描缓存。
        let parent = root.join("versions").join("missing-parent");
        std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
        std::fs::write(
            parent.join("missing-parent.json"),
            r#"{"id":"missing-parent","type":"release","mainClass":"net.minecraft.client.main.Main","clientVersion":"1.19.4"}"#,
        )
        .map_err(|e| e.to_string())?;
        let detail: serde_json::Value =
            serde_json::from_str(&version_detail(dir, "child".into())?).unwrap();
        assert_eq!(detail["minecraftVersion"], "1.19.4");
        assert_eq!(detail["missingDependency"], serde_json::Value::Null);

        std::fs::remove_dir_all(root).map_err(|e| e.to_string())?;
        Ok(())
    }


}
