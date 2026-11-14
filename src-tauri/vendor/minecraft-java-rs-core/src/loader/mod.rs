pub mod fabric;
pub mod forge;
pub mod forge_patcher;
pub mod neoforge;
pub mod quilt;
pub mod types;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::Sender;

use crate::error::LoaderError;
use crate::launcher::events::LaunchEvent;
use crate::launcher::options::LaunchOptions;
use crate::models::loader::{
    LoaderArtifactKind, LoaderLibrary, LoaderType, ResolvedLoaderArtifact,
};
use crate::models::minecraft::AssetItem;
use crate::net::downloader::{DownloadItem, Downloader};
use crate::net::transfer::TransferContext;
use crate::utils::archive::{ArchiveQueryResult, get_file_from_archive};
use crate::utils::paths::get_path_libraries;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Persist a loader's version JSON to `<loader_dir>/versions/<id>/<id>.json`.
/// Used by Fabric/Quilt where we fetch the profile JSON ourselves; Forge and
/// NeoForge let the installer write its own version JSON.
async fn save_loader_version(
    loader_dir: &std::path::Path,
    id: &str,
    json: &impl serde::Serialize,
) -> Result<(), LoaderError> {
    let path = loader_dir
        .join("versions")
        .join(id)
        .join(format!("{id}.json"));
    let bytes = serde_json::to_vec_pretty(json)?;
    write_bytes_atomic(&path, &bytes).await
}

use self::fabric::{FabricMC, FabricVariant};
use self::forge::ForgeMC;
use self::neoforge::NeoForgeMC;
use self::quilt::QuiltMC;
use self::types::{LoaderInstallInput, LoaderResult};

// ── Trait ─────────────────────────────────────────────────────────────────────

#[async_trait]
pub trait ModLoader: Send + Sync {
    async fn install(
        &self,
        options: &LaunchOptions,
        input: &LoaderInstallInput,
        client: &reqwest::Client,
        event_tx: &Sender<LaunchEvent>,
    ) -> Result<LoaderResult, LoaderError>;
}

// ── Dispatcher ────────────────────────────────────────────────────────────────

pub fn create_loader(loader_type: LoaderType) -> Box<dyn ModLoader> {
    match loader_type {
        LoaderType::Forge => Box::new(ForgeMC::new()),
        LoaderType::NeoForge => Box::new(NeoForgeMC::new()),
        LoaderType::Fabric => Box::new(FabricMC::new(FabricVariant::Modern)),
        LoaderType::LegacyFabric => Box::new(FabricMC::new(FabricVariant::Legacy)),
        LoaderType::Quilt => Box::new(QuiltMC::new()),
        LoaderType::OptiFine => Box::new(OptiFineMC),
        LoaderType::LiteLoader => Box::new(LiteLoaderMC),
    }
}

// ── ModLoader impls ───────────────────────────────────────────────────────────

