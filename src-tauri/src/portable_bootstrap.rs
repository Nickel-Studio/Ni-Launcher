//! 显式便携包首启：复用标准整合包事务，完成记录与永久目录标记分离。
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;
use crate::{app_paths, app_tasks, instances, named_profiles, portable_pack};

const IMPORT_RECORD: &str = "portable-import.json";
const STATE_MAX_BYTES: u64 = 256 * 1024;
static ACTIVE: LazyLock<Mutex<HashMap<PathBuf, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportRecord { package_id: String, version_id: String }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapStatus {
    package_id: String,
    name: String,
    game_directory: String,
    state: String,
    task_id: Option<String>,
    version_id: Option<String>,
}

pub struct ImportGuard { root: PathBuf }
impl Drop for ImportGuard {
    fn drop(&mut self) { ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.root); }
}

fn read_value(path: &Path) -> Option<Value> {
    if instances::reject_links(path).is_err() { return None; }
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > STATE_MAX_BYTES { return None; }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn completed_version(root: &Path, manifest: &portable_pack::PortableManifest) -> Option<String> {
    let record = read_value(&root.join(&manifest.data_directory).join(IMPORT_RECORD))?;
    let record: ImportRecord = serde_json::from_value(record).ok()?;
    (record.package_id == manifest.package_id && named_profiles::validate_name(&record.version_id).is_ok())
        .then_some(record.version_id)
}

fn committed_version(root: &Path, manifest: &portable_pack::PortableManifest) -> Option<String> {
    let versions = root.join(&manifest.game_directory).join("versions");
    if instances::reject_links(&versions).is_err() { return None; }
    for entry in std::fs::read_dir(versions).ok()?.flatten() {
        let directory = entry.path();
        if !directory.is_dir() || named_profiles::is_pending_pack(&directory) { continue; }
        let name = entry.file_name().to_string_lossy().into_owned();
        if named_profiles::validate_name(&name).is_err() { continue; }
        let descriptor = read_value(&directory.join(format!("{name}.json")));
        if descriptor.as_ref().and_then(|value| value.pointer("/nickelPack/portablePackageId")).and_then(Value::as_str) == Some(manifest.package_id.as_str()) {
            return Some(name);
        }
    }
    None
}

fn save_completed(root: &Path, manifest: &portable_pack::PortableManifest, version: &str) -> Result<(), String> {
    named_profiles::validate_name(version)?;
    let path = root.join(&manifest.data_directory).join(IMPORT_RECORD);
    instances::reject_links(&path)?;
    let record = ImportRecord { package_id: manifest.package_id.clone(), version_id: version.into() };
    named_profiles::atomic_write(&path, &serde_json::to_vec(&record).map_err(|e| e.to_string())?)
}

fn status_at(root: &Path) -> Result<Option<BootstrapStatus>, String> {
    let Some(manifest) = portable_pack::read_manifest(root)? else { return Ok(None); };
    let root = std::fs::canonicalize(root).map_err(|e| format!("读取便携目录失败：{e}"))?;
    let active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).get(&root).cloned();
    let version = completed_version(&root, &manifest).or_else(|| committed_version(&root, &manifest));
    if let Some(version) = &version {
        if completed_version(&root, &manifest).is_none() { save_completed(&root, &manifest, version)?; }
    }
    Ok(Some(BootstrapStatus {
        package_id: manifest.package_id, name: manifest.name,
        game_directory: root.join(manifest.game_directory).to_string_lossy().into_owned(),
        state: if version.is_some() { "installed" } else if active.is_some() { "running" } else { "pending" }.into(),
        task_id: active, version_id: version,
    }))
}

#[tauri::command]
pub async fn portable_bootstrap_status(app: AppHandle) -> Result<Option<BootstrapStatus>, String> {
    let root = app_paths::executable_directory_for(&app)?;
    tokio::task::spawn_blocking(move || status_at(&root)).await.map_err(|e| format!("读取便携包状态失败：{e}"))?
}

