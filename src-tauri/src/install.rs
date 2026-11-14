//! 可等待基础安装与版本任务：核心负责下载/加载器，app_tasks 唯一持有取消和终态。
//! 整合包复用 install_base，不创建子任务，不在基础文件完成时提前发布实例。

// runtime_settings / 启动编排的下一位消费者由跨 owner 接入；策略本身由单元测试覆盖。
#[path = "java_selection.rs"]
pub mod java_selection;

use crate::app_tasks::{self, TaskContext};
use std::path::{Path, PathBuf};

use minecraft_java_rs_core::error::{DownloadError, LaunchError, LoaderError};
use minecraft_java_rs_core::launcher::Launcher;
use minecraft_java_rs_core::launcher::events::LaunchEvent;
use minecraft_java_rs_core::launcher::options::{LoaderConfig, MemoryConfig};
use minecraft_java_rs_core::models::loader::{LoaderType, ResolvedLoaderArtifact};
use minecraft_java_rs_core::net::client::describe_reqwest_error;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::{instances, loader_catalog, named_profiles, store};

/// 安装事件名（前端 `listen("install-event")`）。
pub const INSTALL_EVENT: &str = "install-event";

/// 阶段名（与前端契约一字不差）。
const STAGE_ENV: &str = "检查运行环境";
const STAGE_VANILLA: &str = "下载原版";
const STAGE_LOADER: &str = "安装加载器";
const STAGE_VERIFY: &str = "校验文件";
#[cfg(test)]

/// 取消 / 完成事件的文案。
const CANCEL_MESSAGE: &str = "已取消安装";

/// 版本清单缺失 `type` 时的默认值。
const DEFAULT_VERSION_TYPE: &str = "release";

/// Java 探测上限：避免在装有大量 JDK 的机器上启动过多进程。
const MAX_PROBED_JAVA: usize = 24;
/// 扫描内置运行时（`runtime/*/*/bin/java`）时的最大递归深度。
const RUNTIME_SCAN_DEPTH: usize = 6;
/// `.minecraft` 目录名（与 `instances` 模块的约定一致）。
const MINECRAFT_DIR_NAME: &str = ".minecraft";
/// 无法读取 `java -version` 时的厂商标记（契约里「标注 unverified」）。
const VENDOR_UNVERIFIED: &str = "unverified";
/// 能跑起来但认不出来源的厂商标记。
const VENDOR_UNKNOWN: &str = "Unknown";

// ─────────────────────────────────────────────────────────────────────────────
// 1. 安装事件（前端契约）
// ─────────────────────────────────────────────────────────────────────────────

/// 推给前端的安装事件（`install-event` 的 payload，由 app_tasks 唯一发布）。
#[derive(Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum InstallEvent {
    Stage {
        task_id: String,
        stage: String,
        detail: String,
    },
    Progress {
        task_id: String,
        percent: f64,
        files_done: u64,
        files_total: u64,
        speed_bps: f64,
        remaining_ms: u64,
    },
    File {
        task_id: String,
        name: String,
        percent: f64,
        done: bool,
    },
    Done {
        task_id: String,
        ok: bool,
        message: String,
        cancelled: bool,
        result: Option<app_tasks::InstalledInstance>,
    },
}

