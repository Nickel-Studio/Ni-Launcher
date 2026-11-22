//! CurseForge API 与普通资源安装；无 Key 时仍尝试社区 API 镜像。

use std::path::PathBuf;

use serde::Serialize;
use tauri::AppHandle;

/// CFCore API 根地址（Studios / REST API v1）。
const API_BASE: &str = "https://api.curseforge.com";
/// 项目页地址（`curseforge_open_project`）。
const PROJECT_PAGE_BASE: &str = "https://www.curseforge.com/minecraft";

// ─────────────────────────────────────────────────────────────────────────────
// 映射表（依据见模块注释；改动前先对照官方 Categories / ModLoaderType 枚举）
// ─────────────────────────────────────────────────────────────────────────────

/// Minecraft 游戏 id（CFCore 全平台共用，Minecraft = 432）。
const GAME_ID: u32 = 432;

/// 前端 kind → CurseForge classId。
fn class_id_of(kind: &str) -> Option<u32> {
    match kind {
        "mod" => Some(6),           // Mods
        "modpack" => Some(4471),    // Modpacks
        "resourcepack" => Some(12), // Resource Packs
        "shader" => Some(6552),     // Shaders
        "datapack" => Some(6945),   // Data Packs
        _ => None,
    }
}

/// 前端加载器标识 → CurseForge ModLoaderType。
fn mod_loader_type_of(loader: &str) -> Option<u32> {
    match loader {
        "forge" => Some(1), // Forge
        "quilt" => Some(5),
        "fabric" => Some(4),   // Fabric
        "neoforge" => Some(6), // NeoForge
        _ => None,             // vanilla / optifine / 未知 → 不过滤
    }
}

/// CurseForge File 的 loaders 标签 → 前端加载器标识（归一化到小写）。
/// CF 返回的 loader 名如 "Forge" / "Fabric" / "NeoForge"，直接小写即可对齐。
fn normalize_loader_names(names: &[String]) -> Vec<String> {
    names
        .iter()
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// 归一化数据结构（与 modrinth.rs 同形，字段即前端契约，顺序即 JSON 字段顺序）
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
    /// CurseForge 的数字项目 id（`/v1/mods/{id}/files` 安装链路必需；
    /// slug 只用于项目页 URL，两者都必须随 hit 下发）。
    project_id: String,
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
    project_id: String,
    name: String,
    version_number: String,
    loaders: Vec<String>,
    game_versions: Vec<String>,
    date_published: String,
    release_type: String,
    files: Vec<VersionFile>,
    dependencies: Option<Vec<crate::community_dependencies::RequiredDependency>>,
}

// ─────────────────────────────────────────────────────────────────────────────
// 纯逻辑（便于单元测试）
// ─────────────────────────────────────────────────────────────────────────────

/// 校验并归一化资源种类（前端五种分类 → CurseForge 的 classId 维度）。
fn normalize_release_type(value: Option<&serde_json::Value>) -> String {
    match value.and_then(|v| v.as_u64()) {
        Some(1) => "release",
        Some(2) => "beta",
        Some(3) => "alpha",
        _ => "unknown",
    }
    .into()
}

fn normalize_kind(kind: &str) -> Result<String, String> {
    let kind = kind.trim().to_ascii_lowercase();
    if class_id_of(&kind).is_some() {
        return Ok(kind);
    }
    Err(format!(
        "不支持的资源种类：{kind}（可选 mod / resourcepack / shader / datapack / modpack）"
    ))
}

/// kind → 实例子目录名。整合包由独立入口创建新实例。
fn kind_folder(kind: &str) -> Option<&'static str> {
    match kind {
        "mod" => Some("mods"),
        "resourcepack" => Some("resourcepacks"),
        "shader" => Some("shaderpacks"),
        "datapack" => Some("datapacks"),
        _ => None,
    }
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

