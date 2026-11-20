use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tokio::sync::mpsc::Sender;

use crate::error::LoaderError;
use crate::launcher::events::LaunchEvent;
use crate::launcher::options::LaunchOptions;
use crate::loader::forge_patcher::{ForgePatcher, PatchConfig};
use crate::models::loader::{
    ArtifactCategory, ForgeProfile, ForgeVersionSection, InstallerInfo, LoaderArtifactKind,
    LoaderLibrary, LoaderType, ResolvedLoaderArtifact,
};
use crate::models::minecraft::AssetItem;
use crate::net::downloader::{DownloadItem, Downloader};
use crate::utils::archive::{ArchiveQueryResult, get_file_from_archive};
use crate::utils::paths::get_path_libraries;

// ── Constants ─────────────────────────────────────────────────────────────────

const META_URL: &str =
    "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml";
const PROMOTIONS_URL: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
const MAVEN_BASE: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";

#[derive(Deserialize)]
struct Promotions {
    promos: HashMap<String, String>,
}

// ── Public API ────────────────────────────────────────────────────────────────

pub struct ForgeMC;

/// 应用层 artifact 是目录、另存与自动安装的共同来源。核心不再根据 mc+build
/// 猜测 Forge 文件类别；这里只校验已解析结果并下载精确 URL。
pub(crate) async fn download_resolved_installer(
    options: &LaunchOptions,
    artifact: &ResolvedLoaderArtifact,
    expected_kind: LoaderArtifactKind,
    loader_name: &str,
    old_api: bool,
    event_tx: &Sender<LaunchEvent>,
) -> Result<InstallerInfo, LoaderError> {
    if artifact.kind != expected_kind
        || artifact.category != ArtifactCategory::Installer
        || !artifact.installable
    {
        return Err(LoaderError::ApiError(
            "所选构建没有可自动安装的官方安装器".into(),
        ));
    }
    if artifact.url.is_empty() || artifact.file_name.is_empty() {
        return Err(LoaderError::ApiError("加载器安装器缺少下载地址".into()));
    }
    let mut components = Path::new(&artifact.file_name).components();
    let file_name = match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(name)), None) => name.to_string_lossy().into_owned(),
        _ => return Err(LoaderError::ApiError("加载器安装器文件名无效".into())),
    };
    let installer_folder = options.loader_dir(loader_name).join("installer");
    tokio::fs::create_dir_all(&installer_folder).await?;
    let installer_path = installer_folder.join(&file_name);
    if !installer_path.exists() {
        let item = DownloadItem {
            url: artifact.url.clone(),
            path: installer_path.clone(),
            folder: installer_folder,
            name: file_name,
            size: artifact.size.unwrap_or(0),
            r#type: Some(loader_name.into()),
            sha1: artifact.sha1.clone(),
        };
        Downloader::new(options.timeout_secs, 1, options.force_ipv4, options.dns)
            .download_multiple(vec![item], event_tx.clone())
            .await
            .map_err(|error| LoaderError::Io(std::io::Error::other(error.to_string())))?;
    }
    Ok(InstallerInfo {
        file_path: installer_path.to_string_lossy().into_owned(),
        meta_data: artifact.installer_version.clone(),
        ext: "jar".into(),
        id: format!("{loader_name}-{}", artifact.installer_version),
        old_api,
    })
}

impl ForgeMC {
    pub fn new() -> Self {
        Self
    }

    /// 从安装器提取版本清单和内置依赖，再用受管下载补齐依赖并运行本地处理器。
    /// 文件布局保留在 `<loader_base>/libraries` 与 `versions/<id>/<id>.json`。
    pub async fn install(
        &self,
        options: &LaunchOptions,
        mc_version: &str,
        java_path: &str,
        mc_jar: &str,
        mc_json: &str,
        build: &str,
        client: &reqwest::Client,
        event_tx: &Sender<LaunchEvent>,
    ) -> Result<
        (
            String,
            Option<String>,
            Vec<AssetItem>,
            Vec<String>,
            Vec<String>,
        ),
        LoaderError,
    > {
        let loader_base = options.loader_dir("forge");
        tokio::fs::create_dir_all(&loader_base).await?;

        // 1. Download the installer JAR.
        let installer = self
            .download_installer(options, mc_version, build, client, event_tx)
            .await?;

        // 2. Determine the version ID the installer will create.
        let version_id = read_installer_version_id(&installer.file_path).await?;
        let version_json_path = loader_base
            .join("versions")
            .join(&version_id)
            .join(format!("{version_id}.json"));

        // 3. All installer transfers go through the managed Rust downloader.
        if !version_json_path.exists() {
            let used_patcher = try_patcher_install(
                &installer.file_path,
                &loader_base,
                &version_json_path,
                mc_jar,
                mc_json,
                java_path,
                &options.path,
                options,
                LoaderType::Forge,
                false, // neo_forge_old: not applicable for Forge
                event_tx,
            )
            .await?;

            if !used_patcher {
                return Err(LoaderError::ApiError(
                    "This installer format is not supported by the managed installer".into(),
                ));
            }

            if !version_json_path.exists() {
                return Err(LoaderError::ApiError(format!(
                    "Forge install finished but no version JSON found at {}",
                    version_json_path.display()
                )));
            }
        }

        // 4. Parse the version JSON and build the loader result.
        let version_json = read_version_json(&version_json_path).await?;
        let libraries = build_library_assets(&loader_base, &version_json);
        let extra_game_args = extract_game_args(&version_json);
        let extra_jvm_args = extract_jvm_args(&loader_base, &version_id, &version_json);
        let main_class = version_json.main_class;

        Ok((
            version_id,
            main_class,
            libraries,
            extra_game_args,
            extra_jvm_args,
        ))
    }

