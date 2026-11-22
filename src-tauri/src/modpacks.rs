//! 社区整合包清单与安装计划。只解析声明，不执行包内脚本。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;

const MAX_MANIFEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_PACK_FILES: usize = 100_000;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_PACK_BYTES: u64 = 32 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackPlatform {
    Modrinth,
    Curseforge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackRequest {
    pub platform: String,
    pub project_id: String,
    pub version_id: String,
    #[serde(default)]
    pub instance_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackLoader {
    pub kind: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackFile {
    Download {
        path: String,
        downloads: Vec<String>,
        size: u64,
        sha1: Option<String>,
        sha512: Option<String>,
    },
    Curseforge {
        project_id: u64,
        file_id: u64,
        required: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackManifest {
    pub name: String,
    pub minecraft_version: String,
    pub loader: Option<PackLoader>,
    pub files: Vec<PackFile>,
    pub overrides: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackSource {
    pub platform: PackPlatform,
    pub project_id: String,
    pub version_id: String,
    pub name: String,
    pub version_name: String,
}

#[derive(Debug, Clone)]
pub struct PackPlan {
    pub display_name: String,
    pub minecraft_version: String,
    pub loader: Option<PackLoader>,
    pub source: Value,
    archive: PathBuf,
    overrides: Vec<String>,
    files: Vec<ResolvedFile>,
    /// 仅便携载荷携带；标准 mrpack / CurseForge 包保持 None。
    pub instance_settings: Option<Value>,
}

#[derive(Debug, Clone)]
struct ResolvedFile {
    path: String,
    downloads: Vec<String>,
    size: u64,
    hashes: minecraft_java_rs_core::net::transfer::FileHashes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackError {
    pub code: String,
    pub message: String,
    pub item: Option<String>,
}

impl PackError {
    pub fn new(code: &str, message: impl Into<String>, item: Option<&str>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            item: item.map(str::to_owned),
        }
    }
}
impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for PackError {}

fn invalid(item: &str, reason: &str) -> PackError {
    PackError::new(
        "invalid_manifest",
        format!("整合包清单 {item}：{reason}"),
        Some(item),
    )
}

fn parse_json(bytes: &[u8]) -> Result<Value, PackError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("manifest", "文件超过大小上限"));
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| invalid("manifest", "JSON 格式无效"))?;
    if !value.is_object() {
        return Err(invalid("manifest", "应为 JSON 对象"));
    }
    Ok(value)
}

fn text(value: &Value, key: &str) -> Result<String, PackError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= 512 && !s.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(|| invalid(key, "缺少有效字符串"))
}

fn exact_version(value: &str, key: &str) -> Result<String, PackError> {
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.contains(['/', '\\', ':', '*', '?', '<', '>', '|', '"'])
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
        || value == "."
        || value == ".."
    {
        return Err(invalid(key, "需要精确版本标识"));
    }
    Ok(value.into())
}

/// 清单路径先校验一次；归档提取和最终落盘仍须在 network 的路径围栏内。
pub fn validate_pack_path(path: &str) -> Result<(), PackError> {
    if path.is_empty() || path.len() > 1024 || path.starts_with('/') || path.contains('\\') {
        return Err(PackError::new(
            "unsafe_path",
            format!("整合包包含无效路径：{path}"),
            Some(path),
        ));
    }
    let components: Vec<&str> = path.split('/').collect();
    for part in &components {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part.is_empty()
            || *part == "."
            || *part == ".."
            || part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|c| c.is_control() || ":*?<>|\"".contains(c))
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(PackError::new(
                "unsafe_path",
                format!("整合包包含无效路径：{path}"),
                Some(path),
            ));
        }
    }
    let root = components[0].to_ascii_lowercase();
    if root.starts_with("nickel.")
        || root.starts_with(".nickel")
        || matches!(
            root.as_str(),
            "gamedata.json"
                | "launcher_profiles.json"
                | "launcher_accounts.json"
                | "versions"
                | "libraries"
                | "assets"
                | "runtime"
                | "natives"
        )
    {
        return Err(PackError::new(
            "protected_path",
            format!("整合包不能覆盖启动器控制文件：{path}"),
            Some(path),
        ));
    }
    Ok(())
}

fn hash(value: &Value, key: &str, length: usize) -> Result<Option<String>, PackError> {
    let Some(raw) = value.get(key) else {
        return Ok(None);
    };
    let raw = raw
        .as_str()
        .filter(|s| s.len() == length && s.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| invalid(key, "校验值格式无效"))?;
    Ok(Some(raw.to_ascii_lowercase()))
}

fn mr_loader(
    dependencies: &serde_json::Map<String, Value>,
) -> Result<(String, Option<PackLoader>), PackError> {
    let mut minecraft = None;
    let mut loader = None;
    for (name, value) in dependencies {
        let version = value
            .as_str()
            .ok_or_else(|| invalid(name, "版本必须是字符串"))?;
        let version = exact_version(version, name)?;
        if name == "minecraft" {
            minecraft = Some(version);
            continue;
        }
        let kind = match name.as_str() {
            "forge" => "forge",
            "neoforge" => "neoforge",
            "fabric-loader" => "fabric",
            "quilt-loader" => "quilt",
            _ => {
                return Err(PackError::new(
                    "unsupported_dependency",
                    format!("整合包声明了不支持的依赖：{name}，请选择其他版本"),
                    Some(name),
                ));
            }
        };
        if loader.is_some() {
            return Err(invalid("dependencies", "不能同时安装多个加载器"));
        }
        loader = Some(PackLoader {
            kind: kind.into(),
            version,
        });
    }
    Ok((
        minecraft.ok_or_else(|| invalid("minecraft", "缺少游戏版本"))?,
        loader,
    ))
}

