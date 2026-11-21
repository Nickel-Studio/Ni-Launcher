//! LiteLoader 全量清单、安装器来源与合成 profile。
//!
//! LiteLoader 的自动安装不运行图形安装器：清单中的 `libraries`、`tweakClass`
//! 与 `version` 足以合成一个继承原版的版本 JSON。安装器另存则使用独立 Jenkins
//! 产物，二者不能共用同一 URL。

use std::path::{Path, PathBuf};

use minecraft_java_rs_core::models::loader::{
    ArtifactCategory, LoaderArtifactKind, ResolvedLoaderArtifact,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const LITELOADER_VERSIONS: &str = "https://dl.liteloader.com/versions/versions.json";
pub const LITELOADER_BMCLAPI: &str =
    "https://bmclapi2.bangbang93.com/maven/com/mumfrey/liteloader/versions.json";
pub const LITELOADER_LIBRARY_BASE: &str = "https://dl.liteloader.com/versions/";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiteLoaderEntry {
    pub minecraft_id: String,
    pub loader_version: String,
    pub label: String,
    pub file_name: String,
    pub installer_url: String,
    pub release_time: String,
    pub stable: bool,
    pub md5: Option<String>,
    pub legacy: bool,
    pub raw_metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteLoaderInstallPlan {
    pub profile: Value,
    pub profile_path: PathBuf,
    pub mods_directory: PathBuf,
}

pub fn parse_versions(text: &str) -> Result<Vec<LiteLoaderEntry>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|_| "LiteLoader 清单不是有效 JSON")?;
    let versions = value
        .get("versions")
        .and_then(Value::as_object)
        .ok_or("LiteLoader 清单缺少 versions")?;
    let mut entries = Vec::new();
    for (minecraft_id, item) in versions {
        if minecraft_id.starts_with("1.5") || minecraft_id.starts_with("1.6") {
            continue;
        }
        let Some(section) = item
            .get("artefacts")
            .filter(|value| value.is_object())
            .or_else(|| item.get("snapshots").filter(|value| value.is_object()))
        else {
            continue;
        };
        let Some(raw) = section
            .get("com.mumfrey:liteloader")
            .and_then(|value| value.get("latest"))
        else {
            continue;
        };
        let Some(loader_version) = raw.get("version").and_then(Value::as_str) else {
            continue;
        };
        let Some(tweak_class) = raw.get("tweakClass").and_then(Value::as_str) else {
            continue;
        };
        if minecraft_id.is_empty()
            || loader_version.trim().is_empty()
            || tweak_class.trim().is_empty()
        {
            continue;
        }
        let legacy = traditional_minor(minecraft_id).is_some_and(|minor| minor < 8);
        let file_name = installer_file_name(minecraft_id);
        let installer_url = installer_url(minecraft_id, &file_name)?;
        let stable = !raw
            .get("stream")
            .and_then(Value::as_str)
            .is_some_and(|stream| stream.eq_ignore_ascii_case("snapshot"));
        entries.push(LiteLoaderEntry {
            minecraft_id: minecraft_id.clone(),
            loader_version: loader_version.to_string(),
            label: minecraft_id.clone(),
            file_name,
            installer_url,
            release_time: raw
                .get("timestamp")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            stable,
            md5: raw
                .get("md5")
                .and_then(Value::as_str)
                .filter(|value| {
                    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                .map(str::to_ascii_lowercase),
            legacy,
            raw_metadata: raw.clone(),
        });
    }
    if entries.is_empty() {
        return Err("LiteLoader 清单没有可识别的 latest 条目".into());
    }
    Ok(entries)
}

fn traditional_minor(mc: &str) -> Option<u32> {
    let mut parts = mc.strip_prefix("1.")?.split('.');
    parts.next()?.parse().ok()
}

fn installer_file_name(minecraft_id: &str) -> String {
    format!(
        "liteloader-installer-{minecraft_id}{}-00-SNAPSHOT.jar",
        if matches!(minecraft_id, "1.8" | "1.9") {
            ".0"
        } else {
            ""
        }
    )
}

fn installer_url(minecraft_id: &str, file_name: &str) -> Result<String, String> {
    if traditional_minor(minecraft_id).is_some_and(|minor| minor < 8) {
        let revision = match minecraft_id {
            "1.7.10" => "04",
            "1.7.2" => "04",
            "1.6.4" => "01",
            "1.6.2" => "04",
            "1.5.2" => "01",
            _ => return Err(format!("LiteLoader 不支持该旧版本安装器：{minecraft_id}")),
        };
        return Ok(format!(
            "https://dl.liteloader.com/redist/{minecraft_id}/liteloader-installer-{minecraft_id}-{revision}.jar"
        ));
    }
    let subdir = if minecraft_id == "1.8" {
        "ant/dist/"
    } else {
        "build/libs/"
    };
    Ok(format!(
        "http://jenkins.liteloader.com/job/LiteLoaderInstaller%20{minecraft_id}/lastSuccessfulBuild/artifact/{subdir}{file_name}"
    ))
}

pub fn profile(entry: &LiteLoaderEntry) -> Result<Value, String> {
    let tweak_class = entry
        .raw_metadata
        .get("tweakClass")
        .and_then(Value::as_str)
        .ok_or("LiteLoader 条目缺少 tweakClass")?;
    let mut libraries = entry
        .raw_metadata
        .get("libraries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    libraries.push(json!({
        "name": format!("com.mumfrey:liteloader:{}", entry.loader_version),
        "url": LITELOADER_LIBRARY_BASE
    }));
    Ok(json!({
        "id": format!("{}-LiteLoader", entry.minecraft_id),
        "time": entry.release_time,
        "releaseTime": entry.release_time,
        "type": "release",
        "arguments": {"game": ["--tweakClass", tweak_class]},
        "libraries": libraries,
        "mainClass": "net.minecraft.launchwrapper.Launch",
        "minimumLauncherVersion": 18,
        "inheritsFrom": entry.minecraft_id,
        "jar": entry.minecraft_id
    }))
}

