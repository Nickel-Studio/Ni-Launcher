//! 运行时入口回归；Wry 用独立 identifier 与临时实例，不读取用户游戏数据。
use super::*;

fn launch_options() -> LaunchOptions {
    build_options(&ResolvedLaunch {
        version: "1.20.4".into(), username: "Player".into(),
        memory_max: "1G".into(), game_dir: None,
    }, std::env::temp_dir())
}

#[test]
fn third_party_session_keeps_identity_and_requires_matching_server() {
    let mut settings = runtime_settings::resolve(&serde_json::json!({}), &serde_json::json!({}), None).unwrap();
    settings.server_login = "thirdParty".into();
    settings.auth_server = "https://example.org/yggdrasil/".into();
    let account = store::StoredAccount {
        account_type: "littleskin".into(), name: "TestProfile".into(),
        uuid: "12345678-1234-1234-1234-123456789abc".into(),
        server: Some("https://example.org/yggdrasil".into()),
        access_token: Some("test-token".into()), client_token: Some("test-client".into()),
        ..Default::default()
    };
    let mut options = launch_options();
    assert_eq!(apply_account(&mut options, &settings, Some(account.clone())).unwrap().as_deref(), Some("https://example.org/yggdrasil"));
    assert_eq!(options.authenticator.uuid, "12345678123412341234123456789abc");
    assert_eq!(options.authenticator.access_token, "test-token");
    assert_eq!(options.authenticator.client_token.as_deref(), Some("test-client"));
    assert!(options.authenticator.xbox_account.is_none());
    assert!(!options.bypass_offline);
    settings.auth_server = "https://different.example/yggdrasil".into();
    assert!(apply_account(&mut launch_options(), &settings, Some(account.clone())).is_err());
    settings.auth_server.clear();
    let mut expired = account;
    expired.expires_at = Some(1);
    assert!(apply_account(&mut launch_options(), &settings, Some(expired)).is_err());
    assert!(apply_account(&mut launch_options(), &settings, None).is_err());
}
#[tokio::test]
async fn third_party_self_contained_profile_launches_without_mojang_manifest() {
    let root = std::env::temp_dir().join(format!("nickel-third-party-{}", std::process::id()));
    let directory = root.join("versions").join("Pack");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("Pack.json"),
        serde_json::json!({
            "id": "Pack",
            "type": "release",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "downloads": {
                "client": { "sha1": "abc", "size": 10, "url": "https://example.com/client.jar" }
            },
            "libraries": [{ "name": "com.example:demo:1" }]
        })
        .to_string(),
    )
    .unwrap();

    let mut options = launch_options();
    options.path = root.clone();
    options.version = "Pack".into();
    let parsed = minecraft_java_rs_core::game::version::local_version_json(&options)
        .await
        .expect("PCL 自包含版本应能从本地 JSON 启动");
    assert_eq!(parsed.id, "Pack");
    assert_eq!(parsed.main_class.as_deref(), Some("net.fabricmc.loader.impl.launch.knot.KnotClient"));

    // 加载器描述只有 inheritsFrom：必须继续走父版本合并，不能当完整版本直读。
    let loader = root.join("versions").join("Loader");
    std::fs::create_dir_all(&loader).unwrap();
    std::fs::write(
        loader.join("Loader.json"),
        serde_json::json!({
            "id": "Loader",
            "inheritsFrom": "1.20.1",
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "libraries": []
        })
        .to_string(),
    )
    .unwrap();
    options.version = "Loader".into();
    assert!(minecraft_java_rs_core::game::version::local_version_json(&options).await.is_none());

    let _ = std::fs::remove_dir_all(root);
}



#[test]
fn offline_launch_has_no_auth_server_or_new_prerequisite() {
    let settings = runtime_settings::resolve(&serde_json::json!({}), &serde_json::json!({}), None).unwrap();
    let mut options = launch_options();
    let identity = options.authenticator.uuid.clone();
    assert!(apply_account(&mut options, &settings, None).unwrap().is_none());
    assert_eq!(options.authenticator.uuid, identity);
    assert!(options.bypass_offline);
}

#[test]
fn capabilities_use_frontend_contract_keys() {
    let value = serde_json::to_value(runtime_settings::game_runtime_info()).unwrap();
    let capabilities = &value["capabilities"];
    assert_eq!(capabilities["highPerformanceGpu"], cfg!(windows));
    assert_eq!(capabilities["launchWrapper"], cfg!(windows));
    assert_eq!(capabilities["lwjglAgent"], true);
}

#[cfg(windows)]
#[test]
#[ignore = "真实 Wry 资源目录隔离与失效目标回归；显式单线程执行，不联网"]
fn live_resource_context_isolation_and_missing_target() -> Result<(), String> {
    use tauri::Manager;
    let identifier = format!("studio.nickel.launcher.r6resources.{}", store::random_token());
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().identifier = identifier;
    let app = tauri::Builder::default().any_thread().build(context).map_err(|e| e.to_string())?;
    let root = std::env::temp_dir().join(format!("nickel-r6-resource-{}", store::random_token()));
    let version = "named-instance";
    let directory = root.join("versions").join(version);
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    std::fs::write(directory.join(format!("{version}.json")), serde_json::json!({
        "id": "1.20.4", "type": "release", "nickelCustom": {"name": "Display name"}
    }).to_string()).map_err(|e| e.to_string())?;
    let config = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let result = (|| {
        for isolation in [true, false] {
            game::game_settings_set(app.handle().clone(), serde_json::json!({"versionIsolation": isolation}).to_string())?;
            let actual = instances::resource_instance_context(app.handle().clone(), root.to_string_lossy().into_owned(), version.into())?.ok_or("没有找到测试实例")?;
            let value = serde_json::to_value(actual).map_err(|e| e.to_string())?;
            let expected = if isolation { &directory } else { &root };
            assert_eq!(value["resourceDir"], expected.to_string_lossy().as_ref());
            assert_eq!(value["displayName"], "Display name");
            assert_eq!(value["minecraftVersion"], "1.20.4");
        }
        std::fs::write(directory.join("nickel.settings.json"), r#"{"versionIsolation":true}"#).map_err(|e| e.to_string())?;
        assert_eq!(runtime_settings::resource_directory(app.handle(), &root, version)?, directory);
        std::fs::write(directory.join("nickel.settings.json"), r#"{"versionIsolation":false}"#).map_err(|e| e.to_string())?;
        assert_eq!(runtime_settings::resource_directory(app.handle(), &root, version)?, root);
        let missing = instances::resource_instance_context(app.handle().clone(), root.to_string_lossy().into_owned(), "missing".into())?;
        assert!(missing.is_none(), "失效目标不能回退到共享根目录");
        assert!(runtime_settings::resource_directory(app.handle(), &root, "missing").is_err());
        std::fs::remove_file(directory.join(format!("{version}.json"))).map_err(|e| e.to_string())?;
        assert!(instances::resource_instance_context(app.handle().clone(), root.to_string_lossy().into_owned(), version.into())?.is_none());
        eprintln!("[r6-resource] global-isolation on/off, local override, missing identity and missing JSON passed");
        Ok(())
    })();
    drop(app);
    std::fs::remove_dir_all(&root).map_err(|e| e.to_string())?;
    if config.exists() { std::fs::remove_dir_all(config).map_err(|e| e.to_string())?; }
    result
}
