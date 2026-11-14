//! OptiFine 清单、下载候选与安装策略。
//!
//! OptiFine 不是普通加载器：它既可作为 Mod 共用别的加载器，也可独立安装为
//! LaunchWrapper（旧版）或运行官方安装器（新版）。这里集中保存提供方原始字段
//! 和纯策略，目录、另存、安装器与最终启动配置都从同一结果派生。

use minecraft_java_rs_core::models::loader::{
    ArtifactCategory, LoaderArtifactKind, ResolvedLoaderArtifact,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::loader_availability::forge_api_id;

pub const OPTIFINE_DOWNLOADS: &str = "https://optifine.net/downloads";
pub const OPTIFINE_ADLOADX: &str = "https://optifine.net/adloadx";
pub const BMCLAPI_OPTIFINE: &str = "https://bmclapi2.bangbang93.com/optifine";
pub const BMCLAPI_VERSION_LIST: &str = "https://bmclapi2.bangbang93.com/optifine/versionList";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptiFineEntry {
    pub minecraft_id: String,
    pub version: String,
    pub display_name: String,
    pub file_name: String,
    pub file_type: String,
    pub patch: String,
    pub is_preview: bool,
    /// 官网 `yyyy-MM-dd`；镜像没有该字段时为空。
    pub release_time: String,
    /// `None` 表示页面明确标注不兼容；`Some("")` 表示未限制。
    pub forge_requirement: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptiFineRoute {
    Mods { directory: String },
    LaunchWrapper,
    Installer,
    Manual { reason: String },
    Incompatible { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptiFinePlan {
    pub route: OptiFineRoute,
    pub dependencies: Vec<String>,
}

/// 共装策略保留为显式数据；当前单选 UI 不能安全表达组合时不伪造可安装。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionLoader<'a> {
    Forge {
        build: &'a str,
    },
    NeoForge,
    Fabric {
        optifabric_matches: bool,
        fabric_api_matches: bool,
    },
    LiteLoader,
}

pub fn canonical_minecraft_id(value: &str) -> String {
    let value = value.trim();
    if value != "1.0" {
        if let Some(stripped) = value.strip_suffix(".0") {
            return stripped.to_string();
        }
    }
    value.to_string()
}

/// BMCLAPI 的 OptiFine 文件接口只为 1.8 / 1.9 保留补丁位 `.0`。
pub fn bmclapi_minecraft_id(value: &str) -> String {
    match canonical_minecraft_id(value).as_str() {
        "1.8" => "1.8.0".into(),
        "1.9" => "1.9.0".into(),
        other => other.into(),
    }
}

pub fn file_is_preview(file_name: &str) -> bool {
    file_name
        .to_ascii_lowercase()
        .trim_start_matches("preview_")
        .contains("_pre")
}

pub fn parse_index(text: &str) -> Result<Vec<OptiFineEntry>, String> {
    if text.trim_start().starts_with(['[', '{']) {
        parse_bmclapi(text)
    } else {
        parse_official(text)
    }
}

fn parse_bmclapi(text: &str) -> Result<Vec<OptiFineEntry>, String> {
    let value: Value = serde_json::from_str(text).map_err(|_| "OptiFine 镜像清单不是有效 JSON")?;
    let items = value.as_array().ok_or("OptiFine 镜像清单必须是数组")?;
    let mut entries = Vec::new();
    for item in items {
        let Some(mc) = item.get("mcversion").and_then(Value::as_str) else {
            continue;
        };
        let Some(file_type) = item.get("type").and_then(Value::as_str) else {
            continue;
        };
        let Some(patch) = item.get("patch").and_then(Value::as_str) else {
            continue;
        };
        let Some(file_name) = item.get("filename").and_then(Value::as_str) else {
            continue;
        };
        let minecraft_id = canonical_minecraft_id(mc);
        if minecraft_id.is_empty()
            || file_type.trim().is_empty()
            || patch.trim().is_empty()
            || file_name.trim().is_empty()
        {
            continue;
        }
        let forge_requirement = match item.get("forge") {
            None | Some(Value::Null) => Some(String::new()),
            Some(Value::String(value)) => normalized_forge_requirement(value),
            _ => Some(String::new()),
        };
        entries.push(OptiFineEntry {
            minecraft_id,
            version: file_name.trim_end_matches(".jar").to_string(),
            display_name: display_name(mc, file_type, patch),
            file_name: file_name.to_string(),
            file_type: file_type.to_string(),
            patch: patch.to_string(),
            is_preview: file_is_preview(file_name) || patch.to_ascii_lowercase().contains("pre"),
            release_time: String::new(),
            forge_requirement,
            source: "bmclapi".into(),
        });
    }
    if entries.is_empty() {
        return Err("OptiFine 镜像清单没有可识别条目".into());
    }
    Ok(entries)
}

fn parse_official(markup: &str) -> Result<Vec<OptiFineEntry>, String> {
    if markup.len() < 200 {
        return Err("OptiFine 官网清单长度异常".into());
    }

    // 现行页面每一行同时包含下载、镜像和 changelog 链接，直接全局搜索文件名会
    // 重复三次并夹带查询串。按下载行解析才能保证文件名、日期和 Forge 字段一一对应。
    let mut entries = official_download_rows(markup)
        .into_iter()
        .filter_map(parse_official_row)
        .collect::<Vec<_>>();

    // 兼容旧页面/固定 fixture：没有 downloadLine 行时沿用表头列提取。
    if entries.is_empty() {
        let forge = extract_texts(markup, "colForge'>", "<");
        let dates = extract_texts(markup, "colDate'>", "<");
        let names = extract_optifine_names(markup);
        if names.is_empty() {
            return Err("OptiFine 官网清单没有可识别文件名".into());
        }
        if names.len() != dates.len() || names.len() != forge.len() {
            return Err("OptiFine 官网版本、日期与 Forge 字段数量无法对应".into());
        }
        for ((name, date), forge) in names.into_iter().zip(dates).zip(forge) {
            let file_name = format!(
                "{}OptiFine_{}.jar",
                if name.to_ascii_lowercase().contains("pre") {
                    "preview_"
                } else {
                    ""
                },
                name
            );
            if let Some(entry) = optifine_entry(file_name, date, forge) {
                entries.push(entry);
            }
        }
    }

    if entries.is_empty() {
        return Err("OptiFine 官网清单没有可用条目".into());
    }
    Ok(entries)
}

fn official_download_rows(markup: &str) -> Vec<&str> {
    let mut rows = Vec::new();
    let mut rest = markup;
    while let Some(index) = rest.find("<tr") {
        let tail = &rest[index..];
        let Some(end) = tail.find("</tr>") else {
            break;
        };
        let row = &tail[..end + "</tr>".len()];
        if row.contains("downloadLine") {
            rows.push(row);
        }
        rest = &tail[end + "</tr>".len()..];
    }
    rows
}

fn parse_official_row(row: &str) -> Option<OptiFineEntry> {
    let file_name = official_file_name(row)?;
    let date = official_cell_text(row, "colDate")?;
    let forge = official_cell_text(row, "colForge")?;
    optifine_entry(file_name, date, forge)
}

fn official_cell_text<'a>(row: &'a str, class: &str) -> Option<&'a str> {
    let single = format!("{class}'>");
    let double = format!("{class}\">");
    let (start, marker_len) = if let Some(index) = row.find(&single) {
        (index, single.len())
    } else {
        (row.find(&double)?, double.len())
    };
    let tail = &row[start + marker_len..];
    let end = tail.find('<')?;
    Some(tail[..end].trim())
}

fn official_file_name(row: &str) -> Option<String> {
    let mut rest = row;
    while let Some(index) = rest.find("f=") {
        rest = &rest[index + 2..];
        let end = rest
            .find(|value: char| {
                !(value.is_ascii_alphanumeric() || matches!(value, '_' | '.' | '-'))
            })
            .unwrap_or(rest.len());
        let candidate = &rest[..end];
        if (candidate.starts_with("OptiFine_") || candidate.starts_with("preview_OptiFine_"))
            && candidate.ends_with(".jar")
            && candidate.len() <= 200
        {
            return Some(candidate.to_string());
        }
        if end == 0 {
            rest = &rest[1..];
        } else {
            rest = &rest[end..];
        }
    }
    None
}

fn optifine_entry(file_name: String, date: &str, forge: &str) -> Option<OptiFineEntry> {
    let is_preview = file_is_preview(&file_name) || file_name.starts_with("preview_");
    let (minecraft_id, file_type, patch) = parse_file_parts(&file_name)?;
    let forge_requirement = normalized_forge_requirement(forge);
    Some(OptiFineEntry {
        minecraft_id: canonical_minecraft_id(&minecraft_id),
        version: file_name.trim_end_matches(".jar").to_string(),
        display_name: display_name(&minecraft_id, &file_type, &patch),
        file_name,
        file_type,
        patch,
        is_preview,
        release_time: normalize_date(date),
        forge_requirement,
        source: "official".into(),
    })
}

fn extract_texts<'a>(markup: &'a str, start: &str, end: &str) -> Vec<&'a str> {
    let mut values = Vec::new();
    let mut rest = markup;
    while let Some(index) = rest.find(start) {
        rest = &rest[index + start.len()..];
        let Some(end_index) = rest.find(end) else {
            break;
        };
        values.push(rest[..end_index].trim());
        rest = &rest[end_index + end.len()..];
    }
    values
}

