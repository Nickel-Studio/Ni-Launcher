//! API 配置优先读取运行时覆盖，最后回退构建内嵌的四字段白名单。
//! 缺失、不可读或解析失败的字段跳过；用户凭据不参与构建投影。
//! CurseForge Key 仅供后端请求，测试占位符永远不作为认证值发送。

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::AppHandle;

// Build and runtime configuration share one whitelist and field-validation contract.
#[allow(dead_code)]
#[path = "../build_config.rs"]
mod build_config;
#[allow(dead_code)]
#[path = "../embedded_config.rs"]
mod embedded_config;
use embedded_config::Field;
use zeroize::Zeroizing;

/// 返回给前端的 Microsoft 登录配置（序列化为 camelCase）。
///
/// 仅包含 Device Code 流程所需的最小字段；`objectId` / `tenantId` 等存档字段不外泄。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MsaConfig {
    pub client_id: String,
    pub authority: String,
    pub redirect_uri: String,
}

/// `authority` 缺省值：个人微软账号。
const DEFAULT_AUTHORITY: &str = "consumers";
/// `redirectUri` 缺省值。
const DEFAULT_REDIRECT_URI: &str = "nickel://auth";
/// 密钥文件名。
const SECRETS_FILE_NAME: &str = "secrets.local.json";
const EMBEDDED_CONFIG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/nickel-embedded-config.bin"));
const TEST_CURSEFORGE_KEY: &str = build_config::TEST_CURSEFORGE_KEY;

/// Backend-only credential handle. Clones copy encrypted bytes, not the plaintext API key.
#[derive(Clone, Default)]
pub struct CurseforgeCredential(Vec<u8>);

impl std::fmt::Debug for CurseforgeCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CurseforgeCredential([REDACTED])")
    }
}

impl CurseforgeCredential {
    fn from_text(value: &str) -> Option<Self> {
        let sealed = embedded_config::seal(&[(Field::CurseforgeKey, value)], |bytes| {
            getrandom::fill(bytes).map_err(|_| "Runtime randomness unavailable")
        }).ok()?;
        Some(Self(sealed))
    }

    pub(crate) fn open(&self) -> Option<Zeroizing<String>> {
        embedded_config::open(&self.0, Field::CurseforgeKey).ok().flatten()
    }

}

fn project_runtime_config(text: &str) -> Result<build_config::SensitiveDocument, String> {
    build_config::project(text)
        .map(build_config::SensitiveDocument)
        .map_err(|error| format!("解析 {SECRETS_FILE_NAME} 失败：{error}"))
}

/// 解析密钥文件文本 → [`MsaConfig`]。
///
/// - 缺 `microsoft` 段、或 `clientId` 缺失 / 为空 → `Ok(None)`（视为未配置）
/// - JSON 非法 → `Err`（错误信息只含行 / 列，不含任何密钥内容）
/// - `authority` / `redirectUri` 缺失或为空白 → 使用缺省值
fn parse_msa_config(text: &str) -> Result<Option<MsaConfig>, String> {
    let value = project_runtime_config(text)?;
    let section = &value["microsoft"];
    let Some(client_id) = section["clientId"].as_str() else {
        return Ok(None);
    };
    Ok(Some(MsaConfig {
        client_id: client_id.to_owned(),
        authority: section["authority"]
            .as_str()
            .unwrap_or(DEFAULT_AUTHORITY)
            .to_owned(),
        redirect_uri: section["redirectUri"]
            .as_str()
            .unwrap_or(DEFAULT_REDIRECT_URI)
            .to_owned(),
    }))
}

/// 解析密钥文件文本 → CurseForge API Key。
///
/// - 缺 `curseforge` 段、或 `apiKey` 缺失 / 为空 → `Ok(None)`（视为未配置）
/// - JSON 非法 → `Err`（错误信息只含行 / 列，不含任何密钥内容）
fn parse_curseforge_key(text: &str) -> Result<Option<String>, String> {
    let value = project_runtime_config(text)?;
    Ok(value["curseforge"]["apiKey"]
        .as_str()
        .filter(|key| *key != TEST_CURSEFORGE_KEY)
        .map(str::to_owned))
}

