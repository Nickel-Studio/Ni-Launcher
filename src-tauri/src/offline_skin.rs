//! 离线皮肤以资源包覆盖默认玩家纹理；不读取、修改或重新生成账号 UUID。
//! 只能接收 launcher 已解析的真实绝对 runtime_directory / client JAR / 原版版本号。
//! random 是零网络清理路径；资源包和用户自有文件不删除，仅撤销本工具的启用项。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use minecraft_java_rs_core::net::transfer::TransferContext;
use minecraft_java_rs_core::utils::image::{
    ALEX_TEXTURE, MAX_SKIN_PNG_BYTES, MODERN_ALEX_TEXTURE, MODERN_STEVE_TEXTURE, STEVE_TEXTURE,
    SkinTextureTarget, adapt_skin_png, decode_skin_png, encode_skin_png, read_client_skin_assets,
    read_skin_png_from_jar,
};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::{http, store};

const PACK_FOLDER: &str = "Nickel-offline-skin";
const PACK_ID: &str = "file/Nickel-offline-skin";
const OWNER_FILE: &str = ".nickel-owner";
const OWNER_CONTENT: &[u8] = b"Nickel offline skin pack v1\n";
const SOURCE_FILE: &str = ".nickel-source.png";
const SOURCE_RECORD: &str = ".nickel-source.json";
const OPTIONS_FILE: &str = "options.txt";
const MAX_OPTIONS_BYTES: usize = 2 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
struct VersionSupport {
    pack_format: u32,
    pack_minor: Option<u32>,
    legacy_skin: bool,
    prefixed_pack_id: bool,
    textures: Vec<SkinTextureTarget>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceRecord {
    mode: String,
    name: String,
    #[serde(default)]
    source_slim: bool,
}

const MOJANG_LOOKUP: &str = "https://api.mojang.com/users/profiles/minecraft";
const MOJANG_PROFILE: &str = "https://sessionserver.mojang.com/session/minecraft/profile";
const TEXTURE_BASE: &str = "https://textures.minecraft.net/texture/";
const MAX_TEXTURE_PROPERTY_BYTES: usize = 16 * 1024;

// 仅为没有内嵌 version.json 的旧客户端保留已知格式；现代版本以 JAR 元数据为准。
fn support_for_version(version: &str) -> Result<VersionSupport, String> {
    let unsupported = || {
        format!(
            "客户端 JAR 缺少资源包版本信息，无法为 Minecraft {version} 生成离线皮肤资源包。请重新安装原版客户端，或在游戏设置将离线皮肤改为“随机”"
        )
    };
    let parts: Vec<&str> = version.split('.').collect();
    if !(2..=3).contains(&parts.len())
        || parts[0] != "1"
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err(unsupported());
    }
    let minor = parts[1].parse::<u32>().map_err(|_| unsupported())?;
    let patch = parts
        .get(2)
        .unwrap_or(&"0")
        .parse::<u32>()
        .map_err(|_| unsupported())?;
    let max_patch = match minor {
        6 => 4,
        7 => 10,
        8 => 9,
        9 => 4,
        10..=13 => 2,
        14 => 4,
        15 => 2,
        16 => 5,
        17 => 1,
        18 | 19 => 2,
        _ => return Err(unsupported()),
    };
    if patch > max_patch {
        return Err(unsupported());
    }
    let pack_format = match minor {
        6..=8 => 1,
        9..=10 => 2,
        11..=12 => 3,
        13..=14 => 4,
        15 => 5,
        16 if patch <= 1 => 5,
        16 => 6,
        17 => 7,
        18 => 8,
        19 => 9,
        _ => return Err(unsupported()),
    };
    let textures = [
        SkinTextureTarget {
            path: STEVE_TEXTURE.into(),
            slim: false,
        },
        SkinTextureTarget {
            path: ALEX_TEXTURE.into(),
            slim: true,
        },
    ]
    .into_iter()
    .filter(|target| !target.slim || minor >= 8)
    .collect();
    Ok(VersionSupport {
        pack_format,
        pack_minor: None,
        legacy_skin: minor < 8,
        prefixed_pack_id: minor >= 13,
        textures,
    })
}

async fn support_from_client(client_jar: &Path, version: &str) -> Result<VersionSupport, String> {
    if !client_jar.is_absolute() || !client_jar.is_file() {
        return Err("未找到已安装的客户端 JAR，请重新安装原版客户端或将离线皮肤改为“随机”".into());
    }
    let path = client_jar.to_path_buf();
    let cancel = TransferContext::current().cancellation().clone();
    let assets = tokio::task::spawn_blocking(move || read_client_skin_assets(&path, &cancel))
        .await
        .map_err(|e| format!("读取客户端资源信息任务中断，请重试：{e}"))?
        .map_err(|e| {
            format!("无法读取客户端皮肤资源：{e}。请重新安装原版客户端或将离线皮肤改为“随机”")
        })?;
    let mut support = if let Some(format) = assets.pack_format {
        VersionSupport {
            pack_format: format.major,
            pack_minor: format.minor,
            legacy_skin: false,
            prefixed_pack_id: true,
            textures: Vec::new(),
        }
    } else {
        support_for_version(version)?
    };
    support.textures = assets.targets;
    Ok(support)
}

fn pack_metadata(support: &VersionSupport) -> serde_json::Value {
    let mut pack = serde_json::json!({
        "pack_format": support.pack_format,
        "description": "Nickel 离线皮肤"
    });
    if let Some(minor) = support.pack_minor {
        // 25w31a 起元数据采用主/次版本，pack.mcmeta 同步写入实际兼容区间。
        pack["min_format"] = serde_json::json!([support.pack_format, minor]);
        pack["max_format"] = serde_json::json!([support.pack_format, minor]);
    }
    serde_json::json!({ "pack": pack })
}

fn is_our_pack(name: &str) -> bool {
    name == PACK_FOLDER || name == PACK_ID
}

