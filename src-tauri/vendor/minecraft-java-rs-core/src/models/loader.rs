use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Loader type enum ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LoaderType {
    Forge,
    NeoForge,
    Fabric,
    // serde rename_all = "lowercase" produces "legacyfabric" ✓
    LegacyFabric,
    Quilt,
    OptiFine,
    LiteLoader,
}

impl std::fmt::Display for LoaderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LoaderType::Forge => "forge",
            LoaderType::NeoForge => "neoforge",
            LoaderType::Fabric => "fabric",
            LoaderType::LegacyFabric => "legacyfabric",
            LoaderType::Quilt => "quilt",
            LoaderType::OptiFine => "optifine",
            LoaderType::LiteLoader => "liteloader",
        };
        f.write_str(s)
    }
}

// ── Common loader library ────────────────────────────────────────────────────
// Used by Fabric, Quilt, Forge, and NeoForge installer JSONs.
// Simpler than the Mojang `Library` type: no native classifiers, and `rules`
// is treated as opaque (we only check presence, not content).

#[derive(Debug, Deserialize, Serialize)]
pub struct LoaderLibrary {
    pub name: String,
    /// Base Maven repository URL (Fabric: `"https://maven.fabricmc.net"`).
    #[serde(default)]
    pub url: Option<String>,
    pub downloads: Option<LoaderLibraryDownloads>,
    /// When present (any value), this library should be skipped.
    #[serde(default)]
    pub rules: Option<Vec<Value>>,
    /// Old-format Forge only: `false` means this library is server-only.
    #[serde(default)]
    pub clientreq: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LoaderLibraryDownloads {
    pub artifact: Option<LoaderArtifact>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LoaderArtifact {
    pub sha1: Option<String>,
    pub size: Option<u64>,
    pub path: Option<String>,
    pub url: String,
}

// ── Resolved application artifact ─────────────────────────────────────────────
// 应用层目录、另存安装器与自动安装共用同一解析结果；核心不再从 mc+build 猜 URL。
// 字段刻意区分展示 ID、目录查询 ID、安装坐标 ID 和上游原始 API 名，避免把
// Forge 的 `- -> _`、Fabric 别名或 OptiFine 的 `.0` 修正混成全局字符串替换。

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LoaderArtifactKind {
    Forge,
    NeoForge,
    Fabric,
    OptiFine,
    LiteLoader,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactCategory {
    Installer,
    Universal,
    Client,
    Profile,
    Jar,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ResolvedLoaderArtifact {
    /// 加载器种类；核心只消费 Forge / NeoForge 的安装器字段。
    pub kind: LoaderArtifactKind,
    /// UI 展示的加载器版本，不重复 MC 前缀。
    pub display_version: String,
    /// 目录选择与缓存身份；必须能在同一 MC 下唯一定位构建。
    pub catalog_version: String,
    /// Maven / 安装器实际使用的版本坐标。
    pub installer_version: String,
    /// 上游清单返回的原始构建名，例如 NeoForge 的 `1.20.1-47.1.99`。
    pub api_name: String,
    /// 兼容旧调用方的版本别名；内容与 `api_name` 一致。
    pub api_version: String,
    /// Maven 实际使用的 fileVersion；Forge 分支后缀必须保留。
    pub file_version: String,
    /// UI 展示的 Minecraft ID。
    pub display_minecraft: String,
    /// 目录查询使用的 Minecraft ID（Forge 为 `- -> _` 后的值）。
    pub catalog_minecraft: String,
    /// 安装器坐标使用的 Minecraft ID。
    pub installer_minecraft: String,
    /// 上游 API 返回的原始 Minecraft ID。
    pub api_minecraft: String,
    pub category: ArtifactCategory,
    /// 自动安装输入：Forge/NeoForge 为安装器，Fabric/LiteLoader 为 profile，
    /// OptiFine 为原始 Jar。
    pub file_name: String,
    pub url: String,
    pub sha1: Option<String>,
    pub md5: Option<String>,
    pub size: Option<u64>,
    /// 另存安装器的独立来源；`None` 表示与自动安装输入相同。
    /// Fabric/LiteLoader 的 profile 与安装器地址不同，必须分开保存。
    pub save_file_name: Option<String>,
    pub save_url: Option<String>,
    pub save_sha1: Option<String>,
    pub save_md5: Option<String>,
    pub save_size: Option<u64>,
    /// 是否可以自动安装。
    pub installable: bool,
    /// 是否可以另存为独立安装器。
    pub save_installable: bool,
    /// 安装器内 install_profile.json 声明的版本目录（若已知）。
    pub version_id: Option<String>,
    /// 首个候选的来源语义（official / bmclapi / derived）。
    pub source: String,
    /// 可回退的具体传输地址；顺序保留提供方语义，不去重合并清单。
    pub download_candidates: Vec<String>,
    /// 提供方原始条目，供 LiteLoader 合成 profile、OptiFine 兼容判断使用。
    pub raw_metadata: Option<Value>,
    /// 已合成的加载器 profile；LiteLoader 直接消费，Forge 系保持为空。
    pub profile: Option<Value>,
}

impl ResolvedLoaderArtifact {
    /// 自动安装候选：显式候选优先，旧调用方仍回落到 `url`。
    pub fn download_urls(&self) -> Vec<&str> {
        let mut urls = self
            .download_candidates
            .iter()
            .map(String::as_str)
            .filter(|url| !url.is_empty())
            .collect::<Vec<_>>();
        if urls.is_empty() && !self.url.is_empty() {
            urls.push(&self.url);
        }
        urls
    }
}

impl ResolvedLoaderArtifact {
    pub fn save_source(&self) -> (&str, &str) {
        (
            self.save_url.as_deref().unwrap_or(&self.url),
            self.save_file_name.as_deref().unwrap_or(&self.file_name),
        )
    }
}

impl Default for LoaderArtifactKind {
    fn default() -> Self {
        Self::Forge
    }
}

// ── Fabric / LegacyFabric / Quilt ────────────────────────────────────────────

/// Response from `meta.fabricmc.net/v2/versions` (and equivalents for Quilt/LegacyFabric).
#[derive(Debug, Deserialize)]
pub struct FabricMeta {
    pub game: Vec<FabricGameVersion>,
    pub loader: Vec<FabricLoaderBuild>,
}

#[derive(Debug, Deserialize)]
pub struct FabricGameVersion {
    pub version: String,
    pub stable: bool,
}

#[derive(Debug, Deserialize)]
pub struct FabricLoaderBuild {
    pub version: String,
    pub stable: bool,
}

/// Quilt's loader build has extra fields and no `stable` flag.
#[derive(Debug, Deserialize)]
pub struct QuiltLoaderBuild {
    pub version: String,
    #[serde(default)]
    pub stable: bool,
}

#[derive(Debug, Deserialize)]
pub struct QuiltMeta {
    pub game: Vec<FabricGameVersion>,
    pub loader: Vec<QuiltLoaderBuild>,
}

/// Profile JSON fetched from the Fabric/Quilt meta API
/// (e.g. `meta.fabricmc.net/v2/versions/loader/{mc}/{build}/profile/json`).
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabricJson {
    pub id: String,
    #[serde(default)]
    pub libraries: Vec<LoaderLibrary>,
    pub main_class: Option<String>,
    pub minecraft_arguments: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ── Forge / NeoForge ─────────────────────────────────────────────────────────
// `install_profile.json` is highly dynamic. Critical fields are typed;
// the rest is captured in `extra` to round-trip without data loss.

/// Variable entry in the Forge profile `data` map.
/// Each variable has a `client` value (and sometimes `server`).
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProfileDataEntry {
    pub client: String,
    #[serde(default)]
    pub server: Option<String>,
}

/// `arguments` block inside a Forge/NeoForge version JSON (1.13+ format).
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct ForgeArguments {
    #[serde(default)]
    pub game: Vec<serde_json::Value>,
    #[serde(default)]
    pub jvm: Vec<serde_json::Value>,
}

/// Top-level `install_profile.json`. Dual format:
/// - **Old format** (pre-1.13): has a top-level `install` key → the profile IS `install`.
/// - **New format** (1.13+): `install` and `version` are separate sections.
#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct ForgeProfile {
    pub install: Option<ForgeInstallSection>,
    pub version: Option<ForgeVersionSection>,
    /// Variable map used by processors (`{BINPATCH}`, `{MAPPINGS}`, etc.).
    /// Present in new-format (1.13+) profiles.
    pub data: Option<HashMap<String, ProfileDataEntry>>,
    /// Path within the installer ZIP to the primary artifact (old format).
    pub file_path: Option<String>,
    /// Maven coordinate of the primary Forge artifact (old format).
    pub path: Option<String>,
    pub processors: Option<Vec<ForgeProcessor>>,
    pub libraries: Option<Vec<LoaderLibrary>>,
    /// Old-format (pre-1.13) Forge: the full version JSON embedded inline.
    #[serde(rename = "versionInfo", default)]
    pub version_info: Option<ForgeVersionSection>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ForgeInstallSection {
    pub libraries: Option<Vec<LoaderLibrary>>,
    /// Name of the version JSON file inside the installer JAR.
    pub json: Option<String>,
    pub path: Option<String>,
    pub file_path: Option<String>,
    pub minecraft: Option<String>,
    pub processors: Option<Vec<ForgeProcessor>>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ForgeVersionSection {
    pub id: Option<String>,
    pub libraries: Option<Vec<LoaderLibrary>>,
    pub main_class: Option<String>,
    pub minecraft_arguments: Option<String>,
    /// Modern 1.13+ argument block (game/jvm arrays).
    pub arguments: Option<ForgeArguments>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// A Forge installer processor: a Java tool that patches/transforms Minecraft files.
#[derive(Debug, Deserialize, Serialize)]
pub struct ForgeProcessor {
    pub jar: String,
    pub classpath: Vec<String>,
    pub args: Vec<String>,
    #[serde(default)]
    pub sides: Option<Vec<String>>,
}

// ── Internal result types (not from JSON) ────────────────────────────────────

/// Result of a successful installer JAR download.
#[derive(Debug)]
pub struct InstallerInfo {
    pub file_path: String,
    pub meta_data: String,
    pub ext: String,
    pub id: String,
    /// NeoForge only: whether to use the legacy (1.20.1-era) API path.
    pub old_api: bool,
}
