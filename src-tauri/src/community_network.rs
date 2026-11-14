//! 社区 API/CDN 候选适配。排序、竞速、取消与文件预算均由 core transfer 实现。
use crate::network_settings::NetworkSettings;
use minecraft_java_rs_core::net::client::reqwest::{self, Url};
use minecraft_java_rs_core::net::transfer::{
    self, CancellationToken, RequestCandidate, TransferContext,
};

pub const MODRINTH_API: &str = "https://api.modrinth.com/v2/";
pub const CURSEFORGE_API: &str = "https://api.curseforge.com/v1/";
const MAX_API_BYTES: usize = 32 * 1024 * 1024;
const USER_AGENT: &str = concat!(
    "Nickel/",
    env!("CARGO_PKG_VERSION"),
    " (studio.nickel.launcher)"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Modrinth,
    Curseforge,
}
impl Platform {
    pub fn api(self) -> &'static str {
        match self {
            Self::Modrinth => MODRINTH_API,
            Self::Curseforge => CURSEFORGE_API,
        }
    }
    pub fn service(self) -> &'static str {
        match self {
            Self::Modrinth => "modrinth-api",
            Self::Curseforge => "curseforge-api",
        }
    }
}

pub fn client() -> Result<reqwest::Client, String> {
    minecraft_java_rs_core::net::client::build_client(crate::http::TIMEOUT_SECS, false, None)
        .map_err(|_| "初始化社区 API 客户端失败".into())
}

pub fn file_client() -> Result<reqwest::Client, String> {
    minecraft_java_rs_core::net::client::build_transfer_client(
        crate::http::TIMEOUT_SECS,
        false,
        None,
    )
    .map_err(|_| "初始化社区下载客户端失败".into())
}

fn checked_https(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "社区地址格式无效".to_string())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("社区地址必须使用无凭据的 HTTPS 地址".into());
    }
    Ok(url)
}

fn mirror_join(base: &str, suffix: &str) -> Result<String, String> {
    let url = checked_https(base)?;
    if url.query().is_some() {
        return Err("镜像基址不能包含查询参数".into());
    }
    Ok(format!(
        "{}/{}",
        base.trim_end_matches('/'),
        suffix.trim_start_matches('/')
    ))
}

pub fn api_candidates(
    platform: Platform,
    official: &str,
    mirror: &str,
    key: Option<&str>,
) -> Result<Vec<RequestCandidate>, String> {
    checked_https(official)?;
    let suffix = official
        .strip_prefix(platform.api())
        .ok_or_else(|| "社区请求地址不属于指定平台".to_string())?;
    let mut candidates = Vec::new();
    // 生产通道不再因为缺少本地 Key 而拒绝官方直连：请求照常发出，由服务端
    // 返回真实状态码（通常 401/403），界面按实际错误提示。开发通道保留原行为，
    // 避免开发期被必然失败的官方请求拖慢。
    let production = crate::build_channel::is_production();
    if platform != Platform::Curseforge || key.is_some_and(|k| !k.is_empty()) || production {
        let mut candidate = RequestCandidate::official(official);
        if platform == Platform::Curseforge {
            if let Some(key) = key.filter(|value| !value.is_empty()) {
                let value = reqwest::header::HeaderValue::from_str(key)
                    .map_err(|_| "CurseForge Key 格式无效，请重新配置".to_string())?;
                candidate.headers.insert("x-api-key", value);
            }
        }
        candidates.push(candidate);
    }
    if !mirror.trim().is_empty() {
        candidates.push(RequestCandidate::mirror(mirror_join(mirror, suffix)?));
    }
    for candidate in &mut candidates {
        candidate.headers.insert(
            "user-agent",
            reqwest::header::HeaderValue::from_static(USER_AGENT),
        );
    }
    if candidates.is_empty() {
        return Err("CurseForge 没有可用接口，请配置 API 镜像或官方 Key 后重试".into());
    }
    Ok(candidates)
}