#[async_trait]
impl ModLoader for FabricMC {
    async fn install(
        &self,
        options: &LaunchOptions,
        input: &LoaderInstallInput,
        client: &reqwest::Client,
        event_tx: &Sender<LaunchEvent>,
    ) -> Result<LoaderResult, LoaderError> {
        let loader_name = match self.loader_type() {
            LoaderType::LegacyFabric => "legacyfabric",
            _ => "fabric",
        };
        let json = self
            .download_json(&input.mc_version, &options.loader.build, client)
            .await?;
        let libraries = self
            .download_libraries(options, &json, client, event_tx)
            .await?;
        save_loader_version(&options.loader_dir(loader_name), &json.id, &json).await?;
        let extra_game_args = json
            .minecraft_arguments
            .as_deref()
            .map(|s| s.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default();
        Ok(LoaderResult {
            libraries,
            main_class: json.main_class,
            loader_version: json.id,
            loader_type: self.loader_type(),
            extra_game_args,
            extra_jvm_args: vec![],
        })
    }
}

#[async_trait]
impl ModLoader for QuiltMC {
    async fn install(
        &self,
        options: &LaunchOptions,
        input: &LoaderInstallInput,
        client: &reqwest::Client,
        event_tx: &Sender<LaunchEvent>,
    ) -> Result<LoaderResult, LoaderError> {
        let json = self
            .download_json(&input.mc_version, &options.loader.build, client)
            .await?;
        let libraries = self
            .download_libraries(options, &json, client, event_tx)
            .await?;
        save_loader_version(&options.loader_dir("quilt"), &json.id, &json).await?;
        let extra_game_args = json
            .minecraft_arguments
            .as_deref()
            .map(|s| s.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default();
        Ok(LoaderResult {
            libraries,
            main_class: json.main_class,
            loader_version: json.id,
            loader_type: LoaderType::Quilt,
            extra_game_args,
            extra_jvm_args: vec![],
        })
    }
}

#[async_trait]
impl ModLoader for ForgeMC {
    async fn install(
        &self,
        options: &LaunchOptions,
        input: &LoaderInstallInput,
        client: &reqwest::Client,
        event_tx: &Sender<LaunchEvent>,
    ) -> Result<LoaderResult, LoaderError> {
        let (version_id, main_class, libraries, extra_game_args, extra_jvm_args) = self
            .install(
                options,
                &input.mc_version,
                &input.java_path,
                &input.mc_jar,
                &input.mc_json,
                &options.loader.build,
                client,
                event_tx,
            )
            .await?;

        Ok(LoaderResult {
            libraries,
            main_class,
            loader_version: version_id,
            loader_type: LoaderType::Forge,
            extra_game_args,
            extra_jvm_args,
        })
    }
}

#[async_trait]
impl ModLoader for NeoForgeMC {
    async fn install(
        &self,
        options: &LaunchOptions,
        input: &LoaderInstallInput,
        client: &reqwest::Client,
        event_tx: &Sender<LaunchEvent>,
    ) -> Result<LoaderResult, LoaderError> {
        let (version_id, main_class, libraries, extra_game_args, extra_jvm_args) = self
            .install(
                options,
                &input.mc_version,
                &input.java_path,
                &input.mc_jar,
                &input.mc_json,
                &options.loader.build,
                client,
                event_tx,
            )
            .await?;

        Ok(LoaderResult {
            libraries,
            main_class,
            loader_version: version_id,
            loader_type: LoaderType::NeoForge,
            extra_game_args,
            extra_jvm_args,
        })
    }
}

// ── OptiFine / LiteLoader ─────────────────────────────────────────────────────

const OPTIFINE_CLASS_ENTRY: &str = "optifine/Installer.class";
const OPTIFINE_INSTALLER_TIMEOUT: Duration = Duration::from_secs(300);
const OPTIFINE_INSTALLER_LOG_LIMIT: usize = 256 * 1024;
const INSTALLER_MAX_UNCOMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
const PROFILE_MAX_BYTES: u64 = 4 * 1024 * 1024;
const LIBRARIES_MINECRAFT: &str = "https://libraries.minecraft.net";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub struct OptiFineMC;

#[derive(Clone, Copy)]
pub struct LiteLoaderMC;

#[async_trait]
impl ModLoader for LiteLoaderMC {
    async fn install(
        &self,
        options: &LaunchOptions,
        input: &LoaderInstallInput,
        _client: &reqwest::Client,
        event_tx: &Sender<LaunchEvent>,
    ) -> Result<LoaderResult, LoaderError> {
        let artifact = resolved_artifact(options, LoaderArtifactKind::LiteLoader)?;
        let profile = artifact
            .profile
            .clone()
            .ok_or_else(|| LoaderError::ApiError("LiteLoader 清单缺少合成 profile".into()))?;
        let loader_version = lite_loader_identity(&input.mc_version, &artifact.installer_version)?;
        let result = finish_profile_loader(
            options,
            LoaderType::LiteLoader,
            "liteloader",
            profile,
            loader_version,
            None,
            None,
            Vec::new(),
        )
        .await?;
        let _ = event_tx
            .send(LaunchEvent::Patch("LiteLoader profile".into()))
            .await;
        Ok(result)
    }
}

#[async_trait]
impl ModLoader for OptiFineMC {
    async fn install(
        &self,
        options: &LaunchOptions,
        input: &LoaderInstallInput,
        client: &reqwest::Client,
        event_tx: &Sender<LaunchEvent>,
    ) -> Result<LoaderResult, LoaderError> {
        let artifact = resolved_artifact(options, LoaderArtifactKind::OptiFine)?;
        let route = optifine_route(artifact, &input.mc_version);
        let result = match route {
            "launchwrapper" => {
                install_optifine_launchwrapper(options, input, artifact, event_tx).await?
            }
            "installer" => {
                install_optifine_with_java(options, input, artifact, client, event_tx).await?
            }
            _ => {
                return Err(LoaderError::ApiError(
                    "OptiFine 条目缺少可自动安装的独立路线".into(),
                ));
            }
        };
        Ok(result)
    }
}

fn resolved_artifact<'a>(
    options: &'a LaunchOptions,
    expected: LoaderArtifactKind,
) -> Result<&'a ResolvedLoaderArtifact, LoaderError> {
    let artifact = options
        .loader
        .artifact
        .as_ref()
        .ok_or_else(|| LoaderError::ApiError("缺少加载器 Artifact，请刷新目录后重新安装".into()))?;
    if artifact.kind != expected || !artifact.installable {
        return Err(LoaderError::ApiError(
            "所选加载器构建没有受支持的自动安装输入".into(),
        ));
    }
    Ok(artifact)
}

