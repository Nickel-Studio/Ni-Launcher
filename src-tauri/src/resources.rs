//! 实例 Mod 文件操作：路径与启动隔离一致，更新下载验证后才替换旧文件。
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use minecraft_java_rs_core::net::client::reqwest::Url;
use minecraft_java_rs_core::net::transfer::{CancellationToken, FileHashes, TransferContext};
use minecraft_java_rs_core::utils::archive::{read_zip_entry, validate_relative_path};
use minecraft_java_rs_core::utils::hash::{get_file_hash, HashAlgorithm};
use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::app_tasks::{self, TaskContext};
use crate::community_network::{self, Platform, MODRINTH_API};
use crate::{instances, network_settings, runtime_settings, store};

#[path = "instance_paths.rs"]
pub(crate) mod instance_paths;

const METADATA_LIMIT: u64 = 256 * 1024;
const ICON_LIMIT: u64 = 512 * 1024;
const UPDATE_CONCURRENCY: usize = 4;
const MOD_UPDATE_BUDGET_SECS: u64 = 30;
/// 单条元数据读取上限：只解压一个清单与少量图标路径，正常远低于此值。
const MOD_META_TIMEOUT_SECS: u64 = 10;
const MAX_SELECTION: usize = 1024;
const MAX_TAGS: usize = 32;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModEntry {
    /// 当前参与启停的主物理文件；启用版本存在时优先。
    path: String,
    /// 同一逻辑 Mod 的全部物理文件，用于显示冲突与诊断。
    physical_paths: Vec<String>,
    name: String,
    description: String,
    version: String,
    enabled: bool,
    update_available: bool,
    /// JAR 内图标条目名；图标字节由 `version_mod_icon` 按需读取。
    icon_path: Option<String>,
    duplicate_group: Option<String>,
    tags: Vec<String>,
}

/// 单条 Mod 的真实元数据；`duplicate_group` 是身份键候选，由前端按全局计数决定是否展示重复。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModMeta {
    name: String,
    description: String,
    version: String,
    tags: Vec<String>,
    icon_path: Option<String>,
    duplicate_group: Option<String>,
}

/// 更新检查的逐项结果；只回传路径与状态，避免重新发送整张列表。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModUpdateState {
    path: String,
    update_available: bool,
}

#[derive(Clone)]
struct Compatibility { minecraft: String, loader: String }

struct ModUpdate { filename: String, url: String, sha1: String }

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ModState {
    Enabled,
    Disabled,
    Old,
}

#[derive(Debug, Clone)]
struct ModFile {
    path: PathBuf,
    filename: String,
    base: String,
    state: ModState,
}

/// 只把已知 Mod 容器与状态后缀识别为可操作文件；其它 ZIP 不进入 Mod 列表。
fn split_mod_name(name: &str) -> Option<(String, ModState)> {
    let lower = name.to_lowercase();
    let (base, state) = if lower.ends_with(".disabled") {
        (&name[..name.len() - ".disabled".len()], ModState::Disabled)
    } else if lower.ends_with(".old") {
        (&name[..name.len() - ".old".len()], ModState::Old)
    } else {
        (name, ModState::Enabled)
    };
    let base_lower = base.to_lowercase();
    [".jar", ".zip", ".litemod"]
        .iter()
        .any(|extension| base_lower.len() > extension.len() && base_lower.ends_with(extension))
        .then(|| (base.to_owned(), state))
}

fn mod_filename(name: &str) -> bool {
    split_mod_name(name).is_some()
}

fn push_tag(tags: &mut Vec<String>, raw: &str) {
    let value = raw.trim();
    if value.is_empty()
        || value.len() > 64
        || value.chars().any(char::is_control)
        || tags.len() >= MAX_TAGS
        || tags.iter().any(|tag| tag.eq_ignore_ascii_case(value))
    {
        return;
    }
    tags.push(value.to_owned());
}

fn collect_tags(value: &Value, tags: &mut Vec<String>) {
    for key in ["categories", "category", "tags", "keywords"] {
        let Some(raw) = value.get(key) else { continue };
        if let Some(items) = raw.as_array() {
            for item in items {
                if let Some(text) = item.as_str() {
                    push_tag(tags, text);
                } else if let Some(text) = item.get("name").and_then(Value::as_str)
                    .or_else(|| item.get("title").and_then(Value::as_str))
                {
                    push_tag(tags, text);
                }
            }
        } else if let Some(text) = raw.as_str() {
            push_tag(tags, text);
        }
    }
}

fn selected_paths(folder: &Path, paths: &[String]) -> Result<Vec<PathBuf>, String> {
    if paths.is_empty() || paths.len() > MAX_SELECTION { return Err("请选择 1 至 1024 个 Mod 文件".into()); }
    instances::reject_links(folder)?;
    let folder = std::fs::canonicalize(folder).map_err(|e| format!("读取 Mod 文件夹失败：{e}"))?;
    let mut seen = HashSet::new();
    paths.iter().map(|raw| {
        let path = PathBuf::from(raw);
        if !path.is_absolute() { return Err("Mod 路径必须是绝对路径".into()); }
        instances::reject_links(&path)?;
        let path = std::fs::canonicalize(&path).map_err(|e| format!("Mod 文件已不存在：{e}"))?;
        let name = path.file_name().and_then(|name| name.to_str()).ok_or("Mod 文件名无效")?;
        if path.parent() != Some(folder.as_path()) || !path.is_file() || !mod_filename(name) {
            return Err("只能操作当前实例 mods 文件夹中的 Mod 文件".into());
        }
        if !seen.insert(path.clone()) { return Err("Mod 选择包含重复文件，请刷新后重试".into()); }
        Ok(path)
    }).collect()
}