/// CFCore 搜索响应 → 前端契约。
///
/// 官方文档：`GET /v1/mods/search` 返回 `{ data: [Mod], pagination: { index,
/// pageSize, total } }`，条目是**平铺的 Mod 对象**（id / slug / name / summary /
/// logo / authors / downloadCount / dateModified / modLoaderTypes / classId …）。
/// 这里同时兼容早期的嵌套包裹形态（条目内再套一层 `mod`），以实测为准。
pub(crate) fn normalize_search(value: &serde_json::Value) -> Result<String, String> {
    let data = value
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| "CurseForge 搜索结果格式无法识别".to_string())?;
    let total = value
        .get("pagination")
        .and_then(|p| p.get("totalCount").or_else(|| p.get("total")))
        .and_then(|t| t.as_u64());
    let out: Vec<SearchHit> = data
        .iter()
        .filter_map(|m| {
            let slug = str_field(m, "slug");
            if slug.is_empty() {
                return None;
            }
            let project_id = m
                .get("id")
                .and_then(|v| v.as_i64())
                .map(|v| v.to_string())
                .unwrap_or_default();
            let author = m
                .get("authors")
                .and_then(|a| a.as_array())
                .and_then(|a| a.first())
                .map(|a| str_field(a, "name"))
                .unwrap_or_default();
            let loaders = normalize_loader_names(&str_array(m, "modLoaderTypes"));
            let title = {
                let name = str_field(m, "name");
                if name.is_empty() { slug.clone() } else { name }
            };
            let icon_url = {
                let thumb = m
                    .get("logo")
                    .map(|l| str_field(l, "thumbnailUrl"))
                    .unwrap_or_default();
                if thumb.is_empty() {
                    m.get("logo")
                        .map(|l| str_field(l, "url"))
                        .unwrap_or_default()
                } else {
                    thumb
                }
            };
            Some(SearchHit {
                slug,
                title,
                description: str_field(m, "summary"),
                icon_url,
                downloads: m
                    .get("downloadCount")
                    .and_then(|d| d.as_f64())
                    .map(|d| d as u64)
                    .unwrap_or(0),
                author,
                project_type: m
                    .get("classId")
                    .and_then(|c| c.as_u64())
                    .map(|c| format!("cf-class-{c}"))
                    .unwrap_or_default(),
                versions: Vec::new(),
                loaders,
                date_modified: str_field(m, "dateModified"),
                project_id,
            })
        })
        .collect();
    let total = total.unwrap_or(out.len() as u64);
    serde_json::to_string(&SearchResult { total, hits: out })
        .map_err(|e| format!("搜索结果序列化失败：{e}"))
}

/// CFCore 文件响应归一化；relationType=3 的必要前置保留在所属文件中。
fn normalize_files(data: &[serde_json::Value]) -> Vec<ProjectVersion> {
    let mut out = Vec::new();
    for file in data {
        let id = file
            .get("id")
            .and_then(|v| v.as_i64())
            .map(|v| v.to_string());
        let Some(id) = id else {
            continue;
        };
        let mut display_name = str_field(file, "displayName");
        if display_name.is_empty() {
            display_name = str_field(file, "fileName");
        }
        let hash_sha1 = file
            .get("hashes")
            .and_then(|h| h.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|h| h.get("algo").and_then(|a| a.as_i64()) == Some(1))
            })
            .map(|h| str_field(h, "value"))
            .unwrap_or_default();
        let url = str_field(file, "downloadUrl");
        let filename = str_field(file, "fileName");
        let mut loaders = normalize_loader_names(&str_array(file, "modLoaderTypes"));
        for tag in str_array(file, "gameVersions") {
            let tag = tag.to_ascii_lowercase();
            if matches!(tag.as_str(), "forge" | "neoforge" | "fabric" | "quilt")
                && !loaders.contains(&tag)
            {
                loaders.push(tag);
            }
        }
        out.push(ProjectVersion {
            id,
            project_id: file
                .get("modId")
                .and_then(|value| value.as_u64())
                .map(|id| id.to_string())
                .unwrap_or_default(),
            name: display_name,
            version_number: str_field(file, "fileName"),
            loaders,
            game_versions: str_array(file, "gameVersions")
                .into_iter()
                .filter(|v| {
                    v.chars().next().is_some_and(|c| c.is_ascii_digit())
                        || v.starts_with(['a', 'b'])
                        || v.starts_with("rd-")
                        || v.starts_with("inf-")
                })
                .collect(),
            date_published: str_field(file, "fileDate"),
            release_type: normalize_release_type(file.get("releaseType")),
            files: if url.is_empty() {
                Vec::new()
            } else {
                vec![VersionFile {
                    url,
                    filename,
                    size_bytes: file.get("fileLength").and_then(|s| s.as_u64()).unwrap_or(0),
                    primary: true,
                    hash_sha1,
                }]
            },
            dependencies: crate::community_dependencies::curseforge_dependencies(file),
        });
    }
    out
}