    /// Download the Forge installer JAR for the given Minecraft version and build.
    pub async fn download_installer(
        &self,
        options: &LaunchOptions,
        mc_version: &str,
        build: &str,
        client: &reqwest::Client,
        event_tx: &Sender<LaunchEvent>,
    ) -> Result<InstallerInfo, LoaderError> {
        if let Some(artifact) = options.loader.artifact.as_ref() {
            return download_resolved_installer(
                options,
                artifact,
                LoaderArtifactKind::Forge,
                "forge",
                false,
                event_tx,
            )
            .await;
        }

        // 已选定的现代构建无需再下载整个 Maven 索引，避免列表成功后安装又被索引故障阻断。
        // 旧版短构建号可能缺少分支后缀，仍查询完整清单解析；latest/recommended 同理。
        let forge_build = if let Some(coordinate) = explicit_forge_coordinate(mc_version, build)? {
            coordinate
        } else {
            let prefix = format!("{mc_version}-");
            let context = crate::net::transfer::TransferContext::current();
            let versions: Vec<String> = crate::net::transfer::fetch_validated_candidates(
                client,
                &context,
                &crate::net::http::service_key(META_URL),
                &crate::net::transfer::core_candidates(META_URL),
                crate::net::transfer::DEFAULT_LIST_LIMIT,
                |bytes| {
                    let xml = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
                    let versions: Vec<_> = crate::loader::neoforge::parse_maven_xml_versions(xml)
                        .into_iter()
                        .filter(|version| version.starts_with(&prefix))
                        .collect();
                    if versions.is_empty() {
                        return Err(format!("清单缺少 Minecraft {mc_version} 构建"));
                    }
                    Ok(versions)
                },
            )
            .await
            .map_err(LoaderError::ApiError)?;
            let requested =
                if matches!(build, "latest" | "recommended") || build.starts_with(&prefix) {
                    build.to_string()
                } else {
                    format!("{prefix}{build}")
                };
            let coordinate = resolve_forge_build(&requested, mc_version, &versions, client).await?;
            if !versions.contains(&coordinate) {
                return Err(LoaderError::VersionNotFound(format!(
                    "Forge build {coordinate} not found for {mc_version}"
                )));
            }
            coordinate
        };
        let installer_name = format!("forge-{forge_build}-installer.jar");
        let installer_folder = options.loader_dir("forge").join("installer");
        let installer_path = installer_folder.join(&installer_name);
        if !installer_path.exists() {
            let item = DownloadItem {
                url: format!("{MAVEN_BASE}/{forge_build}/{installer_name}"),
                path: installer_path.clone(),
                folder: installer_folder,
                name: installer_name,
                size: 0,
                r#type: Some("forge".into()),
                sha1: None,
            };
            Downloader::new(options.timeout_secs, 1, options.force_ipv4, options.dns)
                .download_multiple(vec![item], event_tx.clone())
                .await
                .map_err(|e| LoaderError::Io(std::io::Error::other(e.to_string())))?;
        }
        Ok(InstallerInfo {
            file_path: installer_path.to_string_lossy().into_owned(),
            meta_data: forge_build.clone(),
            ext: "jar".into(),
            id: format!("forge-{forge_build}"),
            old_api: false,
        })
    }
}

impl Default for ForgeMC {
    fn default() -> Self {
        Self::new()
    }
}

// ── Installer driver ──────────────────────────────────────────────────────────

/// Read the `version` field from `install_profile.json` inside the installer.
///
/// This is the directory name the installer will create under `versions/`.
async fn read_installer_version_id(installer_path: &str) -> Result<String, LoaderError> {
    let result = get_file_from_archive(
        PathBuf::from(installer_path),
        Some("install_profile.json".into()),
        None,
        false,
    )
    .await
    .map_err(|e| LoaderError::Archive(e.to_string()))?;

    let bytes = match result {
        ArchiveQueryResult::FileData(b) => b,
        _ => return Err(LoaderError::ProfileNotFound),
    };

    let raw: serde_json::Value = serde_json::from_slice(&bytes)?;

    // New format (1.13+): top-level `version` string.
    if let Some(v) = raw.get("version").and_then(|v| v.as_str()) {
        return Ok(v.to_owned());
    }
    // Old format: install.version or versionInfo.id.
    if let Some(v) = raw
        .get("install")
        .and_then(|i| i.get("version"))
        .and_then(|v| v.as_str())
    {
        return Ok(v.to_owned());
    }
    if let Some(v) = raw
        .get("versionInfo")
        .and_then(|i| i.get("id"))
        .and_then(|v| v.as_str())
    {
        return Ok(v.to_owned());
    }

    Err(LoaderError::ApiError(
        "Could not determine version ID from install_profile.json".into(),
    ))
}

/// 读取 Rust 安装器生成的加载器版本清单。
async fn read_version_json(path: &Path) -> Result<ForgeVersionSection, LoaderError> {
    let bytes = tokio::fs::read(path).await?;
    let version: ForgeVersionSection = serde_json::from_slice(&bytes)?;
    Ok(version)
}

/// Extract plain-string game args from the Forge version JSON.
/// Handles both legacy `minecraftArguments` (string) and modern `arguments.game` (array).
fn extract_game_args(version: &ForgeVersionSection) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if let Some(mc_args) = &version.minecraft_arguments {
        for token in mc_args.split_whitespace() {
            args.push(token.to_owned());
        }
    }
    if let Some(forge_args) = &version.arguments {
        for entry in &forge_args.game {
            if let Some(s) = entry.as_str() {
                args.push(s.to_owned());
            }
        }
    }
    args
}

/// Extract and resolve JVM args from the Forge version JSON (`arguments.jvm`).
///
/// `${library_directory}` is resolved to `loader_base/libraries` (the Forge-local
/// library path, not the vanilla MC path). `${classpath_separator}` and
/// `${version_name}` are also substituted so the strings are ready to use as-is.
fn extract_jvm_args(
    loader_base: &Path,
    version_id: &str,
    version: &ForgeVersionSection,
) -> Vec<String> {
    let lib_dir = loader_base.join("libraries").to_string_lossy().into_owned();
    let sep = if cfg!(target_os = "windows") {
        ";"
    } else {
        ":"
    };
    let mut args = Vec::new();
    if let Some(forge_args) = &version.arguments {
        for entry in &forge_args.jvm {
            if let Some(s) = entry.as_str() {
                args.push(
                    s.replace("${library_directory}", &lib_dir)
                        .replace("${classpath_separator}", sep)
                        .replace("${version_name}", version_id),
                );
            }
        }
    }
    args
}

/// Build classpath entries for the loader libraries the installer wrote.
fn build_library_assets(loader_base: &Path, version: &ForgeVersionSection) -> Vec<AssetItem> {
    let libs = version.libraries.as_deref().unwrap_or(&[]);
    let mut items: Vec<AssetItem> = Vec::with_capacity(libs.len());

    for lib in libs {
        if lib.rules.is_some() {
            continue;
        }
        // Old-format Forge marks server-only libraries with clientreq: false.
        if lib.clientreq == Some(false) {
            continue;
        }

        let (path, sha1, size, url) = resolve_library_entry(loader_base, lib);
        items.push(AssetItem::Asset {
            path,
            sha1,
            size,
            url,
        });
    }

    items
}