/// 只重写两个资源包列表；保留其它行、BOM、CRLF、未知项与用户原有顺序。
fn patch_options(text: &str, enable: Option<&str>) -> Result<String, String> {
    let mut result = String::with_capacity(text.len() + PACK_ID.len() + 32);
    let mut saw_resources = false;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let (body, ending) = if let Some(body) = line.strip_suffix("\r\n") {
            (body, "\r\n")
        } else if let Some(body) = line.strip_suffix('\n') {
            (body, "\n")
        } else {
            (line, "")
        };
        let (bom, body) = if index == 0 && body.starts_with('\u{feff}') {
            ("\u{feff}", &body[3..])
        } else {
            ("", body)
        };
        let Some((key, value)) = body.split_once(':') else {
            result.push_str(line);
            continue;
        };
        if key != "resourcePacks" && key != "incompatibleResourcePacks" {
            result.push_str(line);
            continue;
        }
        if key == "resourcePacks" {
            saw_resources = true;
        }
        let original = match serde_json::from_str::<Vec<String>>(value) {
            Ok(list) => list,
            Err(_) if enable.is_none() && !value.contains(PACK_FOLDER) => {
                result.push_str(line);
                continue;
            }
            Err(_) => {
                return Err(format!(
                    "options.txt 的 {key} 列表无法解析。请先备份该文件，在游戏内重新保存资源包设置后重试"
                ));
            }
        };
        let mut list: Vec<String> = original
            .iter()
            .filter(|name| !is_our_pack(name))
            .cloned()
            .collect();
        if key == "resourcePacks" {
            if let Some(name) = enable {
                list.push(name.to_string());
            }
        }
        if list == original {
            result.push_str(line);
            continue;
        }
        result.push_str(bom);
        result.push_str(key);
        result.push(':');
        result.push_str(
            &serde_json::to_string(&list)
                .map_err(|e| format!("保存资源包列表失败，请重试：{e}"))?,
        );
        result.push_str(ending);
    }
    if !saw_resources {
        if let Some(name) = enable {
            let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
            if !result.is_empty() && !result.ends_with('\n') {
                result.push_str(ending);
            }
            result.push_str("resourcePacks:");
            result.push_str(
                &serde_json::to_string(&[name])
                    .map_err(|e| format!("保存资源包列表失败，请重试：{e}"))?,
            );
            result.push_str(ending);
        }
    }
    Ok(result)
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file = File::open(path)
        .map_err(|e| format!("无法读取 {}，请检查文件位置与读取权限：{e}", path.display()))?;
    if !file
        .metadata()
        .map_err(|e| format!("无法读取文件信息，请重新选择文件：{e}"))?
        .is_file()
    {
        return Err(format!(
            "{} 不是普通文件，请重新选择 PNG 图片",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("无法读取 {}，请检查读取权限：{e}", path.display()))?;
    if bytes.len() > limit {
        return Err(format!("{} 超过允许大小，请缩小文件后重试", path.display()));
    }
    Ok(bytes)
}

fn reparse_point(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn safe_entry(path: &Path, directory: bool) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(meta)
            if reparse_point(&meta)
                || (directory && !meta.is_dir())
                || (!directory && !meta.is_file()) =>
        {
            Err(format!(
                "{} 不是可写入的普通{}。请移动同名链接或文件后重试",
                path.display(),
                if directory { "目录" } else { "文件" }
            ))
        }
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("无法检查 {}，请检查目录权限：{e}", path.display())),
    }
}

fn ensure_directory(path: &Path) -> Result<(), String> {
    if !safe_entry(path, true)? {
        fs::create_dir(path)
            .map_err(|e| format!("无法创建 {}，请检查目录写入权限：{e}", path.display()))?;
    }
    Ok(())
}