pub fn parse_modrinth_manifest(bytes: &[u8]) -> Result<PackManifest, PackError> {
    let value = parse_json(bytes)?;
    if value["formatVersion"].as_u64() != Some(1) {
        return Err(invalid("formatVersion", "仅支持格式版本 1"));
    }
    if value["game"].as_str() != Some("minecraft") {
        return Err(invalid("game", "仅支持 Minecraft 整合包"));
    }
    let name = text(&value, "name")?;
    let dependencies = value["dependencies"]
        .as_object()
        .ok_or_else(|| invalid("dependencies", "缺少依赖声明"))?;
    let (minecraft_version, loader) = mr_loader(dependencies)?;
    let entries = value["files"]
        .as_array()
        .ok_or_else(|| invalid("files", "缺少文件列表"))?;
    if entries.len() > MAX_PACK_FILES {
        return Err(invalid("files", "文件数量超过上限"));
    }
    let mut paths = HashSet::new();
    let mut files = Vec::new();
    let mut total = 0_u64;
    for entry in entries {
        let path = text(entry, "path")?;
        validate_pack_path(&path)?;
        if !paths.insert(path.to_lowercase()) {
            return Err(invalid(&path, "文件路径重复或大小写冲突"));
        }
        let client = match entry.get("env") {
            None => "required",
            Some(env) => {
                let env = env
                    .as_object()
                    .ok_or_else(|| invalid(&path, "env 必须是对象"))?;
                for (side, setting) in env {
                    if !matches!(side.as_str(), "client" | "server")
                        || !matches!(
                            setting.as_str(),
                            Some("required" | "optional" | "unsupported")
                        )
                    {
                        return Err(invalid(&path, "环境声明无效"));
                    }
                }
                env.get("client")
                    .and_then(Value::as_str)
                    .unwrap_or("required")
            }
        };
        if client == "unsupported" {
            continue;
        }
        let size = entry["fileSize"]
            .as_u64()
            .filter(|s| *s <= MAX_FILE_BYTES)
            .ok_or_else(|| invalid(&path, "缺少有效文件大小或文件超过上限"))?;
        total = total
            .checked_add(size)
            .filter(|s| *s <= MAX_PACK_BYTES)
            .ok_or_else(|| invalid("files", "总文件大小超过上限"))?;
        let hashes = entry
            .get("hashes")
            .filter(|v| v.is_object())
            .ok_or_else(|| invalid(&path, "缺少校验值"))?;
        let sha1 = hash(hashes, "sha1", 40)?;
        let sha512 = hash(hashes, "sha512", 128)?;
        if sha1.is_none() && sha512.is_none() {
            return Err(invalid(&path, "缺少 SHA-1 或 SHA-512 校验值"));
        }
        let downloads = entry["downloads"]
            .as_array()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| invalid(&path, "缺少下载地址"))?
            .iter()
            .map(|url| {
                let url = url
                    .as_str()
                    .ok_or_else(|| invalid(&path, "下载地址应为字符串"))?;
                validate_download_url(url)
                    .map_err(|_| invalid(&path, "仅支持无凭据的 HTTPS 下载地址"))?;
                Ok(url.to_owned())
            })
            .collect::<Result<Vec<_>, PackError>>()?;
        files.push(PackFile::Download {
            path,
            downloads,
            size,
            sha1,
            sha512,
        });
    }
    Ok(PackManifest {
        name,
        minecraft_version,
        loader,
        files,
        overrides: vec!["overrides".into(), "client-overrides".into()],
    })
}

pub fn validate_download_url(url: &str) -> Result<(), PackError> {
    use minecraft_java_rs_core::net::client::reqwest::Url;
    let parsed = Url::parse(url).map_err(|_| invalid("downloads", "下载地址无效"))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || url.contains('\\')
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(invalid("downloads", "下载地址无效"));
    }
    Ok(())
}

fn cf_loader(minecraft: &Value) -> Result<Option<PackLoader>, PackError> {
    let entries = minecraft["modLoaders"]
        .as_array()
        .ok_or_else(|| invalid("modLoaders", "缺少加载器列表"))?;
    if entries.is_empty() {
        return Ok(None);
    }
    let mut choices = Vec::new();
    for entry in entries {
        let id = text(entry, "id")?;
        let (kind, version) = id
            .split_once('-')
            .ok_or_else(|| invalid("modLoaders", "加载器缺少精确版本"))?;
        let kind = match kind {
            "forge" => "forge",
            "neoforge" => "neoforge",
            "fabric" => "fabric",
            "quilt" => "quilt",
            _ => {
                return Err(PackError::new(
                    "unsupported_dependency",
                    format!("整合包声明了不支持的加载器：{id}"),
                    Some(&id),
                ));
            }
        };
        let primary = match entry.get("primary") {
            None => false,
            Some(value) => value
                .as_bool()
                .ok_or_else(|| invalid("primary", "必须为布尔值"))?,
        };
        choices.push((
            PackLoader {
                kind: kind.into(),
                version: exact_version(version, &id)?,
            },
            primary,
        ));
    }
    let primary: Vec<_> = choices.iter().filter(|(_, primary)| *primary).collect();
    // 单一加载器无需猜测；多声明必须明确唯一主加载器。
    match primary.len() {
        1 => Ok(Some(primary[0].0.clone())),
        0 if choices.len() == 1 => Ok(Some(choices.remove(0).0)),
        _ => Err(invalid(
            "modLoaders",
            "多个加载器必须标记唯一 primary 主加载器",
        )),
    }
}