fn resolve_library_entry(loader_base: &Path, lib: &LoaderLibrary) -> (String, String, u64, String) {
    let libs_dir = loader_base.join("libraries");

    let artifact = lib.downloads.as_ref().and_then(|d| d.artifact.as_ref());

    let rel_path = artifact
        .and_then(|a| a.path.clone())
        .or_else(|| {
            get_path_libraries(&lib.name, None, None)
                .ok()
                .map(|info| format!("{}/{}", info.path, info.name))
        })
        .unwrap_or_default();

    let abs_path = libs_dir.join(&rel_path);

    let sha1 = artifact.and_then(|a| a.sha1.clone()).unwrap_or_default();
    let size = artifact.and_then(|a| a.size).unwrap_or(0);
    // For old-format Forge libs that have no downloads.artifact, construct the
    // URL from the lib's base `url` + relative Maven path, or fall back to the
    // standard Minecraft library repo (used by launchwrapper, asm-all, etc.).
    let url = artifact
        .map(|a| a.url.clone())
        .filter(|u| !u.is_empty())
        .or_else(|| {
            lib.url
                .as_ref()
                .filter(|u| !u.is_empty())
                .map(|base| format!("{}/{}", base.trim_end_matches('/'), &rel_path))
        })
        .or_else(|| {
            if !rel_path.is_empty() {
                Some(format!("https://libraries.minecraft.net/{rel_path}"))
            } else {
                None
            }
        })
        .unwrap_or_default();

    (abs_path.to_string_lossy().into_owned(), sha1, size, url)
}

// ── Build resolution helpers ──────────────────────────────────────────────────

/// Older Forge builds append `-{mc_version}` to the build number in the Maven
/// artifact ID (e.g. `1.8.9-11.15.1.2318-1.8.9`), while the promotions API
/// returns only the bare build number. Try the plain candidate first; if it
/// isn't in the versions list, try the suffixed form.
fn match_promo_in_versions(candidate: &str, mc_version: &str, versions: &[String]) -> String {
    if versions.iter().any(|v| v == candidate) {
        return candidate.to_owned();
    }
    let with_suffix = format!("{candidate}-{mc_version}");
    if versions.iter().any(|v| v == &with_suffix) {
        return with_suffix;
    }
    candidate.to_owned()
}

fn explicit_forge_coordinate(mc: &str, build: &str) -> Result<Option<String>, LoaderError> {
    let safe = |value: &str| {
        !value.is_empty()
            && !value.contains("..")
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
    };
    if !safe(mc) || !safe(build) {
        return Err(LoaderError::ApiError(
            "Invalid Forge version coordinate".into(),
        ));
    }
    if matches!(build, "latest" | "recommended") {
        return Ok(None);
    }
    let prefix = format!("{mc}-");
    let short = build.strip_prefix(&prefix).unwrap_or(build);
    let legacy = mc
        .strip_prefix("1.")
        .and_then(|v| v.split('.').next())
        .and_then(|v| v.parse::<u32>().ok())
        .is_some_and(|minor| minor <= 12);
    if legacy && !short.contains('-') {
        return Ok(None);
    }
    Ok(Some(format!("{prefix}{short}")))
}

async fn resolve_forge_build(
    build: &str,
    mc_version: &str,
    versions: &[String],
    client: &reqwest::Client,
) -> Result<String, LoaderError> {
    if matches!(build, "latest" | "recommended") {
        if let Ok(promotions) =
            crate::net::http::fetch_json::<Promotions>(client, PROMOTIONS_URL).await
        {
            let key = format!("{mc_version}-{build}");
            let latest = format!("{mc_version}-latest");
            if let Some(version) = promotions
                .promos
                .get(&key)
                .or_else(|| promotions.promos.get(&latest))
            {
                let candidate = match_promo_in_versions(
                    &format!("{mc_version}-{version}"),
                    mc_version,
                    versions,
                );
                if versions.contains(&candidate) {
                    return Ok(candidate);
                }
            }
        }
        crate::net::transfer::TransferContext::current()
            .cancellation()
            .check()
            .map_err(LoaderError::ApiError)?;
        return versions.last().cloned().ok_or_else(|| {
            LoaderError::VersionNotFound(format!("No Forge builds for {mc_version}"))
        });
    }
    Ok(match_promo_in_versions(build, mc_version, versions))
}

// ── Intermediate-path patcher ─────────────────────────────────────────────────

/// 使用受管下载准备安装器依赖，再运行本地处理器。
/// 旧式 versionInfo、新式无处理器 profile 和现代处理器 profile 共用此入口。
/// 失败返回原始错误，不能把未完成安装留下的版本清单当作成功标记。
pub(crate) async fn try_patcher_install(
    installer_path: &str,
    loader_base: &Path,
    version_json_path: &Path,
    mc_jar: &str,
    mc_json: &str,
    java_path: &str,
    game_path: &Path,
    options: &LaunchOptions,
    loader_type: LoaderType,
    neo_forge_old: bool,
    event_tx: &Sender<LaunchEvent>,
) -> Result<bool, LoaderError> {
    let existed = version_json_path.exists();
    let result = try_patcher_install_inner(
        installer_path,
        loader_base,
        version_json_path,
        mc_jar,
        mc_json,
        java_path,
        game_path,
        options,
        loader_type,
        neo_forge_old,
        event_tx,
    )
    .await;
    if result.is_err() && !existed {
        let _ = tokio::fs::remove_file(version_json_path).await;
    }
    result
}

async fn try_patcher_install_inner(
    installer_path: &str,
    loader_base: &Path,
    version_json_path: &Path,
    mc_jar: &str,
    mc_json: &str,
    java_path: &str,
    _game_path: &Path,
    options: &LaunchOptions,
    loader_type: LoaderType,
    neo_forge_old: bool,
    event_tx: &Sender<LaunchEvent>,
) -> Result<bool, LoaderError> {
    let mut profile = read_install_profile(installer_path).await?;
    if profile.version_info.is_some() {
        install_old_forge_legacy(
            installer_path,
            loader_base,
            version_json_path,
            &profile,
            event_tx,
        )
        .await?;
        return Ok(true);
    }

    // Forge 1.12.2 的新版安装器也使用 version.json + maven/，但没有处理器。
    // 不能据 processors 为空判定格式不受支持。
    extract_version_json(installer_path, version_json_path).await?;
    let libs_dir = loader_base.join("libraries");
    extract_maven_entries(installer_path, &libs_dir).await?;
    download_profile_libraries(&profile, &libs_dir, options, event_tx).await?;
    extract_data_files(installer_path, &profile, &libs_dir).await?;

    let patcher = ForgePatcher::new(loader_base.to_path_buf(), loader_type);
    if patcher.check(&profile) {
        return Ok(true);
    }
    let config = PatchConfig {
        java_path,
        minecraft_jar: mc_jar,
        minecraft_json: mc_json,
        game_path: loader_base,
    };
    // installertools 的映射下载任务本身会联网，必须在 Rust 内完成后移除，
    // 其余 Java 处理器仅消费已准备好的本地输入。
    prepare_mojang_mappings(&mut profile, &patcher, &config, options, event_tx).await?;
    patcher
        .patch(&profile, &config, neo_forge_old, event_tx)
        .await?;
    Ok(true)
}