fn metadata_json(path: &Path, name: &str, cancel: &CancellationToken) -> Option<Value> {
    let bytes = read_zip_entry(path, name, METADATA_LIMIT, cancel).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn json_mod_data(path: &Path, cancel: &CancellationToken) -> Option<Value> {
    metadata_json(path, "fabric.mod.json", cancel)
        .or_else(|| metadata_json(path, "quilt.mod.json", cancel).map(|value| {
            let loader = &value["quilt_loader"];
            let mut data = loader["metadata"].clone();
            if !data.is_object() { data = serde_json::json!({}); }
            data["version"] = loader["version"].clone();
            if let Some(id) = loader["id"].as_str() { data["id"] = Value::String(id.into()); }
            data
        }))
        .or_else(|| metadata_json(path, "mcmod.info", cancel).and_then(|value| {
            value.as_array().and_then(|items| items.first()).cloned()
                .or_else(|| value.get("modList").and_then(Value::as_array).and_then(|items| items.first()).cloned())
        }))
}

fn toml_mod_data(
    path: &Path,
    cancel: &CancellationToken,
    neoforge_first: bool,
) -> Option<Value> {
    let manifests = if neoforge_first {
        ["META-INF/neoforge.mods.toml", "META-INF/mods.toml"]
    } else {
        ["META-INF/mods.toml", "META-INF/neoforge.mods.toml"]
    };
    for manifest in manifests {
        let Ok(bytes) = read_zip_entry(path, manifest, METADATA_LIMIT, cancel) else { continue };
        let Ok(text) = std::str::from_utf8(&bytes) else { continue };
        let mut data = serde_json::Map::new();
        for line in text.lines() {
            let Some((key, raw)) = line.trim().split_once('=') else { continue };
            let value = serde_json::from_str::<String>(raw.trim()).ok()
                .or_else(|| raw.trim().strip_prefix('\'').and_then(|value| value.strip_suffix('\'')).map(str::to_owned));
            let Some(value) = value else { continue };
            match key.trim() {
                "modId" => { data.insert("id".into(), Value::String(value)); }
                "displayName" => { data.insert("name".into(), Value::String(value)); }
                "description" => { data.insert("description".into(), Value::String(value)); }
                "version" if !value.contains("${") => { data.insert("version".into(), Value::String(value)); }
                "logoFile" => { data.insert("logoFile".into(), Value::String(value)); }
                _ => {}
            }
        }
        if !data.is_empty() { return Some(Value::Object(data)); }
    }
    None
}
fn mod_metadata(path: &Path, cancel: &CancellationToken, loader: Option<&str>) -> Option<Value> {
    // 按实例加载器选择首个清单，避免每个 Forge Mod 先白读 fabric / quilt / mcmod 再读 TOML。
    match loader.map(str::to_ascii_lowercase).as_deref() {
        Some("forge") => toml_mod_data(path, cancel, false).or_else(|| json_mod_data(path, cancel)),
        Some("neoforge") => toml_mod_data(path, cancel, true).or_else(|| json_mod_data(path, cancel)),
        _ => json_mod_data(path, cancel).or_else(|| toml_mod_data(path, cancel, true)),
    }
}

fn normalize_group_text(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

fn duplicate_group_key(data: Option<&Value>, entry: &ModEntry) -> String {
    if let Some(id) = data.and_then(|value| {
        ["id", "modid", "modId"]
            .iter()
            .find_map(|key| value.get(*key).and_then(Value::as_str))
    }).map(normalize_group_text).filter(|value| !value.is_empty()) {
        return format!("id:{id}");
    }
    let mut name = entry.name.trim().to_lowercase();
    let version = entry.version.trim().to_lowercase();
    if !version.is_empty() {
        if let Some(prefix) = name.strip_suffix(&version) {
            name = prefix.trim_end_matches([' ', '-', '_', 'v']).to_string();
        }
    }
    format!("name:{}", normalize_group_text(&name))
}

/// 单个逻辑 Mod 的解析结果：条目、供重复检测使用的身份键，以及是否真正读到清单。
struct InferredMod {
    entry: ModEntry,
    key: String,
    has_metadata: bool,
}

fn infer_mod(mut files: Vec<ModFile>, loader: Option<&str>) -> Option<InferredMod> {
    files.sort_by(|left, right| {
        left.state.cmp(&right.state)
            .then_with(|| left.filename.to_lowercase().cmp(&right.filename.to_lowercase()))
    });
    let primary = files.first()?;
    let physical_paths = files.iter().map(|file| file.path.to_string_lossy().into_owned()).collect();
    let mut entry = ModEntry {
        path: primary.path.to_string_lossy().into_owned(),
        physical_paths,
        name: primary.base.clone(),
        description: String::new(),
        version: String::new(),
        enabled: primary.state == ModState::Enabled,
        update_available: false,
        icon_path: None,
        duplicate_group: None,
        tags: Vec::new(),
    };
    let cancel = CancellationToken::new();
    let mut metadata = None;
    for file in &files {
        if let Some(data) = mod_metadata(&file.path, &cancel, loader) {
            entry.name = data["name"].as_str().filter(|value| !value.is_empty()).unwrap_or(&file.base).into();
            if let Some(description) = data["description"].as_str() { entry.description = description.into(); }
            if let Some(version) = data["version"].as_str() { entry.version = version.into(); }
            collect_tags(&data, &mut entry.tags);
            entry.icon_path = mod_icon_entry(&data);
            metadata = Some(data);
            break;
        }
    }
    let key = duplicate_group_key(metadata.as_ref(), &entry);
    let has_metadata = metadata.is_some();
    Some(InferredMod { entry, key, has_metadata })
}

/// 元数据里的图标字段可能是字符串或 `{ "1x": "..." }` 映射；这里只接受合法的
/// JAR 内相对路径，真正的字节读取与 base64 编码留到 `version_mod_icon` 按需完成，
/// 避免列表扫描被 306 个大图标拖慢。
fn mod_icon_entry(data: &Value) -> Option<String> {
    let raw = data.get("icon").and_then(|value| value.as_str().or_else(|| value.as_object()?.values().find_map(Value::as_str)))
        .or_else(|| data["logoFile"].as_str())?;
    let entry = raw.trim();
    if entry.is_empty() { return None; }
    validate_relative_path(entry).ok()
}

fn read_icon(path: &Path, name: &str, cancel: &CancellationToken) -> Option<String> {
    use base64::Engine;
    let bytes = read_zip_entry(path, name, ICON_LIMIT, cancel).ok()?;
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") { "image/png" }
        else if bytes.starts_with(&[0xff, 0xd8, 0xff]) { "image/jpeg" }
        else { return None; };
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[cfg(test)]
fn read_mod(path: PathBuf) -> ModEntry {
    let filename = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let Some((base, state)) = split_mod_name(&filename) else {
        return ModEntry {
            path: path.to_string_lossy().into_owned(), physical_paths: vec![path.to_string_lossy().into_owned()],
            name: filename, description: String::new(), version: String::new(), enabled: true,
            update_available: false, icon_path: None, duplicate_group: None, tags: Vec::new(),
        };
    };
    infer_mod(vec![ModFile { path, filename, base, state }], None)
        .map(|inferred| inferred.entry)
        .unwrap_or_else(|| ModEntry {
            path: String::new(), physical_paths: Vec::new(), name: String::new(),
            description: String::new(), version: String::new(), enabled: true,
            update_available: false, icon_path: None,
            duplicate_group: None, tags: Vec::new(),
        })
}

/// 扫描工作线程数：读取 JAR 元数据是解压与解析的混合负载，按逻辑 Mod 分组并行。
/// 上限取逻辑 CPU 数（最多 16），避免机械盘被过多线程拖慢，也不为 300 个 Mod 创建 300 个线程。
fn scan_workers(groups: usize) -> usize {
    let available = std::thread::available_parallelism().map(|value| value.get()).unwrap_or(1);
    available.min(16).min(groups.max(1))
}

fn scan_groups(
    mut groups: Vec<Vec<ModFile>>,
    loader: Option<&str>,
) -> Result<Vec<(ModEntry, String)>, String> {
    let workers = scan_workers(groups.len());
    if workers <= 1 {
        return Ok(groups
            .into_iter()
            .filter_map(|files| infer_mod(files, loader))
            .map(|inferred| (inferred.entry, inferred.key))
            .collect());
    }
    let chunk_size = groups.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let handles: Vec<_> = groups
            .chunks_mut(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter_mut()
                        .filter_map(|files| infer_mod(std::mem::take(files), loader))
                        .map(|inferred| (inferred.entry, inferred.key))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        // 必须全部 join，避免 scoped thread 未回收时在作用域结束时再次 panic。
        let mut parsed = Vec::new();
        let mut failed = false;
        for handle in handles {
            match handle.join() {
                Ok(items) => parsed.extend(items),
                Err(_) => failed = true,
            }
        }
        if failed { Err("扫描 Mod 元数据时线程异常终止，请重新扫描".into()) } else { Ok(parsed) }
    })
}
#[cfg(test)]
fn scan_local(folder: &Path) -> Result<Vec<ModEntry>, String> {
    scan_local_with_loader(folder, None)
}

fn scan_local_with_loader(folder: &Path, loader: Option<&str>) -> Result<Vec<ModEntry>, String> {
    if !folder.exists() { return Ok(Vec::new()); }
    instances::reject_links(folder)?;
    let mut groups: HashMap<String, Vec<ModFile>> = HashMap::new();
    for item in std::fs::read_dir(folder).map_err(|e| format!("读取 Mod 文件夹失败：{e}"))? {
        let item = item.map_err(|e| format!("读取 Mod 文件失败：{e}"))?;
        let path = item.path();
        let filename = item.file_name().to_string_lossy().into_owned();
        let Some((base, state)) = split_mod_name(&filename) else { continue };
        if item.file_type().map_err(|e| e.to_string())?.is_file()
            && instances::reject_links(&path).is_ok()
        {
            groups.entry(base.to_lowercase()).or_default().push(ModFile { path, filename, base, state });
        }
    }
    let mut parsed = scan_groups(groups.into_values().collect(), loader)?;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, key) in &parsed {
        *counts.entry(key.clone()).or_default() += 1;
    }
    for (entry, key) in &mut parsed {
        if counts.get(key).copied().unwrap_or(0) > 1 {
            entry.duplicate_group = Some(key.clone());
        }
    }
    let mut entries: Vec<_> = parsed.into_iter().map(|(entry, _)| entry).collect();
    entries.sort_by(|left, right| {
        left.name.to_lowercase().cmp(&right.name.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(entries)
}

/// 去掉 Mod 容器后缀，保留文件名中间的点和空格：`foo-1.2.jar.disabled` → `foo-1.2`。
fn display_mod_name(base: &str) -> String {
    let lower = base.to_lowercase();
    [".jar", ".zip", ".litemod"]
        .iter()
        .find_map(|extension| lower.ends_with(extension).then(|| base[..base.len() - extension.len()].to_owned()))
        .unwrap_or_else(|| base.to_owned())
}

/// 首屏列表：只做目录枚举与逻辑分组，不打开任何 JAR。
///
/// 名称与简介统一使用去后缀文件名占位，真实元数据由 `version_mod_meta` 逐条补齐；
/// 按 path 排序保证后续静默更新只替换行内容、不改变行顺序。
fn scan_local_list(folder: &Path) -> Result<Vec<ModEntry>, String> {
    if !folder.exists() { return Ok(Vec::new()); }
    instances::reject_links(folder)?;
    let mut groups: HashMap<String, Vec<ModFile>> = HashMap::new();
    for item in std::fs::read_dir(folder).map_err(|e| format!("读取 Mod 文件夹失败：{e}"))? {
        let item = item.map_err(|e| format!("读取 Mod 文件失败：{e}"))?;
        let path = item.path();
        let filename = item.file_name().to_string_lossy().into_owned();
        let Some((base, state)) = split_mod_name(&filename) else { continue };
        if item.file_type().map_err(|e| e.to_string())?.is_file()
            && instances::reject_links(&path).is_ok()
        {
            groups.entry(base.to_lowercase()).or_default().push(ModFile { path, filename, base, state });
        }
    }
    let mut entries = Vec::with_capacity(groups.len());
    for mut files in groups.into_values() {
        files.sort_by(|left, right| {
            left.state.cmp(&right.state)
                .then_with(|| left.filename.to_lowercase().cmp(&right.filename.to_lowercase()))
        });
        let Some(primary) = files.first() else { continue };
        let placeholder = display_mod_name(&primary.base);
        entries.push(ModEntry {
            path: primary.path.to_string_lossy().into_owned(),
            physical_paths: files.iter().map(|file| file.path.to_string_lossy().into_owned()).collect(),
            name: placeholder.clone(),
            description: placeholder,
            version: String::new(),
            enabled: primary.state == ModState::Enabled,
            update_available: false,
            icon_path: None,
            // 同一 base 的 .jar/.disabled/.old 是同一逻辑 Mod 的物理副本，不是重复；
            // 真实重复身份只在元数据到达后由前端按 id/name 计算。
            duplicate_group: None,
            tags: Vec::new(),
        });
    }
    entries.sort_by(|left, right| {
        left.path.to_lowercase().cmp(&right.path.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(entries)
}

fn compatibility(dir: &str, version_id: &str) -> Result<Compatibility, String> {
    let detail: Value = serde_json::from_str(&crate::version_settings::version_detail(dir.into(), version_id.into())?)
        .map_err(|e| format!("读取实例信息失败：{e}"))?;
    Ok(compatibility_from_detail(&detail))
}

fn compatibility_from_detail(detail: &Value) -> Compatibility {
    // 未知版本保留为空，由更新检查返回行内错误；目录身份不能进入社区 API。
    let minecraft = detail["minecraftVersion"].as_str().unwrap_or("").trim().to_owned();
    let loader = detail["loader"].as_str().unwrap_or("").to_lowercase();
    Compatibility { minecraft, loader }
}

/// CurseForge 使用的 MurmurHash2（seed 1，跳过 ASCII 空白字节）。
///
/// 必须流式实现：单个 Mod 可能上百 MB，4 路并发时不能把整份文件读进内存。
struct Murmur2 {
    hash: u32,
    len: u32,
    tail: [u8; 4],
    tail_len: usize,
}

impl Murmur2 {
    const M: u32 = 0x5bd1_e995;
    const R: u32 = 24;

    fn new() -> Self {
        Self { hash: 1, len: 0, tail: [0; 4], tail_len: 0 }
    }

    fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if matches!(byte, b'\t' | b'\n' | b'\r' | b' ') {
                continue;
            }
            self.tail[self.tail_len] = byte;
            self.tail_len += 1;
            self.len = self.len.wrapping_add(1);
            if self.tail_len == 4 {
                self.mix(u32::from_le_bytes(self.tail));
                self.tail_len = 0;
            }
        }
    }

    fn mix(&mut self, mut value: u32) {
        value = value.wrapping_mul(Self::M);
        value ^= value >> Self::R;
        value = value.wrapping_mul(Self::M);
        self.hash = self.hash.wrapping_mul(Self::M);
        self.hash ^= value;
    }

    fn finish(mut self) -> u32 {
        // 尾部字节不经过 k 的两次乘法，这是 MurmurHash2 与 Hash2A 的关键差异。
        match self.tail_len {
            3 => {
                self.hash ^= (self.tail[2] as u32) << 16;
                self.hash ^= (self.tail[1] as u32) << 8;
                self.hash ^= self.tail[0] as u32;
                self.hash = self.hash.wrapping_mul(Self::M);
            }
            2 => {
                self.hash ^= (self.tail[1] as u32) << 8;
                self.hash ^= self.tail[0] as u32;
                self.hash = self.hash.wrapping_mul(Self::M);
            }
            1 => {
                self.hash ^= self.tail[0] as u32;
                self.hash = self.hash.wrapping_mul(Self::M);
            }
            _ => {}
        }
        self.hash ^= self.len;
        self.hash ^= self.hash >> 13;
        self.hash = self.hash.wrapping_mul(Self::M);
        self.hash ^= self.hash >> 15;
        self.hash
    }
}

fn murmur2_file(path: &Path) -> Result<u32, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("读取 Mod 校验值失败：{e}"))?;
    let mut hasher = Murmur2::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| format!("读取 Mod 校验值失败：{e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finish())
}

/// 双来源探测结果：`Found` 表示该来源已确认文件身份（是否有新版本都算确定）。
enum UpdateProbe {
    Found(Option<ModUpdate>),
    Unknown,
}

async fn modrinth_update(hash: &str, compatibility: &Compatibility, settings: &network_settings::NetworkSettings, transfer: &TransferContext) -> Result<Option<ModUpdate>, String> {
    if !["forge", "neoforge", "fabric", "quilt"].contains(&compatibility.loader.as_str()) { return Ok(None); }
    if compatibility.minecraft.is_empty() { return Ok(None); }
    let current = community_network::get_json(Platform::Modrinth, &format!("{MODRINTH_API}/version_file/{hash}?algorithm=sha1"), settings, transfer, None,
        |value| if value["project_id"].is_string() { Ok(value.clone()) } else { Err("Modrinth 无此文件版本".into()) }).await?;
    let project = current["project_id"].as_str().ok_or("Mod 缺少项目编号")?;
    if !project.chars().all(|ch| ch.is_ascii_alphanumeric()) { return Err("Mod 项目编号无效".into()); }
    let mut url = Url::parse(&format!("{MODRINTH_API}/project/{project}/version")).map_err(|e| e.to_string())?;
    url.query_pairs_mut()
        .append_pair("loaders", &serde_json::json!([compatibility.loader]).to_string())
        .append_pair("game_versions", &serde_json::json!([compatibility.minecraft]).to_string());
    let versions = community_network::get_json(Platform::Modrinth, url.as_str(), settings, transfer, None,
        |value| value.as_array().cloned().ok_or_else(|| "Modrinth 更新数据格式无效".into())).await?;
    select_update(&current, &versions, hash, compatibility)
}

fn curseforge_loader_type(loader: &str) -> Option<u32> {
    match loader {
        "forge" => Some(1),
        "fabric" => Some(4),
        "quilt" => Some(5),
        "neoforge" => Some(6),
        _ => None,
    }
}

async fn curseforge_update(murmur2: u32, compatibility: &Compatibility, settings: &network_settings::NetworkSettings, key: Option<&crate::secrets::CurseforgeCredential>) -> Result<Option<ModUpdate>, String> {
    let Some(loader_type) = curseforge_loader_type(&compatibility.loader) else { return Ok(None); };
    if compatibility.minecraft.is_empty() { return Ok(None); }
    let matched = community_network::post_json(
        Platform::Curseforge,
        &format!("{}fingerprints", community_network::CURSEFORGE_API),
        settings,
        key,
        &serde_json::json!({ "fingerprints": [murmur2] }),
        |value| value["data"]["exactMatches"].as_array().cloned()
            .ok_or_else(|| "CurseForge 指纹匹配格式无效".to_string()),
    ).await?;
    let Some(current) = matched.iter().find_map(|item| item.get("file")) else { return Ok(None); };
    let mod_id = current["modId"].as_i64().filter(|id| *id > 0).ok_or("CurseForge 缺少项目编号")?;
    let mut url = Url::parse(&format!("{}mods/{mod_id}/files", community_network::CURSEFORGE_API)).map_err(|e| e.to_string())?;
    url.query_pairs_mut()
        .append_pair("gameVersion", &compatibility.minecraft)
        .append_pair("modLoaderType", &loader_type.to_string())
        .append_pair("pageSize", "50");
    let files = community_network::get_json(Platform::Curseforge, url.as_str(), settings, &community_network::context(settings), key,
        |value| value["data"].as_array().cloned().ok_or_else(|| "CurseForge 文件数据格式无效".to_string())).await?;
    Ok(select_curseforge_update(current, &files))
}

fn select_curseforge_update(current: &Value, files: &[Value]) -> Option<ModUpdate> {
    let current_id = current["id"].as_i64()?;
    let current_date = current["fileDate"].as_str().unwrap_or("");
    let newest = files.iter()
        .filter(|file| file["id"].as_i64() != Some(current_id)
            && file["fileDate"].as_str().is_some_and(|date| date > current_date))
        .max_by_key(|file| file["fileDate"].as_str().unwrap_or(""))?;
    let filename = newest["fileName"].as_str()?;
    if filename.contains(['/', '\\']) || !filename.to_ascii_lowercase().ends_with(".jar") { return None; }
    crate::modpacks::validate_pack_path(filename).ok()?;
    let url = newest["downloadUrl"].as_str()?;
    community_network::file_candidates(url, "").ok()?;
    let sha1 = newest["hashes"].as_array()?.iter()
        .find(|hash| hash["algo"].as_i64() == Some(1))
        .and_then(|hash| hash["value"].as_str())?;
    if sha1.len() != 40 || !sha1.chars().all(|ch| ch.is_ascii_hexdigit()) { return None; }
    Some(ModUpdate { filename: filename.into(), url: url.into(), sha1: sha1.into() })
}

/// 单个文件先算指纹再查询；指纹读取本身也计入该来源的耗时，两个来源并行竞速。
async fn modrinth_probe(path: PathBuf, compatibility: Compatibility, settings: network_settings::NetworkSettings, transfer: TransferContext) -> UpdateProbe {
    let hash = tokio::task::spawn_blocking(move || {
        get_file_hash(&path, HashAlgorithm::Sha1).map_err(|e| e.to_string())
    })
    .await;
    let Ok(Ok(hash)) = hash else { return UpdateProbe::Unknown };
    match modrinth_update(&hash, &compatibility, &settings, &transfer).await {
        Ok(update) => UpdateProbe::Found(update),
        Err(_) => UpdateProbe::Unknown,
    }
}

async fn curseforge_probe(path: PathBuf, compatibility: Compatibility, settings: network_settings::NetworkSettings, key: Option<crate::secrets::CurseforgeCredential>) -> UpdateProbe {
    let murmur2 = tokio::task::spawn_blocking(move || murmur2_file(&path)).await;
    let Ok(Ok(murmur2)) = murmur2 else { return UpdateProbe::Unknown };
    match curseforge_update(murmur2, &compatibility, &settings, key.as_ref()).await {
        Ok(update) => UpdateProbe::Found(update),
        Err(_) => UpdateProbe::Unknown,
    }
}

/// 两个来源并发探测，谁先给出确定结果就用谁；先返回的来源无匹配或失败时，
/// 自动等待另一来源，任一来源失败都不升级为界面错误。
async fn race_probes<A, B>(modrinth: A, curseforge: B) -> Option<ModUpdate>
where
    A: std::future::Future<Output = UpdateProbe>,
    B: std::future::Future<Output = UpdateProbe>,
{
    tokio::pin!(modrinth);
    tokio::pin!(curseforge);
    tokio::select! {
        first = &mut modrinth => match first {
            UpdateProbe::Found(update) => update,
            UpdateProbe::Unknown => match curseforge.await {
                UpdateProbe::Found(update) => update,
                UpdateProbe::Unknown => None,
            },
        },
        first = &mut curseforge => match first {
            UpdateProbe::Found(update) => update,
            UpdateProbe::Unknown => match modrinth.await {
                UpdateProbe::Found(update) => update,
                UpdateProbe::Unknown => None,
            },
        },
    }
}

async fn fastest_update_for_path(path: PathBuf, compatibility: &Compatibility, settings: &network_settings::NetworkSettings, transfer: &TransferContext, key: Option<crate::secrets::CurseforgeCredential>) -> Option<ModUpdate> {
    race_probes(
        modrinth_probe(path.clone(), compatibility.clone(), settings.clone(), transfer.clone()),
        curseforge_probe(path, compatibility.clone(), settings.clone(), key),
    )
    .await
}

fn select_update(current: &Value, versions: &[Value], hash: &str, compatibility: &Compatibility) -> Result<Option<ModUpdate>, String> {
    let current_date = current["date_published"].as_str().unwrap_or("");
    let matches = |value: &Value, field: &str, expected: &str| value[field].as_array().is_some_and(|items| items.iter().any(|item| item.as_str() == Some(expected)));
    let newest = versions.iter().filter(|value| value["project_id"] == current["project_id"]
        && value["id"] != current["id"]
        && value["date_published"].as_str().is_some_and(|date| date > current_date)
        && matches(value, "game_versions", &compatibility.minecraft) && matches(value, "loaders", &compatibility.loader))
        .max_by_key(|value| value["date_published"].as_str().unwrap_or(""));
    let Some(newest) = newest else { return Ok(None); };
    let files = newest["files"].as_array().ok_or("更新版本没有文件")?;
    let file = files.iter().find(|file| file["primary"].as_bool() == Some(true)).or_else(|| files.first()).ok_or("更新版本没有文件")?;
    let filename = file["filename"].as_str().ok_or("更新缺少文件名")?;
    crate::modpacks::validate_pack_path(filename).map_err(|e| e.to_string())?;
    if filename.contains('/') || filename.contains('\\') || !filename.to_ascii_lowercase().ends_with(".jar") { return Err("更新文件必须是单个 JAR 文件".into()); }
    let sha1 = file["hashes"]["sha1"].as_str().filter(|hash| hash.len() == 40 && hash.chars().all(|ch| ch.is_ascii_hexdigit())).ok_or("更新文件缺少 SHA-1 校验值")?;
    if sha1.eq_ignore_ascii_case(hash) { return Ok(None); }
    let url = file["url"].as_str().ok_or("更新文件缺少下载地址")?;
    community_network::file_candidates(url, "")?;
    Ok(Some(ModUpdate { filename: filename.into(), url: url.into(), sha1: sha1.into() }))
}
fn updates_disabled(global: &Value, local: &Value) -> bool {
    let setting = |value: &Value| value.get("disableModUpdates").and_then(Value::as_bool);
    setting(local).or_else(|| setting(global)).unwrap_or(false)
}

fn instance_updates_disabled(app: &AppHandle, root: &Path, version_id: &str) -> Result<bool, String> {
    let global = serde_json::from_str(&crate::game::game_settings_get(app.clone())?).unwrap_or(Value::Null);
    Ok(updates_disabled(&global, &runtime_settings::read_local(root, version_id)))
}

/// 版本 JSON 已能给出加载器；Forge / NeoForge 据此优先读 TOML，避免每个 Mod 先白读 JSON 清单。
fn instance_loader_hint(dir: &str, version_id: &str) -> Option<String> {
    crate::version_settings::version_detail(dir.to_string(), version_id.to_string())
        .ok()
        .and_then(|detail| serde_json::from_str::<Value>(&detail).ok())
        .and_then(|detail| detail["loader"].as_str().map(|value| value.to_ascii_lowercase()))
}

/// 静默更新检查：Modrinth 与 CurseForge 并发探测，先给出确定结果的来源获胜；
/// 先返回的来源无匹配或失败时自动等待另一来源。任何失败都只表示“没有可确认
/// 的更新”，不写行内错误、不打断本地列表。总预算 30 秒，尽力而为。
async fn apply_update_checks(
    app: &AppHandle,
    root: &Path,
    dir: &str,
    version_id: &str,
    entries: &mut [ModEntry],
) -> Result<(), String> {
    if instance_updates_disabled(app, root, version_id)? { return Ok(()); }
    let Ok(compatibility) = compatibility(dir, version_id) else { return Ok(()); };
    if compatibility.minecraft.is_empty() { return Ok(()); }
    if !["forge", "neoforge", "fabric", "quilt"].contains(&compatibility.loader.as_str()) { return Ok(()); }
    let settings = network_settings::snapshot();
    let transfer = community_network::context(&settings);
    let key = crate::secrets::curseforge_api_key(app.clone()).ok().flatten();
    let _ = tokio::time::timeout(Duration::from_secs(MOD_UPDATE_BUDGET_SECS), async {
        for batch in entries.chunks_mut(UPDATE_CONCURRENCY) {
            let mut tasks = tokio::task::JoinSet::new();
            for (index, entry) in batch.iter_mut().enumerate() {
                entry.update_available = false;
                let (compatibility, settings, transfer, key) =
                    (compatibility.clone(), settings.clone(), transfer.clone(), key.clone());
                let path = PathBuf::from(&entry.path);
                tasks.spawn(async move {
                    let update = fastest_update_for_path(path, &compatibility, &settings, &transfer, key).await;
                    (index, update.is_some())
                });
            }
            while let Some(result) = tasks.join_next().await {
                if let Ok((index, available)) = result {
                    batch[index].update_available = available;
                }
            }
        }
    })
    .await;
    Ok(())
}

/// 更新检查只关心路径与结果，占位字段不会被回传。
fn update_only_entry(path: String) -> ModEntry {
    ModEntry {
        path: path.clone(),
        physical_paths: vec![path],
        name: String::new(),
        description: String::new(),
        version: String::new(),
        enabled: true,
        update_available: false,
        icon_path: None,
        duplicate_group: None,
        tags: Vec::new(),
    }
}

/// 去重键：Windows 路径大小写不敏感，同一文件用不同大小写传入时不能重复算哈希。
/// 其它平台保留原始字符串，避免把 `A.jar` / `a.jar` 两个真实文件误合并。
fn dedupe_key(path: &str) -> String {
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.to_string()
    }
}

/// 只保留通过实例 mods 围栏校验的路径；任何无效 / 越界路径静默视为无更新。
///
/// 更新检查会对路径计算 SHA-1 / Murmur2，不能把前端传入的任意可读文件当作输入。
fn validated_update_entries(folder: &Path, paths: &[String]) -> Vec<ModEntry> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for path in paths {
        if !seen.insert(dedupe_key(path)) {
            continue;
        }
        if validated_mod_path(folder, Path::new(path)).is_ok() {
            entries.push(update_only_entry(path.clone()));
        }
    }
    entries
}

#[tauri::command]
pub async fn version_mods(
    app: AppHandle,
    dir: String,
    version_id: String,
    check_updates: Option<bool>,
) -> Result<String, String> {
    let root = PathBuf::from(&dir);
    let resource = runtime_settings::resource_directory(&app, &root, &version_id)?;
    let folder = instance_paths::for_instance(&root, &version_id, &resource)?;
    let loader_hint = instance_loader_hint(&dir, &version_id);
    let mut entries = tokio::task::spawn_blocking(move || scan_local_with_loader(&folder, loader_hint.as_deref()))
        .await
        .map_err(|e| e.to_string())??;
    // 本地列表始终先完成；checkUpdates=false 时立即返回，联网检查由调用方另行发起。
    if check_updates.unwrap_or(true) {
        apply_update_checks(&app, &root, &dir, &version_id, &mut entries).await?;
    }
    serde_json::to_string(&entries).map_err(|e| format!("整理 Mod 列表失败：{e}"))
}

/// 首屏列表：只做 read_dir 与逻辑分组，不打开 JAR；名称/简介先以去后缀文件名占位。
#[tauri::command]
pub async fn version_mod_list(app: AppHandle, dir: String, version_id: String) -> Result<String, String> {
    let root = PathBuf::from(&dir);
    let resource = runtime_settings::resource_directory(&app, &root, &version_id)?;
    let folder = instance_paths::for_instance(&root, &version_id, &resource)?;
    let entries = tokio::task::spawn_blocking(move || scan_local_list(&folder))
        .await
        .map_err(|e| e.to_string())??;
    serde_json::to_string(&entries).map_err(|e| format!("整理 Mod 列表失败：{e}"))
}

/// 单条真实元数据；失败返回 Ok(None)，由前端保留占位，不升级为整页错误。
#[tauri::command]
pub async fn version_mod_meta(
    app: AppHandle,
    dir: String,
    version_id: String,
    path: String,
    physical_paths: Vec<String>,
) -> Result<Option<ModMeta>, String> {
    let root = PathBuf::from(&dir);
    let resource = runtime_settings::resource_directory(&app, &root, &version_id)?;
    let folder = instance_paths::for_instance(&root, &version_id, &resource)?;
    let loader_hint = instance_loader_hint(&dir, &version_id);
    let task = tokio::task::spawn_blocking(move || {
        read_mod_meta(&folder, Path::new(&path), &physical_paths, loader_hint.as_deref())
    });
    tokio::time::timeout(Duration::from_secs(MOD_META_TIMEOUT_SECS), task)
        .await
        .map_err(|_| "读取 Mod 信息超时".to_string())?
        .map_err(|error| format!("读取 Mod 信息失败：{error}"))?
}

/// 只对前端已加载的路径做更新检查，不重新扫描全部 JAR；未知版本不发请求。
#[tauri::command]
pub async fn version_mod_updates(
    app: AppHandle,
    dir: String,
    version_id: String,
    paths: Vec<String>,
) -> Result<Vec<ModUpdateState>, String> {
    let root = PathBuf::from(&dir);
    let resource = runtime_settings::resource_directory(&app, &root, &version_id)?;
    if paths.is_empty() { return Ok(Vec::new()); }
    // for_instance 读取实例详情、validated_update_entries 逐个 canonicalize：慢盘 / 网络盘
    // 上都是阻塞 IO，统一放到阻塞线程，避免占用 Tauri 的 async runtime。
    let mut entries = tauri::async_runtime::spawn_blocking({
        let root = root.clone();
        let version_id = version_id.clone();
        move || -> Result<Vec<ModEntry>, String> {
            // 仍然解析实例目录以拒绝失效实例；路径本身缺失只意味着该行没有可确认的更新。
            let folder = instance_paths::for_instance(&root, &version_id, &resource)?;
            // 只对当前实例 mods 目录内的真实 Mod 文件计算哈希；越界路径静默跳过。
            Ok(validated_update_entries(&folder, &paths))
        }
    })
    .await
    .map_err(|error| format!("校验 Mod 更新路径失败：{error}"))??;
    if entries.is_empty() { return Ok(Vec::new()); }
    apply_update_checks(&app, &root, &dir, &version_id, &mut entries).await?;
    Ok(entries
        .into_iter()
        .map(|entry| ModUpdateState {
            path: entry.path,
            update_available: entry.update_available,
        })
        .collect())
}

/// 校验 Mod 路径：绝对路径、位于当前实例 mods 目录、普通文件、自身不是链接，全程只读。
///
/// 只拒绝 mods 目录 / 文件自身的链接，不检查祖先：macOS 的临时目录位于 `/var`
/// 符号链接之下，Windows 的 OneDrive / junction 重定向目录也会让合法实例带链接祖先。
/// 锚定仍由 canonicalize 后 `parent == 实例 mods` 的围栏保证。
fn validated_mod_path(folder: &Path, path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() { return Err("Mod 路径必须是绝对路径".into()); }
    if !folder.exists() { return Err("Mod 文件夹已不存在，请重新扫描".into()); }
    instances::reject_links_local(folder)?;
    instances::reject_links_local(path)?;
    let folder = std::fs::canonicalize(folder).map_err(|e| format!("读取 Mod 文件夹失败：{e}"))?;
    let file = std::fs::canonicalize(path).map_err(|_| "Mod 文件已不存在，请重新扫描".to_string())?;
    let name = file.file_name().and_then(|name| name.to_str()).ok_or("Mod 文件名无效")?;
    if file.parent() != Some(folder.as_path()) || !file.is_file() || !mod_filename(name) {
        return Err("只能读取当前实例 mods 文件夹中的 Mod 文件".into());
    }
    Ok(file)
}

/// 单条真实元数据：复用 infer_mod / mod_metadata / mod_icon_entry，不复制解析逻辑。
/// 没有清单或清单无法解析时返回 Ok(None)，由前端保留文件名占位。
fn read_mod_meta(
    folder: &Path,
    path: &Path,
    physical_paths: &[String],
    loader: Option<&str>,
) -> Result<Option<ModMeta>, String> {
    let primary = validated_mod_path(folder, path)?;
    let primary_name = primary.file_name().and_then(|name| name.to_str()).ok_or("Mod 文件名无效")?;
    let (primary_base, _) = split_mod_name(primary_name).ok_or("请选择有效的 Mod 文件")?;
    let base_key = primary_base.to_lowercase();
    if physical_paths.is_empty() { return Ok(None); }
    let mut files = Vec::with_capacity(physical_paths.len());
    // A stale sibling must not hide metadata from the surviving primary file.
    for raw in physical_paths {
        let Ok(validated) = selected_paths(folder, std::slice::from_ref(raw)) else { continue; };
        for file in validated {
            let Some(filename) = file.file_name().and_then(|name| name.to_str()).map(str::to_owned) else { continue; };
            let Some((base, state)) = split_mod_name(&filename) else { continue; };
            if base.to_lowercase() != base_key { continue; }
            if files.iter().any(|existing: &ModFile| existing.path == file) { continue; }
            files.push(ModFile { path: file, filename, base, state });
        }
    }
    if !files.iter().any(|file| file.path == primary) {
        let (base, state) = split_mod_name(primary_name).ok_or("请选择有效的 Mod 文件")?;
        files.push(ModFile { path: primary.clone(), filename: primary_name.to_owned(), base, state });
    }
    let Some(inferred) = infer_mod(files, loader) else { return Ok(None); };
    if !inferred.has_metadata { return Ok(None); }
    Ok(Some(ModMeta {
        name: inferred.entry.name,
        description: inferred.entry.description,
        version: inferred.entry.version,
        tags: inferred.entry.tags,
        icon_path: inferred.entry.icon_path,
        duplicate_group: Some(inferred.key),
    }))
}

/// 按需读取单个 Mod 图标：只允许当前实例 mods 目录内的普通文件，
/// 拒绝符号链接、目录穿越与 JAR 外路径；全程只读。
fn read_mod_icon(folder: &Path, path: &Path, icon_entry: &str) -> Result<Option<String>, String> {
    let entry = icon_entry.trim();
    if entry.is_empty() || validate_relative_path(entry).is_err() { return Ok(None); }
    if !folder.exists() { return Ok(None); }
    let file = validated_mod_path(folder, path)?;
    let cancel = CancellationToken::new();
    Ok(read_icon(&file, entry, &cancel))
}

/// 图标与列表分离：列表先返回，前端只为可见条目按需请求图标。
#[tauri::command]
pub async fn version_mod_icon(
    app: AppHandle,
    dir: String,
    version_id: String,
    path: String,
    icon_entry: String,
) -> Result<Option<String>, String> {
    let root = PathBuf::from(&dir);
    let resource = runtime_settings::resource_directory(&app, &root, &version_id)?;
    let folder = instance_paths::for_instance(&root, &version_id, &resource)?;
    let task = tokio::task::spawn_blocking(move || read_mod_icon(&folder, Path::new(&path), &icon_entry));
    // 单个图标不应拖住界面；失败或超时由前端回退到通用图标。
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .map_err(|_| "读取 Mod 图标超时".to_string())?
        .map_err(|error| format!("读取 Mod 图标失败：{error}"))?
}

fn copy_new(source: &Path, destination: &Path) -> Result<(), String> {
    instances::reject_links(source)?;
    instances::reject_links(destination)?;
    let mut input = std::fs::File::open(source).map_err(|e| format!("读取 Mod 失败：{e}"))?;
    let mut output = std::fs::OpenOptions::new().create_new(true).write(true).open(destination)
        .map_err(|e| format!("目标文件已存在或不可写 {}：{e}", destination.display()))?;
    let result = std::io::copy(&mut input, &mut output).and_then(|_| output.flush()).and_then(|_| output.sync_all());
    drop(output);
    if let Err(error) = result { let _ = std::fs::remove_file(destination); return Err(format!("复制 Mod 失败：{error}")); }
    Ok(())
}

#[derive(Default)]
struct ModGroup {
    base: String,
    enabled: Vec<PathBuf>,
    disabled: Vec<PathBuf>,
    old: Vec<PathBuf>,
}

impl ModGroup {
    fn all_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.enabled
            .iter()
            .chain(self.disabled.iter())
            .chain(self.old.iter())
    }

    fn with_extension(&self, extension: &str) -> PathBuf {
        self.enabled
            .first()
            .or_else(|| self.disabled.first())
            .or_else(|| self.old.first())
            .and_then(|path| path.parent())
            .unwrap_or_else(|| Path::new(""))
            .join(format!("{}{extension}", self.base))
    }
}

fn collect_mod_group(path: &Path) -> Result<ModGroup, String> {
    let folder = path.parent().ok_or("Mod 文件夹无效")?;
    let name = path.file_name().and_then(|name| name.to_str()).ok_or("Mod 文件名无效")?;
    let (base, _) = split_mod_name(name).ok_or("请选择有效的 Mod 文件")?;
    let key = base.to_lowercase();
    instances::reject_links(folder)?;
    let mut group = ModGroup { base, ..ModGroup::default() };
    for item in std::fs::read_dir(folder).map_err(|e| format!("读取 Mod 文件夹失败：{e}"))? {
        let item = item.map_err(|e| format!("读取 Mod 文件失败：{e}"))?;
        let sibling = item.path();
        let sibling_name = item.file_name().to_string_lossy().into_owned();
        let Some((sibling_base, state)) = split_mod_name(&sibling_name) else {
            continue;
        };
        if sibling_base.to_lowercase() != key
            || !item.file_type().map_err(|e| e.to_string())?.is_file()
            || instances::reject_links(&sibling).is_err()
        {
            continue;
        }
        match state {
            ModState::Enabled => group.enabled.push(sibling),
            ModState::Disabled => group.disabled.push(sibling),
            ModState::Old => group.old.push(sibling),
        }
    }
    for paths in [&mut group.enabled, &mut group.disabled, &mut group.old] {
        paths.sort_by_key(|path| path.file_name().map(|name| name.to_ascii_lowercase()));
    }
    Ok(group)
}

fn target_present(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(format!("目标路径是符号链接，请先手动处理：{}", path.display()));
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("检查目标文件失败：{error}")),
    }
}