pub fn file_candidates(url: &str, mirror: &str) -> Result<Vec<RequestCandidate>, String> {
    let parsed = checked_https(url)?;
    let mut candidates = vec![RequestCandidate::official(url)];
    if matches!(
        parsed.host_str(),
        Some("cdn.modrinth.com" | "edge.forgecdn.net" | "mediafilez.forgecdn.net")
    ) && parsed.port().is_none()
        && !mirror.trim().is_empty()
    {
        // 只替换精确主机部分；原始百分号编码和 query 不反解、不重新拼参数。
        let authority_end = url.find("://").map(|i| i + 3).unwrap_or(0);
        let suffix_start = url[authority_end..]
            .find(['/', '?'])
            .map(|i| authority_end + i)
            .unwrap_or(url.len());
        candidates.push(RequestCandidate::mirror(mirror_join(
            mirror,
            &url[suffix_start..],
        )?));
    }
    Ok(candidates)
}

/// 同一官方 origin 集合共享测速结果，镜像地址和具体文件路径不改变服务身份。
pub fn file_service<'a>(urls: impl IntoIterator<Item = &'a str>) -> Result<String, String> {
    let mut origins = std::collections::BTreeSet::new();
    for url in urls {
        origins.insert(checked_https(url)?.origin().ascii_serialization());
    }
    if origins.is_empty() {
        return Err("社区文件缺少下载来源".into());
    }
    Ok(format!(
        "community-files:{}",
        origins.into_iter().collect::<Vec<_>>().join("|")
    ))
}

pub fn context(settings: &NetworkSettings) -> TransferContext {
    TransferContext::new(
        settings.community_api_source,
        settings.file_download_source,
        CancellationToken::new(),
    )
}

pub async fn get_json<T, V: Fn(&serde_json::Value) -> Result<T, String>>(
    platform: Platform,
    url: &str,
    settings: &NetworkSettings,
    transfer: &TransferContext,
    key: Option<&str>,
    validate: V,
) -> Result<T, String> {
    let mirror = match platform {
        Platform::Modrinth => &settings.modrinth_api_mirror,
        Platform::Curseforge => &settings.curseforge_api_mirror,
    };
    let candidates = api_candidates(platform, url, mirror, key)?;
    transfer::fetch_validated_candidates(
        &client()?,
        transfer,
        platform.service(),
        &candidates,
        MAX_API_BYTES,
        |bytes| {
            let value = serde_json::from_slice(bytes)
                .map_err(|_| "社区接口返回了无效 JSON，请重试".to_string())?;
            validate(&value)
        },
    )
    .await
}

/// POST JSON（CurseForge 指纹匹配等只接受 POST 的接口）。
///
/// 与 [`get_json`] 共用候选与凭据规则；POST 一律不跟随重定向，避免自定义
/// Key 被带到新域名。官方候选失败时继续尝试镜像候选，最终返回最后一次的真实错误。
pub async fn post_json<T, V: Fn(&serde_json::Value) -> Result<T, String>>(
    platform: Platform,
    url: &str,
    settings: &NetworkSettings,
    key: Option<&str>,
    body: &serde_json::Value,
    validate: V,
) -> Result<T, String> {
    let mirror = match platform {
        Platform::Modrinth => &settings.modrinth_api_mirror,
        Platform::Curseforge => &settings.curseforge_api_mirror,
    };
    let candidates = api_candidates(platform, url, mirror, key)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(crate::http::TIMEOUT_SECS))
        .build()
        .map_err(|_| "初始化社区 API 客户端失败".to_string())?;
    let mut last_error = String::new();
    for candidate in &candidates {
        let response = client
            .post(&candidate.url)
            .headers(candidate.headers.clone())
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .json(body)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                last_error = minecraft_java_rs_core::net::client::describe_reqwest_error(&error);
                continue;
            }
        };
        if !response.status().is_success() {
            last_error = format!("HTTP {}", response.status());
            continue;
        }
        if response
            .content_length()
            .is_some_and(|len| len > MAX_API_BYTES as u64)
        {
            last_error = "社区接口响应超过大小限制".into();
            continue;
        }
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => {
                last_error = minecraft_java_rs_core::net::client::describe_reqwest_error(&error);
                continue;
            }
        };
        if bytes.len() > MAX_API_BYTES {
            last_error = "社区接口响应超过大小限制".into();
            continue;
        }
        let value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                last_error = "社区接口返回了无效 JSON，请重试".into();
                continue;
            }
        };
        return validate(&value);
    }
    Err(if last_error.is_empty() {
        "没有可用的请求地址".into()
    } else {
        last_error
    })
}

