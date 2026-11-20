//! 按需准备固定发布的 Java 组件；校验有效缓存时不查询在线目录。
//! JLW 1.4.4 为 MIT，LWJGL Unsafe Agent 2.0 为 Apache-2.0，authlib-injector 为 AGPL-3.0。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use minecraft_java_rs_core::launcher::game_data::GameData;
use minecraft_java_rs_core::net::client::{build_transfer_client, describe_reqwest_error};
use minecraft_java_rs_core::net::transfer::{
    CancellationToken, DownloadRequest, FileHashes, RequestCandidate, TransferContext,
    download_validated,
};
use minecraft_java_rs_core::utils::archive::{read_zip_entry, validate_zip_file};
use minecraft_java_rs_core::utils::platform::skip_library;
use serde_json::Value;

use crate::runtime_settings::EffectiveSettings;

const AGENTS_DIRECTORY: &str = "agents";
const MANIFEST_ENTRY: &str = "META-INF/MANIFEST.MF";
const MANIFEST_MAX_BYTES: u64 = 64 * 1024;
const CLASS_MAX_BYTES: u64 = 1024 * 1024;
const AGENT_MAX_BYTES: u64 = 4 * 1024 * 1024;
#[cfg(windows)]
const UTF8_CODE_PAGE: u32 = 65001;

#[derive(Debug, Default)]
pub struct AgentPlan {
    pub jvm_args: Vec<String>,
    pub wrapper: Option<PathBuf>,
}

#[derive(Clone, Copy)]
struct Artifact {
    id: &'static str,
    filename: &'static str,
    size: u64,
    sha256: &'static str,
    official_url: &'static str,
    mirror_url: Option<&'static str>,
    manifest_key: &'static str,
    entry_class: &'static str,
}

const WRAPPER: Artifact = Artifact {
    id: "java-launch-wrapper",
    filename: "java_launch_wrapper-1.4.4.jar",
    size: 16_345,
    sha256: "88eb3ee58854e916437831697ef6a2fd493da9bc4ffb018f620cd88f090952aa",
    official_url: "https://github.com/00ll00/java_launch_wrapper/releases/download/v1.4.4/java_launch_wrapper-1.4.4.jar",
    mirror_url: None,
    manifest_key: "Main-Class",
    entry_class: "oolloo.jlw.Wrapper",
};

const LWJGL: Artifact = Artifact {
    id: "lwjgl-unsafe-agent",
    filename: "lwjgl-unsafe-agent-2.0.jar",
    size: 9_821,
    sha256: "6c07d3508dc090a12076876bdc227a19ce195a41c9e2c6535c3f82213b1b2015",
    official_url: "https://github.com/HMCL-dev/lwjgl-unsafe-agent/releases/download/2.0/lwjgl-unsafe-agent-2.0.jar",
    mirror_url: None,
    manifest_key: "Premain-Class",
    entry_class: "org.glavo.lwjgl.UnsafeAgent",
};

// 固定 build 56 来自官方 artifact API；BMCLAPI 为官方文档列出的镜像。
// 下载候选交由当前 TransferContext 排序，既不绕过 TLS，也不为缓存查询 latest.json。
const AUTHLIB: Artifact = Artifact {
    id: "authlib-injector",
    filename: "authlib-injector-1.2.8.jar",
    size: 349_681,
    sha256: "9c7f4343e6c82034958ffb48c14a2cb0c85928be7283103ce17da00c6d5a7b10",
    official_url: "https://authlib-injector.yushi.moe/artifact/56/authlib-injector-1.2.8.jar",
    mirror_url: Some(
        "https://bmclapi2.bangbang93.com/mirrors/authlib-injector/artifact/56/authlib-injector-1.2.8.jar",
    ),
    manifest_key: "Premain-Class",
    entry_class: "moe.yushi.authlibinjector.Premain",
};

fn candidates(artifact: Artifact) -> Vec<RequestCandidate> {
    let mut result = vec![RequestCandidate::official(artifact.official_url)];
    if let Some(mirror) = artifact.mirror_url {
        result.push(RequestCandidate::mirror(mirror));
    }
    result
}