fn files_equal(left: &Path, right: &Path) -> Result<bool, String> {
    let left_meta = std::fs::metadata(left).map_err(|e| format!("读取 Mod 文件信息失败：{e}"))?;
    let right_meta = std::fs::metadata(right).map_err(|e| format!("读取 Mod 文件信息失败：{e}"))?;
    if left_meta.len() != right_meta.len() {
        return Ok(false);
    }
    let mut left_file = std::fs::File::open(left).map_err(|e| format!("读取 Mod 文件失败：{e}"))?;
    let mut right_file =
        std::fs::File::open(right).map_err(|e| format!("读取 Mod 文件失败：{e}"))?;
    let mut left_buffer = [0_u8; 64 * 1024];
    let mut right_buffer = [0_u8; 64 * 1024];
    loop {
        let left_count = left_file
            .read(&mut left_buffer)
            .map_err(|e| format!("读取 Mod 文件失败：{e}"))?;
        let right_count = right_file
            .read(&mut right_buffer)
            .map_err(|e| format!("读取 Mod 文件失败：{e}"))?;
        if left_count != right_count || left_buffer[..left_count] != right_buffer[..right_count] {
            return Ok(false);
        }
        if left_count == 0 {
            return Ok(true);
        }
    }
}

fn conflict_error(action: &str, group: &ModGroup) -> String {
    let files = group
        .all_paths()
        .filter_map(|path| path.file_name())
        .map(|name| name.to_string_lossy())
        .collect::<Vec<_>>()
        .join("、");
    format!(
        "无法{action} {}：同一逻辑 Mod 的副本内容不同（{files}）。请确认要保留的版本，删除其它副本后重试",
        group.base
    )
}