fn optifine_route<'a>(artifact: &'a ResolvedLoaderArtifact, mc: &str) -> &'a str {
    artifact
        .raw_metadata
        .as_ref()
        .and_then(|value| value.get("route"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if optifine_needs_installer(mc) {
                "installer"
            } else {
                "launchwrapper"
            }
        })
}

fn optifine_needs_installer(mc: &str) -> bool {
    if mc.contains('w') {
        return true;
    }
    let drop = mc
        .strip_prefix("1.")
        .and_then(|value| value.split('.').next())
        .and_then(|value| value.parse::<u32>().ok())
        .map(|minor| minor * 10)
        .or_else(|| {
            let mut parts = mc.split('.');
            let major = parts.next()?.parse::<u32>().ok()?;
            let minor = parts.next()?.parse::<u32>().ok()?;
            Some(major * 10 + minor)
        });
    drop.is_some_and(|drop| drop >= 140)
}

fn safe_component(value: &str, label: &str) -> Result<String, LoaderError> {
    let value = value.trim();
    if value.is_empty()
        || matches!(value, "." | "..")
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(LoaderError::ApiError(format!("{label}无效")));
    }
    Ok(value.to_string())
}

fn lite_loader_identity(mc: &str, build: &str) -> Result<String, LoaderError> {
    let build = safe_component(build, "LiteLoader 构建号")?;
    Ok(format!("{mc}-LiteLoader-{build}"))
}
fn artifact_file_name(artifact: &ResolvedLoaderArtifact) -> Result<String, LoaderError> {
    safe_component(&artifact.file_name, "加载器文件名")
}

fn optifine_identity_version(artifact: &ResolvedLoaderArtifact) -> Result<String, LoaderError> {
    let version = safe_component(&artifact.installer_version, "OptiFine 版本号")?;
    Ok(if artifact.file_name.starts_with("preview_") {
        format!("preview_{version}")
    } else {
        version
    })
}

fn artifact_urls(artifact: &ResolvedLoaderArtifact) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut urls = Vec::new();
    for url in artifact
        .download_candidates
        .iter()
        .chain(std::iter::once(&artifact.url))
    {
        let url = url.trim();
        if !url.is_empty() && seen.insert(url.to_string()) {
            urls.push(url.to_string());
        }
    }
    urls
}

async fn ensure_optifine_artifact(
    options: &LaunchOptions,
    artifact: &ResolvedLoaderArtifact,
    event_tx: &Sender<LaunchEvent>,
) -> Result<PathBuf, LoaderError> {
    let path = download_artifact(options, artifact, "optifine", event_tx).await?;
    if validate_optifine_jar(&path).await.is_ok() {
        return Ok(path);
    }
    let _ = tokio::fs::remove_file(&path).await;
    let path = download_artifact(options, artifact, "optifine", event_tx).await?;
    validate_optifine_jar(&path).await?;
    Ok(path)
}

async fn download_artifact(
    options: &LaunchOptions,
    artifact: &ResolvedLoaderArtifact,
    loader_name: &str,
    event_tx: &Sender<LaunchEvent>,
) -> Result<PathBuf, LoaderError> {
    let file_name = artifact_file_name(artifact)?;
    let folder = options.loader_dir(loader_name).join("installer");
    tokio::fs::create_dir_all(&folder).await?;
    let destination = folder.join(&file_name);
    if destination
        .metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
    {
        return Ok(destination);
    }
    let urls = artifact_urls(artifact);
    if urls.is_empty() {
        return Err(LoaderError::ApiError(
            "加载器 Artifact 缺少可下载地址".into(),
        ));
    }
    let mut failures = Vec::new();
    for url in urls {
        let item = DownloadItem {
            url,
            path: destination.clone(),
            folder: folder.clone(),
            name: file_name.clone(),
            size: artifact.size.unwrap_or(0),
            r#type: Some(loader_name.into()),
            sha1: artifact.sha1.clone(),
        };
        let result = Downloader::new(options.timeout_secs, 1, options.force_ipv4, options.dns)
            .download_multiple(vec![item], event_tx.clone())
            .await;
        match result {
            Ok(()) if destination.is_file() => return Ok(destination),
            Ok(()) => failures.push("下载完成但目标文件不存在".to_string()),
            Err(error) => {
                let _ = tokio::fs::remove_file(&destination).await;
                failures.push(error.to_string());
            }
        }
    }
    Err(LoaderError::InstallerDownloadFailed(
        failures.join("；"),
    ))
}

async fn validate_optifine_jar(path: &Path) -> Result<(), LoaderError> {
    let bytes = tokio::fs::read(path).await?;
    crate::utils::archive::validate_executable_jar(
        &bytes,
        INSTALLER_MAX_UNCOMPRESSED_BYTES,
        &[OPTIFINE_CLASS_ENTRY],
    )
    .map_err(|error| LoaderError::Archive(error.to_string()))
}

