//! 加载器完整清单、可用性与受限安装器地址。列表策略不改变正在进行的下载。
//!
//! 目录行、安装器另存与自动安装共用 `resolve_artifact` 的结果；OptiFine 与
//! LiteLoader 的 `installable` 直接反映已接通的 LaunchWrapper / Installer /
//! 合成 profile 路线，不能用“可另存”代替自动安装能力。

#[path = "loader_artifact.rs"]
pub(crate) mod artifact;

#[path = "loader_install_optifine.rs"]
pub(crate) mod optifine_install;

#[path = "loader_install_liteloader.rs"]
pub(crate) mod liteloader_install;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use minecraft_java_rs_core::mirror;
use minecraft_java_rs_core::models::loader::{LoaderArtifactKind, ResolvedLoaderArtifact};
use minecraft_java_rs_core::net::client::{build_client, describe_reqwest_error};
use minecraft_java_rs_core::net::transfer::{
    CancellationToken, RequestCandidate, SourcePolicy, TransferContext, fetch_validated_candidates,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::loader_availability::{
    fabric_api_id, forge_api_id, is_known_broken_neoforge, neoforge_minecraft_id, query_plan,
};

const QUERY_TIMEOUT_SECS: u64 = 10;
const CATALOG_TIMEOUT_SECS: u64 = 35;
const CACHE_TTL: Duration = Duration::from_secs(600);
const METADATA_MAX_BYTES: usize = 8 * 1024 * 1024;
const VERSION_MAX_LEN: usize = 128;
const FORGE_HTML: &str = "https://files.minecraftforge.net/net/minecraftforge/forge";
const FORGE_PROMOTIONS: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
const NEOFORGE_API: &str = "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged";
const FABRIC_META: &str = "https://meta.fabricmc.net/v2/versions";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoaderCapability {
    pub install: bool,
    pub save_installer: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderState {
    Hidden,
    Loading,
    Available,
    #[default]
    Unavailable,
    Failed,
    ManualOnly,
    Incompatible,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoaderProvider {
    pub state: ProviderState,
    pub capability: LoaderCapability,
    /// 与对应加载器数组长度严格一致；前端计数不得再按 capability 二次过滤。
    pub count: usize,
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoaderBuild {
    pub version: String,
    pub label: String,
    pub release_time: String,
    pub modified_time: String,
    pub stable: Option<bool>,
    pub latest: bool,
    pub recommended: bool,
    pub download_url: String,
    pub changelog_url: String,
    /// 目录、另存与自动安装共用的解析结果；不可用的构建不提供 artifact。
    pub artifact: Option<ResolvedLoaderArtifact>,
    pub capability: LoaderCapability,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderCatalog {
    pub forge: Vec<LoaderBuild>,
    pub neoforge: Vec<LoaderBuild>,
    pub fabric: Vec<LoaderBuild>,
    pub liteloader: Vec<LoaderBuild>,
    pub optifine: Vec<LoaderBuild>,
    pub notes: BTreeMap<String, String>,
    pub providers: BTreeMap<String, LoaderProvider>,
}

/// 全量目录成功后缓存索引；Fabric 的构建全局共用，不为每个 MC 复制一份。
#[derive(Debug, Clone, Default)]
struct CatalogIndex {
    games: BTreeSet<String>,
    builds: BTreeMap<String, Vec<LoaderBuild>>,
    common: Vec<LoaderBuild>,
    promotions: Option<Value>,
    forge_files: Vec<artifact::ForgeBuildFiles>,
    fabric_installer: Option<artifact::FabricInstallerSource>,
}

impl CatalogIndex {
    fn for_game(&self, mc: &str) -> Vec<LoaderBuild> {
        self.builds.get(mc).cloned().unwrap_or_else(|| {
            if self.games.contains(mc) {
                self.common.clone()
            } else {
                Vec::new()
            }
        })
    }
}

#[derive(Default)]
struct CacheEntry<T> {
    success: Option<(Instant, Arc<T>)>,
    // 仅在同次在途请求的等待者之间复用错误；后续重试不会命中负缓存。
    completed: Option<(Instant, Result<Arc<T>, String>)>,
}

type CacheSlot = Arc<tokio::sync::Mutex<CacheEntry<CatalogIndex>>>;
static CATALOG_CACHE: LazyLock<Mutex<HashMap<String, CacheSlot>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Default)]
struct ProviderPayload {
    builds: Vec<LoaderBuild>,
    provider: LoaderProvider,
}

type ProviderCacheSlot = Arc<tokio::sync::Mutex<CacheEntry<ProviderPayload>>>;
static PROVIDER_CACHE: LazyLock<Mutex<HashMap<String, ProviderCacheSlot>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn cache_slot(key: &str) -> CacheSlot {
    CATALOG_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(key.to_string())
        .or_default()
        .clone()
}

fn provider_cache_slot(key: &str) -> ProviderCacheSlot {
    PROVIDER_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(key.to_string())
        .or_default()
        .clone()
}

/// 同一输入的在途请求共享一次网络任务；TTL 内成功结果直接复用。
async fn cached_value<T, F, Fut>(
    slot: Arc<tokio::sync::Mutex<CacheEntry<T>>>,
    refresh: bool,
    fetch: F,
) -> Result<(Arc<T>, bool), String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let entered = Instant::now();
    let mut entry = slot.lock().await;
    if let Some((completed, result)) = &entry.completed {
        if *completed >= entered {
            return result.clone().map(|value| (value, true));
        }
    }
    if !refresh {
        if let Some((saved, value)) = &entry.success {
            if saved.elapsed() < CACHE_TTL {
                return Ok((value.clone(), true));
            }
        }
    }
    let result = fetch().await.map(Arc::new);
    let now = Instant::now();
    if let Ok(value) = &result {
        entry.success = Some((now, value.clone()));
    }
    entry.completed = Some((now, result.clone()));
    result.map(|value| (value, false))
}

async fn cached_index<F, Fut>(
    slot: CacheSlot,
    refresh: bool,
    fetch: F,
) -> Result<Arc<CatalogIndex>, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<CatalogIndex, String>>,
{
    cached_value(slot, refresh, fetch)
        .await
        .map(|(value, _)| value)
}

async fn cached_provider<F, Fut>(
    slot: ProviderCacheSlot,
    refresh: bool,
    fetch: F,
) -> Result<(Arc<ProviderPayload>, bool), String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<ProviderPayload, String>>,
{
    cached_value(slot, refresh, fetch).await
}

/// 加载器构建用于 Maven 路径。MC 身份可能有空格/Unicode，不能使用此限制。
pub fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= VERSION_MAX_LEN
        && value != "."
        && !value.contains("..")
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-+".contains(&c))
}

pub fn source_policy(source: Option<&str>) -> Result<SourcePolicy, String> {
    match source.map(str::trim) {
        None => Ok(TransferContext::current().list_policy()),
        Some("official_first") => Ok(SourcePolicy::OfficialFirst),
        Some("mirror_first") => Ok(SourcePolicy::MirrorFirst),
        Some("official_auto") => Ok(SourcePolicy::OfficialAuto),
        _ => Err("源策略无效，请重新选择版本列表源".into()),
    }
}

/// 仅供安装器联网冒烟测试读取；生产请求统一经传输核心。
#[cfg(test)]
pub async fn fetch_bytes(
    url: &str,
    max_bytes: usize,
    timeout_secs: u64,
) -> Result<Vec<u8>, String> {
    tokio::time::timeout(Duration::from_secs(timeout_secs), async {
        let client =
            build_client(timeout_secs, false, None).map_err(|e| describe_reqwest_error(&e))?;
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|e| describe_reqwest_error(&e))?
            .error_for_status()
            .map_err(|e| describe_reqwest_error(&e))?;
        if response
            .content_length()
            .is_some_and(|size| size > max_bytes as u64)
        {
            return Err("服务器响应超过大小上限".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| describe_reqwest_error(&e))?
        {
            if bytes.len().saturating_add(chunk.len()) > max_bytes {
                return Err("服务器响应超过大小上限".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| "请求超时，请检查网络或切换下载源后重试".to_string())?
}

fn version_parts(version: &str) -> Vec<u64> {
    version
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| s.parse().ok())
        .collect()
}

fn compare_versions_desc(a: &str, b: &str) -> std::cmp::Ordering {
    let (a_base, a_pre) = a.split_once('-').unwrap_or((a, ""));
    let (b_base, b_pre) = b.split_once('-').unwrap_or((b, ""));
    version_parts(b_base)
        .cmp(&version_parts(a_base))
        .then_with(|| b_pre.is_empty().cmp(&a_pre.is_empty()))
        .then_with(|| version_parts(b_pre).cmp(&version_parts(a_pre)))
        .then_with(|| b.cmp(a))
}

fn normalize(mut builds: Vec<LoaderBuild>) -> Vec<LoaderBuild> {
    builds.retain(|b| valid_version(&b.version));
    builds.sort_by(|a, b| compare_versions_desc(&a.version, &b.version));
    builds.dedup_by(|a, b| a.version == b.version);
    if let Some(first) = builds.first_mut() {
        first.latest = true;
    }
    builds
}

fn build(version: &str) -> LoaderBuild {
    LoaderBuild {
        version: version.into(),
        label: version.into(),
        ..LoaderBuild::default()
    }
}

fn release_time(value: &Value) -> String {
    // modified 是同步/更新时间，不冒充发布日期。
    ["releaseTime", "published", "publishedAt"]
        .iter()
        .find_map(|key| value.get(key)?.as_str())
        .filter(|s| s.len() <= VERSION_MAX_LEN && !s.chars().any(char::is_control))
        .unwrap_or("")
        .to_string()
}

fn normalize_index(index: &mut CatalogIndex) {
    for rows in index.builds.values_mut() {
        *rows = normalize(std::mem::take(rows));
    }
    index.games.extend(index.builds.keys().cloned());
}

fn forge_row(mc: &str, version: &str, metadata: &Value) -> LoaderBuild {
    let mut row = build(version);
    row.release_time = release_time(metadata);
    row.modified_time = metadata
        .get("modified")
        .and_then(Value::as_str)
        .unwrap_or("")
        .into();
    let coordinate = format!("{}-{version}", forge_api_id(mc));
    row.changelog_url = format!("{FORGE_HTML}/{coordinate}/forge-{coordinate}-changelog.txt");
    row
}

/// 文件详情缺失时保持不可安装；只有真实 installer 才打开目录与另存能力。
fn attach_forge_artifact(
    display_mc: &str,
    row: &mut LoaderBuild,
    entry: Option<&artifact::ForgeBuildFiles>,
) {
    let Some(entry) = entry else {
        row.artifact = None;
        row.download_url.clear();
        row.capability = LoaderCapability::default();
        return;
    };
    let api_mc = if entry.minecraft.is_empty() {
        forge_api_id(display_mc)
    } else {
        forge_api_id(&entry.minecraft)
    };
    let Some(resolved) = artifact::forge_artifact(display_mc, &api_mc, &row.version, entry) else {
        row.artifact = None;
        row.download_url.clear();
        row.capability = LoaderCapability::default();
        return;
    };
    row.download_url = resolved.url.clone();
    row.capability = LoaderCapability {
        install: resolved.installable,
        save_installer: resolved.save_installable,
    };
    row.artifact = Some(resolved);
}

fn split_forge_coordinate(raw: &str) -> Option<(&str, &str)> {
    for (index, _) in raw.char_indices().filter(|(_, ch)| *ch == '-') {
        let (minecraft, version) = raw.split_at(index);
        let version = &version[1..];
        if !minecraft.is_empty()
            && version
                .chars()
                .next()
                .is_some_and(|value| value.is_ascii_digit())
        {
            return Some((minecraft, version));
        }
    }
    None
}

fn parse_forge_index(text: &str) -> Result<CatalogIndex, String> {
    if !text.contains("<metadata>") || !text.contains("</metadata>") {
        return Err("Forge Maven 清单格式不正确".into());
    }
    let section = text
        .split_once("<versions>")
        .and_then(|(_, s)| s.split_once("</versions>"))
        .ok_or("Forge Maven 清单缺少完整 versions 区段")?
        .0;
    let mut index = CatalogIndex::default();
    let mut count = 0;
    for tail in section.split("<version>").skip(1) {
        count += 1;
        let raw = tail
            .split_once("</version>")
            .ok_or("Forge 构建条目不完整")?
            .0
            .trim();
        let Some((mc, version)) = split_forge_coordinate(raw) else {
            continue;
        };
        let api_mc = forge_api_id(mc);
        if valid_version(&api_mc) && valid_version(version) {
            index
                .builds
                .entry(api_mc.clone())
                .or_default()
                .push(forge_row(&api_mc, version, &Value::Null));
        }
    }
    if count > 0 && index.builds.is_empty() {
        return Err("Forge 清单没有可识别的构建条目".into());
    }
    normalize_index(&mut index);
    Ok(index)
}

#[cfg(test)]
fn parse_forge(text: &str, mc: &str) -> Result<Vec<LoaderBuild>, String> {
    if text.trim_start().starts_with('<') {
        return Ok(parse_forge_index(text)?.for_game(&forge_api_id(mc)));
    }
    let entries = artifact::parse_forge_files(text, mc)?;
    let value: Value = serde_json::from_str(text).map_err(|_| "Forge 清单不是有效 JSON")?;
    let items = value.as_array().ok_or("Forge 清单格式不正确")?;
    let mut rows = Vec::new();
    for item in items {
        if item
            .get("mcversion")
            .and_then(Value::as_str)
            .is_some_and(|id| forge_api_id(id) != forge_api_id(mc))
        {
            continue;
        }
        let Some(raw_version) = item.get("version").and_then(Value::as_str) else {
            continue;
        };
        let mut version = raw_version.to_string();
        if let Some(branch) = item
            .get("branch")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            version.push('-');
            version.push_str(branch);
        }
        let mut row = forge_row(mc, &version, item);
        attach_forge_artifact(
            mc,
            &mut row,
            artifact::matching_forge_entry(&entries, &version),
        );
        rows.push(row);
    }
    Ok(normalize(rows))
}

fn mark_promotions(builds: &mut [LoaderBuild], value: &Value, mc: &str) -> Result<(), String> {
    let promos = value
        .get("promos")
        .and_then(Value::as_object)
        .ok_or("Forge 推荐标记格式不正确")?;
    let latest = promos.get(&format!("{mc}-latest")).and_then(Value::as_str);
    let recommended = promos
        .get(&format!("{mc}-recommended"))
        .and_then(Value::as_str);
    for item in builds {
        if let Some(latest) = latest {
            item.latest = item.version == latest;
        }
        item.recommended = recommended == Some(item.version.as_str());
        if item.recommended {
            item.stable = Some(true);
        }
    }
    Ok(())
}

fn parse_promotions(text: &str) -> Result<CatalogIndex, String> {
    let value: Value = serde_json::from_str(text).map_err(|_| "Forge 推荐标记不是有效 JSON")?;
    if !value.get("promos").is_some_and(Value::is_object) {
        return Err("Forge 推荐标记格式不正确".into());
    }
    Ok(CatalogIndex {
        promotions: Some(value),
        ..CatalogIndex::default()
    })
}

fn parse_neoforge_index(text: &str) -> Result<CatalogIndex, String> {
    let value: Value = serde_json::from_str(text).map_err(|_| "NeoForge 清单不是有效 JSON")?;
    let items = value
        .get("versions")
        .or_else(|| value.get("files"))
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .ok_or("NeoForge 完整清单格式不正确")?;
    let mut index = CatalogIndex::default();
    for item in items {
        if item
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "DIRECTORY")
        {
            continue;
        }
        let Some(raw) = item
            .as_str()
            .or_else(|| item.get("version").and_then(Value::as_str))
            .or_else(|| item.get("name").and_then(Value::as_str))
        else {
            continue;
        };
        let declared = item.get("mcversion").and_then(Value::as_str);
        let version = if declared == Some("1.20.1") && raw.starts_with("47.") {
            format!("1.20.1-{raw}")
        } else {
            raw.into()
        };
        if !valid_version(&version) || is_known_broken_neoforge(&version) {
            continue;
        }
        let inferred = neoforge_minecraft_id(&version);
        if declared.is_some_and(|mc| inferred.as_deref().is_some_and(|id| id != mc)) {
            continue;
        }
        let Some(mc) = declared.or(inferred.as_deref()) else {
            continue;
        };
        if !crate::named_profiles::valid_minecraft_version(mc) {
            continue;
        }
        let mut row = build(&version);
        row.release_time = release_time(item);
        row.modified_time = item
            .get("modified")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into();
        row.stable = Some(
            !["beta", "alpha", "rc", "+snapshot"]
                .iter()
                .any(|s| version.contains(s)),
        );
        let Some(resolved) = artifact::neoforge_artifact(mc, &version, &version) else {
            continue;
        };
        row.download_url = resolved.url.clone();
        row.capability = LoaderCapability {
            install: resolved.installable,
            save_installer: resolved.save_installable,
        };
        row.artifact = Some(resolved);
        index.builds.entry(mc.into()).or_default().push(row);
    }
    if !items.is_empty() && index.builds.is_empty() {
        return Err("NeoForge 清单没有可识别的构建条目".into());
    }
    normalize_index(&mut index);
    Ok(index)
}

fn parse_fabric_installer(value: &Value) -> Result<artifact::FabricInstallerSource, String> {
    let items = value.as_array().ok_or("Fabric 安装器清单格式不正确")?;
    let mut versions: Vec<&str> = items
        .iter()
        .filter(|v| v.get("stable").and_then(Value::as_bool) == Some(true))
        .filter_map(|v| v.get("version")?.as_str())
        .filter(|v| valid_version(v))
        .collect();
    versions.sort_by(|a, b| compare_versions_desc(a, b));
    let version = versions.first().ok_or("Fabric 暂无可用的独立稳定安装器")?;
    Ok(artifact::FabricInstallerSource {
        version: (*version).to_string(),
        url: format!(
            "{}/{version}/fabric-installer-{version}.jar",
            artifact::FABRIC_MAVEN
        ),
    })
}

fn parse_fabric_index(text: &str) -> Result<CatalogIndex, String> {
    let value: Value = serde_json::from_str(text).map_err(|_| "Fabric 清单不是有效 JSON")?;
    let games = value
        .get("game")
        .and_then(Value::as_array)
        .ok_or("Fabric 清单缺少 game")?;
    let loaders = value
        .get("loader")
        .and_then(Value::as_array)
        .ok_or("Fabric 清单缺少 loader")?;
    let installers = value
        .get("installer")
        .filter(|v| v.is_array())
        .ok_or("Fabric 清单缺少 installer")?;
    let installer = parse_fabric_installer(installers).ok();
    let mut index = CatalogIndex {
        fabric_installer: installer.clone(),
        ..CatalogIndex::default()
    };
    for game in games {
        let mc = game
            .get("version")
            .and_then(Value::as_str)
            .ok_or("Fabric game 条目无效")?;
        if crate::named_profiles::valid_minecraft_version(mc) {
            index.games.insert(mc.into());
        }
    }
    for loader in loaders {
        let version = loader
            .get("version")
            .and_then(Value::as_str)
            .ok_or("Fabric loader 条目无效")?;
        if !valid_version(version) {
            continue;
        }
        let mut row = build(version);
        row.stable = loader.get("stable").and_then(Value::as_bool);
        row.release_time = release_time(loader);
        row.changelog_url =
            format!("https://github.com/FabricMC/fabric-loader/releases/tag/{version}");
        row.capability = LoaderCapability {
            install: true,
            save_installer: installer.is_some(),
        };
        index.common.push(row);
    }
    if !loaders.is_empty() && index.common.is_empty() {
        return Err("Fabric 清单没有可识别的加载器条目".into());
    }
    index.common = normalize(index.common);
    Ok(index)
}

fn optifine_build(entry: optifine_install::OptiFineEntry) -> LoaderBuild {
    let artifact = optifine_install::artifact(&entry);
    LoaderBuild {
        version: artifact.catalog_version.clone(),
        label: entry.display_name,
        release_time: entry.release_time,
        modified_time: String::new(),
        stable: Some(!entry.is_preview),
        latest: false,
        recommended: false,
        download_url: artifact.url.clone(),
        changelog_url: String::new(),
        capability: LoaderCapability {
            install: artifact.installable,
            save_installer: artifact.save_installable,
        },
        artifact: Some(artifact),
    }
}

fn parse_optifine_index(text: &str) -> Result<CatalogIndex, String> {
    let mut index = CatalogIndex::default();
    for entry in optifine_install::parse_index(text)? {
        index
            .builds
            .entry(entry.minecraft_id.clone())
            .or_default()
            .push(optifine_build(entry));
    }
    normalize_index(&mut index);
    Ok(index)
}

fn liteloader_build(entry: liteloader_install::LiteLoaderEntry) -> LoaderBuild {
    let artifact = liteloader_install::artifact(&entry);
    LoaderBuild {
        version: artifact.catalog_version.clone(),
        label: entry.label,
        release_time: entry.release_time,
        modified_time: String::new(),
        stable: Some(entry.stable),
        latest: false,
        recommended: false,
        download_url: artifact.url.clone(),
        changelog_url: String::new(),
        capability: LoaderCapability {
            install: artifact.installable,
            save_installer: artifact.save_installable,
        },
        artifact: Some(artifact),
    }
}

fn parse_liteloader_index(text: &str) -> Result<CatalogIndex, String> {
    let mut index = CatalogIndex::default();
    for entry in liteloader_install::parse_versions(text)? {
        index
            .builds
            .entry(entry.minecraft_id.clone())
            .or_default()
            .push(liteloader_build(entry));
    }
    normalize_index(&mut index);
    Ok(index)
}

fn catalog_index_cache_key(
    policy: SourcePolicy,
    service: &str,
    official: &str,
    mirrored: Option<&str>,
) -> String {
    format!("{policy:?}|{service}|{official}|{}", mirrored.unwrap_or(""))
}

/// provider 结果缓存同时隔离加载器、MC、发布日期与源策略；force 只绕过本轮读取。
fn provider_cache_key(
    policy: SourcePolicy,
    loader: &str,
    mc: &str,
    release_time: Option<&str>,
) -> String {
    format!("{policy:?}|{loader}|{mc}|{}", release_time.unwrap_or(""))
}

async fn load_index(
    context: &TransferContext,
    service: &str,
    official: &str,
    mirrored: Option<String>,
    refresh: bool,
    parse: fn(&str) -> Result<CatalogIndex, String>,
) -> Result<Arc<CatalogIndex>, String> {
    let key = catalog_index_cache_key(
        context.list_policy(),
        service,
        official,
        mirrored.as_deref(),
    );
    cached_index(cache_slot(&key), refresh, || async {
        let client = build_client(QUERY_TIMEOUT_SECS, false, None)
            .map_err(|e| describe_reqwest_error(&e))?;
        let mut candidates = vec![RequestCandidate::official(official)];
        if let Some(mirror) = mirrored {
            candidates.push(RequestCandidate::mirror(mirror));
        }
        fetch_validated_candidates(
            &client,
            context,
            service,
            &candidates,
            METADATA_MAX_BYTES,
            |bytes| {
                let text = std::str::from_utf8(bytes).map_err(|_| "清单不是 UTF-8 文本")?;
                parse(text)
            },
        )
        .await
    })
    .await
}

async fn matching_builds(
    context: &TransferContext,
    service: &str,
    official: &str,
    mirrored: String,
    mc: &str,
    refresh: bool,
    parse: fn(&str) -> Result<CatalogIndex, String>,
) -> Result<Vec<LoaderBuild>, String> {
    let index = load_index(context, service, official, Some(mirrored), refresh, parse).await?;
    let builds = index.for_game(mc);
    if !builds.is_empty() {
        return Ok(builds);
    }
    // 镜像可能落后：缺条目必须回查官方。官方失败仍为失败，不能持久缓存成不支持。
    let official_index = load_index(context, service, official, None, refresh, parse).await?;
    Ok(official_index.for_game(mc))
}

async fn load_forge_files(
    context: &TransferContext,
    display_mc: &str,
    refresh: bool,
) -> Result<Arc<CatalogIndex>, String> {
    let api_mc = forge_api_id(display_mc);
    let official = format!("{FORGE_HTML}/index_{api_mc}.html");
    let mirrored = format!("{}/forge/minecraft/{api_mc}", mirror::BMCLAPI_BASE);
    let key = catalog_index_cache_key(
        context.list_policy(),
        "forge-files",
        &official,
        Some(&mirrored),
    );
    let expected = display_mc.to_string();
    cached_index(cache_slot(&key), refresh, || async move {
        let client = build_client(QUERY_TIMEOUT_SECS, false, None)
            .map_err(|e| describe_reqwest_error(&e))?;
        let candidates = [
            RequestCandidate::official(&official),
            RequestCandidate::mirror(&mirrored),
        ];
        fetch_validated_candidates(
            &client,
            context,
            "forge-file-index",
            &candidates,
            METADATA_MAX_BYTES,
            |bytes| {
                let text =
                    std::str::from_utf8(bytes).map_err(|_| "Forge 文件清单不是 UTF-8 文本")?;
                let files = artifact::parse_forge_files(text, &expected)?;
                Ok(CatalogIndex {
                    forge_files: files,
                    ..CatalogIndex::default()
                })
            },
        )
        .await
    })
    .await
}

async fn matching_index(
    context: &TransferContext,
    service: &str,
    official: &str,
    mirrored: String,
    mc: &str,
    refresh: bool,
    parse: fn(&str) -> Result<CatalogIndex, String>,
    empty_mirror_is_authoritative: bool,
) -> Result<Arc<CatalogIndex>, String> {
    let index = load_index(context, service, official, Some(mirrored), refresh, parse).await?;
    if !index.for_game(mc).is_empty() {
        return Ok(index);
    }
    // 镜像可能落后：缺条目必须回查官方。对全量列表提供方，镜像成功返回空集合
    // 已足以判定“该 MC 无条目”；官方补充查询失败不能把空结果升级成 Failed。
    let fallback = load_index(context, service, official, None, refresh, parse).await;
    prefer_empty_index(index, fallback, empty_mirror_is_authoritative)
}

fn prefer_empty_index(
    primary: Arc<CatalogIndex>,
    fallback: Result<Arc<CatalogIndex>, String>,
    empty_is_authoritative: bool,
) -> Result<Arc<CatalogIndex>, String> {
    match fallback {
        Ok(index) => Ok(index),
        Err(_) if empty_is_authoritative => Ok(primary),
        Err(error) => Err(error),
    }
}

fn attach_fabric_artifacts(index: &CatalogIndex, display_mc: &str, builds: &mut [LoaderBuild]) {
    for row in builds {
        let resolved =
            artifact::fabric_artifact(display_mc, &row.version, index.fabric_installer.as_ref());
        row.download_url = resolved.url.clone();
        row.capability = LoaderCapability {
            install: resolved.installable,
            save_installer: resolved.save_installable,
        };
        row.artifact = Some(resolved);
    }
}

async fn query_provider(
    context: &TransferContext,
    loader: &str,
    display_mc: &str,
    refresh: bool,
) -> Result<Vec<LoaderBuild>, String> {
    match loader {
        "forge" => {
            let api_mc = forge_api_id(display_mc);
            let metadata_url = format!("{}/maven-metadata.xml", artifact::FORGE_MAVEN);
            let (result, promotions) = tokio::join!(
                matching_builds(
                    context,
                    "forge-maven",
                    &metadata_url,
                    format!(
                        "{}/maven/net/minecraftforge/forge/maven-metadata.xml",
                        mirror::BMCLAPI_BASE
                    ),
                    &api_mc,
                    refresh,
                    parse_forge_index,
                ),
                load_index(
                    context,
                    "forge-promotions",
                    FORGE_PROMOTIONS,
                    Some(format!(
                        "{}/maven/net/minecraftforge/forge/promotions_slim.json",
                        mirror::BMCLAPI_BASE
                    )),
                    refresh,
                    parse_promotions,
                )
            );
            let mut builds = result?;
            if let Ok(index) = promotions {
                if let Some(value) = &index.promotions {
                    let _ = mark_promotions(&mut builds, value, &api_mc);
                }
            }
            if !builds.is_empty() {
                let files = load_forge_files(context, display_mc, refresh).await?;
                for row in &mut builds {
                    attach_forge_artifact(
                        display_mc,
                        row,
                        artifact::matching_forge_entry(&files.forge_files, &row.version),
                    );
                }
            }
            Ok(builds)
        }
        "neoforge" => {
            let artifact_name = if display_mc == "1.20.1" {
                "forge"
            } else {
                "neoforge"
            };
            matching_builds(
                context,
                "neoforge-maven",
                &format!("{NEOFORGE_API}/{artifact_name}"),
                format!(
                    "{}/neoforge/meta/api/maven/details/releases/net/neoforged/{artifact_name}",
                    mirror::BMCLAPI_BASE
                ),
                display_mc,
                refresh,
                parse_neoforge_index,
            )
            .await
        }
        "fabric" => {
            let api_mc = fabric_api_id(display_mc);
            let index = matching_index(
                context,
                "fabric-meta",
                FABRIC_META,
                format!("{}/fabric-meta/v2/versions", mirror::BMCLAPI_BASE),
                &api_mc,
                refresh,
                parse_fabric_index,
                false,
            )
            .await?;
            let mut builds = index.for_game(&api_mc);
            attach_fabric_artifacts(&index, display_mc, &mut builds);
            Ok(builds)
        }
        "optifine" => {
            let mc = optifine_install::canonical_minecraft_id(display_mc);
            let index = matching_index(
                context,
                "optifine-list",
                optifine_install::OPTIFINE_DOWNLOADS,
                optifine_install::BMCLAPI_VERSION_LIST.into(),
                &mc,
                refresh,
                parse_optifine_index,
                true,
            )
            .await?;
            Ok(index.for_game(&mc))
        }
        "liteloader" => {
            let index = matching_index(
                context,
                "liteloader-list",
                liteloader_install::LITELOADER_VERSIONS,
                liteloader_install::LITELOADER_BMCLAPI.into(),
                display_mc,
                refresh,
                parse_liteloader_index,
                true,
            )
            .await?;
            Ok(index.for_game(display_mc))
        }
        _ => Err("该加载器没有可用的安装提供方".into()),
    }
}

fn provider_metadata(builds: &[LoaderBuild]) -> LoaderProvider {
    let capability = LoaderCapability {
        install: builds.iter().any(|b| b.capability.install),
        save_installer: builds.iter().any(|b| b.capability.save_installer),
    };
    // 非空清单本身就可展示；capability 只控制单个构建能否安装/另存。
    let state = if builds.is_empty() {
        ProviderState::Unavailable
    } else if capability.install {
        ProviderState::Available
    } else if capability.save_installer {
        ProviderState::ManualOnly
    } else {
        ProviderState::Available
    };
    LoaderProvider {
        state,
        capability,
        count: builds.len(),
        error: String::new(),
    }
}

fn provider_result(result: Result<Vec<LoaderBuild>, String>) -> (Vec<LoaderBuild>, LoaderProvider) {
    match result {
        Err(error) => (
            Vec::new(),
            LoaderProvider {
                state: ProviderState::Failed,
                error,
                ..LoaderProvider::default()
            },
        ),
        Ok(builds) => {
            let provider = provider_metadata(&builds);
            (builds, provider)
        }
    }
}

/// 单 provider 冷/热路径共用同一缓存入口；35 秒截止只约束本 provider。
async fn load_provider_cached(
    context: &TransferContext,
    loader: &str,
    mc: &str,
    release_time: Option<&str>,
    refresh: bool,
    applicable: bool,
) -> Result<(Arc<ProviderPayload>, bool), String> {
    if !applicable {
        return Ok((
            Arc::new(ProviderPayload {
                builds: Vec::new(),
                provider: LoaderProvider {
                    state: ProviderState::Hidden,
                    ..LoaderProvider::default()
                },
            }),
            false,
        ));
    }
    let key = provider_cache_key(context.list_policy(), loader, mc, release_time);
    cached_provider(provider_cache_slot(&key), refresh, || async move {
        let result = tokio::time::timeout(
            Duration::from_secs(CATALOG_TIMEOUT_SECS),
            query_provider(context, loader, mc, refresh),
        )
        .await
        .unwrap_or_else(|_| Err("查询超时，请重试或更换版本列表源".into()));
        let builds = result?;
        let provider = provider_metadata(&builds);
        Ok(ProviderPayload { builds, provider })
    })
    .await
}

async fn bounded_provider(
    context: &TransferContext,
    loader: &str,
    mc: &str,
    release_time: Option<&str>,
    refresh: bool,
    applicable: bool,
) -> (Vec<LoaderBuild>, LoaderProvider) {
    match load_provider_cached(context, loader, mc, release_time, refresh, applicable).await {
        Ok((payload, _)) => (payload.builds.clone(), payload.provider.clone()),
        Err(error) => provider_result(Err(error)),
    }
}

#[cfg(test)]
pub async fn build_catalog(mc: &str, source: Option<&str>) -> LoaderCatalog {
    build_catalog_with_options(mc, source, None, false).await
}

async fn build_catalog_with_options(
    mc: &str,
    source: Option<&str>,
    release_time: Option<&str>,
    force_refresh: bool,
) -> LoaderCatalog {
    let mut catalog = LoaderCatalog::default();
    let policy = match source_policy(source).and_then(|policy| {
        if crate::named_profiles::valid_minecraft_version(mc) {
            Ok(policy)
        } else {
            Err("Minecraft 版本号无效，请重新选择版本".into())
        }
    }) {
        Ok(policy) => policy,
        Err(error) => {
            for loader in ["forge", "neoforge", "fabric", "liteloader", "optifine"] {
                catalog
                    .providers
                    .insert(loader.into(), provider_result(Err(error.clone())).1);
            }
            return catalog;
        }
    };
    let context = TransferContext::new(policy, policy, CancellationToken::new());
    // OfficialAuto 只决定传输层的 4 秒竞争顺序，不应让每次进入都绕过 10 分钟结果缓存。
    let refresh = force_refresh;
    let plan = query_plan(mc, release_time);
    let (forge, neoforge, fabric, liteloader, optifine) = tokio::join!(
        bounded_provider(&context, "forge", mc, release_time, refresh, plan.forge),
        bounded_provider(
            &context,
            "neoforge",
            mc,
            release_time,
            refresh,
            plan.neoforge
        ),
        bounded_provider(&context, "fabric", mc, release_time, refresh, plan.fabric),
        bounded_provider(
            &context,
            "liteloader",
            mc,
            release_time,
            refresh,
            plan.liteloader
        ),
        bounded_provider(&context, "optifine", mc, release_time, refresh, true),
    );
    for (loader, (builds, provider)) in [
        ("forge", forge),
        ("neoforge", neoforge),
        ("fabric", fabric),
        ("liteloader", liteloader),
        ("optifine", optifine),
    ] {
        catalog.providers.insert(loader.into(), provider);
        match loader {
            "forge" => catalog.forge = builds,
            "neoforge" => catalog.neoforge = builds,
            "fabric" => catalog.fabric = builds,
            "liteloader" => catalog.liteloader = builds,
            _ => catalog.optifine = builds,
        }
    }
    catalog
}

#[tauri::command]
pub async fn list_loaders(
    mc_version: String,
    source: Option<String>,
    release_time: Option<String>,
    force_refresh: Option<bool>,
    operation_id: Option<String>,
) -> String {
    // operationId 用于前端同次进入合并；后端全量端点缓存继续合并并行消费者。
    let _ = operation_id;
    let catalog = build_catalog_with_options(
        mc_version.trim(),
        source.as_deref(),
        release_time.as_deref(),
        force_refresh.unwrap_or(false),
    )
    .await;
    serde_json::to_string(&catalog).unwrap_or_else(|_| "null".into())
}

/// 渐进清单契约：每个 provider 独立完成，前端可立即合并后继续等待其它行。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderProviderResponse {
    pub loader: String,
    pub builds: Vec<LoaderBuild>,
    pub provider: LoaderProvider,
    pub cached: bool,
    pub elapsed_ms: u64,
}

#[tauri::command]
pub async fn list_loader_provider(
    mc_version: String,
    loader: String,
    source: Option<String>,
    release_time: Option<String>,
    force_refresh: Option<bool>,
    operation_id: Option<String>,
) -> Result<String, String> {
    let _ = operation_id;
    let started = Instant::now();
    let mc = mc_version.trim();
    if !crate::named_profiles::valid_minecraft_version(mc) {
        return Err("Minecraft 版本号无效，请重新选择版本".into());
    }
    let policy = source_policy(source.as_deref())?;
    let context = TransferContext::new(policy, policy, CancellationToken::new());
    let plan = query_plan(mc, release_time.as_deref());
    let applicable = match loader.as_str() {
        "forge" => plan.forge,
        "neoforge" => plan.neoforge,
        "fabric" => plan.fabric,
        "liteloader" => plan.liteloader,
        // OptiFine 没有年代隐藏规则，最终由清单和共装兼容性决定。
        "optifine" => true,
        _ => return Err("该加载器没有可用的安装提供方".into()),
    };
    let (payload, cached) = load_provider_cached(
        &context,
        &loader,
        mc,
        release_time.as_deref(),
        force_refresh.unwrap_or(false),
        applicable,
    )
    .await?;
    serde_json::to_string(&LoaderProviderResponse {
        loader,
        builds: payload.builds.clone(),
        provider: payload.provider.clone(),
        cached,
        elapsed_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
    })
    .map_err(|_| "加载器清单序列化失败".into())
}

/// 页面可先调用本命令预热；同一 provider 缓存会合并在途请求，调用方无需等待。
#[tauri::command]
pub async fn preload_loader_catalog(
    mc_version: String,
    source: Option<String>,
    release_time: Option<String>,
    force_refresh: Option<bool>,
) -> Result<(), String> {
    let mc = mc_version.trim().to_string();
    if !crate::named_profiles::valid_minecraft_version(&mc) {
        return Err("Minecraft 版本号无效，请重新选择版本".into());
    }
    source_policy(source.as_deref())?;
    tokio::spawn(async move {
        let _ = build_catalog_with_options(
            &mc,
            source.as_deref(),
            release_time.as_deref(),
            force_refresh.unwrap_or(false),
        )
        .await;
    });
    Ok(())
}

/// 目录、另存与自动安装共用的唯一解析入口。
pub async fn resolve_artifact(
    mc: &str,
    loader: &str,
    version: &str,
    source: Option<&str>,
) -> Result<ResolvedLoaderArtifact, String> {
    let build = selection_build(mc, loader, version, source).await?;
    let mut artifact = build
        .artifact
        .ok_or_else(|| "该构建没有可用的加载器文件，请刷新清单后重新选择".to_string())?;
    if artifact.kind == LoaderArtifactKind::OptiFine {
        append_optifine_official(&mut artifact, source).await;
    }
    Ok(artifact)
}

async fn append_optifine_official(artifact: &mut ResolvedLoaderArtifact, source: Option<&str>) {
    let file_name = artifact.api_name.trim();
    if file_name.is_empty() {
        return;
    }
    let page_url = artifact
        .raw_metadata
        .clone()
        .and_then(|value| serde_json::from_value::<optifine_install::OptiFineEntry>(value).ok())
        .map(|entry| optifine_install::adloadx_url(&entry))
        .unwrap_or_else(|| format!("{}?f={file_name}", optifine_install::OPTIFINE_ADLOADX));
    let Ok(policy) = source_policy(source) else {
        return;
    };
    let context = TransferContext::new(policy, policy, CancellationToken::new());
    let Ok(client) = build_client(QUERY_TIMEOUT_SECS, false, None) else {
        return;
    };
    let parsed = fetch_validated_candidates(
        &client,
        &context,
        "optifine-official-link",
        &[RequestCandidate::official(&page_url)],
        METADATA_MAX_BYTES,
        |bytes| {
            let text =
                std::str::from_utf8(bytes).map_err(|_| "OptiFine 官网下载页不是 UTF-8 文本")?;
            optifine_install::official_download_link(text)
                .ok_or_else(|| "OptiFine 官网下载页没有 downloadx 链接".into())
        },
    )
    .await;
    if let Ok(link) = parsed {
        if !artifact
            .download_candidates
            .iter()
            .any(|item| item == &link)
        {
            artifact.download_candidates.push(link);
        }
    }
}

async fn selection_build(
    mc: &str,
    loader: &str,
    version: &str,
    source: Option<&str>,
) -> Result<LoaderBuild, String> {
    if !crate::named_profiles::valid_minecraft_version(mc) || !valid_version(version) {
        return Err("Minecraft 或加载器版本号无效".into());
    }
    let policy = source_policy(source)?;
    let context = TransferContext::new(policy, policy, CancellationToken::new());
    let plan = query_plan(mc, None);
    let applicable = match loader {
        "forge" => plan.forge,
        "neoforge" => plan.neoforge,
        "fabric" => plan.fabric,
        "liteloader" => plan.liteloader,
        // OptiFine 没有年代隐藏规则，最终由清单和共装兼容性决定。
        "optifine" => true,
        _ => false,
    };
    if !applicable {
        return Err("该 Minecraft 版本没有此加载器的安装方式".into());
    }
    let (builds, provider) = bounded_provider(&context, loader, mc, None, false, true).await;
    if provider.state == ProviderState::Failed {
        return Err(provider.error);
    }
    builds
        .into_iter()
        .find(|b| {
            b.version == version
                || (version == "latest" && b.latest)
                || (version == "recommended" && b.recommended)
                || (loader == "neoforge"
                    && mc == "1.20.1"
                    && (b.version == format!("{mc}-{version}")
                        || b.artifact.as_ref().is_some_and(|artifact| {
                            artifact.api_name == version
                                || artifact.installer_version == version
                                || artifact.display_version == version
                        })))
        })
        .ok_or_else(|| "未找到所选构建，请刷新清单后重试".into())
}

/// 另存只接受提供方与版本；地址和文件名均来自同一 artifact。
pub async fn installer_source(
    mc: &str,
    loader: &str,
    version: &str,
    source: Option<&str>,
) -> Result<ResolvedLoaderArtifact, String> {
    let artifact = resolve_artifact(mc, loader, version, source).await?;
    if !artifact.save_installable {
        return Err("该构建没有可另存的独立安装器".into());
    }
    Ok(artifact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn forge_complete_list_is_not_replaced_by_promotions() {
        let xml = "<metadata><versions><version>1.20.1-47.3.0</version><version>1.20.1-47.9.0</version><version>1.20.1-47.10.0</version><version>1.20.1-47.9.0</version><version>1.21-51.0.1</version></versions></metadata>";
        let mut builds = parse_forge(xml, "1.20.1").unwrap();
        mark_promotions(&mut builds, &serde_json::json!({"promos":{"1.20.1-latest":"47.10.0","1.20.1-recommended":"47.3.0"}}), "1.20.1").unwrap();
        assert_eq!(builds.len(), 3);
        assert_eq!(builds[0].version, "47.10.0");
        assert!(builds[0].latest);
        assert!(builds[2].recommended);
        assert!(builds.iter().all(|b| !b.capability.install));
        assert!(builds.iter().all(|b| b.artifact.is_none()));
    }

    #[test]
    fn forge_filters_non_installer_files_and_preserves_branch() {
        let rows = parse_forge(
            r#"[{"version":"7.8.1.738","files":[{"category":"universal","format":"jar"}]}]"#,
            "1.5.2",
        )
        .unwrap();
        assert!(!rows[0].capability.install);
        assert!(
            rows[0]
                .download_url
                .ends_with("forge-1.5.2-7.8.1.738-universal.jar")
        );
        assert_eq!(
            rows[0].artifact.as_ref().unwrap().category,
            minecraft_java_rs_core::models::loader::ArtifactCategory::Universal
        );
        let rows = parse_forge(r#"[{"version":"10.13.4.1614","branch":"1.7.10","modified":"2020-01-01","files":[{"category":"installer","format":"jar"}]}]"#, "1.7.10").unwrap();
        assert_eq!(rows[0].version, "10.13.4.1614-1.7.10");
        assert!(rows[0].download_url.contains("1.7.10-10.13.4.1614-1.7.10/"));
        assert!(rows[0].release_time.is_empty());
        assert_eq!(rows[0].modified_time, "2020-01-01");
    }

    #[test]
    fn forge_maven_coordinate_keeps_pre_release_and_branch_suffix() {
        let xml = "<metadata><versions><version>1.7.10-pre4-10.12.2.1121</version><version>1.8.9-11.15.1.2318-1.8.9</version><version>1.20.1-47.3.0</version></versions></metadata>";
        let index = parse_forge_index(xml).unwrap();
        assert_eq!(index.for_game("1.7.10_pre4")[0].version, "10.12.2.1121");
        assert_eq!(index.for_game("1.8.9")[0].version, "11.15.1.2318-1.8.9");
        assert_eq!(index.for_game("1.20.1")[0].version, "47.3.0");
    }

    #[test]
    fn forge_26_3_fixture_keeps_all_eighteen_builds_when_one_has_no_files() {
        let versions = [
            "66.0.0", "66.0.2", "66.0.3", "66.0.4", "66.0.5", "66.0.6", "66.0.8", "66.0.9",
            "66.0.10", "66.0.11", "66.0.12", "66.0.13", "66.1.0", "66.1.1", "66.1.2", "66.1.3",
            "66.1.4", "66.1.5",
        ];
        let items = versions
            .iter()
            .enumerate()
            .map(|(index, version)| {
                let files = if index == 0 {
                    serde_json::json!([])
                } else {
                    serde_json::json!([{
                        "category": "installer",
                        "format": "jar",
                        "size": 42
                    }])
                };
                serde_json::json!({
                    "version": version,
                    "mcversion": "26.3",
                    "modified": "2026-09-19T14:07:47.000Z",
                    "files": files
                })
            })
            .collect::<Vec<_>>();
        let fixture = serde_json::to_string(&items).unwrap();
        let rows = parse_forge(&fixture, "26.3").unwrap();
        assert_eq!(rows.len(), 18);
        let (_, provider) = provider_result(Ok(rows.clone()));
        assert_eq!(provider.count, rows.len());
        assert_eq!(provider.state, ProviderState::Available);
        let no_file = rows.iter().find(|row| row.version == "66.0.0").unwrap();
        assert!(no_file.artifact.is_none());
        assert!(!no_file.capability.install);
        assert_eq!(rows.iter().filter(|row| row.artifact.is_some()).count(), 17);
    }

    #[test]
    fn neoforge_26_3_fixture_preserves_all_ninety_eight_builds() {
        let versions = (0..98)
            .map(|index| format!("26.3.0.{index}-beta"))
            .collect::<Vec<_>>();
        let fixture = serde_json::json!({"versions": versions}).to_string();
        let rows = parse_neoforge_index(&fixture).unwrap().for_game("26.3");
        assert_eq!(rows.len(), 98);
        assert!(rows.iter().all(|row| row.artifact.is_some()));
        let (_, provider) = provider_result(Ok(rows));
        assert_eq!(provider.count, 98);
        assert_eq!(provider.state, ProviderState::Available);
    }

    #[test]
    fn fabric_26_3_fixture_preserves_all_two_hundred_fifty_five_builds() {
        let loaders = (0..255)
            .map(|index| {
                serde_json::json!({
                    "version": format!("0.18.{index}"),
                    "stable": index == 0
                })
            })
            .collect::<Vec<_>>();
        let fixture = serde_json::json!({
            "game": [{"version": "26.3"}],
            "loader": loaders,
            "installer": [{"version": "1.1.2", "stable": true}]
        })
        .to_string();
        let index = parse_fabric_index(&fixture).unwrap();
        assert!(index.games.contains("26.3"));
        let mut rows = index.for_game(&fabric_api_id("26.3"));
        assert_eq!(rows.len(), 255);
        attach_fabric_artifacts(&index, "26.3", &mut rows);
        assert!(rows.iter().all(|row| row.artifact.is_some()));
        let (_, provider) = provider_result(Ok(rows));
        assert_eq!(provider.count, 255);
        assert_eq!(provider.state, ProviderState::Available);
    }

    #[test]
    fn optifine_and_liteloader_catalogs_expose_real_install_and_save_sources() {
        let optifine = parse_optifine_index(
            r#"[{"mcversion":"1.12.2","type":"HD_U","patch":"G5","filename":"OptiFine_1.12.2_HD_U_G5.jar","forge":"N/A"}]"#,
        )
        .unwrap();
        let rows = optifine.for_game("1.12.2");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].capability.install);
        assert!(rows[0].capability.save_installer);
        assert_eq!(provider_result(Ok(rows)).1.state, ProviderState::Available);

        let liteloader = parse_liteloader_index(
            r#"{"versions":{"1.12.2":{"artefacts":{"com.mumfrey:liteloader":{"latest":{"version":"1.12.2-SNAPSHOT-r0","tweakClass":"com.mumfrey.liteloader.launch.LiteLoaderTweaker","stream":"RELEASE","libraries":[]}}}}}}"#,
        )
        .unwrap();
        let rows = liteloader.for_game("1.12.2");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].artifact.as_ref().unwrap().profile.is_some());
        assert!(rows[0].capability.save_installer);
        assert!(rows[0].capability.install);
        assert_eq!(provider_result(Ok(rows)).1.state, ProviderState::Available);
    }

    #[test]
    fn optifine_26_3_empty_is_unavailable_but_parse_error_is_failed() {
        let index = parse_optifine_index(
            r#"[{"mcversion":"26.2","type":"HD_U_K2","patch":"pre5","filename":"preview_OptiFine_26.2_HD_U_K2_pre5.jar","forge":"N/A"}]"#,
        )
        .unwrap();
        let rows = index.for_game("26.3");
        assert!(rows.is_empty());
        let (builds, provider) = provider_result(Ok(rows));
        assert!(builds.is_empty());
        assert_eq!(provider.state, ProviderState::Unavailable);
        assert_eq!(provider.count, 0);
        assert!(provider.error.is_empty());

        let malformed = format!("<html>{}</html>", "no optifine rows ".repeat(20));
        assert!(parse_optifine_index(&malformed).is_err());
        let (builds, provider) = provider_result(Err("OptiFine 官网清单没有可识别文件名".into()));
        assert!(builds.is_empty());
        assert_eq!(provider.state, ProviderState::Failed);
        assert_eq!(provider.count, 0);
        assert_eq!(provider.error, "OptiFine 官网清单没有可识别文件名");
    }

    #[test]
    fn authoritative_empty_mirror_does_not_turn_fallback_failure_into_failed() {
        let primary = Arc::new(CatalogIndex::default());
        let result = prefer_empty_index(primary.clone(), Err("HTTP 403".into()), true);
        assert!(result.is_ok());
        assert!(result.unwrap().builds.is_empty());

        let result = prefer_empty_index(primary, Err("HTTP 403".into()), false);
        assert_eq!(result.unwrap_err(), "HTTP 403");
    }

    #[test]
    fn neoforge_legacy_modern_and_snapshot_are_exact() {
        let legacy = parse_neoforge_index(r#"[{"version":"47.1.5","mcversion":"1.20.1"}]"#)
            .unwrap()
            .for_game("1.20.1");
        assert_eq!(legacy[0].version, "1.20.1-47.1.5");
        assert!(
            legacy[0]
                .download_url
                .contains("/forge/1.20.1-47.1.5/forge-")
        );
        let index = parse_neoforge_index(r#"{"files":[{"name":"26.1.0.1-beta","type":"DIRECTORY"},{"name":"26.1.0.0-alpha.1+snapshot-1","type":"DIRECTORY"},{"name":"0.25w14craftmine.3-beta","type":"DIRECTORY"},{"name":"20.4.30-beta","type":"DIRECTORY"}]}"#).unwrap();
        assert_eq!(index.for_game("26.1").len(), 1);
        assert_eq!(
            index.for_game("26.1-snapshot-1")[0].version,
            "26.1.0.0-alpha.1+snapshot-1"
        );
        assert_eq!(index.for_game("25w14craftmine").len(), 1);
        assert_eq!(index.for_game("1.20.4").len(), 1);
        assert!(index.for_game("1.2.1").is_empty());
    }

    #[test]
    fn fabric_support_is_exact_and_installer_failure_keeps_install_capability() {
        let index = parse_fabric_index(r#"{"game":[{"version":"20w14infinite"},{"version":"1.16_combat-3"}],"loader":[{"version":"0.16.10","stable":true}],"installer":[]}"#).unwrap();
        assert!(index.for_game("1.2.1").is_empty());
        for mc in ["20w14∞", "Combat Test 7c"] {
            let rows = index.for_game(&fabric_api_id(mc));
            assert_eq!(rows.len(), 1);
            assert!(rows[0].capability.install);
            assert!(!rows[0].capability.save_installer);
        }
        assert!(parse_fabric_index(r#"{"game":[],"loader":[]}"#).is_err());
        let installer = parse_fabric_installer(&serde_json::json!([{"version":"1.1.2","stable":true,"url":"https://untrusted.invalid"}])).unwrap();
        assert_eq!(installer.version, "1.1.2");
        assert_eq!(
            installer.url,
            format!(
                "{}/1.1.2/fabric-installer-1.1.2.jar",
                artifact::FABRIC_MAVEN
            )
        );
    }

    #[test]
    fn failed_empty_and_non_installable_states_stay_distinct() {
        let (builds, provider) = provider_result(Ok(Vec::new()));
        assert!(builds.is_empty());
        assert_eq!(provider.state, ProviderState::Unavailable);
        assert_eq!(provider.count, 0);
        assert!(provider.error.is_empty());
        for error in ["HTTP 404", "HTTP 429", "JSON 解析失败", "请求超时"] {
            let (_, provider) = provider_result(Err(error.into()));
            assert_eq!(provider.state, ProviderState::Failed);
            assert_eq!(provider.count, 0);
            assert_eq!(provider.error, error);
        }
        let (builds, provider) = provider_result(Ok(vec![build("7.8.1.738")]));
        assert_eq!(builds.len(), 1);
        assert_eq!(provider.state, ProviderState::Available);
        assert_eq!(provider.count, 1);
        assert!(!provider.capability.install);
        assert!(!provider.capability.save_installer);
        let mut manual = build("1.0");
        manual.capability.save_installer = true;
        let (builds, provider) = provider_result(Ok(vec![manual]));
        assert_eq!(builds.len(), 1);
        assert_eq!(provider.state, ProviderState::ManualOnly);
        assert_eq!(provider.count, 1);
        assert!(parse_forge_index("<html>error</html>").is_err());
        assert!(parse_neoforge_index(r#"{"versions":["bad"]}"#).is_err());
    }

    #[test]
    fn public_contract_includes_provider_count_and_build_capabilities() {
        let (_, provider) = provider_result(Err("请求超时".into()));
        let value = serde_json::to_value(provider).unwrap();
        assert_eq!(value["state"], "failed");
        assert_eq!(value["count"], 0);
        assert_eq!(value["capability"]["saveInstaller"], false);
        let (rows, provider) = provider_result(Ok(vec![build("1.0")]));
        let provider = serde_json::to_value(provider).unwrap();
        assert_eq!(provider["state"], "available");
        assert_eq!(provider["count"], rows.len());
        assert_eq!(provider["count"], 1);
        assert!(
            serde_json::to_value(build("1.0"))
                .unwrap()
                .get("capability")
                .is_some()
        );
        for bad in ["", "../a", "a/b", "a%2Fb", "a?x=1", "a\\b"] {
            assert!(!valid_version(bad));
        }
        assert!(crate::named_profiles::valid_minecraft_version("20w14∞"));
        assert!(crate::named_profiles::valid_minecraft_version(
            "Combat Test 7c"
        ));
    }

    #[test]
    fn cache_keys_separate_policy_provider_and_target() {
        let forge = catalog_index_cache_key(
            SourcePolicy::MirrorFirst,
            "forge-files",
            "https://files.minecraftforge.net/net/minecraftforge/forge/index_26.3.html",
            Some("https://bmclapi2.bangbang93.com/forge/minecraft/26.3"),
        );
        let other_mc = catalog_index_cache_key(
            SourcePolicy::MirrorFirst,
            "forge-files",
            "https://files.minecraftforge.net/net/minecraftforge/forge/index_26.2.html",
            Some("https://bmclapi2.bangbang93.com/forge/minecraft/26.2"),
        );
        let other_loader = catalog_index_cache_key(
            SourcePolicy::MirrorFirst,
            "fabric-meta",
            "https://meta.fabricmc.net/v2/versions",
            Some("https://bmclapi2.bangbang93.com/fabric-meta/v2/versions"),
        );
        let other_policy = catalog_index_cache_key(
            SourcePolicy::OfficialFirst,
            "forge-files",
            "https://files.minecraftforge.net/net/minecraftforge/forge/index_26.3.html",
            Some("https://bmclapi2.bangbang93.com/forge/minecraft/26.3"),
        );
        assert_ne!(forge, other_mc);
        assert_ne!(forge, other_loader);
        assert_ne!(forge, other_policy);
    }

    #[test]
    fn provider_cache_keys_include_loader_mc_release_and_policy() {
        let base = provider_cache_key(
            SourcePolicy::OfficialAuto,
            "forge",
            "26.3",
            Some("2026-09-19T14:07:47Z"),
        );
        assert_ne!(
            base,
            provider_cache_key(
                SourcePolicy::OfficialAuto,
                "fabric",
                "26.3",
                Some("2026-09-19T14:07:47Z"),
            )
        );
        assert_ne!(
            base,
            provider_cache_key(
                SourcePolicy::OfficialAuto,
                "forge",
                "26.2",
                Some("2026-09-19T14:07:47Z"),
            )
        );
        assert_ne!(
            base,
            provider_cache_key(
                SourcePolicy::OfficialAuto,
                "forge",
                "26.3",
                Some("2026-09-18T14:07:47Z"),
            )
        );
        assert_ne!(
            base,
            provider_cache_key(SourcePolicy::MirrorFirst, "forge", "26.3", None)
        );
    }

    #[tokio::test]
    async fn cache_merges_inflight_revalidates_and_does_not_keep_failures() {
        let slot = Arc::new(tokio::sync::Mutex::new(CacheEntry::default()));
        let count = AtomicUsize::new(0);
        let fetch = || async {
            count.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok(CatalogIndex::default())
        };
        let (a, b) = tokio::join!(
            cached_index(slot.clone(), true, fetch),
            cached_index(slot.clone(), true, fetch)
        );
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        cached_index(slot.clone(), false, fetch).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        cached_index(slot.clone(), true, fetch).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 2);
        let failed = Arc::new(tokio::sync::Mutex::new(CacheEntry::default()));
        assert!(
            cached_index(failed.clone(), false, || async { Err("offline".into()) })
                .await
                .is_err()
        );
        assert!(cached_index(failed, false, fetch).await.is_ok());
    }

    #[tokio::test]
    async fn provider_cache_hits_refresh_and_providers_finish_independently() {
        let slot = Arc::new(tokio::sync::Mutex::new(CacheEntry::default()));
        let count = AtomicUsize::new(0);
        let fetch = || async {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(ProviderPayload {
                builds: vec![build("1.0")],
                provider: provider_metadata(&[build("1.0")]),
            })
        };
        let (first, first_hit) = cached_provider(slot.clone(), false, fetch).await.unwrap();
        assert!(!first_hit);
        assert_eq!(first.provider.count, 1);
        let (_, second_hit) = cached_provider(slot.clone(), false, fetch).await.unwrap();
        assert!(second_hit);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let (_, refreshed_hit) = cached_provider(slot, true, fetch).await.unwrap();
        assert!(!refreshed_hit);
        assert_eq!(count.load(Ordering::SeqCst), 2);

        let fast_slot = Arc::new(tokio::sync::Mutex::new(CacheEntry::default()));
        let slow_slot = Arc::new(tokio::sync::Mutex::new(CacheEntry::default()));
        let fast = tokio::spawn(async move {
            let started = Instant::now();
            let result = cached_provider(fast_slot, false, || async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(ProviderPayload {
                    builds: vec![build("fabric")],
                    provider: provider_metadata(&[build("fabric")]),
                })
            })
            .await;
            (result.map(|_| ()), started.elapsed())
        });
        let slow = tokio::spawn(async move {
            let started = Instant::now();
            let result = cached_provider(slow_slot, false, || async {
                tokio::time::sleep(Duration::from_millis(160)).await;
                Ok(ProviderPayload {
                    builds: vec![build("optifine")],
                    provider: provider_metadata(&[build("optifine")]),
                })
            })
            .await;
            (result.map(|_| ()), started.elapsed())
        });
        let (fast_result, fast_elapsed) = fast.await.unwrap();
        assert!(fast_result.is_ok());
        let (slow_result, slow_elapsed) = slow.await.unwrap();
        assert!(slow_result.is_ok());
        println!(
            "provider cache fixture: fast={fast_elapsed:?}, slow={slow_elapsed:?}"
        );
        assert!(fast_elapsed < slow_elapsed);

        // 旧全量入口会等待 join 中最慢分支；先记录该基线，再验证单 provider 路径独立完成。
        let joined_started = Instant::now();
        tokio::join!(
            tokio::time::sleep(Duration::from_millis(10)),
            tokio::time::sleep(Duration::from_millis(160))
        );
        let joined_elapsed = joined_started.elapsed();
        println!(
            "provider fixture: joined={joined_elapsed:?}, fast={fast_elapsed:?}, slow={slow_elapsed:?}"
        );
        assert!(joined_elapsed >= Duration::from_millis(150));
        assert!(fast_elapsed < joined_elapsed);
    }

    #[tokio::test]
    async fn hidden_provider_command_is_local_and_serializes_progressive_contract() {
        let raw = list_loader_provider(
            "1.2.1".into(),
            "neoforge".into(),
            Some("mirror_first".into()),
            Some("2012-03-01T00:00:00Z".into()),
            Some(false),
            Some("test".into()),
        )
        .await
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["loader"], "neoforge");
        assert_eq!(value["provider"]["state"], "hidden");
        assert_eq!(value["builds"], serde_json::json!([]));
        assert_eq!(value["cached"], false);
        assert!(value["elapsedMs"].is_u64());
    }

    #[tokio::test]
    async fn local_rejection_never_enters_network_query() {
        let context = TransferContext::new(
            SourcePolicy::OfficialAuto,
            SourcePolicy::OfficialAuto,
            CancellationToken::new(),
        );
        let plan = query_plan("1.2.1", Some("2012-03-01T00:00:00Z"));
        let (_, neo) =
            bounded_provider(&context, "neoforge", "1.2.1", None, true, plan.neoforge).await;
        let (_, fabric) =
            bounded_provider(&context, "fabric", "1.2.1", None, true, plan.fabric).await;
        assert_eq!(neo.state, ProviderState::Hidden);
        assert_eq!(fabric.state, ProviderState::Hidden);
    }

    #[tokio::test]
    #[ignore = "显式运行的联网冒烟测试"]
    async fn live_loader_catalogs() {
        for mc in [
            "1.12.2",
            "1.13.2",
            "1.20.1",
            "26.1",
            "26.1-snapshot-1",
            "26.3",
            "25w14craftmine",
        ] {
            let catalog = build_catalog(mc, Some("mirror_first")).await;
            println!(
                "{mc}: forge={}, neoforge={}, fabric={}, providers={:?}",
                catalog.forge.len(),
                catalog.neoforge.len(),
                catalog.fabric.len(),
                catalog.providers
            );
            for (loader, builds) in [
                ("forge", &catalog.forge),
                ("neoforge", &catalog.neoforge),
                ("fabric", &catalog.fabric),
                ("liteloader", &catalog.liteloader),
                ("optifine", &catalog.optifine),
            ] {
                assert_eq!(catalog.providers[loader].count, builds.len(), "{mc}");
            }
            if mc == "1.20.1" || mc == "26.3" {
                assert!(
                    !catalog.forge.is_empty()
                        && !catalog.neoforge.is_empty()
                        && !catalog.fabric.is_empty()
                );
            }
            if mc.starts_with("26.") || mc == "25w14craftmine" {
                assert!(!catalog.neoforge.is_empty());
            }
            if mc == "26.3" {
                assert_ne!(catalog.providers["optifine"].state, ProviderState::Failed);
            }
        }
    }

    #[tokio::test]
    #[ignore = "显式运行的联网耗时测试"]
    async fn live_provider_latency_breakdown() {
        const MC: &str = "1.20.1";
        const RELEASED: &str = "2017-06-02T00:00:00Z";
        for loader in ["forge", "neoforge", "fabric", "liteloader", "optifine"] {
            let started = Instant::now();
            let result = list_loader_provider(
                MC.into(),
                loader.into(),
                Some("mirror_first".into()),
                Some(RELEASED.into()),
                Some(true),
                None,
            )
            .await;
            println!(
                "live provider {loader}: {:?} ({:?})",
                started.elapsed(),
                result.as_ref().map(|_| "ok").map_err(String::as_str)
            );
            if let Ok(raw) = result {
                let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
                println!(
                    "  state={} count={}",
                    value["provider"]["state"], value["provider"]["count"]
                );
            }
        }
        let started = Instant::now();
        let catalog =
            build_catalog_with_options(MC, Some("mirror_first"), Some(RELEASED), true).await;
        println!(
            "live list_loaders: {:?}, forge={}, neoforge={}, fabric={}",
            started.elapsed(),
            catalog.forge.len(),
            catalog.neoforge.len(),
            catalog.fabric.len()
        );
        let hot_started = Instant::now();
        let hot = list_loader_provider(
            MC.into(),
            "fabric".into(),
            Some("mirror_first".into()),
            Some(RELEASED.into()),
            Some(false),
            None,
        )
        .await
        .unwrap();
        let hot: serde_json::Value = serde_json::from_str(&hot).unwrap();
        println!(
            "live hot fabric: {:?}, cached={}, elapsedMs={}",
            hot_started.elapsed(),
            hot["cached"],
            hot["elapsedMs"]
        );
    }
}
