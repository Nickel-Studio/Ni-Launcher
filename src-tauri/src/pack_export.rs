//! 标准 Modrinth 整合包与便携 ZIP 导出：共用内容快照、校验和任务终态。
#[path = "instance_settings_export.rs"]
pub(crate) mod instance_settings_export;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use minecraft_java_rs_core::net::transfer::TransferContext;
use minecraft_java_rs_core::utils::archive::{ArchiveLimits, write_zip_files};
use minecraft_java_rs_core::utils::hash::{HashAlgorithm, get_file_hash_cancellable};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::{app_tasks, modpacks, named_profiles};

const CONFIG_MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 512;
const CONTENT_DIRECTORIES: &[&str] = &[
    "mods",
    "config",
    "defaultconfigs",
    "kubejs",
    "scripts",
    "resourcepacks",
    "shaderpacks",
    "screenshots",
    "saves",
    "Xaero",
    "xaero",
    "XaeroWaypoints",
    "XaeroWorldMap",
    "journeymap",
    "voxelmap",
];
const CONTENT_FILES: &[&str] = &[
    "options.txt",
    "optionsof.txt",
    "optionsshaders.txt",
    "servers.dat",
    "servers.dat_old",
    "hotbar.nbt",
    "command_history.txt",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFile {
    pub path: String,
    pub size_bytes: u64,
    pub group: String,
    pub selected: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreview {
    pub name: String,
    pub minecraft_version: String,
    pub loader: Option<modpacks::PackLoader>,
    pub files: Vec<ExportFile>,
    pub launcher: crate::portable_pack::LauncherAvailability,
    /// 便携 ZIP 将迁移的实例设置字段（中文标签）。
    pub instance_settings: Vec<String>,
    /// 因本机路径或凭据而跳过迁移的字段。
    pub instance_settings_omitted: Vec<String>,
    /// 实例设置文件损坏时的可操作错误；标准 mrpack 不消费此字段。
    pub instance_settings_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    #[default]
    Mrpack,
    Portable,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Mrpack => "mrpack",
            Self::Portable => "zip",
        }
    }

    fn dialog_title(self) -> &'static str {
        match self {
            Self::Mrpack => "导出 Modrinth 整合包",
            Self::Portable => "导出 Nickel 便携整合包",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportConfig {
    pub name: String,
    pub version: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub bundle_resources: bool,
    #[serde(default)]
    pub modrinth_only: bool,
    #[serde(default)]
    pub format: ExportFormat,
    #[serde(default)]
    pub include_launcher: bool,
    #[serde(default)]
    pub include_personalization: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackExportRequest {
    pub dir: String,
    pub version_id: String,
    pub name: String,
    pub version: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub bundle_resources: bool,
    #[serde(default)]
    pub modrinth_only: bool,
    #[serde(default)]
    pub format: ExportFormat,
    #[serde(default)]
    pub include_launcher: bool,
    #[serde(default)]
    pub include_personalization: bool,
}

struct ExportInstance {
    resource_root: PathBuf,
    preview: ExportPreview,
}

fn config(request: &PackExportRequest) -> ExportConfig {
    ExportConfig {
        name: request.name.clone(),
        version: request.version.clone(),
        files: request.files.clone(),
        bundle_resources: request.bundle_resources,
        modrinth_only: request.modrinth_only,
        format: request.format,
        include_launcher: request.include_launcher,
        include_personalization: request.include_personalization,
    }
}

fn sensitive_path(path: &str) -> bool {
    path.split('/').any(|part| {
        let name = part.to_ascii_lowercase();
        name.starts_with('.')
            || name.starts_with("nowhere.")
            || name.contains("secret")
            || name.contains("token")
            || name.contains("password")
            || name.contains("credential")
            || matches!(
                name.as_str(),
                "accounts.json"
                    | "launcher_accounts.json"
                    | "launcher_profiles.json"
                    | "usercache.json"
                    | "usernamecache.json"
            )
            || name.ends_with(".pem")
            || name.ends_with(".key")
            || name.ends_with(".p12")
    })
}

fn group_for(path: &str) -> Option<(&'static str, bool)> {
    if sensitive_path(path) {
        return None;
    }
    let lower = path.to_ascii_lowercase();
    // 光影包旁的设置文件独立勾选；解压后的 shaders/ 源码和内置属性仍属于光影包本体。
    let shader_options = matches!(lower.as_str(), "optionsshaders.txt" | "config/iris.properties" | "config/oculus.properties")
        || lower.strip_prefix("shaderpacks/").is_some_and(|file| {
            !file.contains('/') && matches!(file.rsplit('.').next(), Some("txt" | "properties" | "cfg" | "ini" | "json" | "toml" | "yaml" | "yml"))
        });
    if shader_options {
        return Some(("shaderOptions", true));
    }
    let root = lower.split('/').next()?;
    Some(match root {
        "mods"
            if lower.ends_with(".disabled")
                || lower.ends_with(".disable")
                || lower.ends_with(".old") =>
        {
            ("disabledMods", false)
        }
        "mods" => ("mods", true),
        "config" | "defaultconfigs" | "kubejs" | "scripts" => ("config", true),
        "resourcepacks" => ("resourcepacks", true),
        "shaderpacks" => ("shaderpacks", true),
        "screenshots" => ("screenshots", false),
        "saves" => ("saves", false),
        "xaero" | "xaerowaypoints" | "xaeroworldmap" | "journeymap" | "voxelmap" => {
            ("maps", false)
        }
        "options.txt" | "optionsof.txt" if !lower.contains('/') => ("gameOptions", true),
        "servers.dat" | "servers.dat_old" if !lower.contains('/') => ("servers", false),
        "hotbar.nbt" | "command_history.txt" if !lower.contains('/') => ("personal", false),
        _ => return None,
    })
}

pub(crate) fn content_directory(
    resource_root: &Path,
    name: &str,
    loader: &str,
    minecraft_version: &str,
) -> PathBuf {
    if name == "mods" {
        crate::resources::instance_paths::mods_directory(
            resource_root,
            Some(loader),
            minecraft_version,
        )
    } else {
        resource_root.join(name)
    }
}

fn collect_files(
    root: &Path,
    path: &Path,
    files: &mut Vec<ExportFile>,
    total: &mut u64,
) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("读取导出文件失败：{error}")),
    };
    modpacks::check_destination(root, path)?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "导出文件超出实例目录")?
        .to_str()
        .ok_or("导出文件名不是有效 Unicode")?
        .replace('\\', "/");
    if sensitive_path(&relative) {
        return Ok(());
    }
    modpacks::validate_pack_path(&relative).map_err(|e| e.to_string())?;
    if metadata.is_dir() {
        for child in std::fs::read_dir(path).map_err(|e| format!("读取导出目录失败：{e}"))?
        {
            collect_files(
                root,
                &child.map_err(|e| e.to_string())?.path(),
                files,
                total,
            )?;
        }
    } else if metadata.is_file() {
        let Some((group, selected)) = group_for(&relative) else {
            return Ok(());
        };
        let limits = ArchiveLimits::default();
        *total = total
            .checked_add(metadata.len())
            .ok_or("导出文件总大小溢出")?;
        if files.len() >= limits.max_entries
            || metadata.len() > limits.max_entry_bytes
            || *total > limits.max_total_bytes
        {
            return Err("导出内容超过归档配额，请减少所选内容".into());
        }
        files.push(ExportFile {
            path: relative,
            size_bytes: metadata.len(),
            group: group.into(),
            selected,
        });
    } else {
        return Err(format!("导出内容不能包含特殊文件：{relative}"));
    }
    Ok(())
}

