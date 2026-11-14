//! 真实 Wry + 磁盘首启验收。确定性缓存只替代下载输入，事务、命令、事件不替换。
use super::*;
use minecraft_java_rs_core::net::transfer::CancellationToken;
use minecraft_java_rs_core::utils::archive::{ArchiveLimits, write_zip_files};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Listener, Manager};

fn app(identifier: &str) -> Result<tauri::App, String> {
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().identifier = identifier.into();
    tauri::Builder::default()
        .any_thread()
        .build(context)
        .map_err(|e| e.to_string())
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(path.parent().ok_or("missing parent")?).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

fn fixture(
    root: &Path,
    data_directory: &str,
    real: bool,
) -> Result<portable_pack::PortableManifest, String> {
    let personalization = format!("{data_directory}/personalization.json");
    let background = format!("{data_directory}/background.png");
    let manifest = portable_pack::PortableManifest {
        schema_version: portable_pack::SCHEMA_VERSION,
        format: portable_pack::FORMAT.into(),
        package_id: store::random_token(),
        name: "Portable acceptance".into(),
        version: "1.0".into(),
        instance_name: "portable-instance".into(),
        payload: format!("{data_directory}/pack.mrpack"),
        game_directory: app_paths::GAME_DIRECTORY.into(),
        data_directory: data_directory.into(),
        launcher: None,
        personalization: Some(personalization.clone()),
    };
    portable_pack::validate_manifest(&manifest)?;
    write(
        &root.join(portable_pack::MANIFEST_FILE),
        &serde_json::to_vec(&manifest).unwrap(),
    )?;
    std::fs::create_dir_all(root.join(&manifest.data_directory)).map_err(|e| e.to_string())?;
    let mut dependencies = json!({"minecraft": if real { "1.16.5" } else { "1.20.1" }});
    if real {
        dependencies["fabric-loader"] = json!("0.15.7");
    }
    let index = json!({"formatVersion":1,"game":"minecraft","name":manifest.name,"versionId":"1.0","files":[],"dependencies":dependencies});
    write_zip_files(
        &root.join(&manifest.payload),
        root,
        &[],
        &[
            (
                "modrinth.index.json".into(),
                serde_json::to_vec(&index).unwrap(),
            ),
            (
                "overrides/config/portable.txt".into(),
                b"portable content".to_vec(),
            ),
        ],
        &ArchiveLimits::default(),
        &CancellationToken::new(),
        |_, _, _| {},
    )?;
    write(
        &root.join(personalization),
        &serde_json::to_vec(&json!({
            "themeMode":"light","accent":"emerald","backgroundBlur":12,"backgroundOpacity":0.4,
            "backgroundImage":background
        }))
        .unwrap(),
    )?;
    write(
        &root.join(background),
        b"\x89PNG\r\n\x1a\npath-only-fixture",
    )?;
    Ok(manifest)
}

fn seed_base(directory: &Path) -> Result<(), String> {
    let java = directory.join("fixture-java");
    write(&java, b"deterministic cache; never executed")?;
    let data = json!({"minecraftJson":{"id":"1.20.1","type":"release","libraries":[]},
        "minecraftVersion":"1.20.1","minecraftJava":{"files":[],"path":java},
        "loaderType":null,"loaderVersionId":null});
    write(
        &directory.join("gameData.json"),
        &serde_json::to_vec(&data).unwrap(),
    )?;
    write(
        &directory.join("versions/1.20.1/1.20.1.json"),
        &serde_json::to_vec(&data["minecraftJson"]).unwrap(),
    )
}

fn attach_seed(
    app: &tauri::App,
    cancel: bool,
    record_failure: Option<PathBuf>,
    real_cache: Option<PathBuf>,
) -> tauri::EventId {
    let handle = app.handle().clone();
    app.listen(install::INSTALL_EVENT, move |event| {
        let value: Value = serde_json::from_str(event.payload()).unwrap();
        if value["type"] != "stage" || value["stage"] != "检查运行环境" {
            return;
        }
        let id = value["taskId"].as_str().unwrap();

        let snapshot = serde_json::to_value(app_tasks::app_tasks_snapshot()).unwrap();
        let target = snapshot
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["taskId"] == id)
            .unwrap()["target"]
            .as_str()
            .unwrap();
        let target = PathBuf::from(target);
        let seeded = if let Some(source) = &real_cache {
            move_cache(source, &target)
        } else {
            seed_base(&target)
        };
        if let Err(error) = seeded {
            panic!("cache seed failed: {error}");
        }
        if cancel {
            // Stage 的同步回调持有生命周期锁；取消必须在回调返回后请求。
            let id = id.to_owned();
            std::thread::spawn(move || install::install_cancel(id));
        }
        eprintln!("[portable-wry] seeded task={id} cancel={cancel}");
        if let Some(record) = &record_failure {
            std::fs::create_dir(record).unwrap();
        }
        assert!(app_paths::config_dir(&handle).is_ok());
    })
}

