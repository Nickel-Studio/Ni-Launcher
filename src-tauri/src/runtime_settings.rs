//! 全局与独立设置的唯一启动解析器；持久化保留未知键，启动只消费已实现能力。
use std::path::{Path, PathBuf};

use minecraft_java_rs_core::launcher::{Launcher, options::LaunchOptions};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::launch_diagnostics::{
    self, DiagnosticInput, DiagnosticReport, MAX_CRASH_REPORT_BYTES, MAX_LATEST_LOG_BYTES,
};
use crate::launch_log_store::{self, LaunchLogStore};
use crate::memory_plan::{self, VersionKind};

pub const DEFAULT_MEMORY_MB: u64 = 4096;
const DEFAULT_WIDTH: u32 = 854;
const DEFAULT_HEIGHT: u32 = 480;
const PRE_LAUNCH_TIMEOUT_SECS: u64 = 60;
const LOG_LINE_MAX_CHARS: usize = 8 * 1024;
const DIAGNOSTIC_CRASH_REPORTS: usize = 3;

#[derive(Debug, Clone)]
pub struct EffectiveSettings {
    pub isolation: bool,
    pub java_path: String,
    pub window_title: String,
    pub custom_info: String,
    pub visibility: String,
    pub priority: String,
    pub window_mode: String,
    pub width: u32,
    pub height: u32,
    pub memory_mb: u64,
    pub high_performance_gpu: bool,
    pub disable_wrapper: bool,
    pub disable_lwjgl_agent: bool,
    pub offline_skin: String,
    pub offline_skin_name: String,
    pub offline_skin_path: String,
    pub offline_skin_slim: bool,
    pub jvm_args: Vec<String>,
    pub game_args: Vec<String>,
    pub pre_launch_command: String,
    pub memory_management: String,
    pub disable_file_check: bool,
    pub server_login: String,
    pub server_name: String,
    pub auth_server: String,
    pub server_address: String,
}
fn inherited<'a>(global: &'a Value, local: &'a Value, key: &str) -> Option<&'a Value> {
    local
        .get(key)
        .filter(|v| !v.is_null() && v.as_str() != Some("global"))
        .or_else(|| global.get(key))
}
fn text(global: &Value, local: &Value, key: &str, default: &str) -> String {
    inherited(global, local, key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .into()
}
fn choice(value: String, choices: &[&str], label: &str) -> Result<String, String> {
    if choices.contains(&value.as_str()) {
        Ok(value)
    } else {
        Err(format!("{label}无效，请重新选择"))
    }
}

/// 保留 Windows 路径反斜杠，支持引号与显式转义，不通过 shell 解释 JVM/游戏参数。
pub fn split_arguments(raw: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\0' {
            return Err("启动参数不能包含 NUL 字符".into());
        }
        if ch == '\\'
            && chars
                .peek()
                .is_some_and(|next| Some(*next) == quote || *next == '"')
        {
            if let Some(next) = chars.next() {
                current.push(next);
            }
            started = true;
        } else if Some(ch) == quote {
            quote = None;
        } else if quote.is_none() && (ch == '\'' || ch == '"') {
            quote = Some(ch);
            started = true;
        } else if quote.is_none() && ch.is_whitespace() {
            if started {
                args.push(std::mem::take(&mut current));
                started = false;
            }
        } else {
            current.push(ch);
            started = true;
        }
    }
    if quote.is_some() {
        return Err("启动参数的引号未闭合，请检查后重试".into());
    }
    if started {
        args.push(current);
    }
    Ok(args)
}

pub fn normalize_memory_management(value: &str) -> Result<String, String> {
    let mode = match value.trim() {
        "" => "g1-tuned",
        other => other,
    };
    choice(
        mode.to_string(),
        &[
            "auto-zgc",
            "generational-zgc",
            "g1",
            "g1-tuned",
            "custom",
        ],
        "内存管理",
    )
}

