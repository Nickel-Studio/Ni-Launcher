//! Compact export selections; recursive file discovery is deferred until export.
use std::collections::{HashMap, HashSet};
use std::fs::Metadata;
use std::io::Read;
use std::path::Path;

use minecraft_java_rs_core::net::transfer::CancellationToken;
use minecraft_java_rs_core::utils::archive::{read_zip_entry, ArchiveLimits};
use serde_json::Value;

use super::{modpacks, ExportEntryKind, ExportFile};

const DIRECTORY_PREFIX: &str = "@dir/";
const METADATA_MAX_BYTES: u64 = 128 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_DISPLAY_CHARS: usize = 160;

pub(super) const CONTENT_DIRECTORIES: &[&str] = &[
    "mods",
    "config",
    "defaultconfigs",
    "kubejs",
    "scripts",
    "patchouli_books",
    "datapacks",
    "openloader",
    "global_packs",
    "resources",
    "structures",
    "packmenu",
    "paxi",
    "tacz",
    "immersive_paintings",
    "schematics",
    "replay_recordings",
    "replay_videos",
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
pub(super) const CONTENT_FILES: &[&str] = &[
    "options.txt",
    "optionsof.txt",
    "optionsshaders.txt",
    "servers.dat",
    "servers.dat_old",
    "hotbar.nbt",
    "command_history.txt",
    "emi.json",
    "patchouli_data.json",
];

const CONFIG_SCOPES: &[(&str, &str, &str)] = &[
    ("config/jei/world", "jeiPersonal", "JEI 个人数据"),
    ("config/tacz/custom", "tacz", "TaCZ 自定义枪包"),
    (
        "config/openloader",
        "packData",
        "OpenLoader 内置数据与资源包",
    ),
    ("config/global_packs", "packData", "全局内置包"),
    ("config/paxi/datapacks", "packData", "Paxi 内置数据包"),
    ("config/paxi/resourcepacks", "packData", "Paxi 内置资源包"),
    ("config/packmenu", "packData", "整合包菜单资源"),
    ("config/patchouli_books", "packData", "帕秋莉内置书籍"),
];
const CONFIG_FILES: &[&str] = &[
    "config/emi.json",
    "config/patchouli_data.json",
    "config/iris.properties",
    "config/oculus.properties",
];

fn within(path: &str, directory: &str) -> bool {
    path == directory
        || path
            .strip_prefix(directory)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub(super) fn sensitive_path(path: &str) -> bool {
    let world = path
        .split_once('/')
        .is_some_and(|(root, _)| root.eq_ignore_ascii_case("saves"));
    path.split('/').enumerate().any(|(index, part)| {
        // A world display name is not a credential-bearing file name.
        if world && index == 1 {
            return false;
        }
        let name = part.to_ascii_lowercase();
        let stem = name
            .rsplit_once('.')
            .map_or(name.as_str(), |(stem, _)| stem);
        name.starts_with('.')
            || name.starts_with("nickel.")
            || super::credential_key(stem)
            || matches!(
                stem,
                "secret"
                    | "secrets"
                    | "passwords"
                    | "credential"
                    | "credentials"
                    | "private-token"
                    | "private_token"
            )
            || matches!(
                name.as_str(),
                "skin-oauth-clients.json"
                    | "accounts.json"
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

fn excluded_path(path: &str) -> bool {
    if sensitive_path(path) {
        return true;
    }
    // Atomic packages own their internal content. Runtime-directory filters must
    // not remove world inventories/statistics or similarly named pack assets.
    if path.split_once('/').is_some_and(|(root, child)| {
        matches!(
            root.to_ascii_lowercase().as_str(),
            "saves" | "resourcepacks" | "shaderpacks"
        ) && !child.is_empty()
    }) {
        return false;
    }
    path.split('/').any(|part| {
        matches!(
            part.to_ascii_lowercase().as_str(),
            "cache" | "caches" | "cached" | "probe" | "exported" | "playerdata" | "stats"
        )
    })
}

pub(super) fn group_for(path: &str) -> Option<(&'static str, bool)> {
    if excluded_path(path) {
        return None;
    }
    let lower = path.to_ascii_lowercase();
    if within(&lower, "config/jei/world") {
        return Some(("jeiPersonal", false));
    }
    if matches!(lower.as_str(), "emi.json" | "config/emi.json") {
        return Some(("emiPersonal", false));
    }
    if matches!(
        lower.as_str(),
        "patchouli_data.json" | "config/patchouli_data.json"
    ) {
        return Some(("patchouliPersonal", false));
    }
    if within(&lower, "config/tacz/custom") {
        return Some(("tacz", true));
    }
    if CONFIG_SCOPES
        .iter()
        .any(|(scope, group, _)| *group == "packData" && within(&lower, scope))
    {
        return Some(("packData", true));
    }
    let shader_options = matches!(
        lower.as_str(),
        "optionsshaders.txt" | "config/iris.properties" | "config/oculus.properties"
    ) || lower.strip_prefix("shaderpacks/").is_some_and(|file| {
        !file.contains('/')
            && matches!(
                file.rsplit('.').next(),
                Some("txt" | "properties" | "cfg" | "ini" | "json" | "toml" | "yaml" | "yml")
            )
    });
    if shader_options {
        return Some(("shaderOptions", true));
    }
    if within(&lower, "journeymap/config")
        || within(&lower, "journeymap/server")
        || lower.strip_prefix("journeymap/").is_some_and(|file| {
            !file.contains('/')
                && matches!(
                    file.rsplit('.').next(),
                    Some("json" | "cfg" | "conf" | "config" | "properties" | "toml")
                )
        })
    {
        return Some(("config", true));
    }
    Some(match lower.split('/').next()? {
        "mods"
            if lower.ends_with(".disabled")
                || lower.ends_with(".disable")
                || lower.ends_with(".old") =>
        {
            ("disabledMods", false)
        }
        "mods" => ("mods", true),
        "config" | "defaultconfigs" => ("config", true),
        "kubejs" | "scripts" | "patchouli_books" | "datapacks" | "openloader" | "global_packs"
        | "resources" | "structures" | "packmenu" | "paxi" => ("packData", true),
        "tacz" => ("tacz", true),
        "immersive_paintings" => ("paintings", true),
        "schematics" => ("schematics", false),
        "replay_recordings" | "replay_videos" => ("replays", false),
        "resourcepacks" => ("resourcepacks", true),
        "shaderpacks" => ("shaderpacks", true),
        "screenshots" => ("screenshots", false),
        "saves" => ("saves", false),
        "xaero" | "xaerowaypoints" | "xaeroworldmap" | "journeymap" | "voxelmap" => ("maps", false),
        "options.txt" | "optionsof.txt" if !lower.contains('/') => ("gameOptions", true),
        "servers.dat" | "servers.dat_old" if !lower.contains('/') => ("servers", false),
        "hotbar.nbt" | "command_history.txt" if !lower.contains('/') => ("personal", false),
        _ => return None,
    })
}

pub(super) enum Selection<'a> {
    File(&'a str),
    Directory { group: &'a str, relative: &'a str },
}

fn directory_allowed(group: &str, relative: &str) -> bool {
    let lower = relative.to_ascii_lowercase();
    if CONFIG_SCOPES
        .iter()
        .any(|(scope, allowed_group, _)| *scope == lower && *allowed_group == group)
    {
        return true;
    }
    if lower == "journeymap" {
        return matches!(group, "config" | "maps");
    }
    if lower == "shaderpacks" {
        return group == "shaderOptions";
    }
    if lower.starts_with("mods/") {
        return matches!(group, "mods" | "disabledMods") && !excluded_path(relative);
    }
    if let Some((root, child)) = lower.split_once('/') {
        if matches!(root, "resourcepacks" | "shaderpacks" | "saves") {
            return !child.contains('/')
                && group_for(relative).is_some_and(|(allowed, _)| allowed == group);
        }
        return false;
    }
    CONTENT_DIRECTORIES.contains(&relative)
        && !matches!(
            lower.as_str(),
            "mods" | "resourcepacks" | "shaderpacks" | "saves"
        )
        && group_for(relative).is_some_and(|(allowed, _)| allowed == group)
}

pub(super) fn parse_selection(value: &str) -> Result<Selection<'_>, String> {
    if value.len() > 4096 || value.ends_with('/') {
        return Err(format!("导出选择路径无效：{value}"));
    }
    if let Some(rest) = value.strip_prefix(DIRECTORY_PREFIX) {
        let (group, relative) = rest.split_once('/').ok_or("导出目录选择缺少分类或路径")?;
        modpacks::validate_pack_path(relative).map_err(|e| e.to_string())?;
        if !directory_allowed(group, relative) || excluded_path(relative) {
            return Err(format!("导出目录或分类不允许：{value}"));
        }
        Ok(Selection::Directory { group, relative })
    } else {
        modpacks::validate_pack_path(value).map_err(|e| e.to_string())?;
        if group_for(value).is_none() {
            return Err(format!("导出文件不允许：{value}"));
        }
        Ok(Selection::File(value))
    }
}

fn relative_path(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "导出文件超出实例目录")?
        .to_str()
        .ok_or("导出文件名不是有效 Unicode")?
        .replace('\\', "/");
    modpacks::validate_pack_path(&relative).map_err(|e| e.to_string())?;
    Ok(relative)
}

fn metadata(root: &Path, path: &Path) -> Result<Option<Metadata>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("读取导出文件失败：{error}")),
    };
    modpacks::check_destination(root, path)?;
    if metadata.file_type().is_symlink() {
        return Err("导出内容不能包含符号链接".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("导出内容不能包含重解析点".into());
        }
    }
    if !metadata.is_dir() && !metadata.is_file() {
        return Err(format!("导出内容不能包含特殊文件：{}", path.display()));
    }
    Ok(Some(metadata))
}

fn plain_label(text: &str) -> Option<String> {
    let mut stripped = String::new();
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character == '\u{a7}' {
            chars.next();
        } else if character == '\n' || character == '\r' {
            if !stripped.trim().is_empty() {
                break;
            }
        } else if !character.is_control() {
            stripped.push(character);
        }
        if stripped.chars().count() >= MAX_DISPLAY_CHARS {
            break;
        }
    }
    let label = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    (!label.is_empty()).then_some(label)
}

