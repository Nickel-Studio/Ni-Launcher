//! 加载器 artifact 的单一解析入口。
//!
//! 目录展示、安装器另存与自动安装必须消费同一份结果。Forge 的文件类别只能来自
//! 上游清单；版本号存在不代表 installer 存在，缺少 installer 时保留 universal /
//! client 的原始信息，但把自动安装能力明确标为不可用。

use std::collections::BTreeMap;

use minecraft_java_rs_core::models::loader::{
    ArtifactCategory, LoaderArtifactKind, ResolvedLoaderArtifact,
};
use serde_json::Value;

use crate::loader_availability::{fabric_api_id, forge_api_id, is_known_broken_neoforge};

pub const FORGE_MAVEN: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";
pub const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged";
pub const FABRIC_META: &str = "https://meta.fabricmc.net/v2/versions";
pub const FABRIC_MAVEN: &str = "https://maven.fabricmc.net/net/fabricmc/fabric-installer";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeFile {
    pub category: ArtifactCategory,
    pub format: String,
    pub file_version: String,
    pub file_name: String,
    pub url: String,
    pub sha1: Option<String>,
    pub md5: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeBuildFiles {
    pub version: String,
    pub branch: String,
    pub minecraft: String,
    pub files: Vec<ForgeFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FabricInstallerSource {
    pub version: String,
    pub url: String,
}

fn category(value: &str) -> ArtifactCategory {
    match value.trim().to_ascii_lowercase().as_str() {
        "installer" => ArtifactCategory::Installer,
        "universal" => ArtifactCategory::Universal,
        "client" => ArtifactCategory::Client,
        "profile" => ArtifactCategory::Profile,
        "jar" => ArtifactCategory::Jar,
        _ => ArtifactCategory::Unknown,
    }
}

fn category_rank(value: ArtifactCategory) -> u8 {
    match value {
        ArtifactCategory::Installer => 5,
        ArtifactCategory::Universal => 4,
        ArtifactCategory::Client => 3,
        ArtifactCategory::Profile => 2,
        ArtifactCategory::Jar => 1,
        ArtifactCategory::Unknown => 0,
    }
}

fn text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn size(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str()?.trim().parse::<u64>().ok())
    })
}

fn hash(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 64
                && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .map(|value| value.to_ascii_lowercase())
}

fn split_hash(value: Option<String>) -> (Option<String>, Option<String>) {
    match value {
        Some(value) if value.len() == 40 => (Some(value), None),
        Some(value) if value.len() == 32 => (None, Some(value)),
        Some(value) => (None, Some(value)),
        None => (None, None),
    }
}

fn forge_file_from_value(value: &Value, mc: &str, file_version: &str) -> Option<ForgeFile> {
    let category = category(&text(value.get("category")));
    if category == ArtifactCategory::Unknown {
        return None;
    }
    let format = text(value.get("format"));
    if format.is_empty() {
        return None;
    }
    let extension = if format == "zip" { "zip" } else { "jar" };
    let explicit_name = {
        let name = text(value.get("name"));
        (!name.is_empty()).then_some(name).or_else(|| {
            let filename = text(value.get("filename"));
            (!filename.is_empty()).then_some(filename)
        })
    };
    let file_name = explicit_name.unwrap_or_else(|| {
        format!(
            "forge-{mc}-{file_version}-{}.{}",
            match category {
                ArtifactCategory::Installer => "installer",
                ArtifactCategory::Universal => "universal",
                ArtifactCategory::Client => "client",
                _ => "unknown",
            },
            extension
        )
    });
    let url = {
        let value = text(value.get("url"));
        if !value.is_empty() {
            value
        } else {
            format!("{FORGE_MAVEN}/{mc}-{file_version}/{file_name}")
        }
    };
    let raw_hash = hash(value.get("sha1"))
        .or_else(|| hash(value.get("md5")))
        .or_else(|| hash(value.get("hash")));
    let (fallback_sha1, fallback_md5) = split_hash(raw_hash);
    Some(ForgeFile {
        category,
        format,
        file_version: file_version.to_string(),
        file_name,
        url,
        sha1: hash(value.get("sha1")).or(fallback_sha1),
        md5: hash(value.get("md5")).or(fallback_md5),
        size: size(value.get("size")),
    })
}

