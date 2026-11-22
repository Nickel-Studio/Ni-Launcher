//! Distribution-only portable marker. It never triggers a modpack import.
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const MARKER_FILE: &str = "nickel.launcher-portable.json";
pub const FORMAT: &str = "nickel-launcher-portable";
pub const SCHEMA_VERSION: u32 = 1;
const MAX_BYTES: u64 = 64 * 1024;
const INVALID_MARKER: &str = "Invalid launcher portable marker; extract the complete portable archive again";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Marker {
    pub schema_version: u32,
    pub format: String,
    pub version: String,
    pub target: String,
    pub launcher: String,
    pub data_directory: String,
    pub game_directory: String,
}

fn validate(marker: &Marker) -> Result<(), String> {
    let launcher = match marker.target.as_str() {
        "x86_64-pc-windows-msvc" => "Nickel.exe",
        "aarch64-apple-darwin" | "x86_64-apple-darwin" => "Nickel.app",
        "x86_64-unknown-linux-gnu" => "Nickel.AppImage",
        _ => return Err(INVALID_MARKER.into()),
    };
    if marker.schema_version != SCHEMA_VERSION
        || marker.format != FORMAT
        || marker.launcher != launcher
        || marker.data_directory != crate::app_paths::DATA_DIRECTORY
        || marker.game_directory != crate::app_paths::GAME_DIRECTORY
        || marker.version.is_empty()
        || marker.version.len() > 128
        || !marker.version.bytes().all(|byte| byte.is_ascii_alphanumeric() || b".-+".contains(&byte))
    {
        return Err(INVALID_MARKER.into());
    }
    Ok(())
}

pub fn read_marker(root: &Path) -> Result<Option<Marker>, String> {
    let path = root.join(MARKER_FILE);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(INVALID_MARKER.into()),
        Ok(_) => {}
    }
    crate::instances::reject_links_local(&path).map_err(|_| INVALID_MARKER.to_string())?;
    // Two different portable formats must not silently compete for the data root.
    if std::fs::symlink_metadata(root.join(crate::portable_pack::MANIFEST_FILE)).is_ok() {
        return Err("Conflicting launcher and modpack portable markers".into());
    }
    let file = std::fs::File::open(&path).map_err(|_| INVALID_MARKER.to_string())?;
    let metadata = file.metadata().map_err(|_| INVALID_MARKER.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_BYTES {
        return Err(INVALID_MARKER.into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|_| INVALID_MARKER.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(INVALID_MARKER.into());
    }
    let marker = serde_json::from_slice(&bytes).map_err(|_| INVALID_MARKER.to_string())?;
    validate(&marker)?;
    Ok(Some(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn fixture_marker() -> Marker {
        Marker {
            schema_version: SCHEMA_VERSION,
            format: FORMAT.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            target: "x86_64-pc-windows-msvc".into(),
            launcher: "Nickel.exe".into(),
            data_directory: crate::app_paths::DATA_DIRECTORY.into(),
            game_directory: crate::app_paths::GAME_DIRECTORY.into(),
        }
    }

    #[test]
    fn distribution_marker_is_distinct_and_accepts_only_the_four_release_targets() {
        assert_ne!(MARKER_FILE, crate::portable_pack::MANIFEST_FILE);
        assert_ne!(FORMAT, crate::portable_pack::FORMAT);
        let mut marker = fixture_marker();
        for (target, launcher) in [
            ("x86_64-pc-windows-msvc", "Nickel.exe"),
            ("aarch64-apple-darwin", "Nickel.app"),
            ("x86_64-apple-darwin", "Nickel.app"),
            ("x86_64-unknown-linux-gnu", "Nickel.AppImage"),
        ] {
            marker.target = target.into();
            marker.launcher = launcher.into();
            assert!(validate(&marker).is_ok());
        }
        marker.target = "unknown-target".into();
        assert!(validate(&marker).is_err());
    }

    #[test]
    fn marker_rejects_traversal_unknown_fields_and_modpack_payloads() {
        let marker = fixture_marker();
        for field in ["dataDirectory", "gameDirectory", "launcher", "format", "version"] {
            let mut value = serde_json::to_value(&marker).unwrap();
            value[field] = "../private".into();
            let changed: Marker = serde_json::from_value(value).unwrap();
            assert!(validate(&changed).is_err());
        }
        let mut value = serde_json::to_value(marker).unwrap();
        value["payload"] = "Nickel/pack.mrpack".into();
        assert!(serde_json::from_value::<Marker>(value).is_err());
    }

    #[test]
    fn marker_selection_does_not_import_or_copy_existing_configuration() {
        let root = std::env::temp_dir().join(format!("nickel-launcher-marker-{}", crate::store::random_token()));
        std::fs::create_dir(&root).unwrap();
        assert!(read_marker(&root).unwrap().is_none());
        let marker = fixture_marker();
        std::fs::write(root.join(MARKER_FILE), serde_json::to_vec(&marker).unwrap()).unwrap();
        assert_eq!(read_marker(&root).unwrap(), Some(marker));
        assert!(crate::portable_pack::read_manifest(&root).unwrap().is_none());
        assert!(!root.join(crate::app_paths::DATA_DIRECTORY).exists());
        std::fs::write(root.join(crate::portable_pack::MANIFEST_FILE), "{}").unwrap();
        assert!(read_marker(&root).is_err());
        std::fs::remove_file(root.join(crate::portable_pack::MANIFEST_FILE)).unwrap();
        std::fs::write(root.join(MARKER_FILE), b"private-malformed-fixture").unwrap();
        assert!(!read_marker(&root).unwrap_err().contains("private-malformed-fixture"));
        let file = std::fs::OpenOptions::new().write(true).open(root.join(MARKER_FILE)).unwrap();
        file.set_len(MAX_BYTES + 1).unwrap();
        assert!(read_marker(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