pub fn class_major_from_header(header: &[u8]) -> Result<u32, String> {
    if header.len() < 8 || header[..4] != [0xCA, 0xFE, 0xBA, 0xBE] {
        return Err("OptiFine Installer.class 文件头无效".into());
    }
    let major = u16::from_be_bytes([header[6], header[7]]) as u32;
    if !(49..=100).contains(&major) {
        return Err(format!("OptiFine Installer.class 版本 {major} 不受支持"));
    }
    Ok(major)
}

pub fn installer_minimum_java(major: u32) -> u32 {
    major.saturating_sub(44).max(8)
}

async fn optifine_class_major(path: &Path) -> Result<u32, LoaderError> {
    let result = get_file_from_archive(
        path.to_path_buf(),
        Some(OPTIFINE_CLASS_ENTRY.into()),
        None,
        false,
    )
    .await
    .map_err(|error| LoaderError::Archive(error.to_string()))?;
    let bytes = match result {
        ArchiveQueryResult::FileData(bytes) => bytes,
        ArchiveQueryResult::NotFound => {
            return Err(LoaderError::ApiError(
                "OptiFine 安装器缺少 optifine/Installer.class".into(),
            ));
        }
        _ => {
            return Err(LoaderError::ApiError(
                "OptiFine 安装器 Class 读取结果无效".into(),
            ));
        }
    };
    class_major_from_header(&bytes).map_err(LoaderError::ApiError)
}

async fn install_optifine_launchwrapper(
    options: &LaunchOptions,
    input: &LoaderInstallInput,
    artifact: &ResolvedLoaderArtifact,
    event_tx: &Sender<LaunchEvent>,
) -> Result<LoaderResult, LoaderError> {
    let installer_path = ensure_optifine_artifact(options, artifact, event_tx).await?;
    let optifine_version = safe_component(&artifact.installer_version, "OptiFine 版本号")?;
    let identity_version = optifine_identity_version(artifact)?;
    let profile_id = format!("{}-OptiFine_{}", input.mc_version, optifine_version);
    let profile = json!({
        "id": profile_id,
        "inheritsFrom": input.mc_version,
        "jar": input.mc_version,
        "type": "release",
        "libraries": [
            {"name": format!("optifine:OptiFine:{optifine_version}")},
            {"name": "net.minecraft:launchwrapper:1.12"}
        ],
        "mainClass": "net.minecraft.launchwrapper.Launch",
        "arguments": {"game": ["--tweakClass", "optifine.OptiFineTweaker"]},
        "minimumLauncherVersion": 21
    });
    let source_url = artifact
        .download_urls()
        .first()
        .copied()
        .unwrap_or(&artifact.url)
        .to_string();
    let result = finish_profile_loader(
        options,
        LoaderType::OptiFine,
        "optifine",
        profile,
        format!("{}-OptiFine_{identity_version}", input.mc_version),
        Some((&installer_path, source_url.as_str())),
        None,
        Vec::new(),
    )
    .await?;
    Ok(result)
}

async fn install_optifine_with_java(
    options: &LaunchOptions,
    input: &LoaderInstallInput,
    artifact: &ResolvedLoaderArtifact,
    _client: &reqwest::Client,
    event_tx: &Sender<LaunchEvent>,
) -> Result<LoaderResult, LoaderError> {
    let installer_path = ensure_optifine_artifact(options, artifact, event_tx).await?;
    let class_major = optifine_class_major(&installer_path).await?;
    let required_java = class_major.saturating_sub(44).max(8);
    let java_major = installed_java_major(&input.java_path)
        .await
        .ok_or_else(|| LoaderError::ApiError("无法读取当前 Java 主版本".into()))?;
    if java_major < required_java {
        return Err(LoaderError::ApiError(format!(
            "OptiFine 安装器需要 Java {required_java} 或更高版本，当前为 Java {java_major}"
        )));
    }
    let temp_home = TempHome::create(&options.loader_dir("optifine").join("installer-tmp"))?;
    prepare_optifine_home(temp_home.path(), input).await?;
    run_optifine_installer(
        &input.java_path,
        &installer_path,
        temp_home.path(),
        event_tx,
    )
    .await?;
    let generated = find_generated_optifine_profile(temp_home.path(), &input.mc_version)?;
    let generated_libraries = temp_home.path().join(".minecraft").join("libraries");
    let loader_root = options.loader_dir("optifine");
    let permanent_dir = loader_root.join("versions").join(&generated.id);
    let permanent_jar = permanent_dir.join(format!("{}.jar", generated.id));
    let mut extras = Vec::new();
    let generated_jar = generated.path.with_extension("jar");
    if generated_jar.is_file() {
        atomic_copy(&generated_jar, &permanent_jar).await?;
        let size = tokio::fs::metadata(&permanent_jar).await?.len();
        extras.push(AssetItem::Asset {
            path: permanent_jar.to_string_lossy().into_owned(),
            sha1: String::new(),
            size,
            url: String::new(),
        });
    }
    let source_url = artifact
        .download_urls()
        .first()
        .copied()
        .unwrap_or(&artifact.url)
        .to_string();
    let internal_version = format!(
        "{}-OptiFine_{}",
        input.mc_version,
        optifine_identity_version(artifact)?
    );
    finish_profile_loader(
        options,
        LoaderType::OptiFine,
        "optifine",
        generated.value,
        internal_version,
        Some((&installer_path, source_url.as_str())),
        Some(&generated_libraries),
        extras,
    )
    .await
}