async fn prepare_mojang_mappings(
    profile: &mut ForgeProfile,
    patcher: &ForgePatcher,
    config: &PatchConfig<'_>,
    options: &LaunchOptions,
    event_tx: &Sender<LaunchEvent>,
) -> Result<(), LoaderError> {
    let mut completed = Vec::new();
    for (index, processor) in profile
        .processors
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .enumerate()
    {
        if !crate::loader::forge_patcher::is_client_side(processor.sides.as_deref())
            || !is_mojang_mapping_download(&processor.jar, &processor.args)
        {
            continue;
        }
        let args: Vec<_> = processor
            .args
            .iter()
            .map(|arg| patcher.resolve_argument(arg, profile, config))
            .collect();
        let argument = |name: &str| {
            args.windows(2)
                .find(|pair| pair[0] == name)
                .map(|pair| pair[1].as_str())
        };
        let version = argument("--version")
            .ok_or_else(|| LoaderError::ApiError("映射下载缺少游戏版本".into()))?;
        let side = argument("--side").unwrap_or("client");
        if side != "client" {
            return Err(LoaderError::ApiError("客户端处理器请求了服务端映射".into()));
        }
        let output = argument("--output")
            .ok_or_else(|| LoaderError::ApiError("映射下载缺少输出路径".into()))?;
        let destination = PathBuf::from(output);
        if !destination.starts_with(patcher.loader_base.join("libraries")) {
            return Err(LoaderError::ApiError(
                "映射输出必须位于加载器 libraries 目录".into(),
            ));
        }
        let metadata: serde_json::Value =
            serde_json::from_slice(&tokio::fs::read(config.minecraft_json).await?)?;
        if metadata.get("id").and_then(|value| value.as_str()) != Some(version) {
            return Err(LoaderError::ApiError(
                "映射下载版本与本地游戏清单不一致".into(),
            ));
        }
        let mapping = metadata
            .pointer("/downloads/client_mappings")
            .ok_or_else(|| LoaderError::ApiError("游戏清单缺少 client_mappings".into()))?;
        let url = mapping
            .get("url")
            .and_then(|value| value.as_str())
            .ok_or_else(|| LoaderError::ApiError("游戏映射缺少下载地址".into()))?;
        let sha1 = mapping
            .get("sha1")
            .and_then(|value| value.as_str())
            .filter(|value| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| LoaderError::ApiError("游戏映射缺少有效 SHA-1".into()))?;
        let size = mapping
            .get("size")
            .and_then(|value| value.as_u64())
            .filter(|size| *size > 0)
            .ok_or_else(|| LoaderError::ApiError("游戏映射大小无效".into()))?;
        let folder = destination
            .parent()
            .ok_or_else(|| LoaderError::ApiError("映射目录无效".into()))?
            .to_path_buf();
        let item = DownloadItem {
            url: url.to_string(),
            path: destination,
            folder,
            name: "client_mappings".into(),
            size,
            r#type: Some("forge-lib".into()),
            sha1: Some(sha1.to_string()),
        };
        Downloader::new(
            options.timeout_secs,
            options.clamped_concurrency(),
            options.force_ipv4,
            options.dns,
        )
        .download_multiple(vec![item], event_tx.clone())
        .await
        .map_err(|error| LoaderError::ApiError(error.to_string()))?;
        completed.push(index);
    }
    if let Some(processors) = profile.processors.as_mut() {
        for index in completed.into_iter().rev() {
            processors.remove(index);
        }
    }
    Ok(())
}

fn is_mojang_mapping_download(jar: &str, args: &[String]) -> bool {
    (jar.starts_with("net.minecraftforge:installertools:")
        || jar.starts_with("net.neoforged.installertools:installertools:"))
        && args
            .windows(2)
            .any(|pair| pair[0] == "--task" && pair[1] == "DOWNLOAD_MOJMAPS")
}

/// Deserialize `install_profile.json` from inside the installer JAR.
///
/// New-format profiles (1.13+) have a top-level `"version": "<id>"` string
/// that conflicts with `ForgeProfile.version: Option<ForgeVersionSection>`.
/// We strip it before deserializing; the version ID is obtained separately
/// via `read_installer_version_id`.
async fn read_install_profile(installer_path: &str) -> Result<ForgeProfile, LoaderError> {
    let result = get_file_from_archive(
        PathBuf::from(installer_path),
        Some("install_profile.json".into()),
        None,
        false,
    )
    .await
    .map_err(|error| LoaderError::Archive(error.to_string()))?;
    let bytes = match result {
        ArchiveQueryResult::FileData(bytes) => bytes,
        _ => return Err(LoaderError::ProfileNotFound),
    };
    let mut raw: serde_json::Value = serde_json::from_slice(&bytes)?;
    let minecraft = raw
        .get("minecraft")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    if let Some(object) = raw.as_object_mut() {
        if object
            .get("version")
            .is_some_and(serde_json::Value::is_string)
        {
            object.remove("version");
        }
    }
    let mut profile: ForgeProfile = serde_json::from_value(raw)?;
    let data = profile.data.get_or_insert_with(HashMap::new);
    if let Some(minecraft) = minecraft {
        data.insert(
            "MINECRAFT_VERSION".into(),
            crate::models::loader::ProfileDataEntry {
                client: format!("'{minecraft}'"),
                server: None,
            },
        );
    }
    data.insert(
        "INSTALLER".into(),
        crate::models::loader::ProfileDataEntry {
            client: format!("'{installer_path}'"),
            server: None,
        },
    );
    Ok(profile)
}