fn manifest_attribute(manifest: &[u8], key: &str) -> Result<String, String> {
    let mut value: Option<Vec<u8>> = None;
    let mut selected = false;
    let mut has_header = false;
    for line in manifest.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            break;
        }
        if let Some(continuation) = line.strip_prefix(b" ") {
            if !has_header {
                return Err("JAR 清单的续行缺少前置字段".into());
            }
            if selected {
                if let Some(value) = value.as_mut() {
                    value.extend_from_slice(continuation);
                }
            }
            continue;
        }
        let separator = line
            .iter()
            .position(|byte| *byte == b':')
            .ok_or("JAR 清单字段格式无效")?;
        let content = line[separator + 1..]
            .strip_prefix(b" ")
            .ok_or("JAR 清单字段值格式无效")?;
        has_header = true;
        selected = line[..separator].eq_ignore_ascii_case(key.as_bytes());
        if selected {
            if value.is_some() {
                return Err(format!("JAR 清单包含重复的 {key}"));
            }
            value = Some(content.to_vec());
        }
    }
    let value = value.ok_or_else(|| format!("JAR 清单缺少 {key}"))?;
    let value = String::from_utf8(value).map_err(|_| "JAR 入口类名不是有效 UTF-8")?;
    if value.trim().is_empty() {
        return Err("JAR 入口类名为空".into());
    }
    Ok(value.trim().to_owned())
}

fn validate_artifact(
    path: &Path,
    artifact: Artifact,
    cancel: &CancellationToken,
) -> Result<(), String> {
    cancel.check()?;
    // SHA-256 与下载大小在 core 中先验证；这里只复用成熟 ZIP 原语检查清单和类。
    // 两个 agent 没有 Main-Class，不能套用安装器的 executable-JAR 前置条件。
    validate_zip_file(path, cancel)?;
    let manifest = read_zip_entry(path, MANIFEST_ENTRY, MANIFEST_MAX_BYTES, cancel)?;
    if manifest_attribute(&manifest, artifact.manifest_key)? != artifact.entry_class {
        return Err(format!("{} 的 JAR 入口不匹配，请重新下载", artifact.id));
    }
    let class_entry = format!("{}.class", artifact.entry_class.replace('.', "/"));
    let class = read_zip_entry(path, &class_entry, CLASS_MAX_BYTES, cancel)?;
    if class.len() < 8 || !class.starts_with(b"\xca\xfe\xba\xbe") {
        return Err(format!("{} 的入口类损坏，请重新下载", artifact.id));
    }
    cancel.check()
}

async fn ensure_artifact(
    directory: &Path,
    artifact: Artifact,
    context: &TransferContext,
) -> Result<PathBuf, String> {
    ensure_artifact_from(directory, artifact, context, candidates(artifact)).await
}

async fn ensure_artifact_from(
    directory: &Path,
    artifact: Artifact,
    context: &TransferContext,
    sources: Vec<RequestCandidate>,
) -> Result<PathBuf, String> {
    let destination = directory.join(artifact.filename);
    let request = DownloadRequest {
        candidates: sources,
        destination: destination.clone(),
        expected_size: Some(artifact.size),
        hashes: FileHashes {
            sha256: Some(artifact.sha256.into()),
            ..FileHashes::default()
        },
        service: artifact.id.into(),
    };
    let client = build_transfer_client(crate::http::TIMEOUT_SECS, false, None)
        .map_err(|error| describe_reqwest_error(&error))?;
    download_validated(
        &client,
        context,
        &request,
        AGENT_MAX_BYTES,
        Arc::new(move |path, cancel| validate_artifact(path, artifact, cancel)),
        None,
    )
    .await
    .map_err(|error| {
        format!(
            "准备 {} 失败：{error}。请检查网络或更换文件下载源后重试",
            artifact.id
        )
    })?;
    Ok(destination)
}

fn is_agent_argument(argument: &str) -> bool {
    ["-javaagent:", "-agentpath:", "-agentlib:", "-Xrun"]
        .iter()
        .any(|prefix| argument.starts_with(prefix))
}

fn is_module_argument(argument: &str) -> bool {
    matches!(
        argument,
        "-p" | "--module-path" | "--upgrade-module-path" | "-m" | "--module"
    ) || [
        "--module-path=",
        "--upgrade-module-path=",
        "--module=",
        "-Djdk.module.path=",
    ]
    .iter()
    .any(|prefix| argument.starts_with(prefix))
        || argument.starts_with('@')
}