async fn finish_profile_loader(
    options: &LaunchOptions,
    loader_type: LoaderType,
    loader_name: &str,
    profile: Value,
    loader_version: String,
    optifine_source: Option<(&Path, &str)>,
    generated_libraries: Option<&Path>,
    extra_assets: Vec<AssetItem>,
) -> Result<LoaderResult, LoaderError> {
    let profile_id = profile
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| LoaderError::ApiError("加载器 profile 缺少 id".into()))?;
    let profile_id = safe_component(profile_id, "加载器 profile id")?;
    let main_class = profile
        .get("mainClass")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    let loader_root = options.loader_dir(loader_name);
    let mut libraries =
        build_profile_assets(&loader_root, &profile, optifine_source, generated_libraries).await?;
    libraries.extend(extra_assets);
    save_loader_version(&loader_root, &profile_id, &profile).await?;
    match loader_type {
        LoaderType::OptiFine => {
            tokio::fs::create_dir_all(options.save_dir().join("mods")).await?;
        }
        LoaderType::LiteLoader => {
            tokio::fs::create_dir_all(
                options
                    .save_dir()
                    .join("mods")
                    .join(profile.get("inheritsFrom").and_then(Value::as_str).unwrap_or("")),
            )
            .await?;
        }
        _ => {}
    }
    Ok(LoaderResult {
        libraries,
        main_class,
        loader_version,
        loader_type,
        extra_game_args: profile_game_args(&profile),
        extra_jvm_args: profile_jvm_args(&loader_root, &profile_id, &profile),
    })
}

async fn build_profile_assets(
    loader_root: &Path,
    profile: &Value,
    optifine_source: Option<(&Path, &str)>,
    generated_libraries: Option<&Path>,
) -> Result<Vec<AssetItem>, LoaderError> {
    let libraries: Vec<LoaderLibrary> = profile
        .get("libraries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect();
    let mut assets = Vec::with_capacity(libraries.len());
    for library in libraries {
        if library.rules.is_some() || library.clientreq == Some(false) {
            continue;
        }
        let Ok(info) = get_path_libraries(&library.name, None, None) else {
            continue;
        };
        let relative = PathBuf::from(&info.path).join(&info.name);
        let destination = loader_root.join("libraries").join(&relative);
        let mut url = library
            .downloads
            .as_ref()
            .and_then(|downloads| downloads.artifact.as_ref())
            .map(|artifact| artifact.url.clone())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                library.url.as_ref().filter(|value| !value.is_empty()).map(|base| {
                    format!(
                        "{}/{}",
                        base.trim_end_matches('/'),
                        relative.to_string_lossy().replace('\\', "/")
                    )
                })
            })
            .unwrap_or_default();
        if library.name.starts_with("optifine:OptiFine:") {
            if let Some((source, source_url)) = optifine_source {
                atomic_copy(source, &destination).await?;
                url = source_url.to_string();
            }
        } else if let Some(root) = generated_libraries {
            let generated = root.join(&relative);
            if generated.is_file() {
                atomic_copy(&generated, &destination).await?;
            }
        }
        if url.is_empty() && library.url.is_none() {
            url = format!(
                "{LIBRARIES_MINECRAFT}/{}",
                relative.to_string_lossy().replace('\\', "/")
            );
        }
        if url.is_empty() && !destination.is_file() {
            continue;
        }
        let artifact = library
            .downloads
            .as_ref()
            .and_then(|downloads| downloads.artifact.as_ref());
        assets.push(AssetItem::Asset {
            path: destination.to_string_lossy().into_owned(),
            sha1: artifact
                .and_then(|artifact| artifact.sha1.clone())
                .unwrap_or_default(),
            size: artifact
                .and_then(|artifact| artifact.size)
                .or_else(|| {
                    std::fs::metadata(&destination)
                        .ok()
                        .filter(|metadata| metadata.is_file())
                        .map(|metadata| metadata.len())
                })
                .unwrap_or(0),
            url,
        });
    }
    Ok(assets)
}