fn inspect(app: &AppHandle, dir: &str, version_id: &str) -> Result<ExportInstance, String> {
    named_profiles::validate_name(version_id)?;
    let root = std::fs::canonicalize(dir).map_err(|e| format!("读取实例根目录失败：{e}"))?;
    let resource_root = crate::runtime_settings::resource_directory(app, &root, version_id)?;
    inspect_resolved(&root, version_id, resource_root)
}

fn inspect_resolved(
    root: &Path,
    version_id: &str,
    resource_root: PathBuf,
) -> Result<ExportInstance, String> {
    let directory = root.join("versions").join(version_id);
    modpacks::check_destination(root, &directory)?;
    if resource_root != root {
        modpacks::check_destination(root, &resource_root)?;
    }
    if named_profiles::is_pending_pack(&directory) {
        return Err("请等待整合包安装完成后再导出".into());
    }
    let entries: Value = serde_json::from_str(&crate::instances::instance_scan(
        root.to_string_lossy().into_owned(),
    ))
    .map_err(|_| "读取实例列表失败")?;
    let entry = entries
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"] == version_id))
        .filter(|entry| entry["hasJson"].as_bool() == Some(true))
        .ok_or("实例不存在或尚未安装完成")?;
    let profile = named_profiles::load(root, version_id)?;
    if profile.as_ref().is_some_and(|profile| !profile.ready) {
        return Err("请先补全实例，再导出整合包".into());
    }
    let minecraft_version = profile
        .as_ref()
        .map(|profile| profile.minecraft_version.clone())
        .or_else(|| entry["minecraftVersion"].as_str().map(str::to_owned))
        .ok_or("无法识别此实例的 Minecraft 版本，请先补全实例")?;
    let kind = profile
        .as_ref()
        .and_then(|profile| profile.loader.as_ref().map(ToString::to_string))
        .unwrap_or_else(|| entry["loader"].as_str().unwrap_or("vanilla").into());
    let mods_loader = kind.clone();
    let loader = match kind.as_str() {
        "vanilla" => None,
        "forge" | "neoforge" | "fabric" | "quilt" => Some(modpacks::PackLoader {
            kind,
            version: profile
                .as_ref()
                .map(|profile| profile.loader_version.clone())
                .or_else(|| entry["loaderVersion"].as_str().map(str::to_owned))
                .filter(|value| !value.is_empty())
                .ok_or("无法识别加载器版本，请先补全实例")?,
        }),
        _ => return Err("此实例的加载器不能导出为标准 Modrinth 整合包".into()),
    };
    let settings = instance_settings_export::project_instance(root, version_id);
    let (instance_settings, instance_settings_omitted, instance_settings_error) = match settings {
        Ok(projection) => (projection.included, projection.omitted, None),
        Err(error) => (Vec::new(), Vec::new(), Some(error)),
    };
    let mut files = Vec::new();
    let mut total = 0_u64;
    // 大小写不敏感文件系统上 Xaero/xaero 只遍历一次。
    let mut visited = HashSet::new();
    for name in CONTENT_DIRECTORIES.iter().chain(CONTENT_FILES) {
        let path = content_directory(&resource_root, name, &mods_loader, &minecraft_version);
        if let Ok(canonical) = std::fs::canonicalize(&path) {
            if !visited.insert(canonical) {
                continue;
            }
        }
        collect_files(&resource_root, &path, &mut files, &mut total)?;
    }
    files.sort_by(|a, b| a.group.cmp(&b.group).then_with(|| a.path.cmp(&b.path)));
    Ok(ExportInstance {
        resource_root,
        preview: ExportPreview {
            name: entry["displayName"].as_str().unwrap_or(version_id).into(),
            minecraft_version,
            loader,
            files,
            launcher: crate::portable_pack::launcher_availability(),
            instance_settings,
            instance_settings_omitted,
            instance_settings_error,
        },
    })
}

fn validate_config(config: &ExportConfig) -> Result<(), String> {
    for (label, value) in [
        ("整合包名称", &config.name),
        ("整合包版本", &config.version),
    ] {
        if value.trim().is_empty()
            || value.len() > MAX_TEXT_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(format!(
                "{label}不能为空、包含控制字符或超过 {MAX_TEXT_BYTES} 字节"
            ));
        }
    }
    if config.format == ExportFormat::Mrpack && (config.include_launcher || config.include_personalization) {
        return Err("附带启动器或个性化设置时，请选择「便携 ZIP」".into());
    }
    if config.files.len() > ArchiveLimits::default().max_entries {
        return Err("导出文件数量超过上限".into());
    }
    let mut seen = HashSet::new();
    for file in &config.files {
        modpacks::validate_pack_path(file).map_err(|e| e.to_string())?;
        if group_for(file).is_none() || !seen.insert(file.to_lowercase()) {
            return Err(format!("导出文件不允许或重复：{file}"));
        }
    }
    Ok(())
}