pub fn artifact(entry: &LiteLoaderEntry) -> ResolvedLoaderArtifact {
    let profile = profile(entry).unwrap_or(Value::Null);
    let profile_id = format!("{}-LiteLoader", entry.minecraft_id);
    let save_name = entry.file_name.replace("-SNAPSHOT", "");
    ResolvedLoaderArtifact {
        kind: LoaderArtifactKind::LiteLoader,
        display_version: entry.loader_version.clone(),
        catalog_version: entry.loader_version.clone(),
        installer_version: entry.loader_version.clone(),
        api_name: entry.loader_version.clone(),
        api_version: entry.loader_version.clone(),
        file_version: entry.minecraft_id.clone(),
        display_minecraft: entry.minecraft_id.clone(),
        catalog_minecraft: entry.minecraft_id.clone(),
        installer_minecraft: entry.minecraft_id.clone(),
        api_minecraft: entry.minecraft_id.clone(),
        category: ArtifactCategory::Profile,
        file_name: format!("{profile_id}.json"),
        url: String::new(),
        sha1: None,
        md5: None,
        size: None,
        save_file_name: Some(save_name),
        save_url: Some(entry.installer_url.clone()),
        save_sha1: None,
        save_md5: entry.md5.clone(),
        save_size: None,
        // 合成 profile 与受管 loader 安装均已接线；另存安装器仍需单独下载。
        installable: true,
        save_installable: true,
        version_id: Some(profile_id),
        source: "liteloader".into(),
        download_candidates: Vec::new(),
        // 同时保存完整条目与 original latest，补全/最终提交无需重新查清单。
        raw_metadata: serde_json::to_value(entry).ok(),
        profile: Some(profile),
    }
}

pub fn mods_directory(instance_root: &Path, minecraft_id: &str) -> PathBuf {
    crate::resources::instance_paths::mods_directory(
        instance_root,
        Some("liteloader"),
        minecraft_id,
    )
}

pub fn install_plan(instance_root: &Path, entry: &LiteLoaderEntry) -> Result<LiteLoaderInstallPlan, String> {
    let profile = profile(entry)?;
    let profile_id = format!("{}-LiteLoader", entry.minecraft_id);
    let profile_path = instance_root
        .join("versions")
        .join(&profile_id)
        .join(format!("{profile_id}.json"));
    Ok(LiteLoaderInstallPlan {
        profile,
        profile_path,
        mods_directory: mods_directory(instance_root, &entry.minecraft_id),
    })
}