fn profile_game_args(profile: &Value) -> Vec<String> {
    if let Some(arguments) = profile.pointer("/arguments/game").and_then(Value::as_array) {
        return arguments
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
    }
    let Some(arguments) = profile
        .get("minecraftArguments")
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    let tokens: Vec<&str> = arguments.split_whitespace().collect();
    tokens
        .iter()
        .position(|token| *token == "--tweakClass")
        .and_then(|index| tokens.get(index + 1).map(|value| {
            vec!["--tweakClass".to_string(), (*value).to_string()]
        }))
        .unwrap_or_default()
}

fn profile_jvm_args(loader_root: &Path, profile_id: &str, profile: &Value) -> Vec<String> {
    let library_directory = loader_root.join("libraries").to_string_lossy().into_owned();
    let separator = if cfg!(windows) { ";" } else { ":" };
    let Some(arguments) = profile.pointer("/arguments/jvm").and_then(Value::as_array) else {
        return Vec::new();
    };
    arguments
        .iter()
        .filter_map(Value::as_str)
        .filter(|argument| !argument.contains("${classpath}") && *argument != "-cp")
        .map(|argument| {
            argument
                .replace("${library_directory}", &library_directory)
                .replace("${classpath_separator}", separator)
                .replace("${version_name}", profile_id)
        })
        .collect()
}

async fn installed_java_major(java_path: &str) -> Option<u32> {
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(java_path)
            .args(["-XshowSettings:properties", "-version"])
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    parse_java_major_text(&text)
}

fn parse_java_major_text(text: &str) -> Option<u32> {
    let version = text
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("java.version")
                .and_then(|value| value.split_once('='))
                .map(|(_, value)| value.trim())
        })
        .or_else(|| {
            let marker = "version \"";
            let start = text.find(marker)? + marker.len();
            text[start..].split('"').next()
        })?;
    let normalized = version.strip_prefix("1.").unwrap_or(version);
    normalized
        .split(['.', '_', '-', '+'])
        .next()
        .and_then(|value| value.parse::<u32>().ok())
}

fn unique_temp_path(root: &Path) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    root.join(format!("{}-{stamp}-{sequence}", std::process::id()))
}

struct TempHome {
    path: PathBuf,
}

impl TempHome {
    fn create(root: &Path) -> Result<Self, LoaderError> {
        std::fs::create_dir_all(root)?;
        for _ in 0..32 {
            let path = unique_temp_path(root);
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(LoaderError::Io(error)),
            }
        }
        Err(LoaderError::Io(std::io::Error::other(
            "无法创建 OptiFine 临时目录",
        )))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

async fn prepare_optifine_home(home: &Path, input: &LoaderInstallInput) -> Result<(), LoaderError> {
    let minecraft = home.join(".minecraft");
    let version_dir = minecraft.join("versions").join(&input.mc_version);
    tokio::fs::create_dir_all(&version_dir).await?;
    tokio::fs::copy(
        &input.mc_json,
        version_dir.join(format!("{}.json", input.mc_version)),
    )
    .await?;
    tokio::fs::copy(
        &input.mc_jar,
        version_dir.join(format!("{}.jar", input.mc_version)),
    )
    .await?;
    write_bytes_atomic(
        &minecraft.join("launcher_profiles.json"),
        br#"{"profiles":{},"settings":{"enableSnapshots":false},"version":3}"#,
    )
    .await?;
    Ok(())
}

async fn run_optifine_installer(
    java_path: &str,
    installer_path: &Path,
    home: &Path,
    event_tx: &Sender<LaunchEvent>,
) -> Result<(), LoaderError> {
    let _ = event_tx
        .send(LaunchEvent::Patch("OptiFine Installer".into()))
        .await;
    let mut command = tokio::process::Command::new(java_path);
    command
        .arg(format!("-Duser.home={}", home.to_string_lossy()))
        .arg("-Dminecraft.launcher.brand=Nickel")
        .arg("-cp")
        .arg(installer_path)
        .arg("optifine.Installer")
        .current_dir(home)
        .env("HOME", home)
        .env("APPDATA", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = tokio::spawn(read_limited_maybe(stdout, OPTIFINE_INSTALLER_LOG_LIMIT));
    let stderr_task = tokio::spawn(read_limited_maybe(stderr, OPTIFINE_INSTALLER_LOG_LIMIT));
    let cancel = TransferContext::current().cancellation().clone();
    let waited = tokio::time::timeout(OPTIFINE_INSTALLER_TIMEOUT, async {
        tokio::select! {
            status = child.wait() => Some(status),
            _ = cancel.cancelled() => None,
        }
    })
    .await;
    let outcome = match waited {
        Ok(Some(Ok(status))) => status,
        Ok(Some(Err(error))) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(LoaderError::Io(error));
        }
        Ok(None) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(LoaderError::ApiError("OptiFine 安装已取消".into()));
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(LoaderError::ApiError(
                "OptiFine 安装器运行超过 5 分钟".into(),
            ));
        }
    };
    let stdout = stdout_task.await.unwrap_or_default();
    let stderr = stderr_task.await.unwrap_or_default();
    if !outcome.success() {
        let detail = installer_log_summary(&stdout, &stderr);
        return Err(LoaderError::ApiError(format!(
            "OptiFine 安装器退出码 {}：{detail}",
            outcome.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

async fn read_limited_maybe<R>(reader: Option<R>, limit: usize) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    match reader {
        Some(reader) => read_limited(reader, limit).await,
        None => Vec::new(),
    }
}

async fn read_limited<R>(mut reader: R, limit: usize) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                let remaining = limit.saturating_sub(output.len());
                if remaining > 0 {
                    output.extend_from_slice(&buffer[..count.min(remaining)]);
                }
            }
        }
    }
    output
}

