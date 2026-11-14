use std::path::{Path, PathBuf};

use tokio::sync::mpsc::Sender;

use crate::error::LoaderError;
use crate::launcher::events::LaunchEvent;
use crate::launcher::options::LaunchOptions;
use crate::loader::forge::{download_resolved_installer, try_patcher_install};
use crate::models::loader::{
    ForgeVersionSection, InstallerInfo, LoaderArtifactKind, LoaderLibrary, LoaderType,
};
use crate::models::minecraft::AssetItem;
use crate::net::downloader::{DownloadItem, Downloader};
use crate::utils::archive::{ArchiveQueryResult, get_file_from_archive};
use crate::utils::paths::get_path_libraries;

// ── Constants ─────────────────────────────────────────────────────────────────

const LEGACY_META_URL: &str =
    "https://maven.neoforged.net/releases/net/neoforged/forge/maven-metadata.xml";
const NEW_META_URL: &str =
    "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";

const LEGACY_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/forge";
const NEW_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";

// ── XML maven-metadata.xml parser ─────────────────────────────────────────────

pub(crate) fn parse_maven_xml_versions(xml: &str) -> Vec<String> {
    let Some((_, tail)) = xml.split_once("<versions>") else {
        return Vec::new();
    };
    let Some((versions, _)) = tail.split_once("</versions>") else {
        return Vec::new();
    };
    let mut versions: Vec<String> = versions
        .split("<version>")
        .skip(1)
        .filter_map(|tail| tail.split_once("</version>"))
        .map(|(version, _)| version.trim())
        .filter(|v| {
            !v.is_empty()
                && !v.contains("..")
                && v.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-+".contains(&c))
        })
        .map(str::to_string)
        .collect();
    versions.sort_by(|a, b| {
        let numbers = |v: &str| {
            v.split(|c: char| !c.is_ascii_digit())
                .filter_map(|s| s.parse::<u64>().ok())
                .collect::<Vec<_>>()
        };
        let (a_base, a_pre) = a.split_once('-').unwrap_or((a, ""));
        let (b_base, b_pre) = b.split_once('-').unwrap_or((b, ""));
        numbers(a_base)
            .cmp(&numbers(b_base))
            .then_with(|| a_pre.is_empty().cmp(&b_pre.is_empty()))
            .then_with(|| numbers(a_pre).cmp(&numbers(b_pre)))
            .then_with(|| a.cmp(b))
    });
    versions.dedup();
    versions
}

// ── Public API ────────────────────────────────────────────────────────────────

pub struct NeoForgeMC;

impl NeoForgeMC {
    pub fn new() -> Self {
        Self
    }

    /// 使用受管下载准备 NeoForge 安装依赖，再执行本地 Java 处理器。
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
        let loader_base = options.loader_dir("neoforge");
        tokio::fs::create_dir_all(&loader_base).await?;

        let installer = self
            .download_installer(options, mc_version, build, client, event_tx)
            .await?;

