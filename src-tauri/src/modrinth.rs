//! Modrinth API 与普通社区资源安装；整合包由 modpacks 解析、统一任务编排。

use std::path::PathBuf;

use serde::Serialize;
use tauri::AppHandle;

/// Labrinth API 根地址。
const API_BASE: &str = "https://api.modrinth.com/v2";
/// 项目页地址（`modrinth_open_project`）。
const PROJECT_PAGE_BASE: &str = "https://modrinth.com/project";

// ─────────────────────────────────────────────────────────────────────────────
// 归一化数据结构（字段即前端契约，顺序即 JSON 字段顺序）
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchHit {
    slug: String,
    title: String,
    description: String,
    icon_url: String,
    downloads: u64,
    author: String,
    project_type: String,
    versions: Vec<String>,
    loaders: Vec<String>,
    date_modified: String,
}

#[derive(Debug, Serialize)]
struct SearchResult {
    total: u64,
    hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionFile {
    url: String,
    filename: String,
    size_bytes: u64,
    primary: bool,
    hash_sha1: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectVersion {
    id: String,
    name: String,
    version_number: String,
    loaders: Vec<String>,
    game_versions: Vec<String>,
    date_published: String,
    release_type: String,
    files: Vec<VersionFile>,
}

// ─────────────────────────────────────────────────────────────────────────────
// 纯逻辑（便于单元测试）
// ─────────────────────────────────────────────────────────────────────────────

/// 校验并归一化资源种类（前端五种分类 → Labrinth 的 project_type）。
fn normalize_release_type(value: Option<&serde_json::Value>) -> String {
    match value.and_then(|v| v.as_str()) {
        Some("release") => "release",
        Some("beta") => "beta",
        Some("alpha") => "alpha",
        _ => "unknown",
    }
    .into()
}

pub(crate) fn normalize_kind(kind: &str) -> Result<String, String> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "mod" => Ok("mod".to_string()),
        "resourcepack" => Ok("resourcepack".to_string()),
        "shader" => Ok("shader".to_string()),
        "datapack" => Ok("datapack".to_string()),
        "modpack" => Ok("modpack".to_string()),
        other => Err(format!(
            "不支持的资源种类：{other}（可选 mod / resourcepack / shader / datapack / modpack）"
        )),
    }
}

/// kind → 实例子目录名。整合包由独立入口创建新实例。
pub(crate) fn kind_folder(kind: &str) -> Option<&'static str> {
    match kind {
        "mod" => Some("mods"),
        "resourcepack" => Some("resourcepacks"),
        "shader" => Some("shaderpacks"),
        "datapack" => Some("datapacks"),
        _ => None,
    }
}

/// 构造 Labrinth 搜索的 `facets` 参数（JSON 数组的数组，转义交给 serde_json）。
///
/// - 恒定带 `project_type`；
/// - 提供了 MC 版本时带 `versions`（Labrinth 对游戏版本的 facet 键名）；
/// - loader 只对 `mod` 生效（见模块注释），且仅接受 forge / neoforge / fabric。
fn build_facets(kind: &str, game_version: Option<&str>, loader: Option<&str>) -> String {
    let mut groups: Vec<String> = Vec::new();
    groups.push(json_array(&[format!("project_type:{kind}")]));

    if let Some(gv) = game_version.map(str::trim).filter(|v| !v.is_empty()) {
        groups.push(json_array(&[format!("versions:{gv}")]));
    }
    if kind == "mod" {
        if let Some(ld) = loader
            .map(str::trim)
            .filter(|v| matches!(*v, "forge" | "neoforge" | "fabric" | "quilt"))
        {
            groups.push(json_array(&[format!("loaders:{ld}")]));
        }
    }
    format!("[{}]", groups.join(","))
}

/// 把一组字符串序列化为 JSON 数组文本（serde_json 负责转义）。
fn json_array(items: &[String]) -> String {
    serde_json::to_string(items).unwrap_or_else(|_| "[]".to_string())
}