pub fn parse_curseforge_manifest(bytes: &[u8]) -> Result<PackManifest, PackError> {
    let value = parse_json(bytes)?;
    if value["manifestType"].as_str() != Some("minecraftModpack") {
        return Err(invalid("manifestType", "不是 Minecraft 整合包"));
    }
    if value["manifestVersion"].as_u64() != Some(1) {
        return Err(invalid("manifestVersion", "仅支持格式版本 1"));
    }
    let name = text(&value, "name")?;
    let minecraft = &value["minecraft"];
    let minecraft_version = exact_version(&text(minecraft, "version")?, "minecraft.version")?;
    let loader = cf_loader(minecraft)?;
    let entries = value["files"]
        .as_array()
        .ok_or_else(|| invalid("files", "缺少文件列表"))?;
    if entries.len() > MAX_PACK_FILES {
        return Err(invalid("files", "文件数量超过上限"));
    }
    let mut ids = HashSet::new();
    let mut files = Vec::new();
    for entry in entries {
        let project_id = entry["projectID"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or_else(|| invalid("projectID", "缺少有效项目 ID"))?;
        let file_id = entry["fileID"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or_else(|| invalid("fileID", "缺少有效文件 ID"))?;
        if !ids.insert((project_id, file_id)) {
            return Err(invalid("files", "重复的项目/文件 ID"));
        }
        let required = match entry.get("required") {
            None => true,
            Some(value) => value
                .as_bool()
                .ok_or_else(|| invalid("required", "必须为布尔值"))?,
        };
        files.push(PackFile::Curseforge {
            project_id,
            file_id,
            required,
        });
    }
    let overrides = text(&value, "overrides")?;
    validate_pack_path(&overrides)?;
    Ok(PackManifest {
        name,
        minecraft_version,
        loader,
        files,
        overrides: vec![overrides],
    })
}

fn platform(request: &PackRequest) -> Result<crate::community_network::Platform, String> {
    if request.project_id.is_empty()
        || request.version_id.is_empty()
        || request.project_id.len() > 128
        || request.version_id.len() > 128
    {
        return Err("缺少有效的整合包项目或版本标识".into());
    }
    match request.platform.as_str() {
        "modrinth" => Ok(crate::community_network::Platform::Modrinth),
        "curseforge" => Ok(crate::community_network::Platform::Curseforge),
        _ => Err("不支持的整合包平台，请重新选择项目".into()),
    }
}

fn verify_mr_project(project: &Value, request: &PackRequest) -> Result<(), String> {
    if project["project_type"].as_str() != Some("modpack")
        || ![project["id"].as_str(), project["slug"].as_str()]
            .contains(&Some(request.project_id.as_str()))
        || project["id"].as_str().is_none()
    {
        return Err("Modrinth 整合包项目与所选项目不匹配，请重新选择".into());
    }
    Ok(())
}

fn verify_mr_identity(
    project: &Value,
    version: &Value,
    request: &PackRequest,
) -> Result<(), String> {
    verify_mr_project(project, request)?;
    if version["id"].as_str() != Some(request.version_id.as_str())
        || project["id"] != version["project_id"]
    {
        return Err("Modrinth 整合包项目与所选版本不匹配，请重新选择版本".into());
    }
    Ok(())
}

fn verify_cf_identity(
    project: &Value,
    file: &Value,
    project_id: u64,
    file_id: u64,
    pack: bool,
) -> Result<(), String> {
    if project["id"].as_u64() != Some(project_id)
        || file["modId"].as_u64() != Some(project_id)
        || file["id"].as_u64() != Some(file_id)
        || (pack && project["classId"].as_u64() != Some(4471))
        || project["gameId"].as_u64() != Some(432)
    {
        return Err(format!(
            "CurseForge 项目 {project_id} 与文件 {file_id} 不匹配，请重新选择版本"
        ));
    }
    Ok(())
}

fn mr_archive(version: &Value) -> Result<ResolvedFile, String> {
    let files = version["files"]
        .as_array()
        .ok_or("整合包版本缺少文件列表")?;
    let files: Vec<_> = files
        .iter()
        .filter(|f| {
            f["filename"]
                .as_str()
                .is_some_and(|n| n.ends_with(".mrpack"))
        })
        .collect();
    let file = files
        .iter()
        .find(|f| f["primary"].as_bool() == Some(true))
        .or_else(|| files.first())
        .ok_or("所选版本没有 .mrpack 整合包文件")?;
    let filename = text(file, "filename").map_err(|e| e.to_string())?;
    validate_pack_path(&filename).map_err(|e| e.to_string())?;
    if filename.contains('/') {
        return Err("整合包文件名不能包含目录".into());
    }
    let url = text(file, "url").map_err(|e| e.to_string())?;
    validate_download_url(&url).map_err(|e| e.to_string())?;
    let sha1 = hash(&file["hashes"], "sha1", 40).map_err(|e| e.to_string())?;
    let sha512 = hash(&file["hashes"], "sha512", 128).map_err(|e| e.to_string())?;
    if sha1.is_none() && sha512.is_none() {
        return Err("整合包原包缺少 SHA 校验值".into());
    }
    let size = file["size"]
        .as_u64()
        .filter(|size| *size <= MAX_FILE_BYTES)
        .ok_or("整合包文件大小无效")?;
    Ok(ResolvedFile {
        path: filename,
        downloads: vec![url],
        size,
        hashes: minecraft_java_rs_core::net::transfer::FileHashes {
            sha1,
            sha512,
            ..Default::default()
        },
    })
}

fn cf_file(
    file: &Value,
    folder: &str,
    project_id: u64,
    file_id: u64,
    required: bool,
) -> Result<ResolvedFile, String> {
    let missing = || {
        format!(
            "CurseForge {}文件不可用：项目 {project_id}，文件 {file_id}；请选择其他整合包版本",
            if required { "必需" } else { "可选" }
        )
    };
    let filename = file["fileName"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(missing)?;
    validate_pack_path(filename).map_err(|e| e.to_string())?;
    if filename.contains('/') {
        return Err(format!("CurseForge 文件名包含目录：{filename}"));
    }
    let url = file["downloadUrl"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(missing)?;
    if file["isAvailable"].as_bool() == Some(false) {
        return Err(missing());
    }
    validate_download_url(url).map_err(|e| e.to_string())?;
    let mut hashes = minecraft_java_rs_core::net::transfer::FileHashes::default();
    if let Some(values) = file["hashes"].as_array() {
        for entry in values {
            let (key, length) = match entry["algo"].as_u64() {
                Some(1) => ("sha1", 40),
                Some(2) => ("md5", 32),
                _ => continue,
            };
            let value = hash(&serde_json::json!({key:entry["value"]}), key, length)
                .map_err(|e| e.to_string())?;
            if key == "sha1" {
                hashes.sha1 = value;
            } else {
                hashes.md5 = value;
            }
        }
    }
    let size = file["fileLength"]
        .as_u64()
        .filter(|s| *s <= MAX_FILE_BYTES)
        .ok_or_else(missing)?;
    Ok(ResolvedFile {
        path: if folder.is_empty() {
            filename.into()
        } else {
            format!("{folder}/{filename}")
        },
        downloads: vec![url.into()],
        size,
        hashes,
    })
}

async fn cf_metadata(
    project_id: u64,
    file_id: u64,
    key: Option<&crate::secrets::CurseforgeCredential>,
    context: &crate::app_tasks::TaskContext,
    transfer: &minecraft_java_rs_core::net::transfer::TransferContext,
    pack: bool,
) -> Result<(Value, Value), String> {
    use crate::community_network::{CURSEFORGE_API, Platform, get_json};
    let project_url = format!("{CURSEFORGE_API}mods/{project_id}");
    let project = get_json(
        Platform::Curseforge,
        &project_url,
        &context.network,
        transfer,
        key,
        |value| {
            let project = &value["data"];
            if project["id"].as_u64() != Some(project_id)
                || project["gameId"].as_u64() != Some(432)
                || (pack && project["classId"].as_u64() != Some(4471))
            {
                return Err(format!("CurseForge 项目 {project_id} 元数据无效"));
            }
            if pack {
                text(project, "name").map_err(|error| error.to_string())?;
            }
            Ok(project.clone())
        },
    )
    .await?;
    let file_url = format!("{CURSEFORGE_API}mods/{project_id}/files/{file_id}");
    let file = get_json(
        Platform::Curseforge,
        &file_url,
        &context.network,
        transfer,
        key,
        |value| {
            let file = &value["data"];
            verify_cf_identity(&project, file, project_id, file_id, pack)?;
            let resolved = cf_file(file, "", project_id, file_id, true)?;
            if pack {
                if !resolved.path.to_ascii_lowercase().ends_with(".zip") {
                    return Err("所选 CurseForge 文件不是 ZIP 整合包".into());
                }
                text(file, "displayName").map_err(|error| error.to_string())?;
            }
            Ok(file.clone())
        },
    )
    .await?;
    Ok((project, file))
}

struct FileProgress {
    started: std::time::Instant,
    total: u64,
    finished: std::sync::atomic::AtomicU64,
    bytes: std::sync::Mutex<std::collections::HashMap<String, u64>>,
}
impl FileProgress {
    fn new(total: usize) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            started: std::time::Instant::now(),
            total: total as u64,
            finished: std::sync::atomic::AtomicU64::new(0),
            bytes: Default::default(),
        })
    }
    fn report(&self, context: &crate::app_tasks::TaskContext, name: &str, bytes: u64) {
        let mut values = self.bytes.lock().unwrap_or_else(|e| e.into_inner());
        values.insert(name.into(), bytes);
        let transferred: u64 = values.values().sum();
        context.progress(
            self.finished.load(std::sync::atomic::Ordering::Relaxed),
            self.total,
            transferred as f64 / self.started.elapsed().as_secs_f64().max(0.001),
            0,
        );
    }
}
async fn download_file(
    file: &ResolvedFile,
    destination: PathBuf,
    context: &crate::app_tasks::TaskContext,
    transfer: &minecraft_java_rs_core::net::transfer::TransferContext,
    metrics: std::sync::Arc<FileProgress>,
    validator: Option<minecraft_java_rs_core::net::transfer::FileValidator>,
) -> Result<(), String> {
    use minecraft_java_rs_core::net::transfer::{DownloadRequest, download, download_validated};
    context.check_cancelled()?;
    let mut candidates = Vec::new();
    for url in &file.downloads {
        candidates.extend(crate::community_network::file_candidates(
            url,
            &context.network.community_file_mirror,
        )?);
    }
    let request = DownloadRequest {
        candidates,
        destination,
        expected_size: Some(file.size),
        hashes: file.hashes.clone(),
        service: crate::community_network::file_service(file.downloads.iter().map(String::as_str))?,
    };
    let reporter = context.clone();
    let name = file.path.clone();
    let reporting = metrics.clone();
    let progress: minecraft_java_rs_core::net::transfer::ProgressCallback = std::sync::Arc::new(
        move |progress: minecraft_java_rs_core::net::transfer::TransferProgress| {
            reporter.file(
                &name,
                progress
                    .total_bytes
                    .filter(|n| *n > 0)
                    .map(|n| progress.completed_bytes as f64 / n as f64)
                    .unwrap_or(0.0),
                false,
            );
            reporting.report(&reporter, &name, progress.transferred_bytes);
        },
    );
    let client = crate::community_network::file_client()?;
    if let Some(validator) = validator {
        download_validated(
            &client,
            transfer,
            &request,
            MAX_FILE_BYTES,
            validator,
            Some(progress),
        )
        .await?;
    } else {
        download(&client, transfer, &request, Some(progress)).await?;
    }
    context.check_cancelled()?;
    metrics
        .finished
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    context.file(&file.path, 1.0, true);
    Ok(())
}

fn read_pack_manifest(
    archive: &std::path::Path,
    platform: crate::community_network::Platform,
    cancel: &minecraft_java_rs_core::net::transfer::CancellationToken,
) -> Result<PackManifest, String> {
    use crate::community_network::Platform;
    cancel.check()?;
    let entry = match platform {
        Platform::Modrinth => "modrinth.index.json",
        Platform::Curseforge => "manifest.json",
    };
    let bytes = minecraft_java_rs_core::utils::archive::read_zip_entry(
        archive,
        entry,
        MAX_MANIFEST_BYTES as u64,
        cancel,
    )?;
    match platform {
        Platform::Modrinth => parse_modrinth_manifest(&bytes),
        Platform::Curseforge => parse_curseforge_manifest(&bytes),
    }
    .map_err(|error| format!("{} [{}]", error.message, error.code))
}

pub async fn resolve_plan(
    app: &tauri::AppHandle,
    request: &PackRequest,
    context: &crate::app_tasks::TaskContext,
) -> Result<PackPlan, String> {
    use crate::community_network::{MODRINTH_API, Platform, get_json};
    use minecraft_java_rs_core::net::transfer::TransferContext;
    context.check_cancelled()?;
    context.stage("正在加载", "整合包元数据");
    let selected_platform = platform(request)?;
    let api = TransferContext::new(
        context.network.community_api_source,
        context.network.file_download_source,
        context.transfer.cancellation().clone(),
    );
    let key = if selected_platform == Platform::Curseforge {
        crate::secrets::curseforge_api_key(app.clone())?
    } else {
        None
    };
    let (archive_file, source) = match selected_platform {
        Platform::Modrinth => {
            let project = get_json(
                Platform::Modrinth,
                &format!(
                    "{MODRINTH_API}project/{}",
                    crate::http::urlencode(&request.project_id)
                ),
                &context.network,
                &api,
                None,
                |value| {
                    verify_mr_project(value, request)?;
                    text(value, "title").map_err(|error| error.to_string())?;
                    Ok(value.clone())
                },
            )
            .await?;
            let version = get_json(
                Platform::Modrinth,
                &format!(
                    "{MODRINTH_API}version/{}",
                    crate::http::urlencode(&request.version_id)
                ),
                &context.network,
                &api,
                None,
                |value| {
                    verify_mr_identity(&project, value, request)?;
                    mr_archive(value)?;
                    text(value, "version_number").map_err(|error| error.to_string())?;
                    Ok(value.clone())
                },
            )
            .await?;
            (
                mr_archive(&version)?,
                PackSource {
                    platform: PackPlatform::Modrinth,
                    project_id: text(&project, "id").map_err(|e| e.to_string())?,
                    version_id: request.version_id.clone(),
                    name: text(&project, "title").map_err(|e| e.to_string())?,
                    version_name: text(&version, "version_number").map_err(|e| e.to_string())?,
                },
            )
        }
        Platform::Curseforge => {
            let project_id = request
                .project_id
                .parse::<u64>()
                .map_err(|_| "CurseForge 项目 ID 必须是正整数")?;
            let file_id = request
                .version_id
                .parse::<u64>()
                .map_err(|_| "CurseForge 文件 ID 必须是正整数")?;
            let (project, file) =
                cf_metadata(project_id, file_id, key.as_ref(), context, &api, true).await?;
            let archive = cf_file(&file, "", project_id, file_id, true)?;
            if !archive.path.to_ascii_lowercase().ends_with(".zip") {
                return Err("所选 CurseForge 文件不是 ZIP 整合包".into());
            }
            (
                archive,
                PackSource {
                    platform: PackPlatform::Curseforge,
                    project_id: project_id.to_string(),
                    version_id: file_id.to_string(),
                    name: text(&project, "name").map_err(|e| e.to_string())?,
                    version_name: text(&file, "displayName").map_err(|e| e.to_string())?,
                },
            )
        }
    };
    context.stage("下载整合包", &source.name);
    let archive = context.workspace.join("modpack.archive");
    download_file(
        &archive_file,
        archive.clone(),
        context,
        &context.transfer,
        FileProgress::new(1),
        Some(std::sync::Arc::new(move |path, cancel| {
            // 清单解析和所有 ZIP 条目的 CRC 都纳入候选校验，错误来源不能提前胜出。
            read_pack_manifest(path, selected_platform, cancel)?;
            minecraft_java_rs_core::utils::archive::validate_zip_file(path, cancel)
        })),
    )
    .await?;
    let archive_path = archive.clone();
    let cancel = context.transfer.cancellation().clone();
    let manifest = tokio::task::spawn_blocking(move || {
        read_pack_manifest(&archive_path, selected_platform, &cancel)
    })
    .await
    .map_err(|e| format!("读取整合包清单失败：{e}"))??;
    let display_name = request
        .instance_name
        .clone()
        .unwrap_or_else(|| source.name.clone());
    if request.instance_name.is_some() {
        crate::named_profiles::validate_name(&display_name)?;
    }
    resolve_manifest(
        manifest,
        archive,
        display_name,
        serde_json::to_value(source).map_err(|e| e.to_string())?,
        key.as_ref(),
        context,
        &api,
    )
    .await
}

/// 本地导入只识别标准清单；ZIP 不能被当作任意目录覆盖包。
pub(crate) fn local_manifest(
    path: &std::path::Path,
    cancel: &minecraft_java_rs_core::net::transfer::CancellationToken,
) -> Result<(crate::community_network::Platform, PackManifest), String> {
    use crate::community_network::Platform;
    cancel.check()?;
    let parent = path.parent().ok_or("整合包路径无效")?;
    check_destination(parent, path)?;
    let metadata = std::fs::metadata(path).map_err(|e| format!("读取整合包失败：{e}"))?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err("请选择不超过 4 GiB 的整合包文件".into());
    }
    let platform = match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mrpack") => Platform::Modrinth,
        Some("zip") => Platform::Curseforge,
        _ => return Err("请选择 Modrinth .mrpack 或包含 manifest.json 的 CurseForge ZIP".into()),
    };
    Ok((platform, read_pack_manifest(path, platform, cancel)?))
}