fn manifest(
    preview: &ExportPreview,
    config: &ExportConfig,
    downloads: &[Value],
) -> Result<Vec<u8>, String> {
    let mut dependencies = json!({ "minecraft": preview.minecraft_version });
    if let Some(loader) = &preview.loader {
        let key = match loader.kind.as_str() {
            "fabric" => "fabric-loader",
            "quilt" => "quilt-loader",
            "forge" => "forge",
            "neoforge" => "neoforge",
            _ => return Err("整合包加载器不支持".into()),
        };
        dependencies[key] = json!(loader.version);
    }
    let bytes = serde_json::to_vec_pretty(&json!({
        "formatVersion": 1, "game": "minecraft", "versionId": config.version,
        "name": config.name, "files": downloads, "dependencies": dependencies,
    }))
    .map_err(|e| e.to_string())?;
    modpacks::parse_modrinth_manifest(&bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn reject_credentials(path: &Path, relative: &str) -> Result<(), String> {
    let extension = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "json" | "toml" | "yaml" | "yml" | "properties" | "cfg" | "ini" | "txt"
    ) {
        return Ok(());
    }
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > CONFIG_MAX_BYTES {
        return Err(format!("配置文件过大，请取消选择后重试：{relative}"));
    }
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("读取配置失败 {relative}：{e}"))?;
    let folded: String = text
        .to_ascii_lowercase()
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .collect();
    if [
        "accesstoken",
        "clienttoken",
        "refreshtoken",
        "clientsecret",
        "apikey",
        "password",
        "authorization",
        "privatekey",
    ]
    .iter()
    .any(|key| folded.contains(key))
    {
        return Err(format!("配置文件含凭据字段，请取消选择后重试：{relative}"));
    }
    Ok(())
}


