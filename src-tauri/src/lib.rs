//! Nickel 后端入口：统一注册 IPC 命令并编排窗口、配置与启动生命周期。
//! 业务模块共享配置根、任务取消上下文及脱敏事件边界。
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use minecraft_java_rs_core::{
    launcher::{
        Launcher,
        events::LaunchEvent,
        options::{JavaOptions, LaunchOptions, LoaderConfig, MemoryConfig, ScreenConfig},
    },
    models::minecraft::Authenticator,
    net::transfer::CancellationToken,
    utils::auth::offline_uuid,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Window};

// ── 真实认证 / 账号后端模块 ──
mod accounts; // 账号存储（accounts.json）
mod app_paths;
mod app_tasks;
mod build_channel; // 构建通道（development / production）
mod community_dependencies;
mod community_network;
mod community_save_as; // 社区资源另存为（先对话框后下载，目标由用户选择）
mod curseforge; // CurseForge 社区资源检索 / 安装（第二引擎，x-api-key 鉴权）
mod dwm; // Windows DWM 外观微调（去掉系统画的 1px 窗口边框）
mod error_report;
mod game; // 游戏 / 版本设置（game.json）
mod game_junk; // 清理游戏与启动器日志、崩溃报告等可再生成文件
mod gpu_preferences;
mod http; // 共享 HTTP 辅助（复用核心库 reqwest client）
mod install; // 版本下载 / 加载器安装 / 安装事件流 / Java 探测
mod installer_download; // 原生对话框另存为独立官方安装器
mod instances; // 实例注册表（instances.json）/ 版本与本地资源扫描
// 纯逻辑诊断 / 日志 API 同时供单元测试与后续导出使用；生产已消费分类与读取主入口。
#[allow(dead_code)]
mod launch_diagnostics;
#[cfg(test)]
mod launch_diagnostics_tests;
#[allow(dead_code)]
mod launch_log_store;
mod launch_output;
mod launch_state;
mod launch_window;
mod launcher_portable;
mod loader_availability;
mod loader_catalog; // 完整加载器清单 / 请求级下载源 / 官方安装器地址
mod media; // Validated background image and video protocols.
mod memory_plan;
mod modpacks;
mod modrinth; // Modrinth 社区资源检索 / 安装（不经镜像，直连公开 API）
mod msa; // 微软正版登录（Device Code Flow）
mod named_profiles; // 实例真实存储名称 / 启动元数据
mod network_settings;
mod offline_skin;
mod onboarding;
mod pack_export;
mod pack_import;
mod portable_bootstrap;
#[cfg(all(test, windows))]
mod portable_live_tests;
mod portable_pack;
mod resources;
mod runtime_agents;
#[cfg(test)]
mod runtime_integration_tests;
mod runtime_settings;
mod secrets; // 本地密钥读取（Microsoft 登录配置）
mod skin; // 皮肤获取 / 上传
mod store; // 本地持久化 + 纯工具
mod vault; // 本机凭据静态加密（皮肤站密码）
mod version_settings; // per-version 设置 / 版本改名 / 删除 / 补全文件
mod yggdrasil; // Yggdrasil（LittleSkin）认证

/// Mojang 官方版本清单（version_manifest_v2）。
/// 镜像模式下会由 `mirror::rewrite_url` 自动重写为 BMCLAPI 地址。
const VERSION_MANIFEST_URL: &str =
    "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";

/// 设置文件大小上限（256KB），防止前端写坏超大内容。
const SETTINGS_MAX_BYTES: usize = 256 * 1024;

/// 发送给前端的启动事件。
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FrontendEvent {
    Preparing {
        version_id: String,
        game_dir: String,
    },
    Progress {
        downloaded: u64,
        total: u64,
        file_type: String,
    },
    Speed {
        bps: f64,
    },
    Stage {
        message: String,
    },
    Log {
        line: String,
    },
    Close {
        code: i32,
    },
    Started,
    Cancelled,
    Warning {
        message: String,
    },
    Error {
        message: String,
    },
    Diagnostic {
        category: String,
        title: String,
        summary: String,
        message: String,
    },
}

impl From<&LaunchEvent> for FrontendEvent {
    fn from(ev: &LaunchEvent) -> Self {
        match ev {
            LaunchEvent::Progress {
                downloaded,
                total,
                kind,
            } => FrontendEvent::Progress {
                downloaded: *downloaded,
                total: *total,
                file_type: kind.clone(),
            },
            LaunchEvent::Speed(bps) => FrontendEvent::Speed { bps: *bps },
            LaunchEvent::Estimated(secs) => FrontendEvent::Stage {
                message: format!("预计剩余 {secs:.0}s"),
            },
            LaunchEvent::Check {
                current,
                total,
                kind,
            } => FrontendEvent::Progress {
                downloaded: *current as u64,
                total: *total as u64,
                file_type: format!("校验:{kind}"),
            },
            LaunchEvent::Extract(name) => FrontendEvent::Stage {
                message: format!("解压 {name}"),
            },
            LaunchEvent::Patch(name) => FrontendEvent::Stage {
                message: format!("加载器处理 {name}"),
            },
            LaunchEvent::GameDownloadFinished => FrontendEvent::Stage {
                message: "游戏文件准备完成".into(),
            },
            LaunchEvent::Data(line) => FrontendEvent::Log { line: line.clone() },
            LaunchEvent::ProcessOutput { .. } | LaunchEvent::OutputClosed { .. } => {
                FrontendEvent::Error {
                    message: "进程输出必须经过会话日志处理".into(),
                }
            }
            LaunchEvent::Close(code) => FrontendEvent::Close { code: *code },
            LaunchEvent::Error(message) => FrontendEvent::Error {
                message: message.clone(),
            },
        }
    }
}

/// 动态文本共用脱敏与长度边界。
fn redact_frontend_event(event: &mut FrontendEvent) {
    let redact = |value: &mut String| {
        *value = runtime_settings::bounded_log_line(value);
    };
    match event {
        FrontendEvent::Progress { file_type, .. } => redact(file_type),
        FrontendEvent::Stage { message } => redact(message),
        FrontendEvent::Log { line } => redact(line),
        FrontendEvent::Warning { message } | FrontendEvent::Error { message } => redact(message),
        FrontendEvent::Diagnostic {
            category,
            title,
            summary,
            message,
        } => {
            redact(category);
            redact(title);
            redact(summary);
            redact(message);
        }
        FrontendEvent::Preparing { .. }
        | FrontendEvent::Speed { .. }
        | FrontendEvent::Close { .. }
        | FrontendEvent::Started
        | FrontendEvent::Cancelled => {}
    }
}

/// 版本条目（精简，供前端下拉列表使用）
#[derive(Debug, Clone, Serialize)]
pub struct VersionInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub version_type: String,
    pub url: String,
}

/// 计算默认的 .minecraft 数据目录（应用数据目录下）
fn default_game_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app_paths::default_game_dir(app)
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. 无边框窗口控制（前端自绘标题栏调用；仅最小化 + 关闭，不做最大化）
// ─────────────────────────────────────────────────────────────────────────────

/// 最小化当前窗口
#[tauri::command]
fn window_minimize(window: Window) -> Result<(), String> {
    window
        .minimize()
        .map_err(|e| format!("最小化窗口失败: {e}"))
}