pub async fn resolve_local_plan(
    app: &tauri::AppHandle,
    path: &std::path::Path,
    instance_name: &str,
    context: &crate::app_tasks::TaskContext,
) -> Result<PackPlan, String> {
    use minecraft_java_rs_core::net::transfer::TransferContext;
    crate::named_profiles::validate_name(instance_name)?;
    context.check_cancelled()?;
    let instance_settings = crate::portable_pack::instance_settings_for_payload(path)?;
    context.stage(
        "读取本地整合包",
        &path.file_name().unwrap_or_default().to_string_lossy(),
    );
    let path = path.to_path_buf();
    let archive = context.workspace.join("local-pack.archive");
    let staged = archive.clone();
    let cancel = context.transfer.cancellation().clone();
    // 固定本任务副本，之后只解析和提取副本，避免选择后原文件变化。
    let (platform, manifest) = tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let (platform, _) = local_manifest(&path, &cancel)?;
        let mut input = std::fs::File::open(&path).map_err(|e| format!("打开整合包失败：{e}"))?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .map_err(|e| format!("暂存整合包失败：{e}"))?;
        let mut buffer = [0_u8; 64 * 1024];
        let mut total = 0_u64;
        loop {
            cancel.check()?;
            let count = input
                .read(&mut buffer)
                .map_err(|e| format!("读取整合包失败：{e}"))?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > MAX_FILE_BYTES {
                return Err("整合包超过 4 GiB 上限".into());
            }
            output
                .write_all(&buffer[..count])
                .map_err(|e| format!("暂存整合包失败：{e}"))?;
        }
        output
            .sync_all()
            .map_err(|e| format!("暂存整合包失败：{e}"))?;
        drop(output);
        minecraft_java_rs_core::utils::archive::validate_zip_file(&staged, &cancel)?;
        Ok::<_, String>((platform, read_pack_manifest(&staged, platform, &cancel)?))
    })
    .await
    .map_err(|e| format!("读取本地整合包失败：{e}"))??;
    let key = if platform == crate::community_network::Platform::Curseforge {
        crate::secrets::curseforge_api_key(app.clone())?
    } else {
        None
    };
    let api = TransferContext::new(
        context.network.community_api_source,
        context.network.file_download_source,
        context.transfer.cancellation().clone(),
    );
    let source = serde_json::json!({
        "platform": if platform == crate::community_network::Platform::Modrinth { "modrinth" } else { "curseforge" },
        "origin": "local", "name": manifest.name,
    });
    let mut plan = resolve_manifest(
        manifest,
        archive,
        instance_name.into(),
        source,
        key.as_ref(),
        context,
        &api,
    )
    .await?;
    plan.instance_settings = instance_settings;
    Ok(plan)
}

