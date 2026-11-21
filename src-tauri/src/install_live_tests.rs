//! 显式运行的 Windows 真后端验收：公开小包、独立根目录、已有 Java；不改产品入口。
//! 设置 NICKEL_PACK_TEST_JAVA_HOME_8 后用 --ignored --test-threads=1 执行。
use super::*;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Listener;

// tauri-build 的 link-arg-bins 不覆盖 lib 单测；此模块只在 Windows cfg(test) 编译。
#[link(name = "resource.lib", kind = "static", modifiers = "+verbatim")]
unsafe extern "C" {}

fn copy_existing_runtime(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err("测试 Java 目录含链接，请提供完整的本机运行时目录".into());
        }
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            copy_existing_runtime(&entry.path(), &destination)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), destination).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn seed_existing_base(directory: &Path) -> Result<(), String> {
    let Some(cache) = std::env::var_os("NICKEL_PACK_TEST_CACHE").map(PathBuf::from) else {
        return Ok(());
    };
    // 只复用可校验的原版文件，不复制绝对路径缓存、实例身份或整合包文件。
    for folder in ["assets", "libraries", "versions"] {
        let source = cache.join(folder);
        if source.is_dir() {
            copy_existing_runtime(&source, &directory.join(folder))?;
        }
    }
    Ok(())
}

async fn prepare_existing_java(
    task: &TaskContext,
    directory: &Path,
    version: &str,
) -> Result<(), String> {
    use minecraft_java_rs_core::game::{java, version::get_version_json};
    let params = crate::ResolvedLaunch {
        version: version.into(),
        username: "PackTest".into(),
        memory_max: "2G".into(),
        game_dir: Some(directory.to_string_lossy().into()),
    };
    let options = crate::build_options(&params, directory.to_path_buf());
    let client = crate::community_network::client()?;
    let metadata = task
        .transfer
        .scope(get_version_json(&options, &client))
        .await
        .map_err(|e| e.to_string())?;
    let (component, major) = java::java_component(&options, &metadata);
    let key = format!("NICKEL_PACK_TEST_JAVA_HOME_{major}");
    let home = std::env::var_os(&key)
        .map(PathBuf::from)
        .ok_or_else(|| format!("设置 {key} 指向本机已有 Java {major}；测试不下载运行时"))?;
    let binary = home.join("bin/java.exe");
    let probe = std::process::Command::new(&binary)
        .arg("-version")
        .output()
        .map_err(|e| format!("已有 Java 不能启动：{e}"))?;
    if !probe.status.success() {
        return Err("已有 Java 版本检查失败".into());
    }
    let detected = parse_java_version(&String::from_utf8_lossy(&probe.stderr))
        .ok_or("已有 Java 没有返回版本号")?;
    let expected = if major == 8 {
        "1.8.".into()
    } else {
        format!("{major}.")
    };
    if !detected.starts_with(&expected) {
        return Err(format!("需要 Java {major}，已有 Java 为 {detected}"));
    }
    let runtime = directory
        .join("runtime")
        .join(component)
        .join(java::mojang_platform_key(false));
    let source = home.clone();
    let destination = runtime.clone();
    tokio::task::spawn_blocking(move || copy_existing_runtime(&source, &destination))
        .await
        .map_err(|e| e.to_string())??;
    if !java::java_bin_path(&runtime).is_file() {
        return Err("已有 Java 复制后缺少启动文件，停止以避免下载运行时".into());
    }
    eprintln!(
        "[pack-live] java={detected} existing_home={}",
        home.display()
    );
    Ok(())
}