fn remove_equal_siblings(group: &ModGroup, keep: &Path) -> Result<(), String> {
    for path in group.all_paths() {
        if path == keep || !path.is_file() {
            continue;
        }
        if files_equal(path, keep)? {
            std::fs::remove_file(path).map_err(|error| {
                format!("清理重复 Mod 副本失败（{}）：{error}", path.display())
            })?;
        }
    }
    Ok(())
}

fn enable_group(path: &Path) -> Result<(), String> {
    let group = collect_mod_group(path)?;
    if !group.enabled.is_empty() {
        if group.enabled.len() > 1 {
            return Err(conflict_error("启用", &group));
        }
        let active = &group.enabled[0];
        let source = group
            .disabled
            .first()
            .or_else(|| group.old.first())
            .ok_or("逻辑 Mod 没有可启用的副本")?;
        if !files_equal(active, source)? {
            return Err(conflict_error("启用", &group));
        }
        std::fs::remove_file(source)
            .map_err(|error| format!("清理重复 Mod 副本失败（{}）：{error}", source.display()))?;
        return remove_equal_siblings(&group, active);
    }

    let source = group
        .disabled
        .first()
        .or_else(|| group.old.first())
        .ok_or("逻辑 Mod 没有可启用的副本")?
        .clone();
    let target = group.with_extension("");
    if target_present(&target)? {
        return Err(format!(
            "启用目标已存在但未纳入逻辑组，请刷新 Mod 列表后重试：{}",
            target.display()
        ));
    }
    std::fs::rename(&source, &target).map_err(|error| format!("切换 Mod 状态失败：{error}"))?;
    remove_equal_siblings(&group, &target)
}