fn component_text(value: &Value, depth: usize, result: &mut String) {
    if depth > 24 || result.chars().count() > MAX_DISPLAY_CHARS * 4 {
        return;
    }
    match value {
        Value::String(text) => {
            result.push_str(&text.chars().take(MAX_DISPLAY_CHARS * 4).collect::<String>())
        }
        Value::Array(parts) => {
            for part in parts {
                component_text(part, depth + 1, result);
            }
        }
        Value::Object(object) => {
            if let Some(text) = object
                .get("text")
                .or_else(|| object.get("fallback"))
                .or_else(|| object.get("translate"))
            {
                component_text(text, depth + 1, result);
            }
            if let Some(extra) = object.get("extra") {
                component_text(extra, depth + 1, result);
            }
        }
        _ => {}
    }
}

fn resource_pack_name(
    root: &Path,
    path: &Path,
    directory: bool,
    cancel: &CancellationToken,
) -> Option<String> {
    let bytes = if directory {
        let file = path.join("pack.mcmeta");
        let size = metadata(root, &file).ok().flatten()?.len();
        if size > METADATA_MAX_BYTES {
            return None;
        }
        let mut data = Vec::new();
        std::fs::File::open(file)
            .ok()?
            .take(METADATA_MAX_BYTES + 1)
            .read_to_end(&mut data)
            .ok()?;
        if data.len() as u64 > METADATA_MAX_BYTES {
            return None;
        }
        data
    } else if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        read_zip_entry(path, "pack.mcmeta", METADATA_MAX_BYTES, cancel).ok()?
    } else {
        return None;
    };
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let mut text = String::new();
    component_text(value.get("pack")?.get("description")?, 0, &mut text);
    plain_label(&text)
}