/// 指定绝对目标路径的共享下载请求：安装（实例资源目录）与另存为（任务暂存区）
/// 都经过同一道路径围栏与候选构造，避免两处各自拼地址。
pub(crate) fn resource_request_to(
    destination: &std::path::Path,
    url: &str,
    hashes: transfer::FileHashes,
    mirror: &str,
) -> Result<transfer::DownloadRequest, String> {
    let filename = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("资源文件名无效")?;
    crate::modpacks::validate_pack_path(filename).map_err(|e| e.to_string())?;
    if filename.contains('/') {
        return Err("资源文件名不能包含目录".into());
    }
    let parent = destination.parent().ok_or("资源目标缺少父目录")?;
    crate::modpacks::check_destination(parent, destination)?;
    Ok(transfer::DownloadRequest {
        candidates: file_candidates(url, mirror)?,
        destination: destination.to_path_buf(),
        expected_size: None,
        hashes,
        service: file_service([url])?,
    })
}

fn resource_request(
    root: &std::path::Path,
    folder: &str,
    filename: &str,
    url: &str,
    hashes: transfer::FileHashes,
    mirror: &str,
) -> Result<transfer::DownloadRequest, String> {
    if !matches!(
        folder,
        "mods" | "resourcepacks" | "shaderpacks" | "datapacks"
    ) {
        return Err("资源安装目录无效".into());
    }
    // 拼接前先校验原始文件名：join 会吞掉 `..` 等危险分量，不能只校验落点。
    crate::modpacks::validate_pack_path(filename).map_err(|e| e.to_string())?;
    if filename.contains('/') {
        return Err("资源文件名不能包含目录".into());
    }
    resource_request_to(&root.join(folder).join(filename), url, hashes, mirror)
}

/// 单文件下载的进度与终态上报（安装与另存为暂存共用）。
async fn download_with_progress(
    request: transfer::DownloadRequest,
    filename: &str,
    context: &crate::app_tasks::TaskContext,
) -> Result<(), String> {
    context.stage("下载资源", filename);
    let reporter = context.clone();
    let name = filename.to_owned();
    let began = std::time::Instant::now();
    let progress = std::sync::Arc::new(move |progress: transfer::TransferProgress| {
        reporter.file(
            &name,
            progress
                .total_bytes
                .filter(|n| *n > 0)
                .map(|n| progress.completed_bytes as f64 / n as f64)
                .unwrap_or(0.0),
            false,
        );
        reporter.progress(
            0,
            1,
            progress.transferred_bytes as f64 / began.elapsed().as_secs_f64().max(0.001),
            0,
        );
    });
    transfer::download(&file_client()?, &context.transfer, &request, Some(progress)).await?;
    context.check_cancelled()?;
    context.file(filename, 1.0, true);
    context.progress(1, 1, 0.0, 0);
    Ok(())
}

pub async fn install_resource(
    root: &std::path::Path,
    folder: &str,
    filename: &str,
    url: &str,
    hashes: transfer::FileHashes,
    context: &crate::app_tasks::TaskContext,
) -> Result<(), String> {
    context.check_cancelled()?;
    let request = resource_request(
        root,
        folder,
        filename,
        url,
        hashes,
        &context.network.community_file_mirror,
    )?;
    download_with_progress(request, filename, context).await
}

