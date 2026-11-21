use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use minecraft_java_rs_core::utils::hash::{HashAlgorithm, get_file_hash};
use serde_json::Value;

fn normal_build_closure(metadata: &Value) -> HashSet<&str> {
    let graph = &metadata["resolve"];
    let nodes: HashMap<_, _> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| (node["id"].as_str().unwrap(), node))
        .collect();
    let mut pending = vec![graph["root"].as_str().unwrap()];
    let mut visited = HashSet::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        for dependency in nodes[id]["deps"].as_array().unwrap() {
            if dependency["dep_kinds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|kind| kind["kind"].is_null() || kind["kind"].as_str() == Some("build"))
            {
                // Keep the union of target-specific edges, but never follow dev edges.
                pending.push(dependency["pkg"].as_str().unwrap());
            }
        }
    }
    visited
}

fn field<'a>(section: &'a str, name: &str) -> &'a str {
    let prefix = format!("{name}: ");
    section
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("Missing notice field: {name}"))
}

fn hash_content<'a>(
    content: &'a str,
    scratch: &Path,
    cache: &mut HashMap<&'a str, String>,
) -> String {
    if let Some(hash) = cache.get(content) {
        return hash.clone();
    }
    std::fs::write(scratch, content.as_bytes()).unwrap();
    let hash = get_file_hash(scratch, HashAlgorithm::Sha256).unwrap();
    cache.insert(content, hash.clone());
    hash
}

struct HashScratch(PathBuf);

impl Drop for HashScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn closure_keeps_normal_and_build_targets_without_following_dev_edges() {
    let metadata = serde_json::json!({"resolve": {"root": "root", "nodes": [
        {"id": "root", "deps": [
            {"pkg": "runtime", "dep_kinds": [{"kind": null, "target": "cfg(windows)"}]},
            {"pkg": "builder", "dep_kinds": [{"kind": "build", "target": "cfg(unix)"}]},
            {"pkg": "dev", "dep_kinds": [{"kind": "dev", "target": null}]}
        ]},
        {"id": "runtime", "deps": [{"pkg": "shared", "dep_kinds": [{"kind": null}]}]},
        {"id": "builder", "deps": [{"pkg": "shared", "dep_kinds": [{"kind": null}]}]},
        {"id": "shared", "deps": []},
        {"id": "dev", "deps": [{"pkg": "test-only", "dep_kinds": [{"kind": null}]}]},
        {"id": "test-only", "deps": []}
    ]}});
    assert_eq!(
        normal_build_closure(&metadata),
        HashSet::from(["root", "runtime", "builder", "shared"])
    );
}

#[test]
fn locked_normal_build_registry_closure_has_original_distributed_notices() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.parent().unwrap();
    let lock = manifest.join("Cargo.lock");
    let lock_hash = get_file_hash(&lock, HashAlgorithm::Sha256).unwrap();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(manifest)
        .args(["metadata", "--locked", "--offline", "--format-version=1"])
        .output()
        .expect("Cargo metadata for the locked normal/build graph");
    assert!(
        output.status.success(),
        "Locked offline Cargo metadata failed"
    );
    assert_eq!(
        get_file_hash(&lock, HashAlgorithm::Sha256).unwrap(),
        lock_hash
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    let reachable = normal_build_closure(&metadata);
    let notices = std::fs::read_to_string(root.join("LICENSES/dependencies-NOTICES.txt")).unwrap();
    let separator = format!("\n{}\nPackage: ", "=".repeat(78));
    let mut sections = HashMap::new();
    for section in notices.split(&separator).skip(1) {
        let (header, body) = section.split_once('\n').unwrap();
        assert!(
            sections.insert(header, body).is_none(),
            "Duplicate notice: {header}"
        );
    }

    let mut registry_count = 0;
    let mut original_file_count = 0;
    for package in metadata["packages"].as_array().unwrap() {
        if !reachable.contains(package["id"].as_str().unwrap())
            || !package["source"]
                .as_str()
                .is_some_and(|source| source.starts_with("registry+"))
        {
            continue;
        }
        registry_count += 1;
        let name = package["name"].as_str().unwrap();
        let version = package["version"].as_str().unwrap();
        let header = format!("cargo / {name} / {version}");
        let section = sections
            .get(header.as_str())
            .unwrap_or_else(|| panic!("Missing notice: {header}"));
        assert_eq!(
            field(section, "Source URL"),
            format!("https://crates.io/crates/{name}/{version}")
        );
        assert_eq!(
            field(section, "License expression (manifest)"),
            package["license"].as_str().unwrap()
        );
        if let Some(repository) = package["repository"].as_str() {
            assert_eq!(field(section, "Repository URL"), repository, "{header}");
        }
        let source = Path::new(package["manifest_path"].as_str().unwrap())
            .parent()
            .unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !path.is_file()
                || !["license", "licence", "copying", "copyright", "notice"]
                    .iter()
                    .any(|part| name.contains(part))
            {
                continue;
            }
            original_file_count += 1;
            let original = std::fs::read_to_string(&path).unwrap();
            assert!(
                section.contains(original.as_str()),
                "Original license/notice missing: {header}/{}",
                path.file_name().unwrap().to_string_lossy()
            );
        }
    }
    assert!(registry_count > 0 && original_file_count > 0);

    let scratch = HashScratch(
        std::env::temp_dir().join(format!(
            "nickel-notice-hash-{}-{}.bin",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )),
    );
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&scratch.0)
        .unwrap();
    let mut hashes = HashMap::new();
    let mut original_count = 0;
    for section in sections.values() {
        let mut remaining = *section;
        while let Some((_, record)) = remaining.split_once("--- Original ") {
            let (_, record) = record.split_once(" ---\n").expect("Original source label");
            let (headers, record) = record.split_once("\n\n").expect("Original hash header");
            let expected = field(headers, "SHA-256 of original content");
            assert_eq!(expected.len(), 64);
            let (content, rest) = record
                .split_once("\n--- End original content ---")
                .expect("Original content boundary");
            // Old wrappers may add one LF around originals with no terminal LF.
            let candidates = [
                Some(content),
                content.strip_suffix('\n'),
                content.strip_suffix("\r\n"),
            ];
            assert!(
                candidates
                    .into_iter()
                    .flatten()
                    .any(|candidate| hash_content(candidate, &scratch.0, &mut hashes) == expected),
                "Original content hash mismatch: {expected}"
            );
            original_count += 1;
            remaining = rest;
        }
    }
    assert!(original_count >= original_file_count);

    let config: Value =
        serde_json::from_slice(&std::fs::read(manifest.join("tauri.conf.json")).unwrap()).unwrap();
    assert_eq!(
        config["bundle"]["resources"]["../LICENSES/"],
        "licenses/LICENSES/"
    );
    assert_eq!(
        std::fs::read(manifest.join("../LICENSES/dependencies-NOTICES.txt")).unwrap(),
        notices.as_bytes()
    );
    eprintln!(
        "[notices] {registry_count} normal/build registry packages; {original_file_count} cached license/notice files retained; {original_count} original hashes verified; bundle input matches"
    );
}