/// `downloaded / total` 归一化到 0.0 ~ 1.0；`total = 0` 时视为未知。
fn ratio(value: f64, total: f64) -> f64 {
    if total > 0.0 {
        (value / total).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// 单文件完成度。已完成恒为 1.0；否则按字节比例（未知大小按 0.0）。
/// 当前仅由单元测试锁定换算口径（生产事件使用聚合 Progress）。
#[cfg(test)]
fn file_percent(downloaded: u64, total: u64, done: bool) -> f64 {
    if done {
        return 1.0;
    }
    ratio(downloaded as f64, total as f64)
}

pub(crate) fn progress_event(
    task_id: String,
    files_done: u64,
    files_total: u64,
    speed_bps: f64,
    remaining_ms: u64,
) -> InstallEvent {
    InstallEvent::Progress {
        task_id,
        percent: ratio(files_done as f64, files_total as f64),
        files_done,
        files_total,
        speed_bps,
        remaining_ms,
    }
}

/// 下载收尾结束前保护实例，目录删除和任务登记共用应用注册表。
pub(crate) fn with_idle_directory<T>(
    directory: &Path,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    app_tasks::with_idle_directory(directory, action)
}

async fn pump_events(
    task: TaskContext,
    mut rx: tokio::sync::mpsc::Receiver<LaunchEvent>,
    loader: String,
) {
    while let Some(event) = rx.recv().await {
        if task.check_cancelled().is_err() {
            continue;
        }
        match event {
            LaunchEvent::Progress {
                downloaded,
                total,
                kind,
            } => {
                task.stage(STAGE_VANILLA, &kind);
                task.progress_files(downloaded, total);
            }
            // 核心 Progress 的单位是文件；速度事件不能恢复上一批文件数。
            LaunchEvent::Speed(value) => task.transfer_progress(Some(value), None),
            LaunchEvent::Estimated(seconds) => {
                let remaining = if seconds.is_finite() && seconds > 0.0 {
                    (seconds * 1000.0) as u64
                } else {
                    0
                };
                task.transfer_progress(None, Some(remaining));
            }
            LaunchEvent::Check {
                current,
                total,
                kind,
            } => {
                task.stage(STAGE_VERIFY, &kind);
                task.progress_files(current as u64, total as u64);
            }
            LaunchEvent::Extract(name) => {
                task.stage("解压文件", "");
                task.file(&name, 1.0, true);
            }
            LaunchEvent::Patch(name) => {
                task.stage(STAGE_LOADER, &loader);
                task.file(&name, 1.0, true);
            }
            LaunchEvent::Error(message) => task.stage("重试下载", &message),
            LaunchEvent::GameDownloadFinished | LaunchEvent::Data(_) | LaunchEvent::Close(_) => {}
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. 安装命令
// ─────────────────────────────────────────────────────────────────────────────

/// 安装失败信息的长度上限（避免把整段 HTTP 响应体贴给用户）。
fn brief(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

fn describe_download_error(err: &DownloadError) -> String {
    match err {
        DownloadError::ChecksumMismatch { file, .. } => {
            format!("文件校验失败：{file}（下载内容与官方校验值不一致，请重试）")
        }
        DownloadError::Connection { url, detail } => {
            format!("无法连接下载服务器：{url}（{detail}）")
        }
        DownloadError::Http(e) => format!("下载失败：{}", describe_reqwest_error(e)),
        DownloadError::Io(e) => format!("写入文件失败：{e}"),
        DownloadError::Timeout => "下载超时，请检查网络后重试".to_string(),
        DownloadError::NoMirrorAvailable(url) => format!("没有可用的下载源：{url}"),
    }
}

fn describe_loader_error(err: &LoaderError) -> String {
    match err {
        LoaderError::InstallerDownloadFailed(detail) => {
            format!("加载器安装包下载失败：{}", brief(detail, 160))
        }
        LoaderError::InstallerNotFound(path) => format!("加载器安装包不存在：{path}"),
        LoaderError::ProfileNotFound => {
            "加载器安装包缺少 install_profile.json，请确认加载器版本与 Minecraft 版本匹配"
                .to_string()
        }
        LoaderError::ProcessorFailed { processor, code } => {
            let code = code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "未知".to_string());
            format!("加载器处理步骤执行失败：{processor}（退出码 {code}）")
        }
        LoaderError::ApiError(detail) => format!("加载器元数据接口失败：{}", brief(detail, 160)),
        LoaderError::VersionNotFound(detail) => {
            format!("加载器版本不可用：{}", brief(detail, 160))
        }
        LoaderError::Archive(detail) => format!("加载器归档处理失败：{detail}"),
        LoaderError::Io(e) => format!("加载器文件读写失败：{e}"),
        LoaderError::Http(e) => format!("加载器网络请求失败：{}", describe_reqwest_error(e)),
        LoaderError::Json(e) => format!("加载器元数据解析失败：{e}"),
    }
}

/// 把核心库错误翻译成可执行的中文提示（纯函数，便于测试）。
fn describe_install_error(err: &LaunchError) -> String {
    match err {
        LaunchError::NoAuthenticator => "缺少账号信息，无法安装".to_string(),
        LaunchError::NoInternetNoCache => "无法连接网络，且本地没有可用的游戏文件缓存".to_string(),
        LaunchError::GameDataNotReady => "游戏数据尚未就绪，请重新安装".to_string(),
        LaunchError::CorruptCache(detail) => format!("本地缓存损坏：{}", brief(detail, 200)),
        LaunchError::VersionNotFound(version) => {
            format!("官方清单里没有版本 {version}（请确认版本号拼写）")
        }
        LaunchError::ProcessError(detail) => format!("进程执行失败：{detail}"),
        LaunchError::Archive(detail) => format!("归档处理失败：{detail}"),
        LaunchError::Download(e) => describe_download_error(e),
        LaunchError::Loader(e) => describe_loader_error(e),
        LaunchError::Io(e) => format!("读写文件失败：{e}"),
        LaunchError::Json(e) => format!("元数据解析失败：{e}"),
        LaunchError::Http(e) => format!("网络请求失败：{}", describe_reqwest_error(e)),
        LaunchError::InvalidData(detail) => format!("元数据不合法：{}", brief(detail, 200)),
    }
}

/// 加载器白名单：`null` / `"vanilla"` / `"none"` → 原版。
fn parse_loader(loader: Option<&str>) -> Result<Option<LoaderType>, String> {
    let Some(raw) = loader else {
        return Ok(None);
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "none" | "null" | "vanilla" => Ok(None),
        "forge" => Ok(Some(LoaderType::Forge)),
        "neoforge" => Ok(Some(LoaderType::NeoForge)),
        "fabric" => Ok(Some(LoaderType::Fabric)),
        "quilt" => Ok(Some(LoaderType::Quilt)),
        "optifine" => Ok(Some(LoaderType::OptiFine)),
        "liteloader" => Ok(Some(LoaderType::LiteLoader)),
        other => Err(format!(
            "不支持的加载器：{other}（可选 forge / neoforge / fabric / quilt / optifine / liteloader）"
        )),
    }
}

/// 组装加载器配置。`build` 由 `install_version` 归一化（缺省用核心默认值 `latest`）。
fn loader_config(
    loader_type: Option<LoaderType>,
    build: &str,
    artifact: Option<ResolvedLoaderArtifact>,
) -> LoaderConfig {
    match loader_type {
        Some(loader_type) => LoaderConfig {
            loader_type: Some(loader_type),
            build: build.to_string(),
            enable: true,
            path: None,
            config: None,
            artifact,
        },
        None => LoaderConfig::default(),
    }
}

/// 阶段文案里展示的加载器名（`LoaderType` 已实现 `Display`）。
fn loader_label(loader_type: &Option<LoaderType>, build: &str) -> String {
    match loader_type {
        Some(t) => format!("{t} {build}"),
        None => String::new(),
    }
}

/// 解析安装目录：空 → 应用数据目录下的默认 `.minecraft`（与启动流程一致）。
fn resolve_install_dir(app: &AppHandle, dir: &str) -> Result<PathBuf, String> {
    let trimmed = dir.trim();
    let base = if trimmed.is_empty() {
        crate::default_game_dir(app)?
    } else {
        PathBuf::from(trimmed)
    };
    std::fs::create_dir_all(&base)
        .map_err(|e| format!("创建游戏目录失败 {}：{e}", base.display()))?;
    Ok(base)
}

/// 新安装才在线复核所选构建，已有实例补全和启动不增加在线前置条件。
#[tauri::command]
pub async fn install_version(
    app: AppHandle,
    dir: String,
    version_id: String,
    loader: Option<String>,
    loader_version: Option<String>,
    source: Option<String>,
    instance_name: Option<String>,
) -> Result<String, String> {
    let version = version_id.trim().to_string();
    if !named_profiles::valid_minecraft_version(&version) {
        return Err("Minecraft 版本号无效，请重新选择版本".into());
    }
    let loader_type = parse_loader(loader.as_deref())?;
    let build = loader_version
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "latest".into());
    if !loader_catalog::valid_version(&build) {
        return Err("加载器版本号无效".into());
    }
    let network = crate::network_settings::snapshot();
    let transfer = network.transfer(source.as_deref())?;
    let artifact = match loader_type.as_ref() {
        Some(kind) => {
            let artifact = loader_catalog::resolve_artifact(
                &version,
                &kind.to_string(),
                &build,
                Some(crate::network_settings::policy_name(
                    network.version_list_source,
                )),
            )
            .await?;
            if !artifact.installable {
                return Err("该构建没有受支持的安装方式，请重新选择".into());
            }
            Some(artifact)
        }
        None => None,
    };
    let root = resolve_install_dir(&app, &dir)?;
    let root = std::fs::canonicalize(root).map_err(|e| format!("解析游戏目录失败：{e}"))?;
    let name = instance_name.unwrap_or_else(|| version.clone());
    let (mut profile, game_dir) =
        named_profiles::reserve(&root, &name, &version, loader_type.clone(), &build)?;
    if let Some(artifact) = artifact.clone() {
        profile.set_loader_artifact(&game_dir, artifact)?;
    }
    let resource_root = crate::runtime_settings::resource_directory(&app, &root, &name)?;
    schedule_install(
        app,
        version,
        loader_type,
        build,
        artifact,
        game_dir,
        resource_root,
        Some(profile),
        Some(crate::network_settings::policy_name(transfer.file_policy())),
    )
}

pub async fn repair_version(
    app: AppHandle,
    dir: String,
    version_id: String,
) -> Result<String, String> {
    let root = resolve_install_dir(&app, &dir)?;
    let root = std::fs::canonicalize(root).map_err(|e| format!("解析游戏目录失败：{e}"))?;
    if let Some(mut profile) = named_profiles::load(&root, &version_id)? {
        let game_dir = root.join("versions").join(&version_id);
        let artifact = match profile.loader_artifact.clone() {
            Some(artifact) => Some(artifact),
            None
                if matches!(
                    profile.loader,
                    Some(LoaderType::OptiFine | LoaderType::LiteLoader)
                ) =>
            {
                Some(
                    loader_catalog::resolve_artifact(
                        &profile.minecraft_version,
                        &profile.loader.as_ref().unwrap().to_string(),
                        &profile.loader_version,
                        None,
                    )
                    .await?,
                )
            }
            None => None,
        };
        if let Some(artifact) = artifact.clone() {
            profile.set_loader_artifact(&game_dir, artifact)?;
        }
        let resource_root = crate::runtime_settings::resource_directory(&app, &root, &version_id)?;
        schedule_install(
            app,
            profile.minecraft_version.clone(),
            profile.loader.clone(),
            profile.loader_version.clone(),
            artifact,
            game_dir,
            resource_root,
            Some(profile),
            None,
        )
    } else {
        if !named_profiles::valid_minecraft_version(&version_id) {
            return Err("旧布局实例缺少真实版本记录，请重新安装该版本".into());
        }
        let resource_root = crate::runtime_settings::resource_directory(&app, &root, &version_id)?;
        schedule_install(
            app,
            version_id,
            None,
            "latest".into(),
            None,
            root,
            resource_root,
            None,
            None,
        )
    }
}

fn schedule_install(
    app: AppHandle,
    version: String,
    loader: Option<LoaderType>,
    build: String,
    artifact: Option<ResolvedLoaderArtifact>,
    game_dir: PathBuf,
    resource_root: PathBuf,
    mut profile: Option<named_profiles::NamedProfile>,
    source: Option<&str>,
) -> Result<String, String> {
    if !game_dir.is_dir() {
        return Err("实例目录已被移除，请重新安装".into());
    }
    let task = app_tasks::begin(&app, &game_dir, source)?;
    task.describe_kind(app_tasks::TaskKind::Version, &format!("Minecraft {version}"));
    let id = task.id.clone();
    tauri::async_runtime::spawn(async move {
        let result = install_base_with_artifact(
            &task,
            &game_dir,
            &resource_root,
            &version,
            loader,
            &build,
            artifact,
        )
        .await;
        task.finish_with(|| {
            let launcher = result?;
            if let Some(profile) = profile.as_mut() {
                profile.complete(
                    &game_dir,
                    launcher
                        .game_data()
                        .ok_or("安装结束但缺少启动缓存，请重新补全文件")?,
                )?;
            }
            Ok(None)
        });
    });
    Ok(id)
}

/// 核心安装完成后执行应用层最终提交：复用目录 resolver 的 Artifact，
/// 原子写入 LiteLoader / 旧版 OptiFine profile，并验证新版 OptiFine 输出。
fn finalize_loader_files(
    game_dir: &Path,
    resource_root: &Path,
    loader: Option<&LoaderType>,
    artifact: Option<&ResolvedLoaderArtifact>,
) -> Result<(), String> {
    let (Some(loader), Some(artifact)) = (loader, artifact) else {
        return Ok(());
    };
    match loader {
        LoaderType::LiteLoader => {
            let entry: loader_catalog::liteloader_install::LiteLoaderEntry =
                serde_json::from_value(
                    artifact
                        .raw_metadata
                        .clone()
                        .ok_or("LiteLoader Artifact 缺少原始条目，请刷新目录后重装")?,
                )
                .map_err(|_| "LiteLoader Artifact 元数据损坏，请刷新目录后重装")?;
            loader_catalog::liteloader_install::write_profile_atomic(resource_root, &entry)?;
        }
        LoaderType::OptiFine => {
            let route = artifact
                .raw_metadata
                .as_ref()
                .and_then(|value| value.get("route"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if route == "launchwrapper" {
                let entry: loader_catalog::optifine_install::OptiFineEntry =
                    serde_json::from_value(
                        artifact
                            .raw_metadata
                            .clone()
                            .ok_or("OptiFine Artifact 缺少原始条目，请刷新目录后重装")?,
                    )
                    .map_err(|_| "OptiFine Artifact 元数据损坏，请刷新目录后重装")?;
                let profile = loader_catalog::optifine_install::launchwrapper_profile(
                    &entry.minecraft_id,
                    &entry.file_name,
                    None,
                );
                let profile_id = profile
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("OptiFine LaunchWrapper profile 缺少 id")?;
                let path = game_dir
                    .join("loader")
                    .join("optifine")
                    .join("versions")
                    .join(profile_id)
                    .join(format!("{profile_id}.json"));
                let parent = path.parent().ok_or("OptiFine profile 缺少父目录")?;
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("创建 OptiFine profile 目录失败：{error}"))?;
                let bytes = serde_json::to_vec_pretty(&profile)
                    .map_err(|error| format!("序列化 OptiFine profile 失败：{error}"))?;
                named_profiles::atomic_write(&path, &bytes)?;
                std::fs::create_dir_all(game_dir.join("mods"))
                    .map_err(|error| format!("创建 OptiFine mods 目录失败：{error}"))?;
            } else if route == "installer" {
                let mut valid = false;
                if let Ok(entries) = std::fs::read_dir(game_dir.join("loader/optifine/versions")) {
                    for entry in entries.flatten() {
                        let source = entry.path();
                        let path = if source.is_dir() {
                            let Some(name) = source.file_name().and_then(|name| name.to_str()) else {
                                continue;
                            };
                            source.join(format!("{name}.json"))
                        } else {
                            source
                        };
                        if !path
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
                        {
                            continue;
                        }
                        let Ok(text) = std::fs::read_to_string(&path) else {
                            continue;
                        };
                        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                            continue;
                        };
                        if value
                            .get("libraries")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|libraries| {
                                libraries.iter().any(|library| {
                                    library
                                        .get("name")
                                        .and_then(serde_json::Value::as_str)
                                        .is_some_and(|name| {
                                            name.starts_with("optifine:OptiFine:")
                                        })
                                })
                            })
                        {
                            valid = true;
                            break;
                        }
                    }
                }
                if !valid {
                    return Err(
                        "OptiFine 安装器未生成可提交 profile，请重试或切换文件下载源".into(),
                    );
                }
                std::fs::create_dir_all(game_dir.join("mods"))
                    .map_err(|error| format!("创建 OptiFine mods 目录失败：{error}"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// 只安装并返回仍持有 GameData 的 launcher，调用者决定何时发布目录和唯一 done。
#[cfg(test)]
pub async fn install_base(
    task: &TaskContext,
    game_dir: &Path,
    version: &str,
    loader: Option<LoaderType>,
    build: &str,
) -> Result<Launcher, String> {
    install_base_with_artifact(task, game_dir, game_dir, version, loader, build, None).await
}

async fn install_base_with_artifact(
    task: &TaskContext,
    game_dir: &Path,
    resource_root: &Path,
    version: &str,
    loader: Option<LoaderType>,
    build: &str,
    artifact: Option<ResolvedLoaderArtifact>,
) -> Result<Launcher, String> {
    task.check_cancelled()?;
    task.stage(STAGE_ENV, &format!("Minecraft {version}"));
    let memory = MemoryConfig::default();
    let resolved = crate::ResolvedLaunch {
        version: version.into(),
        username: String::new(),
        memory_min: memory.min,
        memory_max: memory.max,
        game_dir: Some(game_dir.to_string_lossy().into()),
    };
    let finalizer_artifact = artifact.clone();
    let mut options = crate::build_options(&resolved, game_dir.to_path_buf());
    options.loader = loader_config(loader.clone(), build, artifact);
    let (tx, rx) = tokio::sync::mpsc::channel::<LaunchEvent>(1024);
    let pump =
        tauri::async_runtime::spawn(pump_events(task.clone(), rx, loader_label(&loader, build)));
    let mut launcher = Launcher::new(options);
    let result = task
        .transfer
        .scope(launcher.download_game(tx))
        .await
        .map_err(|error| describe_install_error(&error));
    let _ = pump.await;
    task.check_cancelled()?;
    result?;
    task.check_cancelled()?;
    finalize_loader_files(
        game_dir,
        resource_root,
        loader.as_ref(),
        finalizer_artifact.as_ref(),
    )?;
    Ok(launcher)
}

#[tauri::command]
pub fn install_cancel(task_id: String) {
    app_tasks::cancel(&task_id);
}

fn pack_root(app: &AppHandle, requested: Option<&str>) -> Result<PathBuf, String> {
    let registry = instances::registry_from_text(&instances::instances_get(app.clone()));
    let game: serde_json::Value =
        serde_json::from_str(&crate::game::game_settings_get(app.clone())?)
            .map_err(|_| "游戏设置无法解析，请重新选择实例根目录")?;
    let configured = requested
        .filter(|value| !value.is_empty())
        .or_else(|| {
            game.get("gameDir")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
        })
        .or(registry.active.as_deref());
    let root = match configured {
        Some(root) => PathBuf::from(root),
        None => PathBuf::from(instances::instance_ensure_default(app.clone())?),
    };
    if !root.is_absolute() {
        return Err("请选择绝对路径的游戏目录".into());
    }
    instances::reject_links(&root)?;
    std::fs::create_dir_all(&root).map_err(|error| format!("创建游戏目录失败：{error}"))?;
    std::fs::canonicalize(root).map_err(|error| format!("解析游戏目录失败：{error}"))
}

/// 远程下载和本地导入使用同一预留、提交、取消清理事务。
async fn execute_pack_plan(
    app: &AppHandle,
    task: &TaskContext,
    root: &Path,
    plan: crate::modpacks::PackPlan,
) {
    execute_pack_plan_with_bootstrap(task, root, plan, None, Some(app)).await;
}

pub(crate) async fn execute_portable_plan(
    task: &TaskContext,
    plan: crate::modpacks::PackPlan,
    bootstrap: crate::portable_pack::PortableBootstrap,
) {
    let root = bootstrap.game_directory.clone();
    execute_pack_plan_with_bootstrap(task, &root, plan, Some(bootstrap), None).await;
}

fn pack_loader_requires_artifact(loader: &LoaderType) -> bool {
    matches!(
        loader,
        LoaderType::Forge
            | LoaderType::NeoForge
            | LoaderType::OptiFine
            | LoaderType::LiteLoader
    )
}

async fn execute_pack_plan_with_bootstrap(
    task: &TaskContext,
    root: &Path,
    plan: crate::modpacks::PackPlan,
    bootstrap: Option<crate::portable_pack::PortableBootstrap>,
    app: Option<&AppHandle>,
) {
    let prepared = async {
        task.check_cancelled()?;
        let loader = parse_loader(plan.loader.as_ref().map(|loader| loader.kind.as_str()))?;
        let build = plan
            .loader
            .as_ref()
            .map(|loader| loader.version.as_str())
            .unwrap_or("");
        // Forge/NeoForge 不再在核心按 mc+build 重拼 URL；LiteLoader/OptiFine
        // 的安装也必须消费目录 resolver 提供的 artifact。
        let artifact = match loader.as_ref() {
            Some(loader_type) if pack_loader_requires_artifact(loader_type) => {
                let network = crate::network_settings::snapshot();
                let policy = crate::network_settings::policy_name(network.version_list_source);
                task.check_cancelled()?;
                let artifact = loader_catalog::resolve_artifact(
                    &plan.minecraft_version,
                    &loader_type.to_string(),
                    build,
                    Some(&policy),
                )
                .await?;
                task.check_cancelled()?;
                if !artifact.installable {
                    return Err("该整合包所选加载器没有受支持的自动安装方式".into());
                }
                Some(artifact)
            }
            _ => None,
        };
        let mut reserved = named_profiles::reserve_pack(
            root,
            &plan.display_name,
            &plan.minecraft_version,
            loader.clone(),
            build,
        )?;
        if let Some(artifact) = artifact.clone() {
            reserved
                .profile
                .set_loader_artifact(&reserved.directory, artifact)?;
        }
        let resource_root = match app {
            Some(app) => crate::runtime_settings::resource_directory(
                app,
                root,
                &reserved.profile.name,
            )?,
            None => reserved.directory.clone(),
        };
        Ok::<_, String>((loader, artifact, reserved, resource_root))
    }
    .await;
    let (loader, artifact, reserved, resource_root) = match prepared {
        Ok(value) => value,
        Err(error) => {
            task.finish(Err(error));
            return;
        }
    };
    let result = async {
        task.set_target(&reserved.directory)?;
        let build = plan
            .loader
            .as_ref()
            .map(|loader| loader.version.as_str())
            .unwrap_or("");
        let launcher = install_base_with_artifact(
            task,
            &reserved.directory,
            &resource_root,
            &plan.minecraft_version,
            loader,
            build,
            artifact,
        )
        .await?;
        task.check_cancelled()?;
        task.stage("安装整合包文件", &plan.display_name);
        crate::modpacks::apply_files(&plan, &reserved.directory, task).await?;
        task.check_cancelled()?;
        Ok::<_, String>(launcher)
    }
    .await;
    task.finish_transaction(
        reserved,
        move |reserved| {
            let launcher = result?;
            let data = launcher
                .game_data()
                .ok_or("整合包缺少基础启动数据，请重新安装")?;
            reserved.commit(data, &plan.display_name, &plan.source)?;
            if let Some(bootstrap) = bootstrap {
                crate::portable_bootstrap::finish_import(&bootstrap, &reserved.profile.name)?;
            }
            Ok(Some(app_tasks::InstalledInstance {
                root: reserved.root.to_string_lossy().into(),
                version_id: reserved.profile.name.clone(),
                display_name: plan.display_name,
                source: plan.source,
            }))
        },
        named_profiles::ReservedProfile::cleanup,
    );
}

/// 实例名按原始输入校验，非法名称不会创建后台任务或目录。
#[tauri::command]
pub fn modpack_install(
    app: AppHandle,
    request: crate::modpacks::PackRequest,
) -> Result<String, String> {
    if let Some(name) = request.instance_name.as_deref() {
        named_profiles::validate_name(name)?;
    }
    let root = pack_root(&app, None)?;
    let target = root
        .join("versions")
        .join(format!(".pack-{}", store::random_token()));
    let task = app_tasks::begin(&app, &target, None)?;
    let label = request.instance_name.as_deref().unwrap_or("安装整合包");
    task.describe_kind(app_tasks::TaskKind::Modpack, label);
    let id = task.id.clone();
    tauri::async_runtime::spawn(async move {
        match crate::modpacks::resolve_plan(&app, &request, &task).await {
            Ok(plan) => execute_pack_plan(&app, &task, &root, plan).await,
            Err(error) => task.finish(Err(error)),
        }
    });
    Ok(id)
}

#[tauri::command]
pub fn modpack_import(
    app: AppHandle,
    path: String,
    dir: String,
    instance_name: String,
) -> Result<String, String> {
    named_profiles::validate_name(&instance_name)?;
    let archive = PathBuf::from(path);
    if !archive.is_absolute() || !archive.is_file() {
        return Err("整合包文件已不存在，请重新选择 .mrpack 或 ZIP 文件".into());
    }
    instances::reject_links(&archive)?;
    let root = pack_root(&app, Some(&dir))?;
    let target = root
        .join("versions")
        .join(format!(".pack-{}", store::random_token()));
    let task = app_tasks::begin(&app, &target, None)?;
    task.describe_kind(app_tasks::TaskKind::Modpack, &instance_name);
    let id = task.id.clone();
    tauri::async_runtime::spawn(async move {
        match crate::modpacks::resolve_local_plan(&app, &archive, &instance_name, &task).await {
            Ok(plan) => execute_pack_plan(&app, &task, &root, plan).await,
            Err(error) => task.finish(Err(error)),
        }
    });
    Ok(id)
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. 远程版本清单
// ─────────────────────────────────────────────────────────────────────────────

/// 归一化后的版本清单条目。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteVersion {
    pub id: String,
    #[serde(rename = "type")]
    pub version_type: String,
    pub release_time: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteLatest {
    pub release: String,
    pub snapshot: String,
}

/// `list_remote_versions` 的返回结构。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteVersions {
    pub latest: RemoteLatest,
    pub versions: Vec<RemoteVersion>,
}

fn json_text(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

/// 把 Mojang / BMCLAPI 的版本清单归一化为前端契约的 JSON 文本（纯函数，便于测试）。
fn normalize_manifest(manifest: &serde_json::Value) -> Result<String, String> {
    let object = manifest
        .as_object()
        .ok_or_else(|| "版本清单格式无法识别：期望一个 JSON 对象".to_string())?;
    let entries = object
        .get("versions")
        .ok_or_else(|| "版本清单缺少 versions 字段".to_string())?
        .as_array()
        .ok_or_else(|| "版本清单的 versions 不是数组".to_string())?;

    let latest = object.get("latest");
    let latest = RemoteLatest {
        release: json_text(latest.and_then(|l| l.get("release"))),
        snapshot: json_text(latest.and_then(|l| l.get("snapshot"))),
    };

    let mut versions = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(id) = entry
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let version_type = entry
            .get("type")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_VERSION_TYPE);
        // 个别镜像响应没有 `releaseTime`，回退到 `time`（发布与最后修改时间通常一致）。
        let release_time = entry
            .get("releaseTime")
            .and_then(|v| v.as_str())
            .or_else(|| entry.get("time").and_then(|v| v.as_str()))
            .unwrap_or("");
        versions.push(RemoteVersion {
            id: id.to_string(),
            version_type: version_type.to_string(),
            release_time: release_time.to_string(),
            url: json_text(entry.get("url")),
        });
    }

    if versions.is_empty() {
        return Err("版本清单里没有可用的版本条目".to_string());
    }

    serde_json::to_string(&RemoteVersions { latest, versions })
        .map_err(|e| format!("版本清单序列化失败：{e}"))
}

/// 按本次请求选择版本清单来源，不修改进程全局下载源。
#[tauri::command]
pub async fn list_remote_versions(source: Option<String>) -> Result<String, String> {
    use minecraft_java_rs_core::net::{
        client::build_client,
        transfer::{
            CancellationToken, RequestCandidate, TransferContext, fetch_validated_candidates,
        },
    };
    let settings = crate::network_settings::snapshot();
    let context = TransferContext::new(
        crate::network_settings::list_policy(source.as_deref())?,
        settings.file_download_source,
        CancellationToken::new(),
    );
    let client = build_client(crate::http::TIMEOUT_SECS, false, None)
        .map_err(|error| describe_reqwest_error(&error))?;
    let candidates = [
        RequestCandidate::official(crate::VERSION_MANIFEST_URL),
        RequestCandidate::mirror(format!(
            "{}/mc/game/version_manifest_v2.json",
            minecraft_java_rs_core::mirror::BMCLAPI_BASE
        )),
    ];
    fetch_validated_candidates(
        &client,
        &context,
        "minecraft-manifest",
        &candidates,
        16 * 1024 * 1024,
        |bytes| {
            let value =
                serde_json::from_slice(bytes).map_err(|_| "版本清单不是有效 JSON".to_string())?;
            normalize_manifest(&value)
        },
    )
    .await
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. Java 探测
// ─────────────────────────────────────────────────────────────────────────────

/// 一条本机 Java 安装。无法读取版本号时 `version` 为空串、`vendor` 为 `"unverified"`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaInstallation {
    pub path: String,
    pub version: String,
    pub vendor: String,
    /// 探测顺序不是实际自动选择；启动前由 `select_java_for_launch` 决策。
    pub is_default: bool,
    #[serde(default)]
    pub major: u32,
    #[serde(default)]
    pub bits: u8,
    #[serde(default)]
    pub architecture: String,
    #[serde(default)]
    pub executable: bool,
}

const EMPTY_JSON_ARRAY: &str = "[]";

#[cfg(windows)]
const JAVA_EXE: &str = "java.exe";
#[cfg(not(windows))]
const JAVA_EXE: &str = "java";

/// 从 `java -version` 输出里取版本号（`openjdk version "21.0.1" ...` / `java version "1.8.0_392"`）。
fn parse_java_version(text: &str) -> Option<String> {
    const MARKER: &str = "version \"";
    let start = text.find(MARKER)? + MARKER.len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    let version = rest[..end].trim();
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

/// 从完整版本字符串取主版本；Java 8 的 `1.8.0_xxx` 归一为 8。
fn parse_java_major(version: &str) -> u32 {
    let normalized = version.trim().strip_prefix("1.").unwrap_or(version.trim());
    normalized
        .split(['.', '_', '-', '+'])
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
}

fn java_property<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix(key)?
            .trim_start()
            .strip_prefix('=')?
            .trim()
            .split_whitespace()
            .next()
    })
}

fn parse_java_bits(text: &str) -> u8 {
    if let Some(model) = java_property(text, "sun.arch.data.model") {
        if model.contains("64") {
            return 64;
        }
        if model == "32" {
            return 32;
        }
    }
    match java_property(text, "os.arch")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "amd64" | "x86_64" | "x64" | "aarch64" | "arm64" | "ppc64le" => 64,
        "x86" | "i386" | "i486" | "i586" | "i686" | "arm" => 32,
        _ => 0,
    }
}

fn parse_java_architecture(text: &str) -> String {
    java_property(text, "os.arch")
        .or_else(|| java_property(text, "sun.arch.data.model"))
        .unwrap_or("unknown")
        .to_string()
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// 从 `java -version` 输出里识别厂商（顺序敏感：`Temurin` 的输出里同时含 `OpenJDK`）。
fn detect_java_vendor(text: &str) -> String {
    const MARKERS: [(&str, &str); 9] = [
        ("Temurin", "Eclipse Temurin"),
        ("AdoptOpenJDK", "AdoptOpenJDK"),
        ("Microsoft", "Microsoft OpenJDK"),
        ("Zulu", "Azul Zulu"),
        ("Corretto", "Amazon Corretto"),
        ("Semeru", "IBM Semeru"),
        ("GraalVM", "GraalVM"),
        ("OpenJ9", "Eclipse OpenJ9"),
        ("OpenJDK", "OpenJDK"),
    ];
    for (needle, name) in MARKERS {
        if text.contains(needle) {
            return name.to_string();
        }
    }
    VENDOR_UNKNOWN.to_string()
}

/// 跑一次 Java 设置探测，读取版本、厂商、主版本与位数；失败只标记 unverified。
fn probe_java(path: &Path) -> JavaInstallation {
    let mut command = std::process::Command::new(path);
    command
        .arg("-XshowSettings:properties")
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// `CREATE_NO_WINDOW`：探测时不弹出控制台窗口。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let (version, vendor, major, bits, architecture) = match command.output() {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push('\n');
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            match parse_java_version(&text) {
                Some(version) => {
                    let major = parse_java_major(&version);
                    let bits = parse_java_bits(&text);
                    let architecture = parse_java_architecture(&text);
                    (version, detect_java_vendor(&text), major, bits, architecture)
                }
                None => (
                    String::new(),
                    VENDOR_UNVERIFIED.to_string(),
                    0,
                    0,
                    "unknown".into(),
                ),
            }
        }
        Err(_) => (
            String::new(),
            VENDOR_UNVERIFIED.to_string(),
            0,
            0,
            "unknown".into(),
        ),
    };

    JavaInstallation {
        path: path.to_string_lossy().into_owned(),
        version,
        vendor,
        is_default: false,
        major,
        bits,
        architecture,
        executable: executable_file(path),
    }
}

/// Windows 路径大小写不敏感，去重时按平台规则比较。
fn path_eq(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

fn push_unique(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if candidates.iter().any(|p| path_eq(p, &path)) {
        return;
    }
    candidates.push(path);
}

/// `JAVA_HOME` 之类的目录 → `<dir>/bin/java[.exe]`（含 macOS 的 `Contents/Home` 布局）。
fn push_java_in(candidates: &mut Vec<PathBuf>, home: &Path) {
    let bins = [
        home.join("bin"),
        home.join("Contents").join("Home").join("bin"),
    ];
    for bin in bins {
        let exe = bin.join(JAVA_EXE);
        if exe.is_file() {
            push_unique(candidates, exe);
        }
    }
}

fn read_subdirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// 递归收集 `<root>/**/bin/java[.exe]`（用于启动器下载的内置运行时），深度受限。
fn collect_java_bins(root: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    for dir in read_subdirs(root) {
        let exe = dir.join("bin").join(JAVA_EXE);
        if exe.is_file() {
            push_unique(out, exe);
        }
        collect_java_bins(&dir, depth - 1, out);
    }
}

/// 各平台常见的 JDK / JRE 安装根目录（只保留真实存在的）。
fn java_search_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    #[cfg(windows)]
    {
        const VENDOR_DIRS: [&str; 9] = [
            "Java",
            "Eclipse Adoptium",
            "Eclipse Foundation",
            "Microsoft",
            "Zulu",
            "Amazon Corretto",
            "BellSoft",
            "Semeru",
            "JetBrains",
        ];
        for key in [
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
            "LOCALAPPDATA",
        ] {
            let Some(base) = std::env::var_os(key).map(PathBuf::from) else {
                continue;
            };
            for vendor in VENDOR_DIRS {
                roots.push(base.join(vendor));
            }
            // `%LOCALAPPDATA%` 下的安装通常位于 `Programs/` 子目录。
            for vendor in [
                "Programs/Eclipse Adoptium",
                "Programs/Microsoft",
                "Programs/Zulu",
            ] {
                roots.push(base.join(vendor));
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        roots.push(PathBuf::from("/Library/Java/JavaVirtualMachines"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for dir in ["/usr/lib/jvm", "/usr/java", "/opt/java"] {
            roots.push(PathBuf::from(dir));
        }
    }

    let mut existing: Vec<PathBuf> = Vec::new();
    for root in roots {
        if root.is_dir() {
            push_unique(&mut existing, root);
        }
    }
    existing
}

/// 启动器 / 官方启动器可能存放内置运行时的 `.minecraft` 目录。
fn runtime_roots(app: &AppHandle) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    // 实例注册表里的版本根目录（active 优先）：启动器把 JRE 下载到 `<root>/runtime`。
    if let Ok(path) = store::instances_path(app) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let registry = instances::registry_from_text(&text);
            if let Some(active) = registry.active.as_deref() {
                roots.push(PathBuf::from(active).join("runtime"));
            }
            for root in registry.roots {
                roots.push(PathBuf::from(root).join("runtime"));
            }
        }
    }

    // 便携式默认位置：可执行文件同级的 `.minecraft/runtime`。
    if let Ok(directory) = instances::launcher_self_dir() {
        roots.push(
            PathBuf::from(directory)
                .join(MINECRAFT_DIR_NAME)
                .join("runtime"),
        );
    }

    // 官方启动器的默认位置（Windows 用 `%APPDATA%`，其它平台用家目录）。
    if let Some(dir) = official_minecraft_dir() {
        roots.push(dir.join("runtime"));
    }

    let mut existing: Vec<PathBuf> = Vec::new();
    for root in roots {
        if root.is_dir() {
            push_unique(&mut existing, root);
        }
    }
    existing
}

/// 官方启动器的默认 `.minecraft` 目录。
fn official_minecraft_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
            return Some(appdata.join(MINECRAFT_DIR_NAME));
        }
    }
    instances::user_home_dir().map(|home| home.join(MINECRAFT_DIR_NAME))
}

/// 探测本机可用的 Java：内置运行时 → `JAVA_HOME` → 常见安装目录。
/// 探测顺序只影响列表，不设置默认项；实际选择由 `select_java_for_launch` 计算。
fn detect_java_installations(app: &AppHandle) -> Vec<JavaInstallation> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    for root in runtime_roots(app) {
        collect_java_bins(&root, RUNTIME_SCAN_DEPTH, &mut candidates);
    }

    if let Some(home) = std::env::var_os("JAVA_HOME").map(PathBuf::from) {
        push_java_in(&mut candidates, &home);
    }

    for root in java_search_roots() {
        for child in read_subdirs(&root) {
            push_java_in(&mut candidates, &child);
        }
    }

    candidates
        .into_iter()
        .take(MAX_PROBED_JAVA)
        .map(|path| probe_java(&path))
        .collect()
}

/// 探测本机可用 Java，返回 JSON 数组文本。
#[tauri::command]
pub async fn java_detect(app: AppHandle) -> String {
    // 探测会启动若干子进程，放到阻塞线程池，避免占用异步运行时。
    let list = tauri::async_runtime::spawn_blocking(move || detect_java_installations(&app))
        .await
        .unwrap_or_default();
    serde_json::to_string(&list).unwrap_or_else(|_| EMPTY_JSON_ARRAY.to_string())
}

/// 供 runtime_settings / 启动编排调用的公开策略入口。
#[allow(dead_code)]
pub(crate) fn java_requirement_for(
    mc_version: &str,
    loader: Option<&str>,
    loader_version: Option<&str>,
    release_time: Option<&str>,
) -> java_selection::JavaRequirement {
    java_selection::requirement_for(mc_version, loader, loader_version, release_time)
}

#[allow(dead_code)]
fn java_candidate(installation: &JavaInstallation) -> java_selection::JavaCandidate {
    java_selection::JavaCandidate {
        path: installation.path.clone(),
        version: installation.version.clone(),
        vendor: installation.vendor.clone(),
        major: installation.major,
        bits: installation.bits,
        executable: installation.executable,
    }
}

/// 计算实际会使用的 Java；手动路径在返回前完成路径、可执行、主版本、位数与兼容性检查。
#[allow(dead_code)]
pub(crate) fn select_java_for_launch(
    mc_version: &str,
    loader: Option<&str>,
    loader_version: Option<&str>,
    release_time: Option<&str>,
    installations: &[JavaInstallation],
    manual_path: Option<&str>,
) -> Result<java_selection::JavaSelection, String> {
    let requirement =
        java_selection::requirement_for(mc_version, loader, loader_version, release_time);
    if let Some(raw) = manual_path.map(str::trim).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(raw);
        if !path.is_absolute() {
            return Err("所选 Java 必须使用绝对路径".into());
        }
        if !path.is_file() {
            return Err("所选 Java 已不存在，请重新扫描或选择 Java 可执行文件".into());
        }
        if !executable_file(&path) {
            return Err("所选 Java 没有可执行权限，请选择 java 可执行文件".into());
        }
        let probe = probe_java(&path);
        if probe.major == 0 {
            return Err("无法读取所选 Java 的主版本，请重新选择".into());
        }
        return java_selection::validate_manual(&requirement, &java_candidate(&probe));
    }
    let candidates = installations
        .iter()
        .map(java_candidate)
        .collect::<Vec<_>>();
    Ok(java_selection::select(requirement, &candidates))
}