fn read_config_text(path: &Path) -> Option<Zeroizing<String>> {
    let file = std::fs::File::open(path).ok()?;
    let mut text = Zeroizing::new(String::with_capacity(build_config::MAX_CONFIG_BYTES + 1));
    file.take(build_config::MAX_CONFIG_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// 读取单个路径的密钥文件。
///
/// 文件不存在 / 不可读 / 解析失败 / 未配置 → `None`（静默跳过，绝不 panic）。
fn read_msa_config_from(path: &Path) -> Option<MsaConfig> {
    let text = read_config_text(path)?;
    parse_msa_config(&text).ok().flatten()
}

/// 读取单个路径的 CurseForge API Key（语义与 [`read_msa_config_from`] 一致）。
fn read_curseforge_key_from(path: &Path) -> Option<CurseforgeCredential> {
    let text = read_config_text(path)?;
    let key = Zeroizing::new(parse_curseforge_key(&text).ok()??);
    CurseforgeCredential::from_text(&key)
}

/// 按顺序取第一个命中的配置（缺失的路径自动跳过）。
fn first_msa_config<I: IntoIterator<Item = PathBuf>>(paths: I) -> Option<MsaConfig> {
    paths.into_iter().find_map(|p| read_msa_config_from(&p))
}

/// 按顺序取第一个命中的 CurseForge API Key（缺失的路径自动跳过）。
fn first_curseforge_key<I: IntoIterator<Item = PathBuf>>(paths: I) -> Option<CurseforgeCredential> {
    paths.into_iter().find_map(|p| read_curseforge_key_from(&p))
}

fn embedded_msa_config() -> Option<MsaConfig> {
    let client = embedded_config::open(EMBEDDED_CONFIG, Field::MicrosoftClient).ok()??;
    let authority = embedded_config::open(EMBEDDED_CONFIG, Field::MicrosoftAuthority).ok()??;
    let redirect = embedded_config::open(EMBEDDED_CONFIG, Field::MicrosoftRedirect).ok()??;
    Some(MsaConfig {
        client_id: client.to_string(),
        authority: authority.to_string(),
        redirect_uri: redirect.to_string(),
    })
}

fn embedded_curseforge_key() -> Option<CurseforgeCredential> {
    let credential = CurseforgeCredential(EMBEDDED_CONFIG.to_vec());
    let key = credential.open()?;
    (key.as_str() != TEST_CURSEFORGE_KEY).then_some(credential)
}

fn load_msa_config<I: IntoIterator<Item = PathBuf>>(paths: I) -> Option<MsaConfig> {
    first_msa_config(paths).or_else(embedded_msa_config)
}

fn load_curseforge_key<I: IntoIterator<Item = PathBuf>>(paths: I) -> Option<CurseforgeCredential> {
    first_curseforge_key(paths).or_else(embedded_curseforge_key)
}

/// 开发根仅用于调试构建；发行程序不会读取构建机的源码路径。
#[cfg(debug_assertions)]
fn dev_fallback_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(SECRETS_FILE_NAME)
}

fn runtime_candidate_paths(
    primary: Option<&str>,
    config_directory: Option<&Path>,
    executable_directory: Option<&Path>,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = primary.into_iter()
        .map(str::trim).filter(|path| !path.is_empty()).map(PathBuf::from).collect();
    for directory in [config_directory, executable_directory]
        .into_iter()
        .flatten()
    {
        paths.push(directory.join(SECRETS_FILE_NAME));
    }
    #[cfg(debug_assertions)]
    paths.push(dev_fallback_path());
    paths
}

fn candidate_paths(app: &AppHandle) -> Vec<PathBuf> {
    let primary = std::env::var("NICKEL_SECRETS").ok();
    let config = crate::app_paths::config_dir(app).ok();
    let executable = crate::app_paths::executable_directory_for(app).ok();
    runtime_candidate_paths(
        primary.as_deref(),
        config.as_deref(),
        executable.as_deref(),
    )
}

/// 公共登录配置可发送给设备码登录界面；优先采用运行时覆盖。
#[tauri::command]
pub fn msa_config_get(app: AppHandle) -> Result<Option<MsaConfig>, String> {
    Ok(load_msa_config(candidate_paths(&app)))
}

/// 不注册为前端 IPC；真实 Key 只在后端用于官方请求。
pub fn curseforge_api_key(app: AppHandle) -> Result<Option<CurseforgeCredential>, String> {
    Ok(load_curseforge_key(candidate_paths(&app)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential_text(value: Option<CurseforgeCredential>) -> Option<String> {
        value.and_then(|credential| credential.open()).map(|value| value.to_string())
    }

    const SAMPLE: &str = r#"{
      "microsoft": {
        "clientId": "11111111-2222-3333-4444-555555555555",
        "objectId": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        "tenantId": "ffffffff-0000-1111-2222-333333333333",
        "authority": "common",
        "redirectUri": "nickel://auth"
      },
      "curseforge": { "apiKey": null }
    }"#;

    /// 生成唯一的临时文件路径。
    fn temp_path(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "nickel_secrets_{}_{}_{}_{}.json",
            std::process::id(),
            stamp,
            SEQUENCE.fetch_add(1, Ordering::Relaxed),
            tag
        ))
    }

    #[test]
    fn parses_config_and_ignores_unknown_fields() {
        let cfg = parse_msa_config(SAMPLE).unwrap().unwrap();
        assert_eq!(cfg.client_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(cfg.authority, "common");
        assert_eq!(cfg.redirect_uri, "nickel://auth");
    }

    #[test]
    fn applies_defaults_when_optional_missing() {
        let cfg = parse_msa_config(r#"{ "microsoft": { "clientId": "abc" } }"#)
            .unwrap()
            .unwrap();
        assert_eq!(cfg.authority, DEFAULT_AUTHORITY);
        assert_eq!(cfg.redirect_uri, DEFAULT_REDIRECT_URI);
    }

    #[test]
    fn blank_optional_values_fall_back_to_defaults() {
        let cfg = parse_msa_config(r#"{ "microsoft": { "clientId": "abc", "authority": "  " } }"#)
            .unwrap()
            .unwrap();
        assert_eq!(cfg.authority, DEFAULT_AUTHORITY);
        assert_eq!(cfg.redirect_uri, DEFAULT_REDIRECT_URI);
    }

    #[test]
    fn missing_microsoft_section_is_none() {
        assert!(parse_msa_config("{}").unwrap().is_none());
        assert!(
            parse_msa_config(r#"{ "curseforge": { "apiKey": null } }"#)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn empty_client_id_is_none() {
        assert!(
            parse_msa_config(r#"{ "microsoft": { "clientId": "   " } }"#)
                .unwrap()
                .is_none()
        );
        assert!(
            parse_msa_config(r#"{ "microsoft": { "objectId": "x" } }"#)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn invalid_json_is_err_without_secrets() {
        let err = parse_msa_config("{ not json").unwrap_err();
        // 错误信息只描述文件名 + 位置，不含任何密钥内容。
        assert!(err.contains(SECRETS_FILE_NAME));
        assert!(!err.contains("clientId"));
    }

    #[test]
    fn missing_file_returns_none() {
        assert!(read_msa_config_from(&temp_path("missing")).is_none());
    }

    #[test]
    fn reads_from_existing_file_and_skips_missing() {
        let missing = temp_path("ghost");
        let present = temp_path("present");
        std::fs::write(&present, SAMPLE).unwrap();

        let cfg = first_msa_config(vec![missing, present.clone()]).unwrap();
        assert_eq!(cfg.client_id, "11111111-2222-3333-4444-555555555555");

        std::fs::remove_file(&present).ok();
    }

    #[test]
    fn serializes_as_camel_case() {
        let cfg = MsaConfig {
            client_id: "cid".to_string(),
            authority: DEFAULT_AUTHORITY.to_string(),
            redirect_uri: DEFAULT_REDIRECT_URI.to_string(),
        };
        let value = serde_json::to_value(&cfg).unwrap();
        assert_eq!(value.get("clientId").and_then(|v| v.as_str()), Some("cid"));
        assert_eq!(
            value.get("redirectUri").and_then(|v| v.as_str()),
            Some(DEFAULT_REDIRECT_URI)
        );
        // snake_case 字段名不得出现。
        assert!(value.get("client_id").is_none());
        assert!(value.get("redirect_uri").is_none());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dev_fallback_path_is_derived_without_accessing_project_secrets() {
        assert_eq!(
            dev_fallback_path(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join(SECRETS_FILE_NAME)
        );
        let path = temp_path("dev-fallback");
        std::fs::write(&path, SAMPLE).unwrap();
        let cfg = read_msa_config_from(&path).unwrap();
        assert_eq!(cfg.client_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(cfg.redirect_uri, DEFAULT_REDIRECT_URI);
        std::fs::remove_file(path).unwrap();
    }



    // ── CurseForge API Key 读取 ────────────────────────────────────────────

    const CF_SAMPLE: &str = r#"{
      "microsoft": { "clientId": "cid" },
      "curseforge": { "apiKey": "  $2a$10$TESTKEY  " }
    }"#;

    #[test]
    fn parses_curseforge_key_and_trims() {
        assert_eq!(
            parse_curseforge_key(CF_SAMPLE).unwrap(),
            Some("$2a$10$TESTKEY".to_string())
        );
        // 只有 curseforge 段也必须可用。
        assert_eq!(
            parse_curseforge_key(r#"{ "curseforge": { "apiKey": "abc" } }"#).unwrap(),
            Some("abc".to_string())
        );
    }

    #[test]
    fn missing_curseforge_key_is_none() {
        // 段缺失 / apiKey 为 null / 空白 —— 三种都视为未配置。
        assert_eq!(parse_curseforge_key("{}").unwrap(), None);
        assert_eq!(parse_curseforge_key(SAMPLE).unwrap(), None);
        assert_eq!(
            parse_curseforge_key(r#"{ "curseforge": { "apiKey": "   " } }"#).unwrap(),
            None
        );
        assert_eq!(
            parse_curseforge_key(r#"{ "curseforge": { "apiKey": null } }"#).unwrap(),
            None
        );
    }

    #[test]
    fn curseforge_key_errors_do_not_leak_key() {
        let err = parse_curseforge_key("{ not json").unwrap_err();
        assert!(err.contains(SECRETS_FILE_NAME));
        assert!(!err.contains("apiKey"), "{err}");
    }

    #[test]
    fn reads_curseforge_key_from_existing_file_and_skips_missing() {
        let missing = temp_path("cf-ghost");
        let present = temp_path("cf-present");
        std::fs::write(&present, CF_SAMPLE).unwrap();

        assert_eq!(
            credential_text(first_curseforge_key(vec![missing, present.clone()])),
            Some("$2a$10$TESTKEY".to_string())
        );

        std::fs::remove_file(&present).ok();
    }

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(tag: &str) -> Self {
            let path = temp_path(tag).with_extension("dir");
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn malformed_unrelated_sections_do_not_discard_valid_runtime_fields() {
        let microsoft = r#"{"microsoft":{"clientId":"fixture-client","authority":42,"redirectUri":[]},"curseforge":{"apiKey":false}}"#;
        let msa = parse_msa_config(microsoft).unwrap().unwrap();
        assert_eq!(msa.client_id, "fixture-client");
        assert_eq!(msa.authority, DEFAULT_AUTHORITY);
        assert_eq!(msa.redirect_uri, DEFAULT_REDIRECT_URI);
        assert!(parse_curseforge_key(microsoft).unwrap().is_none());
        let curseforge = r#"{"microsoft":{"clientId":[]},"curseforge":{"apiKey":"fixture-key"}}"#;
        assert_eq!(
            parse_curseforge_key(curseforge).unwrap().as_deref(),
            Some("fixture-key")
        );
        assert!(parse_msa_config(curseforge).unwrap().is_none());
        for text in ["null", "[]", "false", "42", r#""fixture""#] {
            assert!(parse_msa_config(text).unwrap().is_none());
            assert!(parse_curseforge_key(text).unwrap().is_none());
        }
    }

    #[test]
    fn runtime_rejects_control_characters_oversized_values_and_example_client_ids() {
        for invalid in [
            "bad\u{0000}value".into(),
            "bad\nvalue".into(),
            "x".repeat(8193),
        ] {
            let text = serde_json::json!({"microsoft":{"clientId":invalid},"curseforge":{"apiKey":invalid}}).to_string();
            assert!(parse_msa_config(&text).unwrap().is_none());
            assert!(parse_curseforge_key(&text).unwrap().is_none());
        }
        assert!(
            parse_msa_config(
                r#"{"microsoft":{"clientId":"00000000-0000-0000-0000-000000000000"}}"#
            )
            .unwrap()
            .is_none()
        );
        let optional = r#"{"microsoft":{"clientId":"fixture-client","authority":"bad\u0000value","redirectUri":"bad\nvalue"}}"#;
        let config = parse_msa_config(optional).unwrap().unwrap();
        assert_eq!(config.authority, DEFAULT_AUTHORITY);
        assert_eq!(config.redirect_uri, DEFAULT_REDIRECT_URI);
    }

    #[test]
    fn runtime_file_reads_are_bounded_and_bad_files_are_skipped() {
        let directory = TestDirectory::new("bounded");
        let oversized = directory.file("oversized.json");
        let invalid_utf8 = directory.file("invalid-utf8.json");
        let valid = directory.file("valid.json");
        std::fs::write(
            &oversized,
            " ".repeat(build_config::MAX_CONFIG_BYTES + 4096),
        )
        .unwrap();
        std::fs::write(&invalid_utf8, [0xff, 0xfe, 0x00]).unwrap();
        std::fs::write(&valid, CF_SAMPLE).unwrap();
        assert_eq!(
            read_config_text(&oversized).unwrap().len(),
            build_config::MAX_CONFIG_BYTES + 1
        );
        assert!(read_msa_config_from(&oversized).is_none());
        assert!(read_curseforge_key_from(&oversized).is_none());
        assert!(read_config_text(&invalid_utf8).is_none());
        assert!(read_msa_config_from(&directory.0).is_none());
        let paths = vec![oversized, invalid_utf8, valid];
        assert_eq!(first_msa_config(paths.clone()).unwrap().client_id, "cid");
        assert_eq!(
            credential_text(first_curseforge_key(paths)).as_deref(),
            Some("$2a$10$TESTKEY")
        );
    }

    #[test]
    fn runtime_parse_failures_never_include_fixture_credentials() {
        for text in [
            r#"{"microsoft":{"clientId":"private-client-fixture"},"curseforge":{"apiKey":"private-key-fixture"},broken}"#,
            r#"{"microsoft":{"clientId":"private-client-fixture"},"curseforge":{"apiKey":"private-key-fixture"}"#,
        ] {
            for error in [
                parse_msa_config(text).unwrap_err(),
                parse_curseforge_key(text).unwrap_err(),
            ] {
                assert!(!error.contains("private-client-fixture"));
                assert!(!error.contains("private-key-fixture"));
            }
        }
    }

    #[test]
    fn exact_curseforge_placeholder_is_never_an_authentication_value() {
        for key in [
            TEST_CURSEFORGE_KEY.to_owned(),
            format!("  {TEST_CURSEFORGE_KEY}\r\n"),
        ] {
            let text = serde_json::json!({"curseforge":{"apiKey":key}}).to_string();
            assert!(parse_curseforge_key(&text).unwrap().is_none());
        }
        let different_fixture = format!("{TEST_CURSEFORGE_KEY}-different-fixture");
        let text = serde_json::json!({"curseforge":{"apiKey":different_fixture}}).to_string();
        assert!(parse_curseforge_key(&text).unwrap().is_some());
        let directory = TestDirectory::new("placeholder");
        let placeholder = directory.file("placeholder.json");
        let valid = directory.file("valid.json");
        std::fs::write(
            &placeholder,
            serde_json::json!({"curseforge":{"apiKey":TEST_CURSEFORGE_KEY}}).to_string(),
        )
        .unwrap();
        std::fs::write(&valid, CF_SAMPLE).unwrap();
        assert_eq!(
            credential_text(load_curseforge_key(vec![placeholder, valid])).as_deref(),
            Some("$2a$10$TESTKEY")
        );
        let embedded = credential_text(load_curseforge_key(Vec::<PathBuf>::new()));
        assert!(embedded.as_deref() != Some(TEST_CURSEFORGE_KEY));
    }

    #[test]
    fn runtime_candidate_order_is_override_config_executable_then_debug_only_source() {
        let directory = TestDirectory::new("candidate-order");
        let primary = directory.file("primary.json");
        let config = directory.file("config");
        let executable = directory.file("executable");
        let paths = runtime_candidate_paths(
            Some(&primary.to_string_lossy()),
            Some(&config),
            Some(&executable),
        );
        assert_eq!(
            &paths[..3],
            &[
                primary,
                config.join(SECRETS_FILE_NAME),
                executable.join(SECRETS_FILE_NAME)
            ]
        );
        #[cfg(debug_assertions)]
        {
            assert_eq!(paths.len(), 4);
            assert_eq!(paths.last(), Some(&dev_fallback_path()));
        }
        #[cfg(not(debug_assertions))]
        assert_eq!(paths.len(), 3);
    }

    #[test]
    fn runtime_precedence_and_missing_fields_are_resolved_independently() {
        let directory = TestDirectory::new("precedence");
        let primary = directory.file("primary.json");
        let config = directory.file("config");
        let executable = directory.file("executable");
        std::fs::create_dir(&config).unwrap();
        std::fs::create_dir(&executable).unwrap();
        let paths = runtime_candidate_paths(
            Some(&primary.to_string_lossy()),
            Some(&config),
            Some(&executable),
        )
        .into_iter()
        .take(3)
        .collect::<Vec<_>>();
        for (path, name) in paths
            .iter()
            .zip(["primary", "config", "executable"])
        {
            std::fs::write(
                path,
                serde_json::json!({
                    "microsoft":{"clientId":format!("{name}-client")},
                    "curseforge":{"apiKey":format!("{name}-key")}
                })
                .to_string(),
            )
            .unwrap();
        }
        for (index, name) in ["primary", "config", "executable"]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                load_msa_config(paths.clone()).unwrap().client_id,
                format!("{name}-client")
            );
            assert_eq!(
                credential_text(load_curseforge_key(paths.clone())).unwrap(),
                format!("{name}-key")
            );
            std::fs::write(&paths[index], "broken").unwrap();
        }
        std::fs::write(
            &primary,
            r#"{"microsoft":{"clientId":"primary-client"},"curseforge":{"apiKey":false}}"#,
        )
        .unwrap();
        std::fs::write(
            config.join(SECRETS_FILE_NAME),
            r#"{"microsoft":{"clientId":false},"curseforge":{"apiKey":"config-key"}}"#,
        )
        .unwrap();
        assert_eq!(
            load_msa_config(paths.clone()).unwrap().client_id,
            "primary-client"
        );
        assert_eq!(credential_text(load_curseforge_key(paths)).as_deref(), Some("config-key"));
    }

    #[test]
    fn missing_runtime_paths_fall_back_to_the_embedded_configuration() {
        let missing = temp_path("embedded-fallback");
        assert!(
            load_msa_config(vec![missing.clone()]) == embedded_msa_config()
        );
        assert!(
            credential_text(load_curseforge_key(vec![missing])) == credential_text(embedded_curseforge_key())
        );
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_candidates_never_include_the_build_source_root() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../secrets.local.json");
        assert!(runtime_candidate_paths(None, None, None).is_empty());
        let directory = TestDirectory::new("release-candidates");
        let paths = runtime_candidate_paths(None, Some(&directory.0), Some(&directory.0));
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().all(|path| path != &source));
    }

    #[cfg(not(debug_assertions))]
    #[test]
    #[ignore = "Detached release acceptance; run only through tests/embedded-config/verify.mjs"]
    fn detached_release_runtime_loads_embedded_configuration_without_external_files() {
        let directory = PathBuf::from(
            std::env::var_os("NICKEL_EMBEDDED_CONFIG_PROBE_DIR").expect("detached probe directory"),
        );
        let executable = std::env::current_exe().expect("probe executable");
        assert!(
            std::env::current_dir().unwrap() == directory,
            "probe must run outside the source directory"
        );
        assert!(
            executable.parent() == Some(directory.as_path()),
            "probe executable must be detached"
        );
        assert!(
            !Path::new(env!("CARGO_MANIFEST_DIR")).exists(),
            "isolated build manifest must be removed before runtime acceptance"
        );
        for key in [
            "NICKEL_SECRETS",
            "NICKEL_BUILD_SECRETS",
            "NICKEL_MSA_CLIENT_ID",
            "NICKEL_MSA_AUTHORITY",
            "NICKEL_MSA_REDIRECT_URI",
            "NICKEL_CURSEFORGE_API_KEY",
        ] {
            assert!(
                std::env::var_os(key).is_none(),
                "runtime probe must not inherit configuration overrides"
            );
        }
        let config = directory.join("config");
        let paths = runtime_candidate_paths(None, Some(&config), executable.parent());
        assert_eq!(paths.len(), 2);
        assert!(
            paths.iter().all(|path| !path.exists()),
            "no external configuration may be supplied to the detached probe"
        );
        let embedded = build_config::SensitiveDocument(
            embedded_config::snapshot(EMBEDDED_CONFIG).expect("embedded configuration is valid"),
        );
        let text = Zeroizing::new(embedded.to_string());
        let projected = build_config::SensitiveDocument(build_config::project(&text).unwrap());
        assert!(
            *embedded == *projected,
            "embedded configuration must contain only the four allowed fields"
        );
        let expected_msa = embedded_msa_config();
        let msa = load_msa_config(paths.clone());
        assert!(
            expected_msa.is_some(),
            "microsoft.clientId must be embedded for this acceptance"
        );
        assert!(
            msa == expected_msa,
            "runtime Microsoft configuration must match the embedded configuration"
        );
        let curseforge = credential_text(load_curseforge_key(paths));
        assert!(
            curseforge == credential_text(embedded_curseforge_key()),
            "runtime CurseForge configuration must match the embedded configuration"
        );
        let placeholder = embedded["curseforge"]["apiKey"].as_str() == Some(TEST_CURSEFORGE_KEY);
        assert!(
            !placeholder || curseforge.is_none(),
            "CurseForge test placeholder must not leave the runtime loader"
        );
        println!(
            "NICKEL_RUNTIME_PROBE={}",
            serde_json::json!({
                "releaseCompilation": !cfg!(debug_assertions),
                "executableDetached": true,
                "buildManifestUnavailable": true,
                "externalConfigurationAbsent": true,
                "runtimeOverridesAbsent": true,
                "sourceRootExcluded": true,
                "embeddedWhitelistOnly": true,
                "runtimeMatchesEmbedded": true,
                "microsoft.clientId": msa.as_ref().is_some_and(|value| !value.client_id.is_empty()),
                "microsoft.authority": msa.as_ref().is_some_and(|value| !value.authority.is_empty()),
                "microsoft.redirectUri": msa.as_ref().is_some_and(|value| !value.redirect_uri.is_empty()),
                "curseforge.apiKeyEmbedded": embedded["curseforge"]["apiKey"].as_str().is_some(),
                "curseforge.apiKeyUsable": curseforge.is_some(),
                "curseforge.testPlaceholder": placeholder,
                "curseforge.testPlaceholderRejected": !placeholder || curseforge.is_none()
            })
        );
    }
}