fn disable_group(path: &Path) -> Result<(), String> {
    let group = collect_mod_group(path)?;
    if group.enabled.is_empty() {
        return Ok(());
    }
    if group.enabled.len() > 1 {
        return Err(conflict_error("禁用", &group));
    }
    let active = &group.enabled[0];
    if let Some(disabled) = group.disabled.first() {
        if files_equal(active, disabled)? {
            std::fs::remove_file(active)
                .map_err(|error| format!("移除重复启用副本失败：{error}"))?;
            return remove_equal_siblings(&group, disabled);
        }
        if let Some(old) = group.old.first() {
            if files_equal(active, old)? {
                std::fs::remove_file(active)
                    .map_err(|error| format!("移除重复启用副本失败：{error}"))?;
                return remove_equal_siblings(&group, disabled);
            }
            return Err(conflict_error("禁用", &group));
        }
        let target = group.with_extension(".old");
        if target_present(&target)? {
            return Err(format!(
                "禁用目标已存在但未纳入逻辑组，请刷新 Mod 列表后重试：{}",
                target.display()
            ));
        }
        return std::fs::rename(active, target)
            .map_err(|error| format!("切换 Mod 状态失败：{error}"));
    }

    let target = group.with_extension(".disabled");
    if target_present(&target)? {
        return Err(format!(
            "禁用目标已存在但未纳入逻辑组，请刷新 Mod 列表后重试：{}",
            target.display()
        ));
    }
    std::fs::rename(active, &target).map_err(|error| format!("切换 Mod 状态失败：{error}"))?;
    remove_equal_siblings(&group, &target)
}

fn rename_enabled(path: &Path, enabled: bool) -> Result<(), String> {
    let name = path.file_name().and_then(|name| name.to_str()).ok_or("Mod 文件名无效")?;
    let (_, state) = split_mod_name(name).ok_or("请选择有效的 Mod 文件")?;
    if enabled == (state == ModState::Enabled) {
        return Ok(());
    }
    if enabled {
        enable_group(path)
    } else {
        disable_group(path)
    }
}

fn install_name(name: &str) -> Option<String> {
    mod_filename(name).then(|| name.to_owned())
}

fn delete_mod_files(paths: &[PathBuf]) -> Result<usize, String> {
    let mut selected = HashSet::new();
    for path in paths {
        selected.insert(path.clone());
        let folder = path.parent().ok_or("Mod 文件夹无效")?;
        let name = path.file_name().and_then(|name| name.to_str()).ok_or("Mod 文件名无效")?;
        let (base, _) = split_mod_name(name).ok_or("请选择有效的 Mod 文件")?;
        let base = base.to_lowercase();
        instances::reject_links(folder)?;
        for item in std::fs::read_dir(folder).map_err(|e| format!("读取 Mod 文件夹失败：{e}"))? {
            let item = item.map_err(|e| format!("读取 Mod 文件失败：{e}"))?;
            let sibling = item.path();
            let sibling_name = item.file_name().to_string_lossy().into_owned();
            if item.file_type().map_err(|e| e.to_string())?.is_file()
                && split_mod_name(&sibling_name).is_some_and(|(sibling_base, _)| sibling_base.to_lowercase() == base)
                && instances::reject_links(&sibling).is_ok()
            {
                selected.insert(sibling);
            }
        }
    }
    if selected.len() > MAX_SELECTION {
        return Err("一次最多删除 1024 个 Mod 文件".into());
    }
    for path in &selected {
        instances::reject_links(path)?;
        std::fs::remove_file(path).map_err(|e| format!("删除 Mod 失败：{e}"))?;
    }
    Ok(selected.len())
}

async fn pick_mods(app: &AppHandle) -> Result<Option<Vec<PathBuf>>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title("安装本地 Mod")
        .add_filter("Mod 文件", &["jar", "zip", "litemod", "disabled", "old"])
        .pick_files(move |paths| { let _ = tx.send(paths); });
    rx.await.map_err(|_| "文件选择对话框已关闭")?.map(|paths| paths.into_iter().map(|path| path.into_path().map_err(|e| format!("请选择本地文件：{e}"))).collect()).transpose()
}

async fn pick_folder(app: &AppHandle) -> Result<Option<PathBuf>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title("复制 Mod 到文件夹").pick_folder(move |path| { let _ = tx.send(path); });
    rx.await.map_err(|_| "文件夹选择对话框已关闭")?.map(|path| path.into_path().map_err(|e| format!("请选择本地文件夹：{e}"))).transpose()
}

fn update_target(path: &Path, filename: &str) -> Result<PathBuf, String> {
    let folder = path.parent().ok_or("Mod 文件夹无效")?;
    let current = path.file_name().and_then(|name| name.to_str()).ok_or("Mod 文件名无效")?;
    let suffix = split_mod_name(current).map(|(_, state)| match state {
        ModState::Enabled => "",
        ModState::Disabled => ".disabled",
        ModState::Old => ".old",
    }).ok_or("Mod 文件名无效")?;
    let target = folder.join(format!("{filename}{suffix}"));
    if target != path && target.exists() { return Err(format!("新版本文件已存在，请先处理 {}", target.display())); }
    instances::reject_links(&target)?;
    Ok(target)
}

fn commit_mod_update(path: &Path, staged: &Path, target: &Path) -> Result<(), String> {
    let folder = path.parent().ok_or("Mod 文件夹无效")?;
    let backup = folder.join(format!(".mod-backup-{}", store::random_token()));
    instances::reject_links(path)?;
    std::fs::rename(path, &backup).map_err(|e| format!("备份原 Mod 失败：{e}"))?;
    if let Err(error) = copy_new(staged, target) {
        std::fs::rename(&backup, path).map_err(|restore| format!("{error}；恢复失败：{restore}，原文件保留于 {}", backup.display()))?;
        return Err(error);
    }
    std::fs::remove_file(&backup).map_err(|e| format!("新版已安装，清理旧 Mod 备份失败 {}：{e}", backup.display()))
}