/// 关闭当前窗口
#[tauri::command]
fn window_close(window: Window) -> Result<(), String> {
    window.close().map_err(|e| format!("关闭窗口失败: {e}"))
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. 设置持久化（应用配置目录下 settings.json）
//    —— 后端对设置结构保持无感知：纯 JSON 文本透传，仅校验合法性 + 大小上限。
// ─────────────────────────────────────────────────────────────────────────────

/// settings.json 的完整路径（自动创建父目录）
fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_paths::config_dir(app)?.join("settings.json"))
}

/// 缺失或损坏配置回退空对象；便携外观只从经过校验的 manifest 布局读取。
#[tauri::command]
fn settings_get(app: AppHandle) -> Result<String, String> {
    let path = settings_path(&app)?;
    let portable = app_paths::portable_root(&app)?;
    if let Some((root, manifest)) = &portable {
        portable_bootstrap::seed_personalization(root, manifest)?;
    }
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let mut value = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            if text.len() > SETTINGS_MAX_BYTES || !value.is_object() {
                return Ok("{}".into());
            }
            if let Some((root, manifest)) = portable {
                portable_bootstrap::background_for_display(&root, &manifest, &mut value)?;
                return serde_json::to_string(&value).map_err(|e| e.to_string());
            }
            Ok(text)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("{}".into()),
        Err(error) => Err(format!("读取设置失败：{error}")),
    }
}

/// 设置保留未知字段；对象及大小校验先于原子写入，网络预算只在写入成功后应用。
#[tauri::command]
fn settings_set(app: AppHandle, json: String) -> Result<(), String> {
    if json.len() > SETTINGS_MAX_BYTES {
        return Err("设置内容过大，请减少设置内容后重试".into());
    }
    let mut value: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| format!("非法 JSON: {e}"))?;
    if !value.is_object() {
        return Err("设置必须是 JSON 对象".into());
    }
    let network = network_settings::from_value(&value)?;
    // 便携背景在写入边界恢复相对路径，防止首屏自动保存固化当前解压位置。
    let text = if let Some((root, manifest)) = app_paths::portable_root(&app)? {
        portable_bootstrap::background_for_storage(&root, &manifest, &mut value)?;
        serde_json::to_string(&value).map_err(|e| e.to_string())?
    } else {
        json
    };
    named_profiles::atomic_write(&settings_path(&app)?, text.as_bytes())?;
    network_settings::apply(network);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. 下载源切换
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// 4. 版本列表 / 默认目录
// ─────────────────────────────────────────────────────────────────────────────

/// 拉取版本清单并返回版本列表（受当前下载源影响）。
#[tauri::command]
async fn list_versions() -> Result<Vec<VersionInfo>, String> {
    let manifest = install::list_remote_versions(None).await?;
    let manifest: install::RemoteVersions =
        serde_json::from_str(&manifest).map_err(|e| e.to_string())?;
    Ok(manifest
        .versions
        .into_iter()
        .map(|version| VersionInfo {
            id: version.id,
            version_type: version.version_type,
            url: version.url,
        })
        .collect())
}

/// 返回默认游戏目录（供前端展示/使用）
#[tauri::command]
fn default_minecraft_dir(app: AppHandle) -> Result<String, String> {
    Ok(default_game_dir(&app)?.to_string_lossy().to_string())
}

/// 返回指定实例实际会使用的自动内存规划；未选择实例时按普通版本的空 Mod 目录估算。
#[tauri::command]
fn resolved_memory_plan(
    app: AppHandle,
    game_dir: Option<String>,
    version_id: Option<String>,
) -> Result<runtime_settings::MemoryPlanInfo, String> {
    let (total_memory, available_memory) = runtime_settings::physical_memory_mb();
    let (Some(root), Some(version)) = (
        game_dir.filter(|value| !value.trim().is_empty()),
        version_id.filter(|value| !value.trim().is_empty()),
    ) else {
        return Ok(runtime_settings::memory_plan_info(
            total_memory,
            available_memory,
            0,
            memory_plan::VersionKind::Regular,
        ));
    };

    let base = PathBuf::from(root);
    if !base.is_absolute() {
        return Err("实例目录必须使用绝对路径".into());
    }
    named_profiles::validate_name(&version)?;
    instances::reject_links(&base)?;
    let global: serde_json::Value =
        serde_json::from_str(&game::game_settings_get(app)?).unwrap_or_default();
    let local = runtime_settings::read_local(&base, &version);
    resolved_memory_plan_for(
        &base,
        &version,
        &global,
        &local,
        total_memory,
        available_memory,
    )
}

fn resolved_memory_plan_for(
    base: &Path,
    version: &str,
    global: &serde_json::Value,
    local: &serde_json::Value,
    total_memory: Option<u64>,
    available_memory: Option<u64>,
) -> Result<runtime_settings::MemoryPlanInfo, String> {
    named_profiles::validate_name(version)?;
    let isolation = runtime_settings::resolve_isolation(global, local);
    let directory = runtime_settings::game_directory(base, version, isolation);
    instances::reject_links(&directory)?;

    let context = launch_version_context(base, version);
    let metadata_hint = memory_plan::version_metadata_hint(base, version);
    let version_kind =
        memory_plan::classify_version(version, context.loader.as_deref(), &metadata_hint);
    let mod_count = memory_plan::count_mods_for_instance(base, version, &directory)?;
    Ok(runtime_settings::memory_plan_info(
        total_memory,
        available_memory,
        mod_count,
        version_kind,
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. 启动游戏
// ─────────────────────────────────────────────────────────────────────────────

/// 解析后的启动参数
struct ResolvedLaunch {
    version: String,
    username: String,
    memory_max: String,
    game_dir: Option<String>,
}

/// 取消令牌只覆盖游戏进程生成前的准备阶段，运行中的游戏另有生命周期。
struct LaunchControl {
    token: CancellationToken,
    launch_id: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScopedLaunchEvent<'a> {
    launch_id: &'a str,
    #[serde(flatten)]
    event: FrontendEvent,
}

fn emit_launch_event(app: &AppHandle, launch_id: &str, mut event: FrontendEvent) {
    redact_frontend_event(&mut event);
    emit_safe_launch_event(app, launch_id, event);
}

fn emit_safe_launch_event(app: &AppHandle, launch_id: &str, event: FrontendEvent) {
    let _ = app.emit("launcher-event", ScopedLaunchEvent { launch_id, event });
}

fn validate_launch_id(value: String) -> Result<String, String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("启动请求标识无效，请重新打开启动器后重试".into());
    }
    Ok(value)
}

fn cancel_launch_control(control: &LaunchControl, launch_id: &str) -> bool {
    if launch_id != control.launch_id {
        return false;
    }
    control.token.cancel();
    true
}

static LAUNCH_CONTROL: LazyLock<Mutex<Option<Arc<LaunchControl>>>> =
    LazyLock::new(|| Mutex::new(None));

fn launch_control_lock() -> std::sync::MutexGuard<'static, Option<Arc<LaunchControl>>> {
    LAUNCH_CONTROL.lock().unwrap_or_else(|e| e.into_inner())
}

