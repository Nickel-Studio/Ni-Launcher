use serde_json::{Map, Value};

pub const MAX_CONFIG_BYTES: usize = 256 * 1024;
pub const TEST_CURSEFORGE_KEY: &str = "NICKEL_TEST_CURSEFORGE_KEY";

/// Own parsed configuration so private, non-whitelisted values are also cleared on drop.
pub struct SensitiveDocument(pub Value);

impl std::ops::Deref for SensitiveDocument {
    type Target = Value;
    fn deref(&self) -> &Value { &self.0 }
}

impl Drop for SensitiveDocument {
    fn drop(&mut self) { clear(&mut self.0); }
}

pub fn clear(value: &mut Value) {
    use zeroize::Zeroize;
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(values) => values.iter_mut().for_each(clear),
        Value::Object(values) => values.values_mut().for_each(clear),
        _ => {}
    }
}

fn field(section: &Value, name: &str) -> Option<String> {
    section
        .get(name)?
        .as_str()
        .map(str::trim)
        .filter(|value| {
            !value.is_empty() && value.len() <= 8192 && !value.chars().any(char::is_control)
        })
        .map(str::to_owned)
}

fn parse_document(text: &str) -> Result<Value, String> {
    if text.len() > MAX_CONFIG_BYTES {
        return Err("API configuration exceeds the build size limit".into());
    }
    serde_json::from_str(text).map_err(|error| {
        format!(
            "Invalid API configuration at line {}, column {}",
            error.line(),
            error.column()
        )
    })
}

fn project_value(value: &Value) -> Value {
    let mut result = Map::new();
    let mut microsoft = Map::new();
    if let Some(client) = field(&value["microsoft"], "clientId")
        .filter(|client| client != "00000000-0000-0000-0000-000000000000")
    {
        microsoft.insert("clientId".into(), Value::String(client));
        for (key, fallback) in [
            ("authority", "consumers"),
            ("redirectUri", "nickel://auth"),
        ] {
            microsoft.insert(
                key.into(),
                Value::String(field(&value["microsoft"], key).unwrap_or_else(|| fallback.into())),
            );
        }
        result.insert("microsoft".into(), Value::Object(microsoft));
    }
    if let Some(key) = field(&value["curseforge"], "apiKey") {
        result.insert("curseforge".into(), serde_json::json!({ "apiKey": key }));
    }
    let mut skin = Map::new();
    for key in ["clientId", "clientSecret", "redirectUri"] {
        if let Some(value) = field(&value["littleskin"], key) { skin.insert(key.into(), Value::String(value)); }
    }
    if !skin.is_empty() { result.insert("littleskin".into(), Value::Object(skin)); }
    Value::Object(result)
}

pub fn project(text: &str) -> Result<Value, String> {
    let source = SensitiveDocument(parse_document(text)?);
    Ok(project_value(&source))
}

pub fn from_source(text: &str, lookup: impl Fn(&str) -> Option<String>) -> Result<Value, String> {
    // Merge before projecting so an environment client ID can complete a partial source section.
    let source = SensitiveDocument(with_environment(parse_document(text)?, lookup));
    Ok(project_value(&source))
}

pub fn with_environment(mut value: Value, lookup: impl Fn(&str) -> Option<String>) -> Value {
    if !value.is_object() {
        value = Value::Object(Map::new());
    }
    for (env_key, section, field_name) in [
        ("NICKEL_MSA_CLIENT_ID", "microsoft", "clientId"),
        ("NICKEL_MSA_AUTHORITY", "microsoft", "authority"),
        ("NICKEL_MSA_REDIRECT_URI", "microsoft", "redirectUri"),
        ("NICKEL_CURSEFORGE_API_KEY", "curseforge", "apiKey"),
        ("NICKEL_LITTLESKIN_CLIENT_ID", "littleskin", "clientId"),
        ("NICKEL_LITTLESKIN_CLIENT_SECRET", "littleskin", "clientSecret"),
        ("NICKEL_LITTLESKIN_REDIRECT_URI", "littleskin", "redirectUri"),
    ] {
        if let Some(text) = lookup(env_key).filter(|text| !text.trim().is_empty()) {
            if !value[section].is_object() {
                value[section] = Value::Object(Map::new());
            }
            value[section][field_name] = Value::String(text);
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_authorized_public_build_fields_are_embedded() {
        let input = serde_json::json!({
            "$comment": "do not embed", "accounts": ["private"],
            "microsoft": {"clientId": "fixture-client", "authority": "common", "redirectUri": "nickel://auth",
                "objectId": "private-object", "tenantId": "private-tenant", "clientSecret": "private-secret", "refreshToken": "private-token"},
            "curseforge": {"apiKey": "fixture-key", "password": "private-password"},
            "littleskin": {"clientId": "fixture-skin", "clientSecret": "fixture-skin-secret",
                "redirectUri": "nickel://skin", "password": "private-password"}
        });
        let output = project(&input.to_string()).unwrap();
        assert_eq!(
            output,
            serde_json::json!({
                "microsoft": {"clientId":"fixture-client", "authority":"common", "redirectUri":"nickel://auth"},
                "curseforge":{"apiKey":"fixture-key"},
                "littleskin": {"clientId":"fixture-skin", "clientSecret":"fixture-skin-secret", "redirectUri":"nickel://skin"}
            })
        );
        assert!(!output.to_string().contains("private"));
    }

    #[test]
    fn absent_example_and_broken_configuration_cannot_become_credentials() {
        assert_eq!(project("{}").unwrap(), serde_json::json!({}));
        assert_eq!(
            project(r#"{"microsoft":{"clientId":"00000000-0000-0000-0000-000000000000"}}"#)
                .unwrap(),
            serde_json::json!({})
        );
        let error = project(r#"{"apiKey":"private-secret", broken}"#).unwrap_err();
        assert!(!error.contains("private-secret"));
    }

    #[test]
    fn ci_fields_override_local_configuration_without_copying_unknown_fields() {
        let combined = with_environment(
            serde_json::json!({"microsoft":{"clientId":"local"}}),
            |key| match key {
                "NICKEL_MSA_CLIENT_ID" => Some("ci-client".into()),
                "NICKEL_CURSEFORGE_API_KEY" => Some(TEST_CURSEFORGE_KEY.into()),
                _ => None,
            },
        );
        let output = project(&combined.to_string()).unwrap();
        assert_eq!(output["microsoft"]["clientId"], "ci-client");
        assert_eq!(output["curseforge"]["apiKey"], TEST_CURSEFORGE_KEY);
        assert_eq!(output["microsoft"].as_object().unwrap().len(), 3);
    }
}