fn extract_optifine_names(markup: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = markup;
    while let Some(index) = rest.find("OptiFine_") {
        rest = &rest[index + "OptiFine_".len()..];
        // 与官网下载单元格一致：文件名后必须紧跟 `.jar"`，避免把脚本里的
        // 其它带 `.jar` 的地址重复计为一列。
        let Some(end) = rest.find(".jar\"") else {
            break;
        };
        let candidate = &rest[..end];
        if !candidate.is_empty()
            && candidate
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
        {
            names.push(candidate.to_string());
        }
        rest = &rest[end + 5..];
    }
    names
}

fn parse_file_parts(file_name: &str) -> Option<(String, String, String)> {
    let stem = file_name
        .strip_suffix(".jar")?
        .strip_prefix("preview_")
        .unwrap_or(file_name.strip_suffix(".jar")?);
    let stem = stem.strip_prefix("OptiFine_").unwrap_or(stem);
    let mut parts = stem.split('_');
    let mc = parts.next()?.to_string();
    let mut type_parts = Vec::new();
    while let Some(part) = parts.next() {
        if part.len() <= 2 && part.chars().all(|c| c.is_ascii_uppercase()) {
            type_parts.push(part);
        } else {
            let mut patch_parts = vec![part];
            patch_parts.extend(parts);
            if type_parts.is_empty() {
                return None;
            }
            return Some((mc, type_parts.join("_"), patch_parts.join("_")));
        }
    }
    None
}