async fn resolve_manifest(
    manifest: PackManifest,
    archive: PathBuf,
    display_name: String,
    source: Value,
    key: Option<&crate::secrets::CurseforgeCredential>,
    context: &crate::app_tasks::TaskContext,
    api: &minecraft_java_rs_core::net::transfer::TransferContext,
) -> Result<PackPlan, String> {
    use minecraft_java_rs_core::net::transfer::FileHashes;
    context.stage("正在加载", "整合包依赖");
    let mut files = Vec::new();
    let mut paths = HashSet::new();
    let mut total = 0_u64;
    for file in manifest.files {
        context.check_cancelled()?;
        let resolved = match file {
            PackFile::Download {
                path,
                downloads,
                size,
                sha1,
                sha512,
            } => ResolvedFile {
                path,
                downloads,
                size,
                hashes: FileHashes {
                    sha1,
                    sha512,
                    ..Default::default()
                },
            },
            PackFile::Curseforge {
                project_id,
                file_id,
                required,
            } => {
                let (project, file) = cf_metadata(project_id, file_id, key, context, api, false)
                    .await
                    .map_err(|error| {
                        format!(
                            "整合包{}文件缺失：项目 {project_id}，文件 {file_id}；{error}",
                            if required { "必需" } else { "可选" }
                        )
                    })?;
                let folder = match project["classId"].as_u64() {
                    Some(6) => "mods",
                    Some(12) => "resourcepacks",
                    Some(6552) => "shaderpacks",
                    Some(6945) => "datapacks",
                    _ => return Err(format!("整合包依赖项目 {project_id} 的资源种类不支持")),
                };
                cf_file(&file, folder, project_id, file_id, required)?
            }
        };
        validate_pack_path(&resolved.path).map_err(|e| e.to_string())?;
        if !paths.insert(resolved.path.to_lowercase()) {
            return Err(format!("整合包依赖路径冲突：{}", resolved.path));
        }
        total = total
            .checked_add(resolved.size)
            .filter(|n| *n <= MAX_PACK_BYTES)
            .ok_or("整合包依赖总大小超过上限")?;
        files.push(resolved);
    }
    context.check_cancelled()?;
    Ok(PackPlan {
        display_name,
        minecraft_version: manifest.minecraft_version,
        loader: manifest.loader,
        source,
        archive,
        overrides: manifest.overrides,
        files,
        instance_settings: None,
    })
}