fn has_host_path(value: &str, source_roots: &[&Path]) -> bool {
    let normalized = value.replace('\\', "/").to_ascii_lowercase();
    let is_local_path = |value: &str| {
        let bytes = value.as_bytes();
        value.starts_with('/')
            || value.starts_with("file:")
            || (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/')
            || source_roots.iter().any(|root| {
                let root = root.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
                let root = root.trim_end_matches('/');
                root.len() > 1 && (value == root || value.strip_prefix(root).is_some_and(|suffix| suffix.starts_with('/')))
            })
    };
    normalized.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`')).any(|token| {
        let token = token.trim_matches(|c| matches!(c, '[' | ']' | '{' | '}' | '(' | ')' | ',' | ';'));
        if is_local_path(token) {
            return true;
        }
        // 先保留完整 URL 和命名空间资源标识，避免将 URL 查询参数中的斜杠误判为本机路径。
        if minecraft_java_rs_core::net::client::reqwest::Url::parse(token).is_ok() {
            return false;
        }
        let value = token.split_once('=').map_or(token, |(_, value)| value);
        is_local_path(value)
    })
}

fn portable_json_has_host_data(value: &Value, source_roots: &[&Path]) -> bool {
    match value {
        Value::String(value) => has_host_path(value, source_roots),
        Value::Array(values) => values.iter().any(|value| portable_json_has_host_data(value, source_roots)),
        Value::Object(values) => values.iter().any(|(key, value)| {
            if has_host_path(key, source_roots) {
                return true;
            }
            let key: String = key.chars().filter(|c| !matches!(c, '_' | '-' | ' ')).collect();
            key.eq_ignore_ascii_case("preLaunchCommand") || portable_json_has_host_data(value, source_roots)
        }),
        _ => false,
    }
}

fn reject_portable_host_data(path: &Path, relative: &str, source_roots: &[&Path]) -> Result<(), String> {
    let extension = path.extension().and_then(|v| v.to_str()).unwrap_or("").to_ascii_lowercase();
    if !matches!(extension.as_str(), "json" | "toml" | "yaml" | "yml" | "properties" | "cfg" | "ini" | "txt" | "js" | "ts") {
        return Ok(());
    }
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > CONFIG_MAX_BYTES { return Err(format!("配置文件过大，请取消选择后重试：{relative}")); }
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取便携配置失败 {relative}：{e}"))?;
    let folded: String = text.to_ascii_lowercase().chars().filter(|c| !matches!(c, '_' | '-' | ' ')).collect();
    let has_host_data = match serde_json::from_str::<Value>(&text) {
        Ok(value) => portable_json_has_host_data(&value, source_roots),
        Err(_) => has_host_path(&text, source_roots),
    };
    if has_host_data || folded.contains("prelaunchcommand") {
        return Err(format!("请将配置中的本机路径改为相对路径，并移除启动前命令，或取消选择后重试：{relative}"));
    }
    Ok(())
}

struct ExportSnapshot {
    root: PathBuf,
    preview: ExportPreview,
    hashes: Vec<(String, String, u64)>,
}

fn snapshot_files(
    app: &AppHandle,
    request: &PackExportRequest,
    task: &app_tasks::TaskContext,
) -> Result<ExportSnapshot, String> {
    use std::io::{Read, Write};
    task.check_cancelled()?;
    let instance = inspect(app, &request.dir, &request.version_id)?;
    let available: HashSet<_> = instance
        .preview
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    let root = task.workspace.join("export-content");
    std::fs::create_dir(&root).map_err(|e| format!("创建导出暂存目录失败：{e}"))?;
    let mut hashes = Vec::new();
    let mut total = 0_u64;
    let limits = ArchiveLimits::default();
    task.stage("准备导出文件", &request.name);
    for (index, relative) in request.files.iter().enumerate() {
        task.check_cancelled()?;
        if !available.contains(relative.as_str()) {
            return Err(format!("文件已变化，请刷新导出列表：{relative}"));
        }
        let source = instance.resource_root.join(relative);
        modpacks::check_destination(&instance.resource_root, &source)?;
        let destination = root.join(relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut input =
            std::fs::File::open(&source).map_err(|e| format!("读取导出文件失败：{e}"))?;
        let metadata = input.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() > limits.max_entry_bytes {
            return Err(format!("导出文件无效或过大：{relative}"));
        }
        let mut output = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&destination)
            .map_err(|e| e.to_string())?;
        let mut buffer = [0_u8; 64 * 1024];
        let mut copied = 0_u64;
        loop {
            task.check_cancelled()?;
            let count = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            copied += count as u64;
            total += count as u64;
            if copied > metadata.len() || total > limits.max_total_bytes {
                return Err("导出文件已变化或超过归档配额，请刷新后重试".into());
            }
            output
                .write_all(&buffer[..count])
                .map_err(|e| e.to_string())?;
        }
        if copied != metadata.len() {
            return Err(format!("导出文件已变化，请刷新后重试：{relative}"));
        }
        drop(output);
        reject_credentials(&destination, relative)?;
        if request.format == ExportFormat::Portable {
            reject_portable_host_data(&destination, relative, &[Path::new(&request.dir), &instance.resource_root])?;
        }
        // 仅资源文件需要内容身份，以便生成标准下载条目；配置和存档保持 overrides。
        if !request.bundle_resources && is_download_resource(relative) {
            let digest = get_file_hash_cancellable(
                &destination,
                HashAlgorithm::Sha512,
                task.transfer.cancellation(),
            )?;
            hashes.push((relative.clone(), digest, copied));
        }
        task.file(relative, 1.0, true);
        task.progress((index + 1) as u64, request.files.len() as u64, 0.0, 0);
    }
    Ok(ExportSnapshot {
        root,
        preview: instance.preview,
        hashes,
    })
}

fn is_download_resource(path: &str) -> bool {
    matches!(
        group_for(path),
        Some(("mods" | "disabledMods" | "resourcepacks" | "shaderpacks", _))
    ) && matches!(
        path.rsplit('.').next(),
        Some("jar" | "zip" | "disabled" | "disable")
    )
}

fn remote_entry(version: &Value, relative: &str, digest: &str, size: u64) -> Result<Value, String> {
    let file = version["files"]
        .as_array()
        .and_then(|files| {
            files
                .iter()
                .find(|file| file["hashes"]["sha512"].as_str() == Some(digest))
        })
        .ok_or("Modrinth 返回的文件与本地校验值不一致")?;
    if file["size"].as_u64() != Some(size) {
        return Err("Modrinth 返回的文件大小不一致".into());
    }
    let url = file["url"].as_str().ok_or("Modrinth 文件缺少下载地址")?;
    modpacks::validate_download_url(url).map_err(|e| e.to_string())?;
    let host = minecraft_java_rs_core::net::client::reqwest::Url::parse(url)
        .map_err(|_| "Modrinth 下载地址无效")?
        .host_str()
        .unwrap_or("")
        .to_owned();
    if host != "cdn.modrinth.com" {
        return Err("Modrinth 文件地址不属于官方资源域名".into());
    }
    let sha1 = file["hashes"]["sha1"]
        .as_str()
        .filter(|hash| hash.len() == 40 && hash.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or("Modrinth 文件缺少有效 SHA-1 校验值")?;
    Ok(
        json!({ "path": relative, "hashes": { "sha1": sha1, "sha512": digest },
        "downloads": [url], "fileSize": size, "env": { "client": "required", "server": "unsupported" } }),
    )
}

async fn export_to_staging(
    app: AppHandle,
    request: PackExportRequest,
    staged: PathBuf,
    task: &app_tasks::TaskContext,
) -> Result<(), String> {
    let reporter = task.clone();
    let copied_request = request.clone();
    let (snapshot, portable) = tokio::task::spawn_blocking(move || {
        let snapshot = snapshot_files(&app, &copied_request, &reporter)?;
        let portable = if copied_request.format == ExportFormat::Portable {
            reporter.stage("准备便携整合包", &copied_request.name);
            Some(crate::portable_pack::prepare_snapshot(
                &app, &copied_request, &reporter.workspace, reporter.transfer.cancellation(),
            )?)
        } else {
            None
        };
        Ok::<_, String>((snapshot, portable))
    })
    .await
    .map_err(|e| format!("准备导出文件失败：{e}"))??;
    let api = TransferContext::new(
        task.network.community_api_source,
        task.network.file_download_source,
        task.transfer.cancellation().clone(),
    );
    let mut downloads = Vec::new();
    let mut remote_paths = HashSet::new();
    for (relative, digest, size) in &snapshot.hashes {
        task.check_cancelled()?;
        task.stage("匹配 Modrinth 资源", relative);
        let url = format!(
            "{}version_file/{digest}?algorithm=sha512",
            crate::community_network::MODRINTH_API
        );
        match crate::community_network::get_json(
            crate::community_network::Platform::Modrinth,
            &url,
            &task.network,
            &api,
            None,
            |version| remote_entry(version, relative, digest, *size),
        )
        .await
        {
            Ok(entry) => {
                remote_paths.insert(relative.clone());
                downloads.push(entry);
            }
            Err(error) if error.starts_with("HTTP 404 ") => {
                task.check_cancelled()?;
                if request.modrinth_only {
                    return Err(format!(
                        "Modrinth 未收录 {relative}；请取消选择此文件或开启「打包资源文件」"
                    ));
                }
            }
            Err(error) => {
                return Err(format!(
                    "匹配 {relative} 失败，请重试或开启「打包资源文件」：{error}"
                ));
            }
        }
    }
    let index = manifest(&snapshot.preview, &config(&request), &downloads)?;
    let entries: Vec<_> = request
        .files
        .iter()
        .filter(|path| !remote_paths.contains(*path))
        .map(|path| (format!("overrides/{path}"), path.clone()))
        .collect();
    let reporter = task.clone();
    task.stage("写入整合包", &request.name);
    tokio::task::spawn_blocking(move || {
        let payload = portable.as_ref().map(|pack| pack.payload_path()).unwrap_or_else(|| staged.clone());
        write_zip_files(
            &payload,
            &snapshot.root,
            &entries,
            &[("modrinth.index.json".into(), index)],
            &ArchiveLimits::default(),
            reporter.transfer.cancellation(),
            |done, total, name| {
                reporter.file(name, 1.0, true);
                reporter.progress(done as u64, total as u64, 0.0, 0);
            },
        )?;
        if let Some(portable) = portable {
            reporter.stage("写入便携 ZIP", &request.name);
            portable.write(&staged, reporter.transfer.cancellation(), |done, total, name| {
                reporter.file(name, 1.0, true);
                reporter.progress(done as u64, total as u64, 0.0, 0);
            })?;
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("写入整合包失败：{e}"))?
}

pub(crate) async fn choose_file(
    app: &AppHandle,
    save: bool,
    title: &str,
    name: &str,
    extensions: &[&str],
) -> Result<Option<PathBuf>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let dialog = app
        .dialog()
        .file()
        .set_title(title)
        .add_filter(title, extensions)
        .set_file_name(name);
    if save {
        dialog.save_file(move |path| {
            let _ = tx.send(path);
        });
    } else {
        dialog.pick_file(move |path| {
            let _ = tx.send(path);
        });
    }
    rx.await
        .map_err(|_| "文件对话框已关闭，请重试")?
        .map(|path| path.into_path().map_err(|e| format!("请选择本地路径：{e}")))
        .transpose()
}

#[tauri::command]
pub async fn pack_export_inspect(
    app: AppHandle,
    dir: String,
    version_id: String,
) -> Result<ExportPreview, String> {
    tokio::task::spawn_blocking(move || inspect(&app, &dir, &version_id).map(|instance| instance.preview))
        .await
        .map_err(|e| format!("读取导出内容失败：{e}"))?
}

#[tauri::command]
pub async fn pack_export_save(
    app: AppHandle,
    request: PackExportRequest,
) -> Result<Option<String>, String> {
    validate_config(&config(&request))?;
    named_profiles::validate_name(&request.version_id)?;
    if request.format == ExportFormat::Portable {
        crate::portable_pack::check_launcher_request(request.include_launcher)?;
    }
    let extension = request.format.extension();
    let Some(target) = choose_file(
        &app,
        true,
        request.format.dialog_title(),
        &format!("{}.{extension}", request.version_id),
        &[extension],
    )
    .await?
    else {
        return Ok(None);
    };
    let parent = target.parent().ok_or("导出目标缺少父目录")?;
    let parent = std::fs::canonicalize(parent).map_err(|e| format!("读取导出目录失败：{e}"))?;
    let target = parent.join(target.file_name().ok_or("导出文件名无效")?);
    modpacks::check_destination(&parent, &target)?;
    if target
        .extension()
        .and_then(|v| v.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
        != Some(extension)
    {
        return Err(format!("导出文件扩展名必须为 .{extension}"));
    }
    let task = app_tasks::begin(&app, &target, None)?;
    task.describe_kind(app_tasks::TaskKind::Export, &request.name);
    let id = task.id.clone();
    tauri::async_runtime::spawn(async move {
        let temporary = parent.join(format!(".nowhere-export-{}.part", task.id));
        let staged = temporary.clone();
        let result = export_to_staging(app, request, staged, &task).await;
        task.finish_transaction(
            temporary,
            move |temporary| {
                result?;
                modpacks::check_destination(target.parent().ok_or("导出目录无效")?, &target)?;
                std::fs::rename(temporary, &target)
                    .map_err(|e| format!("保存整合包失败，请关闭占用文件的程序后重试：{e}"))?;
                Ok(None)
            },
            |temporary| match std::fs::remove_file(temporary) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("清理导出临时文件失败：{e}")),
            },
        );
    });
    Ok(Some(id))
}

#[tauri::command]
pub async fn pack_export_config_save(
    app: AppHandle,
    config: ExportConfig,
) -> Result<Option<String>, String> {
    validate_config(&config)?;
    let Some(path) =
        choose_file(&app, true, "保存导出配置", "modpack-export.json", &["json"]).await?
    else {
        return Ok(None);
    };
    let bytes = serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?;
    let result = path.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || named_profiles::atomic_write(&path, &bytes))
        .await
        .map_err(|e| format!("保存导出配置失败：{e}"))??;
    Ok(Some(result))
}

#[tauri::command]
pub async fn pack_export_config_load(app: AppHandle) -> Result<Option<ExportConfig>, String> {
    let Some(path) = choose_file(
        &app,
        false,
        "读取导出配置",
        "modpack-export.json",
        &["json"],
    )
    .await?
    else {
        return Ok(None);
    };
    tokio::task::spawn_blocking(move || {
        let metadata = std::fs::metadata(&path).map_err(|e| format!("读取导出配置失败：{e}"))?;
        if !metadata.is_file() || metadata.len() > CONFIG_MAX_BYTES {
            return Err("导出配置超过大小限制".into());
        }
        let bytes = std::fs::read(path).map_err(|e| format!("读取导出配置失败：{e}"))?;
        let config: ExportConfig =
            serde_json::from_slice(&bytes).map_err(|e| format!("导出配置格式无效：{e}"))?;
        validate_config(&config)?;
        Ok(Some(config))
    })
    .await
    .map_err(|e| format!("读取导出配置失败：{e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("nowhere-r6-export-{}", crate::store::random_token()));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn export_selection_roundtrips_through_local_import_and_safe_overrides() {
        use minecraft_java_rs_core::net::transfer::CancellationToken;
        use minecraft_java_rs_core::utils::archive::extract_zip_subtrees;
        let fixture = Fixture::new();
        let version_id = "directory-identity";
        let directory = fixture.0.join("versions").join(version_id);
        std::fs::create_dir_all(directory.join("mods")).unwrap();
        std::fs::create_dir_all(directory.join("config")).unwrap();
        std::fs::write(directory.join("mods/local.jar"), b"local mod").unwrap();
        std::fs::write(directory.join("config/client.json"), br#"{"quality":"high"}"#).unwrap();
        std::fs::write(directory.join("config/accounts.json"), b"must not export").unwrap();
        std::fs::write(directory.join("servers.dat"), b"private servers").unwrap();
        std::fs::write(directory.join("nowhere.profile.json"), serde_json::to_vec(&json!({
            "schemaVersion":1,"name":version_id,"minecraftVersion":"1.20.1","loader":"fabric","loaderVersion":"0.16.9","ready":true
        })).unwrap()).unwrap();
        std::fs::write(directory.join(format!("{version_id}.json")), serde_json::to_vec(&json!({
            "id":version_id,"type":"release","inheritsFrom":"1.20.1",
            "nowhereProfile":{"minecraftVersion":"1.20.1","loader":"fabric","loaderVersion":"0.16.9"},
            "nowhereCustom":{"name":"显示名称不等于目录"}
        })).unwrap()).unwrap();
        let root = std::fs::canonicalize(&fixture.0).unwrap();
        let resources = crate::runtime_settings::game_directory(&root, version_id, true);
        let instance = inspect_resolved(&root, version_id, resources).unwrap();
        assert_eq!(instance.preview.name, "显示名称不等于目录");
        assert!(instance.preview.files.iter().all(|file| !file.path.contains("accounts")));
        assert!(!instance.preview.files.iter().find(|file| file.path == "servers.dat").unwrap().selected);
        let config = ExportConfig { name:"可移植整合包".into(),version:"1.0".into(),files:instance.preview.files.iter().filter(|file|file.selected).map(|file|file.path.clone()).collect(),bundle_resources:true,modrinth_only:false, ..ExportConfig::default() };
        let archive = fixture.0.join("roundtrip.mrpack");
        let entries = config.files.iter().map(|path|(format!("overrides/{path}"),path.clone())).collect::<Vec<_>>();
        let cancel = CancellationToken::new();
        write_zip_files(&archive, &instance.resource_root, &entries, &[("modrinth.index.json".into(),manifest(&instance.preview,&config,&[]).unwrap())],&ArchiveLimits::default(),&cancel,|_,_,_|{}).unwrap();
        let (_, imported) = modpacks::local_manifest(&archive,&cancel).unwrap();
        assert_eq!(imported.name,config.name);
        assert_eq!(imported.loader,instance.preview.loader);
        let destination=fixture.0.join("imported");
        extract_zip_subtrees(&archive,&destination,&imported.overrides,&[],&ArchiveLimits::default(),&cancel).unwrap();
        assert_eq!(std::fs::read(destination.join("mods/local.jar")).unwrap(),b"local mod");
        assert!(!destination.join("servers.dat").exists());
        assert!(!destination.join("config/accounts.json").exists());
    }

    #[test]
    fn export_uses_resolved_isolation_for_named_and_vanilla_instances() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.0.join("mods")).unwrap();
        std::fs::write(fixture.0.join("mods/shared.jar"), b"shared").unwrap();
        let root = std::fs::canonicalize(&fixture.0).unwrap();
        for named in [false, true] {
            let version_id = if named { "named-instance" } else { "1.20.1" };
            let directory = root.join("versions").join(version_id);
            std::fs::create_dir_all(directory.join("mods")).unwrap();
            std::fs::write(directory.join("mods/isolated.jar"), b"isolated").unwrap();
            std::fs::write(
                directory.join(format!("{version_id}.json")),
                serde_json::to_vec(&json!({ "id": "1.20.1", "type": "release" })).unwrap(),
            ).unwrap();
            if named {
                std::fs::write(
                    directory.join("nowhere.profile.json"),
                    serde_json::to_vec(&json!({
                        "schemaVersion": 1, "name": version_id, "minecraftVersion": "1.20.1",
                        "loader": null, "loaderVersion": "", "ready": true
                    })).unwrap(),
                ).unwrap();
            }
            for isolation in [false, true] {
                let resources = crate::runtime_settings::game_directory(&root, version_id, isolation);
                let instance = inspect_resolved(&root, version_id, resources.clone()).unwrap();
                let paths: Vec<_> = instance.preview.files.iter().map(|file| file.path.as_str()).collect();
                let expected = if isolation { "mods/isolated.jar" } else { "mods/shared.jar" };
                assert_eq!(paths, vec![expected], "named={named}, isolation={isolation}");
                assert_eq!(instance.resource_root, resources);
            }
        }
    }

    #[test]
    fn local_import_accepts_curseforge_manifest_but_not_arbitrary_zip() {
        use minecraft_java_rs_core::net::transfer::CancellationToken;
        let fixture=Fixture::new();
        let cancel=CancellationToken::new();
        let cf=fixture.0.join("cf.zip");
        let manifest=json!({"manifestType":"minecraftModpack","manifestVersion":1,"name":"CF local","version":"1","minecraft":{"version":"1.20.1","modLoaders":[{"id":"forge-47.3.0","primary":true}]},"files":[],"overrides":"overrides"});
        write_zip_files(&cf,&fixture.0,&[],&[("manifest.json".into(),serde_json::to_vec(&manifest).unwrap())],&ArchiveLimits::default(),&cancel,|_,_,_|{}).unwrap();
        assert_eq!(modpacks::local_manifest(&cf,&cancel).unwrap().1.name,"CF local");
        let arbitrary=fixture.0.join("arbitrary.zip");
        write_zip_files(&arbitrary,&fixture.0,&[],&[("file.txt".into(),b"not a pack".to_vec())],&ArchiveLimits::default(),&cancel,|_,_,_|{}).unwrap();
        assert!(modpacks::local_manifest(&arbitrary,&cancel).is_err());
    }

    #[test]
    fn remote_resource_requires_matching_hash_size_and_modrinth_host() {
        let hash="a".repeat(128);
        let mut version=json!({"files":[{"hashes":{"sha512":hash,"sha1":"b".repeat(40)},"size":42,"url":"https://cdn.modrinth.com/data/example/file.jar"}]});
        assert!(remote_entry(&version,"mods/a.jar",&hash,42).is_ok());
        assert!(remote_entry(&version,"mods/a.jar",&hash,43).is_err());
        version["files"][0]["url"]=json!("https://example.com/file.jar");
        assert!(remote_entry(&version,"mods/a.jar",&hash,42).is_err());
    }

    #[test]
    fn export_config_roundtrip_keeps_flags_and_rejects_credentials() {
        let config=ExportConfig{name:"包".into(),version:"1".into(),files:vec!["mods/a.jar".into()],bundle_resources:true,modrinth_only:true, ..ExportConfig::default()};
        let decoded:ExportConfig=serde_json::from_slice(&serde_json::to_vec(&config).unwrap()).unwrap();
        assert!(decoded.bundle_resources && decoded.modrinth_only);
        let fixture=Fixture::new();
        let path=fixture.0.join("service.json");
        std::fs::write(&path,br#"{"access_token":"fixture-value"}"#).unwrap();
        assert!(reject_credentials(&path,"config/service.json").is_err());
    }


    #[test]
    fn personal_content_is_opt_in_and_secrets_are_excluded() {
        for path in [
            "screenshots/a.png",
            "saves/World/level.dat",
            "servers.dat",
            "command_history.txt",
            "Xaero/map/a.dat",
            "mods/a.jar.disabled",
            "mods/a.jar.old",
        ] {
            assert!(!group_for(path).unwrap().1, "{path}");
        }
        for path in [
            "config/accounts.json",
            "config/service/secrets.json",
            "mods/.env",
            "config/api-token.txt",
            "nowhere.profile.json",
            "options.txt/file",
        ] {
            assert!(group_for(path).is_none(), "{path}");
        }
    }

    #[test]
    fn liteloader_export_enumerates_minecraft_specific_mods_only() {
        let fixture = Fixture::new();
        let root = fixture.0.clone();
        std::fs::create_dir_all(root.join("mods/1.12.2")).unwrap();
        std::fs::write(root.join("mods/ignored.jar"), b"ignored").unwrap();
        std::fs::write(root.join("mods/1.12.2/lite.jar"), b"lite").unwrap();
        std::fs::write(root.join("mods/1.12.2/legacy.litemod.old"), b"old").unwrap();
        let lite = content_directory(&root, "mods", "LiteLoader", "1.12.2");
        assert_eq!(lite, root.join("mods/1.12.2"));
        let mut files = Vec::new();
        let mut total = 0;
        collect_files(&root, &lite, &mut files, &mut total).unwrap();
        assert!(files
            .iter()
            .any(|file| file.path == "mods/1.12.2/lite.jar" && file.group == "mods"));
        assert!(files.iter().any(|file| {
            file.path == "mods/1.12.2/legacy.litemod.old"
                && file.group == "disabledMods"
                && !file.selected
        }));
        assert!(files.iter().all(|file| !file.path.starts_with("mods/ignored")));
        assert_eq!(
            content_directory(&root, "mods", "forge", "1.20.1"),
            root.join("mods")
        );
    }

    #[test]
    fn xaero_paths_and_safe_instance_settings_preview_without_leaking_settings_file() {
        let fixture = Fixture::new();
        let version_id = "settings-preview";
        let directory = fixture.0.join("versions").join(version_id);
        std::fs::create_dir_all(directory.join("XaeroWaypoints")).unwrap();
        std::fs::create_dir_all(directory.join("XaeroWorldMap")).unwrap();
        std::fs::write(directory.join("XaeroWaypoints/waypoints.txt"), b"waypoint").unwrap();
        std::fs::write(directory.join("XaeroWorldMap/region.dat"), b"map").unwrap();
        std::fs::write(directory.join("nowhere.settings.json"), serde_json::to_vec(&json!({
            "windowTitle": "安全标题", "memoryMode": "custom", "memoryMb": 6144,
            "jvmArgs": "-XX:+UseG1GC", "javaPath": "C:/Users/private/java.exe",
            "accessToken": "secret", "preLaunchCommand": "echo hidden",
        })).unwrap()).unwrap();
        std::fs::write(directory.join("nowhere.profile.json"), serde_json::to_vec(&json!({
            "schemaVersion": 1, "name": version_id, "minecraftVersion": "1.20.1",
            "loader": null, "loaderVersion": "", "ready": true
        })).unwrap()).unwrap();
        std::fs::write(directory.join(format!("{version_id}.json")), serde_json::to_vec(&json!({
            "id": "1.20.1", "type": "release",
            "nowhereCustom": {"name": "设置预览"}
        })).unwrap()).unwrap();

        let root = std::fs::canonicalize(&fixture.0).unwrap();
        let resources = crate::runtime_settings::game_directory(&root, version_id, true);
        let instance = inspect_resolved(&root, version_id, resources).unwrap();
        assert!(instance.preview.instance_settings.iter().any(|field| field == "窗口标题"));
        assert!(instance.preview.instance_settings.iter().any(|field| field == "内存上限"));
        assert!(instance.preview.instance_settings.iter().all(|field| field != "Java 路径"));
        assert!(instance.preview.files.iter().any(|file| file.path == "XaeroWaypoints/waypoints.txt" && file.group == "maps"));
        assert!(instance.preview.files.iter().any(|file| file.path == "XaeroWorldMap/region.dat" && file.group == "maps"));
        assert!(instance.preview.files.iter().all(|file| file.path != "nowhere.settings.json"));
        assert_eq!(group_for("XaeroWaypoints/waypoints.txt"), Some(("maps", false)));
        assert_eq!(group_for("XaeroWorldMap/region.dat"), Some(("maps", false)));
    }

    #[test]
    fn standard_manifest_roundtrips_with_exact_loader() {
        let preview = ExportPreview {
            name: "名称".into(),
            minecraft_version: "1.20.1".into(),
            loader: Some(modpacks::PackLoader {
                kind: "fabric".into(),
                version: "0.16.9".into(),
            }),
            files: vec![],
            launcher: crate::portable_pack::launcher_availability(),
            instance_settings: vec![],
            instance_settings_omitted: vec![],
            instance_settings_error: None,
        };
        let config = ExportConfig {
            name: "我的整合包".into(),
            version: "1.0.0".into(),
            files: vec![],
            bundle_resources: false,
            modrinth_only: false,
            ..ExportConfig::default()
        };
        let parsed =
            modpacks::parse_modrinth_manifest(&manifest(&preview, &config, &[]).unwrap()).unwrap();
        assert_eq!(parsed.name, config.name);
        assert_eq!(parsed.loader, preview.loader);
        assert_eq!(parsed.minecraft_version, preview.minecraft_version);
    }

    #[test]
    fn config_rejects_escape_duplicates_and_credentials() {
        for files in [
            vec!["../x"],
            vec!["mods/A.jar", "mods/a.jar"],
            vec!["config/password.txt"],
            vec!["libraries/a.jar"],
        ] {
            let config = ExportConfig {
                name: "包".into(),
                version: "1".into(),
                files: files.into_iter().map(str::to_owned).collect(),
                bundle_resources: false,
                modrinth_only: false,
                ..ExportConfig::default()
            };
            assert!(validate_config(&config).is_err());
        }
    }


    #[test]
    fn old_export_config_and_request_keep_mrpack_defaults() {
        let old = json!({ "name": "旧配置", "version": "1", "files": ["mods/local.jar"] });
        let config: ExportConfig = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(config.format, ExportFormat::Mrpack);
        assert!(!config.include_launcher && !config.include_personalization);
        validate_config(&config).unwrap();
        let mut request = old;
        request["dir"] = json!("fixture-root");
        request["versionId"] = json!("fixture-instance");
        let decoded: PackExportRequest = serde_json::from_value(request.clone()).unwrap();
        assert_eq!(decoded.format, ExportFormat::Mrpack);
        assert!(!decoded.include_launcher && !decoded.include_personalization);
        request["accessToken"] = json!("not-allowed");
        assert!(serde_json::from_value::<PackExportRequest>(request).is_err());
    }

    #[test]
    fn both_formats_persist_all_options_without_weakening_config_validation() {
        for include_launcher in [false, true] {
            for include_personalization in [false, true] {
                let config = ExportConfig {
                    name: "便携配置".into(), version: "1".into(), format: ExportFormat::Portable,
                    files: vec!["mods/local.jar".into()], bundle_resources: true, modrinth_only: true,
                    include_launcher, include_personalization,
                };
                let encoded = serde_json::to_value(&config).unwrap();
                let decoded: ExportConfig = serde_json::from_value(encoded.clone()).unwrap();
                assert_eq!(decoded.format, ExportFormat::Portable);
                assert_eq!(decoded.include_launcher, include_launcher);
                assert_eq!(decoded.include_personalization, include_personalization);
                assert!(decoded.bundle_resources && decoded.modrinth_only);
                validate_config(&decoded).unwrap();
                let mut unknown = encoded.clone();
                unknown["dir"] = json!("private-path");
                assert!(serde_json::from_value::<ExportConfig>(unknown).is_err());
                let mut invalid = encoded;
                invalid["format"] = json!("renamed-zip");
                assert!(serde_json::from_value::<ExportConfig>(invalid).is_err());
                let standard = ExportConfig { format: ExportFormat::Mrpack, ..config };
                assert_eq!(validate_config(&standard).is_ok(), !include_launcher && !include_personalization);
            }
        }
        assert_eq!(ExportFormat::Mrpack.extension(), "mrpack");
        assert_eq!(ExportFormat::Portable.extension(), "zip");
    }


    #[test]
    fn portable_text_rejects_host_paths_and_commands_without_changing_standard_policy() {
        let fixture = Fixture::new();
        let path = fixture.0.join("client.json");
        for value in [
            json!({"folder": "C:/Users/private/game"}),
            json!({"folder": "C:\\Users\\private\\game"}),
            json!({"folder": "/home/private/game"}),
            json!({"folder": "\\\\server\\private\\game"}),
            json!({"folder": "/custom-root/game/mods"}),
            json!({"cache": "/tmp/local-cache"}),
            json!({"java": "/opt/jdk/bin/java"}),
            json!({"folder": "/"}),
            json!({"nested": [{"folder": " /var/cache/game "}]}),
            json!({"folder": " //server/share/game "}),
            json!({"folder": " \\\\server\\share\\game "}),
            json!({"folder": "cache=/mnt/games/client"}),
            json!({"folder": "file:///opt/jdk/bin/java"}),
            json!({"/tmp/local-cache": "cached"}),
            json!({"nested": [{"C:\\games\\local": true}]}),
            json!({"\\\\server\\share\\game": "cached"}),
            json!({"pre_launch_command": "echo start"}),
        ] {
            std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(reject_portable_host_data(&path, "config/client.json", &[Path::new("/custom-root/game")]).is_err(), "{value}");
            assert!(reject_credentials(&path, "config/client.json").is_ok(), "{value}");
        }
        for text in [
            r#"{"folder":"\u0043:\u002fUsers/private/game"}"#,
            r#"{"cache":"\u002ftmp/local-cache"}"#,
            r#"{"java":"\/opt\/jdk\/bin\/java"}"#,
            r#"{"folder":"\u005c\u005cserver\u005cshare\u005cgame"}"#,
            r#"{"\u002ftmp/local-cache":"cached"}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(reject_portable_host_data(&path, "config/client.json", &[]).is_err(), "{text}");
            assert!(reject_credentials(&path, "config/client.json").is_ok(), "{text}");
        }
        let portable = json!({
            "folder": "config/client",
            "relative": "../shared/settings",
            "url": "https://example.com/mod?cache=/tmp/local-cache&java=/opt/jdk/bin/java",
            "homeUrl": "https://example.com/home/private/game",
            "sourceUrl": "https://example.com/custom-root/game/assets",
            "ipv6Url": "https://[::1]/root/files",
            "resource": "minecraft:textures/block/stone.png",
            "model": "example:models/home/interior",
            "asset": "example:/textures/item.png",
            "https://example.com/home/files": "remote",
            "minecraft:textures/block/stone.png": "resource",
        });
        std::fs::write(&path, serde_json::to_vec(&portable).unwrap()).unwrap();
        reject_portable_host_data(&path, "config/client.json", &[Path::new("/custom-root/game")]).unwrap();
        for text in ["cache=/tmp/local-cache", "java=/opt/jdk/bin/java", r"cache=\\server\share\game"] {
            std::fs::write(&path, text).unwrap();
            assert!(reject_portable_host_data(&path, "config/client.json", &[]).is_err(), "{text}");
        }
        std::fs::write(&path, "url=https://example.com/home/files?cache=/tmp/local-cache\nmodel=minecraft:textures/block/stone.png").unwrap();
        reject_portable_host_data(&path, "config/client.json", &[]).unwrap();
    }

    #[test]
    fn shader_options_are_separate_from_resources_and_keep_old_path_configs() {
        use minecraft_java_rs_core::net::transfer::CancellationToken;
        use minecraft_java_rs_core::utils::archive::extract_zip_subtrees;

        let settings = [
            "optionsshaders.txt", "config/iris.properties", "config/oculus.properties",
            "shaderpacks/Forest.zip.txt", "shaderpacks/Forest.properties", "shaderpacks/Forest.json",
        ];
        for path in settings {
            assert_eq!(group_for(path), Some(("shaderOptions", true)), "{path}");
            assert!(!is_download_resource(path), "{path}");
        }
        assert_eq!(group_for("shaderpacks/Forest.zip"), Some(("shaderpacks", true)));
        assert!(is_download_resource("shaderpacks/Forest.zip"));
        assert_eq!(group_for("shaderpacks/Forest/shaders/shaders.properties"), Some(("shaderpacks", true)));
        assert_eq!(group_for("config/client.properties"), Some(("config", true)));
        assert_eq!(group_for("optionsof.txt"), Some(("gameOptions", true)));
        assert!(group_for("shaderpacks/private-token.txt").is_none());

        let fixture = Fixture::new();
        for path in settings {
            let file = fixture.0.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "quality=high").unwrap();
        }
        let old: ExportConfig = serde_json::from_value(json!({
            "name": "光影设置", "version": "1", "files": settings,
        })).unwrap();
        validate_config(&old).unwrap();
        assert_eq!(old.files, settings);
        let mut files = Vec::new();
        let mut size = 0;
        for path in &old.files {
            collect_files(&fixture.0, &fixture.0.join(path), &mut files, &mut size).unwrap();
        }
        assert!(files.iter().all(|file| file.group == "shaderOptions" && file.selected));
        let preview = ExportPreview {
            name: old.name.clone(), minecraft_version: "1.20.1".into(), loader: None, files,
            launcher: crate::portable_pack::launcher_availability(),
            instance_settings: vec![], instance_settings_omitted: vec![], instance_settings_error: None,
        };
        let entries = old.files.iter().map(|path| (format!("overrides/{path}"), path.clone())).collect::<Vec<_>>();
        let archive = fixture.0.join("shader-options.mrpack");
        let cancel = CancellationToken::new();
        write_zip_files(&archive, &fixture.0, &entries, &[("modrinth.index.json".into(), manifest(&preview, &old, &[]).unwrap())], &ArchiveLimits::default(), &cancel, |_, _, _| {}).unwrap();
        let (_, imported) = modpacks::local_manifest(&archive, &cancel).unwrap();
        let destination = fixture.0.join("imported");
        extract_zip_subtrees(&archive, &destination, &imported.overrides, &[], &ArchiveLimits::default(), &cancel).unwrap();
        for path in settings {
            assert_eq!(std::fs::read_to_string(destination.join(path)).unwrap(), "quality=high");
        }
        assert!(!destination.join("shaderpacks/Forest.zip").exists());
    }



}