fn forge_entries_from_json(
    text_value: &str,
    expected_mc: &str,
) -> Result<Vec<ForgeBuildFiles>, String> {
    let value: Value =
        serde_json::from_str(text_value).map_err(|_| "Forge 文件清单不是有效 JSON")?;
    let items = value.as_array().ok_or("Forge 文件清单格式不正确")?;
    let mut entries = Vec::new();
    for item in items {
        let version = item
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if version.is_empty() {
            continue;
        }
        let branch = item
            .get("branch")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let file_version = if branch.is_empty() {
            version.clone()
        } else {
            format!("{version}-{branch}")
        };
        let minecraft = item
            .get("mcversion")
            .or_else(|| item.get("minecraft"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let expected_api_mc = forge_api_id(expected_mc);
        let api_mc = if minecraft.is_empty() {
            expected_api_mc
        } else {
            forge_api_id(&minecraft)
        };
        let files = item
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|file| forge_file_from_value(file, &api_mc, &file_version))
            .collect::<Vec<_>>();
        entries.push(ForgeBuildFiles {
            version,
            branch,
            minecraft,
            files,
        });
    }
    Ok(entries)
}

fn extract_hrefs(markup: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = markup;
    while let Some(index) = rest.find("href=") {
        rest = &rest[index + 5..];
        let quote = rest.chars().next();
        if !matches!(quote, Some('"' | '\'')) {
            continue;
        }
        let Some(quote) = quote else {
            break;
        };
        rest = &rest[quote.len_utf8()..];
        let Some(end) = rest.find(quote) else {
            break;
        };
        out.push(&rest[..end]);
        rest = &rest[end + quote.len_utf8()..];
    }
    out
}

fn forge_entries_from_html(
    text_value: &str,
    expected_mc: &str,
) -> Result<Vec<ForgeBuildFiles>, String> {
    if text_value.trim().is_empty() {
        return Err("Forge 官网清单为空".into());
    }
    let mc = forge_api_id(expected_mc);
    let marker = format!("forge-{mc}-");
    let mut by_version: BTreeMap<String, ForgeBuildFiles> = BTreeMap::new();
    for href in extract_hrefs(text_value) {
        let Some(file_name) = href.rsplit('/').next() else {
            continue;
        };
        let Some(rest) = file_name.strip_prefix(&marker) else {
            continue;
        };
        let Some((file_version, suffix)) = rest.rsplit_once('-') else {
            continue;
        };
        let (category_name, format) = if suffix == "installer.jar" {
            ("installer", "jar")
        } else if suffix == "universal.zip" {
            ("universal", "zip")
        } else if suffix == "client.jar" || suffix == "client.zip" {
            (
                "client",
                if suffix.ends_with(".jar") {
                    "jar"
                } else {
                    "zip"
                },
            )
        } else {
            continue;
        };
        let entry = by_version
            .entry(file_version.to_string())
            .or_insert_with(|| ForgeBuildFiles {
                version: file_version.to_string(),
                branch: String::new(),
                minecraft: expected_mc.to_string(),
                files: Vec::new(),
            });
        if !entry
            .files
            .iter()
            .any(|file| file.category == category(category_name))
        {
            entry.files.push(ForgeFile {
                category: category(category_name),
                format: format.into(),
                file_version: file_version.into(),
                file_name: file_name.into(),
                url: format!("{FORGE_MAVEN}/{mc}-{file_version}/{file_name}"),
                sha1: None,
                md5: None,
                size: None,
            });
        }
    }
    if by_version.is_empty() {
        return Err("Forge 官网清单没有可识别的构建文件".into());
    }
    Ok(by_version.into_values().collect())
}

/// 自动识别 BMCLAPI JSON 或 Forge 官方 index HTML；两种来源保留同一原始字段。
pub fn parse_forge_files(markup: &str, expected_mc: &str) -> Result<Vec<ForgeBuildFiles>, String> {
    if markup.trim_start().starts_with(['[', '{']) {
        forge_entries_from_json(markup, expected_mc)
    } else {
        forge_entries_from_html(markup, expected_mc)
    }
}

pub fn matching_forge_entry<'a>(
    entries: &'a [ForgeBuildFiles],
    build: &str,
) -> Option<&'a ForgeBuildFiles> {
    entries.iter().find(|entry| {
        let branched = if entry.branch.is_empty() {
            entry.version.clone()
        } else {
            format!("{}-{}", entry.version, entry.branch)
        };
        build == entry.version || build == branched
    })
}

