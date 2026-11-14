use tokio::sync::mpsc::Sender;

use crate::error::LoaderError;
use crate::launcher::events::LaunchEvent;
use crate::launcher::options::LaunchOptions;
use crate::models::loader::{FabricJson, FabricMeta, LoaderType};
use crate::models::minecraft::AssetItem;
use crate::net::downloader::{DownloadItem, Downloader};
use crate::net::http::fetch_json;
use crate::utils::paths::get_path_libraries;

// ── Constants ─────────────────────────────────────────────────────────────────

const FABRIC_META: &str = "https://meta.fabricmc.net/v2/versions";
const FABRIC_PROFILE: &str =
    "https://meta.fabricmc.net/v2/versions/loader/${version}/${build}/profile/json";

const LEGACY_META: &str = "https://meta.legacyfabric.net/v2/versions";
const LEGACY_PROFILE: &str =
    "https://meta.legacyfabric.net/v2/versions/loader/${version}/${build}/profile/json";

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FabricVariant {
    Modern,
    Legacy,
}
pub struct FabricMC {
    variant: FabricVariant,
}

/// 与 Fabric API 的版本 ID 一致，列表与 profile 共用此映射。
pub fn normalize_minecraft_version(version: &str) -> String {
    match version {
        "Combat Test 7c" => "1.16_combat-3".into(),
        other => other.replace('∞', "infinite"),
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

impl FabricMC {
    pub fn new(variant: FabricVariant) -> Self {
        Self { variant }
    }

    pub fn loader_type(&self) -> LoaderType {
        match self.variant {
            FabricVariant::Modern => LoaderType::Fabric,
            FabricVariant::Legacy => LoaderType::LegacyFabric,
        }
    }

    /// Fetch the Fabric/LegacyFabric loader profile JSON for the given Minecraft version.
    ///
    /// `build` can be `"latest"`, `"recommended"` (treated same as latest for Fabric),
    /// or an exact version string.
    pub async fn download_json(
        &self,
        mc_version: &str,
        build: &str,
        client: &reqwest::Client,
    ) -> Result<FabricJson, LoaderError> {
        let (meta_url, profile_template) = match self.variant {
            FabricVariant::Modern => (FABRIC_META, FABRIC_PROFILE),
            FabricVariant::Legacy => (LEGACY_META, LEGACY_PROFILE),
        };

        let mc_version = normalize_minecraft_version(mc_version);
        let build_ver = if matches!(build, "latest" | "recommended") {
            let context = crate::net::transfer::TransferContext::current();
            let meta: FabricMeta = crate::net::transfer::fetch_validated_candidates(
                client, &context, &crate::net::http::service_key(meta_url),
                &crate::net::transfer::core_candidates(meta_url), crate::net::transfer::DEFAULT_LIST_LIMIT,
                |bytes| {
                    let meta: FabricMeta = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
                    if !meta.game.iter().any(|game| game.version == mc_version) || meta.loader.is_empty() {
                        return Err(format!("清单中没有 Minecraft {mc_version} 的可用构建"));
                    }
                    Ok(meta)
                },
            ).await.map_err(LoaderError::ApiError)?;
            meta.loader.first().map(|build| build.version.clone())
                .ok_or_else(|| LoaderError::VersionNotFound(mc_version.clone()))?
        } else {
            if build.is_empty() || build.contains("..") || !build.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-+".contains(&byte)) {
                return Err(LoaderError::VersionNotFound("加载器构建号无效".into()));
            }
            // 新选择已由应用目录复核；profile 本身的实际请求就是最终验证。
            build.to_owned()
        };

        let profile_url = profile_template
            .replace("${version}", &mc_version)
            .replace("${build}", &build_ver);

        let json: FabricJson = fetch_json(client, &profile_url)
            .await
            .map_err(LoaderError::ApiError)?;

        Ok(json)
    }

    /// Download any libraries from `fabric_json` that are not yet on disk.
    ///
    /// Emits `LaunchEvent::Check` for each library processed.
    /// Returns `Vec<AssetItem>` covering every library (for bundle-integrity tracking).
    pub async fn download_libraries(
        &self,
        options: &LaunchOptions,
        fabric_json: &FabricJson,
        _client: &reqwest::Client,
        event_tx: &Sender<LaunchEvent>,
    ) -> Result<Vec<AssetItem>, LoaderError> {
        let libs = &fabric_json.libraries;
        let total = libs.len();
        let mut items: Vec<AssetItem> = Vec::with_capacity(total);
        let mut pending: Vec<DownloadItem> = Vec::new();

        for (idx, lib) in libs.iter().enumerate() {
            let _ = event_tx
                .send(LaunchEvent::Check {
                    current: idx + 1,
                    total,
                    kind: "libraries".into(),
                })
                .await;

            // Skip libraries with OS rules — same as the JS loader.
            if lib.rules.is_some() {
                continue;
            }

            let lib_info = match get_path_libraries(&lib.name, None, None) {
                Ok(i) => i,
                Err(_) => continue,
            };

            let loader_name = match self.variant {
                FabricVariant::Modern => "fabric",
                FabricVariant::Legacy => "legacyfabric",
            };
            let folder = options
                .loader_dir(loader_name)
                .join("libraries")
                .join(&lib_info.path);
            let dest = folder.join(&lib_info.name);

            let url = resolve_lib_url(lib, &lib_info.path, &lib_info.name);

            items.push(AssetItem::Asset {
                path: dest.to_string_lossy().into_owned(),
                sha1: lib
                    .downloads
                    .as_ref()
                    .and_then(|d| d.artifact.as_ref())
                    .and_then(|a| a.sha1.clone())
                    .unwrap_or_default(),
                size: lib
                    .downloads
                    .as_ref()
                    .and_then(|d| d.artifact.as_ref())
                    .and_then(|a| a.size)
                    .unwrap_or(0),
                url: url.clone(),
            });

            if !dest.exists() {
                pending.push(DownloadItem {
                    url,
                    path: dest,
                    folder,
                    name: lib_info.name,
                    size: 0,
                    r#type: Some("libraries".into()),
                    sha1: None,
                });
            }
        }

        if !pending.is_empty() {
            let downloader = Downloader::new(
                options.timeout_secs,
                options.clamped_concurrency(),
                options.force_ipv4,
                options.dns,
            );
            downloader
                .download_multiple(pending, event_tx.clone())
                .await
                .map_err(|e| {
                    LoaderError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    ))
                })?;
        }

        Ok(items)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub(crate) fn resolve_lib_url(
    lib: &crate::models::loader::LoaderLibrary,
    rel_path: &str,
    name: &str,
) -> String {
    // Prefer explicit download URL from the downloads section.
    if let Some(url) = lib
        .downloads
        .as_ref()
        .and_then(|d| d.artifact.as_ref())
        .map(|a| a.url.as_str())
    {
        return url.to_owned();
    }

    // Build URL from the base Maven repo URL.
    let base = lib
        .url
        .as_deref()
        .unwrap_or("https://repo1.maven.org/maven2/");
    let base = base.trim_end_matches('/');
    format!("{base}/{rel_path}/{name}")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fabric_variant_modern_and_legacy_differ() {
        assert_ne!(FabricVariant::Modern, FabricVariant::Legacy);
    }


    #[test]
    fn special_game_ids_match_profile_and_catalog() {
        assert_eq!(normalize_minecraft_version("20w14∞"), "20w14infinite");
        assert_eq!(normalize_minecraft_version("Combat Test 7c"), "1.16_combat-3");
        assert_eq!(normalize_minecraft_version("26.1"), "26.1");
    }


    #[test]
    fn resolve_lib_url_uses_downloads_url_when_present() {
        use crate::models::loader::{LoaderArtifact, LoaderLibraryDownloads};
        let lib = crate::models::loader::LoaderLibrary {
            name: "a:b:1.0".into(),
            url: Some("https://repo.example.com/".into()),
            downloads: Some(LoaderLibraryDownloads {
                artifact: Some(LoaderArtifact {
                    sha1: None,
                    size: None,
                    path: None,
                    url: "https://direct.example.com/b-1.0.jar".into(),
                }),
            }),
            rules: None,
            clientreq: None,
        };
        let url = resolve_lib_url(&lib, "a/b/1.0", "b-1.0.jar");
        assert_eq!(url, "https://direct.example.com/b-1.0.jar");
    }

    #[test]
    fn resolve_lib_url_constructs_from_base_url() {
        let lib = crate::models::loader::LoaderLibrary {
            name: "a:b:1.0".into(),
            url: Some("https://maven.fabricmc.net/".into()),
            downloads: None,
            rules: None,
            clientreq: None,
        };
        let url = resolve_lib_url(&lib, "a/b/1.0", "b-1.0.jar");
        assert_eq!(url, "https://maven.fabricmc.net/a/b/1.0/b-1.0.jar");
    }

    #[test]
    fn resolve_lib_url_falls_back_to_maven_central() {
        let lib = crate::models::loader::LoaderLibrary {
            name: "a:b:1.0".into(),
            url: None,
            downloads: None,
            rules: None,
            clientreq: None,
        };
        let url = resolve_lib_url(&lib, "a/b/1.0", "b-1.0.jar");
        assert!(url.contains("repo1.maven.org"));
        assert!(url.contains("b-1.0.jar"));
    }
}