#[tauri::command]
pub async fn portable_bootstrap_start(app: AppHandle) -> Result<Option<String>, String> {
    crate::onboarding::require_accepted(&app)?;
    let root = app_paths::executable_directory_for(&app)?;
    let root = std::fs::canonicalize(root).map_err(|e| format!("读取便携目录失败：{e}"))?;
    let task = {
        let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(id) = active.get(&root) { return Ok(Some(id.clone())); }
        let Some(manifest) = portable_pack::read_manifest(&root)? else { return Ok(None); };
        if completed_version(&root, &manifest).is_some() || committed_version(&root, &manifest).is_some() { return Ok(None); }
        let task = app_tasks::begin(&app, &root.join(&manifest.game_directory), None)?;
        task.describe_kind(app_tasks::TaskKind::Modpack, &manifest.instance_name);
        active.insert(root.clone(), task.id.clone());
        task
    };
    let id = task.id.clone();
    let guard = ImportGuard { root: root.clone() };
    tauri::async_runtime::spawn(async move {
        let _guard = guard;
        let checked = tokio::task::spawn_blocking(move || portable_pack::load_bootstrap(&root)).await;
        let bootstrap = match checked {
            Ok(Ok(Some(bootstrap))) => bootstrap,
            Ok(Ok(None)) => { task.finish(Err("便携包标记已移除，请重新解压后重试".into())); return; }
            Ok(Err(error)) => { task.finish(Err(error)); return; }
            Err(error) => { task.finish(Err(format!("读取便携整合包失败：{error}"))); return; }
        };
        let prepared = (|| {
            task.check_cancelled()?;
            std::fs::create_dir_all(&bootstrap.game_directory).map_err(|e| format!("创建便携游戏目录失败：{e}"))?;
            std::fs::create_dir_all(&bootstrap.data_directory).map_err(|e| format!("创建便携数据目录失败：{e}"))?;
            Ok::<_, String>(())
        })();
        if let Err(error) = prepared { task.finish(Err(error)); return; }
        match crate::modpacks::resolve_local_plan(&app, &bootstrap.payload_path, &bootstrap.manifest.instance_name, &task).await {
            Ok(mut plan) => {
                plan.source["portablePackageId"] = Value::String(bootstrap.manifest.package_id.clone());
                crate::install::execute_portable_plan(&task, plan, bootstrap).await;
            }
            Err(error) => task.finish(Err(error)),
        }
    });
    Ok(Some(id))
}

pub fn finish_import(bootstrap: &portable_pack::PortableBootstrap, version: &str) -> Result<(), String> {
    save_completed(&bootstrap.root, &bootstrap.manifest, version)
}

pub fn seed_personalization(root: &Path, manifest: &portable_pack::PortableManifest) -> Result<(), String> {
    portable_pack::validate_manifest(manifest)?;
    let settings_path = root.join(&manifest.data_directory).join("settings.json");
    if settings_path.exists() || manifest.personalization.is_none() { return Ok(()); }
    let bootstrap = portable_pack::load_bootstrap(root)?.ok_or("便携包标记已移除")?;
    if bootstrap.manifest != *manifest { return Err("便携包标记已变化，请重新打开启动器".into()); }
    if let Some(settings) = bootstrap.personalization {
        settings.resolved_value(root, manifest)?;
        named_profiles::atomic_write(&settings_path, &serde_json::to_vec(&settings).map_err(|e| e.to_string())?)?;
    }
    Ok(())
}

fn relative_background(path: &Path) -> Result<bool, String> {
    if path.components().any(|part| !matches!(part, std::path::Component::Normal(_))) {
        return Err("便携背景路径无效，请重新选择背景图片".into());
    }
    Ok(path.starts_with(app_paths::DATA_DIRECTORY))
}

pub fn background_for_display(root: &Path, manifest: &portable_pack::PortableManifest, value: &mut Value) -> Result<(), String> {
    portable_pack::validate_manifest(manifest)?;
    let Some(path) = value.get("backgroundImage").and_then(Value::as_str) else { return Ok(()); };
    let path = Path::new(path);
    if path.is_absolute() || !relative_background(path)? { return Ok(()); }
    let resolved = root.join(path);
    instances::reject_links(&resolved)?;
    value["backgroundImage"] = Value::String(resolved.to_string_lossy().into_owned());
    Ok(())
}

pub fn background_for_storage(root: &Path, manifest: &portable_pack::PortableManifest, value: &mut Value) -> Result<(), String> {
    portable_pack::validate_manifest(manifest)?;
    let Some(path) = value.get("backgroundImage").and_then(Value::as_str) else { return Ok(()); };
    let path = Path::new(path);
    if !path.is_absolute() { relative_background(path)?; return Ok(()); }
    let Ok(relative) = path.strip_prefix(root) else { return Ok(()); };
    if !relative_background(relative)? { return Ok(()); }
    instances::reject_links(path)?;
    value["backgroundImage"] = Value::String(relative.to_string_lossy().replace('\\', "/"));
    Ok(())
}