pub(crate) fn normalize_dependency_versions(
    files: &[serde_json::Value],
) -> Result<serde_json::Value, String> {
    serde_json::to_value(normalize_files(files))
        .map_err(|error| format!("前置文件序列化失败：{error}"))
}

fn normalize_file_page(
    value: &serde_json::Value,
    data: &[serde_json::Value],
    offset: u32,
    limit: u32,
) -> Result<String, String> {
    let total = value["pagination"]["totalCount"]
        .as_u64()
        .or_else(|| value["pagination"]["total"].as_u64());
    let count = data.len() as u64;
    let next_offset = u64::from(offset) + count;
    let has_more = count > 0
        && total
            .map(|t| next_offset < t)
            .unwrap_or(count >= u64::from(limit));
    serde_json::to_string(
        &serde_json::json!({ "versions": normalize_files(data), "offset": offset,
        "nextOffset": next_offset, "hasMore": has_more, "total": total }),
    )
    .map_err(|e| format!("版本分页序列化失败：{e}"))
}

/// SHA1 十六进制比较：忽略大小写与首尾空白；期望值为空时视为「无校验值」。
#[cfg(test)]
fn sha1_matches(actual: &str, expected: &str) -> bool {
    let expected = expected.trim().to_ascii_lowercase();
    !expected.is_empty() && actual.trim().eq_ignore_ascii_case(&expected)
}

/// 从 CF 文件列表中选出要下载的文件（`file_id` 必须命中）。
pub(crate) fn pick_file(
    files: &[serde_json::Value],
    file_id: &str,
) -> Result<(String, String, String), String> {
    let file = files
        .iter()
        .find(|f| {
            f.get("id")
                .and_then(|v| v.as_i64())
                .map(|v| v.to_string())
                .as_deref()
                == Some(file_id)
        })
        .ok_or_else(|| "指定的文件在 CurseForge 上不存在或已被删除".to_string())?;

    let url = str_field(file, "downloadUrl");
    if url.is_empty() {
        return Err("该文件的下载地址不可用（作者可能未开放该文件的分发）".to_string());
    }
    let mut filename = str_field(file, "fileName");
    if filename.is_empty() {
        filename = url.rsplit('/').next().unwrap_or("").to_string();
    }
    if filename.is_empty() {
        return Err("该文件的文件名不可用".to_string());
    }
    let sha1 = file
        .get("hashes")
        .and_then(|h| h.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|h| h.get("algo").and_then(|a| a.as_i64()) == Some(1))
        })
        .map(|h| str_field(h, "value"))
        .unwrap_or_default();
    Ok((url, filename, sha1))
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP（带 x-api-key；独立社区候选）
// ─────────────────────────────────────────────────────────────────────────────

/// GET JSON，带 CFCore 的 `x-api-key` 头；HTTP 错误转成可读中文（不回显 key）。
async fn get_json(url: &str, api_key: Option<&crate::secrets::CurseforgeCredential>) -> Result<serde_json::Value, String> {
    let settings = crate::network_settings::snapshot();
    let context = crate::community_network::context(&settings);
    crate::community_network::get_json(
        crate::community_network::Platform::Curseforge,
        url,
        &settings,
        &context,
        api_key,
        |value| {
            if !value["data"].is_array() {
                return Err("CurseForge 响应结构无效，请重试".into());
            }
            Ok(value.clone())
        },
    )
    .await
}

// ─────────────────────────────────────────────────────────────────────────────
// 命令：配置状态
// ─────────────────────────────────────────────────────────────────────────────

/// CurseForge 引擎是否已配置 API Key。
/// 只返回布尔值，**绝不回传 key 本身**；未配置时前端给出 README 指引。
#[tauri::command]
pub async fn cf_config_status(app: AppHandle) -> Result<String, String> {
    let configured = crate::secrets::curseforge_api_key(app)?.is_some();
    serde_json::to_string(&serde_json::json!({ "configured": configured }))
        .map_err(|e| format!("序列化失败：{e}"))
}