fn installer_log_summary(stdout: &[u8], stderr: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(stderr));
    let line: String = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("安装器没有输出错误详情")
        .chars()
        .take(400)
        .collect();
    line
}

struct GeneratedProfile {
    id: String,
    path: PathBuf,
    value: Value,
}

fn find_generated_optifine_profile(
    home: &Path,
    minecraft_id: &str,
) -> Result<GeneratedProfile, LoaderError> {
    let versions = home.join(".minecraft").join("versions");
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(&versions)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let json_path = path.join(format!("{name}.json"));
        let Ok(metadata) = std::fs::symlink_metadata(&json_path) else {
            continue;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > PROFILE_MAX_BYTES
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(&json_path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            continue;
        };
        if id == minecraft_id
            || !id.to_ascii_lowercase().contains("optifine")
            || safe_component(id, "OptiFine profile id").is_err()
        {
            continue;
        }
        if value
            .get("inheritsFrom")
            .and_then(Value::as_str)
            .is_some_and(|parent| parent != minecraft_id)
        {
            continue;
        }
        if value
            .get("mainClass")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            continue;
        }
        if !value
            .get("libraries")
            .and_then(Value::as_array)
            .is_some_and(|libraries| {
                libraries.iter().any(|library| {
                    library
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.starts_with("optifine:OptiFine:"))
                })
            })
        {
            continue;
        }
        candidates.push(GeneratedProfile {
            id: id.to_string(),
            path: json_path,
            value,
        });
    }
    candidates.sort_by(|left, right| left.id.cmp(&right.id));
    candidates
        .pop()
        .ok_or_else(|| LoaderError::ApiError("OptiFine 安装器未生成有效 profile".into()))
}

async fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), LoaderError> {
    let parent = path
        .parent()
        .ok_or_else(|| LoaderError::ApiError("目标文件缺少父目录".into()))?;
    tokio::fs::create_dir_all(parent).await?;
    let temporary = unique_temp_path(parent).with_extension("part");
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temporary, path).await?;
        Ok::<_, std::io::Error>(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result.map_err(LoaderError::Io)
}