fn str_field(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn str_array(value: &serde_json::Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 把 Labrinth 搜索响应归一化为前端契约的 JSON 文本。
fn normalize_search(value: &serde_json::Value) -> Result<String, String> {
    let hits = value
        .get("hits")
        .and_then(|h| h.as_array())
        .ok_or_else(|| "Modrinth 搜索结果格式无法识别".to_string())?;

    let out: Vec<SearchHit> = hits
        .iter()
        .filter_map(|h| {
            // slug 是结果卡片的唯一标识（详情 / 项目页都靠它），缺失或为空的条目丢弃。
            let slug = h.get("slug")?.as_str()?.trim().to_string();
            if slug.is_empty() {
                return None;
            }
            Some(SearchHit {
                slug,
                title: str_field(h, "title"),
                description: str_field(h, "description"),
                icon_url: str_field(h, "icon_url"),
                downloads: h.get("downloads").and_then(|d| d.as_u64()).unwrap_or(0),
                author: str_field(h, "author"),
                project_type: str_field(h, "project_type"),
                versions: str_array(h, "versions"),
                loaders: str_array(h, "loaders"),
                date_modified: str_field(h, "date_modified"),
            })
        })
        .collect();

    let total = value
        .get("total")
        .and_then(|t| t.as_u64())
        .unwrap_or(out.len() as u64);
    serde_json::to_string(&SearchResult { total, hits: out })
        .map_err(|e| format!("搜索结果序列化失败：{e}"))
}

/// 归一化单个版本文件条目；缺少下载地址的文件丢弃。
fn normalize_file(file: &serde_json::Value) -> Option<VersionFile> {
    let url = file.get("url")?.as_str()?;
    if url.is_empty() {
        return None;
    }
    Some(VersionFile {
        url: url.to_string(),
        filename: str_field(file, "filename"),
        size_bytes: file.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
        primary: file
            .get("primary")
            .and_then(|p| p.as_bool())
            .unwrap_or(false),
        hash_sha1: file
            .get("hashes")
            .and_then(|h| h.get("sha1"))
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// 按 MC 版本与加载器过滤并归一化版本列表（纯函数，便于测试）。
///
/// `game_version` / `loader` 为空串时对应维度不过滤。
fn filter_versions(
    value: &serde_json::Value,
    game_version: &str,
    loader: &str,
) -> Result<Vec<ProjectVersion>, String> {
    let items = value
        .as_array()
        .ok_or_else(|| "Modrinth 版本数据格式无法识别".to_string())?;

    let mut out = Vec::new();
    for item in items {
        let id = str_field(item, "id");
        if id.is_empty() {
            continue;
        }
        if !game_version.is_empty()
            && !str_array(item, "game_versions")
                .iter()
                .any(|v| v == game_version)
        {
            continue;
        }
        if !loader.is_empty() && !str_array(item, "loaders").iter().any(|v| v == loader) {
            continue;
        }
        let files = item
            .get("files")
            .and_then(|f| f.as_array())
            .map(|arr| arr.iter().filter_map(normalize_file).collect())
            .unwrap_or_default();
        out.push(ProjectVersion {
            id,
            name: str_field(item, "name"),
            version_number: str_field(item, "version_number"),
            loaders: str_array(item, "loaders"),
            game_versions: str_array(item, "game_versions"),
            date_published: str_field(item, "date_published"),
            release_type: normalize_release_type(item.get("version_type")),
            files,
        });
    }
    Ok(out)
}

/// SHA1 十六进制比较：忽略大小写与首尾空白；期望值为空时视为「无校验值」。
#[cfg(test)]
fn sha1_matches(actual: &str, expected: &str) -> bool {
    let expected = expected.trim().to_ascii_lowercase();
    !expected.is_empty() && actual.trim().eq_ignore_ascii_case(&expected)
}

/// 从版本列表中选出要下载的文件：
/// 必须命中 `version_id`；文件优先取 `primary`，否则第一个带地址的。
/// `filename` 缺失时从 URL 末段推断。
pub(crate) fn pick_primary_file(
    value: &serde_json::Value,
    version_id: &str,
) -> Result<(String, String, String), String> {
    let versions = value
        .as_array()
        .ok_or_else(|| "Modrinth 版本数据格式无法识别".to_string())?;
    let version = versions
        .iter()
        .find(|v| str_field(v, "id") == version_id)
        .ok_or_else(|| "指定的版本在 Modrinth 上不存在或已被删除".to_string())?;

    let files = version
        .get("files")
        .and_then(|f| f.as_array())
        .ok_or_else(|| "该版本没有可下载的文件".to_string())?;

    let file = files
        .iter()
        .find(|f| f.get("primary").and_then(|p| p.as_bool()) == Some(true))
        .or_else(|| files.first())
        .ok_or_else(|| "该版本没有可下载的文件".to_string())?;

    let url = file
        .get("url")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    if url.is_empty() {
        return Err("该版本的文件缺少下载地址".to_string());
    }

    let mut filename = str_field(file, "filename");
    if filename.is_empty() {
        filename = url.rsplit('/').next().unwrap_or("").to_string();
    }
    if filename.is_empty() {
        return Err("该版本的文件名不可用".to_string());
    }

    let sha1 = file
        .get("hashes")
        .and_then(|h| h.get("sha1"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    Ok((url, filename, sha1))
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP（带合规 User-Agent；独立社区候选）
// ─────────────────────────────────────────────────────────────────────────────

/// GET JSON，带 Modrinth 合规 UA；HTTP 错误转成可读中文。
async fn get_json(url: &str) -> Result<serde_json::Value, String> {
    get_json_with_settings(url, &crate::network_settings::snapshot()).await
}

async fn get_json_with_settings(
    url: &str,
    settings: &crate::network_settings::NetworkSettings,
) -> Result<serde_json::Value, String> {
    let context = crate::community_network::context(settings);
    crate::community_network::get_json(
        crate::community_network::Platform::Modrinth,
        url,
        settings,
        &context,
        None,
        |value| {
            let valid = if url.contains("/search?") {
                value["hits"].is_array()
            } else if url.ends_with("/version") {
                value.is_array()
            } else {
                value.is_object()
            };
            if !valid {
                return Err("Modrinth 响应结构无效，请重试".into());
            }
            Ok(value.clone())
        },
    )
    .await
}

// ─────────────────────────────────────────────────────────────────────────────
// 命令：检索
// ─────────────────────────────────────────────────────────────────────────────

/// 检索 Modrinth 资源。`kind` ∈ mod | resourcepack | shader | datapack | modpack。
#[tauri::command]
pub async fn modrinth_search(
    kind: String,
    query: Option<String>,
    game_version: Option<String>,
    loader: Option<String>,
    offset: Option<u32>,
    limit: Option<u32>,
) -> Result<String, String> {
    let kind = normalize_kind(&kind)?;
    let facets = build_facets(&kind, game_version.as_deref(), loader.as_deref());
    let offset = offset.unwrap_or(0).min(10_000);
    let limit = limit.unwrap_or(20).clamp(1, 100);

    let mut url = format!(
        "{API_BASE}/search?facets={}&offset={offset}&limit={limit}&index=relevance",
        crate::http::urlencode(&facets)
    );
    if let Some(q) = query.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        url.push_str("&query=");
        url.push_str(&crate::http::urlencode(q));
    }

    let value = get_json(&url).await?;
    normalize_search(&value)
}

type VersionCache =
    std::sync::Arc<tokio::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>>>;
static VERSION_CACHE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, VersionCache>>,
> = std::sync::OnceLock::new();

async fn cached_versions(url: &str) -> Result<serde_json::Value, String> {
    let settings = crate::network_settings::snapshot();
    let key = format!(
        "{}|{}|{:?}",
        url, settings.modrinth_api_mirror, settings.community_api_source
    );
    let entry = {
        let mut cache = VERSION_CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if cache.len() >= 64 && !cache.contains_key(&key) {
            cache.clear();
        }
        cache.entry(key).or_default().clone()
    };
    let mut guard = entry.lock().await;
    if let Some((at, value)) = guard
        .as_ref()
        .filter(|(at, _)| at.elapsed().as_secs() < 300)
    {
        let value = value.clone();
        // 缓存键和后台重验证使用同一来源快照，设置变化不能污染旧缓存。
        if at.elapsed().as_millis() > 250 {
            let refresh_entry = entry.clone();
            let url = url.to_owned();
            drop(guard);
            tokio::spawn(async move {
                if let Ok(mut cached) = refresh_entry.try_lock() {
                    if let Ok(value) = get_json_with_settings(&url, &settings).await {
                        *cached = Some((std::time::Instant::now(), value));
                    }
                }
            });
        }
        return Ok(value);
    }
    let value = get_json_with_settings(url, &settings).await?;
    *guard = Some((std::time::Instant::now(), value.clone()));
    Ok(value)
}

/// 列出某项目的版本（按 MC 版本与加载器过滤；loader 只对 mod 生效）。
#[tauri::command]
pub async fn modrinth_project_versions(
    kind: String,
    slug_or_id: String,
    game_version: Option<String>,
    loader: Option<String>,
) -> Result<String, String> {
    let kind = normalize_kind(&kind)?;
    let id = slug_or_id.trim();
    if id.is_empty() {
        return Err("缺少项目标识（slug）".to_string());
    }

    let url = format!("{API_BASE}/project/{}/version", crate::http::urlencode(id));
    let value = cached_versions(&url).await?;

    let gv = game_version.as_deref().map(str::trim).unwrap_or("");
    let ld = loader.as_deref().map(str::trim).unwrap_or("");
    let apply_loader = matches!(kind.as_str(), "mod" | "modpack")
        && matches!(ld, "forge" | "neoforge" | "fabric" | "quilt");
    let versions = filter_versions(&value, gv, if apply_loader { ld } else { "" })?;

    serde_json::to_string(&versions).map_err(|e| format!("版本列表序列化失败：{e}"))
}

// ─────────────────────────────────────────────────────────────────────────────
// 命令：安装
// ─────────────────────────────────────────────────────────────────────────────

/// 下载主文件到 `<dir>/<对应目录>`，SHA1 校验后落位；返回 taskId。
///
/// 整合包使用独立的新实例安装入口。
#[tauri::command]
pub async fn modrinth_install(
    app: AppHandle,
    kind: String,
    slug_or_id: String,
    version_id: String,
    dir: String,
) -> Result<String, String> {
    let kind = normalize_kind(&kind)?;
    let folder = kind_folder(&kind).ok_or("整合包请从版本内页创建新实例")?;
    if slug_or_id.trim().is_empty() || version_id.trim().is_empty() {
        return Err("缺少项目或版本标识".into());
    }
    if dir.trim().is_empty() {
        return Err("缺少实例目录，请先选择目标实例".into());
    }
    let root = PathBuf::from(dir.trim());
    let context = crate::app_tasks::begin(&app, &root, None)?;
    context.describe_kind(crate::app_tasks::TaskKind::Resource, &slug_or_id);
    let task_id = context.id.clone();
    tauri::async_runtime::spawn(async move {
        let result = async {
            context.stage("正在加载", "Modrinth 版本信息");
            let api = minecraft_java_rs_core::net::transfer::TransferContext::new(
                context.network.community_api_source,
                context.network.file_download_source,
                context.transfer.cancellation().clone(),
            );
            let url = format!(
                "{API_BASE}/project/{}/version",
                crate::http::urlencode(&slug_or_id)
            );
            let (file_url, filename, sha1) = crate::community_network::get_json(
                crate::community_network::Platform::Modrinth,
                &url,
                &context.network,
                &api,
                None,
                |value| pick_primary_file(value, &version_id),
            )
            .await?;
            crate::community_network::install_resource(
                &root,
                folder,
                &filename,
                &file_url,
                minecraft_java_rs_core::net::transfer::FileHashes {
                    sha1: (!sha1.is_empty()).then_some(sha1),
                    ..Default::default()
                },
                &context,
            )
            .await
        }
        .await;
        context.finish(result.map(|_| None));
    });
    Ok(task_id)
}

// ─────────────────────────────────────────────────────────────────────────────
// 命令：打开项目页
// ─────────────────────────────────────────────────────────────────────────────

/// 用系统默认浏览器打开 Modrinth 项目页。
#[tauri::command]
pub fn modrinth_open_project(slug: String) -> Result<(), String> {
    let slug = slug.trim();
    if slug.is_empty() {
        return Err("缺少项目标识（slug）".to_string());
    }
    let url = format!("{PROJECT_PAGE_BASE}/{}", crate::http::urlencode(slug));
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| format!("打开项目页失败：{e}"))
}

// ─────────────────────────────────────────────────────────────────────────────
// 单元测试（纯逻辑，不联网）
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_whitelist_is_enforced() {
        for kind in ["mod", " resourcepack ", "SHADER", "datapack", "modpack"] {
            assert!(normalize_kind(kind).is_ok(), "{kind} 应在白名单内");
        }
        let err = normalize_kind("plugin").unwrap_err();
        assert!(err.contains("plugin"), "{err}");
        assert!(err.contains("mod"), "{err}");
    }

    #[test]
    fn kind_maps_to_instance_folder() {
        assert_eq!(kind_folder("mod"), Some("mods"));
        assert_eq!(kind_folder("resourcepack"), Some("resourcepacks"));
        assert_eq!(kind_folder("shader"), Some("shaderpacks"));
        assert_eq!(kind_folder("datapack"), Some("datapacks"));
        assert_eq!(kind_folder("modpack"), None, "整合包不安装到已有实例");
        assert_eq!(kind_folder("plugin"), None);
    }

    #[test]
    fn facets_include_project_type_and_version() {
        let facets = build_facets("mod", Some("1.20.1"), Some("forge"));
        assert_eq!(
            facets,
            r#"[["project_type:mod"],["versions:1.20.1"],["loaders:forge"]]"#
        );
    }

    #[test]
    fn facets_loader_filter_only_applies_to_mod() {
        // 其它种类不给 loader facet（Modrinth 上这些种类没有 loader 维度）。
        assert_eq!(
            build_facets("shader", Some("1.20.1"), Some("fabric")),
            r#"[["project_type:shader"],["versions:1.20.1"]]"#
        );
        // mod + vanilla / 空加载器 → 不过滤 loader。
        assert_eq!(
            build_facets("mod", None, Some("vanilla")),
            r#"[["project_type:mod"]]"#
        );
        assert_eq!(
            build_facets("mod", None, Some("")),
            r#"[["project_type:mod"]]"#
        );
        // 未提供 MC 版本时不带 versions facet。
        assert_eq!(
            build_facets("modpack", None, None),
            r#"[["project_type:modpack"]]"#
        );
    }

    #[test]
    fn facets_escape_json_strings() {
        // 版本号里出现引号 / 反斜杠时必须被 serde_json 转义，不能破坏 facets JSON。
        let facets = build_facets("mod", Some("1.20.1\"\\x"), None);
        assert_eq!(facets, r#"[["project_type:mod"],["versions:1.20.1\"\\x"]]"#);
        let parsed: serde_json::Value = serde_json::from_str(&facets).unwrap();
        assert_eq!(
            parsed[1][0].as_str().unwrap(),
            "versions:1.20.1\"\\x",
            "转义后应能原样解析回字符串"
        );
    }

    #[test]
    fn normalizes_search_response() {
        let sample: serde_json::Value = serde_json::from_str(
            r#"{
              "hits": [
                {
                  "project_id": "aabb",
                  "slug": "sodium",
                  "title": "Sodium",
                  "description": "A modern rendering engine",
                  "categories": ["fabric"],
                  "display_categories": ["optimization"],
                  "versions": ["1.21.4", "1.21.3"],
                  "downloads": 4800000,
                  "author": "jellysquid3",
                  "icon_url": "https://cdn.modrinth.com/sodium.png",
                  "date_modified": "2026-01-02T03:04:05Z",
                  "project_type": "mod",
                  "loaders": ["fabric"]
                },
                {"slug": "", "title": "无 slug 的脏数据"}
              ],
              "total": 42
            }"#,
        )
        .unwrap();

        let text = normalize_search(&sample).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["total"], 42);
        let hits = parsed["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1, "缺 slug 的脏数据必须丢弃");
        assert_eq!(hits[0]["slug"], "sodium");
        assert_eq!(hits[0]["iconUrl"], "https://cdn.modrinth.com/sodium.png");
        assert_eq!(hits[0]["projectType"], "mod");
        assert_eq!(hits[0]["dateModified"], "2026-01-02T03:04:05Z");
        assert_eq!(hits[0]["downloads"], 4800000);
        assert_eq!(hits[0]["versions"][0], "1.21.4");
        assert_eq!(hits[0]["loaders"][0], "fabric");
        // 输出不得残留 snake_case 字段。
        assert!(!text.contains("icon_url"));
        assert!(!text.contains("project_type\":"));
        assert!(!text.contains("date_modified"));
    }

    #[test]
    fn normalize_search_rejects_broken_shapes() {
        for bad in ["null", "{}", r#"{"hits":{}}"#, r#"{"hits":[]}"#] {
            let value: serde_json::Value = serde_json::from_str(bad).unwrap();
            let result = normalize_search(&value);
            // 空 hits 是合法结果（无命中），但结构缺失必须报错。
            assert_eq!(result.is_err(), bad != r#"{"hits":[]}"#, "{bad}");
        }
        let value: serde_json::Value = serde_json::from_str(r#"{"hits":[], "total": 0}"#).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&normalize_search(&value).unwrap()).unwrap();
        assert_eq!(parsed["total"], 0);
        assert_eq!(parsed["hits"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn versions_filter_by_game_version_and_loader() {
        let sample: serde_json::Value = serde_json::from_str(
            r#"[
              {
                "id": "v-fabric",
                "name": "Sodium 0.6.0",
                "version_number": "0.6.0",
                "loaders": ["fabric"],
                "game_versions": ["1.21.4"],
                "date_published": "2025-12-01T00:00:00Z",
                "files": [
                  {"url": "https://cdn.modrinth.com/sodium.jar", "filename": "sodium.jar",
                   "size": 1024, "primary": true, "hashes": {"sha1": "AAA"}}
                ]
              },
              {
                "id": "v-forge",
                "version_number": "0.5.0",
                "loaders": ["forge", "neoforge"],
                "game_versions": ["1.20.1"],
                "files": [
                  {"url": "https://cdn.modrinth.com/sodium-forge.jar", "filename": "sodium-forge.jar",
                   "size": 2048, "primary": false, "hashes": {"sha1": "BBB"}},
                  {"url": "https://cdn.modrinth.com/sodium-forge2.jar", "filename": "sodium-forge2.jar",
                   "size": 2048, "primary": true, "hashes": {"sha1": "CCC"}}
                ]
              },
              {"id": "", "version_number": "0.4.0"}
            ]"#,
        )
        .unwrap();

        // 不过滤 → 保留全部有效条目
        let all = filter_versions(&sample, "", "").unwrap();
        assert_eq!(all.len(), 2, "空 id 的脏数据必须丢弃");

        let fabric = filter_versions(&sample, "1.21.4", "fabric").unwrap();
        assert_eq!(fabric.len(), 1);
        assert_eq!(fabric[0].id, "v-fabric");
        assert_eq!(fabric[0].name, "Sodium 0.6.0");
        assert_eq!(fabric[0].files.len(), 1);
        assert!(fabric[0].files[0].primary);
        assert_eq!(fabric[0].files[0].hash_sha1, "AAA");

        // forge + 1.20.1 → v-forge（filter_versions 保持 API 原序；primary 的选择在安装时进行）
        let forge = filter_versions(&sample, "1.20.1", "forge").unwrap();
        assert_eq!(forge.len(), 1);
        assert_eq!(
            forge[0].files.len(),
            2,
            "文件列表原样保留，由 pick_primary_file 挑选"
        );
        let (url, filename, sha1) = pick_primary_file(&sample, "v-forge").unwrap();
        assert_eq!(filename, "sodium-forge2.jar", "primary 文件优先被选中");
        assert_eq!(url, "https://cdn.modrinth.com/sodium-forge2.jar");
        assert_eq!(sha1, "CCC");

        // 无匹配 → 空数组（前端展示「没有适配版本」空态）
        let none = filter_versions(&sample, "1.7.10", "").unwrap();
        assert!(none.is_empty());

        // 响应不是数组 → 可读错误
        assert!(filter_versions(&serde_json::Value::Null, "", "").is_err());
    }

    #[test]
    fn primary_file_selection_prefers_primary() {
        let sample: serde_json::Value = serde_json::from_str(
            r#"[
              {
                "id": "v1",
                "files": [
                  {"url": "https://cdn.modrinth.com/a.jar", "filename": "a.jar",
                   "size": 1, "primary": false, "hashes": {"sha1": "hex-a"}},
                  {"url": "https://cdn.modrinth.com/b.jar", "filename": "b.jar",
                   "size": 2, "primary": true, "hashes": {"sha1": "hex-b"}}
                ]
              }
            ]"#,
        )
        .unwrap();

        let (url, filename, sha1) = pick_primary_file(&sample, "v1").unwrap();
        assert_eq!(url, "https://cdn.modrinth.com/b.jar");
        assert_eq!(filename, "b.jar");
        assert_eq!(sha1, "hex-b");

        // 找不到指定版本 → 可读错误
        let err = pick_primary_file(&sample, "v-missing").unwrap_err();
        assert!(err.contains("v-missing") || err.contains("版本"), "{err}");
    }

    #[test]
    fn filename_falls_back_to_url_tail() {
        let sample: serde_json::Value = serde_json::from_str(
            r#"[{"id": "v1", "files": [{"url": "https://cdn.modrinth.com/xy/NoIndication.jar", "size": 1}]}]"#,
        )
        .unwrap();
        let (_, filename, sha1) = pick_primary_file(&sample, "v1").unwrap();
        assert_eq!(filename, "NoIndication.jar");
        assert_eq!(sha1, "", "缺少哈希时为空串（跳过校验）");

        // 缺少下载地址的文件 → 报错而不是假下载
        let broken: serde_json::Value =
            serde_json::from_str(r#"[{"id": "v1", "files": [{"filename": "x.jar"}]}]"#).unwrap();
        assert!(pick_primary_file(&broken, "v1").is_err());
    }

    #[test]
    fn sha1_comparison_is_case_insensitive() {
        assert!(sha1_matches("ABCDEF01", "abcdef01"));
        assert!(sha1_matches(" abcdef01 ", "ABCDEF01"));
        assert!(!sha1_matches("abcdef01", "abcdef02"));
        // 期望值为空 = 无校验值，视为通过（跳过校验的语义在调用处）。
        assert!(!sha1_matches("abcdef01", ""));
        assert!(!sha1_matches("", "abcdef01"));
    }

    #[test]
    fn release_types_do_not_promote_unknown_to_release() {
        for (raw, expected) in [
            ("release", "release"),
            ("beta", "beta"),
            ("alpha", "alpha"),
            ("future", "unknown"),
        ] {
            assert_eq!(
                normalize_release_type(Some(&serde_json::json!(raw))),
                expected
            );
        }
        assert_eq!(normalize_release_type(None), "unknown");
        assert_eq!(
            normalize_release_type(Some(&serde_json::json!(1))),
            "unknown"
        );
    }
}