/// 独立解析隔离状态，供自动内存规划在正式 `resolve` 前定位有效资源根。
pub fn resolve_isolation(global: &Value, local: &Value) -> bool {
    inherited(global, local, "versionIsolation")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// 生成实际 GC 参数；`None` 表示自定义模式，核心会恢复用户原始 GC 参数。
pub fn gc_arguments(
    mode: &str,
    java_major: u32,
    windows_build: Option<u32>,
) -> Result<Option<Vec<String>>, String> {
    let mode = normalize_memory_management(mode)?;
    if mode == "custom" {
        return Ok(None);
    }
    // ZGC 需要 Windows 10 1809（build 17763）及以上，旧版本回退标准 G1。
    // 传入 build 号就按号判定（与宿主平台无关，便于单测跨平台模拟旧 Windows）；
    // 未传入时仅在非 Windows 平台放行，Windows 检测失败保守回退 G1。
    let windows_zgc_supported = windows_build.map_or(!cfg!(windows), |build| build >= 17_763);
    let zgc_for_auto = java_major >= 15 && windows_zgc_supported;
    let zgc_for_generational = java_major >= 21 && windows_zgc_supported;
    let use_zgc = matches!(mode.as_str(), "auto-zgc") && zgc_for_auto
        || matches!(mode.as_str(), "generational-zgc") && zgc_for_generational;
    let tuned_g1 = mode == "g1-tuned";
    let mut args = vec!["-XX:+UnlockExperimentalVMOptions".to_string()];
    if java_major >= 24 {
        args.push("-XX:+UseCompactObjectHeaders".into());
    }
    match mode.as_str() {
        _ if use_zgc => {
            args.push("-XX:+UseZGC".into());
            if (21..=22).contains(&java_major) {
                args.push("-XX:+ZGenerational".into());
            }
        }
        _ => {
            args.push("-XX:+UseG1GC".into());
            args.push("-XX:G1NewSizePercent=20".into());
            args.push("-XX:G1ReservePercent=20".into());
            args.push("-XX:G1HeapRegionSize=32M".into());
            args.push("-XX:MaxGCPauseMillis=50".into());
            if java_major <= 7 {
                args.push("-XX:MaxPermSize=512m".into());
            }
            if tuned_g1 {
                args.push("-XX:+PerfDisableSharedMem".into());
                if java_major == 8 {
                    args.push("-XX:+ParallelRefProcEnabled".into());
                }
                if java_major >= 12 {
                    args.push("-XX:MinHeapFreeRatio=25".into());
                    args.push("-XX:MaxHeapFreeRatio=40".into());
                }
            }
        }
    }
    Ok(Some(args))
}

/// 测试用简化内存探测；真实启动同时提供总量、可用量和 Mod 数。
#[cfg(test)]
pub fn resolve(
    global: &Value,
    local: &Value,
    available: Option<u64>,
) -> Result<EffectiveSettings, String> {
    resolve_with_memory(
        global,
        local,
        available,
        available,
        0,
        VersionKind::Regular,
    )
}

pub fn resolve_with_memory(
    global: &Value,
    local: &Value,
    total_memory: Option<u64>,
    available_memory: Option<u64>,
    mod_count: u64,
    version_kind: VersionKind,
) -> Result<EffectiveSettings, String> {
    let mode = choice(
        text(
            global,
            local,
            "memoryMode",
            "auto",
        ),
        &["auto", "custom"],
        "内存模式",
    )?;
    let memory_mb = if mode == "auto" {
        memory_plan::automatic_memory_mb(available_memory, total_memory, mod_count, version_kind)
    } else {
        let saved = inherited(global, local, "memoryMb")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_MEMORY_MB);
        memory_plan::manual_memory_mb(saved, total_memory)?
    };
    let width = inherited(global, local, "windowWidth")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_WIDTH as u64)
        .clamp(320, 16384) as u32;
    let height = inherited(global, local, "windowHeight")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_HEIGHT as u64)
        .clamp(240, 16384) as u32;
    let server_name = text(global, local, "serverName", "");
    if server_name.len() > 256 || server_name.chars().any(char::is_control) {
        return Err("服务器名称须少于 256 字节，且不能包含控制字符".into());
    }
    Ok(EffectiveSettings {
        isolation: inherited(global, local, "versionIsolation")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        java_path: text(global, local, "javaPath", ""),
        window_title: text(global, local, "windowTitle", ""),
        custom_info: text(global, local, "customInfo", ""),
        visibility: choice(
            text(global, local, "launcherVisibility", "keep"),
            &["keep", "hide", "hide-close", "close", "minimize"],
            "启动器可见性",
        )?,
        priority: choice(
            text(global, local, "processPriority", "normal"),
            &["low", "normal", "high"],
            "进程优先级",
        )?,
        window_mode: choice(
            text(global, local, "windowMode", "default"),
            &[
                "default",
                "custom",
                "fullscreen",
                "same-as-launcher",
                "maximize",
            ],
            "窗口大小",
        )?,
        width,
        height,
        memory_mb,
        high_performance_gpu: inherited(global, local, "highPerformanceGpu")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        disable_wrapper: inherited(global, local, "disableWrapper")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        disable_lwjgl_agent: inherited(global, local, "disableLwjglAgent")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        offline_skin: choice(
            text(global, local, "offlineSkin", "random"),
            &["random", "steve", "alex", "official", "custom"],
            "离线皮肤",
        )?,
        offline_skin_name: text(global, local, "offlineSkinName", ""),
        offline_skin_path: text(global, local, "offlineSkinPath", ""),
        offline_skin_slim: inherited(global, local, "offlineSkinSlim")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        jvm_args: split_arguments(&text(global, local, "jvmArgs", ""))?,
        game_args: split_arguments(&text(global, local, "gameArgs", ""))?,
        pre_launch_command: text(global, local, "preLaunchCommand", ""),
        memory_management: normalize_memory_management(&text(
            global,
            local,
            "memoryManagement",
            "g1-tuned",
        ))?,
        disable_file_check: inherited(global, local, "disableFileCheck")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        server_login: choice(
            text(global, local, "serverLogin", "any"),
            &["any", "offline", "microsoft", "thirdParty"],
            "登录方式",
        )?,
        server_name,
        auth_server: text(global, local, "authServer", ""),
        server_address: text(global, local, "serverAddress", ""),
    })
}

pub fn read_local(root: &Path, version: &str) -> Value {
    // 本地覆盖来源及兼容格式的优先级由 effective_local 统一决定；全局默认在启动解析时合并。
    crate::version_settings::effective_local(root, version).0
}
pub fn game_directory(root: &Path, version: &str, isolation: bool) -> PathBuf {
    if isolation {
        root.join("versions").join(version)
    } else {
        root.to_path_buf()
    }
}

/// 读取指定实例的有效 `versionIsolation`。
///
/// `resource_directory`（启动路径）与 `resource_directory_for_cleanup`（清理路径）
/// 必须共用同一套设置解析，否则清理目标会与游戏实际写入位置漂移。
fn isolation_for(app: &AppHandle, root: &Path, version: &str) -> Result<bool, String> {
    let global: Value =
        serde_json::from_str(&crate::game::game_settings_get(app.clone())?).unwrap_or(Value::Null);
    let local = read_local(root, version);
    Ok(inherited(&global, &local, "versionIsolation")
        .and_then(Value::as_bool)
        .unwrap_or(true))
}