async fn install_real_pack(
    app: &AppHandle,
    task: &TaskContext,
    root: &Path,
    request: &crate::modpacks::PackRequest,
) -> Result<app_tasks::InstalledInstance, String> {
    let plan = crate::modpacks::resolve_plan(app, request, task).await?;
    eprintln!(
        "[pack-live] resolved={} mc={} loader={:?}",
        plan.display_name, plan.minecraft_version, plan.loader
    );
    let loader = parse_loader(plan.loader.as_ref().map(|value| value.kind.as_str()))?;
    let build = plan
        .loader
        .as_ref()
        .map(|value| value.version.as_str())
        .unwrap_or("");
    let mut reserved = named_profiles::reserve_pack(
        root,
        &plan.display_name,
        &plan.minecraft_version,
        loader.clone(),
        build,
    )?;
    task.set_target(&reserved.directory)?;
    if instances::instance_scan(root.to_string_lossy().into()) != "[]" {
        return Err("预留阶段扫描到了未提交实例".into());
    }
    seed_existing_base(&reserved.directory)?;
    prepare_existing_java(task, &reserved.directory, &plan.minecraft_version).await?;
    let launcher = install_base(
        task,
        &reserved.directory,
        &plan.minecraft_version,
        loader,
        build,
    )
    .await?;
    task.check_cancelled()?;
    crate::modpacks::apply_files(&plan, &reserved.directory, task).await?;
    task.check_cancelled()?;
    if instances::instance_scan(root.to_string_lossy().into()) != "[]" {
        return Err("文件阶段扫描到了未提交实例".into());
    }
    let installed = app_tasks::InstalledInstance {
        root: reserved.root.to_string_lossy().into(),
        version_id: reserved.profile.name.clone(),
        display_name: plan.display_name.clone(),
        source: plan.source.clone(),
    };
    let published = installed.clone();
    task.finish_with(move || {
        reserved.commit(
            launcher.game_data().ok_or("基础安装未返回启动数据")?,
            &plan.display_name,
            &plan.source,
        )?;
        Ok(Some(published))
    });
    let scan: serde_json::Value =
        serde_json::from_str(&instances::instance_scan(root.to_string_lossy().into()))
            .map_err(|e| e.to_string())?;
    if scan
        .as_array()
        .is_none_or(|items| items.len() != 1 || items[0]["id"] != installed.version_id)
    {
        return Err(format!("提交后实例未唯一发布：{scan}"));
    }
    let descriptor = root
        .join("versions")
        .join(&installed.version_id)
        .join(format!("{}.json", installed.version_id));
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(descriptor).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if metadata["nickelPack"] != installed.source {
        return Err("提交后丢失整合包来源".into());
    }
    eprintln!(
        "[pack-live] committed platform={} instance={}",
        request.platform, installed.version_id
    );
    Ok(installed)
}

async fn launch_real_pack(
    installed: &app_tasks::InstalledInstance,
    log_path: &Path,
) -> Result<(), String> {
    let root = PathBuf::from(&installed.root);
    let params = crate::ResolvedLaunch {
        version: installed.version_id.clone(),
        username: "PackTest".into(),
        memory_max: "2G".into(),
        game_dir: Some(installed.root.clone()),
    };
    let mut options = crate::build_options(&params, root.clone());
    named_profiles::apply_for_launch(&mut options, &root, &installed.version_id)?;
    let mut launcher = Launcher::new(options);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    let mut log = std::fs::File::create(log_path).map_err(|e| e.to_string())?;
    let mut child = tokio::time::timeout(Duration::from_secs(90), launcher.start(tx))
        .await
        .map_err(|_| "本地整合包启动超时")?
        .map_err(|e| e.to_string())?;
    eprintln!(
        "[pack-live] spawned platform={} pid={:?}",
        installed.source["platform"],
        child.id()
    );
    let result = async {
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut renderer_ready = None;
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                return Err(format!(
                    "游戏提前退出：{status}；日志 {}",
                    log_path.display()
                ));
            }
            if renderer_ready.is_some_and(|at: Instant| at.elapsed() >= Duration::from_secs(5)) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "游戏未在时限内完成渲染器初始化；日志 {}",
                    log_path.display()
                ));
            }
            if let Ok(Some(LaunchEvent::Data(line))) =
                tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
            {
                writeln!(log, "{line}").map_err(|e| e.to_string())?;
                if line.contains("Created:") && line.contains("textures/atlas/") {
                    renderer_ready.get_or_insert_with(Instant::now);
                }
            }
        }
    }
    .await;
    if child.try_wait().map_err(|e| e.to_string())?.is_none() {
        child.kill().await.map_err(|e| e.to_string())?;
    }
    let _ = child.wait().await;
    result?;
    eprintln!(
        "[pack-live] rendered platform={} log={}",
        installed.source["platform"],
        log_path.display()
    );
    Ok(())
}

