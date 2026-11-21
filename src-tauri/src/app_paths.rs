//! 选择 Nickel 配置根与便携目录；账号和密钥始终从同一完整配置根读取。
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

pub const GAME_DIRECTORY: &str = ".minecraft";
pub const DATA_DIRECTORY: &str = "Nickel";
const APP_IDENTIFIER: &str = "studio.nickel.launcher";
const CONFIG_ENTRIES: &[&str] = &[
    "settings.json",
    "accounts.json",
    "active_account.json",
    "game.json",
    "instances.json",
    "secrets.local.json",
    "credentials.key",
    "offline-skins",
    "agents",
    "cache",
];

fn directory_for_executable(executable: &Path, macos: bool) -> Option<PathBuf> {
    let directory = executable.parent()?;
    if macos && directory.file_name().is_some_and(|name| name == "MacOS") {
        if let Some(contents) = directory.parent() {
            if let Some(bundle) = contents.parent() {
                if contents.file_name().is_some_and(|name| name == "Contents")
                    && bundle
                        .extension()
                        .is_some_and(|extension| extension == "app")
                {
                    // 只识别完整应用包结构；便携标记与数据放在 .app 旁，不写入签名包内。
                    return bundle.parent().map(Path::to_path_buf);
                }
            }
        }
    }
    Some(directory.to_path_buf())
}

pub fn executable_directory() -> Result<PathBuf, String> {
    std::env::current_exe()
        .ok()
        .and_then(|path| directory_for_executable(&path, cfg!(target_os = "macos")))
        .ok_or_else(|| "无法定位程序目录，请将启动器放入可写文件夹后重试".into())
}

#[cfg(test)]
pub struct TestExecutableRoot(pub PathBuf);

pub fn executable_directory_for(app: &AppHandle) -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(root) = app.try_state::<TestExecutableRoot>() {
        return Ok(root.0.clone());
    }
    let _ = app;
    executable_directory()
}

#[cfg(test)]
fn is_product_identifier(identifier: &str) -> bool {
    identifier == APP_IDENTIFIER
}

pub fn portable_root(
    app: &AppHandle,
) -> Result<Option<(PathBuf, crate::portable_pack::PortableManifest)>, String> {
    #[cfg(test)]
    if !is_product_identifier(&app.config().identifier)
        && app.try_state::<TestExecutableRoot>().is_none()
    {
        return Ok(None);
    }
    let root = executable_directory_for(app)?;
    Ok(crate::portable_pack::read_manifest(&root)?.map(|manifest| (root, manifest)))
}

fn has_configuration(directory: &Path) -> bool {
    // 损坏配置、独立密钥和组件缓存都属于已有数据；读取权限不足时也不能切换为空根。
    CONFIG_ENTRIES.iter().any(|name| {
        std::fs::symlink_metadata(directory.join(name)).map_or_else(
            |error| error.kind() != std::io::ErrorKind::NotFound,
            |_| true,
        )
    })
}

fn select_configuration(
    app_config: &Path,
    executable: &Path,
    configured: impl Fn(&Path) -> bool,
) -> PathBuf {
    let adjacent = executable.join(DATA_DIRECTORY);
    // 不拼接多个根的内容，避免账号与加密密钥分离。
    [app_config, adjacent.as_path()]
        .into_iter()
        .find(|directory| configured(directory))
        .unwrap_or(&adjacent)
        .to_path_buf()
}

fn ordinary_config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    #[cfg(test)]
    if !is_product_identifier(&app.config().identifier) {
        let fixture = app
            .path()
            .app_config_dir()
            .map_err(|e| format!("无法获取配置目录：{e}"))?;
        return if app.try_state::<TestExecutableRoot>().is_some() {
            Ok(select_configuration(
                &fixture,
                &executable_directory_for(app)?,
                has_configuration,
            ))
        } else {
            Ok(fixture)
        };
    }
    let base = app
        .path()
        .config_dir()
        .map_err(|e| format!("无法获取配置目录：{e}"))?;
    Ok(select_configuration(
        &base.join(APP_IDENTIFIER),
        &executable_directory_for(app)?,
        has_configuration,
    ))
}