/// Extract `version.json` from the installer JAR to `dest_path`.
/// Install old-format Forge (pre-1.13): write versionInfo as the version JSON
/// and extract the bundled universal JAR into the loader's Maven libs tree.
/// No processors exist in this format; the universal JAR IS the Forge runtime.
async fn install_old_forge_legacy(
    installer_path: &str,
    loader_base: &Path,
    version_json_path: &Path,
    profile: &ForgeProfile,
    event_tx: &Sender<LaunchEvent>,
) -> Result<(), LoaderError> {
    let version_info = profile.version_info.as_ref().expect("caller checked Some");

    if let Some(parent) = version_json_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(version_json_path, serde_json::to_vec_pretty(version_info)?).await?;
    let _ = event_tx
        .send(LaunchEvent::Patch(
            "[patcher] Old-format Forge: wrote version JSON".into(),
        ))
        .await;

    if let Some(install) = &profile.install {
        if let (Some(file_in_zip), Some(maven_coord)) = (&install.file_path, &install.path) {
            if let Ok(lib_info) = get_path_libraries(maven_coord, None, None) {
                let dest = loader_base
                    .join("libraries")
                    .join(&lib_info.path)
                    .join(&lib_info.name);
                if !dest.exists() {
                    let result = get_file_from_archive(
                        PathBuf::from(installer_path),
                        Some(file_in_zip.clone()),
                        None,
                        false,
                    )
                    .await
                    .map_err(|e| LoaderError::Archive(e.to_string()))?;

                    if let ArchiveQueryResult::FileData(bytes) = result {
                        if let Some(parent) = dest.parent() {
                            tokio::fs::create_dir_all(parent).await?;
                        }
                        tokio::fs::write(&dest, bytes).await?;
                        let _ = event_tx
                            .send(LaunchEvent::Patch(format!(
                                "[patcher] Old-format Forge: extracted {}",
                                lib_info.name
                            )))
                            .await;
                    }
                }
            }
        }
    }

    Ok(())
}

async fn extract_version_json(installer_path: &str, dest_path: &Path) -> Result<(), LoaderError> {
    let profile_bytes = match get_file_from_archive(
        PathBuf::from(installer_path),
        Some("install_profile.json".into()),
        None,
        false,
    )
    .await
    .map_err(|error| LoaderError::Archive(error.to_string()))?
    {
        ArchiveQueryResult::FileData(bytes) => bytes,
        _ => return Err(LoaderError::ProfileNotFound),
    };
    let profile: serde_json::Value = serde_json::from_slice(&profile_bytes)?;
    let entry = profile
        .get("json")
        .and_then(|value| value.as_str())
        .unwrap_or("/version.json");
    let entry =
        crate::utils::archive::validate_relative_path(entry.strip_prefix('/').unwrap_or(entry))
            .map_err(LoaderError::Archive)?;
    let bytes = match get_file_from_archive(PathBuf::from(installer_path), Some(entry), None, false)
        .await
        .map_err(|error| LoaderError::Archive(error.to_string()))?
    {
        ArchiveQueryResult::FileData(bytes) => bytes,
        _ => return Err(LoaderError::ProfileNotFound),
    };
    let _: ForgeVersionSection = serde_json::from_slice(&bytes)?;
    if let Some(parent) = dest_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(dest_path, bytes).await?;
    Ok(())
}

/// Extract all `maven/` entries from the installer JAR into `libs_dir`.
///
/// Forge installers bundle processor JARs under `maven/` using Maven layout.
/// We extract them so the patcher can find them without extra downloads.
async fn extract_maven_entries(installer_path: &str, libs_dir: &Path) -> Result<(), LoaderError> {
    let installer = PathBuf::from(installer_path);

    let names = match get_file_from_archive(installer.clone(), None, Some("maven/".into()), false)
        .await
        .map_err(|e| LoaderError::Archive(e.to_string()))?
    {
        ArchiveQueryResult::Names(n) => n,
        _ => return Ok(()),
    };

    for name in names {
        let rel = match name.strip_prefix("maven/") {
            Some(r) if !r.is_empty() => r.to_owned(),
            _ => continue,
        };

        let dest = libs_dir.join(&rel);
        if dest.exists() {
            continue;
        }

        let bytes = match get_file_from_archive(installer.clone(), Some(name), None, false)
            .await
            .map_err(|e| LoaderError::Archive(e.to_string()))?
        {
            ArchiveQueryResult::FileData(b) => b,
            _ => continue,
        };

        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&dest, &bytes).await?;
    }

    Ok(())
}

/// Download processor JARs from `profile.libraries` that aren't already on disk.
///
/// JARs bundled inside the installer (extracted by `extract_maven_entries`) are
/// skipped; only entries with a non-empty URL that are still missing are fetched.
async fn download_profile_libraries(
    profile: &ForgeProfile,
    libs_dir: &Path,
    options: &LaunchOptions,
    event_tx: &Sender<LaunchEvent>,
) -> Result<(), LoaderError> {
    let mut items = Vec::new();
    let mut destinations = std::collections::HashSet::new();
    for library in profile.libraries.as_deref().unwrap_or(&[]) {
        let Some(artifact) = library
            .downloads
            .as_ref()
            .and_then(|downloads| downloads.artifact.as_ref())
        else {
            continue;
        };
        if artifact.url.is_empty() {
            continue;
        }
        let relative = if let Some(path) = &artifact.path {
            path.clone()
        } else {
            let info = get_path_libraries(&library.name, None, None)
                .map_err(|error| LoaderError::ApiError(error.to_string()))?;
            format!("{}/{}", info.path, info.name)
        };
        let relative = crate::utils::archive::validate_relative_path(&relative)
            .map_err(LoaderError::Archive)?;
        let destination = libs_dir.join(relative);
        // NeoForge profile 会为多个处理器重复列出同一库；不能并发提交同一个目标文件。
        if !destinations.insert(destination.clone()) {
            continue;
        }
        if destination.is_file() && artifact.sha1.is_none() {
            continue;
        }
        let folder = destination.parent().unwrap_or(libs_dir).to_path_buf();
        let name = destination
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        items.push(DownloadItem {
            url: artifact.url.clone(),
            path: destination,
            folder,
            name,
            size: artifact.size.unwrap_or(0),
            sha1: artifact.sha1.clone(),
            r#type: Some("forge-lib".into()),
        });
    }
    Downloader::new(
        options.timeout_secs,
        options.clamped_concurrency(),
        options.force_ipv4,
        options.dns,
    )
    .download_multiple(items, event_tx.clone())
    .await
    .map_err(|error| LoaderError::ApiError(error.to_string()))
}