// ─────────────────────────────────────────────────────────────────────────────
// 命令：检索
// ─────────────────────────────────────────────────────────────────────────────

/// 检索 CurseForge 资源。`kind` ∈ mod | resourcepack | shader | datapack | modpack。
///
/// loader 过滤：只对 `mod` 生效，且官方要求 modLoaderType 必须与 gameVersion
/// 同时使用（见模块注释），因此两者都非空才带该参数。
#[tauri::command]
pub async fn curseforge_search(
    app: AppHandle,
    kind: String,
    query: Option<String>,
    game_version: Option<String>,
    loader: Option<String>,
    offset: Option<u32>,
    limit: Option<u32>,
) -> Result<String, String> {
    let kind = normalize_kind(&kind)?;
    let api_key = crate::secrets::curseforge_api_key(app.clone())?;
    let class_id = class_id_of(&kind).ok_or_else(|| format!("不支持的资源种类：{kind}"))?;
    let offset = offset.unwrap_or(0).min(10_000);
    let limit = limit.unwrap_or(20).clamp(1, 50); // CF 官方 pageSize 上限 50

    let gv = game_version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let ld = loader.as_deref().map(str::trim).unwrap_or("");
    // loader 只对 mod 生效；modLoaderType 必须与 gameVersion 成对出现（官方约束）。
    let loader_type = if matches!(kind.as_str(), "mod" | "modpack") && gv.is_some() {
        mod_loader_type_of(ld)
    } else {
        None
    };

    let mut url = format!(
        "{API_BASE}/v1/mods/search?gameId={GAME_ID}&classId={class_id}&index={offset}&pageSize={limit}&sortField=2&sortOrder=desc"
    );
    if let Some(q) = query.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        url.push_str(&format!("&searchFilter={}", crate::http::urlencode(q)));
    }
    if let Some(gv) = gv {
        url.push_str(&format!("&gameVersion={}", crate::http::urlencode(gv)));
    }
    if let Some(loader_type) = loader_type {
        url.push_str(&format!("&modLoaderType={loader_type}"));
    }

    let value = get_json(&url, api_key.as_ref()).await?;
    normalize_search(&value)
}

/// 列出某项目的文件（按 MC 版本与加载器过滤；loader 只对 mod 生效）。
/// 返回结构与 `modrinth_project_versions` 同形：CF 的每个 File 即一个「版本」。
#[tauri::command]
pub async fn curseforge_project_files(
    app: AppHandle,
    kind: String,
    project_id: String,
    game_version: Option<String>,
    loader: Option<String>,
    offset: Option<u32>,
    limit: Option<u32>,
) -> Result<String, String> {
    let kind = normalize_kind(&kind)?;
    let api_key = crate::secrets::curseforge_api_key(app.clone())?;
    let project_id = project_id.trim();
    if project_id.is_empty() {
        return Err("缺少项目编号（projectId）".to_string());
    }
    let mod_id: i64 = project_id
        .parse()
        .map_err(|_| "项目编号必须是 CurseForge 的数字 id（可从项目页地址末段查看）".to_string())?;

    let gv = game_version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let ld = loader.as_deref().map(str::trim).unwrap_or("");
    let loader_type = if kind == "mod" && gv.is_some() {
        mod_loader_type_of(ld)
    } else {
        None
    };

    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(50).clamp(1, 50);
    let mut url = format!("{API_BASE}/v1/mods/{mod_id}/files?pageSize={limit}&index={offset}");
    if let Some(gv) = gv {
        url.push_str(&format!("&gameVersion={}", crate::http::urlencode(gv)));
    }
    if let Some(loader_type) = loader_type {
        url.push_str(&format!("&modLoaderType={loader_type}"));
    }

    let value = get_json(&url, api_key.as_ref()).await?;
    let data = value
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| "CurseForge 文件数据格式无法识别".to_string())?;
    normalize_file_page(&value, data, offset, limit)
}

// ─────────────────────────────────────────────────────────────────────────────
// 命令：安装
// ─────────────────────────────────────────────────────────────────────────────

