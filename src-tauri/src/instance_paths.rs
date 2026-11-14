//! 实例内随加载器变化的目录解析。
//!
//! LiteLoader 只扫描游戏工作目录下的 `mods/<Minecraft 版本>`；其它加载器与原版
//! 继续使用 `mods`。Mod 列表、安装、快捷目录、导出与内存规划都必须经过这里，
//! 避免界面显示与实际加载位置漂移。

use std::path::{Path, PathBuf};

use serde_json::Value;

const LITELOADER_LOADER: &str = "liteloader";

/// 纯路径规划：LiteLoader 使用游戏目录内的版本子文件夹，其它情况保持 `mods`。
pub(crate) fn mods_directory(
    resource_root: &Path,
    loader: Option<&str>,
    minecraft_version: &str,
) -> PathBuf {
    let root = resource_root.join("mods");
    if loader.is_some_and(|value| value.eq_ignore_ascii_case(LITELOADER_LOADER))
        && crate::named_profiles::valid_minecraft_version(minecraft_version)
    {
        root.join(minecraft_version)
    } else {
        root
    }
}

/// 从实例身份与有效资源根解析 Mod 目录；本地详情损坏时不猜测 LiteLoader 目录。
pub(crate) fn for_instance(
    root: &Path,
    version_id: &str,
    resource_root: &Path,
) -> Result<PathBuf, String> {
    let detail_text = crate::version_settings::version_detail(
        root.to_string_lossy().into_owned(),
        version_id.to_string(),
    )?;
    let detail: Value = serde_json::from_str(&detail_text)
        .map_err(|error| format!("读取实例加载器信息失败：{error}"))?;
    let loader = detail.get("loader").and_then(Value::as_str);
    let minecraft_version = detail
        .get("minecraftVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if loader.is_some_and(|value| value.eq_ignore_ascii_case(LITELOADER_LOADER))
        && !crate::named_profiles::valid_minecraft_version(minecraft_version)
    {
        return Err("无法识别 LiteLoader 实例的 Minecraft 版本，请补全实例后重试".into());
    }
    Ok(mods_directory(resource_root, loader, minecraft_version))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "nowhere-instance-paths-{}",
            crate::store::random_token()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_instance(root: &Path, name: &str, loader: Option<&str>, minecraft: &str) -> PathBuf {
        let directory = root.join("versions").join(name);
        std::fs::create_dir_all(&directory).unwrap();
        let loader_version = if loader.is_some() {
            "1.12.2-SNAPSHOT-r0"
        } else {
            ""
        };
        std::fs::write(
            directory.join("nowhere.profile.json"),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "name": name,
                "minecraftVersion": minecraft,
                "loader": loader,
                "loaderVersion": loader_version,
                "ready": true
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec(&serde_json::json!({
                "id": name,
                "inheritsFrom": minecraft,
                "nowhereProfile": {
                    "minecraftVersion": minecraft,
                    "loader": loader,
                    "loaderVersion": loader_version
                }
            }))
            .unwrap(),
        )
        .unwrap();
        directory
    }

    #[test]
    fn liteloader_uses_minecraft_subdirectory_and_regular_keeps_mods() {
        let root = Path::new("C:/game");
        assert_eq!(
            mods_directory(root, Some("LiteLoader"), "1.12.2"),
            root.join("mods/1.12.2")
        );
        assert_eq!(
            mods_directory(root, Some("forge"), "1.20.1"),
            root.join("mods")
        );
        assert_eq!(mods_directory(root, None, "1.20.1"), root.join("mods"));
        assert_eq!(
            mods_directory(root, Some("liteloader"), "../escape"),
            root.join("mods")
        );
    }

    #[test]
    fn real_profile_end_to_end_uses_one_liteloader_directory() {
        let root = fixture();
        let lite = write_instance(&root, "Lite", Some("LiteLoader"), "1.12.2");
        let lite_mods = lite.join("mods/1.12.2");
        assert_eq!(for_instance(&root, "Lite", &lite).unwrap(), lite_mods);
        std::fs::create_dir_all(&lite_mods).unwrap();
        std::fs::write(lite_mods.join("active.jar"), b"active").unwrap();
        std::fs::write(lite_mods.join("active.jar.disabled"), b"disabled").unwrap();
        std::fs::write(lite.join("mods/ignored.jar"), b"ignored").unwrap();
        assert_eq!(
            crate::memory_plan::count_mods_for_instance(&root, "Lite", &lite).unwrap(),
            1
        );
        assert_eq!(crate::memory_plan::count_mods(&lite_mods), 1);
        assert_eq!(
            crate::loader_catalog::liteloader_install::mods_directory(&lite, "1.12.2"),
            lite_mods
        );
        assert_eq!(
            crate::pack_export::content_directory(&lite, "mods", "liteloader", "1.12.2"),
            lite_mods
        );
        let entries = super::super::scan_local(&lite_mods).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].enabled);

        let regular = write_instance(&root, "Regular", None, "1.20.1");
        let regular_mods = regular.join("mods");
        assert_eq!(
            for_instance(&root, "Regular", &regular).unwrap(),
            regular_mods
        );
        std::fs::create_dir_all(regular_mods.join("1.20.1")).unwrap();
        std::fs::write(regular_mods.join("active.jar"), b"active").unwrap();
        std::fs::write(regular_mods.join("1.20.1/ignored.jar"), b"ignored").unwrap();
        assert_eq!(
            crate::memory_plan::count_mods_for_instance(&root, "Regular", &regular).unwrap(),
            1
        );
        assert_eq!(super::super::scan_local(&regular_mods).unwrap().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_isolated_liteloader_count_uses_explicit_resource_root() {
        let root = fixture();
        write_instance(&root, "Lite", Some("LiteLoader"), "1.12.2");
        let lite_mods = root.join("mods/1.12.2");
        std::fs::create_dir_all(&lite_mods).unwrap();
        std::fs::write(lite_mods.join("active.jar"), b"active").unwrap();
        std::fs::write(root.join("mods/ignored.jar"), b"ignored").unwrap();
        assert_eq!(for_instance(&root, "Lite", &root).unwrap(), lite_mods);
        assert_eq!(
            crate::memory_plan::count_mods_for_instance(&root, "Lite", &root).unwrap(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