fn basename(path: &Path, strip_extension: bool) -> String {
    let value = if strip_extension {
        path.file_stem()
    } else {
        path.file_name()
    };
    value
        .and_then(|value| value.to_str())
        .and_then(plain_label)
        .unwrap_or_else(|| "未命名内容".into())
}

fn directory_label(relative: &str) -> String {
    match relative {
        "config" => "Mod 配置（不含个人数据）",
        "defaultconfigs" => "默认世界配置",
        "kubejs" => "KubeJS 脚本与数据",
        "scripts" => "整合包脚本",
        "patchouli_books" => "帕秋莉内置书籍",
        "datapacks" => "内置数据包",
        "openloader" => "OpenLoader 内置包",
        "global_packs" => "全局内置包",
        "resources" => "整合包内置资源",
        "structures" => "整合包内置结构",
        "packmenu" => "整合包菜单资源",
        "paxi" => "Paxi 内置包",
        "tacz" => "TaCZ 枪包",
        "immersive_paintings" => "已上传的沉浸画",
        "schematics" => "导出的结构",
        "replay_recordings" => "录像回放",
        "replay_videos" => "导出的回放视频",
        "screenshots" => "游戏截图",
        "Xaero" | "xaero" | "XaeroWaypoints" | "XaeroWorldMap" | "voxelmap" => relative,
        _ => relative.rsplit('/').next().unwrap_or(relative),
    }
    .into()
}