/// 前端预览与启动编排共用的 Java 解析命令。注册由 runtime-core-3 / Lead 负责。
#[tauri::command]
pub async fn java_resolve(
    app: AppHandle,
    mc_version: String,
    loader: Option<String>,
    loader_version: Option<String>,
    release_time: Option<String>,
    java_path: Option<String>,
) -> Result<java_selection::JavaSelection, String> {
    let manual = java_path.filter(|value| !value.trim().is_empty());
    let installations = if manual.is_some() {
        Vec::new()
    } else {
        tauri::async_runtime::spawn_blocking(move || detect_java_installations(&app))
            .await
            .map_err(|error| format!("读取本机 Java 列表失败：{error}"))?
    };
    select_java_for_launch(
        &mc_version,
        loader.as_deref(),
        loader_version.as_deref(),
        release_time.as_deref(),
        &installations,
        manual.as_deref(),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// 8. 打开目录
// ─────────────────────────────────────────────────────────────────────────────

/// `which` → 目录名。`version` 指版本目录（`versions`）。
fn folder_for(which: &str) -> Option<&'static str> {
    match which {
        "version" => Some("versions"),
        "saves" => Some("saves"),
        "mods" => Some("mods"),
        "screenshots" => Some("screenshots"),
        "resourcepacks" => Some("resourcepacks"),
        "shaderpacks" => Some("shaderpacks"),
        _ => None,
    }
}

/// 会自动创建的目录类型（游戏正常运行时也会建这些目录）。
const AUTO_CREATE_FOLDERS: [&str; 4] = ["version", "saves", "mods", "screenshots"];

/// 用系统默认方式打开目录。
///
/// `explorer` 即使成功也可能返回非 0 退出码，因此只 `spawn` 不等待、也不检查状态。
fn open_with_system(path: &Path) -> Result<(), String> {
    // `explorer` 不接受 `/` 作为分隔符：规范化成平台原生分隔符。
    let native: PathBuf = path.components().collect();

    let mut command = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("explorer");
        c.arg(&native);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(&native);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(&native);
        c
    };

    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开目录失败：{e}"))
}