pub fn resource_directory(app: &AppHandle, root: &Path, version: &str) -> Result<PathBuf, String> {
    crate::named_profiles::validate_name(version)?;
    if !root.is_absolute() || !root.join("versions").join(version).is_dir() {
        return Err("实例目录已不存在，请返回实例列表重新选择".into());
    }
    crate::instances::reject_links(&root.join("versions").join(version))?;
    let isolation = isolation_for(app, root, version)?;
    Ok(game_directory(root, version, isolation))
}

/// 清理路径专用的版本目录校验：版本名、绝对路径、版本目录存在、版本目录自身不是
/// 符号链接 / Windows 重解析点，且 canonicalize 后仍位于实例根内。
///
/// 与 `resource_directory` 的关键区别：只拒绝版本目录**自身**的链接，不拒绝祖先。
/// macOS 的 `/var` → `/private/var`、Windows 的 OneDrive / junction 重定向都会让
/// 合法实例带链接祖先；若连祖先一起拒绝，`game_junk` 会静默跳过这些实例的日志。
pub(crate) fn cleanup_version_directory(root: &Path, version: &str) -> Result<PathBuf, String> {
    crate::named_profiles::validate_name(version)?;
    if !root.is_absolute() {
        return Err("实例目录必须是绝对路径".into());
    }
    let version_dir = root.join("versions").join(version);
    if !version_dir.is_dir() {
        return Err("实例目录已不存在，请返回实例列表重新选择".into());
    }
    // 必须在 canonicalize 之前检查原始路径：canonicalize 会解析掉链接，
    // 之后再检查永远看不到链接本身。
    crate::instances::reject_links_local(&version_dir)?;
    let canonical_root =
        std::fs::canonicalize(root).map_err(|error| format!("无法解析实例目录：{error}"))?;
    let canonical_version = std::fs::canonicalize(&version_dir)
        .map_err(|error| format!("无法解析版本目录：{error}"))?;
    // 版本目录自身不是链接，但上层的 `versions/` 仍可能是链接；只有 canonicalize
    // 后才能发现目标被引到实例之外，避免清理误删别处的日志。
    if !canonical_version.starts_with(&canonical_root) {
        return Err("版本目录不在实例目录内，已跳过清理".into());
    }
    Ok(version_dir)
}

/// 清理路径的资源目录解析：与启动路径使用同一隔离设置，但只信任版本目录自身。
pub(crate) fn resource_directory_for_cleanup(
    app: &AppHandle,
    root: &Path,
    version: &str,
) -> Result<PathBuf, String> {
    cleanup_version_directory(root, version)?;
    let isolation = isolation_for(app, root, version)?;
    Ok(game_directory(root, version, isolation))
}

/// 测试用纯逻辑入口：不构造 `AppHandle` 即可覆盖「祖先链接 / 自身链接 / 逃逸」场景。
#[cfg(test)]
pub(crate) fn cleanup_game_directory(
    root: &Path,
    version: &str,
    isolation: bool,
) -> Result<PathBuf, String> {
    cleanup_version_directory(root, version)?;
    Ok(game_directory(root, version, isolation))
}

pub(crate) fn registration_url(global: &Value, local: &Value) -> Result<String, String> {
    let raw = text(
        global,
        local,
        "serverRegistration",
        &text(global, local, "registerUrl", ""),
    );
    let url = minecraft_java_rs_core::net::client::reqwest::Url::parse(raw.trim())
        .map_err(|_| "请在实例服务器设置中填写完整的 HTTP 或 HTTPS 注册链接")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("注册链接必须为不含凭据的 HTTP 或 HTTPS 页面".into());
    }
    Ok(url.into())
}

#[tauri::command]
pub fn version_server_open_registration(
    app: AppHandle,
    dir: String,
    version_id: String,
) -> Result<(), String> {
    let root = PathBuf::from(dir);
    resource_directory(&app, &root, &version_id)?;
    let global: Value =
        serde_json::from_str(&crate::game::game_settings_get(app)?).unwrap_or(Value::Null);
    let local = read_local(&root, &version_id);
    let name = text(&global, &local, "serverName", "此实例");
    let url = registration_url(&global, &local).map_err(|error| format!("{name}：{error}"))?;
    tauri_plugin_opener::open_url(url, None::<&str>)
        .map_err(|e| format!("打开{name}注册页面失败，请检查默认浏览器：{e}"))
}

fn supports_quick_play(version: &str) -> bool {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|part| part.parse::<u32>().ok());
    let minor = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    major.is_some_and(|major| major > 1 || major == 1 && minor >= 20)
}

/// Tauri 的 `inner_size` 是物理像素；游戏窗口参数需要逻辑像素。
pub fn logical_window_size(width: u32, height: u32, scale_factor: f64) -> Option<(u32, u32)> {
    if width == 0 || height == 0 || !scale_factor.is_finite() || scale_factor <= 0.0 {
        return None;
    }
    let convert = |value: u32| ((value as f64 / scale_factor).round() as u32).max(1);
    Some((convert(width), convert(height)))
}