fn push_entry(
    files: &mut Vec<ExportFile>,
    relative: &str,
    metadata: &Metadata,
    group: &str,
    selected: bool,
    display_name: String,
) -> Result<(), String> {
    if files.len() >= ArchiveLimits::default().max_entries {
        return Err("导出预览项目超过上限，请减少内容".into());
    }
    let kind = if metadata.is_dir() {
        ExportEntryKind::Directory
    } else {
        ExportEntryKind::File
    };
    let path = if metadata.is_dir() {
        format!("{DIRECTORY_PREFIX}{group}/{relative}")
    } else {
        relative.into()
    };
    parse_selection(&path)?;
    files.push(ExportFile {
        path,
        source_path: relative.into(),
        display_name,
        kind,
        size_bytes: metadata.is_file().then_some(metadata.len()),
        group: group.into(),
        selected,
    });
    Ok(())
}

pub(super) fn collect_preview(
    root: &Path,
    path: &Path,
    files: &mut Vec<ExportFile>,
    cancel: &CancellationToken,
) -> Result<(), String> {
    cancel.check()?;
    let relative = relative_path(root, path)?;
    if excluded_path(&relative) {
        return Ok(());
    }
    let Some(meta) = metadata(root, path)? else {
        return Ok(());
    };
    let Some((group, selected)) = group_for(&relative) else {
        return Ok(());
    };
    if !meta.is_dir() {
        return push_entry(
            files,
            &relative,
            &meta,
            group,
            selected,
            basename(path, false),
        );
    }
    let mods_scope = within(&relative, "mods");
    if matches!(relative.as_str(), "resourcepacks" | "shaderpacks" | "saves") || mods_scope {
        let mut shader_options = false;
        for child in std::fs::read_dir(path).map_err(|e| format!("读取导出目录失败：{e}"))?
        {
            cancel.check()?;
            let child = child.map_err(|e| e.to_string())?.path();
            let child_relative = relative_path(root, &child)?;
            if excluded_path(&child_relative) {
                continue;
            }
            let Some(child_meta) = metadata(root, &child)? else {
                continue;
            };
            let Some((child_group, child_selected)) = group_for(&child_relative) else {
                continue;
            };
            if relative == "shaderpacks" && child_group == "shaderOptions" {
                shader_options = true;
                continue;
            }
            if child_meta.is_dir() && mods_scope {
                // File suffixes, not directory names, determine enabled/disabled payloads.
                push_entry(
                    files,
                    &child_relative,
                    &child_meta,
                    "mods",
                    true,
                    basename(&child, false),
                )?;
                push_entry(
                    files,
                    &child_relative,
                    &child_meta,
                    "disabledMods",
                    false,
                    format!("{}（已禁用）", basename(&child, false)),
                )?;
                continue;
            }
            let name = if relative == "resourcepacks" {
                resource_pack_name(root, &child, child_meta.is_dir(), cancel)
                    .unwrap_or_else(|| basename(&child, !child_meta.is_dir()))
            } else {
                basename(
                    &child,
                    !child_meta.is_dir() && child_group != "mods" && child_group != "disabledMods",
                )
            };
            push_entry(
                files,
                &child_relative,
                &child_meta,
                child_group,
                child_selected,
                name,
            )?;
        }
        if shader_options {
            push_entry(
                files,
                &relative,
                &meta,
                "shaderOptions",
                true,
                "光影包独立设置".into(),
            )?;
        }
        return cancel.check();
    }
    if relative == "journeymap" {
        push_entry(
            files,
            &relative,
            &meta,
            "config",
            true,
            "JourneyMap 配置与服务器设置".into(),
        )?;
        return push_entry(
            files,
            &relative,
            &meta,
            "maps",
            false,
            "JourneyMap 个人地图".into(),
        );
    }
    push_entry(
        files,
        &relative,
        &meta,
        group,
        selected,
        directory_label(&relative),
    )?;
    if relative == "config" {
        for (scope, group, label) in CONFIG_SCOPES {
            cancel.check()?;
            if let Some(meta) = metadata(root, &root.join(scope))? {
                if meta.is_dir() {
                    push_entry(
                        files,
                        scope,
                        &meta,
                        group,
                        *group != "jeiPersonal",
                        (*label).into(),
                    )?;
                }
            }
        }
        for file in CONFIG_FILES {
            collect_preview(root, &root.join(file), files, cancel)?;
        }
    }
    cancel.check()
}

