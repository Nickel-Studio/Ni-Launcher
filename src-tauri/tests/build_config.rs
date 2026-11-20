#[path = "../build_config.rs"]
mod build_config;

use build_config::{MAX_CONFIG_BYTES, TEST_CURSEFORGE_KEY, from_source, project};
use serde_json::{Value, json};

#[test]
fn all_build_environment_fields_override_and_trim_source_fields() {
    let source = json!({
        "microsoft": {"clientId": "local-client", "authority": "local-authority", "redirectUri": "local://auth"},
        "curseforge": {"apiKey": "local-key"},
        "accounts": [{"refreshToken": "private-fixture"}]
    });
    let result = from_source(&source.to_string(), |key| match key {
        "NICKEL_MSA_CLIENT_ID" => Some("  env-client  ".into()),
        "NICKEL_MSA_AUTHORITY" => Some("  common  ".into()),
        "NICKEL_MSA_REDIRECT_URI" => Some("  fixture://auth  ".into()),
        "NICKEL_CURSEFORGE_API_KEY" => Some("  env-key  ".into()),
        _ => None,
    })
    .unwrap();
    assert_eq!(
        result,
        json!({
            "microsoft": {"clientId": "env-client", "authority": "common", "redirectUri": "fixture://auth"},
            "curseforge": {"apiKey": "env-key"}
        })
    );
}

#[test]
fn environment_client_id_completes_partial_source_without_losing_optional_fields() {
    let source = r#"{"microsoft":{"authority":"common","redirectUri":"fixture://auth"}}"#;
    let result = from_source(source, |key| {
        (key == "NICKEL_MSA_CLIENT_ID").then(|| "env-client".into())
    })
    .unwrap();
    assert_eq!(
        result["microsoft"],
        json!({
            "clientId": "env-client", "authority": "common", "redirectUri": "fixture://auth"
        })
    );
}