/// 下载指定文件到 `<dir>/<对应目录>`，SHA1 校验后落位；返回 taskId。
///
/// 整合包使用独立的新实例安装入口。
#[tauri::command]
pub async fn curseforge_install(
    app: AppHandle,
    kind: String,
    project_id: String,
    file_id: String,
    dir: String,
) -> Result<String, String> {
    let kind = normalize_kind(&kind)?;
    let folder = kind_folder(&kind).ok_or("整合包请从版本内页创建新实例")?;
    let project_id = project_id
        .parse::<u64>()
        .map_err(|_| "CurseForge 项目 ID 必须为正整数")?;
    let file_id = file_id
        .parse::<u64>()
        .map_err(|_| "CurseForge 文件 ID 必须为正整数")?;
    if project_id == 0 || file_id == 0 {
        return Err("CurseForge 项目及文件 ID 必须为正整数".into());
    }
    if dir.trim().is_empty() {
        return Err("缺少实例目录，请先选择目标实例".into());
    }
    let root = PathBuf::from(dir.trim());
    let key = crate::secrets::curseforge_api_key(app.clone())?;
    let context = crate::app_tasks::begin(&app, &root, None)?;
    context.describe_kind(
        crate::app_tasks::TaskKind::Resource,
        &format!("CurseForge {project_id}"),
    );
    let task_id = context.id.clone();
    tauri::async_runtime::spawn(async move {
        let result = async {
            context.stage("正在加载", "CurseForge 文件信息");
            let api = minecraft_java_rs_core::net::transfer::TransferContext::new(
                context.network.community_api_source,
                context.network.file_download_source,
                context.transfer.cancellation().clone(),
            );
            let url = format!("{API_BASE}/v1/mods/{project_id}/files/{file_id}");
            let (file_url, filename, sha1) = crate::community_network::get_json(
                crate::community_network::Platform::Curseforge,
                &url,
                &context.network,
                &api,
                key.as_ref(),
                |value| {
                    let file = &value["data"];
                    if file["modId"].as_u64() != Some(project_id) {
                        return Err("CurseForge 文件不属于所选项目".into());
                    }
                    pick_file(std::slice::from_ref(file), &file_id.to_string())
                },
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

/// 用系统默认浏览器打开 CurseForge 项目页。
/// 项目页地址形如 `/minecraft/<category>/<slug>`；无 category 时的 slug 直达地址
/// 由 CurseForge 重定向，这里按检索时的 kind 推断 category。
#[tauri::command]
pub fn curseforge_open_project(kind: String, slug: String) -> Result<(), String> {
    let slug = slug.trim();
    if slug.is_empty() {
        return Err("缺少项目标识（slug）".to_string());
    }
    let kind = normalize_kind(&kind)?;
    let category = match kind.as_str() {
        "mod" => "mc-mods",
        "modpack" => "modpacks",
        "resourcepack" => "texture-packs",
        "shader" => "shaders",
        "datapack" => "data-packs",
        _ => "mc-mods",
    };
    let url = format!(
        "{PROJECT_PAGE_BASE}/{category}/{}",
        crate::http::urlencode(slug)
    );
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| format!("打开项目页失败：{e}"))
}

// ─────────────────────────────────────────────────────────────────────────────
// 单元测试（纯逻辑，不联网）
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_whitelist_matches_class_ids() {
        for kind in ["mod", " resourcepack ", "SHADER", "datapack", "modpack"] {
            assert!(normalize_kind(kind).is_ok(), "{kind} 应在白名单内");
        }
        let err = normalize_kind("plugin").unwrap_err();
        assert!(err.contains("plugin"), "{err}");
        assert!(err.contains("mod"), "{err}");
    }

    #[test]
    fn class_ids_match_official_documentation() {
        // 对照 https://docs.curseforge.com/rest-api/ 的 Categories 大类。
        assert_eq!(class_id_of("mod"), Some(6));
        assert_eq!(class_id_of("modpack"), Some(4471));
        assert_eq!(class_id_of("resourcepack"), Some(12));
        assert_eq!(class_id_of("shader"), Some(6552));
        assert_eq!(class_id_of("datapack"), Some(6945));
        assert_eq!(class_id_of("plugin"), None);
    }

    #[test]
    fn mod_loader_types_match_official_enumeration() {
        // ModLoaderType：Any=0, Forge=1, Cauldron=2, LiteLoader=3, Fabric=4, Quilt=5, NeoForge=6。
        assert_eq!(mod_loader_type_of("forge"), Some(1));
        assert_eq!(mod_loader_type_of("fabric"), Some(4));
        assert_eq!(mod_loader_type_of("neoforge"), Some(6));
        // vanilla / optifine / 空串 → 不过滤（None）。
        assert_eq!(mod_loader_type_of("vanilla"), None);
        assert_eq!(mod_loader_type_of("optifine"), None);
        assert_eq!(mod_loader_type_of(""), None);
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
    fn normalizes_search_response_to_modrinth_shape() {
        // CFCore 搜索响应：{ data: [Mod 平铺对象], pagination: { total } }。
        let sample: serde_json::Value = serde_json::from_str(
            r#"{
              "data": [
                {
                  "id": 238222,
                  "name": "Just Enough Items (JEI)",
                  "slug": "jei",
                  "summary": "JEI is an item and recipe viewing mod",
                  "downloadCount": 48000000.0,
                  "dateModified": "2026-01-02T03:04:05.000Z",
                  "classId": 6,
                  "modLoaderTypes": ["Forge", "NeoForge"],
                  "logo": { "thumbnailUrl": "https://media.forgecdn.net/jei-thumb.png", "url": "https://media.forgecdn.net/jei.png" },
                  "authors": [ { "name": "mezz" } ]
                },
                { "id": 2, "slug": "", "name": "无 slug 的脏数据" }
              ],
              "pagination": { "index": 0, "pageSize": 20, "total": 42 }
            }"#,
        )
        .unwrap();

        let text = normalize_search(&sample).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["total"], 42);
        let hits = parsed["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1, "缺 slug 的脏数据必须丢弃");
        assert_eq!(hits[0]["slug"], "jei");
        assert_eq!(hits[0]["projectId"], "238222", "安装链路需要数字项目 id");
        assert_eq!(hits[0]["title"], "Just Enough Items (JEI)");
        assert_eq!(
            hits[0]["iconUrl"],
            "https://media.forgecdn.net/jei-thumb.png"
        );
        assert_eq!(hits[0]["downloads"], 48000000u64);
        assert_eq!(hits[0]["author"], "mezz");
        assert_eq!(hits[0]["dateModified"], "2026-01-02T03:04:05.000Z");
        assert_eq!(
            hits[0]["loaders"],
            serde_json::json!(["forge", "neoforge"]),
            "loaders 必须归一化为小写（与 Modrinth 卡片一致）"
        );
        // 输出不得残留 snake_case 字段。
        assert!(!text.contains("icon_url"));
        assert!(!text.contains("date_modified"));
        assert!(!text.contains("project_id"));
        // 与 modrinth_search 同形的结构键：slug/title/description/iconUrl/downloads/
        // author/projectType/versions/loaders/dateModified。
        for key in [
            "slug",
            "title",
            "description",
            "iconUrl",
            "downloads",
            "author",
            "projectType",
            "versions",
            "loaders",
            "dateModified",
        ] {
            assert!(
                hits[0].get(key).is_some(),
                "hit 缺少与 Modrinth 同形的字段 {key}"
            );
        }
    }

    #[test]
    fn normalize_search_rejects_broken_shapes() {
        for bad in ["null", "{}", r#"{"data":{}}"#, r#"{"data":null}"#] {
            let value: serde_json::Value = serde_json::from_str(bad).unwrap();
            assert!(normalize_search(&value).is_err(), "{bad} 应报格式错误");
        }
        let value: serde_json::Value =
            serde_json::from_str(r#"{"data":[],"pagination":{"total":0}}"#).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&normalize_search(&value).unwrap()).unwrap();
        assert_eq!(parsed["total"], 0);
        assert_eq!(parsed["hits"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn normalizes_files_to_modrinth_version_shape() {
        let sample: serde_json::Value = serde_json::from_str(
            r#"{
              "data": [
                {
                  "id": 5550001,
                  "displayName": "JEI 1.21.4 19.21.4.250",
                  "fileName": "jei-1.21.4-fabric-19.21.4.250.jar",
                  "fileDate": "2025-12-01T00:00:00.000Z",
                  "fileLength": 2345678,
                  "downloadUrl": "https://media.forgecdn.net/files/5550/1/jei.jar",
                  "gameVersions": ["1.21.4", "Fabric"],
                  "modLoaderTypes": ["Fabric"],
                  "hashes": [
                    { "value": "aaaa1111bbbb2222cccc3333dddd4444eeee5555", "algo": 1 },
                    { "value": "md5-value", "algo": 2 }
                  ]
                },
                { "id": 5550002, "fileName": "no-url.jar" },
                { "fileName": "missing-id.jar" }
              ]
            }"#,
        )
        .unwrap();
        let data = sample["data"].as_array().unwrap();

        let versions = normalize_files(data);
        assert_eq!(versions.len(), 2, "缺 id 的脏数据必须丢弃");
        assert_eq!(versions[0].id, "5550001");
        assert_eq!(versions[0].name, "JEI 1.21.4 19.21.4.250");
        assert_eq!(
            versions[0].version_number,
            "jei-1.21.4-fabric-19.21.4.250.jar"
        );
        assert_eq!(versions[0].loaders, vec!["fabric"]);
        assert_eq!(versions[0].game_versions, vec!["1.21.4"]);
        assert_eq!(versions[0].files.len(), 1);
        assert_eq!(
            versions[0].files[0].hash_sha1,
            "aaaa1111bbbb2222cccc3333dddd4444eeee5555"
        );
        assert_eq!(versions[0].files[0].size_bytes, 2345678);
        // 无 downloadUrl 的文件：版本保留（可展示元数据），files 为空 → 前端禁用安装。
        assert_eq!(versions[1].id, "5550002");
        assert!(versions[1].files.is_empty());
    }

    #[test]
    fn pick_file_requires_existing_id_and_url() {
        let sample: serde_json::Value = serde_json::from_str(
            r#"{
              "id": 5550001,
              "fileName": "jei.jar",
              "downloadUrl": "https://media.forgecdn.net/files/5550/1/jei.jar",
              "hashes": [ { "value": "AAaa1111", "algo": 1 } ]
            }"#,
        )
        .unwrap();
        let files = vec![sample];

        let (url, filename, sha1) = pick_file(&files, "5550001").unwrap();
        assert_eq!(url, "https://media.forgecdn.net/files/5550/1/jei.jar");
        assert_eq!(filename, "jei.jar");
        assert_eq!(sha1, "AAaa1111");

        // 未命中的 file_id → 可读错误。
        let err = pick_file(&files, "9999").unwrap_err();
        assert!(err.contains("不存在"), "{err}");
        // 无 downloadUrl → 拒绝假下载。
        let broken: serde_json::Value =
            serde_json::from_str(r#"{ "id": 7, "fileName": "x.jar" }"#).unwrap();
        let err = pick_file(&[broken], "7").unwrap_err();
        assert!(err.contains("下载地址"), "{err}");
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
            (1, "release"),
            (2, "beta"),
            (3, "alpha"),
            (0, "unknown"),
            (9, "unknown"),
        ] {
            assert_eq!(
                normalize_release_type(Some(&serde_json::json!(raw))),
                expected
            );
        }
        assert_eq!(normalize_release_type(None), "unknown");
    }
    #[test]
    fn historical_files_beyond_first_fifty_have_real_offsets() {
        let page: Vec<_> = (1..=50)
            .map(|id| serde_json::json!({"id":id,"gameVersions":["1.20.1","Forge"]}))
            .collect();
        let response = serde_json::json!({"pagination":{"totalCount":75}});
        let first: serde_json::Value =
            serde_json::from_str(&normalize_file_page(&response, &page, 0, 50).unwrap()).unwrap();
        assert_eq!(first["hasMore"], true);
        assert_eq!(first["nextOffset"], 50);
        assert_eq!(first["total"], 75);
        let second: serde_json::Value =
            serde_json::from_str(&normalize_file_page(&response, &page[..25], 50, 50).unwrap())
                .unwrap();
        assert_eq!(second["hasMore"], false);
        assert_eq!(second["nextOffset"], 75);
        let unknown: serde_json::Value = serde_json::from_str(
            &normalize_file_page(&serde_json::json!({}), &page, 0, 50).unwrap(),
        )
        .unwrap();
        assert!(unknown["total"].is_null());
        assert_eq!(unknown["hasMore"], true);
    }
}