pub fn apply(
    options: &mut LaunchOptions,
    settings: &EffectiveSettings,
    launcher_size: Option<(u32, u32)>,
) -> Result<(), String> {
    if !settings.java_path.trim().is_empty() {
        let java = PathBuf::from(&settings.java_path);
        if !java.is_absolute() || !java.is_file() {
            return Err("所选 Java 已不存在，请重新扫描或选择 Java 可执行文件".into());
        }
        options.java.path = Some(java);
    }
    options.memory.max = format!("{}M", settings.memory_mb);
    options.memory.min.clear();
    options.jvm_args = settings.jvm_args.clone();
    options.game_args = settings.game_args.clone();
    options.skip_bundle_check = settings.disable_file_check;
    options.screen.fullscreen = settings.window_mode == "fullscreen";
    let size = match settings.window_mode.as_str() {
        "custom" => Some((settings.width, settings.height)),
        "same-as-launcher" => launcher_size,
        _ => None,
    };
    if let Some((width, height)) = size {
        options.screen.width = Some(width);
        options.screen.height = Some(height);
    }
    if !settings.server_address.trim().is_empty() {
        let address = settings.server_address.trim();
        if address.contains(char::is_whitespace) || address.contains('/') {
            return Err("服务器地址请填写主机名或主机名:端口".into());
        }
        if supports_quick_play(&options.version) {
            options
                .game_args
                .extend(["--quickPlayMultiplayer".into(), address.into()]);
        } else {
            let (host, port) = address
                .rsplit_once(':')
                .filter(|(_, port)| port.parse::<u16>().is_ok())
                .unwrap_or((address, "25565"));
            options.game_args.extend([
                "--server".into(),
                host.into(),
                "--port".into(),
                port.into(),
            ]);
        }
    }
    Ok(())
}
pub fn configure_launcher(
    launcher: &mut Launcher,
    directory: PathBuf,
    settings: &EffectiveSettings,
    java_major: u32,
    windows_build: Option<u32>,
) -> Result<(), String> {
    let gc = gc_arguments(&settings.memory_management, java_major, windows_build)?;
    launcher.set_runtime_options(directory, settings.custom_info.clone(), gc);
    Ok(())
}

#[cfg(windows)]
pub fn windows_build() -> Option<u32> {
    #[repr(C)]
    struct OsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        service_pack: [u16; 128],
        suite_mask: u16,
        product_type: u8,
        reserved: u8,
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
    }
    let mut info = OsVersionInfo {
        size: std::mem::size_of::<OsVersionInfo>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform: 0,
        service_pack: [0; 128],
        suite_mask: 0,
        product_type: 0,
        reserved: 0,
    };
    // SAFETY: 结构大小与 RTL_OSVERSIONINFOW 前缀一致，调用期间指针有效。
    (unsafe { RtlGetVersion(&mut info) } >= 0 && info.build > 0).then_some(info.build)
}

#[cfg(not(windows))]
pub fn windows_build() -> Option<u32> {
    None
}

pub async fn before_launch(settings: &EffectiveSettings, directory: &Path) -> Result<(), String> {
    if settings.pre_launch_command.trim().is_empty() {
        return Ok(());
    }
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("cmd.exe");
        // 此字段本身是用户编写的 shell 程序，不是 argv；CRT 的反斜杠转义会破坏 cmd 内的引号。
        command
            .args(["/D", "/S", "/C"])
            .raw_arg(format!("\"{}\"", settings.pre_launch_command));
        tokio::process::Command::from(command)
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", &settings.pre_launch_command]);
        command
    };
    command
        .current_dir(directory)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(
        std::time::Duration::from_secs(PRE_LAUNCH_TIMEOUT_SECS),
        command.status(),
    )
    .await
    .map_err(|_| "启动前命令执行超时，请检查命令后重试")?
    .map_err(|e| format!("无法执行启动前命令：{e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "启动前命令执行失败（退出码 {:?}），请检查命令后重试",
            status.code()
        ))
    }
}

/// 真实游戏窗口出现后执行动作；`restore` 仅在游戏退出后调用。
pub fn launcher_visibility(app: &AppHandle, mode: &str, restore: bool) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if restore {
        if crate::launch_window::restore_on_exit(mode) {
            let _ = window.show();
            let _ = window.unminimize();
        } else if crate::launch_window::close_on_exit(mode) {
            let _ = window.close();
        }
    } else if crate::launch_window::hide_after_window(mode) {
        let _ = window.hide();
    } else if crate::launch_window::minimize_after_window(mode) {
        let _ = window.minimize();
    } else if crate::launch_window::close_after_window(mode) {
        let _ = window.close();
    }
}

/// 前端展示与真实启动共用同一自动内存算法，避免 UI 复制百分比规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPlanInfo {
    pub total_memory_mb: Option<u64>,
    pub available_memory_mb: Option<u64>,
    pub suggested_memory_mb: u64,
    pub mod_count: u64,
}

pub fn memory_plan_info(
    total_memory_mb: Option<u64>,
    available_memory_mb: Option<u64>,
    mod_count: u64,
    version_kind: VersionKind,
) -> MemoryPlanInfo {
    MemoryPlanInfo {
        total_memory_mb,
        available_memory_mb,
        suggested_memory_mb: memory_plan::automatic_memory_mb(
            available_memory_mb,
            total_memory_mb,
            mod_count,
            version_kind,
        ),
        mod_count,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    total_memory_mb: Option<u64>,
    available_memory_mb: Option<u64>,
    suggested_memory_mb: u64,
    capabilities: Value,
}

#[tauri::command]
pub fn game_runtime_info() -> RuntimeInfo {
    let (total, available) = physical_memory();
    let plan = memory_plan_info(total, available, 0, VersionKind::Regular);
    RuntimeInfo {
        total_memory_mb: total,
        available_memory_mb: available,
        suggested_memory_mb: plan.suggested_memory_mb,
        capabilities: serde_json::json!({
            "windowTitle": cfg!(windows), "processPriority": cfg!(windows),
            "offlineSkin": true,
            "launchWrapper": cfg!(windows), "lwjglAgent": true,
            "highPerformanceGpu": cfg!(windows)
        }),
    }
}
pub fn physical_memory_mb() -> (Option<u64>, Option<u64>) {
    physical_memory()
}

pub fn start_launch_log(directory: &Path) -> PathBuf {
    let store = LaunchLogStore::instance(directory);
    let _ = store.append_text("\n--- Nickel 启动会话 ---");
    store.log_path().to_path_buf()
}

/// 前端缓冲与落盘共用同一脱敏 / 截断结果，避免超长行放大内存占用。
pub fn bounded_log_line(line: &str) -> String {
    let sanitized = launch_log_store::redact_sensitive_text(line);
    if sanitized.chars().count() > LOG_LINE_MAX_CHARS {
        sanitized.chars().take(LOG_LINE_MAX_CHARS).collect()
    } else {
        sanitized
    }
}

/// Derive the instance from its log path; the store owns redaction and rotation.
pub fn append_launch_log(path: &Path, line: &str) -> Result<(), String> {
    let base = path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| format!("无法从日志路径解析实例目录：{}", path.display()))?;
    LaunchLogStore::instance(base).append_text(&bounded_log_line(line))
}