fn push_argument<'a>(value: &'a Value, result: &mut Vec<&'a str>) {
    if let Some(argument) = value.as_str() {
        result.push(argument);
    } else if let Some(arguments) = value.as_array() {
        for argument in arguments {
            push_argument(argument, result);
        }
    } else if let Some(argument) = value.get("value") {
        // 未展开的条件 JVM 参数也可能引入模块路径；JLW 不接管这类启动入口。
        push_argument(argument, result);
    }
}

fn jvm_arguments<'a>(settings: &'a EffectiveSettings, data: &'a GameData) -> Vec<&'a str> {
    let mut result: Vec<_> = settings
        .jvm_args
        .iter()
        .chain(data.loader_extra_jvm_args.iter())
        .map(String::as_str)
        .collect();
    if let Some(arguments) = data
        .minecraft_json
        .arguments
        .as_ref()
        .and_then(|value| value.jvm.as_ref())
    {
        for argument in arguments {
            push_argument(argument, &mut result);
        }
    }
    result
}

fn affected_lwjgl(data: &GameData) -> bool {
    data.minecraft_json.libraries.iter().any(|library| {
        if library
            .rules
            .as_ref()
            .is_some_and(|rules| skip_library(rules))
        {
            return false;
        }
        let mut coordinate = library.name.split(':');
        matches!(
            (
                coordinate.next(),
                coordinate.next(),
                coordinate.next(),
                coordinate.next()
            ),
            (
                Some("org.lwjgl"),
                Some("lwjgl"),
                Some("3.4.0" | "3.4.1"),
                None
            )
        )
    })
}

#[derive(Debug, PartialEq, Eq)]
struct RequiredAgents {
    authlib: bool,
    wrapper: bool,
    lwjgl: bool,
}

fn required_agents(
    settings: &EffectiveSettings,
    data: &GameData,
    major: u32,
    windows_utf8: bool,
    non_ascii_path: bool,
    authlib: bool,
) -> RequiredAgents {
    let arguments = jvm_arguments(settings, data);
    let lwjgl = !settings.disable_lwjgl_agent
        && major >= 25
        && affected_lwjgl(data)
        && !arguments.iter().any(|argument| {
            argument.starts_with("-javaagent:") && argument.contains(LWJGL.filename)
        });
    let wrapper = !settings.disable_wrapper
        && windows_utf8
        && non_ascii_path
        && (6..=18).contains(&major)
        && !authlib
        && !arguments
            .iter()
            .any(|argument| is_agent_argument(argument) || is_module_argument(argument));
    RequiredAgents {
        authlib,
        wrapper,
        lwjgl,
    }
}

fn auth_server_url(raw: &str) -> Result<String, String> {
    use minecraft_java_rs_core::net::client::reqwest::Url;
    let raw = raw.trim();
    if raw.chars().any(char::is_control) {
        return Err("认证服务器地址包含控制字符，请重新填写".into());
    }
    let url = Url::parse(raw).map_err(|_| "请填写完整的 HTTP 或 HTTPS 认证服务器地址")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("认证服务器地址应为不含凭据、查询参数和片段的 HTTP 或 HTTPS 地址".into());
    }
    Ok(url.into())
}

#[cfg(windows)]
fn windows_utf8() -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetACP() -> u32;
    }
    // SAFETY: GetACP 无指针参数，仅返回当前 Windows ANSI 代码页。
    unsafe { GetACP() == UTF8_CODE_PAGE }
}

#[cfg(not(windows))]
fn windows_utf8() -> bool {
    false
}

fn non_ascii_launch_path(settings: &EffectiveSettings, data: &GameData, directory: &Path) -> bool {
    !directory.as_os_str().as_encoded_bytes().is_ascii()
        || !data.minecraft_java.path.is_ascii()
        || jvm_arguments(settings, data).iter().any(|argument| {
            !argument.is_ascii() && (argument.contains('/') || argument.contains('\\'))
        })
        || data.loader_libraries.iter().any(|library| {
            use minecraft_java_rs_core::models::minecraft::AssetItem;
            let path = match library {
                AssetItem::Asset { path, .. }
                | AssetItem::NativeAsset { path, .. }
                | AssetItem::CFile { path, .. } => path,
            };
            !path.is_ascii()
        })
}