/// 取消准备任务；旧请求不得取消新一轮启动。
#[tauri::command]
fn launch_cancel(launch_id: String) {
    if let Some(control) = launch_control_lock().clone() {
        cancel_launch_control(&control, &launch_id);
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn launch_game(
    app: AppHandle,
    version_id: Option<String>,
    username: Option<String>,
    memory_mb: Option<u64>,
    source: Option<String>,
    game_dir: Option<String>,
    launch_id: String,
) -> Result<(), String> {
    onboarding::require_accepted(&app)?;
    let launch_id = validate_launch_id(launch_id)?;
    let version = version_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or("缺少版本号（versionId）")?;
    let username = username.unwrap_or_default();
    let memory_max = memory_mb.map(|value| format!("{value}M")).unwrap_or_else(|| "2G".into());
    let transfer = network_settings::snapshot().transfer(source.as_deref())?;
    let control = Arc::new(LaunchControl {
        token: transfer.cancellation().clone(),
        launch_id: launch_id.clone(),
    });
    {
        let mut current = launch_control_lock();
        if current.is_some() {
            return Err("另一次启动尚未结束，请稍候再试".into());
        }
        *current = Some(control.clone());
    }
    let resolved = ResolvedLaunch {
        version: version.clone(),
        username,
        memory_max,
        game_dir: game_dir.clone(),
    };
    if let Some(directory) = game_dir.as_ref() {
        emit_launch_event(
            &app,
            &launch_id,
            FrontendEvent::Preparing {
                version_id: version.clone(),
                game_dir: directory.clone(),
            },
        );
    }
    let result = transfer
        .scope(run_launch_scoped(app.clone(), resolved, launch_id.clone()))
        .await;
    {
        let mut current = launch_control_lock();
        if current
            .as_ref()
            .is_some_and(|registered| Arc::ptr_eq(registered, &control))
        {
            *current = None;
        }
    }
    match result {
        Ok(()) => Ok(()),
        Err(_) if control.token.is_cancelled() => {
            emit_launch_event(&app, &launch_id, FrontendEvent::Cancelled);
            Ok(())
        }
        Err(error) => {
            error_report::record_launch_failure(&app, game_dir.as_deref(), &version, &error);
            Err(runtime_settings::bounded_log_line(&error))
        }
    }
}

fn emit_launch_warning(app: &AppHandle, launch_id: &str, log_path: &Path, message: String) {
    let message = runtime_settings::bounded_log_line(&message);
    let _ = runtime_settings::append_launch_log(log_path, &format!("[警告] {message}"));
    emit_launch_event(app, launch_id, FrontendEvent::Warning { message });
}

#[derive(Debug, Clone, Default)]
struct LaunchVersionContext {
    minecraft_version: String,
    loader: Option<String>,
    loader_version: Option<String>,
    release_time: Option<String>,
}

/// 版本设置页与启动编排共用 `version_detail` 的真实元数据；读取失败时回退命名实例档案。
fn launch_version_context(base: &Path, version: &str) -> LaunchVersionContext {
    let detail =
        version_settings::version_detail(base.to_string_lossy().into_owned(), version.to_string())
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let profile = named_profiles::load(base, version).ok().flatten();
    let detail_text = |key: &str| {
        detail
            .as_ref()
            .and_then(|value| value.get(key))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let profile_loader = profile
        .as_ref()
        .and_then(|value| value.loader.as_ref())
        .map(|loader| format!("{loader:?}").to_ascii_lowercase());
    let loader = detail_text("loader")
        .filter(|value| !value.eq_ignore_ascii_case("unknown"))
        .or(profile_loader);
    let loader_version = detail_text("loaderVersion").or_else(|| {
        profile
            .as_ref()
            .map(|value| value.loader_version.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    });
    let minecraft_version = detail_text("minecraftVersion")
        .or_else(|| {
            profile
                .as_ref()
                .map(|value| value.minecraft_version.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| version.to_string());
    LaunchVersionContext {
        minecraft_version,
        loader,
        loader_version,
        release_time: detail_text("releaseTime"),
    }
}

/// 启动准备阶段的取消检查。
///
/// `run_launch_scoped` 在 `transfer.scope(...)` 内执行，任务局部上下文里就是本次启动的取消令牌。
/// 若将来从其它入口调用（不在 scope 内），`current()` 会退化为一个新令牌，不会误报取消。
fn ensure_launch_not_cancelled() -> Result<(), String> {
    let context = minecraft_java_rs_core::net::transfer::TransferContext::current();
    if context.cancellation().is_cancelled() {
        Err("启动已取消".into())
    } else {
        Ok(())
    }
}

/// 下载（如需）并启动游戏，进度与日志通过 launcher-event 推送。
async fn run_launch_scoped(
    app: AppHandle,
    p: ResolvedLaunch,
    launch_id: String,
) -> Result<(), String> {
    named_profiles::validate_name(&p.version)?;
    let base = match p.game_dir.as_ref() {
        Some(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
        _ => default_game_dir(&app)?,
    };
    if !base.is_absolute() {
        return Err("请选择绝对路径的游戏目录".into());
    }
    instances::reject_links(&base)?;
    std::fs::create_dir_all(&base).map_err(|e| format!("创建游戏目录失败：{e}"))?;
    let base = std::fs::canonicalize(base).map_err(|e| format!("解析游戏目录失败：{e}"))?;
    let running = app_tasks::with_idle_directory(&base, || launch_state::begin(&base))?;
    let global: serde_json::Value =
        serde_json::from_str(&game::game_settings_get(app.clone())?).unwrap_or_default();
    let local = runtime_settings::read_local(&base, &p.version);
    let isolation = runtime_settings::resolve_isolation(&global, &local);
    let directory = runtime_settings::game_directory(&base, &p.version, isolation);
    instances::reject_links(&directory)?;
    std::fs::create_dir_all(&directory).map_err(|e| format!("创建游戏工作目录失败：{e}"))?;
    error_report::begin_session(&directory)?;
    let log_path = runtime_settings::start_launch_log(&directory);
    let (total_memory, available_memory) = runtime_settings::physical_memory_mb();
    let mod_count = memory_plan::count_mods_for_instance(&base, &p.version, &directory)?;
    let version_context = launch_version_context(&base, &p.version);
    let loader_hint = version_context.loader.clone();
    let metadata_hint = memory_plan::version_metadata_hint(&base, &p.version);
    let version_kind =
        memory_plan::classify_version(&p.version, loader_hint.as_deref(), &metadata_hint);
    let settings = runtime_settings::resolve_with_memory(
        &global,
        &local,
        total_memory,
        available_memory,
        mod_count,
        version_kind,
    )?;
    if settings.isolation != isolation {
        return Err("实例隔离设置解析不一致，请重新保存实例设置后重试".into());
    }
    let launcher_size = app.get_webview_window("main").and_then(|window| {
        let size = window.inner_size().ok()?;
        let scale_factor = window.scale_factor().ok()?;
        runtime_settings::logical_window_size(size.width, size.height, scale_factor)
    });

    let mut options = build_options(&p, base.clone());
    named_profiles::apply_for_launch(&mut options, &base, &p.version)?;
    runtime_settings::apply(&mut options, &settings, launcher_size)?;
    let java_selection = install::java_resolve(
        app.clone(),
        version_context.minecraft_version.clone(),
        version_context.loader.clone(),
        version_context.loader_version.clone(),
        version_context.release_time.clone(),
        Some(settings.java_path.clone()).filter(|value| !value.trim().is_empty()),
    )
    .await?;
    if java_selection.needs_download {
        // 最低主版本只参与缓存校验；下载组件仍由版本 JSON 的 javaVersion.component 决定。
        options.java.path = None;
        options.java.version = None;
        options.java.required_major = Some(java_selection.requirement.min_major);
    } else {
        options.java.required_major = None;
        if let Some(path) = java_selection.path.as_deref() {
            // 预览与实际启动共用同一解析结果，避免列表首项和启动 Java 分叉。
            options.java.path = Some(PathBuf::from(path));
        }
    }
    let java_stage = if java_selection.needs_download {
        format!(
            "未找到兼容 Java，将下载 Java {}：{}",
            java_selection.requirement.preferred_major, java_selection.reason
        )
    } else {
        format!(
            "将使用 Java {} · {}（{}）",
            java_selection.version,
            java_selection.vendor,
            if java_selection.source == "manual" {
                "手动选择"
            } else {
                "自动选择"
            }
        )
    };
    let _ = runtime_settings::append_launch_log(&log_path, &format!("[启动] {java_stage}"));
    emit_launch_event(
        &app,
        &launch_id,
        FrontendEvent::Stage {
            message: java_stage,
        },
    );
    let auth_server = apply_active_account(&app, &mut options, &settings)?;
    let mut launcher = Launcher::new(options);
    let (tx, rx) = tokio::sync::mpsc::channel::<LaunchEvent>(1024);
    let captured = Arc::new(Mutex::new(launch_output::OutputCapture::default()));
    let emit_app = app.clone();
    let emit_launch_id = launch_id.clone();
    let output_pump = launch_output::OutputPump::start(
        launch_log_store::LaunchLogStore::instance(&directory),
        rx,
        captured.clone(),
        move |event| emit_safe_launch_event(&emit_app, &emit_launch_id, event),
    )?;
    let prepared = async {
        launcher
            .download_game(tx.clone())
            .await
            .map_err(|e| format!("准备游戏文件失败：{e}"))?;
        let data = launcher
            .game_data()
            .ok_or("游戏文件准备后缺少启动数据，请补全实例")?;
        let agents =
            runtime_agents::prepare(&app, &settings, data, auth_server.as_deref(), &directory)
                .await?;
        let client_jar = launcher
            .options()
            .path
            .join("versions")
            .join(&data.minecraft_json.id)
            .join(format!("{}.jar", data.minecraft_json.id));
        let (mode, name, path, slim) = if launcher.options().bypass_offline {
            (
                settings.offline_skin.as_str(),
                Some(settings.offline_skin_name.as_str()),
                Some(settings.offline_skin_path.as_str()),
                settings.offline_skin_slim,
            )
        } else {
            ("random", None, None, false)
        };
        offline_skin::prepare(
            &directory,
            &client_jar,
            &data.minecraft_version,
            mode,
            name,
            path,
            slim,
        )
        .await?;
        let java = data.minecraft_java.path.clone();
        let java_major = if settings.memory_management == "custom" {
            0
        } else {
            let selected_major = java_selection
                .path
                .as_deref()
                .filter(|selected| selected.eq_ignore_ascii_case(&java))
                .and(java_selection.major);
            if let Some(major) = selected_major.filter(|major| *major > 0) {
                major
            } else {
                let context = minecraft_java_rs_core::net::transfer::TransferContext::current();
                match runtime_agents::probe_java_major(
                    std::path::Path::new(&java),
                    context.cancellation(),
                )
                .await
                {
                    Ok(major) => major,
                    Err(error) => {
                        emit_launch_warning(
                            &app,
                            &launch_id,
                            &log_path,
                            format!("{error}；本次 GC 回退为标准 G1，游戏将继续启动"),
                        );
                        8
                    }
                }
            }
        };
        runtime_settings::configure_launcher(
            &mut launcher,
            directory.clone(),
            &settings,
            java_major,
            runtime_settings::windows_build(),
        )?;
        launcher.set_launch_extras(agents.jvm_args, agents.wrapper);
        // 启动前命令失败或超过 60 秒即终止准备，避免在前置条件未满足时启动游戏。
        runtime_settings::before_launch(&settings, &directory).await?;
        // 启动前命令可能耗时较长，执行下一步前先确认用户没有在等待期间取消。
        ensure_launch_not_cancelled()?;
        let gpu = match gpu_preferences::prepare(
            std::path::Path::new(&java),
            settings.high_performance_gpu,
        ) {
            Ok(guard) => guard,
            Err(error) => {
                emit_launch_warning(
                    &app,
                    &launch_id,
                    &log_path,
                    format!("高性能 GPU 偏好未生效：{error}。游戏将继续启动"),
                );
                None
            }
        };
        ensure_launch_not_cancelled()?;
        let child = launcher
            .launch(tx.clone())
            .await
            .map_err(|e| format!("启动失败：{e}"))?;
        Ok::<_, String>((child, java, java_major, gpu))
    }
    .await;
    drop(tx);
    let (mut child, _java, java_major, gpu) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            output_pump.finish().await;
            return Err(error);
        }
    };
    if let Some(pid) = child.id() {
        #[cfg(all(test, windows))]
        if let Some(observer) = tauri::Manager::try_state::<portable_live_tests::LaunchPid>(&app) {
            observer.0.store(pid, std::sync::atomic::Ordering::Release);
        }
        if let Err(error) = runtime_settings::set_process_priority(pid, &settings.priority) {
            emit_launch_warning(
                &app,
                &launch_id,
                &log_path,
                format!("{error}。游戏将继续以当前优先级运行"),
            );
        }
    }
    // 进程已生成：准备阶段结束，前端收起启动中面板；此后的取消按钮不再可用。
    emit_launch_event(&app, &launch_id, FrontendEvent::Started);
    let diagnostic_version = version_context.minecraft_version.clone();
    let diagnostic_loader = version_context.loader.clone().unwrap_or_default();
    let diagnostic_optifine = diagnostic_loader
        .to_ascii_lowercase()
        .contains("optifine")
        .then(|| version_context.loader_version.clone())
        .flatten();
    let diagnostic_java_major = java_selection
        .major
        .or_else(|| (java_major > 0).then_some(java_major));
    let diagnostic_java_vendor =
        (!java_selection.vendor.is_empty()).then(|| java_selection.vendor.clone());
    let diagnostic_directory = directory;
    tauri::async_runtime::spawn(async move {
        let _running = running;
        let _gpu = gpu;
        let mut window_interval = tokio::time::interval(std::time::Duration::from_millis(500));
        // Java 初始化可能重置进程优先级；首次真实游戏窗口出现后补设一次。
        let mut priority_pending = cfg!(windows) && settings.priority != "normal";
        let mut poll_window = settings.visibility != "keep"
            || settings.window_mode == "maximize"
            || !settings.window_title.is_empty()
            || priority_pending;
        let mut window_seen = false;
        let mut visibility_applied = false;
        let mut maximize_applied = false;
        loop {
            tokio::select! {
                result = child.wait() => {
                    let (exit_code, event) = match result {
                        Ok(status) => {
                            let code = status.code().unwrap_or(-1);
                            (Some(code), FrontendEvent::Close { code })
                        }
                        Err(error) => (
                            Some(-1),
                            FrontendEvent::Error { message: format!("等待游戏进程失败：{error}") },
                        ),
                    };
                    // Pipe EOF and the session flush precede Close and diagnostic/export availability.
                    output_pump.finish().await;
                    emit_launch_event(&app, &launch_id, event);
                    if exit_code != Some(0) {
                        let process_output = captured.lock()
                            .map(|buffer| buffer.text()).unwrap_or_default();
                        let report = runtime_settings::analyze_launch_failure(
                            exit_code,
                            &process_output,
                            &diagnostic_directory,
                            &diagnostic_version,
                            &diagnostic_loader,
                            diagnostic_optifine.as_deref(),
                            diagnostic_java_major,
                            diagnostic_java_vendor.as_deref(),
                        );
                        let title = report.category.label().to_string();
                        let category = serde_json::to_value(report.category)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "unknown".into());
                        let source = report
                            .evidence
                            .first()
                            .map(|evidence| evidence.source.label())
                            .unwrap_or("日志");
                        let _ = runtime_settings::append_launch_log(
                            &log_path,
                            &format!(
                                "[诊断/{source}] {title}：{} {}",
                                report.summary, report.advice
                            ),
                        );
                        let _ = error_report::store_diagnostic(&diagnostic_directory, &report);
                        emit_launch_event(&app, &launch_id, FrontendEvent::Diagnostic {
                            category, title, summary: report.summary, message: report.advice,
                        });
                    }
                    if visibility_applied {
                        runtime_settings::launcher_visibility(&app, &settings.visibility, true);
                    }
                    break;
                }
                _ = window_interval.tick(), if poll_window => {
                    let Some(pid) = child.id() else { continue };
                    if launch_window::find_game_window(pid).is_none() {
                        continue;
                    }
                    if !window_seen {
                        window_seen = true;
                        visibility_applied = settings.visibility != "keep";
                        runtime_settings::launcher_visibility(&app, &settings.visibility, false);
                    }
                    if settings.window_mode == "maximize" && !maximize_applied {
                        maximize_applied = true;
                        // 窗口创建后仍会调整渲染尺寸，延迟最大化避免初始化覆盖窗口状态。
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            launch_window::maximize_window(pid);
                        });
                    }
                    if !settings.window_title.is_empty() {
                        launch_window::set_window_title(pid, &settings.window_title);
                    }
                    if priority_pending {
                        priority_pending = false;
                        if let Err(message) = runtime_settings::set_process_priority(pid, &settings.priority) {
                            emit_launch_warning(
                                &app,
                                &launch_id,
                                &log_path,
                                format!("{message}。游戏将继续以当前优先级运行"),
                            );
                        }
                    }
                    if settings.window_title.is_empty() && !priority_pending {
                        poll_window = false;
                    }
                }
            }
        }
    });
    Ok(())
}

fn apply_active_account(
    app: &AppHandle,
    options: &mut LaunchOptions,
    settings: &runtime_settings::EffectiveSettings,
) -> Result<Option<String>, String> {
    let active = std::fs::read_to_string(store::active_path(app)?)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| {
            value
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    let account = active
        .as_deref()
        .and_then(|id| store::find_account(app, id));
    apply_account(options, settings, account)
}

fn apply_account(
    options: &mut LaunchOptions,
    settings: &runtime_settings::EffectiveSettings,
    account: Option<store::StoredAccount>,
) -> Result<Option<String>, String> {
    let kind = account
        .as_ref()
        .map(|account| account.account_type.as_str())
        .unwrap_or("offline");
    let server_name = if settings.server_name.trim().is_empty() {
        "此实例"
    } else {
        &settings.server_name
    };
    let allowed = match settings.server_login.as_str() {
        "offline" => kind == "offline",
        "microsoft" => kind == "microsoft",
        "thirdParty" => kind == "littleskin",
        _ => true,
    };
    if !allowed {
        return Err(format!(
            "当前账号不符合{server_name}的登录方式，请切换账号或修改实例设置"
        ));
    }
    if !settings.auth_server.trim().is_empty() && kind != "littleskin" {
        return Err(format!("请切换到{server_name}对应的第三方账号后启动"));
    }
    let auth_server = if kind == "littleskin" {
        let server =
            store::normalize_server(account.as_ref().and_then(|account| account.server.clone()));
        if !settings.auth_server.trim().is_empty()
            && server != store::normalize_server(Some(settings.auth_server.clone()))
        {
            return Err(format!(
                "当前账号的认证服务器与{server_name}不一致，请切换账号后启动"
            ));
        }
        // authlib-injector 要用 Yggdrasil API 根；站点根（`https://littleskin.cn`）拿不到认证接口。
        Some(store::yggdrasil_base(&server))
    } else {
        None
    };
    if let Some(account) = account {
        options.authenticator.name = account.name;
        options.authenticator.uuid = store::undash_uuid(&account.uuid);
        if matches!(account.account_type.as_str(), "microsoft" | "littleskin") {
            if account
                .expires_at
                .is_some_and(|expires| expires <= store::now_secs())
            {
                return Err("账号登录已过期，请重新登录后启动".into());
            }
            options.authenticator.access_token = account
                .access_token
                .filter(|token| !token.is_empty())
                .ok_or("账号缺少有效凭据，请重新登录")?;
            options.authenticator.client_token = account.client_token;
            options.authenticator.xbox_account = (account.account_type == "microsoft").then(|| {
                minecraft_java_rs_core::models::minecraft::XboxAccount {
                    xuid: String::new(),
                }
            });
            options.bypass_offline = false;
        }
    }
    Ok(auth_server)
}

/// 组装 LaunchOptions
/// 注意：该 crate 的 LaunchOptions 未实现 Default，必须显式构造全部 21 个字段。
fn build_options(params: &ResolvedLaunch, base: PathBuf) -> LaunchOptions {
    let username = if params.username.trim().is_empty() {
        "Player".to_string()
    } else {
        params.username.trim().to_string()
    };

    // 离线账号：access_token 用占位值，uuid 由用户名确定性生成
    let authenticator = Authenticator {
        access_token: "0".to_string(),
        name: username.clone(),
        uuid: offline_uuid(&username),
        xbox_account: None,
        user_properties: None,
        client_id: None,
        client_token: None,
    };

    LaunchOptions {
        path: base,
        version: params.version.clone(),
        authenticator,
        timeout_secs: 30,
        download_concurrency: network_settings::snapshot().max_download_threads as u32,
        verify_concurrency: 4,
        memory: MemoryConfig {
            min: String::new(),
            max: params.memory_max.clone(),
        },
        java: JavaOptions::default(),
        loader: LoaderConfig::default(),
        screen: ScreenConfig::default(),
        verify: false,
        game_args: vec![],
        jvm_args: vec![],
        instance: None,
        url: None,
        mcp: None,
        intel_enabled_mac: false,
        bypass_offline: true,
        skip_bundle_check: false,
        force_ipv4: false,
        dns: None,
    }
}

fn launch_from_startup_arguments(app: &AppHandle) {
    let request = version_settings::startup_launch_arguments(std::env::args_os().skip(1));
    if matches!(&request, Ok(None)) {
        return;
    }
    let app = app.clone();
    let launch_id = store::random_token();
    tauri::async_runtime::spawn(async move {
        let result = async {
            let (root, version) = request?.ok_or("启动脚本缺少实例参数")?;
            let directory = std::path::Path::new(&root).join("versions").join(&version);
            instances::reject_links(&directory)?;
            if !directory.is_dir() || named_profiles::is_pending_pack(&directory) {
                return Err(
                    "启动脚本指定的实例已不存在或尚未安装完成，请在实例设置中重新导出".into(),
                );
            }
            launch_game(
                app.clone(), Some(version), None, None, None, Some(root), launch_id.clone(),
            )
            .await
        }
        .await;
        if let Err(message) = result {
            use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
            let message = runtime_settings::bounded_log_line(&message);
            emit_launch_event(
                &app,
                &launch_id,
                FrontendEvent::Error {
                    message: message.clone(),
                },
            );
            app.dialog()
                .message(message)
                .title("启动失败")
                .kind(MessageDialogKind::Error)
                .show(|_| {});
        }
    });
}

#[cfg(any(windows, test))]
fn diagnostics_enabled(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| {
        let value = value.to_string_lossy();
        !value.trim().is_empty() && value.trim() != "0"
    })
}

#[cfg(test)]
mod diagnostics_tests {
    use super::diagnostics_enabled;
    use std::ffi::OsStr;

    #[test]
    fn diagnostics_require_a_nonzero_nonblank_value() {
        assert!(diagnostics_enabled(Some(OsStr::new("1"))));
        for value in [None, Some(""), Some("0"), Some(" "), Some(" 0 ")] {
            assert!(!diagnostics_enabled(value.map(OsStr::new)));
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(media::protocol_plugin())
        // 窗口在配置里设为 `visible: false`：透明 + 无边框窗口一旦先显示、WebView 后绘制，
        // 用户会先看到一块「什么都没有」的透明矩形，然后才是启动画面。
        // 这里等页面加载完成（启动画面已经绘制）再显示，首个可见帧就是启动画面。
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                let window = webview.window();
                // 仅当窗口仍隐藏时才 show：开发期 HMR / 页面重载同样会触发 Finished，
                // 每次都 show + set_focus 会把焦点从用户正在使用的其它应用抢回来。
                match window.is_visible() {
                    // 已经可见：属于重载，不再动窗口。
                    Ok(true) => {}
                    Ok(false) => {
                        let _ = window.show();
                        // 确认是一次「隐藏 → 显示」的转换，才把焦点交给窗口。
                        let _ = window.set_focus();
                    }
                    // 查询失败时无法判断可见性：只保证窗口可见，不抢焦点，
                    // 避免在信息不足时把用户从其它应用里拉出来。
                    Err(_) => {
                        let _ = window.show();
                    }
                }
            }
        })
        .setup(|app| {
            let stored = settings_get(app.handle().clone()).unwrap_or_else(|_| "{}".into());
            let value = serde_json::from_str(&stored).unwrap_or(serde_json::Value::Null);
            network_settings::apply(network_settings::from_value(&value).unwrap_or_default());
            // 方案 A：保留「透明 + 无边框」窗口，只把系统在窗口上额外画的装饰关掉：
            //   1. 1px 窗口边框（沿四边/圆角的一圈浅色细线）
            //   2. 顶边残留的非客户区 caption 条（只有顶边有的一条细线）
            // 它们都绘制在 WebView 之外，纯 CSS 无法消除。
            #[cfg(windows)]
            {
                use tauri::Manager;
                if let Some(win) = app.get_webview_window("main") {
                    match win.hwnd() {
                        Ok(h) => {
                            let hwnd = h.0 as *mut std::ffi::c_void;
                            if !dwm::apply_frameless_tweaks(hwnd) {
                                eprintln!(
                                    "[dwm] 部分窗口装饰未能关闭（通常出现在 Windows 10 或更早版本，可忽略）"
                                );
                            }
                            if diagnostics_enabled(
                                std::env::var_os("NICKEL_DIAG").as_deref(),
                            ) {
                                eprintln!("[dwm] 窗口样式 {}", dwm::describe_window(hwnd));
                            }
                        }
                        Err(err) => eprintln!("[dwm] 获取窗口句柄失败: {err}"),
                    }
                }
            }
            // 兜底：页面加载失败（例如开发服务器没起来）时 on_page_load 不会触发，
            // 不能让窗口永远隐藏；超时后无条件显示，保证用户能看到错误页面。
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    if let Some(win) = handle.get_webview_window("main") {
                        // 查询失败时不能假定窗口仍然隐藏：此时更可能是平台 / 权限问题，
                        // 强行 show + set_focus 会抢走用户当前应用的焦点。只 show，
                        // 既避免窗口永远隐藏，又不打扰用户。
                        match win.is_visible() {
                            Ok(true) => {}
                            Ok(false) => {
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                            Err(_) => {
                                let _ = win.show();
                            }
                        }
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_versions,
            default_minecraft_dir,
            resolved_memory_plan,
            launch_game,
            launch_cancel,
            window_minimize,
            window_close,
            settings_get,
            settings_set,
            onboarding::onboarding_get,
            onboarding::onboarding_accept_eula,
            onboarding::onboarding_ack_changelog,
            onboarding::onboarding_finish,
            onboarding::onboarding_reject,
            community_dependencies::community_dependency_resolve,
            portable_bootstrap::portable_bootstrap_status,
            portable_bootstrap::portable_bootstrap_start,
            // 账号存储
            accounts::accounts_list,
            accounts::accounts_upsert,
            accounts::accounts_remove,
            accounts::accounts_set_active,
            // Yggdrasil（LittleSkin）
            yggdrasil::yggdrasil_login,
            yggdrasil::yggdrasil_refresh,
            yggdrasil::yggdrasil_profiles,
            yggdrasil::yggdrasil_validate,
            // Yggdrasil 多角色：选择 / 放弃本次登录会话
            yggdrasil::yggdrasil_select_profile,
            yggdrasil::yggdrasil_cancel,
            // 微软正版（Device Code Flow）
            msa::msa_device_start,
            msa::msa_device_poll,
            secrets::msa_config_get,
            // 本地图片 → dataURL（背景图兜底渲染）
            media::image_data_url,
            media::background_media_validate,
            // 皮肤
            skin::skin_fetch,
            offline_skin::game_offline_skin_pick,
            // 游戏 / 版本设置
            game::game_settings_get,
            game::game_settings_set,
            runtime_settings::game_runtime_info,
            runtime_settings::launch_log_open,
            error_report::launch_error_report_export,
            runtime_settings::version_server_open_registration,
            game_junk::game_junk_scan,
            game_junk::game_junk_clean,
            // 实例（版本根目录）注册表
            instances::launcher_self_dir,
            instances::instance_default_dir,
            instances::instance_suggest_label,
            instances::instance_ensure_default,
            instances::instances_get,
            instances::instances_set,
            instances::instance_create,
            instances::instance_root_rename,
            instances::instance_root_delete,
            // 版本实例 / 本地资源扫描
            instances::instance_scan,
            instances::resource_instance_context,
            instances::resources_scan,
            resources::version_mods,
            resources::version_mods_action,
            resources::version_mod_list,
            resources::version_mod_meta,
            resources::version_mod_updates,
            resources::version_mod_icon,
            install::install_version,
            install::install_cancel,
            app_tasks::app_tasks_snapshot,
            install::modpack_install,
            install::modpack_import,
            pack_import::pack_import_pick,
            pack_export::pack_export_inspect,
            pack_export::pack_export_save,
            pack_export::pack_export_config_save,
            pack_export::pack_export_config_load,
            install::list_remote_versions,
            loader_catalog::list_loaders,
            loader_catalog::list_loader_provider,
            loader_catalog::preload_loader_catalog,
            installer_download::save_loader_installer_managed,
            install::java_detect,
            install::java_resolve,
            install::open_folder,
            // 版本设置独立页（per-version 设置 / 详情 / 改名 / 删除 / 补全文件）
            version_settings::version_detail,
            version_settings::version_settings_get,
            version_settings::version_settings_set,
            version_settings::version_icon_pick,
            version_settings::version_rename,
            version_settings::version_delete,
            version_settings::version_complete_files,
            version_settings::version_export_script,
            // Modrinth（社区资源检索 / 安装）
            modrinth::modrinth_search,
            modrinth::modrinth_project_versions,
            modrinth::modrinth_install,
            modrinth::modrinth_open_project,
            // CurseForge（社区资源第二引擎，检索 / 文件 / 安装 / 项目页）
            curseforge::cf_config_status,
            curseforge::curseforge_search,
            curseforge::curseforge_project_files,
            curseforge::curseforge_install,
            curseforge::curseforge_open_project,
            // 社区资源另存为（先对话框后下载；取消返回 null，不创建任务）
            community_save_as::modrinth_save_as,
            community_save_as::curseforge_save_as,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约回归：`Progress`/`Check` 事件必须以 `type` 作为判别字段，
    /// 且文件类别字段序列化为 `fileType`（不得再出现与判别字段混淆的 `kind`）。
    #[test]
    fn progress_event_wire_contract() {
        let ev = FrontendEvent::Progress {
            downloaded: 3,
            total: 10,
            file_type: "assets".to_string(),
        };
        let value = serde_json::to_value(&ev).unwrap();
        assert_eq!(value.get("type").and_then(|v| v.as_str()), Some("progress"));
        assert_eq!(value.get("downloaded").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(value.get("total").and_then(|v| v.as_u64()), Some(10));
        assert_eq!(
            value.get("fileType").and_then(|v| v.as_str()),
            Some("assets")
        );
        // File categories must not add a competing event discriminator.
        assert!(value.get("kind").is_none());
    }

    #[test]
    fn launch_identity_validation_is_bounded() {
        for id in ["launch-42", "launch_42", "ABC123", &"a".repeat(128)] {
            assert_eq!(validate_launch_id(id.into()).unwrap(), id);
        }
        for id in [
            "",
            "a b",
            "a/b",
            "a\\b",
            "a.b",
            "a\n",
            "\u{00e9}",
            &"a".repeat(129),
        ] {
            assert!(validate_launch_id(id.into()).is_err());
        }
    }

    #[test]
    fn launch_identity_scopes_every_event_without_changing_wire_fields() {
        let events = [
            FrontendEvent::Preparing {
                version_id: "1.21.1".into(),
                game_dir: "C:/acceptance/game".into(),
            },
            FrontendEvent::Progress {
                downloaded: 3,
                total: 10,
                file_type: "assets".into(),
            },
            FrontendEvent::Speed { bps: 1024.0 },
            FrontendEvent::Stage {
                message: "preparing".into(),
            },
            FrontendEvent::Log {
                line: "complete log".into(),
            },
            FrontendEvent::Close { code: 1 },
            FrontendEvent::Started,
            FrontendEvent::Cancelled,
            FrontendEvent::Warning {
                message: "nonfatal".into(),
            },
            FrontendEvent::Error {
                message: "failed".into(),
            },
            FrontendEvent::Diagnostic {
                category: "memory".into(),
                title: "Memory".into(),
                summary: "OutOfMemoryError".into(),
                message: "retry".into(),
            },
        ];
        for event in events {
            let expected = serde_json::to_value(&event).unwrap();
            let scoped = serde_json::to_value(ScopedLaunchEvent {
                launch_id: "launch-42",
                event,
            })
            .unwrap();
            assert_eq!(scoped["launchId"], "launch-42");
            assert!(scoped.get("event").is_none());
            assert!(scoped.get("launch_id").is_none());
            for (key, value) in expected.as_object().unwrap() {
                assert_eq!(&scoped[key], value, "{key}");
            }
        }
    }

    #[test]
    fn launch_identity_rejects_cancel_from_previous_session() {
        let control = LaunchControl {
            token: CancellationToken::default(),
            launch_id: "current-launch".into(),
        };
        assert!(!cancel_launch_control(&control, "previous-launch"));
        assert!(!cancel_launch_control(&control, ""));
        assert!(!control.token.is_cancelled());
        assert!(cancel_launch_control(&control, "current-launch"));
        assert!(control.token.is_cancelled());
    }

    #[test]
    fn diagnostic_event_carries_structured_classification() {
        let ev = FrontendEvent::Diagnostic {
            category: "outOfMemory".into(),
            title: "内存不足".into(),
            summary: "游戏可用内存不足".into(),
            message: "降低实例内存分配后重试".into(),
        };
        let value = serde_json::to_value(&ev).unwrap();
        assert_eq!(value["type"], "diagnostic");
        assert_eq!(value["category"], "outOfMemory");
        assert_eq!(value["title"], "内存不足");
        assert_eq!(value["summary"], "游戏可用内存不足");
    }

    #[test]
    fn frontend_events_redact_client_secret_variants() {
        let secret = "frontend-client-secret-value";
        let mut log = FrontendEvent::Log {
            line: format!("client_secret={secret}"),
        };
        redact_frontend_event(&mut log);
        assert!(
            !serde_json::to_value(log).unwrap()["line"]
                .as_str()
                .unwrap()
                .contains(secret)
        );
        let mut diagnostic = FrontendEvent::Diagnostic {
            category: "unknown".into(),
            title: format!("clientSecret={secret}"),
            summary: format!("apiKey={secret}"),
            message: format!("{{\"client-secret\":\"{secret}\"}}"),
        };
        redact_frontend_event(&mut diagnostic);
        let rendered = serde_json::to_value(diagnostic).unwrap().to_string();
        assert!(!rendered.contains(secret));
        assert!(rendered.contains("***"));
    }

    #[test]
    fn warning_event_is_nonfatal_and_serializes_as_warning() {
        let ev = FrontendEvent::Warning {
            message: "GPU 偏好未生效，游戏继续启动".into(),
        };
        let value = serde_json::to_value(&ev).unwrap();
        assert_eq!(value.get("type").and_then(|v| v.as_str()), Some("warning"));
        assert_eq!(
            value.get("message").and_then(|v| v.as_str()),
            Some("GPU 偏好未生效，游戏继续启动")
        );
    }

    #[test]
    fn check_event_maps_to_progress_with_file_type() {
        let ev = FrontendEvent::from(&LaunchEvent::Check {
            current: 1,
            total: 2,
            kind: "libraries".to_string(),
        });
        let value = serde_json::to_value(&ev).unwrap();
        assert_eq!(value.get("type").and_then(|v| v.as_str()), Some("progress"));
        assert_eq!(
            value.get("fileType").and_then(|v| v.as_str()),
            Some("校验:libraries")
        );
    }

    #[test]
    fn secret_api_key_is_not_an_ipc_command() {
        let source = include_str!("lib.rs");
        let registration = source
            .split_once(".invoke_handler(tauri::generate_handler![")
            .expect("应用必须保留唯一命令注册表")
            .1
            .split_once("])")
            .expect("命令注册表必须完整")
            .0;
        assert!(!registration.contains("secrets::curseforge_api_key"));
        assert!(registration.contains("curseforge::cf_config_status"));
    }

    #[test]
    fn resolved_memory_plan_preview_non_isolated_liteloader_counts_nested_mods()
    -> Result<(), String> {
        let base =
            std::env::temp_dir().join(format!("nickel-memory-preview-{}", store::random_token()));
        let version = "Lite";
        let directory = base.join("versions").join(version);
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        std::fs::write(
            directory.join("nickel.profile.json"),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "name": version,
                "minecraftVersion": "1.12.2",
                "loader": "LiteLoader",
                "loaderVersion": "1.12.2-SNAPSHOT-r0",
                "ready": true
            }))
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        std::fs::write(
            directory.join(format!("{version}.json")),
            serde_json::to_vec(&serde_json::json!({
                "id": version,
                "inheritsFrom": "1.12.2",
                "nickelProfile": {
                    "minecraftVersion": "1.12.2",
                    "loader": "LiteLoader",
                    "loaderVersion": "1.12.2-SNAPSHOT-r0"
                }
            }))
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let mods = base.join("mods/1.12.2");
        std::fs::create_dir_all(&mods).map_err(|error| error.to_string())?;
        std::fs::write(mods.join("active.jar"), b"active").map_err(|error| error.to_string())?;
        std::fs::write(mods.join("active.jar.disabled"), b"disabled")
            .map_err(|error| error.to_string())?;
        std::fs::write(base.join("mods/ignored.jar"), b"ignored")
            .map_err(|error| error.to_string())?;

        let plan = resolved_memory_plan_for(
            &base,
            version,
            &serde_json::json!({}),
            &serde_json::json!({"versionIsolation": false}),
            Some(32_768),
            Some(16_384),
        )?;
        assert_eq!(plan.mod_count, 1);
        std::fs::remove_dir_all(base).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn launch_cancellation_check_uses_transfer_scope_token() {
        // 启动准备在 transfer.scope 内执行；helper 必须读到同一令牌，而不是新令牌。
        let transfer = minecraft_java_rs_core::net::transfer::TransferContext::current();
        transfer
            .scope(async {
                assert!(ensure_launch_not_cancelled().is_ok());
                transfer.cancellation().cancel();
                assert!(ensure_launch_not_cancelled().is_err());
            })
            .await;
    }

    #[test]
    fn runtime_commands_are_registered_once() {
        let source = include_str!("lib.rs");
        let registration = source
            .split_once(".invoke_handler(tauri::generate_handler![")
            .expect("应用必须保留唯一命令注册表")
            .1
            .split_once("])")
            .expect("命令注册表必须完整")
            .0;
        for command in [
            "install::modpack_import,",
            "pack_import::pack_import_pick,",
            "pack_export::pack_export_inspect,",
            "pack_export::pack_export_save,",
            "pack_export::pack_export_config_save,",
            "pack_export::pack_export_config_load,",
            "resources::version_mods,",
            "resources::version_mods_action,",
            "resources::version_mod_list,",
            "resources::version_mod_meta,",
            "resources::version_mod_updates,",
            "resources::version_mod_icon,",
            "instances::resource_instance_context,",
            "runtime_settings::game_runtime_info,",
            "resolved_memory_plan,",
            "runtime_settings::launch_log_open,",
            "app_tasks::app_tasks_snapshot,",
            "instances::instance_create,",
            "instances::instance_root_rename,",
            "instances::instance_root_delete,",
            "version_settings::version_icon_pick,",
            "install::java_resolve,",
            "community_save_as::modrinth_save_as,",
            "community_save_as::curseforge_save_as,",
            "game_junk::game_junk_scan,",
            "game_junk::game_junk_clean,",
            "accounts::accounts_list,",
            "accounts::accounts_upsert,",
            "accounts::accounts_remove,",
            "accounts::accounts_set_active,",
            "msa::msa_device_start,",
            "msa::msa_device_poll,",
            "secrets::msa_config_get,",
            "skin::skin_fetch,",
            "launch_game,",
            "launch_cancel,",
        ] {
            assert_eq!(registration.matches(command).count(), 1, "{command}");
        }
    }

    #[test]
    fn java_resolve_command_contract_matches_frontend() {
        let source = include_str!("lib.rs");
        let registration = source
            .split_once(".invoke_handler(tauri::generate_handler![")
            .expect("应用必须保留唯一命令注册表")
            .1
            .split_once("])")
            .expect("命令注册表必须完整")
            .0;
        assert_eq!(registration.matches("install::java_resolve,").count(), 1);

        let backend = include_str!("install.rs");
        let signature = backend
            .split_once("pub async fn java_resolve(")
            .expect("java_resolve 必须存在")
            .1
            .split_once("-> Result")
            .expect("java_resolve 必须有返回值")
            .0;
        for field in [
            "mc_version",
            "loader",
            "loader_version",
            "release_time",
            "java_path",
        ] {
            assert!(signature.contains(field), "后端缺少字段 {field}");
        }

        let frontend = include_str!("../../src/components/version/vsettings/api.ts");
        let invocation = frontend
            .split_once("invoke<unknown>(\"java_resolve\", {")
            .expect("前端必须调用 java_resolve")
            .1
            .split_once("})")
            .expect("java_resolve 调用必须完整")
            .0;
        for field in [
            "mcVersion",
            "loader",
            "loaderVersion",
            "releaseTime",
            "javaPath",
        ] {
            assert!(invocation.contains(field), "前端缺少字段 {field}");
        }
    }

    #[test]
    fn managed_task_kinds_match_frontend_contract() {
        for (kind, name) in [
            (app_tasks::TaskKind::Version, "version"),
            (app_tasks::TaskKind::Modpack, "modpack"),
            (app_tasks::TaskKind::Resource, "resource"),
            (app_tasks::TaskKind::Installer, "installer"),
            (app_tasks::TaskKind::Export, "export"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
        }
        // Source assertions must tolerate formatting without changing their contract.
        let compact =
            |source: &str| -> String { source.chars().filter(|ch| !ch.is_whitespace()).collect() };
        let install = compact(include_str!("install.rs"));
        assert!(install.contains("describe_kind(app_tasks::TaskKind::Version"));
        assert_eq!(
            install
                .matches("describe_kind(app_tasks::TaskKind::Modpack")
                .count(),
            2
        );
        assert!(
            compact(include_str!("resources.rs"))
                .contains("describe_kind(app_tasks::TaskKind::Resource")
        );
        for source in [
            include_str!("modrinth.rs"),
            include_str!("curseforge.rs"),
            include_str!("community_save_as.rs"),
        ] {
            assert!(compact(source).contains("describe_kind(crate::app_tasks::TaskKind::Resource"));
        }
    }

    #[test]
    fn save_as_commands_return_optional_task_and_never_leak_key() {
        let source = include_str!("lib.rs");
        let registration = source
            .split_once(".invoke_handler(tauri::generate_handler![")
            .expect("应用必须保留唯一命令注册表")
            .1
            .split_once("])")
            .expect("命令注册表必须完整")
            .0;
        // 另存为取消对话框返回 null（Option<String>），与安装器受管入口同契约。
        let backend = include_str!("community_save_as.rs");
        for command in ["modrinth_save_as", "curseforge_save_as"] {
            assert_eq!(
                registration
                    .matches(&format!("community_save_as::{command},"))
                    .count(),
                1,
                "{command} 必须注册且仅注册一次"
            );
            let signature = backend
                .split_once(&format!("pub async fn {command}("))
                .unwrap_or_else(|| panic!("{command} 必须存在"))
                .1
                .split_once(") -> Result")
                .unwrap_or_else(|| panic!("{command} 必须有返回值"))
                .0;
            assert!(
                signature.contains("instance_version_id"),
                "{command} 缺少实例身份"
            );
            assert!(signature.contains("root"), "{command} 缺少实例根目录");
            assert!(
                backend.contains("Result<Option<String>, String>"),
                "{command} 必须返回 taskId 或 null"
            );
        }
        // 密钥读取只供后端内部使用，另存为不得把 Key 注册成 IPC 或回传前端。
        assert!(!registration.contains("secrets::curseforge_api_key"));
        assert!(backend.contains("crate::secrets::curseforge_api_key(app.clone())?"));
    }
}