#[test]
fn environment_overrides_repair_non_object_documents_and_sections_without_panicking() {
    for source in [
        "null",
        "[]",
        "true",
        "42",
        r#""fixture""#,
        r#"{"microsoft":[],"curseforge":false}"#,
        r#"{"microsoft":"invalid","curseforge":42}"#,
    ] {
        let result = from_source(source, |key| match key {
            "NICKEL_MSA_CLIENT_ID" => Some("env-client".into()),
            "NICKEL_CURSEFORGE_API_KEY" => Some("env-key".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(result["microsoft"]["clientId"], "env-client");
        assert_eq!(result["curseforge"]["apiKey"], "env-key");
    }
}

#[test]
fn empty_build_environment_fields_do_not_erase_local_configuration() {
    let source = r#"{"microsoft":{"clientId":"local-client","authority":"common"},"curseforge":{"apiKey":"local-key"}}"#;
    let expected = project(source).unwrap();
    for blank in ["", " ", "\r\n\t"] {
        assert_eq!(
            from_source(source, |_| Some(blank.into())).unwrap(),
            expected
        );
    }
}

#[test]
fn projection_skips_bad_field_types_and_non_object_documents() {
    for source in ["null", "[]", "true", "42", r#""fixture""#] {
        assert_eq!(project(source).unwrap(), json!({}));
    }
    for invalid in [Value::Null, json!(true), json!(123), json!([]), json!({})] {
        let source = json!({
            "microsoft": {"clientId": invalid}, "curseforge": {"apiKey": invalid}
        });
        assert_eq!(project(&source.to_string()).unwrap(), json!({}));
    }
}

#[test]
fn invalid_optional_fields_use_defaults_without_discarding_a_valid_client() {
    for invalid in [
        Value::Null,
        json!(true),
        json!([]),
        json!("bad\u{0000}value"),
        json!("x".repeat(8193)),
    ] {
        let source = json!({"microsoft": {
            "clientId": "fixture-client", "authority": invalid, "redirectUri": invalid
        }});
        assert_eq!(
            project(&source.to_string()).unwrap()["microsoft"],
            json!({
                "clientId": "fixture-client", "authority": "consumers", "redirectUri": "nickel://auth"
            })
        );
    }
}

#[test]
fn required_fields_reject_control_characters_and_excessive_lengths() {
    for invalid in [
        "bad\u{0000}value".into(),
        "bad\nvalue".into(),
        "x".repeat(8193),
    ] {
        let source = json!({"microsoft":{"clientId":invalid},"curseforge":{"apiKey":invalid}});
        assert_eq!(project(&source.to_string()).unwrap(), json!({}));
    }
    let boundary = "x".repeat(8192);
    let source = json!({"microsoft":{"clientId":boundary},"curseforge":{"apiKey":boundary}});
    let projected = project(&source.to_string()).unwrap();
    assert!(projected["microsoft"]["clientId"].as_str().is_some());
    assert!(projected["curseforge"]["apiKey"].as_str().is_some());
}

#[test]
fn invalid_nonblank_environment_credentials_are_rejected_after_merging() {
    let source = r#"{"microsoft":{"clientId":"local-client"},"curseforge":{"apiKey":"local-key"}}"#;
    for invalid in ["bad\nvalue".into(), "x".repeat(8193)] {
        let projected = from_source(source, |key| match key {
            "NICKEL_MSA_CLIENT_ID" | "NICKEL_CURSEFORGE_API_KEY" => Some(invalid.clone()),
            _ => None,
        })
        .unwrap();
        assert_eq!(projected, json!({}));
    }
}

#[test]
fn document_limits_apply_before_parsing_or_environment_overrides() {
    let boundary = format!("{{}}{}", " ".repeat(MAX_CONFIG_BYTES - 2));
    assert_eq!(project(&boundary).unwrap(), json!({}));
    let oversized = format!("{boundary} ");
    assert!(project(&oversized).is_err());
    assert!(from_source(&oversized, |_| Some("fixture".into())).is_err());
    let error = from_source(r#"{"apiKey":"private-error-fixture",broken}"#, |_| {
        Some("fixture".into())
    })
    .unwrap_err();
    assert!(!error.contains("private-error-fixture"));
    assert!(!error.contains("apiKey"));
}

#[test]
fn build_lookup_and_projection_are_limited_to_the_four_authorized_fields() {
    let looked_up = std::cell::RefCell::new(Vec::new());
    let source = r#"{"microsoft":{"clientSecret":"private-fixture","tenantId":"private-fixture"},"tokens":["private-fixture"]}"#;
    let projected = from_source(source, |key| {
        looked_up.borrow_mut().push(key.to_owned());
        Some("fixture".into())
    })
    .unwrap();
    assert_eq!(
        *looked_up.borrow(),
        [
            "NICKEL_MSA_CLIENT_ID",
            "NICKEL_MSA_AUTHORITY",
            "NICKEL_MSA_REDIRECT_URI",
            "NICKEL_CURSEFORGE_API_KEY"
        ]
    );
    assert_eq!(projected.as_object().unwrap().len(), 2);
    assert_eq!(projected["microsoft"].as_object().unwrap().len(), 3);
    assert_eq!(projected["curseforge"].as_object().unwrap().len(), 1);
    assert!(!projected.to_string().contains("private-fixture"));
}

#[test]
fn exact_curseforge_test_placeholder_is_preserved_only_as_marked_build_test_data() {
    let projected = from_source("{}", |key| {
        (key == "NICKEL_CURSEFORGE_API_KEY").then(|| format!("  {TEST_CURSEFORGE_KEY}  "))
    })
    .unwrap();
    assert_eq!(projected["curseforge"]["apiKey"], TEST_CURSEFORGE_KEY);
}

#[test]
#[ignore = "Reads only the authorized build source and emits boolean evidence; run with the embedded-config verifier"]
fn authorized_build_source_matches_embedded_whitelist_without_logging_values() {
    let explicit = std::env::var_os("NICKEL_BUILD_SECRETS").filter(|path| !path.is_empty());
    let path = explicit.map(std::path::PathBuf::from).unwrap_or_else(|| {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../secrets.local.json")
    });
    let text = std::fs::read_to_string(path).expect("authorized build source is readable");
    let expected = from_source(&text, |key| std::env::var(key).ok())
        .expect("authorized build source passes field validation");
    let embedded_text = include_str!(concat!(env!("OUT_DIR"), "/nickel-embedded-secrets.json"));
    let embedded: Value = serde_json::from_str(embedded_text).expect("embedded JSON is valid");
    assert!(
        expected == embedded,
        "embedded whitelist differs from authorized build source"
    );
    assert!(
        embedded["microsoft"]["clientId"].as_str().is_some(),
        "microsoft.clientId is required for this acceptance"
    );
    println!(
        "NICKEL_BUILD_PROBE={}",
        json!({
            "authorizedSourceMatchesEmbedded": true,
            "microsoft.clientId": embedded["microsoft"]["clientId"].as_str().is_some(),
            "microsoft.authority": embedded["microsoft"]["authority"].as_str().is_some(),
            "microsoft.redirectUri": embedded["microsoft"]["redirectUri"].as_str().is_some(),
            "curseforge.apiKey": embedded["curseforge"]["apiKey"].as_str().is_some(),
            "curseforge.testPlaceholder": embedded["curseforge"]["apiKey"].as_str() == Some(TEST_CURSEFORGE_KEY)
        })
    );
}