/// 不使用失败后直接截断原文件的降级；临时文件写完并同步后才替换。
fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    safe_entry(path, false)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "文件名无效，请重新选择游戏目录".to_string())?;
    let temp = path.with_file_name(format!(".{name}.nickel-{}", store::random_token()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| format!("无法创建临时文件，请检查目录写入权限：{e}"))?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("无法写入 {}，请检查磁盘空间：{e}", path.display()))?;
        drop(file);
        fs::rename(&temp, path).map_err(|e| {
            format!(
                "无法替换 {}，请退出游戏并检查文件权限后重试：{e}",
                path.display()
            )
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn runtime_path(runtime_directory: &Path) -> Result<PathBuf, String> {
    if !runtime_directory.is_absolute() {
        return Err("游戏运行目录不是绝对路径，请重新选择实例目录后重试".into());
    }
    if !runtime_directory.is_dir() {
        return Err(format!(
            "游戏运行目录不存在：{}。请重新选择实例目录后重试",
            runtime_directory.display()
        ));
    }
    fs::canonicalize(runtime_directory)
        .map_err(|e| format!("无法解析游戏运行目录，请检查目录权限：{e}"))
}

fn read_options(runtime_directory: &Path) -> Result<Option<Vec<u8>>, String> {
    let path = runtime_directory.join(OPTIONS_FILE);
    if !safe_entry(&path, false)? {
        return Ok(None);
    }
    read_bounded(&path, MAX_OPTIONS_BYTES).map(Some)
}

fn clear_pack(runtime_directory: &Path) -> Result<(), String> {
    // 没有本工具所有权标记时不碰 options；普通 random 启动无需图片/JAR/版本前提。
    if !runtime_directory.is_absolute() {
        return Err("游戏运行目录不是绝对路径，请重新选择实例目录后重试".into());
    }
    if !runtime_directory.exists() {
        return Ok(());
    }
    let runtime = runtime_path(runtime_directory)?;
    let _guard = store::lock();
    match owned_pack(&runtime) {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => return Ok(()),
    }
    let Some(bytes) = read_options(&runtime)? else {
        return Ok(());
    };
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(_)
            if !bytes
                .windows(PACK_FOLDER.len())
                .any(|part| part == PACK_FOLDER.as_bytes()) =>
        {
            return Ok(());
        }
        Err(_) => {
            return Err(
                "options.txt 不是 UTF-8 文本，请备份后在游戏内重新保存设置再切换为随机皮肤".into(),
            );
        }
    };
    let updated = patch_options(text, None)?;
    if updated != text {
        if read_options(&runtime)?.as_deref() != Some(bytes.as_slice()) {
            return Err("游戏设置正在变化，请退出游戏后重新启动实例".into());
        }
        replace_file(&runtime.join(OPTIONS_FILE), updated.as_bytes())?;
    }
    Ok(())
}

fn owned_pack(runtime: &Path) -> Result<Option<PathBuf>, String> {
    let resourcepacks = runtime.join("resourcepacks");
    if !safe_entry(&resourcepacks, true)? {
        return Ok(None);
    }
    let pack = resourcepacks.join(PACK_FOLDER);
    if !safe_entry(&pack, true)? {
        return Ok(None);
    }
    let owner = pack.join(OWNER_FILE);
    if !safe_entry(&owner, false)? || read_bounded(&owner, MAX_RECORD_BYTES)? != OWNER_CONTENT {
        return Err(format!(
            "资源包目录 {PACK_FOLDER} 缺少有效的 Nickel 管理标记。请给该目录改名后重试"
        ));
    }
    Ok(Some(pack))
}

async fn default_texture(client_jar: &Path, slim: bool) -> Result<Vec<u8>, String> {
    if !client_jar.is_absolute() || !client_jar.is_file() {
        return Err("未找到已安装的客户端 JAR，请先完成该版本安装后重试".into());
    }
    let path = client_jar.to_path_buf();
    let cancel = TransferContext::current().cancellation().clone();
    tokio::task::spawn_blocking(move || {
        let assets = read_client_skin_assets(&path, &cancel)?;
        let candidates = if slim { [MODERN_ALEX_TEXTURE, ALEX_TEXTURE] } else { [MODERN_STEVE_TEXTURE, STEVE_TEXTURE] };
        let texture = candidates.into_iter().find(|candidate| assets.targets.iter().any(|target| target.path == *candidate))
            .ok_or_else(|| "客户端没有所选默认皮肤纹理".to_string())?;
        read_skin_png_from_jar(&path, texture, &cancel)
    })
        .await
        .map_err(|e| format!("读取客户端皮肤任务中断，请重新启动实例：{e}"))?
        .map_err(|e| if slim {
            format!("无法读取客户端 Alex 皮肤：{e}。请选择 Steve 或自定义图片，或重新安装包含 Alex 的原版客户端后重试")
        } else {
            format!("无法读取客户端 Steve 皮肤：{e}。请重新安装原版客户端后重试")
        })
}

fn cached_official(runtime: &Path, name: &str) -> Result<Option<(Vec<u8>, bool)>, String> {
    let Some(pack) = owned_pack(runtime)? else {
        return Ok(None);
    };
    let record_path = pack.join(SOURCE_RECORD);
    if !safe_entry(&record_path, false)? {
        return Ok(None);
    }
    let record: SourceRecord =
        serde_json::from_slice(&read_bounded(&record_path, MAX_RECORD_BYTES)?)
            .map_err(|e| format!("离线皮肤缓存记录损坏，请移走资源包 {PACK_FOLDER} 后重试：{e}"))?;
    if record.mode != "official" || !record.name.eq_ignore_ascii_case(name) {
        return Ok(None);
    }
    let source_path = pack.join(SOURCE_FILE);
    if !safe_entry(&source_path, false)? {
        return Ok(None);
    }
    let bytes = read_bounded(&source_path, MAX_SKIN_PNG_BYTES)?;
    decode_skin_png(&bytes)?;
    Ok(Some((bytes, record.source_slim)))
}

fn official_texture_info(profile: &serde_json::Value) -> Result<(String, bool), String> {
    let invalid = || "正版玩家没有有效皮肤信息，请检查玩家名后重试或选择本地 PNG 图片".to_string();
    let encoded = profile
        .get("properties")
        .and_then(serde_json::Value::as_array)
        .and_then(|properties| {
            properties
                .iter()
                .find(|p| p.get("name").and_then(serde_json::Value::as_str) == Some("textures"))
        })
        .and_then(|property| property.get("value"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid)?;
    if encoded.len() > MAX_TEXTURE_PROPERTY_BYTES {
        return Err(invalid());
    }
    let bytes = STANDARD.decode(encoded).map_err(|_| invalid())?;
    let textures: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let skin = textures.pointer("/textures/SKIN").ok_or_else(invalid)?;
    let url = skin
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid)?;
    // 只接受官方纹理域名并升级 HTTPS；不允许 profile 响应把请求导向本机或其它站点。
    let id = url
        .strip_prefix(TEXTURE_BASE)
        .or_else(|| url.strip_prefix("http://textures.minecraft.net/texture/"))
        .ok_or_else(invalid)?;
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let slim = match skin
        .pointer("/metadata/model")
        .and_then(serde_json::Value::as_str)
    {
        None | Some("classic") => false,
        Some("slim") => true,
        Some(_) => return Err(invalid()),
    };
    Ok((format!("{TEXTURE_BASE}{id}"), slim))
}

async fn official_texture(runtime: &Path, name: &str) -> Result<(Vec<u8>, bool), String> {
    if name.is_empty()
        || name.len() > 16
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("请输入 1–16 位英文字母、数字或下划线组成的正版玩家名".into());
    }
    if let Some(cached) = cached_official(runtime, name)? {
        return Ok(cached);
    }
    let request_error = |error| {
        format!("获取正版玩家 {name} 的皮肤失败：{error}。请检查网络后重试，或选择本地 PNG 图片")
    };
    // skin.rs 的公开接口不返回模型；复用同一 http 能力并读取官方模型元数据，不猜测纤细布局。
    let (status, value) = http::get_json(&format!("{MOJANG_LOOKUP}/{name}"), None)
        .await
        .map_err(request_error)?;
    if status != 200 {
        return Err(format!(
            "未找到正版玩家 {name}（HTTP {status}），请检查玩家名后重试"
        ));
    }
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| "正版玩家信息无效，请稍后重试或选择本地 PNG 图片".to_string())?;
    let (status, profile) = http::get_json(&format!("{MOJANG_PROFILE}/{id}"), None)
        .await
        .map_err(request_error)?;
    if status != 200 {
        return Err(format!(
            "读取正版皮肤档案失败（HTTP {status}），请稍后重试或选择本地 PNG 图片"
        ));
    }
    let (url, slim) = official_texture_info(&profile)?;
    let (status, _, bytes) = http::get_bytes(&url).await.map_err(request_error)?;
    if status != 200 {
        return Err(format!(
            "下载正版皮肤失败（HTTP {status}），请稍后重试或选择本地 PNG 图片"
        ));
    }
    decode_skin_png(&bytes)?;
    Ok((bytes, slim))
}