fn needs_java_probe(
    settings: &EffectiveSettings,
    data: &GameData,
    windows_utf8: bool,
    non_ascii_path: bool,
    authlib: bool,
) -> bool {
    // 先检查与 Java 版本无关的条件；旧实例不使用新组件时不启动探测进程。
    required_agents(settings, data, 8, windows_utf8, non_ascii_path, authlib).wrapper
        || required_agents(settings, data, 25, windows_utf8, non_ascii_path, authlib).lwjgl
}

fn parse_java_major(text: &str) -> Option<u32> {
    text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if !matches!(parts.next(), Some("java" | "openjdk")) {
            return None;
        }
        let mut version = parts.next()?;
        if version == "version" {
            version = parts.next()?;
        }
        let version = version.trim_matches('"');
        let version = version.strip_prefix("1.").unwrap_or(version);
        let digits: String = version.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u32>().ok().filter(|major| *major > 0)
    })
}

pub(crate) async fn probe_java_major(path: &Path, cancel: &CancellationToken) -> Result<u32, String> {
    use std::process::Stdio;
    use std::time::Duration;
    const JAVA_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
    cancel.check()?;
    let mut executable = path.to_path_buf();
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("javaw.exe"))
    {
        let console = path.with_file_name("java.exe");
        if console.is_file() {
            executable = console;
        }
    }
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = tokio::select! {
        _ = cancel.cancelled() => return Err("操作已取消".into()),
        output = tokio::time::timeout(JAVA_PROBE_TIMEOUT, command.output()) =>
            output.map_err(|_| "Java 版本检测超时，请重新选择 Java 或关闭相应运行组件后重试")?
                .map_err(|error| format!("无法检测 Java 版本：{error}。请重新选择 Java"))?,
    };
    if !output.status.success() {
        return Err("Java 版本检测失败，请重新选择 Java 或关闭相应运行组件后重试".into());
    }
    let major = parse_java_major(&String::from_utf8_lossy(&output.stderr))
        .or_else(|| parse_java_major(&String::from_utf8_lossy(&output.stdout)))
        .ok_or("无法识别 Java 版本，请重新选择 Java 或关闭相应运行组件后重试")?;
    cancel.check()?;
    Ok(major)
}