fn display_name(mc: &str, file_type: &str, patch: &str) -> String {
    format!("{mc} {file_type} {patch}")
        .replace("HD_U", "")
        .replace("_", " ")
        .replace(".0 ", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn clean_forge_requirement(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("Forge")
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_string()
}

/// 官网与镜像都可能写成 `N/A` 或 `Forge N/A`；两者都表示不兼容 Forge。
fn normalized_forge_requirement(value: &str) -> Option<String> {
    let value = clean_forge_requirement(value);
    (!value.eq_ignore_ascii_case("N/A")).then_some(value)
}

fn normalize_date(value: &str) -> String {
    let value = value.trim();
    if value.len() == 10 && value.as_bytes()[4] == b'.' && value.as_bytes()[7] == b'.' {
        return format!("{}-{}-{}", &value[..4], &value[5..7], &value[8..10]);
    }
    value.to_string()
}

fn bmclapi_file_url(entry: &OptiFineEntry) -> Option<String> {
    let mc = bmclapi_minecraft_id(&entry.minecraft_id);
    if entry.is_preview {
        let split = entry.patch.to_ascii_lowercase().find("_pre")?;
        let build = &entry.patch[..split];
        let preview = &entry.patch[split + 1..];
        Some(format!(
            "{BMCLAPI_OPTIFINE}/{mc}/{}_{build}/{preview}",
            entry.file_type
        ))
    } else {
        Some(format!(
            "{BMCLAPI_OPTIFINE}/{mc}/{}/{}",
            entry.file_type, entry.patch
        ))
    }
}

pub fn adloadx_url(entry: &OptiFineEntry) -> String {
    format!("{OPTIFINE_ADLOADX}?f={}", entry.file_name)
}

pub fn official_download_link(markup: &str) -> Option<String> {
    let index = markup.find("downloadx?f=")?;
    let tail = &markup[index..];
    let end = tail.find(['"', '\'', '<', ' ']).unwrap_or(tail.len());
    let link = tail[..end].trim().replace("&amp;", "&");
    if link.is_empty() {
        return None;
    }
    if link.starts_with("https://") || link.starts_with("http://") {
        Some(link)
    } else {
        Some(format!(
            "https://optifine.net/{}",
            link.trim_start_matches('/')
        ))
    }
}

pub fn artifact(entry: &OptiFineEntry) -> ResolvedLoaderArtifact {
    let url = bmclapi_file_url(entry).unwrap_or_default();
    let mut candidates = Vec::new();
    if !url.is_empty() {
        candidates.push(url.clone());
    }
    let route = plan(
        entry.minecraft_id.as_str(),
        entry.forge_requirement.as_deref(),
        None,
    )
    .route;
    let route_name = match &route {
        OptiFineRoute::LaunchWrapper => "launchwrapper",
        OptiFineRoute::Installer => "installer",
        OptiFineRoute::Mods { .. } => "mods",
        OptiFineRoute::Manual { .. } => "manual",
        OptiFineRoute::Incompatible { .. } => "incompatible",
    };
    let profile = match route {
        OptiFineRoute::LaunchWrapper => Some(launchwrapper_profile(
            &entry.minecraft_id,
            &entry.file_name,
            None,
        )),
        _ => None,
    };
    let version_id = profile
        .as_ref()
        .and_then(|profile| profile.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let installable = !candidates.is_empty();
    let raw_metadata = serde_json::to_value(entry).ok().map(|mut value| {
        if let Some(object) = value.as_object_mut() {
            object.insert("route".into(), Value::String(route_name.into()));
            object.insert(
                "optifineVersion".into(),
                Value::String(strip_optifine_prefix(&entry.file_name)),
            );
        }
        value
    });
    ResolvedLoaderArtifact {
        kind: LoaderArtifactKind::OptiFine,
        display_version: entry.version.clone(),
        catalog_version: entry.version.clone(),
        installer_version: strip_optifine_prefix(&entry.file_name),
        api_name: entry.file_name.clone(),
        api_version: entry.file_name.clone(),
        file_version: entry.patch.clone(),
        display_minecraft: entry.minecraft_id.clone(),
        catalog_minecraft: canonical_minecraft_id(&entry.minecraft_id),
        installer_minecraft: bmclapi_minecraft_id(&entry.minecraft_id),
        api_minecraft: forge_api_id(&entry.minecraft_id),
        category: ArtifactCategory::Jar,
        file_name: entry.file_name.clone(),
        url: url.clone(),
        sha1: None,
        md5: None,
        size: None,
        save_file_name: Some(entry.file_name.clone()),
        save_url: (!url.is_empty()).then_some(url),
        save_sha1: None,
        save_md5: None,
        save_size: None,
        // 旧版本合成 LaunchWrapper，新版本由受管 Java 安装器生成 profile；
        // 两条路线都已接入运行时 LoaderType，因此目录可以真实选择自动安装。
        installable,
        save_installable: installable,
        version_id,
        source: entry.source.clone(),
        download_candidates: candidates,
        raw_metadata,
        profile,
    }
}

fn strip_optifine_prefix(file_name: &str) -> String {
    file_name
        .trim_end_matches(".jar")
        .strip_prefix("preview_")
        .unwrap_or(file_name.trim_end_matches(".jar"))
        .strip_prefix("OptiFine_")
        .unwrap_or(file_name.trim_end_matches(".jar"))
        .to_string()
}

pub fn plan(
    minecraft_id: &str,
    forge_requirement: Option<&str>,
    companion: Option<CompanionLoader<'_>>,
) -> OptiFinePlan {
    let mc = canonical_minecraft_id(minecraft_id);
    match companion {
        Some(CompanionLoader::NeoForge) => OptiFinePlan {
            route: OptiFineRoute::Incompatible {
                reason: "NeoForge 与 OptiFine 不兼容".into(),
            },
            dependencies: Vec::new(),
        },
        Some(CompanionLoader::Forge { build }) => {
            if in_range(&mc, (13, 0), (14, 3)) {
                return incompatible("Minecraft 1.13 至 1.14.3 的 Forge 与 OptiFine 不兼容");
            }
            if !forge_matches(forge_requirement, build, &mc) {
                return incompatible("OptiFine 版本要求不同的 Forge 构建");
            }
            OptiFinePlan {
                route: OptiFineRoute::Mods {
                    directory: "mods".into(),
                },
                dependencies: Vec::new(),
            }
        }
        Some(CompanionLoader::Fabric {
            optifabric_matches,
            fabric_api_matches,
        }) => {
            if at_least(&mc, (20, 5)) {
                return incompatible("Minecraft 1.20.5 及以后不再支持 Fabric + OptiFine");
            }
            if in_range(&mc, (14, 0), (15, 999)) {
                return OptiFinePlan {
                    route: OptiFineRoute::Manual {
                        reason: "该版本需手动下载 OptiFabric Origins".into(),
                    },
                    dependencies: vec!["Fabric API".into(), "OptiFabric Origins".into()],
                };
            }
            if !optifabric_matches {
                return OptiFinePlan {
                    route: OptiFineRoute::Manual {
                        reason: "没有匹配当前 Minecraft 版本的 OptiFabric".into(),
                    },
                    dependencies: vec!["Fabric API".into(), "OptiFabric".into()],
                };
            }
            let route = if fabric_api_matches {
                OptiFineRoute::Mods {
                    directory: "mods".into(),
                }
            } else {
                OptiFineRoute::Manual {
                    reason: "缺少匹配当前 Minecraft 版本的 Fabric API".into(),
                }
            };
            OptiFinePlan {
                route,
                dependencies: vec!["Fabric API".into(), "OptiFabric".into()],
            }
        }
        Some(CompanionLoader::LiteLoader) => OptiFinePlan {
            route: OptiFineRoute::Mods {
                directory: format!("mods/{mc}"),
            },
            dependencies: Vec::new(),
        },
        None if needs_installer(&mc) => OptiFinePlan {
            route: OptiFineRoute::Installer,
            dependencies: Vec::new(),
        },
        None => OptiFinePlan {
            route: OptiFineRoute::LaunchWrapper,
            dependencies: Vec::new(),
        },
    }
}

fn incompatible(reason: &str) -> OptiFinePlan {
    OptiFinePlan {
        route: OptiFineRoute::Incompatible {
            reason: reason.into(),
        },
        dependencies: Vec::new(),
    }
}

fn traditional_parts(mc: &str) -> Option<(u32, u32)> {
    let mut parts = mc.strip_prefix("1.")?.split('.');
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    parts.next().is_none().then_some((minor, patch))
}

fn in_range(mc: &str, start: (u32, u32), end: (u32, u32)) -> bool {
    traditional_parts(mc).is_some_and(|version| version >= start && version <= end)
}

fn at_least(mc: &str, minimum: (u32, u32)) -> bool {
    traditional_parts(mc).is_some_and(|version| version >= minimum)
}

fn version_drop(mc: &str) -> Option<u32> {
    if let Some((minor, _)) = traditional_parts(mc) {
        return Some(minor * 10);
    }
    let mut parts = mc.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    Some(major * 10 + minor)
}

fn needs_installer(mc: &str) -> bool {
    mc.contains('w') || version_drop(mc).is_some_and(|drop| drop >= 140)
}

pub fn forge_matches(requirement: Option<&str>, build: &str, mc: &str) -> bool {
    let Some(requirement) = requirement else {
        return false;
    };
    if requirement.trim().is_empty() {
        return true;
    }
    let requirement = clean_forge_requirement(requirement);
    let build = normalized_forge_version(build, mc);
    if requirement.contains('.') {
        build == requirement
    } else {
        build
            .rsplit('.')
            .next()
            .is_some_and(|revision| revision == requirement)
    }
}

fn normalized_forge_version(build: &str, mc: &str) -> String {
    let build = build.trim();
    let build = build.strip_prefix(&format!("{mc}-")).unwrap_or(build);
    build
        .strip_suffix(&format!("-{mc}"))
        .unwrap_or(build)
        .to_string()
}

pub fn optifine_library_version(file_name: &str) -> String {
    strip_optifine_prefix(file_name)
}

/// 旧版独立安装配置：同时覆盖旧 `minecraftArguments` 与新 `arguments.game`。
pub fn launchwrapper_profile(
    minecraft_id: &str,
    file_name: &str,
    old_minecraft_arguments: Option<&str>,
) -> Value {
    let version = optifine_library_version(file_name);
    let id = format!("{minecraft_id}-OptiFine_{version}");
    let libraries = json!([
        {"name": format!("optifine:OptiFine:{version}")},
        {"name": "net.minecraft:launchwrapper:1.12"}
    ]);
    if let Some(arguments) = old_minecraft_arguments {
        json!({
            "id": id,
            "inheritsFrom": minecraft_id,
            "jar": minecraft_id,
            "type": "release",
            "libraries": libraries,
            "mainClass": "net.minecraft.launchwrapper.Launch",
            "minecraftArguments": format!("{arguments}  --tweakClass optifine.OptiFineTweaker"),
            "minimumLauncherVersion": 18
        })
    } else {
        json!({
            "id": id,
            "inheritsFrom": minecraft_id,
            "jar": minecraft_id,
            "type": "release",
            "libraries": libraries,
            "mainClass": "net.minecraft.launchwrapper.Launch",
            "arguments": {"game": ["--tweakClass", "optifine.OptiFineTweaker"]},
            "minimumLauncherVersion": 21
        })
    }
}

#[cfg(test)]
pub fn class_major_from_header(header: &[u8]) -> Result<u32, String> {
    minecraft_java_rs_core::loader::class_major_from_header(header)
}

#[cfg(test)]
pub fn installer_minimum_java(major: u32) -> u32 {
    minecraft_java_rs_core::loader::installer_minimum_java(major)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmclapi_list_preserves_source_fields_and_dot_aliases() {
        let entries = parse_index(
            r#"[{"mcversion":"1.8.0","type":"HD_U","patch":"I7","filename":"OptiFine_1.8.0_HD_U_I7.jar","forge":"Forge 11.15.1.2318"},{"mcversion":"1.16.5","type":"HD_U","patch":"G8_pre1","filename":"preview_OptiFine_1.16.5_HD_U_G8_pre1.jar","forge":"N/A"}]"#,
        )
        .unwrap();
        assert_eq!(entries[0].minecraft_id, "1.8");
        assert_eq!(
            entries[0].forge_requirement.as_deref(),
            Some("11.15.1.2318")
        );
        assert_eq!(entries[1].minecraft_id, "1.16.5");
        assert!(entries[1].is_preview);
        assert_eq!(entries[1].forge_requirement, None);
        let artifact = artifact(&entries[1]);
        assert!(artifact.url.ends_with("/1.16.5/HD_U_G8/pre1"));
        assert_eq!(artifact.source, "bmclapi");
        assert!(artifact.installable);
        assert!(artifact.save_installable);
        assert_eq!(artifact.raw_metadata.as_ref().unwrap()["patch"], "G8_pre1");
        assert_eq!(
            artifact.raw_metadata.as_ref().unwrap()["route"],
            "installer"
        );
    }

    #[test]
    fn official_html_rows_keep_query_filenames_and_field_alignment() {
        let markup = r#"
        <html><body><table class="downloadList">
          <tr class='downloadLine'>
            <td class='colFile'>OptiFine HD U G5</td>
            <td class='colDownload'><a href="http://adfoc.us/serve/sitelinks/?id=475250&url=http://optifine.net/adloadx?f=OptiFine_1.12.2_HD_U_G5.jar&x=75a4">Download</a></td>
            <td class='colMirror'><a href="http://optifine.net/adloadx?f=OptiFine_1.12.2_HD_U_G5.jar">(Mirror)</a></td>
            <td class='colChangelog'><a href='changelog?f=OptiFine_1.12.2_HD_U_G5.jar'>Changelog</a></td>
            <td class='colForge'>Forge 14.23.5.2859</td>
            <td class='colDate'>2020.01.02</td>
          </tr>
          <tr class='downloadLine downloadLinePreview'>
            <td class='colFile'>OptiFine HD U K2 pre5</td>
            <td class='colDownload'><a href="http://adfoc.us/serve/sitelinks/?id=475250&url=http://optifine.net/adloadx?f=preview_OptiFine_26.2_HD_U_K2_pre5.jar&x=75a4">Download</a></td>
            <td class='colMirror'><a href="http://optifine.net/adloadx?f=preview_OptiFine_26.2_HD_U_K2_pre5.jar">(Mirror)</a></td>
            <td class='colChangelog'><a href='changelog?f=preview_OptiFine_26.2_HD_U_K2_pre5.jar'>Changelog</a></td>
            <td class='colForge'>Forge N/A</td>
            <td class='colDate'>2026.11.01</td>
          </tr>
        </table></body></html>
        "#;
        let entries = parse_index(markup).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].file_name, "OptiFine_1.12.2_HD_U_G5.jar");
        assert_eq!(entries[0].minecraft_id, "1.12.2");
        assert_eq!(entries[0].release_time, "2020-01-02");
        assert_eq!(
            entries[0].forge_requirement.as_deref(),
            Some("14.23.5.2859")
        );
        assert_eq!(
            adloadx_url(&entries[0]),
            "https://optifine.net/adloadx?f=OptiFine_1.12.2_HD_U_G5.jar"
        );
        assert_eq!(
            entries[1].file_name,
            "preview_OptiFine_26.2_HD_U_K2_pre5.jar"
        );
        assert_eq!(entries[1].minecraft_id, "26.2");
        assert!(entries[1].is_preview);
        assert_eq!(entries[1].forge_requirement, None);
    }

    #[test]
    fn legacy_column_markup_remains_supported() {
        let mut markup = String::from("<html>");
        for _ in 0..20 {
            markup.push_str("<td class='colForge'>Forge 14.23.5.2859</td>");
        }
        for _ in 0..20 {
            markup.push_str("<td class='colDate'>2020.01.02</td>");
        }
        for _ in 0..20 {
            markup.push_str("OptiFine_1.12.2_HD_U_G5.jar\"");
        }
        markup.push_str("</html>");
        let entries = parse_index(&markup).unwrap();
        assert_eq!(entries.len(), 20);
        assert_eq!(entries[0].file_name, "OptiFine_1.12.2_HD_U_G5.jar");
        assert_eq!(entries[0].release_time, "2020-01-02");
        assert_eq!(
            entries[0].forge_requirement.as_deref(),
            Some("14.23.5.2859")
        );
    }

    #[test]
    fn official_download_link_keeps_every_query_parameter() {
        let link = official_download_link(
            r#"<a href="downloadx?f=OptiFine_1.12.2_HD_U_G5.jar&amp;x=1">download</a>"#,
        )
        .unwrap();
        assert_eq!(
            link,
            "https://optifine.net/downloadx?f=OptiFine_1.12.2_HD_U_G5.jar&x=1"
        );
    }

    #[test]
    fn compatibility_uses_equal_forge_revision_and_explicit_ranges() {
        assert!(forge_matches(
            Some("Forge 14.23.5.2859"),
            "1.12.2-14.23.5.2859",
            "1.12.2"
        ));
        assert!(!forge_matches(
            Some("14.23.5.2859"),
            "1.12.2-14.23.5.2860",
            "1.12.2"
        ));
        assert!(matches!(
            plan(
                "1.13.2",
                Some(""),
                Some(CompanionLoader::Forge { build: "25.0.0" })
            )
            .route,
            OptiFineRoute::Incompatible { .. }
        ));
        assert!(matches!(
            plan("1.20.1", Some(""), Some(CompanionLoader::NeoForge)).route,
            OptiFineRoute::Incompatible { .. }
        ));
        assert_eq!(
            plan(
                "1.21.1",
                Some(""),
                Some(CompanionLoader::Forge { build: "52.0.1" })
            )
            .route,
            OptiFineRoute::Mods {
                directory: "mods".into()
            }
        );
        assert_eq!(
            plan("1.12.2", Some(""), Some(CompanionLoader::LiteLoader)).route,
            OptiFineRoute::Mods {
                directory: "mods/1.12.2".into()
            }
        );
    }

    #[test]
    fn fabric_optifabric_rules_are_data_driven() {
        assert!(matches!(
            plan(
                "1.20.5",
                Some(""),
                Some(CompanionLoader::Fabric {
                    optifabric_matches: true,
                    fabric_api_matches: true
                })
            )
            .route,
            OptiFineRoute::Incompatible { .. }
        ));
        assert!(matches!(
            plan(
                "1.15.2",
                Some(""),
                Some(CompanionLoader::Fabric {
                    optifabric_matches: true,
                    fabric_api_matches: true
                })
            )
            .route,
            OptiFineRoute::Manual { .. }
        ));
        assert_eq!(
            plan(
                "1.20.1",
                Some(""),
                Some(CompanionLoader::Fabric {
                    optifabric_matches: true,
                    fabric_api_matches: true
                })
            )
            .dependencies,
            vec!["Fabric API".to_string(), "OptiFabric".to_string()]
        );
    }

    #[test]
    fn standalone_route_uses_installer_from_1_14_and_launchwrapper_before() {
        assert_eq!(
            plan("1.12.2", Some(""), None).route,
            OptiFineRoute::LaunchWrapper
        );
        assert_eq!(plan("1.14", Some(""), None).route, OptiFineRoute::Installer);
        assert_eq!(plan("26.1", Some(""), None).route, OptiFineRoute::Installer);
    }

    #[test]
    fn launchwrapper_profile_supports_both_arguments_formats() {
        let old = launchwrapper_profile(
            "1.7.10",
            "OptiFine_1.7.10_HD_U_E7.jar",
            Some("--username ${auth_player_name}"),
        );
        assert!(
            old["minecraftArguments"]
                .as_str()
                .unwrap()
                .contains("--tweakClass optifine.OptiFineTweaker")
        );
        let modern = launchwrapper_profile("1.12.2", "OptiFine_1.12.2_HD_U_G5.jar", None);
        assert_eq!(modern["arguments"]["game"][0], "--tweakClass");
        assert_eq!(
            modern["libraries"][0]["name"],
            "optifine:OptiFine:1.12.2_HD_U_G5"
        );
    }

    #[test]
    fn installer_class_major_maps_to_minimum_java() {
        let header = [0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 52];
        assert_eq!(class_major_from_header(&header).unwrap(), 52);
        assert_eq!(installer_minimum_java(52), 8);
        assert_eq!(installer_minimum_java(65), 21);
        assert!(class_major_from_header(b"not-class").is_err());
    }

    #[test]
    fn canonical_ids_do_not_strip_legacy_1_0() {
        assert_eq!(canonical_minecraft_id("1.8.0"), "1.8");
        assert_eq!(canonical_minecraft_id("1.0"), "1.0");
        assert_eq!(bmclapi_minecraft_id("1.9"), "1.9.0");
        assert_eq!(bmclapi_minecraft_id("1.12.2"), "1.12.2");
        assert_eq!(
            BMCLAPI_VERSION_LIST,
            "https://bmclapi2.bangbang93.com/optifine/versionList"
        );
    }
}
