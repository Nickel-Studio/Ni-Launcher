//! 便携包实例设置的安全投影：只迁移可复用的启动偏好，不携带本机路径或凭据。
//!
//! 导出采用字段白名单；单个字段含绝对路径或凭据标记时整字段跳过。
//! 导入再次严格校验未知键，并在写回时保留目标实例已有的未知键。

use std::path::Path;

use serde_json::{Map, Value};

use crate::{instances, named_profiles};

pub const SETTINGS_FILE_NAME: &str = "nickel.settings.json";
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_SERVER_TEXT_BYTES: usize = 4096;

/// 固定顺序同时作为预览中的中文说明来源。
const ALLOWED_FIELDS: &[(&str, &str)] = &[
    ("windowTitle", "窗口标题"),
    ("customInfo", "自定义信息"),
    ("versionIsolation", "版本隔离"),
    ("launcherVisibility", "启动器可见性"),
    ("windowMode", "窗口大小"),
    ("windowWidth", "窗口宽度"),
    ("windowHeight", "窗口高度"),
    ("memoryMode", "内存模式"),
    ("memoryMb", "内存上限"),
    ("memoryManagement", "内存管理"),
    ("highPerformanceGpu", "高性能 GPU"),
    ("processPriority", "进程优先级"),
    ("serverLogin", "服务器登录方式"),
    ("serverName", "服务器名称"),
    ("serverRegistration", "服务器注册地址"),
    ("authServer", "认证服务器"),
    ("serverAddress", "自动进入服务器"),
    ("jvmArgs", "JVM 参数"),
    ("gameArgs", "游戏参数"),
    ("disableModUpdates", "禁止 Mod 更新"),
    ("noModUpdates", "禁止 Mod 更新（旧字段）"),
    ("disableFileCheck", "跳过文件检查"),
    ("disableWrapper", "禁用 Java 包装器"),
    ("disableLwjglAgent", "禁用 LWJGL Agent"),
];

const CREDENTIAL_MARKERS: &[&str] = &[
    "accesstoken",
    "clienttoken",
    "refreshtoken",
    "clientsecret",
    "clientcredential",
    "sessiontoken",
    "bearertoken",
    "apikey",
    "password",
    "authorization",
    "privatekey",
];
const MEMORY_MANAGEMENT_VALUES: &[&str] = &[
    "auto-zgc",
    "generational-zgc",
    "g1",
    "g1-tuned",
    "custom",
    "serial",
    "parallel",
];
const VISIBILITY_VALUES: &[&str] = &["keep", "hide", "hide-close", "close", "minimize"];
const WINDOW_MODE_VALUES: &[&str] = &[
    "default",
    "custom",
    "fullscreen",
    "same-as-launcher",
    "maximize",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Projection {
    pub settings: Value,
    pub included: Vec<String>,
    pub omitted: Vec<String>,
}

fn field_allowed(key: &str) -> bool {
    ALLOWED_FIELDS.iter().any(|(field, _)| *field == key)
}

fn contains_host_path(value: &str) -> bool {
    if super::has_host_path(value, &[]) {
        return true;
    }
    // `-javaagent:C:/...` 会被 URL 解析成自定义 scheme；这里按 token 再检查一次。
    let normalized = value.replace('\\', "/");
    let mut search = 0;
    while let Some(relative) = normalized[search..].find(":/") {
        let index = search + relative;
        let start = normalized[..index]
            .rfind(|ch: char| {
                ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | '=' | '(' | '[')
            })
            .map_or(0, |position| position + 1);
        let scheme = normalized[start..index]
            .rsplit('=')
            .next()
            .unwrap_or("")
            .trim_matches('"');
        if !["http", "https", "ftp", "ws", "wss"]
            .iter()
            .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
        {
            return true;
        }
        search = index + 2;
    }
    false
}

fn safe_text(value: &str) -> bool {
    if value.len() > MAX_TEXT_BYTES
        || value
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    {
        return false;
    }
    if contains_host_path(value) {
        return false;
    }
    let folded: String = value
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| !matches!(ch, '_' | '-' | ' '))
        .collect();
    !CREDENTIAL_MARKERS.iter().any(|marker| folded.contains(marker))
}

fn valid_choice(value: &Value, choices: &[&str]) -> bool {
    value.as_str().is_some_and(|value| choices.contains(&value))
}