/// 读取实例内最近的有界游戏日志并分类；任一文件损坏都只跳过该来源。
pub fn analyze_launch_failure(
    exit_code: Option<i32>,
    process_output: &str,
    directory: &Path,
    minecraft_version: &str,
    loader: &str,
    optifine: Option<&str>,
    java_major: Option<u32>,
    java_vendor: Option<&str>,
) -> DiagnosticReport {
    let store = LaunchLogStore::instance(directory);
    let mut input = DiagnosticInput::new(exit_code, minecraft_version, loader)
        .with_process_output("", process_output);

    if let Ok(Some(log)) = store.read_latest_game_log(MAX_LATEST_LOG_BYTES) {
        input.input_truncated |= log.truncated;
        input = input.with_latest_log(log.text);
    }

    if let Ok(reports) = store.read_crash_reports(DIAGNOSTIC_CRASH_REPORTS, MAX_CRASH_REPORT_BYTES)
    {
        let mut combined = String::new();
        let mut crash_truncated = false;
        for report in reports {
            crash_truncated |= report.truncated;
            if !combined.is_empty() {
                combined.push_str("\n\n");
            }
            combined.push_str("===== ");
            combined.push_str(
                &report
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy())
                    .unwrap_or_else(|| "crash-report".into()),
            );
            combined.push_str(" =====\n");
            combined.push_str(&report.text);
        }
        if !combined.is_empty() {
            input.input_truncated |= crash_truncated;
            input = input.with_crash_report(combined);
        }
    }

    if let Some(version) = optifine.filter(|value| !value.trim().is_empty()) {
        input = input.with_optifine(version);
    }
    if let Some(major) = java_major.filter(|major| *major > 0) {
        input = input.with_java(major, java_vendor.unwrap_or("unknown"));
    }

    launch_diagnostics::analyze(&input)
}

#[tauri::command]
pub fn launch_log_open(app: AppHandle, dir: String, version_id: String) -> Result<(), String> {
    let directory = resource_directory(&app, Path::new(&dir), &version_id)?;
    let logs = LaunchLogStore::instance(&directory)
        .directory_to_open()
        .to_path_buf();
    std::fs::create_dir_all(&logs).map_err(|error| format!("创建日志目录失败：{error}"))?;
    tauri_plugin_opener::open_path(&logs, None::<&str>)
        .map_err(|error| format!("打开日志目录失败：{error}"))
}

#[cfg(windows)]
fn physical_memory_bytes() -> (Option<u64>, Option<u64>) {
    #[repr(C)]
    struct MemoryStatus {
        length: u32,
        load: u32,
        total: u64,
        available: u64,
        page_total: u64,
        page_available: u64,
        virtual_total: u64,
        virtual_available: u64,
        extended: u64,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(status: *mut MemoryStatus) -> i32;
    }
    let mut status = MemoryStatus {
        length: std::mem::size_of::<MemoryStatus>() as u32,
        load: 0,
        total: 0,
        available: 0,
        page_total: 0,
        page_available: 0,
        virtual_total: 0,
        virtual_available: 0,
        extended: 0,
    };
    // SAFETY: Windows 要求的结构大小与布局完整，指针只在同步调用中使用。
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return (None, None);
    }
    (Some(status.total), Some(status.available))
}

#[cfg(windows)]
fn physical_memory() -> (Option<u64>, Option<u64>) {
    let (total, available) = physical_memory_bytes();
    (
        total.map(|bytes| bytes / 1024 / 1024),
        available.map(|bytes| bytes / 1024 / 1024),
    )
}