/// 优先选择 installer，其次 universal / client；后两类不能标为可安装或可另存安装器。
pub fn forge_artifact(
    display_mc: &str,
    api_mc: &str,
    display_version: &str,
    entry: &ForgeBuildFiles,
) -> Option<ResolvedLoaderArtifact> {
    let file = entry
        .files
        .iter()
        .max_by_key(|file| category_rank(file.category))?;
    let (sha1, md5) = (file.sha1.clone(), file.md5.clone());
    let installer_installable = file.category == ArtifactCategory::Installer;
    let installer_mc = forge_api_id(display_mc);
    let file_version = if file.file_version.is_empty() {
        if entry.branch.is_empty() {
            entry.version.clone()
        } else {
            format!("{}-{}", entry.version, entry.branch)
        }
    } else {
        file.file_version.clone()
    };
    Some(ResolvedLoaderArtifact {
        kind: LoaderArtifactKind::Forge,
        display_version: display_version.to_string(),
        catalog_version: display_version.to_string(),
        installer_version: format!("{installer_mc}-{file_version}"),
        api_name: entry.version.clone(),
        api_version: entry.version.clone(),
        file_version: file_version.clone(),
        display_minecraft: display_mc.to_string(),
        catalog_minecraft: api_mc.to_string(),
        installer_minecraft: installer_mc,
        api_minecraft: entry.minecraft.clone(),
        category: file.category,
        file_name: file.file_name.clone(),
        url: file.url.clone(),
        sha1,
        md5,
        size: file.size,
        save_file_name: None,
        save_url: None,
        save_sha1: None,
        save_md5: None,
        save_size: None,
        installable: installer_installable,
        save_installable: installer_installable,
        version_id: None,
        source: "official".into(),
        download_candidates: vec![file.url.clone()],
        raw_metadata: Some(serde_json::json!({
            "version": entry.version.clone(),
            "branch": entry.branch.clone(),
            "minecraft": entry.minecraft.clone(),
            "category": format!("{:?}", file.category),
            "format": file.format.clone(),
        })),
        profile: None,
    })
}