fn install_pack(
    runtime: &Path,
    support: VersionSupport,
    source: &[u8],
    source_slim: bool,
    record: SourceRecord,
) -> Result<(), String> {
    let steve = adapt_skin_png(source, source_slim, false, support.legacy_skin)?;
    let alex = if support.textures.iter().any(|target| target.slim) {
        Some(adapt_skin_png(source, source_slim, true, false)?)
    } else {
        None
    };
    let metadata = serde_json::to_vec_pretty(&pack_metadata(&support))
        .map_err(|e| format!("生成离线皮肤资源包失败，请重试：{e}"))?;
    let record =
        serde_json::to_vec(&record).map_err(|e| format!("保存皮肤来源失败，请重试：{e}"))?;
    let _guard = store::lock();
    let before = read_options(runtime)?;
    let text = std::str::from_utf8(before.as_deref().unwrap_or_default())
        .map_err(|_| "options.txt 不是 UTF-8 文本，请先备份并在游戏内重新保存设置".to_string())?;
    let pack_id = if support.prefixed_pack_id {
        PACK_ID
    } else {
        PACK_FOLDER
    };
    let updated = patch_options(text, Some(pack_id))?;
    let existing = owned_pack(runtime)?;
    let resourcepacks = runtime.join("resourcepacks");
    ensure_directory(&resourcepacks)?;
    let pack = match existing {
        Some(pack) => pack,
        None => {
            let pack = resourcepacks.join(PACK_FOLDER);
            fs::create_dir(&pack)
                .map_err(|e| format!("无法创建离线皮肤资源包，请检查目录写入权限：{e}"))?;
            replace_file(&pack.join(OWNER_FILE), OWNER_CONTENT)?;
            pack
        }
    };
    // 覆盖 JAR 实际存在的全部默认纹理；UUID 的选择算法与几何模型保持原样。
    for target in &support.textures {
        let png = if target.slim {
            alex.as_deref()
                .ok_or_else(|| "缺少纤细模型纹理，请重新选择皮肤".to_string())?
        } else {
            steve.as_slice()
        };
        let mut directory = pack.clone();
        let (parent, _) = target
            .path
            .rsplit_once('/')
            .ok_or_else(|| "客户端皮肤路径无效".to_string())?;
        for part in parent.split('/') {
            directory.push(part);
            ensure_directory(&directory)?;
        }
        replace_file(&pack.join(&target.path), png)?;
    }
    replace_file(&pack.join("pack.mcmeta"), &metadata)?;
    replace_file(&pack.join(SOURCE_FILE), source)?;
    replace_file(&pack.join(SOURCE_RECORD), &record)?;
    // 网络和图片转换结束后才读取 options；提交前再检查，避免覆盖游戏同时保存的选项。
    if read_options(runtime)? != before {
        return Err("游戏设置在准备皮肤期间发生变化。请退出游戏后重新启动实例".into());
    }
    if updated != text {
        replace_file(&runtime.join(OPTIONS_FILE), updated.as_bytes())?;
    }
    Ok(())
}

/// account_type 为 offline 时使用所选设置；其它账号传入 random 以撤销旧离线包。
/// client_jar 和 minecraft_version 必须来自已解析的原版继承链，不能使用显示名称猜测。
/// 此接口不接收账号或 UUID，不能通过皮肤选择修改存档身份。
pub async fn prepare(
    runtime_directory: &Path,
    client_jar: &Path,
    minecraft_version: &str,
    mode: &str,
    name: Option<&str>,
    path: Option<&str>,
    slim: bool,
) -> Result<(), String> {
    if mode == "random" {
        return clear_pack(runtime_directory);
    }
    if !matches!(mode, "steve" | "alex" | "official" | "custom") {
        return Err("离线皮肤类型无效，请在游戏设置中重新选择皮肤".into());
    }
    let runtime = runtime_path(runtime_directory)?;
    owned_pack(&runtime)?;
    let support = support_from_client(client_jar, minecraft_version).await?;
    let name = name.unwrap_or_default().trim();
    let (source, source_slim) = match mode {
        "steve" => (default_texture(client_jar, false).await?, false),
        "alex" => (default_texture(client_jar, true).await?, true),
        "official" => official_texture(&runtime, name).await?,
        "custom" => {
            let path = path
                .filter(|p| !p.trim().is_empty())
                .ok_or_else(|| "请先在游戏设置中选择本地 PNG 皮肤".to_string())?;
            let path = Path::new(path);
            if !path.is_absolute() {
                return Err("皮肤路径不是绝对路径，请重新选择本地 PNG 皮肤".into());
            }
            (read_bounded(path, MAX_SKIN_PNG_BYTES)?, slim)
        }
        _ => return Err("请重新选择离线皮肤".into()),
    };
    install_pack(
        &runtime,
        support,
        &source,
        source_slim,
        SourceRecord {
            mode: mode.to_string(),
            name: name.to_string(),
            source_slim,
        },
    )
}

fn persist_selected_png(source: &Path, directory: &Path) -> Result<PathBuf, String> {
    let bytes = read_bounded(source, MAX_SKIN_PNG_BYTES)?;
    let image = decode_skin_png(&bytes)?;
    let png = encode_skin_png(&image)?;
    fs::create_dir_all(directory)
        .map_err(|e| format!("无法创建皮肤保存目录，请检查配置目录写入权限：{e}"))?;
    let path = directory.join(format!("skin-{}.png", store::random_token()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|e| format!("无法保存皮肤，请检查配置目录写入权限：{e}"))?;
    if let Err(error) = file.write_all(&png).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(format!("无法保存皮肤，请检查磁盘空间后重新选择：{error}"));
    }
    Ok(path)
}