fn move_cache(source: &Path, target: &Path) -> Result<(), String> {
    let old = source.to_string_lossy();
    let new = target.to_string_lossy();
    let mut data = read_json(&source.join("gameData.json"))?;
    fn rebase(value: &mut Value, old: &str, new: &str) {
        match value {
            Value::String(text) => {
                *text = text
                    .replace(old, new)
                    .replace(&old.replace('\\', "/"), &new.replace('\\', "/"));
            }
            Value::Array(items) => items.iter_mut().for_each(|value| rebase(value, old, new)),
            Value::Object(items) => items.values_mut().for_each(|value| rebase(value, old, new)),
            _ => {}
        }
    }
    rebase(&mut data, &old, &new);
    for name in ["assets", "libraries", "runtime", "versions", "loader"] {
        std::fs::rename(source.join(name), target.join(name))
            .map_err(|e| format!("move {name}: {e}"))?;
    }
    write(
        &target.join("gameData.json"),
        &serde_json::to_vec(&data).unwrap(),
    )
}

async fn import(app: &tauri::App) -> Result<Value, String> {
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = events.clone();
    let listener = app.listen(install::INSTALL_EVENT, move |event| {
        let value: Value = serde_json::from_str(event.payload()).unwrap();
        if value["type"] == "done" {
            observed.lock().unwrap().push(value);
        }
    });
    let id = portable_bootstrap::portable_bootstrap_start(app.handle().clone())
        .await?
        .ok_or("expected new task")?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let result = loop {
        let terminal = events
            .lock()
            .unwrap()
            .iter()
            .find(|value| value["taskId"] == id)
            .cloned();
        if let Some(terminal) = terminal {
            break Ok(terminal);
        }
        if Instant::now() >= deadline {
            install::install_cancel(id.clone());
            break Err("portable task timeout".into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    app.unlisten(listener);
    // Done 发布在 ImportGuard drop 之前；等回调任务退出后再尝试重试。
    tokio::time::sleep(Duration::from_millis(30)).await;
    result
}

#[test]
#[ignore = "真实 Wry 本地导入原始名称/唯一终态/提交，以及目录 create/rename/delete；仅基础缓存为确定性种子"]
fn live_local_import_name_and_root_lifecycle() -> Result<(), String> {
    let app = app(&format!(
        "com.youtu.nowhere.r6localimport.{}",
        store::random_token()
    ))?;
    let root =
        std::env::temp_dir().join(format!("nowhere-r6-local-import-{}", store::random_token()));
    let package = root.join("fixture");
    let manifest = fixture(&package, app_paths::DATA_DIRECTORY, false)?;
    app.manage(app_paths::TestExecutableRoot(root.clone()));
    let configuration = app_paths::config_dir(app.handle())?;
    let dir = instances::instance_create(app.handle().clone(), root.to_string_lossy().into())?;
    let game = PathBuf::from(&dir);
    assert!(game.is_dir());
    assert_eq!(game.file_name().unwrap(), app_paths::GAME_DIRECTORY);
    let label = "隔离验收目录";
    instances::instance_root_rename(app.handle().clone(), dir.clone(), label.into())?;
    let mut registry =
        instances::registry_from_text(&instances::instances_get(app.handle().clone()));
    assert_eq!(registry.roots, vec![dir.clone()]);
    assert_eq!(registry.labels.get(&dir).map(String::as_str), Some(label));
    assert!(game.is_dir());
    assert!(!root.join(label).exists());
    registry.active = Some(dir.clone());
    instances::instances_set(app.handle().clone(), instances::registry_to_text(&registry))?;
    game::game_settings_set(
        app.handle().clone(),
        json!({"gameDir":dir,"extra":"keep"}).to_string(),
    )?;

    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = events.clone();
    let listener = app.listen(install::INSTALL_EVENT, move |event| {
        observed
            .lock()
            .unwrap()
            .push(serde_json::from_str(event.payload()).unwrap());
    });
    let archive = package
        .join(&manifest.payload)
        .to_string_lossy()
        .into_owned();
    let before = serde_json::to_value(app_tasks::app_tasks_snapshot()).unwrap();
    for invalid in [" 原始名称 ", "../outside", "invalid.", "CON"] {
        let error = install::modpack_import(
            app.handle().clone(),
            archive.clone(),
            dir.clone(),
            invalid.into(),
        )
        .unwrap_err();
        assert!(error.contains("实例名称"), "{invalid:?}: {error}");
        assert_eq!(
            serde_json::to_value(app_tasks::app_tasks_snapshot()).unwrap(),
            before
        );
        assert!(events.lock().unwrap().is_empty());
        assert!(!game.join("versions").exists());
    }

    let name = "自定义导入名称 R6";
    let seed = attach_seed(&app, false, None, None);
    let id = install::modpack_import(app.handle().clone(), archive, dir.clone(), name.into())?;
    let result = tauri::async_runtime::block_on(async {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(done) = events
                .lock()
                .unwrap()
                .iter()
                .find(|event| event["type"] == "done" && event["taskId"] == id)
                .cloned()
            {
                break Ok(done);
            }
            if Instant::now() >= deadline {
                install::install_cancel(id.clone());
                break Err("local import task timeout".to_string());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    app.unlisten(seed);
    let done = result?;
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(done["cancelled"], false);
    assert_eq!(done["result"]["displayName"], name);
    assert_eq!(done["result"]["versionId"], name);
    assert_eq!(done["result"]["root"], dir);
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["type"] == "done" && event["taskId"] == id)
            .count(),
        1
    );
    app.unlisten(listener);
    let snapshot = serde_json::to_value(app_tasks::app_tasks_snapshot()).unwrap();
    let task = snapshot
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["taskId"] == id)
        .unwrap();
    assert_eq!(task["status"], "done");
    assert_eq!(task["transferredBytes"], 0);
    let installed = game.join("versions").join(name);
    assert!(!named_profiles::is_pending_pack(&installed));
    assert_eq!(
        read_json(&installed.join("nowhere.profile.json"))?["name"],
        name
    );
    let descriptor = read_json(&installed.join(format!("{name}.json")))?;
    assert_eq!(descriptor["nowhereCustom"]["name"], name);
    assert!(descriptor["nowherePack"].is_object());
    assert_eq!(
        std::fs::read(installed.join("config/portable.txt")).unwrap(),
        b"portable content"
    );
    assert!(!game.join("versions/Portable acceptance").exists());
    let scanned: Value = serde_json::from_str(&instances::instance_scan(dir.clone())).unwrap();
    assert_eq!(scanned.as_array().unwrap().len(), 1);
    assert_eq!(scanned[0]["displayName"], name);
    assert_eq!(scanned[0]["id"], name);

    instances::instance_root_delete(app.handle().clone(), dir.clone())?;
    assert!(!game.exists());
    let registry = instances::registry_from_text(&instances::instances_get(app.handle().clone()));
    assert!(registry.roots.is_empty());
    assert!(registry.labels.is_empty());
    assert!(registry.active.is_none());
    let settings = read_json(&configuration.join("game.json"))?;
    assert_eq!(settings["gameDir"], "");
    assert_eq!(settings["extra"], "keep");
    assert!(configuration.is_dir());
    assert!(package.join(&manifest.payload).is_file());
    eprintln!(
        "[local-import-wry] PASS raw invalid names before task + custom name over manifest + one Done + committed disk + zero network bytes + directory create/label-only rename/delete"
    );
    drop(app);
    std::fs::remove_dir_all(root).map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
#[ignore = "真实 Wry 新旧布局首启/个性化/重定位/取消/重试/永久 marker；仅基础缓存为确定性种子"]
fn live_portable_bootstrap_transaction_and_paths() -> Result<(), String> {
    for data_directory in [app_paths::DATA_DIRECTORY, app_paths::LEGACY_DATA_DIRECTORY] {
        let identifier = format!("com.youtu.nowhere.r6portable.{}", store::random_token());
        let original_root =
            std::env::temp_dir().join(format!("nowhere-r6-portable-{}", store::random_token()));
        let manifest = fixture(&original_root, data_directory, false)?;
        let original_root = std::fs::canonicalize(original_root).map_err(|e| e.to_string())?;
        let background = format!("{}/background.png", manifest.data_directory);
        let initial_app = app(&identifier)?;
        let legacy = initial_app
            .path()
            .app_config_dir()
            .map_err(|e| e.to_string())?;
        write(&legacy.join("settings.json"), br#"{"accent":"rose"}"#)?;
        initial_app.manage(app_paths::TestExecutableRoot(original_root.clone()));
        let marker = std::fs::read(original_root.join(portable_pack::MANIFEST_FILE)).unwrap();
        let original_configuration = original_root.join(&manifest.data_directory);
        assert_eq!(
            app_paths::config_dir(initial_app.handle())?,
            original_configuration
        );
        assert_eq!(
            default_game_dir(initial_app.handle())?,
            original_root.join(&manifest.game_directory)
        );
        let mut displayed: Value =
            serde_json::from_str(&settings_get(initial_app.handle().clone())?).unwrap();
        assert_eq!(displayed["themeMode"], "light");
        assert_eq!(displayed["accent"], "emerald");
        assert_eq!(displayed["backgroundBlur"].as_f64(), Some(12.0));
        assert_eq!(displayed["backgroundOpacity"], json!(0.4));
        assert_eq!(
            Path::new(displayed["backgroundImage"].as_str().unwrap()),
            original_root.join(&background)
        );
        displayed["backgroundBlur"] = json!(18);
        settings_set(initial_app.handle().clone(), displayed.to_string())?;
        let saved_settings = std::fs::read(original_configuration.join("settings.json")).unwrap();
        assert_eq!(
            read_json(&original_configuration.join("settings.json"))?["backgroundImage"],
            background
        );
        assert_eq!(read_json(&legacy.join("settings.json"))?["accent"], "rose");
        drop(initial_app);

        // 模拟关闭后整体移动便携包；重开真实 Wry 验证相对路径，而不修改生产根选择逻辑。
        let relocated = original_root.with_file_name(format!(
            "nowhere-r6-portable-relocated-{}",
            store::random_token()
        ));
        std::fs::rename(&original_root, &relocated).map_err(|e| e.to_string())?;
        let root = std::fs::canonicalize(relocated).map_err(|e| e.to_string())?;
        let app = app(&identifier)?;
        app.manage(app_paths::TestExecutableRoot(root.clone()));
        let configuration = root.join(&manifest.data_directory);
        let game_directory = root.join(&manifest.game_directory);
        let installed = game_directory
            .join("versions")
            .join(&manifest.instance_name);
        let settings_path = configuration.join("settings.json");
        let record = configuration.join("portable-import.json");
        assert!(!original_root.exists());
        assert_eq!(app_paths::config_dir(app.handle())?, configuration);
        assert_eq!(default_game_dir(app.handle())?, game_directory);
        let displayed: Value = serde_json::from_str(&settings_get(app.handle().clone())?).unwrap();
        assert_eq!(displayed["themeMode"], "light");
        assert_eq!(displayed["accent"], "emerald");
        assert_eq!(displayed["backgroundBlur"].as_f64(), Some(18.0));
        assert_eq!(displayed["backgroundOpacity"], json!(0.4));
        let resolved_background = Path::new(displayed["backgroundImage"].as_str().unwrap());
        assert_eq!(resolved_background, root.join(&background));
        assert!(resolved_background.is_file());
        assert_eq!(std::fs::read(&settings_path).unwrap(), saved_settings);
        settings_set(app.handle().clone(), displayed.to_string())?;
        assert_eq!(read_json(&settings_path)?["backgroundImage"], background);
        assert_eq!(read_json(&legacy.join("settings.json"))?["accent"], "rose");
        tauri::async_runtime::block_on(async {
            let payload = root.join(&manifest.payload);
            let saved = std::fs::read(&payload).unwrap();
            std::fs::remove_file(&payload).unwrap();
            assert_eq!(import(&app).await?["ok"], false);
            assert!(!record.exists());
            write(&payload, &saved)?;
            let listener = attach_seed(&app, true, None, None);
            let cancelled = import(&app).await?;
            app.unlisten(listener);
            assert_eq!(cancelled["cancelled"], true);
            assert!(!record.exists());
            assert!(
                serde_json::from_str::<Vec<Value>>(&instances::instance_scan(
                    game_directory.to_string_lossy().into()
                ))
                .unwrap()
                .is_empty()
            );
            let listener = attach_seed(&app, false, Some(record.clone()), None);
            let failed_record = import(&app).await?;
            app.unlisten(listener);
            assert_eq!(failed_record["ok"], false);
            assert!(installed.join("config/portable.txt").is_file());
            assert!(record.is_dir());
            assert!(
                portable_bootstrap::portable_bootstrap_start(app.handle().clone())
                    .await?
                    .is_none()
            );
            std::fs::remove_dir(&record).unwrap();
            let status = serde_json::to_value(
                portable_bootstrap::portable_bootstrap_status(app.handle().clone()).await?,
            )
            .unwrap();
            assert_eq!(status["state"], "installed");
            assert_eq!(status["versionId"], manifest.instance_name);
            assert!(record.is_file());
            let completed = read_json(&record)?;
            assert_eq!(completed["packageId"], manifest.package_id);
            assert_eq!(completed["versionId"], manifest.instance_name);
            assert!(
                portable_bootstrap::portable_bootstrap_start(app.handle().clone())
                    .await?
                    .is_none()
            );
            assert_eq!(
                std::fs::read_dir(game_directory.join("versions"))
                    .unwrap()
                    .count(),
                1
            );
            let context = instances::resource_instance_context(
                app.handle().clone(),
                game_directory.to_string_lossy().into(),
                manifest.instance_name.clone(),
            )?
            .ok_or("missing portable resource")?;
            assert_eq!(
                serde_json::to_value(context).unwrap()["resourceDir"],
                installed.to_string_lossy().as_ref()
            );
            Ok::<_, String>(())
        })?;
        assert_eq!(
            std::fs::read(root.join(portable_pack::MANIFEST_FILE)).unwrap(),
            marker
        );
        let registry =
            instances::registry_from_text(&instances::instances_get(app.handle().clone()));
        assert_eq!(
            registry.active,
            Some(game_directory.to_string_lossy().into())
        );
        assert_eq!(read_json(&settings_path)?["backgroundImage"], background);
        let other_directory = if data_directory == app_paths::DATA_DIRECTORY {
            app_paths::LEGACY_DATA_DIRECTORY
        } else {
            app_paths::DATA_DIRECTORY
        };
        assert!(!root.join(other_directory).exists());
        let marker_path = root.join(portable_pack::MANIFEST_FILE);
        let held_marker = root.join("marker-held.json");
        std::fs::rename(&marker_path, &held_marker).unwrap();
        assert_eq!(app_paths::config_dir(app.handle())?, legacy);
        std::fs::rename(held_marker, marker_path).unwrap();
        eprintln!(
            "[portable-wry] layout={data_directory} marker priority + personalization relocation + relative background + parse retry + cancel cleanup + commit/record recovery + import-once + resource identity passed"
        );
        drop(app);
        std::fs::remove_dir_all(&legacy).map_err(|e| e.to_string())?;
        std::fs::remove_dir_all(root).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[test]
#[ignore = "真实 Wry 无manifest首运行/旧配置优先/只读错误；全部使用唯一identifier和临时目录"]
fn live_ordinary_first_run_and_legacy_paths() -> Result<(), String> {
    let identifier = format!("com.youtu.nowhere.r6ordinary.{}", store::random_token());
    let app = app(&identifier)?;
    let legacy = app.path().app_config_dir().map_err(|e| e.to_string())?;
    assert_eq!(
        legacy.file_name().and_then(|value| value.to_str()),
        Some(identifier.as_str())
    );
    assert!(!legacy.join("settings.json").exists());
    let root = std::env::temp_dir().join(format!("nowhere-r6-ordinary-{}", store::random_token()));
    std::fs::create_dir(&root).map_err(|e| e.to_string())?;
    app.manage(app_paths::TestExecutableRoot(root.clone()));
    assert!(app_paths::portable_root(app.handle())?.is_none());
    let configuration = root.join(app_paths::DATA_DIRECTORY);
    let game_directory = root.join(app_paths::GAME_DIRECTORY);
    assert_eq!(app_paths::config_dir(app.handle())?, configuration);
    assert_eq!(settings_get(app.handle().clone())?, "{}");
    settings_set(
        app.handle().clone(),
        json!({"accent":"emerald"}).to_string(),
    )?;
    assert_eq!(
        PathBuf::from(instances::instance_ensure_default(app.handle().clone())?),
        game_directory
    );
    assert!(game_directory.is_dir());
    let saved = std::fs::read(configuration.join("settings.json")).map_err(|e| e.to_string())?;
    assert_eq!(
        read_json(&configuration.join("settings.json"))?["accent"],
        "emerald"
    );
    assert!(!legacy.join("settings.json").exists());

    // Windows 只读目标让真实原子替换失败；先恢复权限，再断言，避免失败污染临时目录。
    let settings_path = configuration.join("settings.json");
    let original_permissions = std::fs::metadata(&settings_path)
        .map_err(|e| e.to_string())?
        .permissions();
    let mut readonly = original_permissions.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&settings_path, readonly).map_err(|e| e.to_string())?;
    let denied = settings_set(app.handle().clone(), json!({"accent":"rose"}).to_string());
    std::fs::set_permissions(&settings_path, original_permissions).map_err(|e| e.to_string())?;
    assert!(denied.unwrap_err().contains("保存文件失败"));
    assert_eq!(std::fs::read(&settings_path).unwrap(), saved);
    let held = root.join("held-configuration");
    std::fs::rename(&configuration, &held).map_err(|e| e.to_string())?;
    std::fs::write(&configuration, b"blocked-directory").map_err(|e| e.to_string())?;
    let denied_directory = app_paths::config_dir(app.handle());
    std::fs::remove_file(&configuration).map_err(|e| e.to_string())?;
    std::fs::rename(&held, &configuration).map_err(|e| e.to_string())?;
    assert!(denied_directory.unwrap_err().contains("有写入权限的文件夹"));
    assert!(!legacy.join("settings.json").exists());

    let old_game = root.join("existing-game");
    write(&old_game.join("saves/sentinel.txt"), b"keep existing save")?;
    write(&legacy.join("settings.json"), br#"{"accent":"rose"}"#)?;
    write(
        &legacy.join("game.json"),
        json!({"gameDir":old_game}).to_string().as_bytes(),
    )?;
    assert_eq!(app_paths::config_dir(app.handle())?, legacy);
    assert_eq!(read_json(&legacy.join("settings.json"))?["accent"], "rose");
    assert_eq!(
        serde_json::from_str::<Value>(&settings_get(app.handle().clone())?).unwrap()["accent"],
        "rose"
    );
    assert_eq!(
        PathBuf::from(instances::instance_ensure_default(app.handle().clone())?),
        old_game
    );
    write(&legacy.join("settings.json"), b"{broken")?;
    assert_eq!(app_paths::config_dir(app.handle())?, legacy);
    assert_eq!(settings_get(app.handle().clone())?, "{}");
    assert_eq!(
        std::fs::read(legacy.join("settings.json")).unwrap(),
        b"{broken"
    );
    assert_eq!(
        std::fs::read(configuration.join("settings.json")).unwrap(),
        saved
    );
    assert_eq!(
        std::fs::read(old_game.join("saves/sentinel.txt")).unwrap(),
        b"keep existing save"
    );
    assert!(!root.join(portable_pack::MANIFEST_FILE).exists());
    assert!(!configuration.join("portable-import.json").exists());
    eprintln!(
        "[ordinary-wry] no-manifest first-run settings + .minecraft created; readonly/blocked writes reject without fallback; existing/broken legacy wins and save preserved"
    );
    drop(app);
    std::fs::remove_dir_all(&legacy).map_err(|e| e.to_string())?;
    std::fs::remove_dir_all(&root).map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
#[ignore = "真实磁盘 settings/favoriteAt 跨子进程；显式单线程执行"]
fn live_instance_settings_and_favorite_restart() -> Result<(), String> {
    const TEST: &str = "portable_live_tests::live_instance_settings_and_favorite_restart";
    let mode = std::env::var("NOWHERE_R6_DISK_MODE").ok();
    if mode.is_none() {
        let root = std::env::temp_dir().join(format!("nowhere-r6-disk-{}", store::random_token()));
        for mode in ["write", "reload"] {
            let output =
                std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
                    .args([
                        TEST,
                        "--exact",
                        "--ignored",
                        "--nocapture",
                        "--test-threads=1",
                    ])
                    .env("NOWHERE_R6_DISK_MODE", mode)
                    .env("NOWHERE_R6_DISK_ROOT", &root)
                    .stdin(std::process::Stdio::null())
                    .output()
                    .map_err(|e| e.to_string())?;
            eprintln!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if !output.status.success() {
                return Err(format!("disk {mode} failed"));
            }
        }
        std::fs::remove_dir_all(root).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let root = PathBuf::from(std::env::var_os("NOWHERE_R6_DISK_ROOT").ok_or("missing root")?);
    if !root.is_absolute()
        || !root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .starts_with("nowhere-r6-disk-")
    {
        return Err("invalid fixture root".into());
    }
    let dir = root.to_string_lossy().into_owned();
    let id = "disk-instance";
    let descriptor = root.join("versions/disk-instance/disk-instance.json");
    let settings_file = root.join("versions/disk-instance/nowhere.settings.json");
    if mode.as_deref() == Some("write") {
        write(
            &descriptor,
            br#"{"id":"1.20.1","type":"release","keep":"original"}"#,
        )?;
        version_settings::version_rename(
            dir.clone(),
            id.into(),
            "Display".into(),
            "Description".into(),
            "Group".into(),
            true,
        )?;
        let favorite = read_json(&descriptor)?["nowhereCustom"]["favoriteAt"].clone();
        assert!(favorite.as_u64().unwrap() > 0);
        version_settings::version_rename(
            dir.clone(),
            id.into(),
            "Updated".into(),
            "Description".into(),
            "Group".into(),
            true,
        )?;
        assert_eq!(
            read_json(&descriptor)?["nowhereCustom"]["favoriteAt"],
            favorite
        );
        write(
            &root.join("expected-favorite.json"),
            favorite.to_string().as_bytes(),
        )?;
        let global = json!({"disableFileCheck":true,"disableWrapper":true,"disableLwjglAgent":true,"jvmArgs":"-Dglobal=1"});
        for input in [
            json!({}),
            json!({"disableFileCheck":null,"disableWrapper":null,"disableLwjglAgent":null,"noModUpdates":null}),
            json!({"disableFileCheck":"global","disableWrapper":"global","disableLwjglAgent":"global","noModUpdates":"global"}),
        ] {
            version_settings::version_settings_set(dir.clone(), id.into(), input.to_string())?;
            let local = runtime_settings::read_local(&root, id);
            let resolved = runtime_settings::resolve(&global, &local, None, None)?;
            assert!(
                resolved.disable_file_check
                    && resolved.disable_wrapper
                    && resolved.disable_lwjgl_agent
            );
            assert!(local["noModUpdates"].is_null() || local["noModUpdates"] == "global");
        }
        let explicit = json!({"disableFileCheck":false,"disableWrapper":false,"disableLwjglAgent":false,"noModUpdates":false,"jvmArgs":"","preLaunchCommand":""});
        version_settings::version_settings_set(dir.clone(), id.into(), explicit.to_string())?;
        let before_cancel = std::fs::read(&settings_file).unwrap();
        let mut draft = read_json(&settings_file)?;
        draft["jvmArgs"] = json!("-Dunsaved=1");
        drop(draft);
        assert_eq!(std::fs::read(&settings_file).unwrap(), before_cancel);
    } else {
        let expected = read_json(&root.join("expected-favorite.json"))?;
        assert_eq!(
            read_json(&descriptor)?["nowhereCustom"]["favoriteAt"],
            expected
        );
        assert_eq!(read_json(&descriptor)?["keep"], "original");
        let global = json!({"disableFileCheck":true,"disableWrapper":true,"disableLwjglAgent":true,"jvmArgs":"-Dglobal=1"});
        let local = runtime_settings::read_local(&root, id);
        let resolved = runtime_settings::resolve(&global, &local, None, None)?;
        assert!(
            !resolved.disable_file_check
                && !resolved.disable_wrapper
                && !resolved.disable_lwjgl_agent
        );
        assert!(resolved.jvm_args.is_empty());
        version_settings::version_settings_set(dir.clone(), id.into(), "{}".into())?;
        assert!(
            runtime_settings::resolve(
                &global,
                &runtime_settings::read_local(&root, id),
                None,
                None
            )?
            .disable_wrapper
        );
        version_settings::version_rename(
            dir,
            id.into(),
            "Updated".into(),
            "".into(),
            "".into(),
            false,
        )?;
        assert!(read_json(&descriptor)?["nowhereCustom"]["favoriteAt"].is_null());
    }
    eprintln!(
        "[disk-restart] mode={:?} pid={} inheritance/null/global/false/empty/reset/favoriteAt passed",
        mode,
        std::process::id()
    );
    Ok(())
}

pub struct LaunchPid(pub std::sync::atomic::AtomicU32);

fn native_launch_snapshot(java: &Path, pid: u32) -> Result<Value, String> {
    let script = r#"
$ErrorActionPreference='Stop'
$name=$env:NOWHERE_TEST_JAVA
if ($name.StartsWith('\\?\UNC\')) { $name='\\'+$name.Substring(8) }
elseif ($name.StartsWith('\\?\')) { $name=$name.Substring(4) }
$key=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Microsoft\DirectX\UserGpuPreferences')
$gpu=$null
if ($key) { try { $gpu=$key.GetValue($name,$null) } finally { $key.Dispose() } }
$result=@{ gpu=$gpu }
if ($env:NOWHERE_TEST_PID -ne '0') {
    $process=Get-Process -Id ([int]$env:NOWHERE_TEST_PID)
    $native=Get-CimInstance Win32_Process -Filter ('ProcessId='+$env:NOWHERE_TEST_PID)
    $result.priority=$process.PriorityClass.ToString()
    $result.title=$process.MainWindowTitle
    $result.jvmMarker=$native.CommandLine.Contains('-Dnowhere.r6=accepted')
}
$result | ConvertTo-Json -Compress
"#;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("NOWHERE_TEST_JAVA", java)
        .env("NOWHERE_TEST_PID", pid.to_string())
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "native snapshot failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())
}

#[test]
#[ignore = "使用已授权隔离 MC1.16.5/Fabric 缓存：真实导入、生产 run_launch、皮肤包渲染日志"]
fn live_portable_real_cache_launch_and_skin() -> Result<(), String> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};
    let source = PathBuf::from(
        std::env::var_os("NOWHERE_R6_REAL_CACHE")
            .ok_or("请设置 NOWHERE_R6_REAL_CACHE 为隔离缓存绝对路径")?,
    );
    let root = PathBuf::from(
        std::env::var_os("NOWHERE_R6_REAL_ROOT")
            .ok_or("请设置 NOWHERE_R6_REAL_ROOT 为独立验收目录")?,
    );
    if !source.is_absolute() || !root.is_absolute() || source == root {
        return Err("验收路径必须独立且为绝对路径".into());
    }
    let app = app(&format!(
        "com.youtu.nowhere.r6real.{}",
        store::random_token()
    ))?;
    if !root.join(portable_pack::MANIFEST_FILE).exists() {
        fixture(&root, app_paths::DATA_DIRECTORY, true)?;
    }
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let manifest = portable_pack::read_manifest(&root)?.ok_or("missing portable manifest")?;
    let configuration = root.join(&manifest.data_directory);
    app.manage(app_paths::TestExecutableRoot(root.clone()));
    app.manage(LaunchPid(AtomicU32::new(0)));
    tauri::async_runtime::block_on(async {
        let initial = serde_json::to_value(
            portable_bootstrap::portable_bootstrap_status(app.handle().clone()).await?,
        )
        .unwrap();
        if initial["state"] != "installed" {
            let listener = attach_seed(&app, false, None, Some(source));
            let done = import(&app).await?;
            app.unlisten(listener);
            eprintln!("[portable-real] import={done}");
            if done["ok"] != true {
                return Err(format!("真实导入失败：{}", done["message"]));
            }
        }
        let game_root = root.join(&manifest.game_directory);
        let version = manifest.instance_name.as_str();
        let directory = game_root.join("versions").join(version);
        let data = read_json(&directory.join("gameData.json"))?;
        let java = PathBuf::from(
            data["minecraftJava"]["path"]
                .as_str()
                .ok_or("missing java")?,
        );
        if !java.is_file() {
            return Err(format!("Java不存在：{}", java.display()));
        }
        let observe = std::env::var_os("NOWHERE_R6_OBSERVE_GAME").is_some();
        let custom_skin = std::env::var_os("NOWHERE_R6_CUSTOM_SKIN").map(PathBuf::from);
        let native = std::env::var_os("NOWHERE_R6_NATIVE_FLAGS").is_some();
        let original_gpu = if native {
            Some(native_launch_snapshot(&java, 0)?)
        } else {
            None
        };
        let before_name = format!("r6-prelaunch-{}.txt", store::random_token());
        let before_file = directory.join(&before_name);
        let mut settings = json!({"memoryMode":"custom","memoryMb":1024,
            "versionIsolation":true,"disableWrapper":true,"disableLwjglAgent":true,
            "offlineSkin":if custom_skin.is_some() { "custom" } else { "steve" },
            "offlineSkinPath":custom_skin,"offlineSkinSlim":false,"memoryManagement":"default",
            "windowMode":"custom","windowWidth":854,"windowHeight":480});
        if native {
            settings["windowTitle"] = json!("Nickel native acceptance");
            settings["processPriority"] = json!("high");
            settings["highPerformanceGpu"] = json!(true);
            settings["memoryOptimize"] = json!(true);
            settings["jvmArgs"] = json!("-Dnowhere.r6=accepted");
            settings["preLaunchCommand"] = json!(format!("(echo accepted& cd)>{before_name}"));
        }
        game::game_settings_set(app.handle().clone(), settings.to_string())?;
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed = lines.clone();
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let closed_event = closed.clone();
        let memory_optimized = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let memory_event = memory_optimized.clone();
        let log_path = root.join(if native {
            "game-native.log"
        } else if observe {
            "game-observe.log"
        } else {
            "game-launch.log"
        });
        let log = Arc::new(Mutex::new(
            std::fs::File::create(&log_path).map_err(|e| e.to_string())?,
        ));
        let listener = app.listen("launcher-event", move |event| {
            let value: Value = serde_json::from_str(event.payload()).unwrap();
            if value["type"] == "close" {
                closed_event.store(true, Ordering::Release);
            }
            if value["type"] == "stage" && value["message"] == "已回收启动器工作集，正在启动游戏"
            {
                memory_event.store(true, Ordering::Release);
            }
            if let Some(line) = value["line"].as_str() {
                let _ = writeln!(log.lock().unwrap(), "{line}");
                observed.lock().unwrap().push(line.into());
            }
        });
        eprintln!(
            "[portable-real] production run_launch root={} java={}",
            game_root.display(),
            java.display()
        );
        let result = async {
            tokio::time::timeout(Duration::from_secs(90), run_launch(app.handle().clone(), ResolvedLaunch {
                version: version.into(), username: "PortableTest".into(), memory_min: "512M".into(), memory_max: "1G".into(), game_dir: Some(game_root.to_string_lossy().into())
            })).await.map_err(|_| "production launch timeout")??;
            let deadline = Instant::now() + Duration::from_secs(90);
            loop {
                if closed.load(Ordering::Acquire) { return Err("游戏渲染前退出，请查看游戏日志".into()); }
                let (renderer, skin) = {
                    let lines = lines.lock().unwrap();
                    (lines.iter().any(|line| line.contains("Created:") && line.contains("textures/atlas/")),
                     lines.iter().any(|line| line.contains("Reloading ResourceManager") && line.contains("NoWheRE-offline-skin")))
                };
                if renderer && skin { break; }
                if Instant::now() >= deadline { return Err("未观测到皮肤包加载和渲染器初始化，请查看游戏日志".into()); }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let pack = directory.join("resourcepacks/NoWheRE-offline-skin");
            for texture in ["steve.png", "alex.png"] {
                let bytes = std::fs::read(pack.join("assets/minecraft/textures/entity").join(texture)).map_err(|e| e.to_string())?;
                minecraft_java_rs_core::utils::image::decode_skin_png(&bytes)?;
            }
            assert!(configuration.join("portable-import.json").is_file());
            assert!(root.join(portable_pack::MANIFEST_FILE).is_file());
            eprintln!("[portable-real] PASS production launch + ResourceManager skin pack + atlas renderer; log={}", log_path.display());
            if native {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let pid = app.state::<LaunchPid>().0.load(Ordering::Acquire);
                let snapshot = native_launch_snapshot(&java, pid)?;
                let before = std::fs::read_to_string(&before_file).map_err(|e| e.to_string())?;
                let expected_directory = directory.to_string_lossy().trim_start_matches(r"\\?\").to_string();
                let gpu = snapshot["gpu"].as_str().unwrap_or_default();
                if snapshot["priority"] != "AboveNormal" || snapshot["title"] != "Nickel native acceptance"
                    || snapshot["jvmMarker"] != true || !gpu.split(';').any(|part| part == "GpuPreference=2")
                    || !memory_optimized.load(Ordering::Acquire) || !before.contains("accepted")
                    || !before.lines().any(|line| line.trim().eq_ignore_ascii_case(&expected_directory)) {
                    return Err(format!("native flags mismatch: {snapshot}; memory={}; before={before:?}", memory_optimized.load(Ordering::Acquire)));
                }
                eprintln!("[native-flags] running={snapshot}; memoryOptimize=true; beforeLaunchCwd=true");
            }
            if observe {
                // 有界保留本次子进程供原生角色界面观察；截图结论由人工核验，不由标记文件代替。
                let pid = app.state::<LaunchPid>().0.load(Ordering::Acquire);
                std::fs::write(root.join("observe-pid.txt"), pid.to_string()).map_err(|e| e.to_string())?;
                let done = root.join("observe-done");
                let deadline = Instant::now() + Duration::from_secs(300);
                while !done.exists() && !closed.load(Ordering::Acquire) && Instant::now() < deadline {
                    eprintln!("[portable-real] observe pid={pid}; capture character before deadline");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                if done.exists() { std::fs::remove_file(done).map_err(|e| e.to_string())?; }
            }
            Ok::<_, String>(())
        }.await;
        let pid = app.state::<LaunchPid>().0.load(Ordering::Acquire);
        if pid != 0 && !closed.load(Ordering::Acquire) {
            // 只终止本次生产入口刚创建的 Java；不按进程名匹配。
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdin(std::process::Stdio::null())
                .output();
        }
        for _ in 0..50 {
            if pid == 0 || closed.load(Ordering::Acquire) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        app.unlisten(listener);
        if let Some(original) = original_gpu {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let restored = native_launch_snapshot(&java, 0)?;
            if restored["gpu"] != original["gpu"] {
                return Err(format!("GPU preference not restored: {restored}"));
            }
            eprintln!("[native-flags] restored={restored}");
        }
        result
    })
}
