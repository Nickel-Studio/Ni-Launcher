//! 密钥候选顺序为 NICKEL_SECRETS、兼容别名 NOWHERE_SECRETS、选定配置根和开发根。
//! 只使用候选中有效的所需字段；缺失、不可读或解析失败时返回 None。
//! 密钥文件内容不进入日志或错误信息，CurseForge Key 仅供后端请求使用。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

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

/// `secrets.local.json` 的 `microsoft` 段（字段名经 camelCase 映射，未知字段忽略）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MicrosoftSection {
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    authority: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
}

/// `secrets.local.json` 的 `curseforge` 段（CurseForge 社区资源检索引擎）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurseForgeSection {
    #[serde(default)]
    api_key: Option<String>,
}

/// `secrets.local.json` 顶层（允许存在 `curseforge` 等额外字段）。
#[derive(Debug, Deserialize)]
struct SecretsFile {
    #[serde(default)]
    microsoft: Option<MicrosoftSection>,
    #[serde(default)]
    curseforge: Option<CurseForgeSection>,
}

/// `authority` 缺省值：个人微软账号。
const DEFAULT_AUTHORITY: &str = "consumers";
/// `redirectUri` 缺省值。
const DEFAULT_REDIRECT_URI: &str = "nowhere://auth";
/// 密钥文件名。
const SECRETS_FILE_NAME: &str = "secrets.local.json";

/// 解析密钥文件文本 → [`MsaConfig`]。
///
/// - 缺 `microsoft` 段、或 `clientId` 缺失 / 为空 → `Ok(None)`（视为未配置）
/// - JSON 非法 → `Err`（错误信息只含行 / 列，不含任何密钥内容）
/// - `authority` / `redirectUri` 缺失或为空白 → 使用缺省值
fn parse_msa_config(text: &str) -> Result<Option<MsaConfig>, String> {
    let file: SecretsFile =
        serde_json::from_str(text).map_err(|e| format!("解析 {SECRETS_FILE_NAME} 失败: {e}"))?;

    let Some(section) = file.microsoft else {
        return Ok(None);
    };

    let client_id = section.client_id.unwrap_or_default().trim().to_string();
    if client_id.is_empty() {
        return Ok(None);
    }

    let authority = section
        .authority
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_AUTHORITY.to_string());

    let redirect_uri = section
        .redirect_uri
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_REDIRECT_URI.to_string());

    Ok(Some(MsaConfig {
        client_id,
        authority,
        redirect_uri,
    }))
}

/// 解析密钥文件文本 → CurseForge API Key。
///
/// - 缺 `curseforge` 段、或 `apiKey` 缺失 / 为空 → `Ok(None)`（视为未配置）
/// - JSON 非法 → `Err`（错误信息只含行 / 列，不含任何密钥内容）
fn parse_curseforge_key(text: &str) -> Result<Option<String>, String> {
    let file: SecretsFile =
        serde_json::from_str(text).map_err(|e| format!("解析 {SECRETS_FILE_NAME} 失败: {e}"))?;

    let key = file
        .curseforge
        .and_then(|section| section.api_key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    Ok(key)
}

/// 读取单个路径的密钥文件。
///
/// 文件不存在 / 不可读 / 解析失败 / 未配置 → `None`（静默跳过，绝不 panic）。
fn read_msa_config_from(path: &Path) -> Option<MsaConfig> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_msa_config(&text).ok().flatten()
}

/// 读取单个路径的 CurseForge API Key（语义与 [`read_msa_config_from`] 一致）。
fn read_curseforge_key_from(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_curseforge_key(&text).ok().flatten()
}

/// 按顺序取第一个命中的配置（缺失的路径自动跳过）。
fn first_msa_config<I: IntoIterator<Item = PathBuf>>(paths: I) -> Option<MsaConfig> {
    paths.into_iter().find_map(|p| read_msa_config_from(&p))
}

/// 按顺序取第一个命中的 CurseForge API Key（缺失的路径自动跳过）。
fn first_curseforge_key<I: IntoIterator<Item = PathBuf>>(paths: I) -> Option<String> {
    paths.into_iter().find_map(|p| read_curseforge_key_from(&p))
}