pub fn neoforge_artifact(
    mc: &str,
    display_version: &str,
    api_name: &str,
) -> Option<ResolvedLoaderArtifact> {
    if is_known_broken_neoforge(api_name) {
        return None;
    }
    let artifact_name = if mc == "1.20.1" { "forge" } else { "neoforge" };
    let coordinate = if mc == "1.20.1" && !api_name.starts_with("1.20.1-") {
        format!("1.20.1-{api_name}")
    } else {
        api_name.to_string()
    };
    let file_name = format!("{artifact_name}-{coordinate}-installer.jar");
    Some(ResolvedLoaderArtifact {
        kind: LoaderArtifactKind::NeoForge,
        display_version: display_version.to_string(),
        catalog_version: display_version.to_string(),
        installer_version: coordinate.clone(),
        api_name: api_name.to_string(),
        api_version: api_name.to_string(),
        file_version: api_name.to_string(),
        display_minecraft: mc.to_string(),
        catalog_minecraft: mc.to_string(),
        installer_minecraft: mc.to_string(),
        api_minecraft: mc.to_string(),
        category: ArtifactCategory::Installer,
        url: format!("{NEOFORGE_MAVEN}/{artifact_name}/{coordinate}/{file_name}"),
        file_name: file_name.clone(),
        sha1: None,
        md5: None,
        size: None,
        save_file_name: None,
        save_url: None,
        save_sha1: None,
        save_md5: None,
        save_size: None,
        installable: true,
        save_installable: true,
        version_id: None,
        source: "official".into(),
        download_candidates: vec![format!(
            "{NEOFORGE_MAVEN}/{artifact_name}/{coordinate}/{}",
            file_name
        )],
        raw_metadata: Some(serde_json::json!({
            "version": display_version,
            "apiName": api_name,
            "coordinate": coordinate,
        })),
        profile: None,
    })
}