fn folder_target_with_resource<F>(
    dir: &str,
    which: &str,
    version_id: Option<&str>,
    resolve_resource: F,
) -> Result<PathBuf, String>
where
    F: FnOnce(&Path, &str) -> Result<PathBuf, String>,
{
    let folder = folder_for(which).ok_or_else(|| {
        format!("不支持的目录类型：{which}（可选 version / saves / mods / screenshots / resourcepacks / shaderpacks）")
    })?;
    let root = dir.trim();
    if root.is_empty() {
        return Err("缺少游戏目录".to_string());
    }
    let root = PathBuf::from(root);
    if let Some(id) = version_id {
        crate::named_profiles::validate_name(id)?;
        let instance = root.join("versions").join(id);
        if !instance.is_dir() {
            return Err("当前实例已不存在，请返回版本列表重新选择".into());
        }
        if which == "version" {
            return Ok(instance);
        }
        let resource_root = resolve_resource(&root, id)?;
        if which == "mods" {
            return crate::resources::instance_paths::for_instance(&root, id, &resource_root);
        }
        return Ok(resource_root.join(folder));
    }
    Ok(root.join(folder))
}

/// 资源目录与 Mod 列表、安装、导出统一走 `runtime_settings::resource_directory`；
/// “版本文件夹”仍固定返回 `root/versions/<versionId>`。
#[tauri::command]
pub fn open_folder(
    app: AppHandle,
    dir: String,
    which: String,
    version_id: Option<String>,
) -> Result<(), String> {
    let which = which.trim().to_ascii_lowercase();
    let target = folder_target_with_resource(
        &dir,
        &which,
        version_id.as_deref(),
        |root, id| crate::runtime_settings::resource_directory(&app, root, id),
    )?;
    if !target.is_dir() {
        if AUTO_CREATE_FOLDERS.contains(&which.as_str()) {
            std::fs::create_dir_all(&target)
                .map_err(|e| format!("创建目录失败 {}：{e}", target.display()))?;
        } else {
            return Err(format!("目录不存在：{}", target.display()));
        }
    }
    open_with_system(&target)
}