#[cfg(not(windows))]
fn physical_memory() -> (Option<u64>, Option<u64>) {
    let Ok(raw) = std::fs::read_to_string("/proc/meminfo") else {
        return (None, None);
    };
    let field = |name: &str| {
        raw.lines()
            .find_map(|line| {
                line.strip_prefix(name)?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .map(|kb| kb / 1024)
    };
    (field("MemTotal:"), field("MemAvailable:"))
}

#[cfg(windows)]
pub fn set_process_priority(pid: u32, priority: &str) -> Result<(), String> {
    use std::ffi::c_void;
    const PROCESS_SET_INFORMATION: u32 = 0x0200;
    const BELOW_NORMAL: u32 = 0x4000;
    const NORMAL: u32 = 0x0020;
    const ABOVE_NORMAL: u32 = 0x8000;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn SetPriorityClass(process: *mut c_void, priority: u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    let class = match priority {
        "low" => BELOW_NORMAL,
        "high" => ABOVE_NORMAL,
        _ => NORMAL,
    };
    // SAFETY: 句柄仅授予设置优先级权限，使用后始终关闭。
    unsafe {
        let process = OpenProcess(PROCESS_SET_INFORMATION, 0, pid);
        if process.is_null() {
            return Err("无法调整游戏优先级，请使用普通优先级重试".into());
        }
        let success = SetPriorityClass(process, class) != 0;
        CloseHandle(process);
        if success {
            Ok(())
        } else {
            Err("设置游戏优先级失败，请使用普通优先级重试".into())
        }
    }
}
#[cfg(not(windows))]
pub fn set_process_priority(_pid: u32, priority: &str) -> Result<(), String> {
    if priority == "normal" {
        Ok(())
    } else {
        Err("当前系统不支持调整游戏优先级，请选择普通优先级".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_values_are_redacted_before_log_write() {
        let text = "Authorization: Bearer abc123 accessToken=def456 client_token: 'ghi789' password=\"secret\" --accessToken xyz";
        let redacted = launch_log_store::redact_sensitive_text(text);
        for secret in ["abc123", "def456", "ghi789", "secret\"", "xyz"] {
            assert!(!redacted.contains(secret), "{secret}: {redacted}");
        }
        assert!(redacted.contains("accessToken"));
        assert!(redacted.contains("***"));
    }

    #[test]
    fn launch_log_is_bounded_and_rotates() {
        let root =
            std::env::temp_dir().join(format!("nickel-log-{}", crate::store::random_token()));
        let logs = root.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join(launch_log_store::LOG_FILE_NAME);
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(launch_log_store::DEFAULT_MAX_LOG_BYTES)
            .unwrap();
        drop(file);
        append_launch_log(&path, "after rotation").unwrap();
        assert!(PathBuf::from(format!("{}.1", path.display())).is_file());
        assert!(!std::fs::read(&path).unwrap().is_empty());
        assert!(std::fs::metadata(&path).unwrap().len() < launch_log_store::DEFAULT_MAX_LOG_BYTES);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn launch_failure_analysis_reads_bounded_logs_and_java_context() {
        let root =
            std::env::temp_dir().join(format!("nickel-diag-{}", crate::store::random_token()));
        std::fs::create_dir_all(root.join("logs")).unwrap();
        let mut latest = vec![0xff, 0xfe, b'\n'];
        latest.extend_from_slice(b"java.lang.OutOfMemoryError: Java heap space\n");
        std::fs::write(root.join("logs").join("latest.log"), latest).unwrap();

        let report = analyze_launch_failure(
            Some(1),
            "",
            &root,
            "1.20.1",
            "forge",
            None,
            Some(17),
            Some("OpenJDK"),
        );
        assert_eq!(
            report.category,
            crate::launch_diagnostics::DiagnosticCategory::OutOfMemory
        );
        assert_eq!(report.context.exit_code, Some(1));
        assert_eq!(report.context.java_major, Some(17));
        assert_eq!(report.context.java_vendor.as_deref(), Some("OpenJDK"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_launch_report_propagates_stored_log_truncation() {
        let root = std::env::temp_dir().join(format!(
            "nickel-diag-truncated-{}",
            crate::store::random_token()
        ));
        std::fs::create_dir_all(root.join("logs")).unwrap();

        let mut latest = vec![b'a'; MAX_LATEST_LOG_BYTES + 1];
        latest.extend_from_slice(b"\njava.lang.OutOfMemoryError: Java heap space\n");
        std::fs::write(root.join("logs").join("latest.log"), latest).unwrap();
        let report = analyze_launch_failure(Some(1), "", &root, "1.20.1", "", None, None, None);
        assert!(report.input_truncated);

        std::fs::remove_file(root.join("logs").join("latest.log")).unwrap();
        std::fs::create_dir_all(root.join("crash-reports")).unwrap();
        let mut crash = vec![b'b'; MAX_CRASH_REPORT_BYTES + 1];
        crash.extend_from_slice(b"\njava.lang.OutOfMemoryError: Java heap space\n");
        std::fs::write(root.join("crash-reports").join("crash-large.txt"), crash).unwrap();
        let report = analyze_launch_failure(Some(1), "", &root, "1.20.1", "", None, None, None);
        assert!(report.input_truncated);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn broken_log_sources_fall_back_without_hiding_process_output() {
        let root =
            std::env::temp_dir().join(format!("nickel-diag-bad-{}", crate::store::random_token()));
        std::fs::create_dir_all(root.join("logs")).unwrap();
        // latest.log 是目录、crash-reports 是普通文件：两个来源都应被安全跳过。
        std::fs::create_dir_all(root.join("logs").join("latest.log")).unwrap();
        std::fs::write(root.join("crash-reports"), b"not a directory").unwrap();

        let report = analyze_launch_failure(
            Some(1),
            "Unrecognized option: -XX:+UseNopeGC",
            &root,
            "1.20.1",
            "",
            None,
            None,
            None,
        );
        assert_eq!(
            report.category,
            crate::launch_diagnostics::DiagnosticCategory::JvmArgumentError
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parser_preserves_quoted_windows_paths() {
        assert_eq!(
            split_arguments(r#"-Dpath="C:\Program Files\Java" --name 'hello world'"#).unwrap(),
            vec![r#"-Dpath=C:\Program Files\Java"#, "--name", "hello world"]
        );
        assert!(split_arguments("\"unfinished").is_err());
    }
    #[test]
    fn gc_mode_matrix_honors_java_and_windows_build() {
        let args = |mode: &str, java: u32, build: Option<u32>| {
            gc_arguments(mode, java, build).unwrap().unwrap_or_default()
        };
        assert!(args("auto-zgc", 14, Some(17_763)).contains(&"-XX:+UseG1GC".into()));
        assert!(args("auto-zgc", 15, Some(17_762)).contains(&"-XX:+UseG1GC".into()));
        assert!(args("auto-zgc", 15, Some(17_763)).contains(&"-XX:+UseZGC".into()));
        assert!(args("generational-zgc", 20, Some(17_763)).contains(&"-XX:+UseG1GC".into()));
        assert!(args("generational-zgc", 21, Some(17_763)).contains(&"-XX:+ZGenerational".into()));
        assert!(args("generational-zgc", 22, Some(17_763)).contains(&"-XX:+ZGenerational".into()));
        assert!(!args("generational-zgc", 23, Some(17_763)).contains(&"-XX:+ZGenerational".into()));
        assert!(args("auto-zgc", 22, Some(17_763)).contains(&"-XX:+ZGenerational".into()));
        assert!(args("auto-zgc", 25, Some(17_763)).contains(&"-XX:+UseZGC".into()));
        assert!(!args("auto-zgc", 25, Some(17_763)).contains(&"-XX:+ZGenerational".into()));
        assert!(args("g1", 21, Some(17_763)).contains(&"-XX:+UseG1GC".into()));
        assert!(args("g1-tuned", 8, Some(17_763)).contains(&"-XX:+ParallelRefProcEnabled".into()));
        assert!(args("g1-tuned", 21, Some(17_763)).contains(&"-XX:MaxHeapFreeRatio=40".into()));
        assert!(args("g1", 24, Some(17_763)).contains(&"-XX:+UseCompactObjectHeaders".into()));
        assert!(
            args("g1", 23, Some(17_763))
                .iter()
                .all(|arg| arg != "-XX:+UseCompactObjectHeaders")
        );
        assert!(gc_arguments("custom", 21, Some(17_763)).unwrap().is_none());
    }

    #[test]
    fn auto_gc_fallbacks_use_standard_g1_only() {
        let args = |mode: &str, java: u32, build: Option<u32>| {
            gc_arguments(mode, java, build).unwrap().unwrap_or_default()
        };
        for (mode, java, build) in [
            ("auto-zgc", 8, Some(17_763)),
            ("auto-zgc", 14, Some(17_763)),
            ("generational-zgc", 20, Some(17_763)),
            ("auto-zgc", 21, Some(17_762)),
            ("generational-zgc", 21, Some(17_762)),
        ] {
            let arguments = args(mode, java, build);
            assert!(
                arguments.contains(&"-XX:+UseG1GC".into()),
                "{mode}/Java {java}/{build:?}"
            );
            for tuned in [
                "-XX:+PerfDisableSharedMem",
                "-XX:+ParallelRefProcEnabled",
                "-XX:MinHeapFreeRatio=25",
                "-XX:MaxHeapFreeRatio=40",
            ] {
                assert!(
                    !arguments.contains(&tuned.into()),
                    "自动 ZGC 的 G1 回退不得带调优参数 {tuned}：{mode}/Java {java}/{build:?}"
                );
            }
        }

        let auto20 = args("auto-zgc", 20, Some(17_763));
        assert!(auto20.contains(&"-XX:+UseZGC".into()));
        assert!(!auto20.contains(&"-XX:+UseG1GC".into()));

        let tuned = args("g1-tuned", 8, Some(17_763));
        assert!(tuned.contains(&"-XX:+PerfDisableSharedMem".into()));
        assert!(tuned.contains(&"-XX:+ParallelRefProcEnabled".into()));
        let tuned_modern = args("g1-tuned", 21, Some(17_763));
        assert!(tuned_modern.contains(&"-XX:MinHeapFreeRatio=25".into()));
        assert!(tuned_modern.contains(&"-XX:MaxHeapFreeRatio=40".into()));

        // 回归锁定：只有 g1-tuned 拥有调优项，自动 ZGC 的 G1 回退与标准 G1 完全一致。
        assert_eq!(
            args("auto-zgc", 8, Some(17_763)),
            args("g1", 8, Some(17_763))
        );
        assert_eq!(
            args("generational-zgc", 20, Some(17_763)),
            args("g1", 20, Some(17_763))
        );
    }

    #[test]
    fn memory_plan_info_uses_shared_automatic_algorithm() {
        let plan = memory_plan_info(Some(32_768), Some(16_384), 200, VersionKind::Moddable);
        assert_eq!(plan.suggested_memory_mb, 10_138);
        assert_eq!(plan.mod_count, 200);
        let json = serde_json::to_value(plan).unwrap();
        assert_eq!(json["suggestedMemoryMb"], 10_138);
        assert_eq!(json["modCount"], 200);
    }

    #[test]
    fn launcher_size_is_converted_from_physical_to_logical_pixels() {
        assert_eq!(
            logical_window_size(3840, 2160, 2.0),
            Some((1920, 1080))
        );
        assert_eq!(
            logical_window_size(3840, 2160, 1.5),
            Some((2560, 1440))
        );
        assert_eq!(logical_window_size(0, 2160, 2.0), None);
        assert_eq!(logical_window_size(3840, 2160, 0.0), None);
        assert_eq!(logical_window_size(3840, 2160, f64::NAN), None);
    }

    #[test]
    fn memory_management_validates_current_modes() {
        assert_eq!(normalize_memory_management("").unwrap(), "g1-tuned");
        assert_eq!(normalize_memory_management("g1").unwrap(), "g1");
        assert_eq!(normalize_memory_management("auto-zgc").unwrap(), "auto-zgc");
        assert!(normalize_memory_management("unknown").is_err());
    }

    #[test]
    fn local_values_override_and_global_fields_inherit() {
        let global = serde_json::json!({"javaPath":"global-java","memoryMode":"custom","memoryMb":6144,"jvmArgs":"-Dglobal=1","windowMode":"fullscreen"});
        let local = serde_json::json!({"memoryMode":"global","javaPath":null,"gameArgs":"--demo","versionIsolation":false});
        let result = resolve_with_memory(
            &global,
            &local,
            Some(16_384),
            Some(12_000),
            0,
            VersionKind::Regular,
        )
        .unwrap();
        assert_eq!(result.memory_mb, 6144);
        assert_eq!(result.java_path, "global-java");
        assert_eq!(result.jvm_args, vec!["-Dglobal=1"]);
        assert!(!result.isolation);
        assert_eq!(result.game_args, vec!["--demo"]);
    }

    #[test]
    fn empty_local_strings_clear_global_launch_values() {
        let global = serde_json::json!({
            "javaPath": "global-java", "windowTitle": "global-title",
            "jvmArgs": "-Dglobal=1", "gameArgs": "--demo",
            "preLaunchCommand": "echo ready", "serverAddress": "server.example",
            "versionIsolation": true
        });
        let local = serde_json::json!({
            "javaPath": "", "windowTitle": "", "jvmArgs": "", "gameArgs": "",
            "preLaunchCommand": "", "serverAddress": "", "versionIsolation": false
        });
        let result = resolve_with_memory(
            &global,
            &local,
            Some(16_384),
            Some(12_000),
            0,
            VersionKind::Regular,
        )
        .unwrap();
        assert!(result.java_path.is_empty());
        assert!(result.window_title.is_empty());
        assert!(result.jvm_args.is_empty());
        assert!(result.game_args.is_empty());
        assert!(result.pre_launch_command.is_empty());
        assert!(result.server_address.is_empty());
        assert!(!result.isolation);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn pre_launch_preserves_cmd_quotes_and_working_directory() {
        let root = std::env::temp_dir().join(format!(
            "nickel prelaunch {}",
            crate::store::random_token()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let output = root.join("quoted output.txt");
        let script = root.join("pre launch.cmd");
        std::fs::write(&script, b"@echo off\r\necho %~1\r\ncd\r\n").unwrap();
        let mut settings = resolve_with_memory(
            &serde_json::json!({}),
            &serde_json::json!({}),
            None,
            None,
            0,
            VersionKind::Regular,
        )
        .unwrap();
        for command in [
            format!("(echo accepted& cd)>\"{}\"", output.display()),
            format!(
                "\"{}\" \"accepted argument\" >\"{}\"",
                script.display(),
                output.display()
            ),
        ] {
            settings.pre_launch_command = command;
            before_launch(&settings, &root).await.unwrap();
            let text = std::fs::read_to_string(&output).unwrap();
            assert!(text.contains("accepted"));
            assert!(
                text.lines()
                    .any(|line| line.eq_ignore_ascii_case(&root.to_string_lossy()))
            );
            std::fs::remove_file(&output).unwrap();
        }
        settings.pre_launch_command = "exit /b 7".into();
        assert!(
            before_launch(&settings, &root)
                .await
                .unwrap_err()
                .contains('7')
        );
        settings.pre_launch_command.clear();
        before_launch(&settings, &root).await.unwrap();
        assert!(!output.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn cleanup_temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nickel-runtime-cleanup-{tag}-{}",
            crate::store::random_token()
        ))
    }

    /// 祖先链接是 macOS /var、OneDrive junction 这类合法重定向，不能拒绝。
    #[cfg(unix)]
    #[test]
    fn cleanup_accepts_symlinked_ancestor() {
        use std::os::unix::fs::symlink;

        let base = cleanup_temp_root("ancestor");
        let real = base.join("real/instances/game");
        std::fs::create_dir_all(real.join("versions/1.20.1")).unwrap();
        std::fs::create_dir_all(base.join("real/instances")).unwrap();
        let linked = base.join("linked");
        symlink(base.join("real/instances"), &linked).unwrap();

        // 候选根经符号链接祖先定位、自身是普通目录：必须放行。
        let resolved = cleanup_game_directory(&linked.join("game"), "1.20.1", true).unwrap();
        assert_eq!(resolved, linked.join("game/versions/1.20.1"));

        std::fs::remove_file(&linked).unwrap();
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_rejects_symlinked_version_directory() {
        use std::os::unix::fs::symlink;

        let base = cleanup_temp_root("version-link");
        let game = base.join("game");
        let outside = base.join("outside");
        std::fs::create_dir_all(game.join("versions")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, game.join("versions/1.20.1")).unwrap();

        assert!(cleanup_game_directory(&game, "1.20.1", true).is_err());

        std::fs::remove_file(game.join("versions/1.20.1")).unwrap();
        std::fs::remove_dir_all(base).unwrap();
    }

    /// `versions/` 本身是链接时，版本目录自身不是链接，只有 canonicalize 能发现逃逸。
    #[cfg(unix)]
    #[test]
    fn cleanup_rejects_versions_directory_escaping_root() {
        use std::os::unix::fs::symlink;

        let base = cleanup_temp_root("escape");
        let game = base.join("game");
        let outside_versions = base.join("outside/versions");
        std::fs::create_dir_all(outside_versions.join("1.20.1")).unwrap();
        std::fs::create_dir_all(&game).unwrap();
        symlink(&outside_versions, game.join("versions")).unwrap();

        let error = cleanup_game_directory(&game, "1.20.1", true).unwrap_err();
        assert!(error.contains("不在实例目录内"), "{error}");

        std::fs::remove_file(game.join("versions")).unwrap();
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn cleanup_rejects_relative_root_and_missing_version() {
        assert!(cleanup_game_directory(Path::new("relative/game"), "1.20.1", true).is_err());
        let base = cleanup_temp_root("missing");
        std::fs::create_dir_all(&base).unwrap();
        assert!(cleanup_game_directory(&base, "1.20.1", true).is_err());
        std::fs::remove_dir_all(base).unwrap();
    }
}