/// 另存为：下载到指定绝对暂存路径（任务 workspace），校验与进度同安装；
/// 最终落位由调用方在 `finish_with` 内原子替换用户选择的目标。
pub(crate) async fn stage_resource(
    destination: &std::path::Path,
    display_name: &str,
    url: &str,
    hashes: transfer::FileHashes,
    context: &crate::app_tasks::TaskContext,
) -> Result<(), String> {
    context.check_cancelled()?;
    let request = resource_request_to(
        destination,
        url,
        hashes,
        &context.network.community_file_mirror,
    )?;
    download_with_progress(request, display_name, context).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cf_without_key_still_has_mirror() {
        let official = format!("{CURSEFORGE_API}mods/1");
        let values = api_candidates(
            Platform::Curseforge,
            &official,
            "https://mod.mcimirror.top/curseforge/v1/",
            None,
        )
        .unwrap();
        let production = crate::build_channel::is_production();
        assert_eq!(values.len(), if production { 2 } else { 1 });
        if production {
            assert!(!values[0].is_mirror);
            assert_eq!(values[0].url, official);
        }
        assert!(values.last().is_some_and(|candidate| candidate.is_mirror));
        assert!(values.iter().all(|candidate| {
            !candidate.headers.contains_key("x-api-key")
                && candidate
                    .headers
                    .get("user-agent")
                    .is_some_and(|value| value == USER_AGENT)
        }));
    }

    #[test]
    fn cf_without_key_or_mirror_obeys_build_channel() {
        let official = format!("{CURSEFORGE_API}mods/1");
        let result = api_candidates(Platform::Curseforge, &official, "", None);
        if crate::build_channel::is_production() {
            let values = result.unwrap();
            assert_eq!(values.len(), 1);
            assert_eq!(values[0].url, official);
            assert!(!values[0].is_mirror);
            assert!(!values[0].headers.contains_key("x-api-key"));
        } else {
            assert!(result.is_err());
        }
    }
    #[test]
    fn cf_key_is_only_on_exact_official_candidate() {
        let values = api_candidates(
            Platform::Curseforge,
            "https://api.curseforge.com/v1/mods/1",
            "https://mod.mcimirror.top/curseforge/v1/",
            Some("test-key"),
        )
        .unwrap();
        assert!(values[0].headers.contains_key("x-api-key"));
        assert!(!values[1].headers.contains_key("x-api-key"));
        assert!(
            api_candidates(
                Platform::Curseforge,
                "https://api.curseforge.com.evil/v1/mods/1",
                "",
                Some("test-key")
            )
            .is_err()
        );
    }
    #[test]
    fn exact_cdn_hosts_preserve_encoded_path_and_query() {
        for host in [
            "cdn.modrinth.com",
            "edge.forgecdn.net",
            "mediafilez.forgecdn.net",
        ] {
            let values = file_candidates(
                &format!("https://{host}/files/a%20b%2Fc.jar?x=a%2Fb&v=2"),
                "https://mod.mcimirror.top/",
            )
            .unwrap();
            assert_eq!(
                values[1].url,
                "https://mod.mcimirror.top/files/a%20b%2Fc.jar?x=a%2Fb&v=2"
            );
        }
        assert_eq!(
            file_candidates(
                "https://cdn.modrinth.com.evil/file.jar",
                "https://mod.mcimirror.top/"
            )
            .unwrap()
            .len(),
            1
        );
        assert!(file_candidates("https://user:secret@cdn.modrinth.com/file", "").is_err());
    }
    #[test]
    fn api_paths_keep_version_prefix_and_query() {
        let values = api_candidates(
            Platform::Modrinth,
            "https://api.modrinth.com/v2/search?query=a%20b",
            "https://mod.mcimirror.top/modrinth/v2/",
            None,
        )
        .unwrap();
        assert_eq!(
            values[1].url,
            "https://mod.mcimirror.top/modrinth/v2/search?query=a%20b"
        );
    }

    #[test]
    fn ordinary_resources_keep_target_hashes_and_source_candidates() {
        let root = std::env::temp_dir().join(crate::store::random_token());
        for folder in ["mods", "resourcepacks", "shaderpacks", "datapacks"] {
            for url in [
                "https://cdn.modrinth.com/a.jar",
                "https://edge.forgecdn.net/a.jar",
            ] {
                let request = resource_request(
                    &root,
                    folder,
                    "a.jar",
                    url,
                    transfer::FileHashes {
                        sha1: Some("a".repeat(40)),
                        ..Default::default()
                    },
                    "https://mod.mcimirror.top/",
                )
                .unwrap();
                assert_eq!(request.destination, root.join(folder).join("a.jar"));
                assert_eq!(request.service, file_service([url]).unwrap());
                assert_eq!(
                    request.hashes.sha1.as_deref(),
                    Some("a".repeat(40).as_str())
                );
                assert_eq!(request.candidates.len(), 2);
                assert!(
                    request
                        .candidates
                        .iter()
                        .all(|candidate| !candidate.headers.contains_key("x-api-key"))
                );
            }
        }
    }

    #[test]
    fn file_service_uses_normalized_official_origin_sets() {
        let modrinth = file_service(["https://cdn.modrinth.com/a.jar"]).unwrap();
        assert_eq!(modrinth, "community-files:https://cdn.modrinth.com");
        assert_eq!(
            modrinth,
            file_service([
                "https://CDN.MODRINTH.COM:443/data/b%20c.jar?version=2",
                "https://cdn.modrinth.com/d.jar",
            ])
            .unwrap()
        );
        let combined = file_service([
            "https://edge.forgecdn.net/a.jar",
            "https://cdn.modrinth.com/b.jar",
        ])
        .unwrap();
        assert_eq!(
            combined,
            file_service([
                "https://cdn.modrinth.com/c.jar?version=3",
                "https://EDGE.FORGECDN.NET:443/d.jar",
                "https://edge.forgecdn.net/e.jar",
            ])
            .unwrap()
        );
        assert_ne!(combined, modrinth);
        let origins: std::collections::HashSet<_> = [
            "https://cdn.modrinth.com/a.jar",
            "https://edge.forgecdn.net/a.jar",
            "https://mediafilez.forgecdn.net/a.jar",
            "https://cdn.modrinth.com:8443/a.jar",
            "https://example.org/a.jar",
        ]
        .into_iter()
        .map(|url| file_service([url]).unwrap())
        .collect();
        assert_eq!(origins.len(), 5);
        assert!(file_service(std::iter::empty::<&str>()).is_err());
        assert!(file_service(["http://cdn.modrinth.com/a.jar"]).is_err());
    }

    #[test]
    fn ordinary_resources_reject_unsafe_filename_and_folder() {
        let root = std::env::temp_dir().join(crate::store::random_token());
        for (folder, filename) in [
            ("../outside", "a.jar"),
            ("versions", "a.jar"),
            ("mods", "../a.jar"),
            ("mods", "nested/a.jar"),
            ("mods", "a.jar:stream"),
            ("mods", "a.jar."),
            ("mods", "CON.jar"),
        ] {
            assert!(
                resource_request(
                    &root,
                    folder,
                    filename,
                    "https://cdn.modrinth.com/a.jar",
                    Default::default(),
                    ""
                )
                .is_err()
            );
        }
    }

    async fn fixture_server(body: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/resource.jar", listener.local_addr().unwrap());
        let worker = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = [0_u8; 4096];
                if socket.read(&mut request).await.is_err() {
                    continue;
                }
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if socket.write_all(header.as_bytes()).await.is_ok() {
                    let _ = socket.write_all(&body).await;
                }
            }
        });
        (url, worker)
    }

    #[tokio::test]
    async fn ordinary_resource_transfer_falls_back_and_commits_zip() {
        use base64::Engine;
        struct TestDirectory(std::path::PathBuf);
        impl Drop for TestDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory = TestDirectory(std::env::temp_dir().join(crate::store::random_token()));
        let zip = base64::engine::general_purpose::STANDARD.decode("UEsDBBQAAAAAAHKlZ11K1A4THQAAAB0AAAAMAAAAcmVzb3VyY2UudHh0Y29tbXVuaXR5IHRyYW5zZmVyIHJlZ3Jlc3Npb25QSwECFAAUAAAAAABypWddStQOEx0AAAAdAAAADAAAAAAAAAAAAAAAgAEAAAAAcmVzb3VyY2UudHh0UEsFBgAAAAABAAEAOgAAAEcAAAAAAA==").unwrap();
        let (bad, bad_server) = fixture_server(b"not a zip".to_vec()).await;
        let (good, good_server) = fixture_server(zip.clone()).await;
        let mut request = resource_request(
            &directory.0,
            "mods",
            "resource.jar",
            "https://cdn.modrinth.com/resource.jar",
            Default::default(),
            "",
        )
        .unwrap();
        // 测试只把已验证请求的候选替换为本机 TCP，不放宽产品 HTTPS 边界。
        request.candidates = vec![
            RequestCandidate::official(bad),
            RequestCandidate::mirror(good),
        ];
        let transfer = context(&NetworkSettings::default());
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            transfer::download(&file_client().unwrap(), &transfer, &request, None),
        )
        .await;
        assert!(result.unwrap().is_ok());
        assert_eq!(std::fs::read(&request.destination).unwrap(), zip);
        assert_eq!(
            std::fs::read_dir(request.destination.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        let stopped = TransferContext::new(transfer.list_policy(), transfer.file_policy(), cancel);
        request.destination = directory.0.join("mods/cancelled.jar");
        assert!(
            transfer::download(&file_client().unwrap(), &stopped, &request, None)
                .await
                .is_err()
        );
        assert!(!request.destination.exists());
        bad_server.abort();
        good_server.abort();
    }
}
