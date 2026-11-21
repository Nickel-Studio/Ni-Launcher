#[allow(dead_code)]
mod build_config;

fn embed_api_config() {
    for key in [
        "NICKEL_BUILD_SECRETS",
        "NICKEL_MSA_CLIENT_ID",
        "NICKEL_MSA_AUTHORITY",
        "NICKEL_MSA_REDIRECT_URI",
        "NICKEL_CURSEFORGE_API_KEY",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    println!("cargo:rerun-if-changed=build_config.rs");
    let root = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"),
    );
    let explicit = std::env::var_os("NICKEL_BUILD_SECRETS").filter(|path| !path.is_empty());
    let path = explicit
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("../secrets.local.json"));
    println!("cargo:rerun-if-changed={}", path.display());
    let local = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if explicit.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
            "{}".to_owned()
        }
        Err(_) => panic!("Unable to read build API configuration"),
    };
    let projected = build_config::from_source(&local, |key| std::env::var(key).ok())
        .expect("Invalid build API configuration");
    if projected["curseforge"]["apiKey"].as_str() == Some(build_config::TEST_CURSEFORGE_KEY) {
        println!(
            "cargo:warning=CurseForge test placeholder is embedded but will not be used for authentication"
        );
    }
    let out =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("build output directory"));
    std::fs::write(
        out.join("nickel-embedded-secrets.json"),
        projected.to_string(),
    )
    .expect("Unable to write embedded API configuration");
}

fn validate_release_metadata() {
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let version = std::env::var("CARGO_PKG_VERSION").expect("package version");
    for relative in ["../package.json", "tauri.conf.json", "../CHANGELOG.json"] {
        let path = root.join(relative);
        println!("cargo:rerun-if-changed={}", path.display());
        let text = std::fs::read_to_string(path).expect("Unable to read release metadata");
        let value: serde_json::Value = serde_json::from_str(&text).expect("Invalid release metadata");
        assert_eq!(value["version"].as_str(), Some(version.as_str()), "Release metadata versions must match");
        if relative == "../CHANGELOG.json" {
            assert!(value["title"].as_str().is_some_and(|text| !text.trim().is_empty()), "Changelog title is required");
            let sections = value["sections"].as_array().expect("Changelog sections are required");
            assert!(!sections.is_empty(), "Changelog sections must not be empty");
            for section in sections {
                assert!(section["title"].as_str().is_some_and(|text| !text.trim().is_empty()), "Changelog section title is required");
                let items = section["items"].as_array().expect("Changelog items are required");
                assert!(!items.is_empty() && items.iter().all(|item| item.as_str().is_some_and(|text| !text.trim().is_empty())), "Changelog items must not be empty");
            }
        }
    }
}

fn main() {
    validate_release_metadata();
    embed_api_config();
    tauri_build::build();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // 只提供现有资源的搜索目录，cfg(test) 模块显式链接，避免 bin 资源被重复加入。
        if let Some(out) = std::env::var_os("OUT_DIR") {
            println!(
                "cargo:rustc-link-search=native={}",
                std::path::PathBuf::from(out).display()
            );
        }
    }
}