// ─────────────────────────────────────────────────────────────────────────────
// 9. 单元测试（纯逻辑 / 本地，不联网）
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 对实际生产 payload 做回归，避免只测试已弃用的事件副本。
    #[test]
    fn install_event_wire_contract() {
        let cases: Vec<(InstallEvent, &str, &[&str])> = vec![
            (
                InstallEvent::Stage {
                    task_id: "t1".into(),
                    stage: STAGE_VANILLA.into(),
                    detail: "Minecraft 1.21.4".into(),
                },
                "stage",
                &["taskId", "stage", "detail"],
            ),
            (
                progress_event("t1".into(), 120, 300, 1_048_576.0, 12_345),
                "progress",
                &[
                    "taskId",
                    "percent",
                    "filesDone",
                    "filesTotal",
                    "speedBps",
                    "remainingMs",
                ],
            ),
            (
                InstallEvent::File {
                    task_id: "t1".into(),
                    name: "minecraft.jar".into(),
                    percent: 0.7,
                    done: false,
                },
                "file",
                &["taskId", "name", "percent", "done"],
            ),
            (
                InstallEvent::Done {
                    task_id: "t1".into(),
                    ok: false,
                    message: CANCEL_MESSAGE.into(),
                    cancelled: true,
                    result: None,
                },
                "done",
                &["taskId", "ok", "message", "cancelled", "result"],
            ),
        ];
        for (event, tag, fields) in cases {
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(value["type"], tag);
            let object = value.as_object().unwrap();
            assert_eq!(object.len(), fields.len() + 1);
            for field in fields {
                assert!(object.contains_key(*field), "缺少字段 {field}: {value}");
            }
            assert!(object.keys().all(|key| !key.contains('_')));
        }
        assert_eq!(
            serde_json::to_value(progress_event("t1".into(), 300, 100, 0.0, 0)).unwrap()["percent"],
            1.0
        );
    }

    #[test]
    fn file_percent_is_clamped() {
        assert_eq!(file_percent(0, 0, false), 0.0);
        assert_eq!(file_percent(0, 0, true), 1.0);
        assert_eq!(file_percent(70, 100, false), 0.7);
        assert_eq!(file_percent(300, 100, false), 1.0);
    }

    // ── 版本清单 ──────────────────────────────────────────────────────────

    const MANIFEST_SAMPLE: &str = r#"{
      "latest": { "release": "1.21.4", "snapshot": "25w03a" },
      "versions": [
        {
          "id": "1.21.4",
          "type": "release",
          "releaseTime": "2024-12-03T10:00:00+00:00",
          "url": "https://piston-meta.mojang.com/v1/packages/aa/1.21.4.json"
        },
        {
          "id": "25w03a",
          "type": "snapshot",
          "releaseTime": "2025-01-15T10:00:00+00:00",
          "url": "https://piston-meta.mojang.com/v1/packages/bb/25w03a.json"
        }
      ]
    }"#;

    #[test]
    fn normalizes_version_manifest() {
        let value: serde_json::Value = serde_json::from_str(MANIFEST_SAMPLE).unwrap();
        let text = normalize_manifest(&value).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();

        assert_eq!(parsed["latest"]["release"], "1.21.4");
        assert_eq!(parsed["latest"]["snapshot"], "25w03a");
        assert_eq!(parsed["versions"][0]["id"], "1.21.4");
        assert_eq!(parsed["versions"][0]["type"], "release");
        assert_eq!(
            parsed["versions"][0]["releaseTime"],
            "2024-12-03T10:00:00+00:00"
        );
        assert!(
            parsed["versions"][0]["url"]
                .as_str()
                .unwrap()
                .ends_with("1.21.4.json")
        );
        assert_eq!(parsed["versions"][1]["type"], "snapshot");
        // 归一化输出不得出现 snake_case 字段。
        assert!(!text.contains("release_time"));
    }

    #[test]
    fn normalize_manifest_is_lenient_but_rejects_broken_shapes() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{"versions":[
                {"id":"1.20.1"},
                {"id":"   "},
                {"no":"id"},
                {"id":"1.19.2","type":"old_beta","time":"2012-01-01T00:00:00+00:00"}
            ]}"#,
        )
        .unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&normalize_manifest(&value).unwrap()).unwrap();

        // 缺 latest 段 → 空串（不报错）
        assert_eq!(parsed["latest"]["release"], "");
        assert_eq!(parsed["latest"]["snapshot"], "");

        let versions = parsed["versions"].as_array().unwrap();
        assert_eq!(versions.len(), 2, "空 id / 无 id 的条目必须被丢弃");
        assert_eq!(versions[0]["type"], DEFAULT_VERSION_TYPE);
        assert_eq!(versions[0]["url"], "");
        assert_eq!(
            versions[1]["releaseTime"], "2012-01-01T00:00:00+00:00",
            "releaseTime 缺失时应回退 time"
        );

        // 无法归一化的形态 → Err（可读中文，不含英文字段名之外的噪声）
        for bad in [
            "[]",
            "null",
            r#"{"versions":{}}"#,
            r#"{"versions":[]}"#,
            "{}",
        ] {
            let value: serde_json::Value = serde_json::from_str(bad).unwrap();
            let err = normalize_manifest(&value).unwrap_err();
            assert!(!err.is_empty(), "必须给出错误原因: {bad}");
            assert!(!err.is_ascii(), "错误信息应为可读中文: {err}");
        }
    }

    // ── 加载器白名单 ──────────────────────────────────────────────────────

    #[test]
    fn loader_whitelist_is_enforced() {
        assert_eq!(parse_loader(None).unwrap(), None);
        assert_eq!(parse_loader(Some("")).unwrap(), None);
        assert_eq!(parse_loader(Some("  vanilla ")).unwrap(), None);
        assert_eq!(parse_loader(Some("none")).unwrap(), None);
        assert_eq!(
            parse_loader(Some("Forge")).unwrap(),
            Some(LoaderType::Forge)
        );
        assert_eq!(
            parse_loader(Some("neoforge")).unwrap(),
            Some(LoaderType::NeoForge)
        );
        assert_eq!(
            parse_loader(Some("fabric")).unwrap(),
            Some(LoaderType::Fabric)
        );
        assert_eq!(
            parse_loader(Some("quilt")).unwrap(),
            Some(LoaderType::Quilt)
        );

        assert_eq!(
            parse_loader(Some("optifine")).unwrap(),
            Some(LoaderType::OptiFine)
        );
        assert_eq!(
            parse_loader(Some("LiteLoader")).unwrap(),
            Some(LoaderType::LiteLoader)
        );
    }

    #[test]
    fn liteloader_finalizer_uses_shared_artifact_and_is_repeatable() {
        let entries = loader_catalog::liteloader_install::parse_versions(
            r#"{"versions":{"1.12.2":{"artefacts":{"com.mumfrey:liteloader":{"latest":{"version":"1.12.2-SNAPSHOT-r0","tweakClass":"com.mumfrey.liteloader.launch.LiteLoaderTweaker","stream":"RELEASE","libraries":[]}}}}}}"#,
        )
        .unwrap();
        let artifact = loader_catalog::liteloader_install::artifact(&entries[0]);
        let root = std::env::temp_dir().join(format!(
            "nowhere-liteloader-finalizer-{}",
            store::random_token()
        ));
        std::fs::create_dir_all(&root).unwrap();
        finalize_loader_files(
            &root,
            &root,
            Some(&LoaderType::LiteLoader),
            Some(&artifact),
        )
        .unwrap();
        finalize_loader_files(
            &root,
            &root,
            Some(&LoaderType::LiteLoader),
            Some(&artifact),
        )
        .unwrap();
        assert!(root
            .join("versions/1.12.2-LiteLoader/1.12.2-LiteLoader.json")
            .is_file());
        assert!(root.join("mods/1.12.2").is_dir());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn liteloader_finalizer_uses_effective_resource_root_for_non_isolated_instance() {
        let entries = loader_catalog::liteloader_install::parse_versions(
            r#"{"versions":{"1.12.2":{"artefacts":{"com.mumfrey:liteloader":{"latest":{"version":"1.12.2-SNAPSHOT-r0","tweakClass":"com.mumfrey.liteloader.launch.LiteLoaderTweaker","stream":"RELEASE","libraries":[]}}}}}}"#,
        )
        .unwrap();
        let artifact = loader_catalog::liteloader_install::artifact(&entries[0]);
        let base = std::env::temp_dir().join(format!(
            "nowhere-liteloader-non-isolated-{}",
            store::random_token()
        ));
        let game_dir = base.join("versions/Lite");
        std::fs::create_dir_all(&game_dir).unwrap();
        finalize_loader_files(
            &game_dir,
            &base,
            Some(&LoaderType::LiteLoader),
            Some(&artifact),
        )
        .unwrap();
        assert!(base
            .join("versions/1.12.2-LiteLoader/1.12.2-LiteLoader.json")
            .is_file());
        assert!(base.join("mods/1.12.2").is_dir());
        assert!(!game_dir
            .join("versions/1.12.2-LiteLoader/1.12.2-LiteLoader.json")
            .exists());
        assert!(!game_dir.join("mods/1.12.2").exists());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn optifine_launchwrapper_finalizer_writes_profile_and_mods() {
        let entries = loader_catalog::optifine_install::parse_index(
            r#"[{"mcversion":"1.12.2","type":"HD_U","patch":"G5","filename":"OptiFine_1.12.2_HD_U_G5.jar","forge":"N/A"}]"#,
        )
        .unwrap();
        let artifact = loader_catalog::optifine_install::artifact(&entries[0]);
        let root = std::env::temp_dir().join(format!(
            "nowhere-optifine-launchwrapper-{}",
            store::random_token()
        ));
        std::fs::create_dir_all(&root).unwrap();
        finalize_loader_files(
            &root,
            &root,
            Some(&LoaderType::OptiFine),
            Some(&artifact),
        )
        .unwrap();
        let path = root.join(
            "loader/optifine/versions/1.12.2-OptiFine_1.12.2_HD_U_G5/1.12.2-OptiFine_1.12.2_HD_U_G5.json",
        );
        let profile: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(profile["mainClass"], "net.minecraft.launchwrapper.Launch");
        assert_eq!(profile["arguments"]["game"][1], "optifine.OptiFineTweaker");
        assert!(root.join("mods").is_dir());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn optifine_installer_finalizer_requires_generated_profile() {
        let entries = loader_catalog::optifine_install::parse_index(
            r#"[{"mcversion":"1.20.1","type":"HD_U","patch":"I6","filename":"OptiFine_1.20.1_HD_U_I6.jar","forge":"N/A"}]"#,
        )
        .unwrap();
        let artifact = loader_catalog::optifine_install::artifact(&entries[0]);
        let root = std::env::temp_dir().join(format!(
            "nowhere-optifine-installer-{}",
            store::random_token()
        ));
        std::fs::create_dir_all(&root).unwrap();
        assert!(
            finalize_loader_files(
                &root,
                &root,
                Some(&LoaderType::OptiFine),
                Some(&artifact),
            )
            .is_err()
        );
        let generated = root.join("loader/optifine/versions/1.20.1-OptiFine_HD_U_I6");
        std::fs::create_dir_all(&generated).unwrap();
        std::fs::write(
            generated.join("1.20.1-OptiFine_HD_U_I6.json"),
            serde_json::to_vec(&serde_json::json!({
                "id": "1.20.1-OptiFine_HD_U_I6",
                "libraries": [{"name": "optifine:OptiFine:1.20.1_HD_U_I6"}]
            }))
            .unwrap(),
        )
        .unwrap();
        finalize_loader_files(
            &root,
            &root,
            Some(&LoaderType::OptiFine),
            Some(&artifact),
        )
        .unwrap();
        assert!(root.join("mods").is_dir());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pack_install_resolves_artifact_only_for_loader_paths_that_need_it() {
        for loader in [
            LoaderType::Forge,
            LoaderType::NeoForge,
            LoaderType::OptiFine,
            LoaderType::LiteLoader,
        ] {
            assert!(pack_loader_requires_artifact(&loader), "{loader}");
        }
        for loader in [LoaderType::Fabric, LoaderType::Quilt] {
            assert!(!pack_loader_requires_artifact(&loader), "{loader}");
        }
    }

    #[test]
    fn loader_config_matches_requested_build() {
        let config = loader_config(Some(LoaderType::Fabric), "0.16.9", None);
        assert_eq!(config.loader_type, Some(LoaderType::Fabric));
        assert_eq!(config.build, "0.16.9");
        assert!(config.enable, "指定加载器时必须启用安装");
        assert_eq!(config.path, None);

        let vanilla = loader_config(None, "latest", None);
        assert_eq!(vanilla.loader_type, None);
        assert!(!vanilla.enable, "原版安装不得启用加载器");

        assert_eq!(
            loader_label(&Some(LoaderType::NeoForge), "21.4.100"),
            "neoforge 21.4.100"
        );
        assert_eq!(loader_label(&None, "latest"), "");
    }

    #[test]
    fn cancelled_tasks_protect_directory_until_writer_finishes() {
        let root = std::env::temp_dir().join(format!("nowhere-task-{}", store::random_token()));
        let (_, directory) =
            named_profiles::reserve(&root, "Pending world", "1.20.1", None, "").unwrap();
        let guard = app_tasks::test_reserve(&directory);
        assert!(
            crate::version_settings::version_delete(
                root.to_string_lossy().into(),
                "Pending world".into()
            )
            .is_err()
        );
        assert!(directory.is_dir());
        drop(guard);
        crate::version_settings::version_delete(
            root.to_string_lossy().into(),
            "Pending world".into(),
        )
        .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    // ── 错误文案 ──────────────────────────────────────────────────────────

    #[test]
    fn brief_truncates_long_messages() {
        let long = "x".repeat(500);
        let short = brief(&long, 10);
        assert_eq!(short.chars().count(), 11, "超长要截断并加省略号");
        assert!(short.ends_with('…'));
        assert_eq!(brief("短", 10), "短");
        assert_eq!(brief(&"多".repeat(4), 4), "多多多多");
    }

    #[test]
    fn install_errors_are_described_in_chinese() {
        let cases = vec![
            LaunchError::NoInternetNoCache,
            LaunchError::VersionNotFound("1.21.9".into()),
            LaunchError::Download(DownloadError::Timeout),
            LaunchError::Download(DownloadError::ChecksumMismatch {
                file: "client.jar".into(),
                expected: "a".into(),
                actual: "b".into(),
            }),
            LaunchError::Download(DownloadError::Connection {
                url: "https://piston-data.mojang.com/x".into(),
                detail: "connection timed out".into(),
            }),
            LaunchError::Loader(LoaderError::ProfileNotFound),
            LaunchError::Loader(LoaderError::ProcessorFailed {
                processor: "net.minecraftforge.Foo".into(),
                code: Some(1),
            }),
            LaunchError::Loader(LoaderError::VersionNotFound("no builds".into())),
            LaunchError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied",
            )),
            LaunchError::InvalidData("x".repeat(400)),
        ];

        for err in cases {
            let text = describe_install_error(&err);
            assert!(!text.is_empty(), "必须给出原因");
            assert!(!text.is_ascii(), "安装错误提示应为可读中文: {text}");
            assert!(text.chars().count() <= 240, "提示不应过长: {text}");
        }

        assert!(
            describe_install_error(&LaunchError::VersionNotFound("1.21.9".into()))
                .contains("1.21.9")
        );
        assert!(
            describe_install_error(&LaunchError::Loader(LoaderError::ProcessorFailed {
                processor: "net.minecraftforge.Foo".into(),
                code: Some(1),
            }))
            .contains("net.minecraftforge.Foo")
        );
    }

    // ── Java 探测 ─────────────────────────────────────────────────────────

    #[test]
    fn parses_java_version_and_vendor() {
        let temurin = "openjdk version \"21.0.1\" 2023-10-17 LTS\n\
                      OpenJDK Runtime Environment Temurin-21.0.1+12 (build 21.0.1+12-LTS)\n\
                      OpenJDK 64-Bit Server VM Temurin-21.0.1+12 (build 21.0.1+12-LTS, mixed mode)";
        assert_eq!(parse_java_version(temurin).as_deref(), Some("21.0.1"));
        assert_eq!(parse_java_major("21.0.1"), 21);
        assert_eq!(detect_java_vendor(temurin), "Eclipse Temurin");
        let props = "    sun.arch.data.model = 64\n    os.arch = amd64";
        assert_eq!(parse_java_bits(props), 64);
        assert_eq!(parse_java_architecture(props), "amd64");

        let legacy = "java version \"1.8.0_392\"\n\
                      Java(TM) SE Runtime Environment (build 1.8.0_392-b08)";
        assert_eq!(parse_java_version(legacy).as_deref(), Some("1.8.0_392"));
        assert_eq!(parse_java_major("1.8.0_392"), 8);
        assert_eq!(detect_java_vendor(legacy), VENDOR_UNKNOWN);

        let corretto = "openjdk version \"17.0.9\" 2023-10-17 LTS\nOpenJDK Runtime Environment Corretto-17.0.9.8.1";
        assert_eq!(detect_java_vendor(corretto), "Amazon Corretto");

        // 认不出来 / 无法解析 → unverified（不 panic）
        assert_eq!(parse_java_version("Error: could not find java.dll"), None);
        assert_eq!(parse_java_version("version \"\""), None);
        assert_eq!(detect_java_vendor(""), VENDOR_UNKNOWN);
    }

    #[test]
    fn probe_reports_unverified_for_missing_binary() {
        let missing =
            std::env::temp_dir().join(format!("nowhere-java-missing-{}", store::random_token()));
        let probe = probe_java(&missing);
        assert!(probe.version.is_empty());
        assert_eq!(probe.vendor, VENDOR_UNVERIFIED);
        assert!(!probe.is_default);
        assert_eq!(probe.path, missing.to_string_lossy());
        assert_eq!(probe.major, 0);
        assert_eq!(probe.bits, 0);
        assert!(!probe.executable);
    }

    fn java_installation(path: &str, major: u32) -> JavaInstallation {
        JavaInstallation {
            path: path.into(),
            version: if major == 8 {
                "1.8.0_392".into()
            } else {
                format!("{major}.0.1")
            },
            vendor: "OpenJDK".into(),
            is_default: false,
            major,
            bits: 64,
            architecture: "amd64".into(),
            executable: true,
        }
    }

    #[test]
    fn actual_java_selection_does_not_use_first_list_item() {
        let installations = [
            java_installation("C:/java8/bin/java.exe", 8),
            java_installation("C:/java21/bin/java.exe", 21),
        ];
        let selected = select_java_for_launch(
            "1.20.5",
            None,
            None,
            None,
            &installations,
            None,
        )
        .unwrap();
        assert_eq!(selected.path.as_deref(), Some("C:/java21/bin/java.exe"));
        assert_eq!(selected.source, "auto");
        assert!(!selected.needs_download);

        let unavailable = select_java_for_launch(
            "1.20.5",
            None,
            None,
            None,
            &installations[..1],
            None,
        )
        .unwrap();
        assert!(!unavailable.compatible);
        assert!(unavailable.needs_download);
        assert_eq!(unavailable.source, "unavailable");
    }


    // ── 打开目录 ──────────────────────────────────────────────────────────

    #[test]
    fn folder_mapping_and_open_errors() {
        assert_eq!(folder_for("version"), Some("versions"));
        assert_eq!(folder_for("mods"), Some("mods"));
        assert_eq!(folder_for("saves"), Some("saves"));
        assert_eq!(folder_for("screenshots"), Some("screenshots"));
        assert_eq!(folder_for("resourcepacks"), Some("resourcepacks"));
        assert_eq!(folder_for("shaderpacks"), Some("shaderpacks"));
        assert_eq!(folder_for("nope"), None);

        // 空目录 → 可读错误（不打开任何东西）
        let err = folder_target_with_resource(String::new().as_str(), "mods", None, |_, _| {
            unreachable!()
        })
        .unwrap_err();
        assert!(err.contains("游戏目录"), "{err}");

        // 未知 which → 可读错误
        let err = folder_target_with_resource(
            "C:/nowhere-does-not-exist",
            "NOPE",
            None,
            |_, _| unreachable!(),
        )
        .unwrap_err();
        assert!(err.contains("不支持的目录类型"), "{err}");
        assert!(err.contains("NOPE"), "错误信息应包含原始取值: {err}");
    }

    #[test]
    fn shortcuts_use_effective_resource_root_for_all_isolation_combinations() {
        let root =
            std::env::temp_dir().join(format!("nowhere-shortcuts-{}", store::random_token()));
        let (_, named) =
            crate::named_profiles::reserve(&root, "My world", "1.21.4", None, "").unwrap();
        let legacy = root.join("versions/1.20.1");
        std::fs::create_dir_all(&legacy).unwrap();
        let dir = root.to_string_lossy();

        for (id, instance) in [("My world", &named), ("1.20.1", &legacy)] {
            assert_eq!(
                folder_target_with_resource(&dir, "version", Some(id), |_, _| {
                    unreachable!()
                })
                .unwrap(),
                *instance
            );
            for isolation in [true, false] {
                let expected = crate::runtime_settings::game_directory(&root, id, isolation)
                    .join("mods");
                assert_eq!(
                    folder_target_with_resource(&dir, "mods", Some(id), |root, version| {
                        Ok(crate::runtime_settings::game_directory(
                            root, version, isolation,
                        ))
                    })
                    .unwrap(),
                    expected,
                    "{id} isolation={isolation}"
                );
            }
        }

        assert_eq!(
            folder_target_with_resource(&dir, "version", None, |_, _| unreachable!()).unwrap(),
            root.join("versions")
        );
        assert!(folder_target_with_resource(&dir, "mods", Some("missing"), |root, id| Ok(
            crate::runtime_settings::game_directory(root, id, true)
        ))
        .is_err());
        assert!(folder_target_with_resource(&dir, "mods", Some("../escape"), |root, id| Ok(
            crate::runtime_settings::game_directory(root, id, true)
        ))
        .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn liteloader_mod_shortcut_uses_minecraft_specific_directory() {
        let root = std::env::temp_dir().join(format!(
            "nowhere-lit-shortcut-{}",
            store::random_token()
        ));
        let version = "Lite world";
        let directory = root.join("versions").join(version);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("nowhere.profile.json"),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "name": version,
                "minecraftVersion": "1.12.2",
                "loader": "LiteLoader",
                "loaderVersion": "1.12.2-SNAPSHOT-r0",
                "ready": true
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            directory.join(format!("{version}.json")),
            serde_json::to_vec(&serde_json::json!({
                "id": version,
                "inheritsFrom": "1.12.2",
                "nowhereProfile": {
                    "minecraftVersion": "1.12.2",
                    "loader": "LiteLoader",
                    "loaderVersion": "1.12.2-SNAPSHOT-r0"
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let expected = directory.join("mods/1.12.2");
        assert_eq!(
            folder_target_with_resource(
                root.to_string_lossy().as_ref(),
                "mods",
                Some(version),
                |root, id| Ok(crate::runtime_settings::game_directory(root, id, true)),
            )
            .unwrap(),
            expected
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_path_uses_platform_separator() {
        // `explorer` 不接受 `/` 分隔符：components 归一化后必须是平台原生分隔符。
        let native: PathBuf = PathBuf::from("C:/mc/mods").components().collect();
        let text = native.to_string_lossy();
        if cfg!(windows) {
            assert!(!text.contains('/'), "{text}");
            assert!(text.contains('\\'), "{text}");
        } else {
            assert_eq!(text, "C:/mc/mods");
        }
    }
}

#[cfg(all(test, windows))]
#[path = "install_live_tests.rs"]
mod live_tests;
