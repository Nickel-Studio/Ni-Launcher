//! 独立加载器安装器另存为：只接受加载器/版本，地址由服务端官方清单生成。

use minecraft_java_rs_core::net::client::{build_transfer_client, describe_reqwest_error};
use minecraft_java_rs_core::net::transfer::{
    DownloadRequest, FileHashes, RequestCandidate, TransferContext, core_candidates,
    download_validated,
};
use std::sync::Arc;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::{loader_catalog, network_settings};

const INSTALLER_MAX_BYTES: usize = 64 * 1024 * 1024;
const INSTALLER_MAX_UNCOMPRESSED_BYTES: u64 = 256 * 1024 * 1024;
#[cfg(test)]
const INSTALLER_TIMEOUT_SECS: u64 = 90;

fn validate_installer(bytes: &[u8], loader: &str) -> Result<(), String> {
    let required_entries: &[&str] = match loader {
        "forge" | "neoforge" => &["install_profile.json"],
        "optifine" => &["optifine/Installer.class"],
        _ => &[],
    };
    minecraft_java_rs_core::utils::archive::validate_executable_jar(
        bytes,
        INSTALLER_MAX_UNCOMPRESSED_BYTES,
        required_entries,
    )
    .map_err(|e| format!("安装器校验失败，请切换下载源后重试：{e}"))
}

async fn download_installer_to(
    target: std::path::PathBuf,
    candidates: Vec<RequestCandidate>,
    loader: String,
    hashes: FileHashes,
    expected_size: Option<u64>,
    context: &TransferContext,
    progress: Option<minecraft_java_rs_core::net::transfer::ProgressCallback>,
) -> Result<String, String> {
    let client = build_transfer_client(crate::http::TIMEOUT_SECS, false, None)
        .map_err(|e| describe_reqwest_error(&e))?;
    let request = DownloadRequest {
        candidates,
        destination: target.clone(),
        expected_size,
        hashes,
        service: format!("{loader}-installer"),
    };
    let validator = Arc::new(
        move |path: &std::path::Path,
              cancel: &minecraft_java_rs_core::net::transfer::CancellationToken| {
            cancel.check()?;
            let bytes = std::fs::read(path).map_err(|e| format!("读取安装器失败：{e}"))?;
            validate_installer(&bytes, &loader)?;
            cancel.check()
        },
    );
    download_validated(
        &client,
        context,
        &request,
        INSTALLER_MAX_BYTES as u64,
        validator,
        progress,
    )
    .await
    .map_err(|e| format!("下载官方安装器失败，请重试或更换文件下载源：{e}"))?;
    Ok(target.to_string_lossy().into_owned())
}

struct InstallerChoice {
    target: std::path::PathBuf,
    candidates: Vec<RequestCandidate>,
    hashes: FileHashes,
    expected_size: Option<u64>,
    loader: String,
    network: network_settings::NetworkSettings,
}

fn candidate_for(url: &str) -> RequestCandidate {
    if url.contains("bmclapi2.bangbang93.com") || url.contains("bmclapi.bangbang93.com") {
        RequestCandidate::mirror(url)
    } else {
        RequestCandidate::official(url)
    }
}

fn installer_candidates(
    artifact: &minecraft_java_rs_core::models::loader::ResolvedLoaderArtifact,
) -> Vec<RequestCandidate> {
    let (save_url, _) = artifact.save_source();
    let mut candidates = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut push = |candidate: RequestCandidate| {
        if seen.insert((candidate.url.clone(), candidate.is_mirror)) {
            candidates.push(candidate);
        }
    };
    if artifact.kind == minecraft_java_rs_core::models::loader::LoaderArtifactKind::OptiFine {
        // OptiFine 的两条真实来源都已由 Resolver 解析成具体文件地址；浏览器
        // 页面和接口路径不能直接交给下载器。
        push(candidate_for(save_url));
        for url in &artifact.download_candidates {
            push(candidate_for(url));
        }
    } else {
        for candidate in core_candidates(save_url) {
            push(candidate);
        }
    }
    candidates
}