async fn update_files(task: &TaskContext, paths: &[PathBuf], compatibility: &Compatibility, key: Option<crate::secrets::CurseforgeCredential>) -> Result<(), String> {
    let mut updated = 0;
    for path in paths {
        task.check_cancelled()?;
        task.stage("检查 Mod 更新", path.file_name().unwrap_or_default().to_string_lossy().as_ref());
        let Some(update) = fastest_update_for_path(path.clone(), compatibility, &task.network, &task.transfer, key.clone()).await else { continue; };
        let target = update_target(path, &update.filename)?;
        let filename = target.file_name().and_then(|name| name.to_str()).ok_or("Mod 文件名无效")?;
        community_network::install_resource(&task.workspace, "mods", filename, &update.url,
            FileHashes { sha1: Some(update.sha1), ..Default::default() }, task).await?;
        task.check_cancelled()?;
        commit_mod_update(path, &task.workspace.join("mods").join(filename), &target)?;
        updated += 1;
        task.progress(updated, paths.len() as u64, 0.0, 0);
    }
    if updated == 0 { return Err("所选 Mod 没有适配此游戏与加载器的更新".into()); }
    Ok(())
}

#[tauri::command]
pub async fn version_mods_action(app: AppHandle, dir: String, version_id: String, paths: Vec<String>, action: String) -> Result<Option<String>, String> {
    if !["enable", "disable", "delete", "copy", "install", "update"].contains(&action.as_str()) { return Err("未知 Mod 操作".into()); }
    let root = PathBuf::from(&dir);
    let resource = runtime_settings::resource_directory(&app, &root, &version_id)?;
    if action == "update" && instance_updates_disabled(&app, &root, &version_id)? {
        return Err("此实例已禁止更新 Mod，请先在实例高级设置中允许更新".into());
    }
    let folder = instance_paths::for_instance(&root, &version_id, &resource)?;
    let selection = if action == "install" {
        let Some(paths) = pick_mods(&app).await? else { return Ok(None); };
        if paths.len() > MAX_SELECTION { return Err("一次最多安装 1024 个 Mod".into()); }
        for path in &paths {
            instances::reject_links(path)?;
            let name = path.file_name().and_then(|name| name.to_str()).ok_or("Mod 文件名无效")?;
            if !path.is_file() || !mod_filename(name) { return Err("请选择 JAR、ZIP 或 LiteLoader Mod 文件".into()); }
        }
        paths
    } else { selected_paths(&folder, &paths)? };
    let copy_target = if action == "copy" {
        let Some(target) = pick_folder(&app).await? else { return Ok(None); };
        instances::reject_links(&target)?;
        Some(target)
    } else { None };
    if action == "update" {
        let compatibility = compatibility(&dir, &version_id)?;
        let task = app_tasks::with_idle_directory(&resource, || crate::launch_state::with_idle(&resource, || {
            Ok(())
        })).and_then(|_| app_tasks::begin(&app, &resource, None))?;
        task.describe_kind(app_tasks::TaskKind::Resource, &format!("{version_id} · Mod 更新"));
        let id = task.id.clone();
        let key = crate::secrets::curseforge_api_key(app.clone()).ok().flatten();
        tauri::async_runtime::spawn(async move {
            let result = update_files(&task, &selection, &compatibility, key).await;
            task.finish(result.map(|_| None));
        });
        return Ok(Some(id));
    }
    tokio::task::spawn_blocking(move || app_tasks::with_idle_directory(&resource, || crate::launch_state::with_idle(&resource, || {
        instances::reject_links(&folder)?;
        if action == "install" { std::fs::create_dir_all(&folder).map_err(|e| format!("创建 Mod 文件夹失败：{e}"))?; }
        if action == "delete" {
            delete_mod_files(&selection)?;
            return Ok(None);
        }
        for path in &selection {
            instances::reject_links(path)?;
            match action.as_str() {
                "enable" => rename_enabled(path, true)?,
                "disable" => rename_enabled(path, false)?,
                "install" => {
                    let name = path.file_name().and_then(|name| name.to_str()).ok_or("Mod 文件名无效")?;
                    let destination = install_name(name).ok_or("请选择有效的 Mod 文件")?;
                    copy_new(path, &folder.join(destination))?;
                },
                "copy" => copy_new(path, &copy_target.as_ref().ok_or("请选择目标目录")?.join(path.file_name().ok_or("Mod 文件名无效")?))?,
                _ => return Err("未知 Mod 操作".into()),
            }
        }
        Ok(None)
    }))).await.map_err(|e| format!("Mod 操作失败：{e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!("nickel-mods-{}", store::random_token()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn community_version_update_context_ignores_instance_name() {
        let mut detail = serde_json::json!({
            "id": "arbitrary-directory", "displayName": "Foo 26.3",
            "minecraftVersion": "1.26.3", "loader": "fabric",
        });
        assert_eq!(compatibility_from_detail(&detail).minecraft, "1.26.3");
        for unknown in [Value::Null, Value::from(""), Value::from("  "), Value::from(263)] {
            detail["minecraftVersion"] = unknown;
            assert!(compatibility_from_detail(&detail).minecraft.is_empty());
        }
    }

    #[tokio::test]
    async fn community_version_unknown_stops_updates_before_network() {
        let compatibility = Compatibility { minecraft: String::new(), loader: "fabric".into() };
        let settings = network_settings::NetworkSettings::default();
        let transfer = community_network::context(&settings);
        // 未知版本静默视为“没有可确认的更新”，不发起请求也不报错。
        assert!(modrinth_update("sha1", &compatibility, &settings, &transfer).await.unwrap().is_none());
        assert!(curseforge_update(1, &compatibility, &settings, None).await.unwrap().is_none());
    }

    fn sample_update(filename: &str) -> ModUpdate {
        ModUpdate { filename: filename.into(), url: "https://edge.forgecdn.net/a.jar".into(), sha1: "a".repeat(40) }
    }

    #[tokio::test]
    async fn race_takes_first_confirmed_source_and_falls_back_when_it_misses() {
        use std::time::Duration;

        // 两个来源都命中：先返回的 Modrinth 获胜。
        let winner = race_probes(
            async {
                tokio::time::sleep(Duration::from_millis(5)).await;
                UpdateProbe::Found(Some(sample_update("modrinth.jar")))
            },
            async {
                tokio::time::sleep(Duration::from_millis(40)).await;
                UpdateProbe::Found(Some(sample_update("curseforge.jar")))
            },
        )
        .await
        .expect("应有结果");
        assert_eq!(winner.filename, "modrinth.jar");

        // 先返回的来源查不到文件：静默等待另一来源。
        let fallback = race_probes(
            async {
                tokio::time::sleep(Duration::from_millis(5)).await;
                UpdateProbe::Unknown
            },
            async {
                tokio::time::sleep(Duration::from_millis(30)).await;
                UpdateProbe::Found(Some(sample_update("curseforge.jar")))
            },
        )
        .await
        .expect("应回退到另一来源");
        assert_eq!(fallback.filename, "curseforge.jar");

        // 先返回的来源确认文件身份且没有新版本：这是确定结果，立即结束。
        assert!(race_probes(
            async {
                tokio::time::sleep(Duration::from_millis(5)).await;
                UpdateProbe::Found(None)
            },
            async {
                tokio::time::sleep(Duration::from_millis(40)).await;
                UpdateProbe::Found(Some(sample_update("late.jar")))
            },
        )
        .await
        .is_none());

        // 两个来源都失败：静默视为没有更新。
        assert!(race_probes(
            async { UpdateProbe::Unknown },
            async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                UpdateProbe::Unknown
            },
        )
        .await
        .is_none());
    }

    #[test]
    fn enable_disable_preserves_file_and_rejects_collision() {
        let root = fixture();
        let source = root.join("sample.jar");
        std::fs::write(&source, b"original").unwrap();
        rename_enabled(&source, false).unwrap();
        assert!(!source.exists());
        let disabled = root.join("sample.jar.disabled");
        std::fs::write(&source, b"different").unwrap();
        assert!(rename_enabled(&disabled, true).is_err());
        assert_eq!(std::fs::read(&disabled).unwrap(), b"original");
        std::fs::remove_file(&source).unwrap();
        rename_enabled(&disabled, true).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), b"original");
        std::fs::remove_dir_all(root).unwrap();
    }

    fn write_mod(path: &Path, id: &str, name: &str, version: &str, categories: &[&str]) {
        use minecraft_java_rs_core::utils::archive::{write_zip_files, ArchiveLimits};
        let metadata = serde_json::json!({
            "id": id, "name": name, "version": version,
            "description": format!("{name} description"), "categories": categories,
        });
        write_zip_files(
            path,
            path.parent().unwrap(),
            &[],
            &[("fabric.mod.json".into(), serde_json::to_vec(&metadata).unwrap())],
            &ArchiveLimits::default(),
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .unwrap();
    }

    #[test]
    fn logical_group_toggle_cleans_identical_disabled_and_old_copies() {
        let root = fixture();
        let mods = root.join("mods");
        std::fs::create_dir(&mods).unwrap();
        let enabled = mods.join("foo.jar");
        let disabled = mods.join("foo.jar.disabled");
        let old = mods.join("foo.jar.old");
        for path in [&enabled, &disabled, &old] {
            std::fs::write(path, b"same mod").unwrap();
        }

        rename_enabled(&enabled, false).unwrap();
        assert!(!enabled.exists());
        assert_eq!(std::fs::read(&disabled).unwrap(), b"same mod");
        assert!(!old.exists());
        let entries = scan_local(&mods).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].enabled);
        assert_eq!(entries[0].physical_paths.len(), 1);

        rename_enabled(&disabled, true).unwrap();
        assert_eq!(std::fs::read(&enabled).unwrap(), b"same mod");
        assert!(!disabled.exists());
        assert!(!old.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn logical_group_enable_from_old_copy_restores_single_active_mod() {
        let root = fixture();
        let mods = root.join("mods");
        std::fs::create_dir(&mods).unwrap();
        let old = mods.join("legacy.litemod.old");
        let enabled = mods.join("legacy.litemod");
        std::fs::write(&old, b"legacy mod").unwrap();

        let entries = scan_local(&mods).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].enabled);
        rename_enabled(&old, true).unwrap();
        assert_eq!(std::fs::read(&enabled).unwrap(), b"legacy mod");
        assert!(!old.exists());
        let entries = scan_local(&mods).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].enabled);
        assert_eq!(entries[0].physical_paths.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn logical_group_toggle_preserves_different_copies_without_overwrite() {
        let root = fixture();
        let mods = root.join("mods");
        std::fs::create_dir(&mods).unwrap();
        let enabled = mods.join("foo.jar");
        let disabled = mods.join("foo.jar.disabled");
        std::fs::write(&enabled, b"active copy").unwrap();
        std::fs::write(&disabled, b"disabled copy").unwrap();

        rename_enabled(&enabled, false).unwrap();
        let old = mods.join("foo.jar.old");
        assert!(!enabled.exists());
        assert_eq!(std::fs::read(&disabled).unwrap(), b"disabled copy");
        assert_eq!(std::fs::read(&old).unwrap(), b"active copy");
        let entries = scan_local(&mods).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].enabled);
        assert_eq!(entries[0].physical_paths.len(), 2);

        rename_enabled(&disabled, true).unwrap();
        assert_eq!(std::fs::read(&enabled).unwrap(), b"disabled copy");
        assert!(!disabled.exists());
        assert_eq!(std::fs::read(&old).unwrap(), b"active copy");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn logical_group_toggle_rejects_three_distinct_copies_without_mutation() {
        let root = fixture();
        let mods = root.join("mods");
        std::fs::create_dir(&mods).unwrap();
        let enabled = mods.join("foo.jar");
        let disabled = mods.join("foo.jar.disabled");
        let old = mods.join("foo.jar.old");
        std::fs::write(&enabled, b"active").unwrap();
        std::fs::write(&disabled, b"disabled").unwrap();
        std::fs::write(&old, b"old").unwrap();

        let error = rename_enabled(&enabled, false).unwrap_err();
        assert!(error.contains("内容不同"), "{error}");
        assert_eq!(std::fs::read(&enabled).unwrap(), b"active");
        assert_eq!(std::fs::read(&disabled).unwrap(), b"disabled");
        assert_eq!(std::fs::read(&old).unwrap(), b"old");

        let error = rename_enabled(&disabled, true).unwrap_err();
        assert!(error.contains("内容不同"), "{error}");
        assert_eq!(std::fs::read(&enabled).unwrap(), b"active");
        assert_eq!(std::fs::read(&disabled).unwrap(), b"disabled");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn supported_mod_names_require_known_container_and_state_suffix() {
        for name in [
            "a.jar", "a.JAR", "a.zip", "a.litemod", "a.jar.disabled",
            "a.zip.disabled", "a.litemod.old", "a.JAR.OLD",
        ] {
            assert!(mod_filename(name), "{name}");
        }
        for name in ["a.txt", "a.jar.old.bak", "a.disable", "disabled", ".jar"] {
            assert!(!mod_filename(name), "{name}");
        }
        assert_eq!(install_name("a.jar.old").as_deref(), Some("a.jar.old"));
        assert_eq!(install_name("a.zip.disabled").as_deref(), Some("a.zip.disabled"));
        assert_eq!(install_name("a.jar").as_deref(), Some("a.jar"));
    }

    #[test]
    fn disabled_and_old_only_groups_keep_state_and_prefer_disabled_copy() {
        let root = fixture();
        let mods = root.join("mods");
        std::fs::create_dir(&mods).unwrap();
        write_mod(&mods.join("toggle.jar.disabled"), "toggle", "Toggle", "1.0.0", &[]);
        write_mod(&mods.join("toggle.jar.old"), "toggle", "Toggle", "1.0.0", &[]);
        std::fs::write(mods.join("legacy.litemod.old"), b"legacy").unwrap();

        let entries = scan_local(&mods).unwrap();
        assert_eq!(entries.len(), 2);
        let toggle = entries.iter().find(|entry| entry.name == "Toggle").unwrap();
        assert!(!toggle.enabled);
        assert_eq!(toggle.physical_paths.len(), 2);
        assert!(toggle.path.ends_with("toggle.jar.disabled"));
        let legacy = entries.iter().find(|entry| entry.name == "legacy.litemod").unwrap();
        assert!(!legacy.enabled);
        assert!(legacy.path.ends_with("legacy.litemod.old"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_removes_all_physical_siblings_without_touching_unrelated_mods() {
        let root = fixture();
        let mods = root.join("mods");
        std::fs::create_dir(&mods).unwrap();
        let enabled = mods.join("foo.jar");
        let disabled = mods.join("foo.jar.disabled");
        let old = mods.join("foo.jar.old");
        let unrelated = mods.join("bar.jar");
        for path in [&enabled, &disabled, &old, &unrelated] {
            std::fs::write(path, b"fixture").unwrap();
        }

        let removed = delete_mod_files(&[enabled.clone(), disabled.clone()]).unwrap();
        assert_eq!(removed, 3);
        assert!(!enabled.exists() && !disabled.exists() && !old.exists());
        assert!(unrelated.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scan_pairs_physical_states_and_marks_metadata_duplicates() {
        let root = fixture();
        let mods = root.join("mods");
        std::fs::create_dir(&mods).unwrap();
        write_mod(&mods.join("foo.jar"), "foo", "Foo", "1.0.0", &["Utility"]);
        write_mod(&mods.join("foo.jar.disabled"), "foo", "Foo", "1.0.0", &["Utility"]);
        write_mod(&mods.join("extra-1.jar"), "extra", "Extra", "1.0.0", &["Technology"]);
        write_mod(&mods.join("extra-2.jar"), "extra", "Extra", "2.0.0", &["Magic"]);
        std::fs::write(mods.join("lite.litemod"), b"legacy fixture").unwrap();

        let entries = scan_local(&mods).unwrap();
        assert_eq!(entries.len(), 4);
        let foo = entries.iter().find(|entry| entry.name == "Foo").unwrap();
        assert_eq!(foo.physical_paths.len(), 2);
        assert!(foo.enabled);
        assert!(foo.path.ends_with("foo.jar"));
        assert!(foo.physical_paths.iter().any(|path| path.ends_with("foo.jar.disabled")));
        assert_eq!(foo.tags, vec!["Utility"]);

        let duplicates: Vec<_> = entries.iter().filter(|entry| entry.name == "Extra").collect();
        assert_eq!(duplicates.len(), 2);
        assert_eq!(duplicates[0].duplicate_group, duplicates[1].duplicate_group);
        assert!(duplicates[0].duplicate_group.as_deref().unwrap().starts_with("id:extra"));
        let groups: HashSet<_> = entries.iter().filter_map(|entry| entry.duplicate_group.as_deref()).collect();
        assert_eq!(groups.len(), 1);
        assert!(entries.iter().any(|entry| entry.name == "lite.litemod" && entry.enabled));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_entries_only_include_instance_mod_paths() {
        let root = fixture();
        let mods = root.join("mods");
        std::fs::create_dir(&mods).unwrap();
        let inside = mods.join("inside.jar");
        let outside = root.join("outside.jar");
        std::fs::write(&inside, b"inside").unwrap();
        std::fs::write(&outside, b"outside").unwrap();

        let entries = validated_update_entries(
            &mods,
            &[
                inside.to_string_lossy().into_owned(),
                outside.to_string_lossy().into_owned(),
                "relative.jar".into(),
                inside.to_string_lossy().into_owned(),
            ],
        );
        // 越界 / 相对 / 重复路径均被跳过，只保留实例 mods 内的真实文件。
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, inside.to_string_lossy().into_owned());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dedupe_key_folds_case_only_on_windows() {
        let mixed = "C:/Games/Mods/Example.jar";
        let lower = "c:/games/mods/example.jar";
        if cfg!(windows) {
            // Windows 下同一文件的不同大小写必须折叠成一个键。
            assert_eq!(dedupe_key(mixed), dedupe_key(lower));
            assert_eq!(dedupe_key(mixed), lower);
        } else {
            // 其它平台大小写敏感，不能把两个真实文件误合并。
            assert_ne!(dedupe_key(mixed), dedupe_key(lower));
            assert_eq!(dedupe_key(mixed), mixed);
        }
    }

    #[test]
    fn operations_reject_files_outside_mods_and_copy_overwrite() {
        let root = fixture();
        let mods = root.join("mods");
        std::fs::create_dir(&mods).unwrap();
        let outside = root.join("outside.jar");
        std::fs::write(&outside, b"outside").unwrap();
        assert!(selected_paths(&mods, &[outside.to_string_lossy().into()]).is_err());
        let target = mods.join("target.jar");
        copy_new(&outside, &target).unwrap();
        assert!(copy_new(&outside, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"outside");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_requires_matching_game_loader_newer_date_and_valid_hash() {
        let current = serde_json::json!({"id":"old","project_id":"project","date_published":"2026-01-01"});
        let compatibility = Compatibility { minecraft: "1.20.1".into(), loader: "fabric".into() };
        let mut update = serde_json::json!({"id":"new","project_id":"project","date_published":"2026-02-01","game_versions":["1.20.1"],"loaders":["fabric"],"files":[{"filename":"new.jar","url":"https://cdn.modrinth.com/data/project/versions/new/new.jar","hashes":{"sha1":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}]});
        assert!(select_update(&current, &[update.clone()], "oldhash", &compatibility).unwrap().is_some());
        update["loaders"] = serde_json::json!(["forge"]);
        assert!(select_update(&current, &[update], "oldhash", &compatibility).unwrap().is_none());
    }

    #[test]
    fn disabled_updates_respect_inheritance_and_explicit_false() {
        use serde_json::json;
        let global = json!({"disableModUpdates":true});
        assert!(updates_disabled(&global, &json!({})));
        assert!(updates_disabled(&global, &json!({"disableModUpdates":null})));
        assert!(!updates_disabled(&global, &json!({"disableModUpdates":false})));
        assert!(updates_disabled(&json!({"disableModUpdates":false}), &json!({"disableModUpdates":true})));
        assert!(!updates_disabled(&json!({}), &json!({})));
    }

    #[test]
    fn disabled_update_replaces_bytes_preserves_state_and_rolls_back_on_failure() {
        let root = fixture();
        let old = root.join("old.jar.disabled");
        let staged = root.join("staged.jar");
        std::fs::write(&old, b"old mod").unwrap();
        std::fs::write(&staged, b"new mod").unwrap();
        let target = update_target(&old, "new.jar").unwrap();
        assert_eq!(target, root.join("new.jar.disabled"));
        let legacy = root.join("legacy.jar.old");
        std::fs::write(&legacy, b"legacy").unwrap();
        assert_eq!(update_target(&legacy, "newer.jar").unwrap(), root.join("newer.jar.old"));
        commit_mod_update(&old, &staged, &target).unwrap();
        assert!(!old.exists());
        assert!(!root.join("new.jar").exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"new mod");
        assert!(!read_mod(target.clone()).enabled);
        assert!(commit_mod_update(&target, &root.join("missing.jar"), &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"new mod");
        commit_mod_update(&target, &staged, &target).unwrap();
        std::fs::write(&old, b"collision").unwrap();
        assert!(update_target(&target, "old.jar").is_err());
        assert_eq!(std::fs::read(&old).unwrap(), b"collision");
        std::fs::remove_dir_all(root).unwrap();
    }

    fn write_mod_with_icon(path: &Path) {
        use minecraft_java_rs_core::utils::archive::{write_zip_files, ArchiveLimits};
        let metadata = serde_json::json!({
            "id": "icon-mod", "name": "Icon Mod", "version": "1.0.0",
            "icon": "assets/icon.png",
        });
        write_zip_files(
            path,
            path.parent().unwrap(),
            &[],
            &[
                ("fabric.mod.json".into(), serde_json::to_vec(&metadata).unwrap()),
                ("assets/icon.png".into(), b"\x89PNG\r\n\x1a\nfixture".to_vec()),
            ],
            &ArchiveLimits::default(),
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .unwrap();
    }

    #[test]
    fn icon_metadata_only_accepts_safe_archive_paths() {
        for (data, expected) in [
            (serde_json::json!({"icon": "assets/icon.png"}), Some("assets/icon.png")),
            (serde_json::json!({"icon": {"1x": "pack.png"}}), Some("pack.png")),
            (serde_json::json!({"logoFile": "logo.png"}), Some("logo.png")),
        ] {
            assert_eq!(mod_icon_entry(&data).as_deref(), expected);
        }
        for bad in ["", "  ", "../icon.png", "/icon.png", "a\\b.png", "C:icon.png", "a//b.png", "a/./b.png"] {
            assert!(mod_icon_entry(&serde_json::json!({"icon": bad})).is_none(), "{bad}");
        }
        assert!(mod_icon_entry(&serde_json::json!({})).is_none());
    }

    #[test]
    fn list_omits_icon_bytes_and_icon_ipc_rejects_outside_paths() {
        let root = fixture();
        let mods = root.join("mods");
        std::fs::create_dir(&mods).unwrap();
        let jar = mods.join("icon-mod.jar");
        write_mod_with_icon(&jar);

        let entries = scan_local(&mods).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].icon_path.as_deref(), Some("assets/icon.png"));
        let list = serde_json::to_string(&entries).unwrap();
        assert!(!list.contains("data:image"), "列表不应内嵌图标字节");

        let icon = read_mod_icon(&mods, &jar, "assets/icon.png").unwrap();
        assert!(icon.is_some_and(|value| value.starts_with("data:image/png;base64,")));
        // JAR 内越界条目只返回空图标，不尝试读取。
        assert!(read_mod_icon(&mods, &jar, "../fabric.mod.json").unwrap().is_none());

        let outside = root.join("outside.jar");
        std::fs::write(&outside, b"outside").unwrap();
        assert!(read_mod_icon(&mods, &outside, "assets/icon.png").is_err());
        assert!(read_mod_icon(&mods, Path::new("relative.jar"), "assets/icon.png").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn display_name_strips_container_suffix_and_keeps_middle_dots() {
        for (raw, expected) in [
            ("foo-1.2.jar", "foo-1.2"),
            ("Foo.Bar.JAR", "Foo.Bar"),
            ("lite.litemod", "lite"),
            ("a.zip", "a"),
        ] {
            assert_eq!(display_mod_name(raw), expected, "{raw}");
        }
    }

    #[test]
    fn list_uses_filename_placeholder_and_path_order_without_opening_jars() {
        let root = fixture();
        let mods = root.join("mods");
        std::fs::create_dir(&mods).unwrap();
        // 刻意不是合法 ZIP：首屏列表不应打开 JAR，仍能立即返回文件名占位。
        std::fs::write(mods.join("zeta-1.2.jar"), b"not a zip").unwrap();
        std::fs::write(mods.join("alpha.mod.jar.disabled"), b"not a zip").unwrap();
        write_mod(&mods.join("mid.jar"), "mid", "Mid Display", "2.0.0", &["Tech"]);
        write_mod(&mods.join("mid.jar.disabled"), "mid", "Mid Display", "2.0.0", &["Tech"]);

        let entries = scan_local_list(&mods).unwrap();
        assert_eq!(entries.len(), 3);

        let zeta = entries.iter().find(|entry| entry.path.ends_with("zeta-1.2.jar")).unwrap();
        assert_eq!(zeta.name, "zeta-1.2");
        assert_eq!(zeta.description, "zeta-1.2");
        assert_eq!(zeta.version, "");
        assert!(zeta.icon_path.is_none());
        assert!(zeta.tags.is_empty());
        assert!(!zeta.update_available);

        let alpha = entries.iter().find(|entry| entry.path.ends_with("alpha.mod.jar.disabled")).unwrap();
        assert_eq!(alpha.name, "alpha.mod");
        assert_eq!(alpha.description, "alpha.mod");
        assert!(!alpha.enabled);

        let mid = entries.iter().find(|entry| entry.path.ends_with("mid.jar")).unwrap();
        assert_eq!(mid.name, "mid");
        assert!(mid.enabled);
        assert_eq!(mid.physical_paths.len(), 2);
        // 同一 base 的启用/禁用副本是一个逻辑 Mod，占位阶段不能标成重复。
        assert!(mid.duplicate_group.is_none());
        // 元数据中的展示名不应出现在首屏占位里。
        assert!(!entries.iter().any(|entry| entry.name == "Mid Display"));

        let paths: Vec<_> = entries.iter().map(|entry| entry.path.to_lowercase()).collect();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn meta_reads_real_metadata_and_rejects_paths_outside_instance_mods() {
        let root = fixture();
        let mods = root.join("mods");
        std::fs::create_dir(&mods).unwrap();
        write_mod(&mods.join("foo.jar"), "foo", "Foo Display", "1.2.3", &["Magic", "Utility"]);
        write_mod(&mods.join("foo.jar.disabled"), "foo", "Foo Display", "1.2.3", &["Magic", "Utility"]);
        let list = scan_local_list(&mods).unwrap();
        let foo = list.iter().find(|entry| entry.path.ends_with("foo.jar")).unwrap();

        let meta = read_mod_meta(&mods, Path::new(&foo.path), &foo.physical_paths, None)
            .unwrap()
            .expect("必须读到清单");
        assert_eq!(meta.name, "Foo Display");
        assert_eq!(meta.version, "1.2.3");
        assert_eq!(meta.description, "Foo Display description");
        assert_eq!(meta.tags, vec!["Magic", "Utility"]);
        assert!(meta.duplicate_group.as_deref().unwrap().starts_with("id:foo"));

        assert!(read_mod_meta(&mods, Path::new(&foo.path), &[], None).unwrap().is_none());

        // 列表快照中的 .disabled 副本消失后，主文件仍应返回真实元数据。
        std::fs::remove_file(mods.join("foo.jar.disabled")).unwrap();
        let stale = read_mod_meta(&mods, Path::new(&foo.path), &foo.physical_paths, None)
            .unwrap()
            .expect("失效副本不应拖垮主文件元数据");
        assert_eq!(stale.name, "Foo Display");

        let outside = root.join("outside.jar");
        write_mod(&outside, "outside", "Outside", "1.0.0", &[]);
        assert!(read_mod_meta(&mods, &outside, &[], None).is_err());
        assert!(read_mod_meta(&mods, Path::new("relative.jar"), &[], None).is_err());
        std::fs::write(mods.join("notes.txt"), b"x").unwrap();
        assert!(read_mod_meta(&mods, &mods.join("notes.txt"), &[], None).is_err());
        std::fs::write(mods.join("plain.jar"), b"plain").unwrap();
        assert!(read_mod_meta(&mods, &mods.join("plain.jar"), &[mods.join("plain.jar").to_string_lossy().into_owned()], None).unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn murmur2_matches_curseforge_reference_values() {
        let hash = |text: &str| {
            let mut hasher = Murmur2::new();
            hasher.update(text.as_bytes());
            hasher.finish()
        };
        assert_eq!(hash(""), 0x5bd1_5e36);
        assert_eq!(hash("a"), 0xee3a_b957);
        assert_eq!(hash("abc"), 0x42dd_4e3d);
        assert_eq!(hash("test"), 0xedd3_6d9c);
        assert_eq!(hash("The quick brown fox jumps over the lazy dog"), 0x827c_56d8);
        // CurseForge 在哈希前跳过空白；换行与空格不应改变结果。
        assert_eq!(hash("hello world"), hash("hello\nworld"));
    }

    #[test]
    fn curseforge_update_requires_newer_file_with_download_and_sha1() {
        let current = serde_json::json!({ "id": 10, "fileDate": "2026-01-01T00:00:00Z", "modId": 7 });
        let newer = serde_json::json!({
            "id": 11,
            "fileDate": "2026-02-01T00:00:00Z",
            "fileName": "example-2.0.jar",
            "downloadUrl": "https://edge.forgecdn.net/files/1/1/example-2.0.jar",
            "hashes": [{ "algo": 1, "value": "a".repeat(40) }],
        });
        let older = serde_json::json!({
            "id": 12,
            "fileDate": "2025-12-01T00:00:00Z",
            "fileName": "example-0.9.jar",
            "downloadUrl": "https://edge.forgecdn.net/files/1/1/example-0.9.jar",
            "hashes": [{ "algo": 1, "value": "b".repeat(40) }],
        });
        let update = select_curseforge_update(&current, &[older, newer.clone()]).expect("应识别更新");
        assert_eq!(update.filename, "example-2.0.jar");
        assert_eq!(update.sha1, "a".repeat(40));
        assert!(select_curseforge_update(&current, &[newer.clone()]).is_some());

        let mut missing_url = newer.clone();
        missing_url["downloadUrl"] = Value::Null;
        assert!(select_curseforge_update(&current, &[missing_url]).is_none());
        let mut unsafe_name = newer;
        unsafe_name["fileName"] = Value::from("../escape.jar");
        assert!(select_curseforge_update(&current, &[unsafe_name]).is_none());
    }

    /// 真实 Mod 目录只读基准：`NICKEL_MODS_SCAN_DIR` 指向实例 mods 文件夹。
    /// 不把本机路径写进仓库；默认跳过，避免 CI 依赖外部目录。
    #[test]
    #[ignore = "需要 NICKEL_MODS_SCAN_DIR 指向真实 Mod 目录"]
    fn bench_real_mods_scan() {
        let Some(folder) = std::env::var_os("NICKEL_MODS_SCAN_DIR") else {
            eprintln!("[bench] 未设置 NICKEL_MODS_SCAN_DIR，跳过真实扫描基准");
            return;
        };
        let folder = PathBuf::from(folder);
        if !folder.is_dir() {
            eprintln!("[bench] 目录不存在，跳过：{}", folder.display());
            return;
        }
        // 首屏只枚举目录；这里同时记录 10 条元数据与一次完整扫描，作为渐进加载证据。
        let list_started = std::time::Instant::now();
        let entries = scan_local_list(&folder).expect("真实 Mod 目录列表失败");
        let list_elapsed = list_started.elapsed();
        let meta_started = std::time::Instant::now();
        let mut loaded = 0;
        for entry in entries.iter().take(10) {
            let meta = read_mod_meta(&folder, Path::new(&entry.path), &entry.physical_paths, None)
                .unwrap_or(None);
            if meta.is_some() { loaded += 1; }
        }
        let meta_elapsed = meta_started.elapsed();
        // 前端元数据队列并发为 4；这里用同样并发度测量 10 条的真实收尾时间。
        let parallel_started = std::time::Instant::now();
        let first_ten = &entries[..entries.len().min(10)];
        let chunk_size = first_ten.len().div_ceil(4).max(1);
        let parallel_loaded = std::thread::scope(|scope| {
            let folder = folder.as_path();
            let handles: Vec<_> = first_ten
                .chunks(chunk_size)
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk
                            .iter()
                            .filter(|entry| {
                                read_mod_meta(
                                    folder,
                                    Path::new(&entry.path),
                                    &entry.physical_paths,
                                    None,
                                )
                                .unwrap_or(None)
                                .is_some()
                            })
                            .count()
                    })
                })
                .collect();
            handles.into_iter().map(|handle| handle.join().unwrap_or(0)).sum::<usize>()
        });
        let parallel_elapsed = parallel_started.elapsed();
        let full_started = std::time::Instant::now();
        let full = scan_local(&folder).expect("真实 Mod 目录扫描失败");
        let full_elapsed = full_started.elapsed();
        let with_icon = full.iter().filter(|entry| entry.icon_path.is_some()).count();
        eprintln!(
            "[bench] list mods={} elapsed={:.3}s; meta10-seq loaded={} elapsed={:.3}s; meta10-parallel4 loaded={} elapsed={:.3}s; full mods={} withIconEntry={} elapsed={:.3}s",
            entries.len(),
            list_elapsed.as_secs_f64(),
            loaded,
            meta_elapsed.as_secs_f64(),
            parallel_loaded,
            parallel_elapsed.as_secs_f64(),
            full.len(),
            with_icon,
            full_elapsed.as_secs_f64()
        );
    }

}