fn valid_text(value: &Value, limit: usize) -> bool {
    value
        .as_str()
        .is_some_and(|value| value.len() <= limit && safe_text(value))
}

fn valid_field(key: &str, value: &Value) -> bool {
    if value.is_null() {
        return true;
    }
    match key {
        "versionIsolation" | "highPerformanceGpu"
        | "disableModUpdates" | "noModUpdates" | "disableFileCheck"
        | "disableWrapper" | "disableLwjglAgent" => value.is_boolean(),
        "windowTitle" | "customInfo" | "jvmArgs" | "gameArgs" => valid_text(value, MAX_TEXT_BYTES),
        "launcherVisibility" => valid_choice(value, VISIBILITY_VALUES),
        "windowMode" => valid_choice(value, WINDOW_MODE_VALUES),
        "memoryMode" => valid_choice(value, &["global", "auto", "custom"]),
        "memoryMb" => value
            .as_u64()
            .is_some_and(|value| (512..=65_536).contains(&value)),
        "memoryManagement" => valid_choice(value, MEMORY_MANAGEMENT_VALUES),
        "processPriority" => valid_choice(value, &["low", "normal", "high"]),
        "serverLogin" => valid_choice(value, &["any", "offline", "microsoft", "thirdParty"]),
        "serverName" => value.as_str().is_some_and(|value| {
            value.len() <= 256 && safe_text(value)
        }),
        "serverRegistration" | "authServer" | "serverAddress" => {
            valid_text(value, MAX_SERVER_TEXT_BYTES)
        }
        "windowWidth" => value
            .as_u64()
            .is_some_and(|value| (320..=16_384).contains(&value)),
        "windowHeight" => value
            .as_u64()
            .is_some_and(|value| (240..=16_384).contains(&value)),
        _ => false,
    }
}

/// 从实例设置对象挑选可迁移字段；不安全或类型不符的值只跳过对应字段。
pub fn project(value: &Value) -> Projection {
    let Some(source) = value.as_object() else {
        return Projection::default();
    };
    let mut settings = Map::new();
    let mut included = Vec::new();
    let mut omitted = Vec::new();
    for (key, label) in ALLOWED_FIELDS {
        let value = source.get(*key).filter(|value| !value.is_null());
        let Some(value) = value else {
            continue;
        };
        if valid_field(key, value) {
            settings.insert((*key).into(), value.clone());
            included.push((*label).into());
        } else {
            omitted.push((*label).into());
        }
    }
    Projection {
        settings: Value::Object(settings),
        included,
        omitted,
    }
}

pub fn serialize(settings: &Value) -> Result<Vec<u8>, String> {
    let settings = validate(settings)?;
    serde_json::to_vec_pretty(&settings).map_err(|e| format!("整理便携实例设置失败：{e}"))
}

pub fn read_file(path: &Path) -> Result<Option<Value>, String> {
    instances::reject_links(path)?;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("读取便携实例设置失败：{error}")),
    };
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err("便携实例设置超过大小限制或不是普通文件".into());
    }
    let bytes = std::fs::read(path).map_err(|error| format!("读取便携实例设置失败：{error}"))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("便携实例设置格式无效：{error}"))?;
    validate(&value).map(Some)
}

/// 严格校验来自便携包的设置：未知键和不安全值都拒绝，避免把篡改内容写入实例。
pub fn validate(value: &Value) -> Result<Value, String> {
    let Some(source) = value.as_object() else {
        return Err("便携实例设置必须是 JSON 对象".into());
    };
    let mut checked = Map::new();
    for (key, value) in source {
        if !field_allowed(key) {
            return Err(format!("便携实例设置包含不允许的字段：{key}"));
        }
        if !valid_field(key, value) {
            return Err(format!("便携实例设置字段不安全或格式无效：{key}"));
        }
        checked.insert(key.clone(), value.clone());
    }
    Ok(Value::Object(checked))
}