pub fn fabric_artifact(
    mc: &str,
    loader_version: &str,
    installer: Option<&FabricInstallerSource>,
) -> ResolvedLoaderArtifact {
    let catalog_mc = fabric_api_id(mc);
    let installer_file = installer.map(|source| format!("fabric-installer-{}.jar", source.version));
    ResolvedLoaderArtifact {
        kind: LoaderArtifactKind::Fabric,
        display_version: loader_version.to_string(),
        catalog_version: loader_version.to_string(),
        installer_version: installer
            .map(|source| source.version.clone())
            .unwrap_or_default(),
        api_name: loader_version.to_string(),
        api_version: loader_version.to_string(),
        file_version: loader_version.to_string(),
        display_minecraft: mc.to_string(),
        catalog_minecraft: catalog_mc.clone(),
        installer_minecraft: catalog_mc.clone(),
        api_minecraft: mc.to_string(),
        category: ArtifactCategory::Profile,
        file_name: format!("fabric-loader-{loader_version}-{catalog_mc}.json"),
        url: format!("{FABRIC_META}/loader/{catalog_mc}/{loader_version}/profile/json"),
        sha1: None,
        md5: None,
        size: None,
        save_file_name: installer_file,
        save_url: installer.map(|source| source.url.clone()),
        save_sha1: None,
        save_md5: None,
        save_size: None,
        installable: true,
        save_installable: installer.is_some(),
        version_id: None,
        source: "official".into(),
        download_candidates: vec![format!(
            "{FABRIC_META}/loader/{catalog_mc}/{loader_version}/profile/json"
        )],
        raw_metadata: installer.map(|source| serde_json::json!({
            "installerVersion": source.version.clone(),
            "installerUrl": source.url.clone(),
        })),
        profile: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_installer_preserves_branch_file_version_and_hash() {
        let markup = r#"[{"version":"10.13.4.1614","branch":"1.7.10","mcversion":"1.7.10","files":[{"category":"client","format":"zip","hash":"11111111111111111111111111111111"},{"category":"installer","format":"jar","hash":"22222222222222222222222222222222","size":42}]}]"#;
        let entries = parse_forge_files(markup, "1.7.10").unwrap();
        let row = matching_forge_entry(&entries, "10.13.4.1614-1.7.10").unwrap();
        let artifact = forge_artifact("1.7.10", "1.7.10", "10.13.4.1614-1.7.10", row).unwrap();
        assert_eq!(artifact.category, ArtifactCategory::Installer);
        assert_eq!(artifact.file_version, "10.13.4.1614-1.7.10");
        assert_eq!(artifact.installer_minecraft, "1.7.10");
        assert_eq!(artifact.installer_version, "1.7.10-10.13.4.1614-1.7.10");
        assert_eq!(
            artifact.md5.as_deref(),
            Some("22222222222222222222222222222222")
        );
        assert_eq!(artifact.size, Some(42));
        let (save_url, save_name) = artifact.save_source();
        assert_eq!(save_url, artifact.url);
        assert_eq!(save_name, artifact.file_name);
    }

    #[test]
    fn forge_without_installer_is_not_installable_but_keeps_file_metadata() {
        let markup = r#"[{"version":"7.8.1.738","files":[{"category":"universal","format":"zip","hash":"33333333333333333333333333333333"}]}]"#;
        let entries = parse_forge_files(markup, "1.5.2").unwrap();
        let artifact = forge_artifact("1.5.2", "1.5.2", "7.8.1.738", &entries[0]).unwrap();
        assert_eq!(artifact.category, ArtifactCategory::Universal);
        assert!(!artifact.installable);
        assert!(!artifact.save_installable);
        assert!(
            artifact
                .url
                .ends_with("forge-1.5.2-7.8.1.738-universal.zip")
        );
        assert!(
            forge_artifact(
                "1.6.4",
                "1.6.4",
                "9.11.1.965",
                &ForgeBuildFiles {
                    version: "9.11.1.965".into(),
                    branch: String::new(),
                    minecraft: "1.6.4".into(),
                    files: vec![],
                }
            )
            .is_none()
        );
    }

    #[test]
    fn forge_html_uses_normalized_catalog_id_for_pre4_and_modern_files() {
        let html = r#"
          <a href="/net/minecraftforge/forge/1.7.10_pre4-10.12.2.1121/forge-1.7.10_pre4-10.12.2.1121-installer.jar">installer</a>
          <a href="/net/minecraftforge/forge/1.8.9-11.15.1.2318-1.8.9/forge-1.8.9-11.15.1.2318-1.8.9-installer.jar">installer</a>
        "#;
        let older = parse_forge_files(html, "1.7.10-pre4").unwrap();
        assert_eq!(older[0].version, "10.12.2.1121");
        assert_eq!(older[0].minecraft, "1.7.10-pre4");
        let modern = parse_forge_files(html, "1.8.9").unwrap();
        assert_eq!(modern[0].version, "11.15.1.2318-1.8.9");
    }

    #[test]
    fn neoforge_legacy_uses_forge_artifact_and_filters_known_bad_build() {
        let artifact = neoforge_artifact("1.20.1", "47.1.99", "1.20.1-47.1.99").unwrap();
        assert_eq!(artifact.installer_version, "1.20.1-47.1.99");
        assert!(artifact.url.contains("/forge/1.20.1-47.1.99/forge-"));
        assert!(neoforge_artifact("1.20.1", "47.1.82", "1.20.1-47.1.82").is_none());
        let modern = neoforge_artifact("1.20.4", "20.4.237", "20.4.237").unwrap();
        assert!(modern.url.contains("/neoforge/20.4.237/neoforge-"));
    }

    #[test]
    fn fabric_keeps_special_profile_ids_separate_from_installer_source() {
        let installer = FabricInstallerSource {
            version: "1.0.1".into(),
            url: format!("{FABRIC_MAVEN}/1.0.1/fabric-installer-1.0.1.jar"),
        };
        for (mc, expected) in [
            ("20w14∞", "20w14infinite"),
            ("Combat Test 7c", "1.16_combat-3"),
        ] {
            let artifact = fabric_artifact(mc, "0.16.9", Some(&installer));
            assert_eq!(artifact.display_minecraft, mc);
            assert_eq!(artifact.catalog_minecraft, expected);
            assert!(
                artifact
                    .url
                    .contains(&format!("/loader/{expected}/0.16.9/profile/json"))
            );
            assert!(
                artifact
                    .save_url
                    .as_deref()
                    .unwrap()
                    .contains("fabric-installer")
            );
            assert_ne!(artifact.url, artifact.save_url.clone().unwrap());
        }
    }
}
