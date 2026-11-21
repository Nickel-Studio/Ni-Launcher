//! 社区资源另存为：确认保存目标后才创建下载任务。
//!
//! 与 `modrinth_install` / `curseforge_install` 的差异：安装直接写进实例资源目录，
//! 本入口先在对话框前解析文件元数据（保留选择前的网络快照），再打开原生保存对话框，
//! 用户确认后才创建受管任务；取消对话框返回 `Ok(None)`，不创建任务。
//! 默认目录由实例身份解析（mod 走 `instance_paths::for_instance`，保留 LiteLoader
//! `mods/<mc>` 特例），其它种类用资源根下对应子目录。下载先落任务暂存区并做 SHA1
//! 校验，`finish_with` 内原子替换用户选择的目标；资源文件不设安装器的 64MiB 上限。

use std::path::{Path, PathBuf};

use minecraft_java_rs_core::net::transfer::FileHashes;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::{community_network, modpacks, network_settings};

/// 对话框前的完整选择结果：目标路径 + 文件元数据 + 选择前网络快照。
struct SaveAsChoice {
    target: PathBuf,
    file_url: String,
    filename: String,
    sha1: String,
    network: network_settings::NetworkSettings,
}

/// Mod 默认文件名中的 `~` 替换为 `-`，避免 Mixin 对含波浪号路径的加载失败。
fn dialog_filename(kind: &str, filename: &str) -> String {
    if kind == "mod" {
        filename.replace('~', "-")
    } else {
        filename.to_string()
    }
}