async fn atomic_copy(source: &Path, destination: &Path) -> Result<(), LoaderError> {
    let parent = destination
        .parent()
        .ok_or_else(|| LoaderError::ApiError("目标文件缺少父目录".into()))?;
    tokio::fs::create_dir_all(parent).await?;
    let temporary = unique_temp_path(parent).with_extension("part");
    let result = async {
        tokio::fs::copy(source, &temporary).await?;
        tokio::fs::rename(&temporary, destination).await?;
        Ok::<_, std::io::Error>(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result.map_err(LoaderError::Io)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_loader_identities_have_stable_serde_names() {
        assert_eq!(
            serde_json::to_string(&LoaderType::OptiFine).unwrap(),
            "\"optifine\""
        );
        assert_eq!(
            serde_json::to_string(&LoaderType::LiteLoader).unwrap(),
            "\"liteloader\""
        );
    }

    #[test]
    fn optifine_route_separates_launchwrapper_and_installer() {
        assert_eq!(optifine_route(&empty_artifact(), "1.12.2"), "launchwrapper");
        assert_eq!(optifine_route(&empty_artifact(), "1.14"), "installer");
        assert_eq!(optifine_route(&empty_artifact(), "26.1"), "installer");
    }

    #[test]
    fn optifine_class_major_maps_to_minimum_java() {
        let header = [0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 52];
        assert_eq!(class_major_from_header(&header).unwrap(), 52);
        assert_eq!(installer_minimum_java(52), 8);
        assert_eq!(installer_minimum_java(65), 21);
        assert!(class_major_from_header(b"not-class").is_err());
    }

    #[test]
    fn java_version_parser_accepts_legacy_and_modern_output() {
        assert_eq!(
            parse_java_major_text("    java.version = 1.8.0_392"),
            Some(8)
        );
        assert_eq!(parse_java_major_text("openjdk version \"21.0.2\" 2024"), Some(21));
    }

    #[test]
    fn temporary_installer_home_is_removed_on_failure_paths() {
        let root = std::env::temp_dir().join(format!(
            "nowhere-optifine-temp-{}",
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let home = TempHome::create(&root).unwrap();
        let path = home.path().to_path_buf();
        std::fs::create_dir_all(path.join(".minecraft/versions/partial")).unwrap();
        std::fs::write(path.join(".minecraft/versions/partial/partial.json"), b"{}").unwrap();
        drop(home);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn generated_profile_finder_is_scoped_to_target_minecraft() {
        let root = std::env::temp_dir().join(format!(
            "nowhere-optifine-profile-{}",
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let version_dir = root.join(".minecraft/versions/1.20.1-OptiFine_HD_U_I6");
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(
            version_dir.join("1.20.1-OptiFine_HD_U_I6.json"),
            serde_json::to_vec(&json!({
                "id": "1.20.1-OptiFine_HD_U_I6",
                "inheritsFrom": "1.20.1",
                "mainClass": "net.minecraft.launchwrapper.Launch",
                "libraries": [{"name": "optifine:OptiFine:1.20.1_HD_U_I6"}]
            }))
            .unwrap(),
        )
        .unwrap();
        let profile = find_generated_optifine_profile(&root, "1.20.1").unwrap();
        assert_eq!(profile.id, "1.20.1-OptiFine_HD_U_I6");
        assert_eq!(
            profile.value["inheritsFrom"],
            Value::String("1.20.1".into())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn liteloader_profile_builds_runtime_assets_without_network() {
        let root = std::env::temp_dir().join(format!(
            "nowhere-liteloader-assets-{}",
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let profile = json!({
            "id": "1.12.2-LiteLoader",
            "inheritsFrom": "1.12.2",
            "mainClass": "net.minecraft.launchwrapper.Launch",
            "arguments": {"game": ["--tweakClass", "com.mumfrey.liteloader.launch.LiteLoaderTweaker"]},
            "libraries": [
                {"name": "com.mumfrey:liteloader:1.12.2-SNAPSHOT-r0", "url": "https://dl.liteloader.com/versions/"},
                {"name": "net.minecraft:launchwrapper:1.12"}
            ]
        });
        let assets = build_profile_assets(&root, &profile, None, None)
            .await
            .unwrap();
        assert!(assets.iter().any(|asset| match asset {
            AssetItem::Asset { path, .. } => path.ends_with("liteloader-1.12.2-SNAPSHOT-r0.jar"),
            _ => false,
        }));
        assert!(assets.iter().any(|asset| match asset {
            AssetItem::Asset { path, .. } => path.ends_with("launchwrapper-1.12.jar"),
            _ => false,
        }));
        assert_eq!(
            profile_game_args(&profile),
            ["--tweakClass", "com.mumfrey.liteloader.launch.LiteLoaderTweaker"]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn empty_artifact() -> ResolvedLoaderArtifact {
        ResolvedLoaderArtifact {
            kind: LoaderArtifactKind::OptiFine,
            installable: true,
            ..ResolvedLoaderArtifact::default()
        }
    }

    #[test]
    fn create_loader_forge_is_dyn() {
        let _loader: Box<dyn ModLoader> = create_loader(LoaderType::Forge);
    }

    #[test]
    fn create_loader_neoforge_is_dyn() {
        let _loader: Box<dyn ModLoader> = create_loader(LoaderType::NeoForge);
    }

    #[test]
    fn create_loader_fabric_is_dyn() {
        let _loader: Box<dyn ModLoader> = create_loader(LoaderType::Fabric);
    }

    #[test]
    fn create_loader_legacy_fabric_is_dyn() {
        let _loader: Box<dyn ModLoader> = create_loader(LoaderType::LegacyFabric);
    }

    #[test]
    fn create_loader_quilt_is_dyn() {
        let _loader: Box<dyn ModLoader> = create_loader(LoaderType::Quilt);
    }

    #[test]
    fn all_loader_types_are_dispatchable() {
        let types = [
            LoaderType::Forge,
            LoaderType::NeoForge,
            LoaderType::Fabric,
            LoaderType::LegacyFabric,
            LoaderType::Quilt,
            LoaderType::OptiFine,
            LoaderType::LiteLoader,
        ];
        for t in types {
            let _loader = create_loader(t);
        }
    }
}