/// 将 data 中的归档内资源放入 libraries 下，与处理器参数解析共用相对路径。
async fn extract_data_files(
    installer_path: &str,
    profile: &ForgeProfile,
    libs_dir: &Path,
) -> Result<(), LoaderError> {
    for entry in profile
        .data
        .as_ref()
        .into_iter()
        .flat_map(|data| data.values())
    {
        let Some(name) = entry.client.strip_prefix('/') else {
            continue;
        };
        let relative =
            crate::utils::archive::validate_relative_path(name).map_err(LoaderError::Archive)?;
        let destination = libs_dir.join(relative);
        let bytes = match get_file_from_archive(
            PathBuf::from(installer_path),
            Some(name.to_string()),
            None,
            false,
        )
        .await
        .map_err(|error| LoaderError::Archive(error.to_string()))?
        {
            ArchiveQueryResult::FileData(bytes) => bytes,
            _ => {
                return Err(LoaderError::Archive(format!(
                    "安装器缺少 data 条目：{name}"
                )));
            }
        };
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(destination, bytes).await?;
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_versions_include_new_builds_in_numeric_order() {
        let xml = "<metadata><versioning><versions><version>1.20.1-47.9.0</version><version>1.20.1-47.10.0</version><version>1.20.1-47.2.0</version></versions></versioning></metadata>";
        let versions = crate::loader::neoforge::parse_maven_xml_versions(xml);
        assert_eq!(versions.last().map(String::as_str), Some("1.20.1-47.10.0"));
    }

    fn test_options(root: &Path, version: &str) -> LaunchOptions {
        serde_json::from_value(serde_json::json!({"path":root, "version":version,
            "authenticator":{"accessToken":"", "name":"test", "uuid":"test"}}))
        .unwrap()
    }

    #[tokio::test]
    async fn duplicate_processor_dependencies_download_once_and_reuse_verified_file() {
        use sha1::Digest;
        use std::io::Write;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        archive
            .start_file("test.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"processor dependency").unwrap();
        let body = archive.finish().unwrap().into_inner();
        Mock::given(wiremock::matchers::path("/library"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .expect(1)
            .mount(&server)
            .await;
        let root = tempfile::TempDir::new().unwrap();
        let library = serde_json::json!({"name":"test:library:1", "downloads":{"artifact":{
            "url":format!("{}/library", server.uri()), "path":"test/library/1/library-1.jar",
            "sha1":format!("{:x}", sha1::Sha1::digest(&body)), "size":body.len()
        }}});
        let profile: ForgeProfile =
            serde_json::from_value(serde_json::json!({"libraries":[library.clone(),library]}))
                .unwrap();
        let options = test_options(root.path(), "1.20.4");
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        download_profile_libraries(&profile, root.path(), &options, &tx)
            .await
            .unwrap();
        download_profile_libraries(&profile, root.path(), &options, &tx)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("test/library/1/library-1.jar")).unwrap(),
            body
        );
    }

    fn installer_fixture(path: &Path, profile: &serde_json::Value, entries: &[(&str, &[u8])]) {
        use std::io::Write;
        let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("install_profile.json", options).unwrap();
        archive
            .write_all(&serde_json::to_vec(profile).unwrap())
            .unwrap();
        for (name, bytes) in entries {
            archive.start_file(*name, options).unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap();
    }

    #[tokio::test]
    async fn old_inline_and_new_spec_zero_forge_install_without_java_fallback() {
        for legacy in [true, false] {
            let root = tempfile::TempDir::new().unwrap();
            let installer = root.path().join("installer.jar");
            let loader = root.path().join("loader");
            let version = loader.join("versions/forge/forge.json");
            let bundled =
                "net/minecraftforge/forge/1.12.2-14.23.5.2859/forge-1.12.2-14.23.5.2859.jar";
            let profile = if legacy {
                serde_json::json!({
                    "install":{"filePath":"forge.jar", "path":"net.minecraftforge:forge:1.12.2-14.23.5.2859"},
                    "versionInfo":{"id":"forge", "mainClass":"net.minecraft.launchwrapper.Launch"}
                })
            } else {
                serde_json::json!({
                    "spec":0, "version":"forge", "json":"/version.json", "minecraft":"1.12.2", "processors":[],
                    "data":{"BINPATCH":{"client":"/data/client.lzma"}}
                })
            };
            let maven_entry = format!("maven/{bundled}");
            installer_fixture(
                &installer,
                &profile,
                &[
                    ("forge.jar", b"runtime"),
                    (&maven_entry, b"runtime"),
                    ("data/client.lzma", b"patch"),
                    (
                        "version.json",
                        b"{\"id\":\"forge\",\"mainClass\":\"net.minecraft.launchwrapper.Launch\"}",
                    ),
                ],
            );
            let options = test_options(root.path(), "1.12.2");
            let (tx, _rx) = tokio::sync::mpsc::channel(16);
            assert!(
                try_patcher_install(
                    installer.to_str().unwrap(),
                    &loader,
                    &version,
                    "unused.jar",
                    "unused.json",
                    "java-must-not-be-launched",
                    root.path(),
                    &options,
                    LoaderType::Forge,
                    false,
                    &tx
                )
                .await
                .unwrap()
            );
            assert_eq!(
                std::fs::read(loader.join("libraries").join(bundled)).unwrap(),
                b"runtime"
            );
            assert!(version.is_file());
            if !legacy {
                assert_eq!(
                    std::fs::read(loader.join("libraries/data/client.lzma")).unwrap(),
                    b"patch"
                );
            }
        }
    }

    #[tokio::test]
    async fn forge_and_neoforge_mappings_use_managed_download_and_verified_cache() {
        use sha1::Digest;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let body = b"official mappings";
        Mock::given(wiremock::matchers::path("/mappings"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .expect(1)
            .mount(&server)
            .await;
        let root = tempfile::TempDir::new().unwrap();
        let metadata = root.path().join("1.20.1.json");
        std::fs::write(&metadata, serde_json::to_vec(&serde_json::json!({"id":"1.20.1", "downloads":{"client_mappings":{
            "url":format!("{}/mappings", server.uri()), "size":body.len(), "sha1":format!("{:x}", sha1::Sha1::digest(body))
        }}})).unwrap()).unwrap();
        let options = test_options(root.path(), "1.20.1");
        let patcher = ForgePatcher::new(root.path().to_path_buf(), LoaderType::Forge);
        let config = PatchConfig {
            java_path: "unused",
            minecraft_jar: "unused",
            minecraft_json: metadata.to_str().unwrap(),
            game_path: root.path(),
        };
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        for jar in [
            "net.minecraftforge:installertools:1.4.1",
            "net.neoforged.installertools:installertools:2.1.2",
        ] {
            let mut profile: ForgeProfile = serde_json::from_value(serde_json::json!({
                "data":{"MOJMAPS":{"client":"[net.minecraft:client:1.20.1:mappings@txt]"}},
                "processors":[{"jar":jar, "classpath":[], "args":["--task","DOWNLOAD_MOJMAPS","--version","1.20.1","--side","{SIDE}","--output","{MOJMAPS}"]}]
            })).unwrap();
            prepare_mojang_mappings(&mut profile, &patcher, &config, &options, &tx)
                .await
                .unwrap();
            assert!(profile.processors.unwrap().is_empty());
        }
        assert_eq!(
            std::fs::read(
                root.path()
                    .join("libraries/net/minecraft/client/1.20.1/client-1.20.1-mappings.txt")
            )
            .unwrap(),
            body
        );
    }

    #[tokio::test]
    #[ignore = "通过 NICKEL_INSTALLER_SAMPLES 指定已经下载的安装器样例目录"]
    async fn real_installer_profiles_preserve_managed_contract() {
        let sample_dir = PathBuf::from(
            std::env::var_os("NICKEL_INSTALLER_SAMPLES").expect("explicit local samples"),
        );
        for name in ["forge-1.12.2", "forge-1.20.1", "neoforge-20.4"] {
            let installer = sample_dir.join(format!("{name}-sample.jar"));
            let root = tempfile::TempDir::new().unwrap();
            let options = test_options(root.path(), "1.12.2");
            let (tx, _rx) = tokio::sync::mpsc::channel(16);
            if name == "forge-1.12.2" {
                assert!(
                    try_patcher_install(
                        installer.to_str().unwrap(),
                        root.path(),
                        &root.path().join("version.json"),
                        "unused",
                        "unused",
                        "java-must-not-be-launched",
                        root.path(),
                        &options,
                        LoaderType::Forge,
                        false,
                        &tx
                    )
                    .await
                    .unwrap()
                );
                continue;
            }
            let profile = read_install_profile(installer.to_str().unwrap())
                .await
                .unwrap();
            extract_data_files(
                installer.to_str().unwrap(),
                &profile,
                &root.path().join("libraries"),
            )
            .await
            .unwrap();
            let config = PatchConfig {
                java_path: "unused",
                minecraft_jar: "client.jar",
                minecraft_json: "version.json",
                game_path: root.path(),
            };
            let patcher = ForgePatcher::new(root.path().to_path_buf(), LoaderType::Forge);
            let processors = profile.processors.as_ref().unwrap();
            assert!(
                processors
                    .iter()
                    .any(|processor| is_mojang_mapping_download(&processor.jar, &processor.args))
            );
            for processor in processors.iter().filter(|processor| {
                crate::loader::forge_patcher::is_client_side(processor.sides.as_deref())
            }) {
                for argument in &processor.args {
                    let resolved = patcher.resolve_argument(argument, &profile, &config);
                    assert!(
                        !resolved.contains('{')
                            && !resolved.starts_with('[')
                            && !resolved.starts_with('\''),
                        "{name}: {resolved}"
                    );
                }
            }
            assert!(root.path().join("libraries/data/client.lzma").is_file());
        }
    }

    #[tokio::test]
    #[ignore = "显式 NICKEL_INSTALLER_SAMPLES / NICKEL_INSTALLER_E2E；使用已有 Java 并下载真实游戏输入"]
    async fn real_modern_installer_processor_chain() {
        use crate::net::transfer::{CancellationToken, SourcePolicy, TransferContext};
        let sample_dir = PathBuf::from(
            std::env::var_os("NICKEL_INSTALLER_SAMPLES").expect("explicit local samples"),
        );
        let name = std::env::var("NICKEL_INSTALLER_E2E").expect("explicit sample name");
        let (mc, kind) = match name.as_str() {
            "forge-1.20.1" => ("1.20.1", LoaderType::Forge),
            "neoforge-20.4" => ("1.20.4", LoaderType::NeoForge),
            _ => panic!("unknown sample"),
        };
        let root = sample_dir.join(format!("managed-install-{name}"));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let options = test_options(&root, mc);
        let client = crate::net::client::build_transfer_client(30, false, None).unwrap();
        let context = TransferContext::new(
            SourcePolicy::OfficialAuto,
            SourcePolicy::OfficialAuto,
            CancellationToken::new(),
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let logger = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let LaunchEvent::Patch(line) = event {
                    println!("{line}");
                }
            }
        });
        let result = context
            .scope(tokio::time::timeout(
                std::time::Duration::from_secs(240),
                async {
                    let metadata = root.join(format!("{mc}.json"));
                    let version = if metadata.is_file() {
                        serde_json::from_slice::<crate::models::minecraft::MinecraftVersionJson>(
                            &tokio::fs::read(&metadata).await.unwrap(),
                        )
                        .unwrap()
                    } else {
                        let version = crate::game::version::get_version_json(&options, &client)
                            .await
                            .unwrap();
                        tokio::fs::write(&metadata, serde_json::to_vec(&version).unwrap())
                            .await
                            .unwrap();
                        version
                    };
                    let artifact = &version.downloads.as_ref().unwrap().client;
                    let jar = root.join(format!("{mc}.jar"));
                    Downloader::new(30, 10, false, None)
                        .download_multiple(
                            vec![DownloadItem {
                                url: artifact.url.clone(),
                                path: jar.clone(),
                                folder: root.clone(),
                                name: format!("{mc}.jar"),
                                size: artifact.size,
                                sha1: Some(artifact.sha1.clone()),
                                r#type: Some("client".into()),
                            }],
                            tx.clone(),
                        )
                        .await
                        .unwrap();
                    let installer = sample_dir.join(format!("{name}-sample.jar"));
                    assert!(
                        try_patcher_install(
                            installer.to_str().unwrap(),
                            &root,
                            &root.join("installed.json"),
                            jar.to_str().unwrap(),
                            metadata.to_str().unwrap(),
                            "java",
                            &root,
                            &options,
                            kind.clone(),
                            false,
                            &tx
                        )
                        .await
                        .unwrap()
                    );
                    let profile = read_install_profile(installer.to_str().unwrap())
                        .await
                        .unwrap();
                    let patcher = ForgePatcher::new(root.clone(), kind);
                    let config = PatchConfig {
                        java_path: "java",
                        minecraft_jar: jar.to_str().unwrap(),
                        minecraft_json: metadata.to_str().unwrap(),
                        game_path: &root,
                    };
                    let patched = patcher.resolve_argument("{PATCHED}", &profile, &config);
                    assert!(
                        Path::new(&patched).is_file(),
                        "missing final patched jar: {patched}"
                    );
                    crate::utils::archive::validate_zip_file(
                        Path::new(&patched),
                        context.cancellation(),
                    )
                    .unwrap();
                },
            ))
            .await;
        context.cancellation().cancel();
        drop(tx);
        logger.await.unwrap();
        result.expect("managed installer exceeded 240s");
    }

    #[tokio::test]
    #[ignore = "显式 NICKEL_INSTALLER_SAMPLES；定向下载真实 NeoForge profile 的两个小依赖"]
    async fn real_neoforge_dependency_candidates() {
        use crate::net::transfer::{self, CancellationToken, SourcePolicy, TransferContext};
        let sample_dir = PathBuf::from(
            std::env::var_os("NICKEL_INSTALLER_SAMPLES").expect("explicit local samples"),
        );
        let installer = sample_dir.join("neoforge-20.4-sample.jar");
        let profile = read_install_profile(installer.to_str().unwrap())
            .await
            .unwrap();
        let root = tempfile::TempDir::new().unwrap();
        let client = crate::net::client::build_transfer_client(5, false, None).unwrap();
        let context = TransferContext::new(
            SourcePolicy::OfficialFirst,
            SourcePolicy::OfficialFirst,
            CancellationToken::new(),
        );
        let mut outcomes = Vec::new();
        for artifact_name in ["installertools-2.1.2.jar", "asm-analysis-9.3.jar"] {
            let artifact = profile
                .libraries
                .as_ref()
                .unwrap()
                .iter()
                .filter_map(|library| {
                    library
                        .downloads
                        .as_ref()
                        .and_then(|downloads| downloads.artifact.as_ref())
                })
                .find(|artifact| {
                    artifact
                        .path
                        .as_ref()
                        .is_some_and(|path| path.ends_with(artifact_name))
                })
                .unwrap();
            let candidates = transfer::core_candidates(&artifact.url);
            assert_eq!(candidates.len(), 2);
            assert!(!candidates[0].is_mirror);
            assert!(candidates[1].is_mirror);
            let official = reqwest::Url::parse(&candidates[0].url).unwrap();
            let mirror = reqwest::Url::parse(&candidates[1].url).unwrap();
            assert_eq!(
                mirror.path(),
                format!(
                    "/maven{}",
                    official.path().strip_prefix("/releases").unwrap()
                )
            );
            println!(
                "[candidate] {artifact_name}: official={} mirror={}",
                official, mirror
            );
            for candidate in &candidates {
                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(12),
                    client
                        .get(&candidate.url)
                        .header(reqwest::header::ACCEPT_ENCODING, "identity")
                        .send(),
                )
                .await;
                match result {
                    Ok(Ok(response)) => println!(
                        "[candidate] {}: status={} final_origin={} length={:?}",
                        candidate.diagnostic_label(),
                        response.status(),
                        response.url().origin().ascii_serialization(),
                        response.content_length()
                    ),
                    Ok(Err(error)) => println!(
                        "[candidate] {}: {}",
                        candidate.diagnostic_label(),
                        crate::net::client::describe_reqwest_error(&error)
                    ),
                    Err(_) => println!(
                        "[candidate] {}: request timeout",
                        candidate.diagnostic_label()
                    ),
                }
            }
            let destination = root.path().join(artifact_name);
            let item = DownloadItem {
                url: artifact.url.clone(),
                path: destination.clone(),
                folder: root.path().to_path_buf(),
                name: artifact_name.into(),
                size: artifact.size.unwrap_or(0),
                sha1: artifact.sha1.clone(),
                r#type: Some("forge-lib".into()),
            };
            let started = std::time::Instant::now();
            let result = Downloader::with_context(5, 1, false, None, context.child())
                .download_file(&item)
                .await;
            println!(
                "[official-first] {artifact_name} elapsed={:?}: {result:?}",
                started.elapsed()
            );
            if result.is_ok() {
                assert_eq!(
                    std::fs::metadata(destination).unwrap().len(),
                    artifact.size.unwrap()
                );
            }
            outcomes.push(result);
        }
        assert!(
            outcomes.iter().all(Result::is_ok),
            "some real candidates failed; see per-item diagnostics"
        );
    }

    #[test]
    fn resolves_legacy_build_suffix_without_embedded_snapshot() {
        let versions = vec!["1.8.9-11.15.1.2318-1.8.9".to_string()];
        assert_eq!(
            match_promo_in_versions("1.8.9-11.15.1.2318", "1.8.9", &versions),
            versions[0]
        );
    }

    #[test]
    fn explicit_builds_skip_metadata_but_legacy_short_builds_keep_lookup() {
        assert_eq!(
            explicit_forge_coordinate("1.20.1", "47.4.28")
                .unwrap()
                .as_deref(),
            Some("1.20.1-47.4.28")
        );
        assert_eq!(
            explicit_forge_coordinate("1.20.1", "1.20.1-47.4.28")
                .unwrap()
                .as_deref(),
            Some("1.20.1-47.4.28")
        );
        assert_eq!(
            explicit_forge_coordinate("1.8.9", "11.15.1.2318").unwrap(),
            None
        );
        assert_eq!(
            explicit_forge_coordinate("1.8.9", "11.15.1.2318-1.8.9")
                .unwrap()
                .as_deref(),
            Some("1.8.9-11.15.1.2318-1.8.9")
        );
        assert_eq!(explicit_forge_coordinate("1.20.1", "latest").unwrap(), None);
        assert_eq!(
            explicit_forge_coordinate("1.20.1", "recommended").unwrap(),
            None
        );
        assert!(explicit_forge_coordinate("1.20.1", "../escape").is_err());
    }

    #[test]
    fn forge_mc_constructs() {
        let _f = ForgeMC::new();
    }

    #[test]
    fn build_library_assets_uses_explicit_artifact_path() {
        let version = ForgeVersionSection {
            id: Some("1.20.1-forge-47.4.20".into()),
            libraries: Some(vec![LoaderLibrary {
                name: "cpw.mods:bootstraplauncher:1.1.2".into(),
                url: None,
                downloads: Some(crate::models::loader::LoaderLibraryDownloads {
                    artifact: Some(crate::models::loader::LoaderArtifact {
                        sha1: Some("abc".into()),
                        size: Some(123),
                        path: Some(
                            "cpw/mods/bootstraplauncher/1.1.2/bootstraplauncher-1.1.2.jar".into(),
                        ),
                        url: "https://example.com/x.jar".into(),
                    }),
                }),
                rules: None,
                clientreq: None,
            }]),
            main_class: None,
            minecraft_arguments: None,
            arguments: None,
            extra: HashMap::new(),
        };
        let base = PathBuf::from("/mc/loader/forge");
        let items = build_library_assets(&base, &version);
        assert_eq!(items.len(), 1);
        match &items[0] {
            AssetItem::Asset { path, .. } => {
                assert!(path.ends_with("bootstraplauncher-1.1.2.jar"), "got {path}");
                assert!(Path::new(path).starts_with(base.join("libraries").join("cpw/mods")));
            }
            _ => panic!("expected Asset"),
        }
    }

    #[test]
    fn build_library_assets_skips_rule_restricted() {
        let version = ForgeVersionSection {
            id: None,
            libraries: Some(vec![LoaderLibrary {
                name: "x:y:1".into(),
                url: None,
                downloads: None,
                rules: Some(vec![serde_json::json!({"action":"disallow"})]),
                clientreq: None,
            }]),
            main_class: None,
            minecraft_arguments: None,
            arguments: None,
            extra: HashMap::new(),
        };
        let items = build_library_assets(Path::new("/mc/loader/forge"), &version);
        assert!(items.is_empty());
    }
}