/// 种类 → 对话框过滤器（Mod 区分 jar / litemod，其余资源均为 zip）。
fn dialog_filter(kind: &str, filename: &str) -> (&'static str, &'static [&'static str]) {
    match kind {
        "mod" if filename.to_ascii_lowercase().ends_with(".litemod") => ("Mod 文件", &["litemod"]),
        "mod" => ("Mod 文件", &["jar"]),
        "resourcepack" => ("资源包文件", &["zip"]),
        "shader" => ("光影包文件", &["zip"]),
        "datapack" => ("数据包文件", &["zip"]),
        _ => ("资源文件", &["zip"]),
    }
}

/// 默认保存目录：mod 用实例 Mod 目录（LiteLoader 为 `mods/<mc>`），其它种类用
/// 资源根下对应子目录；预创建目录，保证对话框起点真实存在。
fn default_directory(
    kind: &str,
    root: &Path,
    instance_version_id: &str,
    resource_root: &Path,
) -> Result<PathBuf, String> {
    let directory = if kind == "mod" {
        crate::resources::instance_paths::for_instance(root, instance_version_id, resource_root)?
    } else {
        let folder = crate::modrinth::kind_folder(kind).ok_or("整合包请从版本内页创建新实例")?;
        resource_root.join(folder)
    };
    std::fs::create_dir_all(&directory).map_err(|e| format!("创建默认保存目录失败：{e}"))?;
    Ok(directory)
}

/// 原生保存对话框：取消返回 `Ok(None)`；确认后做路径安全收敛
/// （into_path、拒绝目录、canonicalize 父目录、文件名非空、路径围栏）。
async fn choose_target(
    app: &AppHandle,
    kind: &str,
    directory: &Path,
    filename: &str,
) -> Result<Option<PathBuf>, String> {
    let (label, extensions) = dialog_filter(kind, filename);
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("另存为资源文件")
        .set_directory(directory)
        .set_file_name(filename)
        .add_filter(label, extensions)
        .save_file(move |path| {
            let _ = tx.send(path);
        });
    let Some(path) = rx.await.map_err(|_| "保存对话框已关闭，请重试")? else {
        return Ok(None);
    };
    let target = path
        .into_path()
        .map_err(|e| format!("请选择本地文件路径：{e}"))?;
    if target.is_dir() {
        return Err("请选择文件而不是目录".into());
    }
    let parent = target.parent().ok_or("目标文件缺少父目录")?;
    let parent = std::fs::canonicalize(parent).map_err(|e| format!("读取保存目录失败：{e}"))?;
    let name = target.file_name().ok_or("目标文件名无效")?;
    if name.is_empty() {
        return Err("目标文件名无效".into());
    }
    let target = parent.join(name);
    modpacks::check_destination(&parent, &target)?;
    Ok(Some(target))
}

/// 原子替换目标：同卷直接 rename；暂存区在配置目录、目标可能在其它卷，
/// rename 失败时先复制到目标旁临时文件再 rename，失败保留原文件并清理临时文件。
fn commit_save_as(staged: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or("目标文件缺少父目录")?;
    modpacks::check_destination(parent, target)?;
    if std::fs::rename(staged, target).is_ok() {
        return Ok(());
    }
    let temporary = parent.join(format!(
        ".nickel-save-as-{}.part",
        crate::store::random_token()
    ));
    let result = (|| {
        std::fs::copy(staged, &temporary).map_err(|e| format!("写入保存目标失败：{e}"))?;
        std::fs::rename(&temporary, target)
            .map_err(|e| format!("保存文件失败，请关闭占用目标文件的程序后重试：{e}"))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map(|_| ())
}

/// 确认选择后创建受管任务：暂存下载 + 校验 → `finish_with` 内原子落位。
/// 取消（`None`）不创建任务、不改变下载状态。
fn schedule_save_as(
    app: &AppHandle,
    choice: Option<SaveAsChoice>,
    source: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(choice) = choice else {
        return Ok(None);
    };
    let task = crate::app_tasks::begin_with_network(app, &choice.target, source, choice.network)?;
    let id = task.id.clone();
    let name = choice
        .target
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    task.describe_kind(crate::app_tasks::TaskKind::Resource, &name);
    tauri::async_runtime::spawn(async move {
        // 先下载校验到任务暂存区；最终覆盖与取消共用生命周期锁。
        let staged = task.workspace.join("resource.part");
        let result = community_network::stage_resource(
            &staged,
            &choice.filename,
            &choice.file_url,
            FileHashes {
                sha1: (!choice.sha1.is_empty()).then(|| choice.sha1.clone()),
                ..Default::default()
            },
            &task,
        )
        .await;
        if result.is_ok() {
            task.stage("保存资源文件", &choice.target.to_string_lossy());
        }
        task.finish_with(move || {
            result?;
            commit_save_as(&staged, &choice.target)?;
            Ok(None)
        });
    });
    Ok(Some(id))
}

/// Modrinth 资源另存为：返回 taskId；取消对话框返回 null（不创建任务）。
#[tauri::command]
pub async fn modrinth_save_as(
    app: AppHandle,
    kind: String,
    slug_or_id: String,
    version_id: String,
    root: String,
    instance_version_id: String,
    dir: String,
    source: Option<String>,
) -> Result<Option<String>, String> {
    let kind = crate::modrinth::normalize_kind(&kind)?;
    if crate::modrinth::kind_folder(&kind).is_none() {
        return Err("整合包请从版本内页创建新实例".into());
    }
    let slug_or_id = slug_or_id.trim();
    let version_id = version_id.trim();
    if slug_or_id.is_empty() || version_id.is_empty() {
        return Err("缺少项目或版本标识".into());
    }
    if root.trim().is_empty() || instance_version_id.trim().is_empty() || dir.trim().is_empty() {
        return Err("缺少实例目录，请先选择目标实例".into());
    }
    let network = network_settings::snapshot();
    network.transfer(source.as_deref())?;
    // 对话框前完成元数据解析，保留选择前的网络快照（与安装器另存先例一致）。
    let api = community_network::context(&network);
    let url = format!(
        "{}project/{}/version",
        community_network::MODRINTH_API,
        crate::http::urlencode(slug_or_id)
    );
    let (file_url, filename, sha1) = community_network::get_json(
        community_network::Platform::Modrinth,
        &url,
        &network,
        &api,
        None,
        |value| crate::modrinth::pick_primary_file(value, version_id),
    )
    .await?;
    let directory = default_directory(
        &kind,
        Path::new(root.trim()),
        instance_version_id.trim(),
        Path::new(dir.trim()),
    )?;
    let dialog_name = dialog_filename(&kind, &filename);
    let Some(target) = choose_target(&app, &kind, &directory, &dialog_name).await? else {
        return Ok(None);
    };
    schedule_save_as(
        &app,
        Some(SaveAsChoice {
            target,
            file_url,
            filename,
            sha1,
            network,
        }),
        source.as_deref(),
    )
}

/// CurseForge 资源另存为：返回 taskId；取消对话框返回 null（不创建任务）。
#[tauri::command]
pub async fn curseforge_save_as(
    app: AppHandle,
    kind: String,
    project_id: String,
    file_id: String,
    root: String,
    instance_version_id: String,
    dir: String,
    source: Option<String>,
) -> Result<Option<String>, String> {
    let kind = crate::modrinth::normalize_kind(&kind)?;
    if crate::modrinth::kind_folder(&kind).is_none() {
        return Err("整合包请从版本内页创建新实例".into());
    }
    let project_id = project_id
        .parse::<u64>()
        .map_err(|_| "CurseForge 项目 ID 必须为正整数")?;
    let file_id = file_id
        .parse::<u64>()
        .map_err(|_| "CurseForge 文件 ID 必须为正整数")?;
    if project_id == 0 || file_id == 0 {
        return Err("CurseForge 项目及文件 ID 必须为正整数".into());
    }
    if root.trim().is_empty() || instance_version_id.trim().is_empty() || dir.trim().is_empty() {
        return Err("缺少实例目录，请先选择目标实例".into());
    }
    let key = crate::secrets::curseforge_api_key(app.clone())?;
    let network = network_settings::snapshot();
    network.transfer(source.as_deref())?;
    let api = community_network::context(&network);
    let url = format!(
        "{}mods/{project_id}/files/{file_id}",
        community_network::CURSEFORGE_API
    );
    let (file_url, filename, sha1) = community_network::get_json(
        community_network::Platform::Curseforge,
        &url,
        &network,
        &api,
        key.as_deref(),
        |value| {
            let file = &value["data"];
            if file["modId"].as_u64() != Some(project_id) {
                return Err("CurseForge 文件不属于所选项目".into());
            }
            crate::curseforge::pick_file(std::slice::from_ref(file), &file_id.to_string())
        },
    )
    .await?;
    let directory = default_directory(
        &kind,
        Path::new(root.trim()),
        instance_version_id.trim(),
        Path::new(dir.trim()),
    )?;
    let dialog_name = dialog_filename(&kind, &filename);
    let Some(target) = choose_target(&app, &kind, &directory, &dialog_name).await? else {
        return Ok(None);
    };
    schedule_save_as(
        &app,
        Some(SaveAsChoice {
            target,
            file_url,
            filename,
            sha1,
            network,
        }),
        source.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "nickel-save-as-{}",
            crate::store::random_token()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_instance(root: &Path, name: &str, loader: Option<&str>, minecraft: &str) -> PathBuf {
        let directory = root.join("versions").join(name);
        std::fs::create_dir_all(&directory).unwrap();
        let loader_version = if loader.is_some() { "1.12.2-SNAPSHOT-r0" } else { "" };
        std::fs::write(
            directory.join("nickel.profile.json"),
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
                "nickelProfile": {
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
    fn mod_dialog_name_replaces_tilde_and_other_kinds_keep_name() {
        assert_eq!(dialog_filename("mod", "jei~1.20.jar"), "jei-1.20.jar");
        assert_eq!(dialog_filename("resourcepack", "a~b.zip"), "a~b.zip");
    }

    #[test]
    fn dialog_filters_match_kind_and_litemod_special_case() {
        assert_eq!(dialog_filter("mod", "a.jar"), ("Mod 文件", &["jar"][..]));
        assert_eq!(
            dialog_filter("mod", "a.litemod"),
            ("Mod 文件", &["litemod"][..])
        );
        assert_eq!(dialog_filter("resourcepack", "a.zip"), ("资源包文件", &["zip"][..]));
        assert_eq!(dialog_filter("shader", "a.zip"), ("光影包文件", &["zip"][..]));
        assert_eq!(dialog_filter("datapack", "a.zip"), ("数据包文件", &["zip"][..]));
    }

    #[test]
    fn default_directory_uses_instance_mods_and_liteloader_subdirectory() {
        let root = fixture_root();
        let lite = write_instance(&root, "Lite", Some("LiteLoader"), "1.12.2");
        let mods = default_directory("mod", &root, "Lite", &lite).unwrap();
        assert_eq!(mods, lite.join("mods/1.12.2"));
        assert!(mods.is_dir(), "默认目录必须预创建");

        let regular = write_instance(&root, "Regular", Some("forge"), "1.20.1");
        let mods = default_directory("mod", &root, "Regular", &regular).unwrap();
        assert_eq!(mods, regular.join("mods"));
        assert!(mods.is_dir());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn default_directory_maps_other_kinds_to_resource_subfolders() {
        let root = fixture_root();
        for (kind, folder) in [
            ("resourcepack", "resourcepacks"),
            ("shader", "shaderpacks"),
            ("datapack", "datapacks"),
        ] {
            let directory = default_directory(kind, &root, "Any", &root).unwrap();
            assert_eq!(directory, root.join(folder));
            assert!(directory.is_dir());
        }
        assert!(default_directory("modpack", &root, "Any", &root).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn commit_atomically_replaces_existing_target_without_leftovers() {
        let root = fixture_root();
        let staged = root.join("resource.part");
        let target = root.join("mods").join("example.jar");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&staged, b"new-content").unwrap();
        std::fs::write(&target, b"old-content").unwrap();
        commit_save_as(&staged, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new-content");
        assert!(!staged.exists(), "同卷 rename 后暂存文件应已移动");
        let leftovers: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".nickel-"))
            .collect();
        assert!(leftovers.is_empty(), "不得残留临时文件");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn commit_failure_keeps_existing_target_and_cleans_temporary() {
        let root = fixture_root();
        let staged = root.join("resource.part");
        std::fs::write(&staged, b"new-content").unwrap();
        // 目标是目录：rename 与 copy 均失败，原目录保持不变。
        let target = root.join("mods");
        std::fs::create_dir_all(&target).unwrap();
        assert!(commit_save_as(&staged, &target).is_err());
        assert!(target.is_dir());
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".nickel-"))
            .collect();
        assert!(leftovers.is_empty());
        // 暂存文件缺失 → 可读错误而不是 panic。
        let missing = root.join("missing.part");
        assert!(commit_save_as(&missing, &root.join("out.jar")).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

}