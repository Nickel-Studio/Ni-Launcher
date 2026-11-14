//! 首次便携目录与旧配置选择；仅选择路径，不迁移、覆盖或删除旧数据。
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

pub const GAME_DIRECTORY: &str = ".minecraft";
pub const DATA_DIRECTORY: &str = "Nickel";
pub const LEGACY_DATA_DIRECTORY: &str = "NoWheRE";
const APP_IDENTIFIER: &str = "studio.nickel.launcher";
const LEGACY_APP_IDENTIFIER: &str = "com.youtu.nowhere";
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
    matches!(identifier, APP_IDENTIFIER | LEGACY_APP_IDENTIFIER)
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
    legacy_system: Option<&Path>,
    executable: &Path,
    configured: impl Fn(&Path) -> bool,
) -> PathBuf {
    let adjacent = executable.join(DATA_DIRECTORY);
    let legacy_adjacent = executable.join(LEGACY_DATA_DIRECTORY);
    // 只选一个完整根，accounts.json 与 credentials.key 始终来自同一目录。
    [
        Some(app_config),
        Some(adjacent.as_path()),
        legacy_system,
        Some(legacy_adjacent.as_path()),
    ]
    .into_iter()
    .flatten()
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
                None,
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
        Some(&base.join(LEGACY_APP_IDENTIFIER)),
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
    #[cfg(test)]
    if !is_product_identifier(&app.config().identifier) {
        return Ok(executable_directory_for(app)?.join(GAME_DIRECTORY));
    }
    // 旧实例与含绝对路径的缓存原位沿用；旧应用数据根从系统 data_dir 推导。
    let legacy_game = app
        .path()
        .data_dir()
        .map_err(|e| format!("无法获取旧游戏目录：{e}"))?
        .join(LEGACY_APP_IDENTIFIER)
        .join("minecraft");
    if has_game_data(&legacy_game) {
        return Ok(legacy_game);
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
        let legacy = root.join(LEGACY_APP_IDENTIFIER);
        let adjacent = root.join(DATA_DIRECTORY);
        let legacy_adjacent = root.join(LEGACY_DATA_DIRECTORY);
        let candidates = [&app_config, &adjacent, &legacy, &legacy_adjacent];
        for mask in 0..(1 << candidates.len()) {
            let configured = |directory: &Path| {
                candidates
                    .iter()
                    .enumerate()
                    .any(|(index, path)| mask & (1 << index) != 0 && directory == path.as_path())
            };
            let expected = candidates
                .iter()
                .find(|path| configured(path))
                .copied()
                .unwrap_or(&adjacent);
            assert_eq!(
                select_configuration(&app_config, Some(&legacy), &root, configured),
                *expected
            );
        }
    }

    #[test]
    fn existing_broken_configuration_key_and_caches_are_not_first_run() {
        let root =
            std::env::temp_dir().join(format!("nickel-paths-{}", crate::store::random_token()));
        let legacy = root.join("legacy");
        let app_config = root.join("new-system");
        let executable = root.join("executable");
        std::fs::create_dir_all(&legacy).unwrap();
        assert_eq!(
            select_configuration(&app_config, Some(&legacy), &executable, has_configuration),
            executable.join(DATA_DIRECTORY)
        );
        for entry in CONFIG_ENTRIES {
            let path = legacy.join(entry);
            std::fs::write(&path, "broken-or-cached").unwrap();
            assert_eq!(
                select_configuration(&app_config, Some(&legacy), &executable, has_configuration),
                legacy
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken-or-cached");
            assert!(!app_config.exists());
            assert!(!executable.exists());
            std::fs::remove_file(path).unwrap();
        }
        std::fs::create_dir(legacy.join("agents")).unwrap();
        assert!(has_configuration(&legacy));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_download_cache_is_existing_configuration() {
        let root = std::env::temp_dir().join(format!(
            "nickel-legacy-download-cache-{}",
            crate::store::random_token()
        ));
        let app_config = root.join(APP_IDENTIFIER);
        let executable = root.join("executable");
        let legacy_system = root.join(LEGACY_APP_IDENTIFIER);
        let legacy_adjacent = executable.join(LEGACY_DATA_DIRECTORY);
        for legacy in [&legacy_system, &legacy_adjacent] {
            let downloads = legacy.join("cache/downloads");
            std::fs::create_dir_all(&downloads).unwrap();
            assert!(has_configuration(legacy));
            assert_eq!(
                select_configuration(
                    &app_config,
                    Some(&legacy_system),
                    &executable,
                    has_configuration
                ),
                *legacy
            );
            assert!(downloads.is_dir());
            assert!(!app_config.exists());
            assert!(!executable.join(DATA_DIRECTORY).exists());
            std::fs::remove_dir_all(legacy).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn higher_priority_cache_root_wins_over_lower_priority_json() {
        let root = std::env::temp_dir().join(format!(
            "nickel-cache-root-priority-{}",
            crate::store::random_token()
        ));
        let app_config = root.join(APP_IDENTIFIER);
        let executable = root.join("executable");
        let adjacent = executable.join(DATA_DIRECTORY);
        let legacy_system = root.join(LEGACY_APP_IDENTIFIER);
        let legacy_adjacent = executable.join(LEGACY_DATA_DIRECTORY);
        let candidates = [&app_config, &adjacent, &legacy_system, &legacy_adjacent];
        for (index, higher) in candidates.iter().enumerate() {
            for lower in &candidates[index + 1..] {
                let downloads = higher.join("cache/downloads");
                std::fs::create_dir_all(&downloads).unwrap();
                std::fs::create_dir_all(lower).unwrap();
                let json_path = lower.join("settings.json");
                let json = br#"{"accent":"rose"}"#;
                std::fs::write(&json_path, json).unwrap();
                let selected = select_configuration(
                    &app_config,
                    Some(&legacy_system),
                    &executable,
                    has_configuration,
                );
                assert_eq!(selected, **higher);
                assert!(!selected.join("settings.json").exists());
                assert!(downloads.is_dir());
                assert_eq!(std::fs::read(json_path).unwrap(), json);
                std::fs::remove_dir_all(higher).unwrap();
                std::fs::remove_dir_all(lower).unwrap();
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn test_guard_accepts_both_product_identifiers_but_not_fixture_ids() {
        assert!(is_product_identifier(APP_IDENTIFIER));
        assert!(is_product_identifier(LEGACY_APP_IDENTIFIER));
        assert!(!is_product_identifier("studio.nickel.launcher.fixture"));
        assert!(!is_product_identifier("com.youtu.nowhere.fixture"));
    }

    #[test]
    fn executable_directory_only_normalizes_exact_macos_bundle_structure() {
        let root = std::env::temp_dir().join("nickel-path-shapes");
        for (bundle, binary) in [("Nickel", "ni-launcher"), ("NoWheRE", "nowhere")] {
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
        let legacy = root.join("legacy");
        let app_config = root.join("new-system");
        assert_eq!(
            select_configuration(&app_config, Some(&legacy), &selected, has_configuration),
            root.join(DATA_DIRECTORY)
        );
        std::fs::create_dir(&legacy).unwrap();
        std::fs::write(legacy.join("settings.json"), "broken").unwrap();
        assert_eq!(
            select_configuration(&app_config, Some(&legacy), &selected, has_configuration),
            legacy
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