#[test]
#[ignore = "只复验已提交测试实例的真实启动；需 NICKEL_PACK_TEST_EXISTING"]
fn live_existing_pack_launch() -> Result<(), String> {
    let directory = std::env::var_os("NICKEL_PACK_TEST_EXISTING")
        .map(PathBuf::from)
        .ok_or("设置 NICKEL_PACK_TEST_EXISTING 指向本轮已提交测试实例")?;
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("实例目录名无效")?;
    let root = directory
        .parent()
        .and_then(Path::parent)
        .ok_or("实例根目录无效")?;
    let descriptor: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.join(format!("{name}.json"))).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if !descriptor["nickelPack"].is_object() {
        return Err("指定目录不是已提交整合包实例".into());
    }
    let installed = app_tasks::InstalledInstance {
        root: root.to_string_lossy().into(),
        version_id: name.into(),
        display_name: descriptor["nickelCustom"]["name"]
            .as_str()
            .unwrap_or(name)
            .into(),
        source: descriptor["nickelPack"].clone(),
    };
    tauri::async_runtime::block_on(launch_real_pack(
        &installed,
        &directory.join("integration-launch.log"),
    ))
}

#[test]
#[ignore = "真实平台下载与游戏窗口；需已有 Java 8，单线程执行并保存独立测试根"]
fn live_mr_cf_pack_install_and_launch() -> Result<(), String> {
    let parent = std::env::var_os("NICKEL_PACK_TEST_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let selected_platform = std::env::var("NICKEL_PACK_TEST_PLATFORM").ok();
    if selected_platform
        .as_deref()
        .is_some_and(|value| !matches!(value, "modrinth" | "curseforge"))
    {
        return Err("NICKEL_PACK_TEST_PLATFORM 仅接受 modrinth 或 curseforge".into());
    }
    let root = parent.join(format!("nickel-pack-live-{}", store::random_token()));
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    eprintln!("[pack-live] root={}", root.display());
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().identifier =
        format!("studio.nickel.launcher.packtest.{}", store::random_token());
    let app = tauri::Builder::default()
        .any_thread()
        .build(context)
        .map_err(|e| e.to_string())?;
    let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let collected = events.clone();
    let progress_time = Arc::new(Mutex::new(Instant::now()));
    app.listen(INSTALL_EVENT, move |event| {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(event.payload()) {
            if value["type"] == "stage" || value["type"] == "done" {
                eprintln!("[pack-live] event={value}");
            } else {
                let mut last = progress_time.lock().unwrap();
                if last.elapsed() >= Duration::from_secs(5) {
                    eprintln!("[pack-live] progress={value}");
                    *last = Instant::now();
                }
            }
            collected.lock().unwrap().push(value);
        }
    });
    let result = tauri::async_runtime::block_on(async {
        let mut failures = Vec::new();
        for (platform, project_id, version_id) in [
            ("modrinth", "1KVo5zza", "kKonWfiD"),
            ("curseforge", "678510", "5143333"),
        ] {
            if selected_platform
                .as_deref()
                .is_some_and(|selected| selected != platform)
            {
                continue;
            }
            let instance_root = root.join(platform);
            std::fs::create_dir_all(&instance_root).map_err(|e| e.to_string())?;
            let request = crate::modpacks::PackRequest {
                platform: platform.into(),
                project_id: project_id.into(),
                version_id: version_id.into(),
                instance_name: None,
            };
            let task = app_tasks::begin(app.handle(), &instance_root.join("pending"), None)?;
            let result = tokio::time::timeout(
                Duration::from_secs(900),
                install_real_pack(app.handle(), &task, &instance_root, &request),
            )
            .await;
            let result = match result {
                Ok(result) => result,
                Err(_) => {
                    app_tasks::cancel(&task.id);
                    Err("整合包安装超过测试时限".into())
                }
            };
            let result = match result {
                Ok(installed) => {
                    launch_real_pack(&installed, &root.join(format!("{platform}-game.log"))).await
                }
                Err(error) => {
                    task.finish(Err(error.clone()));
                    Err(error)
                }
            };
            let terminal_count = events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event["type"] == "done" && event["taskId"] == task.id)
                .count();
            if terminal_count != 1 {
                failures.push(format!("{platform}: 终态数量为 {terminal_count}"));
            }
            if let Err(error) = result {
                eprintln!("[pack-live] FAILED {platform}: {error}");
                failures.push(format!("{platform}: {error}"));
            } else {
                eprintln!("[pack-live] PASSED {platform}");
            }
        }
        std::fs::write(
            root.join("install-events.json"),
            serde_json::to_vec_pretty(&*events.lock().unwrap()).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    });
    eprintln!("[pack-live] evidence={}", root.display());
    result
}

fn acceptance_app(identifier: &str) -> Result<tauri::App, String> {
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().identifier = identifier.into();
    tauri::Builder::default()
        .any_thread()
        .build(context)
        .map_err(|error| error.to_string())
}

#[test]
#[ignore = "真实 Wry settings_set/get 与独立子进程重载；显式单线程执行"]
fn live_settings_persist_and_reload() -> Result<(), String> {
    use crate::network_settings::{self, NetworkSettings, SourcePolicy};
    use tauri::Manager;
    const TEST_NAME: &str = "install::live_tests::live_settings_persist_and_reload";
    const ID_PREFIX: &str = "studio.nickel.launcher.settingstest.";
    let mode = std::env::var("NICKEL_SETTINGS_TEST_MODE").ok();
    let Some(mode) = mode else {
        let identifier = format!("{ID_PREFIX}{}", store::random_token());
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        for mode in ["write", "reload"] {
            let output = std::process::Command::new(&executable)
                .args([
                    TEST_NAME,
                    "--exact",
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("NICKEL_SETTINGS_TEST_MODE", mode)
                .env("NICKEL_SETTINGS_TEST_ID", &identifier)
                .stdin(std::process::Stdio::null())
                .output()
                .map_err(|error| error.to_string())?;
            eprintln!("{}", String::from_utf8_lossy(&output.stdout));
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            if !output.status.success() {
                return Err(format!("设置持久化子进程 {mode} 失败：{}", output.status));
            }
        }
        return Ok(());
    };
    let identifier =
        std::env::var("NICKEL_SETTINGS_TEST_ID").map_err(|error| error.to_string())?;
    if !identifier.starts_with(ID_PREFIX) || !matches!(mode.as_str(), "write" | "reload") {
        return Err("隔离设置测试参数无效".into());
    }
    let app = acceptance_app(&identifier)?;
    let config = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?;
    let cache = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?;
    assert_eq!(
        config.file_name().and_then(|value| value.to_str()),
        Some(identifier.as_str())
    );
    assert_eq!(network_settings::snapshot(), NetworkSettings::default());
    let expected = serde_json::json!({
        "versionListSource": "official_first",
        "fileDownloadSource": "mirror_first",
        "communityApiSource": "official_auto",
        "maxDownloadThreads": 7,
        "downloadLimitBps": 262144,
        "verifyMicrosoftSsl": false,
        "themeMode": "light"
    });
    if mode == "write" {
        assert_eq!(crate::settings_get(app.handle().clone())?, "{}");
        crate::settings_set(app.handle().clone(), expected.to_string())?;
    }
    let text = crate::settings_get(app.handle().clone())?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| error.to_string())?;
    assert_eq!(value, expected);
    let on_disk: serde_json::Value = serde_json::from_slice(
        &std::fs::read(crate::settings_path(app.handle())?).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    assert_eq!(on_disk, expected);
    // 与 run().setup 一致的装配顺序；reload 分支是新进程，未调用 settings_set。
    network_settings::apply(network_settings::from_value(&value)?);
    let actual = network_settings::snapshot();
    assert_eq!(actual.version_list_source, SourcePolicy::OfficialFirst);
    assert_eq!(actual.file_download_source, SourcePolicy::MirrorFirst);
    assert_eq!(actual.community_api_source, SourcePolicy::OfficialAuto);
    assert_eq!(actual.max_download_threads, 7);
    assert_eq!(actual.download_limit_bps, 262144);
    assert!(!actual.verify_microsoft_ssl);
    let task = app_tasks::begin(app.handle(), &config.join("target"), None)?;
    assert_eq!(task.network, actual);
    assert_eq!(task.transfer.list_policy(), SourcePolicy::OfficialFirst);
    assert_eq!(task.transfer.file_policy(), SourcePolicy::MirrorFirst);
    assert!(crate::http::MicrosoftClient::new(actual.verify_microsoft_ssl).is_ok());
    task.finish(Ok(None));
    eprintln!(
        "[settings-persist] mode={mode} pid={} path={} disk={} reassembled={}",
        std::process::id(),
        config.join("settings.json").display(),
        value,
        serde_json::to_string(&actual).map_err(|error| error.to_string())?
    );
    if mode == "reload" {
        drop(task);
        drop(app);
        std::fs::remove_dir_all(&config).map_err(|error| error.to_string())?;
        if cache != config && cache.exists() {
            std::fs::remove_dir_all(cache).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn acceptance_game_data(mc: &str) -> minecraft_java_rs_core::launcher::game_data::GameData {
    serde_json::from_value(serde_json::json!({
        "minecraftJson": {"id": mc, "type": "release"},
        "minecraftVersion": mc, "minecraftJava": {"files": [], "path": ""},
        "loaderType": null, "loaderVersionId": null
    }))
    .unwrap()
}

#[test]
#[ignore = "真实 Wry 终态与五阶段确定性故障注入；不联网，显式单线程执行"]
fn live_pack_transaction_stage_failures_and_cancellations() -> Result<(), String> {
    use std::collections::HashMap;
    use tauri::Manager;
    let app = acceptance_app(&format!(
        "studio.nickel.launcher.transactiontest.{}",
        store::random_token()
    ))?;
    let cache = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?;
    let root = std::env::temp_dir().join(format!("nickel-stage-matrix-{}", store::random_token()));
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let watched = Arc::new(Mutex::new(HashMap::<String, (PathBuf, PathBuf)>::new()));
    let observations = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let watch = watched.clone();
    let observed = observations.clone();
    app.listen(INSTALL_EVENT, move |event| {
        let value: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
        if value["type"] != "done" {
            return;
        }
        let id = value["taskId"].as_str().unwrap();
        let targets = watch.lock().unwrap();
        let (root, workspace) = targets.get(id).unwrap();
        observed.lock().unwrap().push(serde_json::json!({
            "event": value,
            "scanAtDone": instances::instance_scan(root.to_string_lossy().into()),
            "workspaceExistsAtDone": workspace.exists()
        }));
    });
    for stage in ["parse", "base", "dependencies", "overrides", "commit"] {
        for cancelled in [false, true] {
            let case = root.join(format!("{stage}-{cancelled}"));
            let mut existing = named_profiles::reserve_pack(&case, "Existing", "1.20.1", None, "")?;
            existing.commit(
                &acceptance_game_data("1.20.1"),
                "Existing",
                &serde_json::json!({"fixture":true}),
            )?;
            let sentinel = existing.directory.join("keep.txt");
            std::fs::write(&sentinel, b"existing instance").map_err(|error| error.to_string())?;
            let before = instances::instance_scan(case.to_string_lossy().into());
            let task = app_tasks::begin(app.handle(), &case.join("pending"), None)?;
            watched
                .lock()
                .unwrap()
                .insert(task.id.clone(), (case.clone(), task.workspace.clone()));
            task.stage(stage, "确定性阶段注入");
            let mut reserved = None;
            let mut reserved_path = None;
            let result = (|| -> Result<(), String> {
                if stage == "parse" {
                    if cancelled {
                        install_cancel(task.id.clone());
                        task.check_cancelled()?;
                    }
                    let error =
                        crate::modpacks::parse_modrinth_manifest(b"invalid manifest").unwrap_err();
                    return Err(error.to_string());
                }
                let manifest = crate::modpacks::parse_modrinth_manifest(br#"{"formatVersion":1,"game":"minecraft","name":"Candidate","dependencies":{"minecraft":"1.20.1"},"files":[]}"#)
                    .map_err(|error| error.to_string())?;
                let candidate = named_profiles::reserve_pack(
                    &case,
                    &manifest.name,
                    &manifest.minecraft_version,
                    None,
                    "",
                )?;
                let directory = candidate.directory.clone();
                task.set_target(&directory)?;
                reserved_path = Some(directory.clone());
                reserved = Some(candidate);
                // 阶段注入只替代联网工作，保留真实目录、取消与最终事务调用。
                for (step, relative) in [
                    ("base", "libraries/base.part"),
                    ("dependencies", "mods/dependency.part"),
                    ("overrides", "config/partial.txt"),
                ] {
                    let path = directory.join(relative);
                    std::fs::create_dir_all(path.parent().unwrap())
                        .map_err(|error| error.to_string())?;
                    std::fs::write(&path, b"partial stage output")
                        .map_err(|error| error.to_string())?;
                    assert_eq!(
                        instances::instance_scan(case.to_string_lossy().into()),
                        before
                    );
                    if stage == step {
                        if cancelled {
                            install_cancel(task.id.clone());
                            task.check_cancelled()?;
                        }
                        if step == "overrides" {
                            let archive = task.workspace.join("invalid.zip");
                            std::fs::write(&archive, b"invalid archive")
                                .map_err(|error| error.to_string())?;
                            return minecraft_java_rs_core::utils::archive::extract_zip_subtrees_with_existing(
                                &archive, &directory, &["overrides".into()], &[], &[],
                                &Default::default(), task.transfer.cancellation(),
                            ).map(|_| ());
                        }
                        return Err(format!("injected {step} failure"));
                    }
                }
                // 提交使用真实文件系统失败：把描述文件目标占用为目录。
                let descriptor = directory.join(format!("{}.json", manifest.name));
                std::fs::create_dir(&descriptor).map_err(|error| error.to_string())?;
                if cancelled {
                    install_cancel(task.id.clone());
                }
                Ok(())
            })();
            let commit_attempted = std::cell::Cell::new(false);
            task.finish_transaction(
                reserved,
                |reserved| {
                    result?;
                    commit_attempted.set(true);
                    let reserved = reserved.as_mut().ok_or("missing reservation")?;
                    reserved.commit(
                        &acceptance_game_data("1.20.1"),
                        "Candidate",
                        &serde_json::json!({"fixture":true}),
                    )?;
                    Ok(Some(app_tasks::InstalledInstance {
                        root: reserved.root.to_string_lossy().into(),
                        version_id: reserved.profile.name.clone(),
                        display_name: "Candidate".into(),
                        source: serde_json::json!({"fixture":true}),
                    }))
                },
                |reserved| match reserved.as_mut() {
                    Some(reserved) => reserved.cleanup(),
                    None => Ok(()),
                },
            );
            assert_eq!(commit_attempted.get(), stage == "commit" && !cancelled);
            task.finish(Err("duplicate finish must be ignored".into()));
            install_cancel(task.id.clone());
            let all = observations.lock().unwrap();
            let terminal: Vec<_> = all
                .iter()
                .filter(|value| value["event"]["taskId"] == task.id)
                .collect();
            assert_eq!(terminal.len(), 1);
            let terminal = terminal[0];
            assert_eq!(terminal["event"]["ok"], false);
            assert_eq!(terminal["event"]["cancelled"], cancelled);
            assert!(terminal["event"]["result"].is_null());
            assert!(!terminal["event"]["message"].as_str().unwrap().is_empty());
            assert_eq!(terminal["scanAtDone"], before);
            assert_eq!(terminal["workspaceExistsAtDone"], false);
            assert!(reserved_path.as_ref().is_none_or(|path| !path.exists()));
            assert_eq!(
                std::fs::read(&sentinel).map_err(|error| error.to_string())?,
                b"existing instance"
            );
            assert_eq!(
                std::fs::read_dir(case.join("versions"))
                    .map_err(|error| error.to_string())?
                    .count(),
                1
            );
            assert_eq!(
                std::fs::read_dir(&case)
                    .map_err(|error| error.to_string())?
                    .count(),
                1
            );
            app_tasks::with_idle_directory(&case, || Ok(()))?;
            eprintln!(
                "[transaction-matrix] stage={stage} cancelled={cancelled} unique_done=1 unpublished=true cleaned_before_done=true existing_preserved=true event={}",
                terminal["event"]
            );
        }
    }
    std::fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
    drop(app);
    if cache.exists() {
        std::fs::remove_dir_all(cache).map_err(|error| error.to_string())?;
    }
    eprintln!(
        "[transaction-matrix] PASS 10 cases; real TaskContext/ReservedProfile/scan/events; base/dependency work uses deterministic injection, no downloads"
    );
    Ok(())
}

#[test]
#[ignore = "真实 Wry 下载事件序列与快照；显式单线程执行，不联网"]
fn live_pump_events_resets_batches_without_speed_overwriting_counts() -> Result<(), String> {
    use tauri::Manager;
    let app = acceptance_app(&format!("studio.nickel.launcher.r6pump.{}", store::random_token()))?;
    let root = std::env::temp_dir().join(format!("nickel-r6-pump-{}", store::random_token()));
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let task = app_tasks::begin(app.handle(), &root, None)?;
    let snapshot = || -> Result<serde_json::Value, String> {
        let values = serde_json::to_value(app_tasks::app_tasks_snapshot()).map_err(|e| e.to_string())?;
        values.as_array().and_then(|items| items.iter().find(|item| item["taskId"] == task.id)).cloned().ok_or("测试任务丢失".into())
    };
    tauri::async_runtime::block_on(async {
        let batches = [
            vec![LaunchEvent::Progress { downloaded: 9, total: 10, kind: "libraries".into() }, LaunchEvent::Estimated(8.0)],
            vec![LaunchEvent::Check { current: 1, total: 4, kind: "assets".into() }, LaunchEvent::Speed(1024.0)],
            vec![LaunchEvent::Progress { downloaded: 2, total: 5, kind: "assets".into() }, LaunchEvent::Estimated(3.0)],
            vec![LaunchEvent::Progress { downloaded: 0, total: 2, kind: "java".into() }, LaunchEvent::Speed(512.0)],
            vec![LaunchEvent::Patch("processor".into()), LaunchEvent::Speed(0.0)],
        ];
        for (index, events) in batches.into_iter().enumerate() {
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            for event in events { tx.send(event).await.map_err(|e| e.to_string())?; }
            drop(tx);
            pump_events(task.clone(), rx, "Forge".into()).await;
            let value = snapshot()?;
            match index {
                0 => { assert_eq!(value["filesDone"], 9); assert_eq!(value["remainingMs"], 8000); }
                1 => { assert_eq!(value["stage"], STAGE_VERIFY); assert_eq!(value["filesDone"], 1); assert_eq!(value["filesTotal"], 4); assert!(value["remainingMs"].is_null()); }
                2 => { assert_eq!(value["filesDone"], 2); assert_eq!(value["filesTotal"], 5); assert_eq!(value["remainingMs"], 3000); }
                3 => { assert_eq!(value["detail"], "java"); assert_eq!(value["filesDone"], 0); assert_eq!(value["filesTotal"], 2); assert!(value["remainingMs"].is_null()); }
                _ => { assert_eq!(value["stage"], STAGE_LOADER); assert_eq!(value["filesTotal"], 0); assert!(value["percent"].is_null()); }
            }
            eprintln!("[r6-pump] batch={index} snapshot={value}");
        }
        Ok::<_, String>(())
    })?;
    task.finish(Ok(None));
    let cache = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    drop(task);
    drop(app);
    std::fs::remove_dir_all(root).map_err(|e| e.to_string())?;
    if cache.exists() { std::fs::remove_dir_all(cache).map_err(|e| e.to_string())?; }
    Ok(())
}