/// 取消返回 null；成功仅返回已复制到应用配置目录的绝对 PNG 路径。
#[tauri::command]
pub async fn game_offline_skin_pick(app: AppHandle) -> Result<Option<String>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择离线皮肤")
        .add_filter("PNG 皮肤", &["png"])
        .pick_file(move |file| {
            let _ = sender.send(file);
        });
    let Some(selected) = receiver
        .await
        .map_err(|_| "文件选择器已关闭，请重新选择皮肤".to_string())?
    else {
        return Ok(None);
    };
    let source = selected
        .into_path()
        .map_err(|_| "无法读取所选位置，请将 PNG 保存到本地后重新选择".to_string())?;
    let directory = crate::app_paths::config_dir(&app)?.join("offline-skins");
    let path = tokio::task::spawn_blocking(move || persist_selected_png(&source, &directory))
        .await
        .map_err(|e| format!("保存皮肤任务中断，请重新选择：{e}"))??;
    path.to_str()
        .map(|path| Some(path.to_string()))
        .ok_or_else(|| "皮肤保存路径包含无法识别的字符，请调整配置目录后重试".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("nickel-skin-test-{}", store::random_token()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    // 由 Node 内置 zlib 独立生成的 64×64 RGBA PNG，不依赖被测编码器。
    fn sample_png() -> Vec<u8> {
        STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAYAAACqaXHeAAAAmElEQVR4nO3QMREAIBDAsNeIIKwhDWRkoEP2Xmftc382OkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAO0B9EriWYmR+28AAAAASUVORK5CYII=").unwrap()
    }

    fn client_jar_fixture(
        directory: &Path,
        pack_version: Option<serde_json::Value>,
        modern: bool,
    ) -> PathBuf {
        use minecraft_java_rs_core::net::transfer::CancellationToken;
        use minecraft_java_rs_core::utils::archive::{ArchiveLimits, write_zip_files};
        let path = directory.join("client.jar");
        let mut entries = Vec::new();
        if let Some(pack_version) = pack_version {
            entries.push((
                "version.json".into(),
                serde_json::to_vec(&serde_json::json!({ "pack_version": pack_version })).unwrap(),
            ));
        }
        if modern {
            for model in ["wide", "slim"] {
                for name in [
                    "alex", "ari", "efe", "kai", "makena", "noor", "steve", "sunny", "zuri",
                ] {
                    entries.push((
                        format!("assets/minecraft/textures/entity/player/{model}/{name}.png"),
                        sample_png(),
                    ));
                }
            }
        } else {
            entries.push((STEVE_TEXTURE.into(), sample_png()));
            entries.push((ALEX_TEXTURE.into(), sample_png()));
        }
        write_zip_files(
            &path,
            directory,
            &[],
            &entries,
            &ArchiveLimits::default(),
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .unwrap();
        path
    }

    #[tokio::test]
    async fn modern_metadata_and_all_default_models_support_each_skin_mode() {
        for (version, pack_version, major, minor) in [
            (
                "1.19.3",
                serde_json::json!({"resource":12,"data":10}),
                12,
                None,
            ),
            (
                "1.20.1",
                serde_json::json!({"resource":15,"data":15}),
                15,
                None,
            ),
            (
                "1.21.9",
                serde_json::json!({"resource_major":69,"resource_minor":0}),
                69,
                Some(0),
            ),
            (
                "25w31a",
                serde_json::json!({"resource_major":65,"resource_minor":0}),
                65,
                Some(0),
            ),
        ] {
            let directory = TestDirectory::new();
            let jar = client_jar_fixture(&directory.0, Some(pack_version), true);
            let support = support_from_client(&jar, version).await.unwrap();
            assert_eq!(support.pack_format, major);
            assert_eq!(support.pack_minor, minor);
            assert_eq!(support.textures.len(), 18);
            assert_eq!(
                support.textures.iter().filter(|target| target.slim).count(),
                9
            );
            let source = directory.0.join("custom.png");
            fs::write(&source, sample_png()).unwrap();
            let options = "gamma:0.75\nresourcePacks:[\"file/user.zip\"]\ncustom:keep\n";
            fs::write(directory.0.join(OPTIONS_FILE), options).unwrap();
            fs::create_dir_all(directory.0.join("saves")).unwrap();
            fs::write(
                directory.0.join("saves").join("identity.dat"),
                b"existing UUID identity",
            )
            .unwrap();
            let pack = directory.0.join("resourcepacks").join(PACK_FOLDER);
            for mode in ["steve", "alex", "custom", "official"] {
                if mode == "official" {
                    // 命中真实缓存读取路径，测试不依赖 Mojang 服务或外网。
                    fs::write(
                        pack.join(SOURCE_RECORD),
                        serde_json::to_vec(&SourceRecord {
                            mode: "official".into(),
                            name: "ExistingPlayer".into(),
                            source_slim: true,
                        })
                        .unwrap(),
                    )
                    .unwrap();
                }
                prepare(
                    &directory.0,
                    &jar,
                    version,
                    mode,
                    Some("ExistingPlayer"),
                    source.to_str(),
                    true,
                )
                .await
                .unwrap();
                for target in &support.textures {
                    let expected =
                        adapt_skin_png(&sample_png(), mode != "steve", target.slim, false).unwrap();
                    assert_eq!(fs::read(pack.join(&target.path)).unwrap(), expected);
                }
                let metadata: serde_json::Value =
                    serde_json::from_slice(&fs::read(pack.join("pack.mcmeta")).unwrap()).unwrap();
                assert_eq!(metadata["pack"]["pack_format"], major);
                if let Some(minor) = minor {
                    assert_eq!(
                        metadata["pack"]["min_format"],
                        serde_json::json!([major, minor])
                    );
                    assert_eq!(
                        metadata["pack"]["max_format"],
                        serde_json::json!([major, minor])
                    );
                } else {
                    assert!(metadata["pack"].get("min_format").is_none());
                }
            }
            prepare(
                &directory.0,
                &directory.0.join("absent.jar"),
                "unknown",
                "random",
                None,
                None,
                false,
            )
            .await
            .unwrap();
            assert_eq!(
                fs::read_to_string(directory.0.join(OPTIONS_FILE)).unwrap(),
                options
            );
            assert_eq!(
                fs::read(directory.0.join("saves").join("identity.dat")).unwrap(),
                b"existing UUID identity"
            );
            assert!(pack.join(MODERN_STEVE_TEXTURE).is_file());
        }
    }

    #[tokio::test]
    async fn installed_client_jar_returns_complete_png_and_rejects_corruption_size_and_missing_texture()
     {
        // ZIP Stored 向量独立于核心库 ZIP 写入器，两个完整 PNG 条目总计 758 字节。
        let jar = STANDARD.decode("UEsDBBQAAAAAAAAAAAAjQVJB0QAAANEAAAAqAAAAYXNzZXRzL21pbmVjcmFmdC90ZXh0dXJlcy9lbnRpdHkvc3RldmUucG5niVBORw0KGgoAAAANSUhEUgAAAEAAAABACAYAAACqaXHeAAAAmElEQVR4nO3QMREAIBDAsNeIIKwhDWRkoEP2Xmftc382OkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAK0BOkBrgA7QGqADtAboAO0B9EriWYmR+28AAAAASUVORK5CYIJQSwMEFAAAAAAAAAAAACNBUkHRAAAA0QAAACkAAABhc3NldHMvbWluZWNyYWZ0L3RleHR1cmVzL2VudGl0eS9hbGV4LnBuZ4lQTkcNChoKAAAADUlIRFIAAABAAAAAQAgGAAAAqmlx3gAAAJhJREFUeJzt0DERACAQwLDXiCCsIQ1kZKBD9l5n7XN/NjpAa4AO0BqgA7QG6ACtATpAa4AO0BqgA7QG6ACtATpAa4AO0BqgA7QG6ACtATpAa4AO0BqgA7QG6ACtATpAa4AO0BqgA7QG6ACtATpAa4AO0BqgA7QG6ACtATpAa4AO0BqgA7QG6ACtATpAa4AO0BqgA7QG6ADtAfRK4lmJkftvAAAAAElFTkSuQmCCUEsBAhQAFAAAAAAAAAAAACNBUkHRAAAA0QAAACoAAAAAAAAAAAAAAAAAAAAAAGFzc2V0cy9taW5lY3JhZnQvdGV4dHVyZXMvZW50aXR5L3N0ZXZlLnBuZ1BLAQIUABQAAAAAAAAAAAAjQVJB0QAAANEAAAApAAAAAAAAAAAAAAAAABkBAABhc3NldHMvbWluZWNyYWZ0L3RleHR1cmVzL2VudGl0eS9hbGV4LnBuZ1BLBQYAAAAAAgACAK8AAAAxAgAAAAA=").unwrap();
        let directory = TestDirectory::new();
        let path = directory.0.join("client.jar");
        fs::write(&path, &jar).unwrap();
        for slim in [false, true] {
            let actual = default_texture(&path, slim).await.unwrap();
            assert_eq!(actual, sample_png());
            assert_eq!(decode_skin_png(&actual).unwrap().height(), 64);
        }
        let mut corrupt = jar.clone();
        corrupt[30 + STEVE_TEXTURE.len()] ^= 1;
        fs::write(&path, corrupt).unwrap();
        assert!(
            default_texture(&path, false)
                .await
                .unwrap_err()
                .contains("重新安装")
        );
        let mut oversize = jar.clone();
        let central = jar
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .unwrap();
        oversize[central + 24..central + 28]
            .copy_from_slice(&((MAX_SKIN_PNG_BYTES + 1) as u32).to_le_bytes());
        fs::write(&path, oversize).unwrap();
        assert!(
            default_texture(&path, false)
                .await
                .unwrap_err()
                .contains("大小限制")
        );
        let mut missing = jar.clone();
        for index in jar
            .windows(ALEX_TEXTURE.len())
            .enumerate()
            .filter_map(|(i, bytes)| (bytes == ALEX_TEXTURE.as_bytes()).then_some(i))
        {
            missing[index..index + ALEX_TEXTURE.len()]
                .copy_from_slice(b"assets/minecraft/textures/entity/none.png");
        }
        fs::write(&path, missing).unwrap();
        assert!(
            default_texture(&path, true)
                .await
                .unwrap_err()
                .contains("请选择 Steve")
        );
    }

    #[tokio::test]
    async fn missing_metadata_is_actionable_and_changes_no_files() {
        let directory = TestDirectory::new();
        let jar = client_jar_fixture(&directory.0, None, true);
        let options = "resourcePacks:[\"file/user.zip\"]\n";
        fs::write(directory.0.join(OPTIONS_FILE), options).unwrap();
        let error = prepare(&directory.0, &jar, "1.19.3", "custom", None, None, false)
            .await
            .unwrap_err();
        assert!(error.contains("改为“随机”"));
        assert!(error.contains("缺少资源包版本信息"));
        assert_eq!(
            fs::read_to_string(directory.0.join(OPTIONS_FILE)).unwrap(),
            options
        );
        assert!(!directory.0.join("resourcepacks").exists());
    }

    #[test]
    fn valid_pick_is_persisted_and_survives_source_removal() {
        let directory = TestDirectory::new();
        let source = directory.0.join("source.png");
        fs::write(&source, sample_png()).unwrap();
        let cache = directory.0.join("offline-skins");
        let persisted = persist_selected_png(&source, &cache).unwrap();
        assert!(persisted.is_absolute());
        assert!(persisted.starts_with(&cache));
        fs::remove_file(&source).unwrap();
        assert_eq!(
            decode_skin_png(&fs::read(persisted).unwrap())
                .unwrap()
                .height(),
            64
        );
    }

    #[tokio::test]
    async fn custom_install_and_random_roundtrip_do_not_change_identity_or_user_files() {
        let directory = TestDirectory::new();
        let jar = client_jar_fixture(
            &directory.0,
            Some(serde_json::json!({"resource":9,"data":10})),
            false,
        );
        let source = directory.0.join("source.png");
        fs::write(&source, sample_png()).unwrap();
        let options = "gamma:0.75\nresourcePacks:[\"file/user.zip\"]\ncustom:keep\n";
        fs::write(directory.0.join(OPTIONS_FILE), options).unwrap();
        let user_pack = directory.0.join("resourcepacks").join("user.zip");
        fs::create_dir_all(user_pack.parent().unwrap()).unwrap();
        fs::write(&user_pack, b"user pack bytes").unwrap();
        let account = store::StoredAccount {
            name: "ExistingPlayer".into(),
            ..Default::default()
        }
        .normalize();
        let identity = account.uuid.clone();
        prepare(
            &directory.0,
            &jar,
            "1.19.2",
            "custom",
            None,
            source.to_str(),
            true,
        )
        .await
        .unwrap();
        assert_eq!(account.uuid, identity);
        let pack = directory.0.join("resourcepacks").join(PACK_FOLDER);
        assert_eq!(fs::read(pack.join(OWNER_FILE)).unwrap(), OWNER_CONTENT);
        assert_eq!(
            decode_skin_png(&fs::read(pack.join(STEVE_TEXTURE)).unwrap())
                .unwrap()
                .height(),
            64
        );
        assert_eq!(
            decode_skin_png(&fs::read(pack.join(ALEX_TEXTURE)).unwrap())
                .unwrap()
                .height(),
            64
        );
        assert!(
            fs::read_to_string(directory.0.join(OPTIONS_FILE))
                .unwrap()
                .contains(PACK_ID)
        );
        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(pack.join("pack.mcmeta")).unwrap()).unwrap();
        assert_eq!(metadata["pack"]["pack_format"], 9);
        fs::write(pack.join("user-note.txt"), b"keep").unwrap();
        prepare(
            &directory.0,
            &directory.0.join("unused.jar"),
            "future",
            "random",
            None,
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(account.uuid, identity);
        assert_eq!(
            fs::read_to_string(directory.0.join(OPTIONS_FILE)).unwrap(),
            options
        );
        assert_eq!(fs::read(user_pack).unwrap(), b"user pack bytes");
        assert_eq!(fs::read(pack.join("user-note.txt")).unwrap(), b"keep");
    }

    #[test]
    fn invalid_skin_leaves_options_unchanged() {
        let directory = TestDirectory::new();
        let options = "resourcePacks:[\"file/user.zip\"]\n";
        fs::write(directory.0.join(OPTIONS_FILE), options).unwrap();
        let error = install_pack(
            &directory.0,
            support_for_version("1.19.2").unwrap(),
            b"bad",
            false,
            SourceRecord {
                mode: "custom".into(),
                name: String::new(),
                source_slim: false,
            },
        )
        .unwrap_err();
        assert!(error.contains("重新导出"));
        assert_eq!(
            fs::read_to_string(directory.0.join(OPTIONS_FILE)).unwrap(),
            options
        );
        assert!(!directory.0.join("resourcepacks").exists());
    }

    #[test]
    fn official_cache_is_name_scoped_and_png_validated() {
        let directory = TestDirectory::new();
        install_pack(
            &directory.0,
            support_for_version("1.19.2").unwrap(),
            &sample_png(),
            true,
            SourceRecord {
                mode: "official".into(),
                name: "ExistingPlayer".into(),
                source_slim: true,
            },
        )
        .unwrap();
        assert!(
            cached_official(&directory.0, "existingplayer")
                .unwrap()
                .unwrap()
                .1
        );
        assert!(
            cached_official(&directory.0, "OtherPlayer")
                .unwrap()
                .is_none()
        );
        fs::write(
            directory
                .0
                .join("resourcepacks")
                .join(PACK_FOLDER)
                .join(SOURCE_FILE),
            b"broken",
        )
        .unwrap();
        assert!(cached_official(&directory.0, "ExistingPlayer").is_err());
    }

    #[test]
    fn official_profile_preserves_model_and_restricts_texture_origin() {
        let profile = |url: &str, model: &str| {
            let texture =
                serde_json::json!({"textures":{"SKIN":{"url":url,"metadata":{"model":model}}}});
            serde_json::json!({"properties":[{"name":"textures","value": STANDARD.encode(serde_json::to_vec(&texture).unwrap())}]})
        };
        let result = official_texture_info(&profile(
            "http://textures.minecraft.net/texture/abcdef",
            "slim",
        ))
        .unwrap();
        assert_eq!(result, (format!("{TEXTURE_BASE}abcdef"), true));
        for url in [
            "http://127.0.0.1/secret",
            "https://textures.minecraft.net.evil/texture/abc",
            "https://textures.minecraft.net/texture/abc?token=secret",
        ] {
            assert!(official_texture_info(&profile(url, "classic")).is_err());
        }
        assert!(
            official_texture_info(&profile(
                "https://textures.minecraft.net/texture/abc",
                "unknown"
            ))
            .is_err()
        );
    }

    #[test]
    fn version_boundaries_and_pack_formats() {
        for (version, format) in [
            ("1.6", 1),
            ("1.7.10", 1),
            ("1.8.9", 1),
            ("1.9", 2),
            ("1.10.2", 2),
            ("1.11", 3),
            ("1.12.2", 3),
            ("1.13", 4),
            ("1.14.4", 4),
            ("1.15.2", 5),
            ("1.16.1", 5),
            ("1.16.2", 6),
            ("1.17.1", 7),
            ("1.18.2", 8),
            ("1.19.2", 9),
        ] {
            assert_eq!(support_for_version(version).unwrap().pack_format, format);
        }
        for version in [
            "1.5.2",
            "1.19.3",
            "1.20",
            "1.6.99",
            "23w01a",
            "1.19.2-forge",
            "1.19.2.1",
            "1.19-pre1",
            "",
        ] {
            assert!(
                support_for_version(version)
                    .unwrap_err()
                    .contains("改为“随机”")
            );
        }
        assert!(support_for_version("1.7.10").unwrap().legacy_skin);
        assert!(!support_for_version("1.8").unwrap().legacy_skin);
        assert!(!support_for_version("1.12.2").unwrap().prefixed_pack_id);
        assert!(support_for_version("1.13").unwrap().prefixed_pack_id);
    }

    #[test]
    fn options_preserve_unknown_fields_user_pack_order_and_line_endings() {
        let original = "\u{feff}gamma:0.75\r\nresourcePacks:[\"vanilla\",\"file/用户包.zip\",\"file/Nickel-offline-skin\"]\r\nincompatibleResourcePacks:[\"file/old.zip\",\"Nickel-offline-skin\"]\r\nkey_key.forward:87";
        let updated = patch_options(original, Some(PACK_ID)).unwrap();
        assert!(updated.starts_with("\u{feff}gamma:0.75\r\n"));
        assert!(updated.contains(
            "resourcePacks:[\"vanilla\",\"file/用户包.zip\",\"file/Nickel-offline-skin\"]\r\n"
        ));
        assert!(updated.contains("incompatibleResourcePacks:[\"file/old.zip\"]\r\n"));
        assert!(updated.ends_with("key_key.forward:87"));
        assert_eq!(patch_options(&updated, Some(PACK_ID)).unwrap(), updated);
        let cleared = patch_options(&updated, None).unwrap();
        assert!(cleared.contains("resourcePacks:[\"vanilla\",\"file/用户包.zip\"]\r\n"));
        assert_eq!(patch_options(&cleared, None).unwrap(), cleared);
    }

    #[test]
    fn random_only_removes_exact_owned_pack_names() {
        let original = "resourcePacks:[\"Nickel-offline-skin\",\"file/Nickel-offline-skin\",\"file/Nickel-offline-skin-user.zip\",\"file/mine.zip\"]\n";
        assert_eq!(
            patch_options(original, None).unwrap(),
            "resourcePacks:[\"file/Nickel-offline-skin-user.zip\",\"file/mine.zip\"]\n"
        );
        assert_eq!(
            patch_options("broken:still:here\n", None).unwrap(),
            "broken:still:here\n"
        );
        assert_eq!(
            patch_options("resourcePacks:broken\n", None).unwrap(),
            "resourcePacks:broken\n"
        );
        assert!(patch_options("resourcePacks:broken\n", Some(PACK_ID)).is_err());
        assert!(patch_options("resourcePacks:[Nickel-offline-skin]\n", None).is_err());
        assert_eq!(patch_options("", None).unwrap(), "");
    }

    #[test]
    fn activation_adds_missing_list_and_deduplicates_only_tool_pack() {
        let initial = "gamma:1";
        assert_eq!(
            patch_options(initial, Some(PACK_FOLDER)).unwrap(),
            format!("gamma:1\nresourcePacks:[\"{PACK_FOLDER}\"]\n")
        );
        let duplicate =
            format!("resourcePacks:[\"mine\",\"{PACK_FOLDER}\",\"mine\",\"{PACK_ID}\"]\n");
        assert_eq!(
            patch_options(&duplicate, Some(PACK_ID)).unwrap(),
            format!("resourcePacks:[\"mine\",\"mine\",\"{PACK_ID}\"]\n")
        );
    }

    #[tokio::test]
    async fn random_has_no_jar_version_network_or_uuid_precondition() {
        let directory = TestDirectory::new();
        let name = "ExistingPlayer";
        let uuid = minecraft_java_rs_core::utils::auth::offline_uuid(name);
        let original_uuid = uuid.clone();
        prepare(
            &directory.0,
            &directory.0.join("not-installed.jar"),
            "future-snapshot",
            "random",
            Some("invalid / username"),
            Some("missing.png"),
            true,
        )
        .await
        .unwrap();
        assert_eq!(uuid, original_uuid);
        assert_eq!(
            minecraft_java_rs_core::utils::auth::offline_uuid(name),
            original_uuid
        );
        assert!(!directory.0.join(OPTIONS_FILE).exists());
        assert!(!directory.0.join("resourcepacks").exists());
    }

    #[test]
    fn random_preserves_files_and_only_cleans_owned_options() {
        let directory = TestDirectory::new();
        let pack = directory.0.join("resourcepacks").join(PACK_FOLDER);
        fs::create_dir_all(&pack).unwrap();
        fs::write(pack.join("keep-user-note.txt"), b"keep").unwrap();
        let options = format!("gamma:0.5\nresourcePacks:[\"file/user.zip\",\"{PACK_ID}\"]\n");
        fs::write(directory.0.join(OPTIONS_FILE), &options).unwrap();
        clear_pack(&directory.0).unwrap();
        assert_eq!(
            fs::read_to_string(directory.0.join(OPTIONS_FILE)).unwrap(),
            options
        );
        fs::write(pack.join(OWNER_FILE), OWNER_CONTENT).unwrap();
        clear_pack(&directory.0).unwrap();
        assert_eq!(fs::read(pack.join("keep-user-note.txt")).unwrap(), b"keep");
        assert_eq!(
            fs::read_to_string(directory.0.join(OPTIONS_FILE)).unwrap(),
            "gamma:0.5\nresourcePacks:[\"file/user.zip\"]\n"
        );
    }

    #[test]
    fn collision_does_not_claim_user_resourcepack() {
        let directory = TestDirectory::new();
        let pack = directory.0.join("resourcepacks").join(PACK_FOLDER);
        fs::create_dir_all(&pack).unwrap();
        fs::write(pack.join("user.txt"), b"keep").unwrap();
        assert!(owned_pack(&directory.0).unwrap_err().contains("改名"));
        assert_eq!(fs::read(pack.join("user.txt")).unwrap(), b"keep");
        assert!(!pack.join(OWNER_FILE).exists());
    }

    #[test]
    fn picker_rejects_fake_png_without_creating_cache() {
        let directory = TestDirectory::new();
        let source = directory.0.join("fake.png");
        fs::write(&source, b"\x89PNG\r\n\x1a\nfake").unwrap();
        let cache = directory.0.join("offline-skins");
        assert!(persist_selected_png(&source, &cache).is_err());
        assert!(!cache.exists());
    }
}