        let version_id = read_installer_version_id(&installer.file_path).await?;
        let version_json_path = loader_base
            .join("versions")
            .join(&version_id)
            .join(format!("{version_id}.json"));

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
                LoaderType::NeoForge,
                installer.old_api,
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
                    "NeoForge installer finished but no version JSON was created at {}",
                    version_json_path.display()
                )));
            }
        }

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

    /// Download the NeoForge installer JAR.
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
                LoaderArtifactKind::NeoForge,
                "neoforge",
                mc_version == "1.20.1",
                event_tx,
            )
            .await;
        }

        // 1.20.1 使用历史 forge artifact；不要把网络错误当作不存在而切到新坐标。
        let old_api = mc_version == "1.20.1";
        let (metadata, maven_base, artifact_prefix, prefix) = if old_api {
            (
                LEGACY_META_URL,
                LEGACY_MAVEN,
                "forge",
                format!("{mc_version}-"),
            )
        } else {
            (
                NEW_META_URL,
                NEW_MAVEN,
                "neoforge",
                make_short_prefix(mc_version),
            )
        };
        let requested =
            if old_api && !matches!(build, "latest" | "recommended") && !build.starts_with(&prefix)
            {
                format!("{prefix}{build}")
            } else {
                build.to_string()
            };
        let chosen = if matches!(build, "latest" | "recommended") {
            let context = crate::net::transfer::TransferContext::current();
            let versions: Vec<String> = crate::net::transfer::fetch_validated_candidates(
                client,
                &context,
                &crate::net::http::service_key(metadata),
                &crate::net::transfer::core_candidates(metadata),
                crate::net::transfer::DEFAULT_LIST_LIMIT,
                |bytes| {
                    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
                    let versions: Vec<_> = parse_maven_xml_versions(text)
                        .into_iter()
                        .filter(|version| matches_minecraft_version(mc_version, version))
                        .collect();
                    if versions.is_empty() {
                        return Err(format!("清单缺少 Minecraft {mc_version} 构建"));
                    }
                    Ok(versions)
                },
            )
            .await
            .map_err(LoaderError::ApiError)?;
            resolve_neo_build(&requested, &versions)?
        } else {
            if requested.is_empty()
                || requested.contains("..")
                || !requested
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-+".contains(&byte))
                || !matches_minecraft_version(mc_version, &requested)
            {
                return Err(LoaderError::VersionNotFound(format!(
                    "NeoForge 构建与游戏版本不匹配：{requested}"
                )));
            }
            // 显式构建已由目录入口复核；保持原始 Maven ID，离线复用无需再次拉目录。
            requested
        };
        let installer_name = format!("{artifact_prefix}-{chosen}-installer.jar");
        let installer_folder = options.loader_dir("neoforge").join("installer");
        let installer_path = installer_folder.join(&installer_name);
        if !installer_path.exists() {
            let item = DownloadItem {
                url: format!("{maven_base}/{chosen}/{installer_name}"),
                path: installer_path.clone(),
                folder: installer_folder,
                name: installer_name,
                size: 0,
                r#type: Some("neoforge".into()),
                sha1: None,
            };
            Downloader::new(options.timeout_secs, 1, options.force_ipv4, options.dns)
                .download_multiple(vec![item], event_tx.clone())
                .await
                .map_err(|e| LoaderError::Io(std::io::Error::other(e.to_string())))?;
        }
        Ok(InstallerInfo {
            file_path: installer_path.to_string_lossy().into_owned(),
            meta_data: chosen.clone(),
            ext: "jar".into(),
            id: format!("neoforge-{chosen}"),
            old_api,
        })
    }
}

impl Default for NeoForgeMC {
    fn default() -> Self {
        Self::new()
    }
}

// ── Installer driver (identical to Forge) ─────────────────────────────────────

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

    if let Some(v) = raw.get("version").and_then(|v| v.as_str()) {
        return Ok(v.to_owned());
    }
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

async fn read_version_json(path: &Path) -> Result<ForgeVersionSection, LoaderError> {
    let bytes = tokio::fs::read(path).await?;
    let version: ForgeVersionSection = serde_json::from_slice(&bytes)?;
    Ok(version)
}

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