async fn choose_installer(
    app: &AppHandle,
    mc_version: &str,
    loader: &str,
    loader_version: &str,
    network: &network_settings::NetworkSettings,
) -> Result<Option<InstallerChoice>, String> {
    let artifact = loader_catalog::installer_source(
        mc_version.trim(),
        loader,
        loader_version.trim(),
        Some(network_settings::policy_name(network.version_list_source)),
    )
    .await?;
    let (_, file_name) = artifact.save_source();
    let file_name = file_name.to_string();
    let candidates = installer_candidates(&artifact);
    let hashes = FileHashes {
        sha1: artifact.save_sha1.clone().or_else(|| artifact.sha1.clone()),
        md5: artifact.save_md5.clone().or_else(|| artifact.md5.clone()),
        ..FileHashes::default()
    };
    let expected_size = artifact.save_size.or(artifact.size);
    let loader_key = match artifact.kind {
        minecraft_java_rs_core::models::loader::LoaderArtifactKind::Forge => "forge",
        minecraft_java_rs_core::models::loader::LoaderArtifactKind::NeoForge => "neoforge",
        minecraft_java_rs_core::models::loader::LoaderArtifactKind::Fabric => "fabric",
        minecraft_java_rs_core::models::loader::LoaderArtifactKind::OptiFine => "optifine",
        minecraft_java_rs_core::models::loader::LoaderArtifactKind::LiteLoader => "liteloader",
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("另存为加载器官方安装器")
        .add_filter("Java 安装器", &["jar"])
        .set_file_name(&file_name)
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
    Ok(Some(InstallerChoice {
        target: parent.join(name),
        candidates,
        hashes,
        expected_size,
        loader: loader_key.into(),
        network: network.clone(),
    }))
}

fn schedule_installer(
    app: &AppHandle,
    choice: Option<InstallerChoice>,
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
    task.describe_kind(crate::app_tasks::TaskKind::Installer, &name);
    tauri::async_runtime::spawn(async move {
        task.stage("下载加载器安装器", &name);
        let reporter = task.clone();
        let report_name = name.clone();
        let progress = Arc::new(
            move |progress: minecraft_java_rs_core::net::transfer::TransferProgress| {
                reporter.progress_bytes(progress.completed_bytes, progress.total_bytes);
                reporter.file(
                    &report_name,
                    progress
                        .total_bytes
                        .filter(|n| *n > 0)
                        .map(|n| progress.completed_bytes as f64 / n as f64)
                        .unwrap_or(0.0),
                    false,
                );
                reporter.progress(0, 1, 0.0, 0);
            },
        );
        // 先下载校验到任务暂存区；最终覆盖与取消共用生命周期锁。
        let staged = task.workspace.join("installer.jar");
        let result = download_installer_to(
            staged.clone(),
            choice.candidates,
            choice.loader,
            choice.hashes,
            choice.expected_size,
            &task.transfer,
            Some(progress),
        )
        .await;
        if result.is_ok() {
            task.file(&name, 1.0, true);
            task.progress(1, 1, 0.0, 0);
            task.stage("保存加载器安装器", &choice.target.to_string_lossy());
        }
        task.finish_with(move || {
            result?;
            let bytes =
                std::fs::read(staged).map_err(|e| format!("读取安装器暂存文件失败：{e}"))?;
            crate::named_profiles::atomic_write(&choice.target, &bytes)?;
            Ok(None)
        });
    });
    Ok(Some(id))
}

/// UI 专用入口：取消对话框不创建任务，选定文件后返回统一下载 taskId。
#[tauri::command]
pub async fn save_loader_installer_managed(
    app: AppHandle,
    mc_version: String,
    loader: String,
    loader_version: String,
    source: Option<String>,
) -> Result<Option<String>, String> {
    let network = network_settings::snapshot();
    network.transfer(source.as_deref())?;
    let choice = choose_installer(&app, &mc_version, &loader, &loader_version, &network).await?;
    schedule_installer(&app, choice, source.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installer_candidates_include_official_and_mirror_without_global_switch() {
        let url =
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.1/installer.jar";
        let candidates = core_candidates(url);
        assert!(
            candidates
                .iter()
                .any(|candidate| !candidate.is_mirror && candidate.url == url)
        );
        assert!(candidates.iter().any(|candidate| {
            candidate.is_mirror
                && candidate
                    .url
                    .ends_with("/maven/net/neoforged/neoforge/21.1.1/installer.jar")
        }));
        assert!(
            core_candidates("https://example.com/installer.jar")
                .iter()
                .all(|candidate| !candidate.is_mirror)
        );
    }

    async fn test_download(
        target: std::path::PathBuf,
        urls: Vec<String>,
    ) -> Result<String, String> {
        let context =
            network_settings::NetworkSettings::default().transfer(Some("mirror_first"))?;
        let candidates = urls
            .into_iter()
            .enumerate()
            .map(|(index, url)| {
                if index == 0 {
                    RequestCandidate::mirror(url)
                } else {
                    RequestCandidate::official(url)
                }
            })
            .collect();
        download_installer_to(
            target,
            candidates,
            "fabric".into(),
            FileHashes::default(),
            None,
            &context,
            None,
        )
        .await
    }

    // Python zipfile ZIP_STORED：manifest + 带空 main 方法的 example.Main。
    // 使用现有 base64 依赖，避免主 crate 为测试再引入 zip。
    fn executable_jar() -> Vec<u8> {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(concat!(
                "UEsDBBQAAAAAAAAAIVwVPisGMwAAADMAAAAUAAAATUVUQS1JTkYvTUFOSUZFU1QuTUZN",
                "YW5pZmVzdC1WZXJzaW9uOiAxLjANCk1haW4tQ2xhc3M6IGV4YW1wbGUuTWFpbg0KDQpQ",
                "SwMEFAAAAAAAAAAhXPliAV2CAAAAggAAABIAAABleGFtcGxlL01haW4uY2xhc3PK/rq+AAAA",
                "NAAIAQAMZXhhbXBsZS9NYWluBwABAQAQamF2YS9sYW5nL09iamVjdAcAAwEABG1haW4B",
                "ABYoW0xqYXZhL2xhbmcvU3RyaW5nOylWAQAEQ29kZQAhAAIABAAAAAAAAQAJAAUABgAB",
                "AAcAAAANAAAAAQAAAAGxAAAAAAAAUEsBAhQAFAAAAAAAAAAhXBU+KwYzAAAAMwAAABQAAAAA",
                "AAAAAAAAAIABAAAAAE1FVEEtSU5GL01BTklGRVNULk1GUEsBAhQAFAAAAAAAAAAhXPliAV2C",
                "AAAAggAAABIAAAAAAAAAAAAAAIABZQAAAGV4YW1wbGUvTWFpbi5jbGFzc1BLBQYAAAAAAgAC",
                "AIIAAAAXAQAAAAA="
            ))
            .unwrap()
    }

    #[test]
    fn rejects_empty_html_and_truncated_downloads() {
        for loader in ["forge", "neoforge", "fabric", "optifine", "liteloader"] {
            for bytes in [b"".as_slice(), b"<html>error</html>", b"PK\x03\x04test"] {
                assert!(validate_installer(bytes, loader).is_err());
            }
        }
    }

    #[test]
    fn optifine_and_liteloader_candidates_preserve_real_source_order() {
        let optifine = loader_catalog::optifine_install::parse_index(
            r#"[{"mcversion":"1.12.2","type":"HD_U","patch":"G5","filename":"OptiFine_1.12.2_HD_U_G5.jar","forge":""}]"#,
        )
        .unwrap();
        let mut optifine = loader_catalog::optifine_install::artifact(&optifine[0]);
        optifine
            .download_candidates
            .push("https://optifine.net/downloadx?f=OptiFine_1.12.2_HD_U_G5.jar&x=1".into());
        let candidates = installer_candidates(&optifine);
        assert_eq!(candidates.len(), 2);
        assert!(candidates[0].is_mirror);
        assert!(candidates[0].url.contains("bmclapi2.bangbang93.com/optifine/"));
        assert!(!candidates[1].is_mirror);
        assert!(candidates[1].url.starts_with("https://optifine.net/downloadx?"));

        let liteloader = loader_catalog::liteloader_install::parse_versions(
            r#"{"versions":{"1.12.2":{"artefacts":{"com.mumfrey:liteloader":{"latest":{"version":"1.12.2-SNAPSHOT-r0","tweakClass":"com.mumfrey.liteloader.launch.LiteLoaderTweaker","libraries":[]}}}}}}"#,
        )
        .unwrap();
        let liteloader = loader_catalog::liteloader_install::artifact(&liteloader[0]);
        let candidates = installer_candidates(&liteloader);
        assert_eq!(candidates.len(), 1);
        assert!(!candidates[0].is_mirror);
        assert!(candidates[0].url.contains("LiteLoaderInstaller%201.12.2"));
    }

    #[test]
    fn fabric_installer_does_not_require_forge_profile() {
        let bytes = executable_jar();
        validate_installer(&bytes, "fabric").unwrap();
        for loader in ["forge", "neoforge"] {
            let error = validate_installer(&bytes, loader).unwrap_err();
            assert!(error.contains("install_profile.json"), "{error}");
        }
    }

    async fn serve_installers(bodies: Vec<Vec<u8>>) -> (Vec<String>, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let urls = (0..bodies.len())
            .map(|index| format!("http://{address}/{index}.jar"))
            .collect();
        let server = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(10), async move {
                for body in bodies {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        assert_ne!(stream.read_buf(&mut request).await.unwrap(), 0);
                    }
                    let headers = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(headers.as_bytes()).await.unwrap();
                    stream.write_all(&body).await.unwrap();
                    stream.shutdown().await.unwrap();
                }
            })
            .await
            .unwrap();
        });
        (urls, server)
    }

    #[cfg(windows)]
    async fn wait_task_done(
        events: &std::sync::Mutex<Vec<serde_json::Value>>,
        id: &str,
    ) -> serde_json::Value {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(event) = events
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|event| event["taskId"] == id && event["type"] == "done")
                    .cloned()
                {
                    return event;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "真实 Wry 任务和本地 HTTP：显式单线程运行，不打开保存对话框"]
    fn live_managed_installer_task_lifecycle() {
        use tauri::Listener;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut context = tauri::generate_context!();
        context.config_mut().app.windows.clear();
        context.config_mut().identifier = format!(
            "studio.nickel.launcher.installertest.{}",
            crate::store::random_token()
        );
        let app = tauri::Builder::default()
            .any_thread()
            .build(context)
            .unwrap();
        let events = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let collected = events.clone();
        app.listen(crate::install::INSTALL_EVENT, move |event| {
            collected
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).unwrap());
        });
        let snapshot_for = |id: &str| {
            crate::app_tasks::app_tasks_snapshot()
                .into_iter()
                .map(|snapshot| serde_json::to_value(snapshot).unwrap())
                .find(|snapshot| snapshot["taskId"] == id)
                .expect("真实任务必须可由快照重建")
        };
        assert!(
            schedule_installer(app.handle(), None, None)
                .unwrap()
                .is_none()
        );
        assert!(events.lock().unwrap().is_empty());
        assert!(crate::app_tasks::app_tasks_snapshot().is_empty());
        let root = std::env::temp_dir().join(format!(
            "nickel-managed-installer-{}",
            crate::store::random_token()
        ));
        std::fs::create_dir_all(&root).unwrap();
        tauri::async_runtime::block_on(async {
            let good = executable_jar();
            for (name, body, ok) in [
                ("valid.jar", good.clone(), true),
                ("invalid.jar", b"not a jar".to_vec(), false),
            ] {
                let target = root.join(name);
                std::fs::write(&target, b"existing").unwrap();
                let transferred = body.len() as u64;
                let (urls, server) = serve_installers(vec![body]).await;
                let choice = InstallerChoice {
                    target: target.clone(),
                    loader: "fabric".into(),
                    network: network_settings::NetworkSettings::default(),
                    candidates: urls.into_iter().map(RequestCandidate::official).collect(),
                    hashes: FileHashes::default(),
                    expected_size: None,
                };
                let id = schedule_installer(app.handle(), Some(choice), Some("official_first"))
                    .unwrap()
                    .unwrap();
                let done = wait_task_done(&events, &id).await;
                server.await.unwrap();
                assert_eq!(done["ok"], ok);
                assert_eq!(done["cancelled"], false);
                assert!(done["result"].is_null());
                assert_eq!(
                    std::fs::read(&target).unwrap(),
                    if ok {
                        good.clone()
                    } else {
                        b"existing".to_vec()
                    }
                );
                let snapshot = snapshot_for(&id);
                assert_eq!(snapshot["kind"], "installer");
                assert_eq!(snapshot["status"], if ok { "done" } else { "error" });
                assert_eq!(
                    snapshot["target"],
                    std::fs::canonicalize(&target)
                        .unwrap()
                        .to_string_lossy()
                        .as_ref()
                );
                assert_eq!(snapshot["transferredBytes"], transferred);
                assert!(snapshot["speedBps"].is_null());
                assert!(snapshot["endedAt"].as_u64().is_some());
                assert_eq!(
                    events
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|event| event["taskId"] == id && event["type"] == "done")
                        .count(),
                    1
                );
                eprintln!("[managed-installer] {name}: done={done}; snapshot={snapshot}");
            }
            let target = root.join("cancelled.jar");
            std::fs::write(&target, b"existing").unwrap();
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let url = format!("http://{}/installer.jar", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    assert_ne!(stream.read_buf(&mut request).await.unwrap(), 0);
                }
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1048576\r\nConnection: close\r\n\r\nPK").await.unwrap();
                let mut byte = [0];
                tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut byte))
                    .await
                    .unwrap()
            });
            let id = schedule_installer(
                app.handle(),
                Some(InstallerChoice {
                    target: target.clone(),
                    loader: "fabric".into(),
                    network: network_settings::NetworkSettings::default(),
                    candidates: vec![RequestCandidate::official(url)],
                    hashes: FileHashes::default(),
                    expected_size: None,
                }),
                Some("official_first"),
            )
            .unwrap()
            .unwrap();
            let active = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let snapshot = snapshot_for(&id);
                    if snapshot["transferredBytes"] == 2 {
                        break snapshot;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(active["status"], "running");
            assert_eq!(active["completedBytes"], 0);
            assert_eq!(active["kind"], "installer");
            crate::install::install_cancel(id.clone());
            let done = wait_task_done(&events, &id).await;
            assert_eq!(done["ok"], false);
            assert_eq!(done["cancelled"], true);
            let ended = snapshot_for(&id);
            assert_eq!(ended["status"], "cancelled");
            assert_eq!(ended["transferredBytes"], 2);
            assert_eq!(std::fs::read(&target).unwrap(), b"existing");
            assert!(
                matches!(server.await.unwrap(), Ok(0) | Err(_)),
                "取消后连接应关闭"
            );
            assert_eq!(
                events
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|event| event["taskId"] == id && event["type"] == "done")
                    .count(),
                1
            );
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), 3);
            eprintln!(
                "[managed-installer] cancelled: done={done}; streaming_snapshot={active}; terminal_snapshot={ended}; existing file retained"
            );
        });
        let bounded = crate::app_tasks::begin(app.handle(), &root.join("bounded"), None).unwrap();
        bounded.describe_kind(crate::app_tasks::TaskKind::Resource, "快照边界验收");
        bounded.stage("校验文件", "本地阶段");
        for index in 0..140 {
            bounded.file(&format!("file-{index}"), 0.5, false);
        }
        let active = snapshot_for(&bounded.id);
        assert_eq!(active["files"].as_array().unwrap().len(), 128);
        assert_eq!(active["files"][0]["name"], "file-12");
        let mut last_id = String::new();
        for index in 0..34 {
            let task =
                crate::app_tasks::begin(app.handle(), &root.join(format!("history-{index}")), None)
                    .unwrap();
            last_id = task.id.clone();
            task.finish(Ok(None));
        }
        let snapshots = crate::app_tasks::app_tasks_snapshot();
        assert_eq!(snapshots.len(), 33);
        assert_eq!(snapshot_for(&bounded.id)["status"], "running");
        assert_eq!(snapshot_for(&last_id)["status"], "done");
        bounded.finish(Ok(None));
        bounded.finish(Err("duplicate finish".into()));
        assert_eq!(crate::app_tasks::app_tasks_snapshot().len(), 32);
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event["taskId"] == bounded.id && event["type"] == "done")
                .count(),
            1
        );
        eprintln!(
            "[task-snapshot] active_plus_history=33; terminal_limit=32; file_limit=128; unique_done=1"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn invalid_candidates_preserve_existing_target() {
        let root = std::env::temp_dir().join(format!(
            "nickel-invalid-installer-{}",
            crate::store::random_token()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("installer.jar");
        std::fs::write(&target, b"existing installer").unwrap();
        let (urls, server) = serve_installers(vec![
            b"PK\x03\x04test".to_vec(),
            b"<html>error</html>".to_vec(),
        ])
        .await;
        let result = test_download(target.clone(), urls).await;
        let server_result = server.await;
        let actual = std::fs::read(&target).unwrap();
        let file_count = std::fs::read_dir(&root).unwrap().count();
        std::fs::remove_dir_all(&root).unwrap();
        server_result.unwrap();
        assert!(result.is_err(), "{result:?}");
        assert_eq!(actual, b"existing installer");
        assert_eq!(file_count, 1);
    }

    #[tokio::test]
    async fn invalid_mirror_falls_back_to_valid_official_candidate() {
        let root = std::env::temp_dir().join(format!(
            "nickel-valid-installer-{}",
            crate::store::random_token()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("installer.jar");
        std::fs::write(&target, b"existing installer").unwrap();
        let bytes = executable_jar();
        let (urls, server) =
            serve_installers(vec![b"PK\x03\x04test".to_vec(), bytes.clone()]).await;
        let result = test_download(target.clone(), urls).await;
        let server_result = server.await;
        let actual = std::fs::read(&target).unwrap();
        let file_count = std::fs::read_dir(&root).unwrap().count();
        std::fs::remove_dir_all(&root).unwrap();
        server_result.unwrap();
        assert_eq!(result.unwrap(), target.to_string_lossy());
        assert_eq!(actual, bytes);
        assert_eq!(file_count, 1);
    }

    #[tokio::test]
    async fn cancelled_installer_never_replaces_existing_target() {
        let root = std::env::temp_dir().join(format!(
            "nickel-cancelled-installer-{}",
            crate::store::random_token()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("installer.jar");
        std::fs::write(&target, b"existing installer").unwrap();
        let context = network_settings::NetworkSettings::default()
            .transfer(None)
            .unwrap();
        context.cancellation().cancel();
        let result =
            download_installer_to(
                target.clone(),
                Vec::new(),
                "fabric".into(),
                FileHashes::default(),
                None,
                &context,
                None,
            )
            .await;
        assert!(result.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"existing installer");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn cached_explicit_forge_build_does_not_need_network_metadata() {
        use minecraft_java_rs_core::loader::forge::ForgeMC;
        use minecraft_java_rs_core::net::client::build_client;
        let root = std::env::temp_dir().join(format!(
            "nickel-forge-cache-{}",
            crate::store::random_token()
        ));
        let params = crate::ResolvedLaunch {
            version: "1.20.1".into(),
            username: "Player".into(),
            memory_max: "2G".into(),
            game_dir: None,
        };
        let options = crate::build_options(&params, root.clone());
        let path = options
            .loader_dir("forge")
            .join("installer/forge-1.20.1-47.4.28-installer.jar");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, executable_jar()).unwrap();
        let client = build_client(1, false, None).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        for build in ["47.4.28", "1.20.1-47.4.28"] {
            let result = ForgeMC::new()
                .download_installer(&options, "1.20.1", build, &client, &tx)
                .await
                .unwrap();
            assert_eq!(std::path::PathBuf::from(result.file_path), path);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    #[ignore = "显式下载三个小型安装器；不安装游戏或 Java"]
    async fn live_installer_downloads() {
        use minecraft_java_rs_core::loader::{forge::ForgeMC, neoforge::NeoForgeMC};
        use minecraft_java_rs_core::net::client::build_client;
        use minecraft_java_rs_core::utils::archive::{ArchiveQueryResult, get_file_from_archive};
        use std::{path::PathBuf, time::Duration};
        let root = std::env::temp_dir().join(format!(
            "nickel-installers-{}",
            crate::store::random_token()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(128);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let result = tokio::time::timeout(Duration::from_secs(INSTALLER_TIMEOUT_SECS), async {
            let catalog = loader_catalog::build_catalog("1.20.1", Some("official_first")).await;
            let params = crate::ResolvedLaunch {
                version: "1.20.1".into(),
                username: "Player".into(),
                memory_max: "2G".into(),
                game_dir: None,
            };
            let options = crate::build_options(&params, root.clone());
            let client = build_client(10, false, None).map_err(|e| e.to_string())?;
            let forge = catalog.forge.first().ok_or("Forge 清单为空")?;
            let neo = catalog.neoforge.first().ok_or("NeoForge 清单为空")?;
            let forge_loader = ForgeMC::new();
            let neo_loader = NeoForgeMC::new();
            let (forge, neo) = tokio::join!(
                forge_loader.download_installer(&options, "1.20.1", &forge.version, &client, &tx),
                neo_loader.download_installer(&options, "1.20.1", &neo.version, &client, &tx),
            );
            for (name, installer) in [("forge", forge), ("neoforge", neo)] {
                let installer = installer.map_err(|e| e.to_string())?;
                let path = PathBuf::from(&installer.file_path);
                let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                tauri::async_runtime::spawn_blocking(move || validate_installer(&bytes, name))
                    .await
                    .map_err(|e| e.to_string())??;
                let profile =
                    get_file_from_archive(path, Some("install_profile.json".into()), None, false)
                        .await
                        .map_err(|e| e.to_string())?;
                let ArchiveQueryResult::FileData(bytes) = profile else {
                    return Err("安装器缺少 profile".into());
                };
                let profile: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                println!(
                    "{name}: artifact={}, profile_version={}, old_api={}",
                    installer.meta_data, profile["version"], installer.old_api
                );
            }
            let fabric = catalog.fabric.first().ok_or("Fabric 清单为空")?;
            let bytes =
                loader_catalog::fetch_bytes(&fabric.download_url, INSTALLER_MAX_BYTES, 20).await?;
            let size = bytes.len();
            tauri::async_runtime::spawn_blocking(move || validate_installer(&bytes, "fabric"))
                .await
                .map_err(|e| e.to_string())??;
            println!("fabric: official_installer_bytes={size}");
            Ok::<(), String>(())
        })
        .await;
        drop(tx);
        drain.await.unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert!(matches!(result, Ok(Ok(()))), "{result:?}");
    }
}