/// 安装事务只在最终目录落盘；调用方负责在工作目录完成后一次性提交。
pub fn write_profile_atomic(
    instance_root: &Path,
    entry: &LiteLoaderEntry,
) -> Result<PathBuf, String> {
    let plan = install_plan(instance_root, entry)?;
    let parent = plan
        .profile_path
        .parent()
        .ok_or("LiteLoader profile 缺少父目录")?;
    std::fs::create_dir_all(parent).map_err(|error| format!("创建 LiteLoader 目录失败：{error}"))?;
    let bytes =
        serde_json::to_vec_pretty(&plan.profile).map_err(|error| format!("序列化 LiteLoader profile 失败：{error}"))?;
    crate::named_profiles::atomic_write(&plan.profile_path, &bytes)?;
    std::fs::create_dir_all(&plan.mods_directory)
        .map_err(|error| format!("创建 LiteLoader mods 目录失败：{error}"))?;
    Ok(plan.profile_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        r#"{"versions":{"1.12.2":{"artefacts":{"com.mumfrey:liteloader":{"latest":{"version":"1.12.2-SNAPSHOT-r0","tweakClass":"com.mumfrey.liteloader.launch.LiteLoaderTweaker","stream":"RELEASE","timestamp":"2017-09-19T21:45:00+00:00","md5":"0123456789abcdef0123456789abcdef","libraries":[{"name":"net.minecraft:launchwrapper:1.12"}]}}}},"1.8":{"snapshots":{"com.mumfrey:liteloader":{"latest":{"version":"1.8.0-SNAPSHOT-r0","tweakClass":"com.mumfrey.liteloader.launch.LiteLoaderTweaker","stream":"SNAPSHOT","libraries":[]}}}},"1.5.2":{"artefacts":{"com.mumfrey:liteloader":{"latest":{"version":"bad","tweakClass":"bad","libraries":[]}}}},"1.6.4":{"snapshots":{"com.mumfrey:liteloader":{"latest":{"version":"bad","tweakClass":"bad","libraries":[]}}}}}}"#
    }

    #[test]
    fn full_manifest_uses_latest_and_skips_1_5_1_6() {
        let entries = parse_versions(sample()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| entry.minecraft_id != "1.5.2"));
        assert!(entries.iter().all(|entry| entry.minecraft_id != "1.6.4"));
        let legacy = entries.iter().find(|entry| entry.minecraft_id == "1.8").unwrap();
        assert!(!legacy.legacy);
        assert!(!legacy.stable);
        assert!(legacy.file_name.contains("1.8.0"));
        assert!(legacy.installer_url.contains("/ant/dist/"));
    }

    #[test]
    fn synthetic_profile_contains_loader_library_and_tweak() {
        let entries = parse_versions(sample()).unwrap();
        let entry = entries
            .iter()
            .find(|entry| entry.minecraft_id == "1.12.2")
            .unwrap();
        let profile = profile(entry).unwrap();
        assert_eq!(profile["id"], "1.12.2-LiteLoader");
        assert_eq!(profile["mainClass"], "net.minecraft.launchwrapper.Launch");
        assert_eq!(
            profile["arguments"]["game"][1],
            "com.mumfrey.liteloader.launch.LiteLoaderTweaker"
        );
        assert!(profile["libraries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|library| {
                library["name"] == "com.mumfrey:liteloader:1.12.2-SNAPSHOT-r0"
                    && library["url"] == LITELOADER_LIBRARY_BASE
            }));
        assert_eq!(profile["inheritsFrom"], "1.12.2");
    }

    #[test]
    fn installer_sources_keep_legacy_and_jenkins_semantics() {
        let entries = parse_versions(
            r#"{"versions":{"1.7.10":{"artefacts":{"com.mumfrey:liteloader":{"latest":{"version":"1.7.10_04","tweakClass":"t","libraries":[]}}}}}}"#,
        )
        .unwrap();
        assert_eq!(
            entries[0].installer_url,
            "https://dl.liteloader.com/redist/1.7.10/liteloader-installer-1.7.10-04.jar"
        );
        assert!(installer_url("1.12.2", "lite.jar")
            .unwrap()
            .contains("/build/libs/lite.jar"));
    }

    #[test]
    fn artifact_separates_profile_from_saved_installer() {
        let entries = parse_versions(sample()).unwrap();
        let entry = entries
            .iter()
            .find(|entry| entry.minecraft_id == "1.12.2")
            .unwrap();
        let artifact = artifact(entry);
        assert_eq!(artifact.kind, LoaderArtifactKind::LiteLoader);
        assert_eq!(artifact.category, ArtifactCategory::Profile);
        assert!(artifact.installable);
        assert!(artifact.save_installable);
        assert_eq!(artifact.download_urls(), Vec::<&str>::new());
        let (url, name) = artifact.save_source();
        assert!(url.contains("LiteLoaderInstaller%201.12.2"));
        assert_eq!(name, "liteloader-installer-1.12.2-00.jar");
        assert_eq!(artifact.profile.as_ref().unwrap()["id"], "1.12.2-LiteLoader");
    }

    #[test]
    fn install_plan_uses_minecraft_specific_mods_directory() {
        let entries = parse_versions(sample()).unwrap();
        let entry = entries
            .iter()
            .find(|entry| entry.minecraft_id == "1.12.2")
            .unwrap();
        let root = std::env::temp_dir().join("nickel-liteloader-plan");
        let plan = install_plan(&root, entry).unwrap();
        assert!(plan
            .profile_path
            .ends_with("versions/1.12.2-LiteLoader/1.12.2-LiteLoader.json"));
        assert!(plan.mods_directory.ends_with("mods/1.12.2"));
    }

    #[test]
    fn write_profile_atomic_is_self_contained_and_repeatable() {
        let entries = parse_versions(sample()).unwrap();
        let entry = entries
            .iter()
            .find(|entry| entry.minecraft_id == "1.12.2")
            .unwrap();
        let root = std::env::temp_dir().join(format!(
            "nickel-liteloader-write-{}",
            crate::store::random_token()
        ));
        let path = write_profile_atomic(&root, entry).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["id"], "1.12.2-LiteLoader");
        write_profile_atomic(&root, entry).unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