fn projected_settings(root: &Path, version_id: &str) -> Result<Value, String> {
    named_profiles::validate_name(version_id)?;
    let directory = root.join("versions").join(version_id);
    instances::reject_links(&directory)?;
    let path = directory.join(SETTINGS_FILE_NAME);
    instances::reject_links(&path)?;
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Value::Object(Map::new()));
        }
        Err(error) => return Err(format!("读取实例设置失败：{error}")),
    };
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err("实例设置超过大小限制或不是普通文件".into());
    }
    let bytes = std::fs::read(&path).map_err(|error| format!("读取实例设置失败：{error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("实例设置格式无效：{error}"))
}

pub fn project_instance(root: &Path, version_id: &str) -> Result<Projection, String> {
    let project = project(&projected_settings(root, version_id)?);
    if project.settings.as_object().is_none_or(Map::is_empty) && project.omitted.is_empty() {
        return Ok(Projection::default());
    }
    Ok(project)
}

/// 合并目标实例现有 JSON，未知键保持原样；已知键由便携投影覆盖。
pub fn merge(existing: Value, incoming: &Value) -> Result<Value, String> {
    let incoming = validate(incoming)?;
    let mut output = match existing {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        _ => return Err("目标实例设置不是 JSON 对象".into()),
    };
    if let Some(settings) = incoming.as_object() {
        for (key, value) in settings {
            output.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(output))
}

pub fn apply_to_instance(instance_directory: &Path, settings: &Value) -> Result<(), String> {
    instances::reject_links(instance_directory)?;
    if !instance_directory.is_dir() {
        return Err("实例目录已不存在，无法导入设置".into());
    }
    let path = instance_directory.join(SETTINGS_FILE_NAME);
    let existing = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
                return Err("目标实例设置超过大小限制或不是普通文件".into());
            }
            let bytes = std::fs::read(&path)
                .map_err(|error| format!("读取目标实例设置失败：{error}"))?;
            serde_json::from_slice(&bytes)
                .map_err(|error| format!("目标实例设置已损坏，未写入便携设置：{error}"))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(error) => return Err(format!("读取目标实例设置失败：{error}")),
    };
    let merged = merge(existing, settings)?;
    let bytes = serde_json::to_vec_pretty(&merged)
        .map_err(|error| format!("整理目标实例设置失败：{error}"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("合并后的实例设置超过大小限制".into());
    }
    named_profiles::atomic_write(&path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn validated_roundtrip(value: &Value) -> Value {
        let validated = validate(value).unwrap();
        let serialized: Value = serde_json::from_slice(&serialize(value).unwrap()).unwrap();
        assert_eq!(serialized, validated);
        validated
    }

    #[test]
    fn projection_keeps_safe_launch_fields_and_drops_host_data() {
        let source = json!({
            "windowTitle": "整合包 · 生存",
            "customInfo": "服务器信息",
            "versionIsolation": true,
            "launcherVisibility": "hide",
            "windowMode": "custom",
            "windowWidth": 1280,
            "windowHeight": 720,
            "memoryMode": "custom",
            "memoryMb": 6144,
            "memoryManagement": "g1-tuned",
            "serverLogin": "thirdParty",
            "serverName": "LittleSkin",
            "serverRegistration": "https://example.com/register",
            "authServer": "https://example.com/api/yggdrasil",
            "serverAddress": "mc.example.com:25565",
            "jvmArgs": "-XX:+UseG1GC",
            "gameArgs": "--demo",
            "disableModUpdates": false,
            "javaPath": "C:/Users/private/java.exe",
            "preLaunchCommand": "echo private",
            "offlineSkinPath": "C:/Users/private/skin.png",
            "accessToken": "secret",
        });
        let project = project(&source);
        let settings = project.settings.as_object().unwrap();
        assert_eq!(settings["memoryMode"], "custom");
        assert_eq!(settings["memoryMb"], 6144);
        assert_eq!(settings["launcherVisibility"], "hide");
        assert_eq!(settings["serverAddress"], "mc.example.com:25565");
        assert_eq!(settings["authServer"], "https://example.com/api/yggdrasil");
        assert_eq!(settings["jvmArgs"], "-XX:+UseG1GC");
        assert!(validate(&json!({"jvmArgs": "-Durl=https://example.com/remote"})).is_ok());
        assert!(!settings.contains_key("javaPath"));
        assert!(!settings.contains_key("preLaunchCommand"));
        assert!(!settings.contains_key("offlineSkinPath"));
        assert!(!settings.contains_key("accessToken"));
        assert!(project.included.iter().any(|label| label == "内存上限"));
    }

    #[test]
    fn unsafe_allowed_values_are_omitted_or_rejected() {
        let project = project(&json!({
            "jvmArgs": "-javaagent:C:/Users/private/agent.jar",
            "gameArgs": "cache=/tmp/private",
            "windowTitle": "C:/Users/private/title",
            "serverName": "access_token=secret",
            "memoryMb": 1024,
            "windowWidth": 1280,
        }));
        let settings = project.settings.as_object().unwrap();
        assert!(!settings.contains_key("jvmArgs"));
        assert!(!settings.contains_key("gameArgs"));
        assert!(!settings.contains_key("windowTitle"));
        assert!(!settings.contains_key("serverName"));
        assert_eq!(settings["memoryMb"], 1024);
        assert!(validate(&json!({"unknown": true})).is_err());
        assert!(validate(&json!({"memoryMb": 1})).is_err());
        assert!(validate(&json!({"jvmArgs": "C:/private/java"})).is_err());
    }

    #[test]
    fn portable_visibility_window_and_memory_enums_roundtrip() {
        for visibility in VISIBILITY_VALUES {
            for window_mode in WINDOW_MODE_VALUES {
                let source = json!({
                    "launcherVisibility": visibility,
                    "windowMode": window_mode,
                });
                let projected = project(&source);
                assert_eq!(projected.settings["launcherVisibility"], *visibility);
                assert_eq!(projected.settings["windowMode"], *window_mode);
                let validated = validate(&projected.settings).unwrap();
                assert_eq!(validated, projected.settings);
            }
        }
        for management in MEMORY_MANAGEMENT_VALUES {
            let source = json!({ "memoryManagement": management });
            let projected = project(&source);
            assert_eq!(projected.settings["memoryManagement"], *management);
            validated_roundtrip(&projected.settings);
        }
    }

    #[test]
    fn invalid_portable_enums_are_rejected() {
        for value in ["default", "G1", "g1-tuned-extra", ""] {
            assert!(
                validate(&json!({ "memoryManagement": value })).is_err(),
                "{value}"
            );
        }
        for value in ["hide-all", "close-now", ""] {
            assert!(
                validate(&json!({ "launcherVisibility": value })).is_err(),
                "{value}"
            );
        }
        for value in ["maximized", "windowed", ""] {
            assert!(
                validate(&json!({ "windowMode": value })).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn client_secret_variants_are_omitted_and_rejected() {
        for secret in [
            "clientSecret=value",
            "client_secret=value",
            "client-secret=value",
            "clientsecretvalue",
            "clientCredential=value",
            "sessionToken=value",
        ] {
            let source = json!({ "customInfo": secret, "serverName": secret });
            let projected = project(&source);
            assert!(
                !projected.settings.as_object().unwrap().contains_key("customInfo"),
                "{secret}"
            );
            assert!(
                !projected.settings.as_object().unwrap().contains_key("serverName"),
                "{secret}"
            );
            assert!(
                validate(&json!({ "customInfo": secret })).is_err(),
                "{secret}"
            );
        }
    }

    #[test]
    fn import_merge_preserves_unknown_keys_and_clears_with_null() {
        let existing = json!({"unknownFutureKey": {"keep": true}, "windowTitle": "旧标题", "memoryMb": 2048});
        let incoming = json!({"windowTitle": null, "memoryMb": 8192, "gameArgs": "--demo"});
        let merged = merge(existing, &incoming).unwrap();
        assert_eq!(merged["unknownFutureKey"], json!({"keep": true}));
        assert!(merged["windowTitle"].is_null());
        assert_eq!(merged["memoryMb"], 8192);
        assert_eq!(merged["gameArgs"], "--demo");
    }

    #[test]
    fn apply_to_instance_preserves_unknown_and_never_overwrites_corruption() {
        let root = std::env::temp_dir().join(format!("nickel-instance-settings-{}", crate::store::random_token()));
        let instance = root.join("versions/example");
        std::fs::create_dir_all(&instance).unwrap();
        let path = instance.join(SETTINGS_FILE_NAME);
        std::fs::write(&path, br#"{"unknownFutureKey":"keep","windowTitle":"old"}"#).unwrap();
        apply_to_instance(&instance, &json!({"windowTitle": "new", "memoryMb": 4096})).unwrap();
        let stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored["unknownFutureKey"], "keep");
        assert_eq!(stored["windowTitle"], "new");
        assert_eq!(stored["memoryMb"], 4096);

        std::fs::write(&path, b"{broken").unwrap();
        assert!(apply_to_instance(&instance, &json!({"windowTitle": "lost"})).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
        let _ = std::fs::remove_dir_all(root);
    }
}