pub async fn apply_files(
    plan: &PackPlan,
    resource_root: &std::path::Path,
    context: &crate::app_tasks::TaskContext,
) -> Result<(), String> {
    context.check_cancelled()?;
    context.stage("下载整合包依赖", &plan.display_name);
    let transfer = context.transfer.child();
    let batch_cancel = transfer.cancellation().clone();
    let metrics = FileProgress::new(plan.files.len());
    let mut running = tokio::task::JoinSet::new();
    let mut pending = plan.files.iter();
    let concurrency = context.network.max_download_threads.clamp(1, 64);
    let descriptor = resource_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("{name}.json"));
    for file in &plan.files {
        validate_pack_path(&file.path).map_err(|error| error.to_string())?;
        if descriptor.as_ref().is_some_and(|name| {
            file.path
                .split('/')
                .next()
                .is_some_and(|part| part.eq_ignore_ascii_case(name))
        }) {
            return Err(format!("整合包不能覆盖实例描述文件：{}", file.path));
        }
    }
    loop {
        while running.len() < concurrency {
            let Some(file) = pending.next() else {
                break;
            };
            let file = file.clone();
            let context = context.clone();
            let transfer = transfer.clone();
            let metrics = metrics.clone();
            let destination = resource_root.join(&file.path);
            let root = resource_root.to_path_buf();
            running.spawn(async move {
                check_destination(&root, &destination)?;
                download_file(&file, destination, &context, &transfer, metrics, None).await
            });
        }
        let result = tokio::select! {
            _ = context.transfer.cancellation().cancelled() => {
                batch_cancel.cancel();
                while running.join_next().await.is_some() {}
                return context.check_cancelled();
            }
            result = running.join_next() => result,
        };
        let Some(result) = result else {
            break;
        };
        if let Err(error) = result
            .map_err(|e| format!("整合包文件任务失败：{e}"))
            .and_then(|r| r)
        {
            // 只取消同批文件，保留原始错误；不能把失败任务伪装成用户取消。
            batch_cancel.cancel();
            while running.join_next().await.is_some() {}
            return Err(error);
        }
    }
    context.check_cancelled()?;
    context.progress(plan.files.len() as u64, plan.files.len() as u64, 0.0, 0);
    context.stage("应用整合包文件", &plan.display_name);
    apply_overrides(plan, resource_root, context.transfer.cancellation()).await?;
    apply_pack_instance_settings(plan, resource_root)?;
    context.check_cancelled()
}

fn apply_pack_instance_settings(plan: &PackPlan, resource_root: &std::path::Path) -> Result<(), String> {
    if let Some(settings) = &plan.instance_settings {
        crate::pack_export::instance_settings_export::apply_to_instance(resource_root, settings)?;
    }
    Ok(())
}

async fn apply_overrides(
    plan: &PackPlan,
    resource_root: &std::path::Path,
    cancel: &minecraft_java_rs_core::net::transfer::CancellationToken,
) -> Result<(), String> {
    cancel.check()?;
    let archive = plan.archive.clone();
    let root = resource_root.to_path_buf();
    let subtrees = plan.overrides.clone();
    let existing_files: Vec<_> = plan.files.iter().map(|file| file.path.clone()).collect();
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || {
        let mut protected = vec![
            ".nickel-pack.pending".into(),
            "gameData.json".into(),
            "nickel.settings.json".into(),
            "launcher_profiles.json".into(),
            "launcher_accounts.json".into(),
            "natives".into(),
        ];
        if let Some(name) = root.file_name().and_then(|name| name.to_str()) {
            protected.push(format!("{name}.json"));
        }
        minecraft_java_rs_core::utils::archive::extract_zip_subtrees_with_existing(
            &archive,
            &root,
            &subtrees,
            &protected,
            &existing_files,
            &Default::default(),
            &cancel,
        )?;
        cancel.check()
    })
    .await
    .map_err(|e| format!("应用整合包覆盖文件失败：{e}"))?
}