fn build_library_assets(loader_base: &Path, version: &ForgeVersionSection) -> Vec<AssetItem> {
    let libs = version.libraries.as_deref().unwrap_or(&[]);
    let mut items: Vec<AssetItem> = Vec::with_capacity(libs.len());

    for lib in libs {
        if lib.rules.is_some() {
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
    let url = artifact.map(|a| a.url.clone()).unwrap_or_default();

    (abs_path.to_string_lossy().into_owned(), sha1, size, url)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// 精确匹配稳定版本与带 +snapshot 标记的原始构建名。
pub fn matches_minecraft_version(mc_version: &str, build: &str) -> bool {
    if mc_version == "1.20.1" {
        return build.starts_with("1.20.1-");
    }
    if let Some((base, number)) = mc_version.split_once("-snapshot-") {
        let suffix = format!("+snapshot-{number}");
        return !number.is_empty()
            && build.starts_with(&make_short_prefix(base))
            && build.ends_with(&suffix);
    }
    if mc_version.bytes().any(|byte| byte.is_ascii_alphabetic()) {
        return build.starts_with(&format!("0.{mc_version}."));
    }
    build.starts_with(&make_short_prefix(mc_version)) && !build.contains("+snapshot")
}

fn make_short_prefix(mc_version: &str) -> String {
    let parts: Vec<&str> = mc_version.splitn(3, '.').collect();
    let major = parts.first().copied().unwrap_or("1");
    let minor = parts.get(1).copied().unwrap_or("0");
    let patch = parts.get(2).copied().unwrap_or("0");
    if major == "1" {
        format!("{minor}.{patch}.")
    } else {
        // 26.1 与 26.1.2 分别对应 26.1.0.* 和 26.1.2.*，不能丢弃补丁号。
        format!("{major}.{minor}.{patch}.")
    }
}

fn resolve_neo_build(build: &str, versions: &[String]) -> Result<String, LoaderError> {
    match build {
        "latest" => versions
            .last()
            .cloned()
            .ok_or_else(|| LoaderError::VersionNotFound("No NeoForge builds available".into())),
        "recommended" => versions
            .iter()
            .rev()
            .find(|v| {
                !["alpha", "beta", "rc", "+snapshot"]
                    .iter()
                    .any(|marker| v.contains(marker))
            })
            .cloned()
            .or_else(|| versions.last().cloned())
            .ok_or_else(|| LoaderError::VersionNotFound("No stable NeoForge build found".into())),
        specific => versions
            .iter()
            .find(|v| v.as_str() == specific)
            .cloned()
            .ok_or_else(|| {
                LoaderError::VersionNotFound(format!("NeoForge build {specific} not found"))
            }),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_short_prefix_splits_correctly() {
        assert_eq!(make_short_prefix("1.20.4"), "20.4.");
        assert_eq!(make_short_prefix("1.21.0"), "21.0.");
        assert_eq!(make_short_prefix("1.21"), "21.0.");
        assert_eq!(make_short_prefix("26.1"), "26.1.0.");
        assert_eq!(make_short_prefix("26.1.2"), "26.1.2.");
        assert_eq!(make_short_prefix("26.2.0"), "26.2.0.");
    }

    #[test]
    fn original_snapshot_and_legacy_coordinates_are_matched_exactly() {
        assert!(matches_minecraft_version("1.20.1", "1.20.1-47.1.106"));
        assert!(matches_minecraft_version("1.20.4", "20.4.237"));
        assert!(matches_minecraft_version(
            "26.1-snapshot-1",
            "26.1.0.0-alpha.1+snapshot-1"
        ));
        assert!(!matches_minecraft_version(
            "26.1",
            "26.1.0.0-alpha.1+snapshot-1"
        ));
        assert!(!matches_minecraft_version(
            "26.1-snapshot-1",
            "26.1.0.0-alpha.1+snapshot-11"
        ));
        assert!(matches_minecraft_version(
            "25w14craftmine",
            "0.25w14craftmine.4-beta"
        ));
        assert!(!matches_minecraft_version(
            "1.21.4",
            "0.25w14craftmine.4-beta"
        ));
    }

    #[test]
    fn resolve_neo_build_latest() {
        let versions = vec!["20.4.1".into(), "20.4.2".into(), "20.4.3-beta".into()];
        let result = resolve_neo_build("latest", &versions).unwrap();
        assert_eq!(result, "20.4.3-beta");
    }

    #[test]
    fn resolve_neo_build_recommended_skips_beta() {
        let versions = vec!["20.4.1".into(), "20.4.2".into(), "20.4.3-beta".into()];
        let result = resolve_neo_build("recommended", &versions).unwrap();
        assert_eq!(result, "20.4.2");
    }

    #[test]
    fn resolve_neo_build_specific() {
        let versions = vec!["20.4.1".into(), "20.4.2".into()];
        let result = resolve_neo_build("20.4.1", &versions).unwrap();
        assert_eq!(result, "20.4.1");
    }

    #[test]
    fn resolve_neo_build_specific_not_found() {
        let versions = vec!["20.4.1".into()];
        assert!(resolve_neo_build("99.9.9", &versions).is_err());
    }

    #[test]
    fn parse_maven_xml_versions_extracts_all() {
        let xml = "<metadata><versioning><versions>\
                   <version>1.0</version><version>1.1</version>\
                   </versions></versioning></metadata>";
        let v = parse_maven_xml_versions(xml);
        assert_eq!(v, vec!["1.0", "1.1"]);
    }

    #[test]
    fn neo_forge_mc_constructs() {
        let _n = NeoForgeMC::new();
    }
}