/// 在 download_game 完成后、启动前调用；runtime_directory 是本次真实游戏运行目录。
pub async fn prepare(
    app: &tauri::AppHandle,
    settings: &EffectiveSettings,
    data: &GameData,
    auth_server: Option<&str>,
    runtime_directory: &Path,
) -> Result<AgentPlan, String> {
    let context = TransferContext::current();
    context.cancellation().check()?;
    let auth_server = auth_server.map(auth_server_url).transpose()?;
    let utf8 = windows_utf8();
    let non_ascii = non_ascii_launch_path(settings, data, runtime_directory);
    let major = if needs_java_probe(settings, data, utf8, non_ascii, auth_server.is_some()) {
        probe_java_major(Path::new(&data.minecraft_java.path), context.cancellation()).await?
    } else {
        0
    };
    let required = required_agents(
        settings,
        data,
        major,
        utf8,
        non_ascii,
        auth_server.is_some(),
    );
    if !required.authlib && !required.wrapper && !required.lwjgl {
        return Ok(AgentPlan::default());
    }
    let directory = crate::app_paths::config_dir(app)?.join(AGENTS_DIRECTORY);
    let mut plan = AgentPlan::default();
    if let Some(server) = auth_server {
        let path = ensure_artifact(&directory, AUTHLIB, &context).await?;
        plan.jvm_args
            .push(format!("-javaagent:{}={server}", path.display()));
        plan.jvm_args.push("-Dauthlibinjector.noLogFile".into());
    }
    if required.lwjgl {
        let path = ensure_artifact(&directory, LWJGL, &context).await?;
        plan.jvm_args.push(format!("-javaagent:{}", path.display()));
    }
    if required.wrapper {
        plan.wrapper = Some(ensure_artifact(&directory, WRAPPER, &context).await?);
    }
    context.cancellation().check()?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use minecraft_java_rs_core::net::transfer::SourcePolicy;
    use minecraft_java_rs_core::utils::archive::{ArchiveLimits, write_zip_files};
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const FIXTURE: Artifact = Artifact {
        id: "runtime-agent-fixture",
        filename: "fixture.jar",
        size: 431,
        sha256: "3b1cf14c3d4f515d0107f5654928811eba30fa0169ebfcef80433f076db8ae50",
        entry_class: "example.Main",
        ..WRAPPER
    };

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("nickel-agents-{}", crate::store::random_token()));
            std::fs::create_dir_all(&path).expect("创建隔离组件测试目录");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn context(policy: SourcePolicy) -> TransferContext {
        TransferContext::new(policy, policy, CancellationToken::new())
    }

    fn settings() -> EffectiveSettings {
        crate::runtime_settings::resolve(&Value::Null, &Value::Null, None)
            .expect("解析默认设置")
    }

    fn game(libraries: &[&str]) -> GameData {
        serde_json::from_value(serde_json::json!({
            "minecraftJson": {
                "id": "fixture-game", "type": "release",
                "libraries": libraries.iter().map(|name| serde_json::json!({"name":name})).collect::<Vec<_>>()
            },
            "minecraftVersion": "fixture-game",
            "minecraftJava": {"files":[], "path":"java"}
        })).expect("解析旧版 GameData 测试夹具")
    }

    // 复用 installer_download 的 ZIP_STORED 夹具，不为测试添加 zip 依赖。
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
            .expect("解码安装器 JAR 夹具")
    }

    #[test]
    fn wrapper_requires_windows_utf8_non_ascii_and_java_6_through_18() {
        let settings = settings();
        let data = game(&[]);
        for major in [0, 5, 6, 8, 17, 18, 19, 21, 25] {
            assert_eq!(
                required_agents(&settings, &data, major, true, true, false).wrapper,
                (6..=18).contains(&major),
                "Java {major}"
            );
        }
        assert!(!required_agents(&settings, &data, 8, false, true, false).wrapper);
        assert!(!required_agents(&settings, &data, 8, true, false, false).wrapper);
        let mut disabled = settings;
        disabled.disable_wrapper = true;
        assert!(!required_agents(&disabled, &data, 8, true, true, false).wrapper);
    }

    #[test]
    fn wrapper_skips_authlib_other_agents_and_module_launches() {
        let mut settings = settings();
        let mut data = game(&[]);
        assert!(!required_agents(&settings, &data, 8, true, true, true).wrapper);
        for argument in [
            "-javaagent:custom.jar",
            "-agentpath:C:/native.dll",
            "-agentlib:jdwp",
            "-Xrunjdwp:transport=dt_socket",
            "--module-path",
            "--module-path=modules",
            "-p",
            "--upgrade-module-path=modules",
            "--module=example/main",
            "@launch.args",
        ] {
            settings.jvm_args = vec![argument.into()];
            assert!(
                !required_agents(&settings, &data, 17, true, true, false).wrapper,
                "{argument}"
            );
        }
        settings.jvm_args.clear();
        data.loader_extra_jvm_args
            .push("--module-path=loader-modules".into());
        assert!(!required_agents(&settings, &data, 17, true, true, false).wrapper);
        data.loader_extra_jvm_args.clear();
        data.minecraft_json.arguments = Some(
            serde_json::from_value(serde_json::json!({
                "jvm": [{"value":["--module-path", "${library_directory}"]}]
            }))
            .expect("解析模块启动参数"),
        );
        assert!(!required_agents(&settings, &data, 17, true, true, false).wrapper);
    }

    #[test]
    fn lwjgl_uses_actual_base_library_version_and_java_25_boundary() {
        let mut settings = settings();
        for version in ["3.3.3", "3.4.0", "3.4.1", "3.4.2", "3.4.10"] {
            let library = format!("org.lwjgl:lwjgl:{version}");
            let data = game(&[&library]);
            for major in [8, 17, 21, 24, 25, 26] {
                assert_eq!(
                    required_agents(&settings, &data, major, true, false, false).lwjgl,
                    major >= 25 && matches!(version, "3.4.0" | "3.4.1"),
                    "{version} / {major}"
                );
            }
        }
        for library in [
            "org.lwjgl:lwjgl:3.4.1:natives-windows",
            "org.lwjgl:lwjgl-glfw:3.4.1",
            "other:lwjgl:3.4.1",
        ] {
            assert!(!required_agents(&settings, &game(&[library]), 25, true, false, false).lwjgl);
        }
        let data = game(&["org.lwjgl:lwjgl:3.4.1"]);
        settings.disable_lwjgl_agent = true;
        assert!(!required_agents(&settings, &data, 25, true, false, false).lwjgl);
        settings.disable_lwjgl_agent = false;
        settings.jvm_args = vec![format!("-javaagent:C:/agents/{}", LWJGL.filename)];
        assert!(!required_agents(&settings, &data, 25, true, false, false).lwjgl);
    }

    #[test]
    fn lwjgl_ignores_platform_excluded_libraries() {
        let mut data = game(&["org.lwjgl:lwjgl:3.4.1"]);
        data.minecraft_json.libraries[0].rules = Some(
            serde_json::from_value(serde_json::json!([
                {"action":"disallow"}
            ]))
            .expect("解析排除规则"),
        );
        assert!(!required_agents(&settings(), &data, 25, true, false, false).lwjgl);
    }

    #[test]
    fn authlib_only_follows_explicit_server_and_does_not_mutate_user_arguments() {
        let mut settings = settings();
        settings.jvm_args = vec!["-Dcustom=value".into(), "-Xmx3G".into()];
        let original = settings.jvm_args.clone();
        let data = game(&[]);
        assert!(!required_agents(&settings, &data, 21, true, false, false).authlib);
        assert!(required_agents(&settings, &data, 21, true, false, true).authlib);
        assert_eq!(settings.jvm_args, original);
        assert_eq!(
            auth_server_url("https://example.com/api/yggdrasil").expect("合法地址"),
            "https://example.com/api/yggdrasil"
        );
        for invalid in [
            "",
            "file:///tmp/server",
            "https://user:password@example.com/",
            "https://example.com/?token=secret",
            "https://example.com/#secret",
            "https://example.com/\npath",
        ] {
            let error = auth_server_url(invalid).expect_err("拒绝含凭据或非认证地址");
            assert!(!error.contains("secret"));
            assert!(!error.contains("password"));
        }
    }

    #[test]
    fn manifest_requires_single_main_section_entry_and_unfolds_lines() {
        assert_eq!(
            manifest_attribute(
                b"Manifest-Version: 1.0\r\nPremain-Class: example.\r\n Main\r\n\r\n",
                "Premain-Class"
            )
            .expect("展开续行"),
            "example.Main"
        );
        for manifest in [
            b"\r\nPremain-Class: example.Main\r\n".as_slice(),
            b"Premain-Class: example.Main\nPremain-Class: other.Main\n",
            b" no-header\n",
            b"Premain-Class: \n",
            b"Premain-Class:example.Main\n",
        ] {
            assert!(manifest_attribute(manifest, "Premain-Class").is_err());
        }
    }

    #[test]
    fn agent_jar_needs_premain_and_a_real_class_but_not_main_class() {
        let directory = TestDirectory::new();
        let original = directory.0.join("original.jar");
        std::fs::write(&original, executable_jar()).expect("写入测试 JAR");
        let cancel = CancellationToken::new();
        let class = read_zip_entry(&original, "example/Main.class", CLASS_MAX_BYTES, &cancel)
            .expect("读取合法类");
        let entries = vec![
            (
                MANIFEST_ENTRY.into(),
                b"Manifest-Version: 1.0\r\nPremain-Class: example.Main\r\n\r\n".to_vec(),
            ),
            ("example/Main.class".into(), class),
        ];
        let path = directory.0.join("agent.jar");
        write_zip_files(
            &path,
            &directory.0,
            &[],
            &entries,
            &ArchiveLimits::default(),
            &cancel,
            |_, _, _| {},
        )
        .expect("生成不含 Main-Class 的 agent 夹具");
        let artifact = Artifact {
            manifest_key: "Premain-Class",
            ..FIXTURE
        };
        validate_artifact(&path, artifact, &cancel).expect("合法 Premain JAR");
        assert!(validate_artifact(&path, FIXTURE, &cancel).is_err());
        assert!(validate_artifact(&original, artifact, &cancel).is_err());
        for (index, bytes) in [b"".as_slice(), b"<html>error</html>", b"PK\x03\x04broken"]
            .iter()
            .enumerate()
        {
            let invalid = directory.0.join(format!("invalid-{index}.jar"));
            std::fs::write(&invalid, bytes).expect("写入损坏夹具");
            assert!(validate_artifact(&invalid, artifact, &cancel).is_err());
        }
    }

    #[tokio::test]
    async fn valid_cache_is_offline_without_any_source_or_metadata() {
        let directory = TestDirectory::new();
        let path = directory.0.join(FIXTURE.filename);
        std::fs::write(&path, executable_jar()).expect("写入缓存");
        let context = context(SourcePolicy::OfficialAuto);
        let actual = ensure_artifact_from(&directory.0, FIXTURE, &context, vec![])
            .await
            .expect("有效缓存无需下载候选或在线目录");
        assert_eq!(actual, path);
        assert_eq!(std::fs::read(&path).expect("读取缓存"), executable_jar());
    }

    struct TestServer {
        base: String,
        requests: Arc<Mutex<Vec<String>>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn serve(official: Vec<u8>, mirror: Vec<u8>) -> TestServer {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("绑定本地测试源");
        let base = format!("http://{}", listener.local_addr().expect("读取测试源端口"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.expect("接受下载连接");
                let mut request = Vec::new();
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    assert!(request.len() < 8192);
                    if stream.read_buf(&mut request).await.expect("读取下载请求") == 0 {
                        break;
                    }
                }
                let line = String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let body = if line.contains("/mirror.jar") {
                    &mirror
                } else {
                    &official
                };
                captured.lock().expect("锁定请求记录").push(line);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if stream.write_all(header.as_bytes()).await.is_ok() {
                    let _ = stream.write_all(body).await;
                }
            }
        });
        TestServer {
            base,
            requests,
            task,
        }
    }

    fn test_candidates(server: &TestServer) -> Vec<RequestCandidate> {
        vec![
            RequestCandidate::official(format!("{}/official.jar", server.base)),
            RequestCandidate::mirror(format!("{}/mirror.jar", server.base)),
        ]
    }

    #[tokio::test]
    async fn corrupted_cache_retries_alternate_source_then_commits_atomically() {
        let directory = TestDirectory::new();
        let path = directory.0.join(FIXTURE.filename);
        std::fs::write(&path, b"old-corrupted-cache").expect("写入旧缓存");
        let mut corrupt = executable_jar();
        corrupt[0] ^= 1;
        let server = serve(corrupt, executable_jar()).await;
        let context = context(SourcePolicy::OfficialFirst);
        tokio::time::timeout(
            Duration::from_secs(10),
            ensure_artifact_from(&directory.0, FIXTURE, &context, test_candidates(&server)),
        )
        .await
        .expect("下载有界")
        .expect("备用源恢复缓存");
        assert_eq!(std::fs::read(&path).expect("读取新缓存"), executable_jar());
        let requests = server.requests.lock().expect("读取请求记录");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("/official.jar"));
        assert!(requests[1].contains("/mirror.jar"));
        assert_eq!(
            std::fs::read_dir(&directory.0)
                .expect("读取缓存目录")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn mirror_first_uses_shared_policy_and_valid_cache_makes_zero_requests() {
        let directory = TestDirectory::new();
        let server = serve(vec![], executable_jar()).await;
        let context = context(SourcePolicy::MirrorFirst);
        let sources = test_candidates(&server);
        ensure_artifact_from(&directory.0, FIXTURE, &context, sources.clone())
            .await
            .expect("镜像优先下载");
        assert_eq!(server.requests.lock().expect("读取请求数").len(), 1);
        assert!(server.requests.lock().expect("读取请求")[0].contains("/mirror.jar"));
        ensure_artifact_from(&directory.0, FIXTURE, &context, sources)
            .await
            .expect("有效缓存复用");
        assert_eq!(server.requests.lock().expect("读取缓存请求数").len(), 1);
    }

    #[tokio::test]
    async fn failed_or_cancelled_download_preserves_existing_file() {
        let directory = TestDirectory::new();
        let path = directory.0.join(FIXTURE.filename);
        let original = b"old-invalid-cache";
        std::fs::write(&path, original).expect("写入旧文件");
        let server = serve(
            vec![0; FIXTURE.size as usize],
            vec![0; FIXTURE.size as usize],
        )
        .await;
        let context = context(SourcePolicy::OfficialFirst);
        let error = tokio::time::timeout(
            Duration::from_secs(10),
            ensure_artifact_from(&directory.0, FIXTURE, &context, test_candidates(&server)),
        )
        .await
        .expect("失败有界")
        .expect_err("全部校验失败");
        assert!(error.contains("校验"));
        assert_eq!(std::fs::read(&path).expect("读取旧文件"), original);
        assert_eq!(
            std::fs::read_dir(&directory.0).expect("读取目录").count(),
            1
        );
        let previous = server.requests.lock().expect("读取失败次数").len();
        context.cancellation().cancel();
        let error = ensure_artifact_from(&directory.0, FIXTURE, &context, test_candidates(&server))
            .await
            .expect_err("已取消");
        assert!(error.contains("取消"));
        assert_eq!(
            server.requests.lock().expect("读取取消后次数").len(),
            previous
        );
        assert_eq!(std::fs::read(&path).expect("读取原文件"), original);
    }

    #[tokio::test]
    async fn cancellation_during_transfer_never_publishes_partial_jar() {
        let directory = TestDirectory::new();
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("绑定取消测试源");
        let source = format!(
            "http://{}/slow.jar",
            listener.local_addr().expect("读取端口")
        );
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("接受取消测试连接");
            let mut request = [0; 4096];
            stream.read(&mut request).await.expect("读取取消测试请求");
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                FIXTURE.size
            );
            stream.write_all(header.as_bytes()).await.expect("发送头");
            stream.write_all(b"P").await.expect("发送部分内容");
            let _ = started_tx.send(());
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
        let server = TestServer {
            base: source.clone(),
            requests: Arc::new(Mutex::new(Vec::new())),
            task,
        };
        let context = context(SourcePolicy::OfficialFirst);
        let operation = ensure_artifact_from(
            &directory.0,
            FIXTURE,
            &context,
            vec![RequestCandidate::official(source)],
        );
        tokio::pin!(operation);
        tokio::select! {
            _ = tokio::time::timeout(Duration::from_secs(5), started_rx) => {},
            result = &mut operation => panic!("取消前提前结束：{result:?}"),
        }
        context.cancellation().cancel();
        let error = tokio::time::timeout(Duration::from_secs(2), operation)
            .await
            .expect("取消及时结束")
            .expect_err("中止传输");
        assert!(error.contains("取消"));
        assert!(!directory.0.join(FIXTURE.filename).exists());
        drop(server);
    }

    #[test]
    fn java_major_parses_legacy_modern_and_early_access_outputs() {
        for (output, expected) in [
            ("java version \"1.6.0_45\"", 6),
            ("java version \"1.8.0_392\"", 8),
            ("openjdk version \"17.0.12\" 2024-07-16", 17),
            ("openjdk 25 2025-09-16", 25),
            ("openjdk version \"26-ea\"", 26),
        ] {
            assert_eq!(parse_java_major(output), Some(expected));
        }
        for output in [
            "",
            "Error: java.dll not found",
            "version \"25\"",
            "openjdk version \"bad\"",
        ] {
            assert_eq!(parse_java_major(output), None);
        }
    }

    #[test]
    fn ordinary_cached_instances_need_neither_version_probe_nor_network_catalog() {
        let mut settings = settings();
        let data = game(&[]);
        assert!(!needs_java_probe(&settings, &data, false, true, false));
        assert!(!needs_java_probe(&settings, &data, true, false, false));
        assert!(!needs_java_probe(&settings, &data, true, true, true));
        settings.disable_wrapper = true;
        settings.disable_lwjgl_agent = true;
        assert!(!needs_java_probe(
            &settings,
            &game(&["org.lwjgl:lwjgl:3.4.1"]),
            true,
            true,
            false
        ));
    }

    #[test]
    fn non_ascii_detection_uses_actual_directory_and_not_the_instance_label() {
        let settings = settings();
        let mut data = game(&[]);
        assert!(non_ascii_launch_path(
            &settings,
            &data,
            Path::new("C:/游戏/versions/example")
        ));
        data.minecraft_version = "展示名称".into();
        assert!(!non_ascii_launch_path(
            &settings,
            &data,
            Path::new("C:/games/example")
        ));
        data.minecraft_java.path = "C:/运行时/bin/javaw.exe".into();
        assert!(non_ascii_launch_path(
            &settings,
            &data,
            Path::new("C:/games/example")
        ));
    }
}