pub(super) struct SelectedFile {
    pub path: String,
    pub size_bytes: u64,
}

struct Discovery {
    files: Vec<SelectedFile>,
    seen: HashMap<String, String>,
    total: u64,
    limits: ArchiveLimits,
}

fn can_descend(relative: &str, group: &str) -> bool {
    group_for(relative).is_some_and(|(current, _)| current == group)
        || (relative == "journeymap" && group == "config")
        || (relative == "shaderpacks" && group == "shaderOptions")
        || (relative.starts_with("mods/") && group == "disabledMods")
}

impl Discovery {
    fn visit(
        &mut self,
        root: &Path,
        path: &Path,
        group: &str,
        cancel: &CancellationToken,
        depth: usize,
    ) -> Result<(), String> {
        cancel.check()?;
        if depth > MAX_DEPTH {
            return Err("导出目录嵌套超过上限".into());
        }
        let relative = relative_path(root, path)?;
        if excluded_path(&relative) {
            return Ok(());
        }
        let Some(meta) = metadata(root, path)? else {
            return Err(format!("文件已变化，请刷新导出列表：{relative}"));
        };
        if meta.is_dir() {
            if !can_descend(&relative, group) {
                return Ok(());
            }
            for child in std::fs::read_dir(path).map_err(|e| format!("读取导出目录失败：{e}"))?
            {
                cancel.check()?;
                self.visit(
                    root,
                    &child.map_err(|e| e.to_string())?.path(),
                    group,
                    cancel,
                    depth + 1,
                )?;
            }
        } else if group_for(&relative).is_some_and(|(current, _)| current == group) {
            let folded = relative.to_lowercase();
            if let Some(previous) = self.seen.get(&folded) {
                if previous != &relative {
                    return Err(format!("导出文件存在大小写冲突：{relative}"));
                }
                return Ok(());
            }
            self.total = self
                .total
                .checked_add(meta.len())
                .ok_or("导出文件总大小溢出")?;
            if self.files.len() >= self.limits.max_entries
                || meta.len() > self.limits.max_entry_bytes
                || self.total > self.limits.max_total_bytes
            {
                return Err("导出内容超过归档配额，请减少所选内容".into());
            }
            self.seen.insert(folded, relative.clone());
            self.files.push(SelectedFile {
                path: relative,
                size_bytes: meta.len(),
            });
        }
        Ok(())
    }
}

pub(super) fn selected_files(
    root: &Path,
    preview: &[ExportFile],
    selections: &[String],
    cancel: &CancellationToken,
) -> Result<Vec<SelectedFile>, String> {
    cancel.check()?;
    let available: HashMap<_, _> = preview
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let mut discovery = Discovery {
        files: Vec::new(),
        seen: HashMap::new(),
        total: 0,
        limits: ArchiveLimits::default(),
    };
    let mut selected = HashSet::new();
    for value in selections {
        cancel.check()?;
        if !selected.insert(value.to_lowercase()) {
            return Err(format!("导出选择重复：{value}"));
        }
        match parse_selection(value)? {
            Selection::Directory { group, relative } => {
                let entry = available
                    .get(value.as_str())
                    .ok_or_else(|| format!("目录已变化，请刷新导出列表：{relative}"))?;
                if entry.kind != ExportEntryKind::Directory
                    || entry.source_path != relative
                    || entry.group != group
                {
                    return Err(format!("导出目录选择不匹配：{value}"));
                }
                let meta = metadata(root, &root.join(relative))?
                    .ok_or_else(|| format!("目录已变化：{relative}"))?;
                if !meta.is_dir() {
                    return Err(format!("导出目录已变成文件：{relative}"));
                }
                discovery.visit(root, &root.join(relative), group, cancel, 0)?;
            }
            Selection::File(relative) => {
                let (group, _) = group_for(relative).ok_or("导出文件分类无效")?;
                let allowed = available
                    .get(relative)
                    .is_some_and(|entry| entry.kind == ExportEntryKind::File);
                if !allowed {
                    return Err(format!("文件已变化，请刷新导出列表：{relative}"));
                }
                let meta = metadata(root, &root.join(relative))?
                    .ok_or_else(|| format!("文件已变化：{relative}"))?;
                if !meta.is_file() {
                    return Err(format!("文件选择不能隐式选择目录：{relative}"));
                }
                discovery.visit(root, &root.join(relative), group, cancel, 0)?;
            }
        }
    }
    discovery.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(discovery.files)
}