pub(crate) fn check_destination(
    root: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), String> {
    let relative = destination
        .strip_prefix(root)
        .map_err(|_| "整合包路径超出实例目录")?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("整合包路径超出实例目录".into());
    }
    // 也检查实例根目录的祖先，Windows junction 不一定被识别为 symlink。
    for current in destination.ancestors() {
        match std::fs::symlink_metadata(current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(format!("整合包目标包含符号链接：{}", current.display()));
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
                    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                        return Err(format!("整合包目标包含重解析点：{}", current.display()));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("检查整合包目标失败：{error}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn mr() -> Value {
        json!({"formatVersion":1,"game":"minecraft","name":"测试包","dependencies":{"minecraft":"1.20.1","fabric-loader":"0.16.0"},"files":[]})
    }
    fn mr_file() -> Value {
        json!({"path":"mods/example.jar","downloads":["https://cdn.modrinth.com/data/a/example.jar"],"fileSize":42,"hashes":{"sha1":"a".repeat(40),"sha512":"b".repeat(128)}})
    }
    fn cf() -> Value {
        json!({"manifestType":"minecraftModpack","manifestVersion":1,"name":"测试包","minecraft":{"version":"1.20.1","modLoaders":[{"id":"forge-47.3.0","primary":true}]},"files":[{"projectID":1,"fileID":2,"required":false}],"overrides":"overrides"})
    }
    fn parse_mr(value: Value) -> Result<PackManifest, PackError> {
        parse_modrinth_manifest(value.to_string().as_bytes())
    }
    fn parse_cf(value: Value) -> Result<PackManifest, PackError> {
        parse_curseforge_manifest(value.to_string().as_bytes())
    }

    struct OverrideFixture {
        workspace: PathBuf,
        root: PathBuf,
        plan: PackPlan,
    }

    impl OverrideFixture {
        fn new(existing_files: &[&str]) -> Self {
            use base64::Engine;
            let workspace = std::env::temp_dir().join(crate::store::random_token());
            let root = workspace.join("instance");
            std::fs::create_dir_all(&root).unwrap();
            let archive = workspace.join("pack.zip");
            // ZIP 内容：overrides/config.txt、overrides/mods/a.jar、client-overrides/mods/a.jar。
            let zip = base64::engine::general_purpose::STANDARD.decode("UEsDBBQAAAAAAAAAIVx8L4rUBgAAAAYAAAAUAAAAb3ZlcnJpZGVzL2NvbmZpZy50eHRjb25maWdQSwMEFAAAAAAAAAAhXOUGA4IIAAAACAAAABQAAABvdmVycmlkZXMvbW9kcy9hLmphcm92ZXJyaWRlUEsDBBQAAAAAAAAAIVxVBETHBgAAAAYAAAAbAAAAY2xpZW50LW92ZXJyaWRlcy9tb2RzL2EuamFyY2xpZW50UEsBAhQAFAAAAAAAAAAhXHwvitQGAAAABgAAABQAAAAAAAAAAAAAAKSBAAAAAG92ZXJyaWRlcy9jb25maWcudHh0UEsBAhQAFAAAAAAAAAAhXOUGA4IIAAAACAAAABQAAAAAAAAAAAAAAKSBOAAAAG92ZXJyaWRlcy9tb2RzL2EuamFyUEsBAhQAFAAAAAAAAAAhXFUERMcGAAAABgAAABsAAAAAAAAAAAAAAKSBcgAAAGNsaWVudC1vdmVycmlkZXMvbW9kcy9hLmphclBLBQYAAAAAAwADAM0AAACxAAAAAAA=").unwrap();
            std::fs::write(&archive, zip).unwrap();
            let files = existing_files
                .iter()
                .map(|path| {
                    let destination = root.join(path);
                    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
                    std::fs::write(&destination, b"downloaded").unwrap();
                    ResolvedFile {
                        path: (*path).to_owned(),
                        downloads: vec!["https://cdn.modrinth.com/fixture.jar".into()],
                        size: b"downloaded".len() as u64,
                        hashes: Default::default(),
                    }
                })
                .collect();
            let plan = PackPlan {
                display_name: "Override fixture".into(),
                minecraft_version: "1.20.1".into(),
                loader: None,
                source: json!({"platform":"modrinth","projectId":"fixture","versionId":"v1"}),
                archive,
                overrides: vec!["overrides".into(), "client-overrides".into()],
                files,
                instance_settings: None,
            };
            Self {
                workspace,
                root,
                plan,
            }
        }
    }

    impl Drop for OverrideFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.workspace);
        }
    }

    #[tokio::test]
    async fn overrides_reject_cross_stage_conflicts_before_writing() {
        for path in ["mods/A.jar", "Mods/b.jar", "mods", "mods/a.jar/nested.txt"] {
            let fixture = OverrideFixture::new(&[path]);
            let cancel = minecraft_java_rs_core::net::transfer::CancellationToken::new();
            let error = apply_overrides(&fixture.plan, &fixture.root, &cancel)
                .await
                .unwrap_err();
            assert!(error.contains("冲突"), "{path}: {error}");
            assert_eq!(
                std::fs::read(fixture.root.join(path)).unwrap(),
                b"downloaded"
            );
            assert!(!fixture.root.join("config.txt").exists(), "{path}");
        }
    }

    #[test]
    fn portable_instance_settings_merge_after_overrides_and_keep_unknown_keys() {
        let mut fixture = OverrideFixture::new(&[]);
        let path = fixture.root.join("nickel.settings.json");
        std::fs::write(&path, br#"{"unknownFutureKey":"keep","windowTitle":"old"}"#).unwrap();
        fixture.plan.instance_settings = Some(json!({
            "windowTitle": "portable",
            "memoryMode": "custom",
            "memoryMb": 6144,
            "gameArgs": "--demo",
        }));
        apply_pack_instance_settings(&fixture.plan, &fixture.root).unwrap();
        let settings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(settings["unknownFutureKey"], "keep");
        assert_eq!(settings["windowTitle"], "portable");
        assert_eq!(settings["memoryMb"], 6144);
        assert_eq!(settings["gameArgs"], "--demo");
    }

    #[tokio::test]
    async fn overrides_allow_exact_downloaded_path_and_client_priority() {
        let fixture = OverrideFixture::new(&["mods/a.jar", "mods/b.jar"]);
        let cancel = minecraft_java_rs_core::net::transfer::CancellationToken::new();
        apply_overrides(&fixture.plan, &fixture.root, &cancel)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(fixture.root.join("mods/a.jar")).unwrap(),
            b"client"
        );
        assert_eq!(
            std::fs::read(fixture.root.join("mods/b.jar")).unwrap(),
            b"downloaded"
        );
        assert_eq!(
            std::fs::read(fixture.root.join("config.txt")).unwrap(),
            b"config"
        );
        assert_eq!(
            std::fs::read_dir(fixture.root.join("mods"))
                .unwrap()
                .count(),
            2
        );
    }
    #[test]
    fn mr_exact_versions_and_override_order() {
        let plan = parse_mr(mr()).unwrap();
        assert_eq!(plan.minecraft_version, "1.20.1");
        assert_eq!(
            plan.loader.unwrap(),
            PackLoader {
                kind: "fabric".into(),
                version: "0.16.0".into()
            }
        );
        assert_eq!(plan.overrides, ["overrides", "client-overrides"]);
    }
    #[test]
    fn mr_environment_optional_required_and_unsupported() {
        for state in [
            None,
            Some("required"),
            Some("optional"),
            Some("unsupported"),
        ] {
            let mut value = mr();
            let mut file = mr_file();
            if let Some(state) = state {
                file["env"] = json!({"client":state});
            }
            value["files"] = json!([file]);
            assert_eq!(
                parse_mr(value).unwrap().files.len(),
                usize::from(state != Some("unsupported"))
            );
        }
    }
    #[test]
    fn mr_rejects_bad_environment_format_and_hash() {
        for (field, val) in [
            ("formatVersion", json!(2)),
            ("game", json!("other")),
            ("dependencies", json!({"minecraft":"1.20.1","unknown":"1"})),
        ] {
            let mut value = mr();
            value[field] = val;
            assert!(parse_mr(value).is_err());
        }
        for env in [
            json!({"client":"maybe"}),
            json!({"client":false}),
            json!("required"),
        ] {
            let mut value = mr();
            let mut file = mr_file();
            file["env"] = env;
            value["files"] = json!([file]);
            assert!(parse_mr(value).is_err());
        }
        let mut value = mr();
        let mut file = mr_file();
        file["hashes"] = json!({"sha1":"bad"});
        value["files"] = json!([file]);
        assert!(parse_mr(value).is_err());
    }
    #[test]
    fn mr_supports_each_exact_loader_without_replacement() {
        for (dependency, kind) in [
            ("forge", "forge"),
            ("neoforge", "neoforge"),
            ("fabric-loader", "fabric"),
            ("quilt-loader", "quilt"),
        ] {
            let mut value = mr();
            value["dependencies"] = json!({"minecraft":"1.20.1",dependency:"1.2.3-beta"});
            assert_eq!(parse_mr(value).unwrap().loader.unwrap().kind, kind);
        }
        let mut value = mr();
        value["dependencies"]["quilt-loader"] = json!("0.1");
        assert!(parse_mr(value).is_err());
    }
    #[test]
    fn malicious_paths_and_duplicate_names_are_rejected() {
        for path in [
            "../x",
            "/x",
            "C:/x",
            "\\\\server\\x",
            "mods/../x",
            "mods\\x",
            "mods/x.",
            "CON",
            "mods/aux.jar",
            "nickel.profile.json",
            "gameData.json",
            "versions/v.json",
            "libraries/x",
            "mods/x:stream",
        ] {
            assert!(validate_pack_path(path).is_err(), "{path}");
        }
        let mut value = mr();
        let a = mr_file();
        let mut b = a.clone();
        b["path"] = json!("MODS/example.jar");
        value["files"] = json!([a, b]);
        assert!(parse_mr(value).is_err());
    }

    #[test]
    fn pending_metadata_and_nested_control_paths_are_rejected() {
        for path in [
            ".nickel-pack.pending",
            ".NICKEL-PACK.PENDING/owner",
            "nickel.profile.json/child",
            "launcher_accounts.json",
            "natives/native.dll",
        ] {
            assert_eq!(validate_pack_path(path).unwrap_err().code, "protected_path");
        }
        let root = std::env::temp_dir().join(crate::store::random_token());
        assert!(check_destination(&root, &root.join("mods/a.jar")).is_ok());
        assert!(check_destination(&root, &root.join("../escape.jar")).is_err());
        assert!(check_destination(&root, &root).is_err());
        assert!(check_destination(&root, &root.with_extension("outside")).is_err());
    }

    #[test]
    fn cancelled_pack_validation_does_not_read_archive() {
        let cancel = minecraft_java_rs_core::net::transfer::CancellationToken::new();
        cancel.cancel();
        let result = read_pack_manifest(
            std::path::Path::new("absent.archive"),
            crate::community_network::Platform::Modrinth,
            &cancel,
        );
        assert_eq!(result.unwrap_err(), "操作已取消");
    }
    #[test]
    fn cf_exact_ids_optional_and_primary() {
        let plan = parse_cf(cf()).unwrap();
        assert_eq!(plan.loader.unwrap().version, "47.3.0");
        assert_eq!(
            plan.files,
            [PackFile::Curseforge {
                project_id: 1,
                file_id: 2,
                required: false
            }]
        );
        let mut value = cf();
        value["minecraft"]["modLoaders"] =
            json!([{"id":"forge-47.3.0"},{"id":"fabric-0.16.0","primary":true}]);
        assert_eq!(parse_cf(value).unwrap().loader.unwrap().kind, "fabric");
    }
    #[test]
    fn cf_rejects_ambiguous_primary_and_missing_file_identity() {
        for loaders in [
            json!([{"id":"forge-1"},{"id":"fabric-2"}]),
            json!([{"id":"forge-1","primary":true},{"id":"fabric-2","primary":true}]),
            json!([{"id":"unknown-1"}]),
        ] {
            let mut value = cf();
            value["minecraft"]["modLoaders"] = loaders;
            assert!(parse_cf(value).is_err());
        }
        let mut value = cf();
        value["files"] = json!([{"projectID":1}]);
        assert!(parse_cf(value).is_err());
        let mut value = cf();
        value["overrides"] = json!("../outside");
        assert!(parse_cf(value).is_err());
    }

    #[test]
    fn validates_platform_and_exact_project_version_identity() {
        let request = PackRequest {
            platform: "modrinth".into(),
            project_id: "pack-slug".into(),
            version_id: "v1".into(),
            instance_name: None,
        };
        let project = json!({"project_type":"modpack","id":"p1","slug":"pack-slug"});
        assert!(
            verify_mr_identity(&project, &json!({"id":"v1","project_id":"p1"}), &request).is_ok()
        );
        assert!(
            verify_mr_identity(&project, &json!({"id":"v2","project_id":"p1"}), &request).is_err()
        );
        assert!(
            verify_mr_identity(&project, &json!({"id":"v1","project_id":"other"}), &request)
                .is_err()
        );
        assert!(
            platform(&PackRequest {
                platform: "arbitrary".into(),
                ..request
            })
            .is_err()
        );
        let project = json!({"id":11,"gameId":432,"classId":4471});
        assert!(verify_cf_identity(&project, &json!({"id":12,"modId":11}), 11, 12, true).is_ok());
        assert!(verify_cf_identity(&project, &json!({"id":12,"modId":99}), 11, 12, true).is_err());
    }
    #[test]
    fn required_cf_file_missing_reports_concrete_identity() {
        let file = json!({"id":20,"modId":10,"fileName":"missing.jar","downloadUrl":null});
        let error = cf_file(&file, "mods", 10, 20, true).unwrap_err();
        assert!(error.contains("必需"));
        assert!(error.contains("10"));
        assert!(error.contains("20"));
        assert!(cf_file(&file, "mods", 10, 20, false).is_err());
    }
    #[test]
    fn cf_sha1_md5_and_download_paths_are_kept() {
        let file = json!({"fileName":"a.jar","downloadUrl":"https://edge.forgecdn.net/files/a.jar","fileLength":12,
            "hashes":[{"algo":1,"value":"A".repeat(40)},{"algo":2,"value":"b".repeat(32)}]});
        let parsed = cf_file(&file, "mods", 10, 20, true).unwrap();
        assert_eq!(parsed.path, "mods/a.jar");
        assert_eq!(parsed.hashes.sha1, Some("a".repeat(40)));
        assert_eq!(parsed.hashes.md5, Some("b".repeat(32)));
    }
    #[test]
    fn invalid_urls_and_archive_identity_fail_closed() {
        for url in [
            "http://cdn.modrinth.com/a",
            "https://u:p@host/a",
            "https://host/a#fragment",
            "https://host\\outside/a",
        ] {
            assert!(validate_download_url(url).is_err(), "{url}");
        }
        assert!(mr_archive(&json!({"files":[{"filename":"not-a-pack.jar"}]})).is_err());
        assert!(parse_modrinth_manifest(b"[]").is_err());
        assert!(parse_curseforge_manifest(b"not json").is_err());
    }
}