pub fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let selected = if let Some((root, manifest)) = portable_root(app)? {
        let directory = root.join(manifest.data_directory);
        crate::instances::reject_links(&directory)?;
        directory
    } else {
        ordinary_config_dir(app)?
    };
    std::fs::create_dir_all(&selected).map_err(|e| {
        format!(
            "无法写入配置目录 {}：{e}。请将启动器放入有写入权限的文件夹后重试",
            selected.display()
        )
    })?;
    Ok(selected)
}

pub fn default_game_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some((root, manifest)) = portable_root(app)? {
        let directory = root.join(manifest.game_directory);
        crate::instances::reject_links(&directory)?;
        return Ok(directory);
    }
    let configuration = config_dir(app)?;
    if let Ok(text) = std::fs::read_to_string(configuration.join("game.json")) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(dir) = value
                .get("gameDir")
                .and_then(|v| v.as_str())
                .filter(|v| !v.trim().is_empty())
            {
                return Ok(PathBuf::from(dir));
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(configuration.join("instances.json")) {
        let registry = crate::instances::registry_from_text(&text);
        if let Some(dir) = registry
            .active
            .filter(|v| !v.trim().is_empty())
            .or_else(|| registry.roots.into_iter().next())
        {
            return Ok(PathBuf::from(dir));
        }
    }
    let current_game = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取游戏目录：{e}"))?
        .join("minecraft");
    if has_game_data(&current_game) {
        return Ok(current_game);
    }
    Ok(executable_directory_for(app)?.join(GAME_DIRECTORY))
}

fn has_game_data(directory: &Path) -> bool {
    directory.join("versions").is_dir() || directory.join("gameData.json").is_file()
}