fn env_secrets_paths(primary: Option<&str>, legacy: Option<&str>) -> Vec<PathBuf> {
    [primary, legacy]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// 开发期回退路径：`<CARGO_MANIFEST_DIR>/../secrets.local.json`。
///
/// release 构建下该路径通常不存在，`read_msa_config_from` 会自然跳过。
fn dev_fallback_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(SECRETS_FILE_NAME)
}

fn candidate_paths(app: &AppHandle) -> Vec<PathBuf> {
    let primary = std::env::var("NICKEL_SECRETS").ok();
    let legacy = std::env::var("NOWHERE_SECRETS").ok();
    let mut paths = env_secrets_paths(primary.as_deref(), legacy.as_deref());
    if let Ok(directory) = crate::app_paths::config_dir(app) {
        paths.push(directory.join(SECRETS_FILE_NAME));
    }
    paths.push(dev_fallback_path());
    paths
}

/// 读取 Microsoft 登录配置。
///
/// 未配置（三处文件都不存在 / 都无有效 `clientId`）返回 `None`，前端据此显示
/// 「复制示例文件并填入 Client ID」的可执行指引；配置存在但不可解析同样返回
/// `None` —— 本命令**不会**向调用方抛出错误。
#[tauri::command]
pub fn msa_config_get(app: AppHandle) -> Result<Option<MsaConfig>, String> {
    Ok(first_msa_config(candidate_paths(&app)))
}

/// 读取 CurseForge API Key（`curseforge.apiKey`）。
///
/// 读取顺序与 [`msa_config_get`] 完全一致；未配置返回 `None`，
/// 前端据此给出「在 secrets.local.json 填入 curseforge.apiKey」的可执行指引。
/// **key 本身只在本进程内使用（作为 CurseForge 请求头），不写日志、不进错误信息。**
#[tauri::command]
pub fn curseforge_api_key(app: AppHandle) -> Result<Option<String>, String> {
    Ok(first_curseforge_key(candidate_paths(&app)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "microsoft": {
        "clientId": "11111111-2222-3333-4444-555555555555",
        "objectId": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        "tenantId": "ffffffff-0000-1111-2222-333333333333",
        "authority": "common",
        "redirectUri": "nowhere://auth"
      },
      "curseforge": { "apiKey": null }
    }"#;

    /// 生成唯一的临时文件路径。
    fn temp_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "nowhere_secrets_{}_{}_{}.json",
            std::process::id(),
            crate::store::random_token(),
            tag
        ));
        p
    }

    #[test]
    fn parses_config_and_ignores_unknown_fields() {
        let cfg = parse_msa_config(SAMPLE).unwrap().unwrap();
        assert_eq!(cfg.client_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(cfg.authority, "common");
        assert_eq!(cfg.redirect_uri, "nowhere://auth");
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

    #[test]
    fn secrets_aliases_prefer_primary_and_skip_blank_paths() {
        let primary = temp_path("primary");
        let legacy = temp_path("legacy");
        let primary_text = primary.to_string_lossy();
        let legacy_text = legacy.to_string_lossy();
        assert_eq!(
            env_secrets_paths(Some(&primary_text), Some(&legacy_text)),
            vec![primary.clone(), legacy.clone()]
        );
        assert_eq!(
            env_secrets_paths(Some(" "), Some(&legacy_text)),
            vec![legacy.clone()]
        );
        assert!(env_secrets_paths(None, Some(" ")).is_empty());
        std::fs::write(&primary, r#"{"microsoft":{"clientId":"primary-fixture"},"curseforge":{"apiKey":"primary-fixture"}}"#).unwrap();
        std::fs::write(&legacy, SAMPLE).unwrap();
        let paths = env_secrets_paths(Some(&primary_text), Some(&legacy_text));
        assert_eq!(
            first_msa_config(paths.clone()).unwrap().client_id,
            "primary-fixture"
        );
        assert_eq!(
            first_curseforge_key(paths.clone()).as_deref(),
            Some("primary-fixture")
        );
        std::fs::write(&primary, "broken").unwrap();
        assert_eq!(
            first_msa_config(paths).unwrap().client_id,
            "11111111-2222-3333-4444-555555555555"
        );
        std::fs::remove_file(primary).unwrap();
        std::fs::remove_file(legacy).unwrap();
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
            first_curseforge_key(vec![missing, present.clone()]),
            Some("$2a$10$TESTKEY".to_string())
        );

        std::fs::remove_file(&present).ok();
    }
}