/// 启动器自带 / 便携包定义的游戏目录。
///
/// 与 `default_game_dir` 不同，这里**不读取** `game.json` 或实例注册表，
/// 因此可以稳定识别「启动器自己创建的那个默认实例目录」——普通运行是程序旁的
/// `.minecraft`，便携包是包内的实例目录。界面只把它展示为「默认实例目录」，
/// 目录本身的物理名称保持不变。
pub fn bundled_game_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some((root, manifest)) = portable_root(app)? {
        let directory = root.join(manifest.game_directory);
        crate::instances::reject_links(&directory)?;
        return Ok(directory);
    }
    Ok(executable_directory_for(app)?.join(GAME_DIRECTORY))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_priority_selects_one_complete_root() {
        let root = std::env::temp_dir().join("nickel-path-priority");
        let app_config = root.join(APP_IDENTIFIER);
        let adjacent = root.join(DATA_DIRECTORY);
        let candidates = [&app_config, &adjacent];
        for mask in 0..4 {
            let configured = |directory: &Path| candidates.iter().enumerate()
                .any(|(index, path)| mask & (1 << index) != 0 && directory == path.as_path());
            let expected = candidates.iter().find(|path| configured(path)).copied().unwrap_or(&adjacent);
            assert_eq!(select_configuration(&app_config, &root, configured), *expected);
        }
    }

    #[test]
    fn configuration_entries_preserve_the_selected_root() {
        let root = std::env::temp_dir().join(format!("nickel-paths-{}", crate::store::random_token()));
        let app_config = root.join(APP_IDENTIFIER);
        let executable = root.join("executable");
        std::fs::create_dir_all(&app_config).unwrap();
        assert_eq!(select_configuration(&app_config, &executable, has_configuration), executable.join(DATA_DIRECTORY));
        for entry in CONFIG_ENTRIES {
            let path = app_config.join(entry);
            std::fs::write(&path, "broken-or-cached").unwrap();
            assert_eq!(select_configuration(&app_config, &executable, has_configuration), app_config);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken-or-cached");
            assert!(!executable.exists());
            std::fs::remove_file(path).unwrap();
        }
        std::fs::create_dir(app_config.join("agents")).unwrap();
        assert!(has_configuration(&app_config));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn system_cache_root_wins_over_adjacent_settings() {
        let root = std::env::temp_dir().join(format!("nickel-path-cache-{}", crate::store::random_token()));
        let app_config = root.join(APP_IDENTIFIER);
        let adjacent = root.join(DATA_DIRECTORY);
        std::fs::create_dir_all(app_config.join("cache/downloads")).unwrap();
        std::fs::create_dir_all(&adjacent).unwrap();
        std::fs::write(adjacent.join("settings.json"), "{}").unwrap();
        assert_eq!(select_configuration(&app_config, &root, has_configuration), app_config);
        assert!(!app_config.join("settings.json").exists());
        assert_eq!(std::fs::read_to_string(adjacent.join("settings.json")).unwrap(), "{}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn test_guard_excludes_fixture_identifiers() {
        assert!(is_product_identifier(APP_IDENTIFIER));
        assert!(!is_product_identifier("studio.nickel.launcher.fixture"));
    }

    #[test]
    fn executable_directory_only_normalizes_exact_macos_bundle_structure() {
        let root = std::env::temp_dir().join("nickel-path-shapes");
        for (bundle, binary) in [("Nickel", "ni-launcher")] {
            let bundled = root.join(format!("{bundle}.app/Contents/MacOS/{binary}"));
            assert_eq!(directory_for_executable(&bundled, true), Some(root.clone()));
            for executable in [
                root.join(format!("{binary}.exe")),
                root.join(format!("debug/{binary}")),
                root.join(format!("{bundle}/Contents/MacOS/{binary}")),
                root.join(format!("{bundle}.app/Resources/MacOS/{binary}")),
                root.join(format!("{bundle}.app/Contents/Other/{binary}")),
                root.join(format!("{bundle}.app/Contents/MacOS/helpers/{binary}")),
            ] {
                assert_eq!(
                    directory_for_executable(&executable, true),
                    executable.parent().map(Path::to_path_buf)
                );
            }
            assert_eq!(
                directory_for_executable(&bundled, false),
                bundled.parent().map(Path::to_path_buf)
            );
        }
    }

    #[test]
    fn macos_bundle_reads_adjacent_marker_without_searching_ancestors() {
        let root = std::env::temp_dir().join(format!(
            "nickel-macos-marker-{}",
            crate::store::random_token()
        ));
        let executable = root.join("Nickel.app/Contents/MacOS/ni-launcher");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        let selected = directory_for_executable(&executable, true).unwrap();
        assert_eq!(selected, root);
        let marker = crate::portable_pack::PortableManifest {
            schema_version: crate::portable_pack::SCHEMA_VERSION,
            format: crate::portable_pack::FORMAT.into(),
            package_id: crate::store::random_token(),
            name: "Bundle path fixture".into(),
            version: "1.0".into(),
            instance_name: "bundle-instance".into(),
            payload: format!("{DATA_DIRECTORY}/pack.mrpack"),
            game_directory: GAME_DIRECTORY.into(),
            data_directory: DATA_DIRECTORY.into(),
            launcher: None,
            personalization: None,
        };
        let marker_path = root.join(crate::portable_pack::MANIFEST_FILE);
        std::fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
        let found = crate::portable_pack::read_manifest(&selected)
            .unwrap()
            .unwrap();
        assert_eq!(found, marker);
        assert_eq!(
            selected.join(&found.data_directory),
            root.join(DATA_DIRECTORY)
        );
        assert_eq!(
            selected.join(&found.game_directory),
            root.join(GAME_DIRECTORY)
        );
        assert!(
            crate::portable_pack::read_manifest(executable.parent().unwrap())
                .unwrap()
                .is_none()
        );
        let nested = root.join("nested/Nickel.app/Contents/MacOS/ni-launcher");
        let nested_root = directory_for_executable(&nested, true).unwrap();
        std::fs::create_dir_all(&nested_root).unwrap();
        assert!(
            crate::portable_pack::read_manifest(&nested_root)
                .unwrap()
                .is_none()
        );
        std::fs::remove_file(marker_path).unwrap();
        // 仅程序根的 marker 生效，应用包内部及祖先目录不参与选择。
        std::fs::write(
            executable
                .parent()
                .unwrap()
                .join(crate::portable_pack::MANIFEST_FILE),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
        assert!(
            crate::portable_pack::read_manifest(&selected)
                .unwrap()
                .is_none()
        );
        let app_config = root.join(APP_IDENTIFIER);
        assert_eq!(select_configuration(&app_config, &selected, has_configuration), root.join(DATA_DIRECTORY));
        std::fs::remove_dir_all(root).unwrap();
    }
}
