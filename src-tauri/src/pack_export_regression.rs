use super::*;
use minecraft_java_rs_core::utils::archive::{extract_zip_subtrees, read_zip_entry};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let parent = std::env::var_os("NICKEL_EXPORT_REGRESSION_OUTPUT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = parent.join(format!(
            "nickel-export-regression-{}",
            crate::store::random_token()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn put(&self, relative: &str, bytes: impl AsRef<[u8]>) {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn instance(&self) -> ExportInstance {
        let cancel = CancellationToken::new();
        let mut files = Vec::new();
        for name in CONTENT_DIRECTORIES.iter().chain(CONTENT_FILES) {
            content::collect_preview(&self.0, &self.0.join(name), &mut files, &cancel).unwrap();
        }
        ExportInstance {
            resource_root: self.0.clone(),
            preview: ExportPreview {
                name: "Regression Pack".into(),
                minecraft_version: "1.20.1".into(),
                loader: None,
                files,
                launcher: crate::portable_pack::launcher_availability(),
                instance_settings: vec![],
                instance_settings_omitted: vec![],
                instance_settings_error: None,
            },
        }
    }

    fn request(&self, format: ExportFormat, files: Vec<String>) -> PackExportRequest {
        PackExportRequest {
            dir: self.0.to_string_lossy().into_owned(),
            version_id: "regression-instance".into(),
            name: "Regression Pack".into(),
            version: "1.0".into(),
            files,
            bundle_resources: true,
            modrinth_only: false,
            format,
            include_launcher: false,
            include_personalization: false,
        }
    }

    fn record_archive(
        &self,
        name: &str,
        snapshot: &ExportSnapshot,
        request: &PackExportRequest,
        archive: &Path,
        payload: Option<&str>,
    ) {
        let report = json!({
            "schemaVersion": 1,
            "container": if payload.is_some() { "portablezip" } else { "mrpack" },
            "requestFormat": request.format,
            "archive": archive.strip_prefix(&self.0).unwrap().to_string_lossy(),
            "payload": payload,
            "snapshotRoot": snapshot.root.strip_prefix(&self.0).unwrap().to_string_lossy(),
            "sourceRoot": ".",
            "previewEntries": snapshot.preview.files.len(),
            "previewBytes": serde_json::to_vec(&snapshot.preview).unwrap().len(),
            "selections": request.files,
            "files": snapshot.files,
            "byteComparisonCount": snapshot.files.len(),
        });
        self.put(
            &format!("archive-evidence-{name}.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        );
        eprintln!(
            "[pack-export] {name}: preview={} entries/{} bytes, selected={} files, archive={}",
            report["previewEntries"],
            report["previewBytes"],
            snapshot.files.len(),
            archive.display()
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::env::var_os("NICKEL_EXPORT_REGRESSION_OUTPUT").is_some() {
            eprintln!("[pack-export] retained fixture={}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn thousand_file_fixture() -> Fixture {
    let fixture = Fixture::new();
    for index in 0..1200 {
        fixture.put(
            &format!("resourcepacks/Selected/assets/example/textures/{index}.png"),
            format!("texture-{index}\0\u{ff}"),
        );
        fixture.put(
            &format!("config/bulk/{index}.json"),
            format!(r#"{{"index":{index}}}"#),
        );
        fixture.put(
            &format!("screenshots/{index}.png"),
            format!("screenshot-{index}\0"),
        );
        fixture.put(
            &format!("saves/Selected/region/{index}.mca"),
            format!("region-{index}\0"),
        );
        fixture.put(
            &format!("journeymap/data/sp/world/{index}.dat"),
            format!("map-{index}"),
        );
    }
    fixture.put("resourcepacks/Selected/pack.mcmeta", br#"{"pack":{"pack_format":15,"description":{"text":"\u00a7aEmerald ","extra":[{"text":"Workshop"},{"text":"\nSecond line"}]}}}"#);
    fixture.put(
        "resourcepacks/Other/pack.mcmeta",
        br#"{"pack":{"description":"Other Pack"}}"#,
    );
    fixture.put("resourcepacks/Other/assets/example/ignored.png", b"ignored");
    fixture.put("saves/Selected/level.dat", b"selected level");
    fixture.put(
        "saves/Selected/playerdata/player.dat",
        b"world player inventory\0\xff",
    );
    fixture.put(
        "saves/Selected/stats/player.json",
        br#"{"stats":{"minecraft:custom":{"minecraft:play_time":42}}}"#,
    );
    fixture.put("saves/Other/level.dat", b"ignored world");
    fixture.put("shaderpacks/Forest/shaders/main.fsh", b"shader source");
    fixture.put("config/jei/world/private/bookmarks.ini", b"personal");
    fixture.put("config/accounts.json", b"never exported");
    fixture.put("config/cache/ignored.bin", b"ignored");
    fixture.put("screenshots_other/one.png", b"ignored sibling");
    fixture.put("libraries/core.jar", b"game core");
    fixture.put("assets/core.bin", b"game core");
    fixture.put("accounts/profile.json", b"account");
    fixture
}

fn bulk_selections() -> Vec<String> {
    [
        "@dir/resourcepacks/resourcepacks/Selected",
        "@dir/config/config",
        "@dir/screenshots/screenshots",
        "@dir/saves/saves/Selected",
        "@dir/shaderpacks/shaderpacks/Forest",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn assert_payload_bytes(
    fixture: &Fixture,
    snapshot: &ExportSnapshot,
    archive: &Path,
    destination: &Path,
) {
    let cancel = CancellationToken::new();
    let (_, imported) = modpacks::local_manifest(archive, &cancel).unwrap();
    let extracted = extract_zip_subtrees(
        archive,
        destination,
        &imported.overrides,
        &[],
        &ArchiveLimits::default(),
        &cancel,
    )
    .unwrap();
    assert_eq!(extracted.len(), snapshot.files.len());
    for relative in &snapshot.files {
        let expected = std::fs::read(fixture.0.join(relative)).unwrap();
        assert_eq!(
            std::fs::read(snapshot.root.join(relative)).unwrap(),
            expected,
            "snapshot {relative}"
        );
        assert_eq!(
            std::fs::read(destination.join(relative)).unwrap(),
            expected,
            "archive {relative}"
        );
    }
}

#[test]
fn thousand_file_directories_have_constant_preview_and_complete_staged_payloads() {
    let fixture = thousand_file_fixture();
    let instance = fixture.instance();
    assert_eq!(instance.preview.files.len(), 10);
    assert_eq!(
        instance
            .preview
            .files
            .iter()
            .filter(|entry| entry.group == "config")
            .count(),
        2
    );
    assert_eq!(
        instance
            .preview
            .files
            .iter()
            .filter(|entry| entry.group == "screenshots")
            .count(),
        1
    );
    assert!(
        instance
            .preview
            .files
            .iter()
            .all(|entry| entry.kind == ExportEntryKind::Directory && entry.size_bytes.is_none())
    );
    assert_eq!(
        instance
            .preview
            .files
            .iter()
            .find(|entry| entry.source_path == "resourcepacks/Selected")
            .unwrap()
            .display_name,
        "Emerald Workshop"
    );
    let preview_bytes = serde_json::to_vec(&instance.preview).unwrap();
    assert!(preview_bytes.len() < 6000);
    fixture.put("preview.json", &preview_bytes);
    let default = instance
        .preview
        .files
        .iter()
        .filter(|entry| entry.selected)
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    let default_files = content::selected_files(
        &fixture.0,
        &instance.preview.files,
        &default,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(
        default_files
            .iter()
            .all(|file| !file.path.starts_with("saves/"))
    );
    // The former 4803 count omitted two world-owned files; full selection is 4805.
    for format in [ExportFormat::Mrpack, ExportFormat::Portable] {
        let name = if format == ExportFormat::Mrpack {
            "standard"
        } else {
            "portable-inner"
        };
        let workspace = fixture.0.join(name);
        std::fs::create_dir(&workspace).unwrap();
        let request = fixture.request(format, bulk_selections());
        let cancel = CancellationToken::new();
        let mut copied = 0;
        let snapshot = snapshot_instance(
            fixture.instance(),
            &request,
            &workspace,
            &cancel,
            |done, total, _| {
                copied = done;
                assert_eq!(total, 4805);
            },
        )
        .unwrap();
        assert_eq!(copied, 4805);
        assert_eq!(snapshot.files.len(), 4805);
        assert!(
            snapshot
                .files
                .contains(&"saves/Selected/playerdata/player.dat".into())
        );
        assert!(
            snapshot
                .files
                .contains(&"saves/Selected/stats/player.json".into())
        );
        assert!(!snapshot.files.iter().any(|path| path.contains("Other")
            || path.starts_with("journeymap/")
            || path.contains("jei/world")
            || path.contains("accounts")
            || path.contains("cache")));
        let archive = workspace.join("payload.mrpack");
        write_snapshot_payload(
            &snapshot,
            &archive,
            manifest(&snapshot.preview, &config(&request), &[]).unwrap(),
            &HashSet::new(),
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        let destination = workspace.join("imported");
        assert_payload_bytes(&fixture, &snapshot, &archive, &destination);
        for excluded in [
            "resourcepacks/Other",
            "saves/Other",
            "config/jei/world",
            "screenshots_other",
            "libraries",
            "assets",
            "accounts",
        ] {
            assert!(!destination.join(excluded).exists(), "{excluded}");
        }
        fixture.record_archive(name, &snapshot, &request, &archive, None);
    }
}

#[test]
fn new_content_categories_separate_configuration_from_personal_data() {
    let fixture = Fixture::new();
    let expected = [
        ("config/jei/world/world/bookmarks.ini", "jeiPersonal", false),
        ("config/jei/jei-client.ini", "config", true),
        ("emi.json", "emiPersonal", false),
        ("config/emi.json", "emiPersonal", false),
        ("patchouli_data.json", "patchouliPersonal", false),
        ("config/patchouli_data.json", "patchouliPersonal", false),
        ("journeymap/config/client.json", "config", true),
        ("journeymap/server/server.json", "config", true),
        ("journeymap/journeymap.topo.config", "config", true),
        ("journeymap/data/sp/world/map.dat", "maps", false),
        ("tacz/default/guns/a.json", "tacz", true),
        ("config/tacz/custom/example/a.json", "tacz", true),
        ("immersive_paintings/paintings/one.png", "paintings", true),
        ("schematics/house.nbt", "schematics", false),
        ("replay_recordings/game.mcpr", "replays", false),
        ("replay_videos/game.mp4", "replays", false),
        ("patchouli_books/guide/book.json", "packData", true),
        ("datapacks/example/data/recipe.json", "packData", true),
        ("kubejs/server_scripts/recipes.js", "packData", true),
        ("scripts/recipes.zs", "packData", true),
        ("openloader/data/example.zip", "packData", true),
        ("global_packs/required/example.zip", "packData", true),
        ("resources/example/textures/one.png", "packData", true),
        ("structures/example.nbt", "packData", true),
        ("packmenu/textures/menu.png", "packData", true),
        ("config/openloader/data/example.zip", "packData", true),
        ("config/paxi/datapacks/example.zip", "packData", true),
        ("saves/World/playerdata/player.dat", "saves", false),
        ("saves/World/stats/player.json", "saves", false),
        ("saves/World/advancements/player.json", "saves", false),
        ("saves/World/data/cache/mod-state.dat", "saves", false),
        (
            "resourcepacks/Token Workshop/assets/example/cache/one.png",
            "resourcepacks",
            true,
        ),
        (
            "shaderpacks/Forest/shaders/lib/stats/util.glsl",
            "shaderpacks",
            true,
        ),
    ];
    for (path, group, selected) in expected {
        fixture.put(path, format!(r#"{{"content":"{path}"}}"#));
        assert_eq!(group_for(path), Some((group, selected)), "{path}");
    }
    let excluded = [
        "kubejs/cache/data.json",
        "scripts/probe/output.json",
        "tacz/exported/gun.json",
        "resources/playerdata/player.json",
        "structures/stats/a.json",
        "config/service/credentials.json",
        "libraries/a.jar",
        "assets/index.json",
    ];
    for path in excluded {
        fixture.put(path, b"excluded");
        assert!(group_for(path).is_none(), "{path}");
    }
    let instance = fixture.instance();
    for entry in &instance.preview.files {
        if matches!(
            entry.group.as_str(),
            "jeiPersonal"
                | "emiPersonal"
                | "patchouliPersonal"
                | "maps"
                | "schematics"
                | "replays"
                | "saves"
        ) {
            assert!(!entry.selected, "{}", entry.path);
        }
    }
    let default = instance
        .preview
        .files
        .iter()
        .filter(|entry| entry.selected)
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    let all = instance
        .preview
        .files
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    let cancel = CancellationToken::new();
    for (name, selections, include_personal) in [
        ("default-categories", default, false),
        ("all-categories", all, true),
    ] {
        let workspace = fixture.0.join(name);
        std::fs::create_dir(&workspace).unwrap();
        let request = fixture.request(ExportFormat::Mrpack, selections);
        let snapshot = snapshot_instance(
            fixture.instance(),
            &request,
            &workspace,
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        let actual: HashSet<_> = snapshot.files.iter().map(String::as_str).collect();
        for (path, _, selected) in expected {
            assert_eq!(
                actual.contains(path),
                selected || include_personal,
                "{name}: {path}"
            );
        }
        for path in excluded {
            assert!(!actual.contains(path), "{path}");
        }
        let archive = workspace.join("payload.mrpack");
        write_snapshot_payload(
            &snapshot,
            &archive,
            manifest(&snapshot.preview, &config(&request), &[]).unwrap(),
            &HashSet::new(),
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        assert_payload_bytes(&fixture, &snapshot, &archive, &workspace.join("imported"));
        fixture.record_archive(name, &snapshot, &request, &archive, None);
    }
}

#[test]
fn directory_tokens_cannot_escape_scopes() {
    let fixture = Fixture::new();
    fixture.put("screenshots/one.png", b"image");
    fixture.put("saves/World/level.dat", b"world");
    let cancel = CancellationToken::new();
    for value in [
        "@dir/config/config/jei/world",
        "@dir/jeiPersonal/config",
        "@dir/resourcepacks/resourcepacks",
        "@dir/saves/saves/World/region",
        "@dir/config/../config",
        "@dir/config/C:/config",
        "@dir/config/config/",
        "@dir/packData/resources/playerdata",
        "@dir/maps/libraries",
        "config/../../accounts.json",
    ] {
        assert!(content::parse_selection(value).is_err(), "{value}");
    }
    let preview = fixture.instance().preview;
    assert!(
        content::selected_files(
            &fixture.0,
            &preview.files,
            &["@dir/saves/saves/Other".into()],
            &cancel
        )
        .is_err()
    );
    assert!(
        content::selected_files(&fixture.0, &preview.files, &["screenshots".into()], &cancel)
            .is_err()
    );
    assert!(
        content::selected_files(
            &fixture.0,
            &preview.files,
            &["screenshots/one.png".into(), "screenshots/ONE.png".into()],
            &cancel
        )
        .is_err()
    );
}

#[test]
fn zipped_resources_are_single_items_with_bounded_metadata_and_safe_fallbacks() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.0.join("resourcepacks")).unwrap();
    let archive = fixture.0.join("resourcepacks/file-name.zip");
    let mut inline = vec![(
        "pack.mcmeta".into(),
        br#"{"pack":{"description":[{"text":"\u00a7bOcean "},{"text":"Glass"}]}}"#.to_vec(),
    )];
    for index in 0..1000 {
        inline.push((
            format!("assets/example/textures/{index}.png"),
            b"image".to_vec(),
        ));
    }
    let cancel = CancellationToken::new();
    write_zip_files(
        &archive,
        &fixture.0,
        &[],
        &inline,
        &ArchiveLimits::default(),
        &cancel,
        |_, _, _| {},
    )
    .unwrap();
    fixture.put("resourcepacks/broken.zip", b"not a zip");
    fixture.put(
        "resourcepacks/Empty/pack.mcmeta",
        br#"{"pack":{"description":"\u00a7r  "}}"#,
    );
    fixture.put("resourcepacks/Corrupt/pack.mcmeta", b"not json");
    fixture.put(
        "resourcepacks/Large/pack.mcmeta",
        vec![b'x'; 128 * 1024 + 1],
    );
    let instance = fixture.instance();
    assert_eq!(instance.preview.files.len(), 5);
    let zip = instance
        .preview
        .files
        .iter()
        .find(|entry| entry.source_path == "resourcepacks/file-name.zip")
        .unwrap();
    assert_eq!(zip.path, "resourcepacks/file-name.zip");
    assert_eq!(zip.kind, ExportEntryKind::File);
    assert_eq!(zip.display_name, "Ocean Glass");
    for fallback in ["broken", "Empty", "Corrupt", "Large"] {
        assert!(
            instance
                .preview
                .files
                .iter()
                .any(|entry| entry.display_name == fallback),
            "{fallback}"
        );
    }
    assert!(!is_download_resource(
        "resourcepacks/Empty/assets/example/nested.zip"
    ));
    let workspace = fixture.0.join("zip-stage");
    std::fs::create_dir(&workspace).unwrap();
    let request = fixture.request(ExportFormat::Mrpack, vec![zip.path.clone()]);
    let snapshot =
        snapshot_instance(instance, &request, &workspace, &cancel, |_, _, _| {}).unwrap();
    assert_eq!(snapshot.files, vec!["resourcepacks/file-name.zip"]);
    assert_eq!(
        std::fs::read(snapshot.root.join("resourcepacks/file-name.zip")).unwrap(),
        std::fs::read(archive).unwrap()
    );
    assert_eq!(
        read_zip_entry(
            &snapshot.root.join("resourcepacks/file-name.zip"),
            "assets/example/textures/999.png",
            16,
            &cancel
        )
        .unwrap(),
        b"image"
    );
}

#[test]
fn cancelled_and_credential_bearing_directory_snapshots_are_not_exportable() {
    let fixture = Fixture::new();
    fixture.put("config/bulk/client.json", br#"{"quality":"high"}"#);
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let workspace = fixture.0.join("cancel-stage");
    std::fs::create_dir(&workspace).unwrap();
    let request = fixture.request(ExportFormat::Mrpack, vec!["@dir/config/config".into()]);
    let error = snapshot_instance(
        fixture.instance(),
        &request,
        &workspace,
        &cancelled,
        |_, _, _| {},
    )
    .err()
    .unwrap();
    assert!(error.contains("取消"), "{error}");
    assert!(!workspace.join("export-content").exists());
    let cancel = CancellationToken::new();
    let mid_stage = fixture.0.join("mid-cancel-stage");
    std::fs::create_dir(&mid_stage).unwrap();
    fixture.put("config/bulk/second.json", b"{}");
    let mid_cancel = CancellationToken::new();
    let interrupted = snapshot_instance(
        fixture.instance(),
        &request,
        &mid_stage,
        &mid_cancel,
        |done, _, _| {
            if done == 1 {
                mid_cancel.cancel();
            }
        },
    );
    assert!(interrupted.err().unwrap().contains("取消"));
    assert!(!mid_stage.join("payload.mrpack").exists());
    fixture.put(
        "config/bulk/service.json",
        br#"{"access\u005ftoken":"private"}"#,
    );
    let error = snapshot_instance(
        fixture.instance(),
        &request,
        &workspace,
        &cancel,
        |_, _, _| {},
    )
    .err()
    .unwrap();
    assert!(
        error.contains("真实凭据") && error.contains("config/bulk/service.json"),
        "{error}"
    );
    assert!(!error.contains("private"));
    let cases = [
        ("config/client.json", r#"{"accessToken":"private-value"}"#),
        (
            "config/client.json",
            r#"{"nested":[{"api_key":"private-value"}]}"#,
        ),
        (
            "config/client.json",
            r#"{"access\u005ftoken":"private-value"}"#,
        ),
        (
            "config/client.json",
            r#"{"headers":{"Authorization":"Bearer private-value"}}"#,
        ),
        (
            "config/client.json",
            r#"{"endpoint":"https://example.com/data?api_key=private-value"}"#,
        ),
        (
            "config/client.json",
            r#"{"endpoint":"https://user:private-value@example.com/data"}"#,
        ),
        (
            "config/client.json5",
            r#"{ accessToken: 'private-value', }"#,
        ),
        (
            "config/client.toml",
            "[service]\napi_key = \"private-value\"\n",
        ),
        (
            "config/client.yaml",
            "service:\n  refresh_token: private-value\n",
        ),
        (
            "config/client.properties",
            "service.api_key=private-value\n",
        ),
        (
            "scripts/service.js",
            "const client_secret = 'private-value';",
        ),
        (
            "scripts/service.js",
            r#"const accessToken = "private-value";"#,
        ),
        (
            "scripts/service.js",
            r#"const access\u005fToken = 'private-value';"#,
        ),
    ];
    for (relative, text) in cases {
        fixture.put(relative, text);
        let error = reject_credentials(&fixture.0.join(relative), relative)
            .err()
            .unwrap_or_else(|| panic!("accepted {relative}: {text}"));
        assert!(error.contains(relative));
        assert!(!error.contains("private-value"));
    }
    let fixture = Fixture::new();
    fixture.put("scripts/host.zs", b"folder=C:/Users/private/game");
    let host_stage = fixture.0.join("host-standard");
    std::fs::create_dir(&host_stage).unwrap();
    let host_request = fixture.request(ExportFormat::Mrpack, vec!["@dir/packData/scripts".into()]);
    let standard = snapshot_instance(
        fixture.instance(),
        &host_request,
        &host_stage,
        &cancel,
        |_, _, _| {},
    )
    .unwrap();
    assert_eq!(
        std::fs::read(standard.root.join("scripts/host.zs")).unwrap(),
        b"folder=C:/Users/private/game"
    );
    let portable_stage = fixture.0.join("host-portable");
    std::fs::create_dir(&portable_stage).unwrap();
    let portable_request = fixture.request(ExportFormat::Portable, host_request.files);
    let error = snapshot_instance(
        fixture.instance(),
        &portable_request,
        &portable_stage,
        &cancel,
        |_, _, _| {},
    )
    .err()
    .unwrap();
    assert!(
        error.contains("本机路径") && error.contains("scripts/host.zs"),
        "{error}"
    );
    fixture.put("policy-evidence.json", serde_json::to_vec_pretty(&json!({"credentialCases": cases.len(), "preCancelled": true, "midSnapshotCancelled": true, "standardHostPathRetained": true, "portableHostPathRejected": true})).unwrap());
}

#[test]
fn credential_collections_are_rejected_before_either_snapshot_format() {
    let cases = [
        (
            "config/service.toml",
            r#"api_keys = ["private-collection-value"]"#,
        ),
        (
            "config/service.json5",
            "{apiKeys: ['private-collection-value'],}",
        ),
        (
            "config/service.yaml",
            r#"apiKeys: ["private-collection-value"]"#,
        ),
        (
            "config/service.toml",
            r#"api_keys = { primary = ["private-collection-value"] }"#,
        ),
        (
            "config/service.json5",
            "{apiKeys: [{primary: ['private-collection-value']}],}",
        ),
        (
            "config/service.yaml",
            "apiKeys: {primary: [null, 'private-collection-value']}",
        ),
        (
            "config/service.json5",
            r#"{apiKeys: [/* ] ignored */ "private\"collection-value"],}"#,
        ),
        (
            "scripts/service.js",
            "const apiKeys = [{primary: 'private-collection-value'}];",
        ),
    ];
    for (relative, text) in cases {
        let fixture = Fixture::new();
        fixture.put(relative, text);
        for (index, format) in [ExportFormat::Mrpack, ExportFormat::Portable]
            .into_iter()
            .enumerate()
        {
            let workspace = fixture.0.join(format!("collection-stage-{index}"));
            std::fs::create_dir(&workspace).unwrap();
            let token = if relative.starts_with("config/") {
                "@dir/config/config"
            } else {
                "@dir/packData/scripts"
            };
            let request = fixture.request(format, vec![token.into()]);
            let error = snapshot_instance(
                fixture.instance(),
                &request,
                &workspace,
                &CancellationToken::new(),
                |_, _, _| {},
            )
            .err()
            .unwrap_or_else(|| panic!("accepted {relative}: {text}"));
            assert!(
                error.contains("真实凭据") && error.contains(relative),
                "{error}"
            );
            assert!(!error.contains("private"));
            assert!(!workspace.join("payload.mrpack").exists());
        }
    }
}

fn assert_policy_archive(relative: &str, text: &str, allowed: bool) {
    let fixture = Fixture::new();
    fixture.put(relative, text);
    let cancel = CancellationToken::new();
    for (index, format) in [ExportFormat::Mrpack, ExportFormat::Portable]
        .into_iter()
        .enumerate()
    {
        let workspace = fixture.0.join(format!("policy-stage-{index}"));
        std::fs::create_dir(&workspace).unwrap();
        let instance = fixture.instance();
        let selected = instance
            .preview
            .files
            .iter()
            .filter(|entry| entry.selected)
            .map(|entry| entry.path.clone())
            .collect();
        let request = fixture.request(format, selected);
        let archive = workspace.join("payload.mrpack");
        let result = snapshot_instance(instance, &request, &workspace, &cancel, |_, _, _| {});
        if !allowed {
            let error = result
                .err()
                .unwrap_or_else(|| panic!("accepted {relative}: {text}"));
            assert!(error.contains(relative), "{error}");
            assert!(
                !error.contains("archive-secret") && !error.contains("round03-secret"),
                "{error}"
            );
            assert!(!archive.exists());
            fixture.put(&format!("policy-rejection-{index}.json"), serde_json::to_vec_pretty(&json!({
                "relative":relative,"requestFormat":request.format,"error":error,"archiveAbsent":true,
            })).unwrap());
            continue;
        }
        let snapshot = result.unwrap_or_else(|error| panic!("{relative}: {text}: {error}"));
        assert_eq!(snapshot.files, vec![relative.to_owned()]);
        write_snapshot_payload(
            &snapshot,
            &archive,
            manifest(&snapshot.preview, &config(&request), &[]).unwrap(),
            &HashSet::new(),
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        let bytes = read_zip_entry(
            &archive,
            &format!("overrides/{relative}"),
            CONFIG_MAX_BYTES,
            &cancel,
        )
        .unwrap();
        assert_eq!(bytes, text.as_bytes());
        assert!(!String::from_utf8_lossy(&bytes).contains("archive-secret"));
        fixture.record_archive(
            &format!("policy-{index}"),
            &snapshot,
            &request,
            &archive,
            None,
        );
    }
}

#[test]
fn yaml_block_and_flow_values_retain_sensitive_parent_context_in_both_formats() {
    for text in [
        "api_keys:\n  - round03-secret\nquality: high\n",
        "api_keys:\n  primary: round03-secret\nquality: high\n",
        "service:\n  api_keys:\n    - primary: archive-secret\nquality: high\n",
        "service:\n  api_keys:\n    primary:\n      - []\n      - {secondary: archive-secret}\nquality: high\n",
        "api_keys: [{primary: archive-secret}]\nquality: high\n",
        "password: |\n  archive-secret\nquality: high\n",
        "password: >-\n  archive-secret\nquality: high\n",
        "api_keys:\n  primary: '${PASSWORD:-archive-secret}'\nquality: high\n",
        "api_keys:\n  primary: 12345\nquality: high\n",
        "api_keys: []\n---\napi_keys:\n  - archive-secret\n",
        "api_keys: []\napi_keys: {primary: archive-secret}\n",
    ] {
        assert_eq!(yaml_has_credentials(text), Ok(true), "{text}");
        for extension in ["yaml", "yml"] {
            assert_policy_archive(&format!("config/service.{extension}"), text, false);
        }
    }
    for text in [
        "api_keys:\nquality: high\n",
        "api_keys: []\nquality: high\n",
        "api_keys: {}\nquality: high\n",
        "api_keys:\n  primary: []\n  secondary: {}\nquality: high\n",
        "api_keys:\n  - null\n  - ~\n  - false\n  - true\n  - ''\nquality: high\n",
        "api_keys:\n  primary: '${PASSWORD}'\n  other: '%API_KEY%'\n  empty: '***'\nquality: high\n",
        "api_keys:\n  - primary: [placeholder, 'YOUR_API_KEY_HERE']\n  - {secondary: []}\nquality: high\n",
        "api_keys: {primary: ['', '${API_KEY}', '***', null, false]}\nquality: high\n",
        "password: >-\n  ${PASSWORD}\nquality: high\n",
        "description: |\n  password = example\nquality: high\n",
        "api_keys: [] # example is a comment\nquality: high\n",
        "quality: high\nprofiles:\n  - primary: ordinary-data\n",
    ] {
        assert_eq!(yaml_has_credentials(text), Ok(false), "{text}");
        for extension in ["yaml", "yml"] {
            assert_policy_archive(&format!("config/service.{extension}"), text, true);
        }
    }
}

#[test]
fn yaml_syntax_aliases_tags_and_processing_limits_fail_closed_without_expansion() {
    let depth_at_limit = format!(
        "{}null{}",
        "[".repeat(MAX_POLICY_DEPTH),
        "]".repeat(MAX_POLICY_DEPTH)
    );
    assert_eq!(yaml_has_credentials(&depth_at_limit), Ok(false));
    for text in [
        "api_keys: ['archive-secret'\n".to_owned(),
        "api_keys:\n  primary: \"unterminated\n".to_owned(),
        "defaults: &base [placeholder]\napi_keys: *base\n".to_owned(),
        "defaults: &base [*base]\napi_keys: *base\n".to_owned(),
        "a: &a [placeholder]\nb: &b [*a, *a]\nc: &c [*b, *b]\napi_keys: *c\n".to_owned(),
        "api_keys: !!str archive-secret\n".to_owned(),
        "? [password, secondary]\n: []\n".to_owned(),
        format!(
            "{}null{}",
            "[".repeat(MAX_POLICY_DEPTH + 1),
            "]".repeat(MAX_POLICY_DEPTH + 1)
        ),
        format!("[{}]", "null,".repeat(MAX_POLICY_EVENTS)),
        format!("description: '{}'", "x".repeat(CONFIG_MAX_BYTES as usize)),
    ] {
        assert_eq!(yaml_has_credentials(&text), Err(()));
        for extension in ["yaml", "yml"] {
            assert_policy_archive(&format!("config/service.{extension}"), &text, false);
        }
    }
}

#[test]
fn javascript_regexes_have_opaque_lexical_boundaries_and_byte_preserved_payloads() {
    for text in [
        r#"const cleanName = 'foo"bar'.replace(/"/g, '');"#,
        r#"const cleanName = 'foo/bar'.replace(/\//g, '');"#,
        r#"const path = 'foo\\bar'.replace(/\\/g, '');"#,
        r#"const cleanName = name.replace(/["'/\\\]]/g, '');"#,
        r#"const regex = /[a-z]+/dgimsuy;"#,
        r#"const regex = /password="example"/g;"#,
        r#"function clean(name) { return /"/g.test(name); }"#,
        r#"if (ready) /"/g.test(name); else /'/g.test(name);"#,
        r#"const x = ready ? /"/g : /'/g;"#,
        r#"const regex = /* " harmless */ /[/*'"]/g; // password='example'"#,
        "const x = 12 / 3; const clean = /\"/g;",
        "const x = (12 + 6) / (3); const clean = /\"/g;",
        "let x = 12; x /= 3; const clean = /\"/g;",
        "let x = 12; const y = x++ / 3; const clean = /\"/g;",
        "let x = 12; const y = --x / 3; const clean = /\"/g;",
        "const x = [12][0] / 3; const clean = /\"/g;",
        "const x = ({count:12}).count / 3; const clean = /\"/g;",
        "const x = 12 / /* safe */ 3; const clean = /\"/g;",
    ] {
        assert!(javascript_policy_text(text).is_ok(), "{text}");
        assert_policy_archive("kubejs/server_scripts/service.js", text, true);
        let protected = format!("{text}\nsettings['password'] = ('archive-secret');");
        assert!(javascript_policy_text(&protected).is_ok(), "{protected}");
        assert_policy_archive("kubejs/server_scripts/service.js", &protected, false);
    }
    for text in [
        r#"settings['password'] = process.env.PASSWORD; const clean = /"/g;"#,
        r#"settings['password'] = System.getenv('PASSWORD'); const clean = /"/g;"#,
        r#"settings['password'] = '${PASSWORD}'; const clean = /"/g;"#,
    ] {
        assert_policy_archive("kubejs/server_scripts/service.js", text, true);
    }
}

#[test]
fn javascript_regex_division_comments_and_limits_cannot_hide_sensitive_writes() {
    for text in [
        r#"const regex = /"/g; settings.password = process.env.PASSWORD || 'archive-secret';"#
            .to_owned(),
        "settings.password = 12 / 'archive-secret';".to_owned(),
        "settings.password /= 'archive-secret';".to_owned(),
        "settings.password = process.env.PASSWORD / 'archive-secret';".to_owned(),
        "let count = 12; count--; settings.password = 'archive-secret';".to_owned(),
        "const regex = /unterminated".to_owned(),
        "const regex = /[unterminated/;".to_owned(),
        "const regex = /a\\\nb/;".to_owned(),
        "const regex = /a/gg;".to_owned(),
        "const regex = /a/uv;".to_owned(),
        "const regex = /a/z;".to_owned(),
        "/* unterminated".to_owned(),
        format!(
            "const x={}0{};",
            "(".repeat(MAX_POLICY_DEPTH + 1),
            ")".repeat(MAX_POLICY_DEPTH + 1)
        ),
        ";".repeat(MAX_POLICY_EVENTS),
        format!("const regex=/{} /;", "x".repeat(CONFIG_MAX_BYTES as usize)),
    ] {
        assert_policy_archive("kubejs/server_scripts/service.js", &text, false);
    }
}

#[test]
fn javascript_regex_expression_context_cannot_mask_division_operand_credentials() {
    for expression in [
        "function() {}",
        "function named() {}",
        "function* named() {}",
        "function named(value = (12 / 3)) {}",
        "api.return",
        "api.if(ready)",
        "api?.return",
        "class extends mixin(Base) {}",
    ] {
        let sensitive = format!(
            "const x = {expression} / (settings.password = 'archive-secret') / denominator;"
        );
        assert_policy_archive("kubejs/server_scripts/service.js", &sensitive, false);
        assert!(credential_text(&sensitive, true), "{sensitive}");
        let safe = format!("const x = {expression} / denominator; const clean = /\"/g;");
        assert_policy_archive("kubejs/server_scripts/service.js", &safe, true);
    }
    for safe in [
        "function declared() {} /\"/g.test(name);",
        "if (ready) {} /\"/g.test(name);",
        "const fnValue = function() { return /\"/g.test(name); };",
    ] {
        assert_policy_archive("kubejs/server_scripts/service.js", safe, true);
    }
}

#[test]
fn yaml_and_javascript_policy_original_payloads_reject_credentials_and_preserve_controls() {
    for text in [
        "api_keys:\n  - round03-secret\nquality: high\n",
        "api_keys:\n  primary: round03-secret\nquality: high\n",
        "api_keys: [\"round03-secret\"]\nquality: high\n",
    ] {
        assert_policy_archive("config/service.yaml", text, false);
    }
    assert_policy_archive("config/service.yaml", "api_keys: []\nquality: high\n", true);
    for text in [
        "const cleanName = 'foo\"bar'.replace(/\"/g, '');\n",
        "const cleanName = 'foobar';\n",
    ] {
        assert_policy_archive("kubejs/server_scripts/service.js", text, true);
    }
    assert_policy_archive(
        "kubejs/server_scripts/service.js",
        "settings['password'] = ('round03-secret');\n",
        false,
    );
}

#[test]
fn toml_multiline_credentials_and_legal_quote_boundaries_are_checked_in_both_formats() {
    let mut unsafe_documents = vec![
        "password = \"\"\"archive-secret\"\"\"".to_owned(),
        "password = '''archive-secret'''".to_owned(),
        "password = \"\"\"\narchive-secret\nsecond line\"\"\"".to_owned(),
        "password = '''\narchive-secret\nsecond line'''".to_owned(),
        "password = \"\"\"archive-\\\n  secret\"\"\"".to_owned(),
        r#"password = """archive-\u0073ecret\"quoted\\tail""""#.to_owned(),
        "[password]\nprimary = '''archive-secret'''".to_owned(),
        "api_keys = [{primary = '''archive-secret'''}]".to_owned(),
    ];
    // Four/five quote delimiters include one/two literal quotes in the value.
    // Check the decoded value as well as rejection, so syntax errors cannot pass.
    for quote in ['"', '\''] {
        for extra in 1..=2 {
            let delimiter = quote.to_string().repeat(3 + extra);
            let document = format!("password = {delimiter}archive-secret{delimiter}");
            let parsed: toml::Value = toml::from_str(&document).unwrap();
            let inner = quote.to_string().repeat(extra);
            assert_eq!(
                parsed["password"].as_str().unwrap(),
                format!("{inner}archive-secret{inner}")
            );
            unsafe_documents.push(document);
        }
    }
    for text in unsafe_documents {
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        assert!(toml_has_credentials(&parsed, false), "{text}");
        assert_policy_archive("config/service.toml", &text, false);
    }
    for text in [
        "password = \"\"\"\"\"\"",
        "password = ''''''",
        "password = \"\"\"\n  \n\"\"\"",
        "password = '''\n  \n'''",
        "password = \"\"\"\\\n  \"\"\"",
        "password = \"\"\"YOUR_\\\n  PASSWORD_HERE\"\"\"",
        "password = '''${PASSWORD}'''",
        "password = \"\"\"***\"\"\"",
        "api_keys = [{primary = \"\"\"\"\"\"}, {primary = '''placeholder'''}]",
        "description = \"\"\"\"password documentation\"\"\"\"\npassword = false",
        "description = '''''password documentation'''''\npassword = ''",
        "description = '''literal\\text\npassword = example'''\napi_keys = []",
    ] {
        let parsed: toml::Value = toml::from_str(text).unwrap();
        assert!(!toml_has_credentials(&parsed, false), "{text}");
        assert_policy_archive("config/service.toml", text, true);
    }
}

#[test]
fn toml_invalid_or_over_budget_documents_cannot_fall_back_to_scalar_scanning() {
    for text in [
        "password = \"\"\"archive-secret".to_owned(),
        "password = '''archive-secret".to_owned(),
        "password = \"\"\"\\qarchive-secret\"\"\"".to_owned(),
        format!("password = {}{}", "[".repeat(100), "]".repeat(100)),
        format!("description = '{}'", "x".repeat(CONFIG_MAX_BYTES as usize)),
    ] {
        assert_policy_archive("config/service.toml", &text, false);
    }
}

#[test]
fn literal_script_subscript_writes_reject_secrets_but_reads_and_comparisons_export() {
    for text in [
        r#"settings["password"] = "archive-secret";"#,
        "settings['api_key'] = 'archive-secret';",
        "settings[ /* key */ 'password' /* end */ ] /* write */ = 'archive-secret';",
        "settings[\n'password'\n]\n=\n'archive-secret';",
        r#"settings["pass\u0077ord"] = "archive-secret";"#,
        "settings['password'] = [{value: 'archive-secret'}];",
    ] {
        assert_policy_archive("kubejs/server_scripts/service.js", text, false);
    }
    for text in [
        "const value = settings['password'];",
        "settings['password'] == 'example';",
        "settings['password'] === 'example';",
        "settings['password'] != 'example';",
        "settings['password'] !== 'example';",
        "const value = ok ? settings['password'] : 'example';",
        "settings['password'] = '';",
        "settings['password'] = '${PASSWORD}';",
        "settings['password'] = process.env.PASSWORD;",
        "settings['password'] = [null, {}, 'placeholder'];",
        "// settings['password'] = 'example';\nconst passwordRequired = true;",
        "/* settings['password'] = 'example'; */ const key = 'password';",
    ] {
        assert_policy_archive("kubejs/server_scripts/service.js", text, true);
    }
}

#[test]
fn script_literal_writes_use_bounded_expression_and_escape_rules() {
    for operator in ["=", "??=", "||=", "&&=", "+="] {
        for key in [
            r#""password""#,
            r#""pass\x77ord""#,
            r#"'pass\u{77}ord'"#,
            r#"'pass\167ord'"#,
        ] {
            for value in [
                "'archive-secret'",
                "(('archive-secret'))",
                "process.env.PASSWORD || 'archive-secret'",
                "[({value:'archive-secret'})]",
            ] {
                assert_policy_archive(
                    "kubejs/server_scripts/service.js",
                    &format!("settings[{key}] {operator} {value};"),
                    false,
                );
            }
        }
    }
    for text in [
        "settings['password'] = (process.env.PASSWORD);",
        "settings['password'] ??= ('${PASSWORD}');",
        "settings['password'] = System.getenv('PASSWORD');",
        "settings['password'] = ['', null, {primary: '%PASSWORD%'}];",
        "settings['password'] === ('example');",
        "settings['password'] !== ('example');",
        "const x = ok ? settings['password'] : 'example';",
        "const documentation = 'password = not executed';",
    ] {
        assert_policy_archive("kubejs/server_scripts/service.js", text, true);
    }
    for text in [
        "settings.password = ('unterminated",
        "settings.password = ([)]",
    ] {
        assert_policy_archive("kubejs/server_scripts/service.js", text, false);
    }
    assert_policy_archive(
        "kubejs/server_scripts/service.js",
        &format!("settings.password={}''{};", "(".repeat(65), ")".repeat(65)),
        false,
    );
}

#[test]
fn only_complete_valid_environment_placeholders_are_exempt_in_all_formats() {
    for (value, allowed) in [
        ("${PASSWORD}", true),
        ("%PASSWORD%", true),
        ("${_PASSWORD2}", true),
        ("archive-secret${}", false),
        ("archive-secret-${USER}", false),
        ("${PASSWORD:-archive-secret}", false),
        ("${}", false),
        ("${1PASSWORD}", false),
        ("%PASSWORD%archive-secret", false),
        ("%archive-secret%", false),
        ("process.env.PASSWORD + archive-secret", false),
    ] {
        let literal = serde_json::to_string(value).unwrap();
        for (path, source) in [
            ("config/service.toml", format!("password = {literal}")),
            ("config/service.json", format!("{{\"password\":{literal}}}")),
            (
                "kubejs/server_scripts/service.js",
                format!("settings['password'] = ({literal});"),
            ),
        ] {
            assert_policy_archive(path, &source, allowed);
        }
    }
}

#[test]
fn credential_collections_keep_empty_placeholder_and_description_values_safe() {
    for text in [
        "api_keys = []",
        "api_keys = {}",
        "{apiKeys: [{primary: []}, {}, [null, false, true]],}",
        "apiKeys: {primary: ['', 'YOUR_API_KEY_HERE', '${API_KEY}', '***']}",
        "api_keys = [# private-collection-value\n 'placeholder', {}]",
        "{apiKeys: [/* private-collection-value */], description: 'apiKeys: documentation',}",
        "const apiKeys = [process.env.API_KEY, {primary: process.env.OTHER_KEY}];",
    ] {
        assert!(
            !credential_text(text, text.starts_with("const ")),
            "rejected {text}"
        );
    }
    for text in [
        "api_keys = [",
        "api_keys = [{]}",
        "api_keys = ['unterminated",
    ] {
        assert!(
            credential_text(text, false),
            "accepted malformed value: {text}"
        );
    }
    let deep = format!("api_keys = {}{}", "[".repeat(65), "]".repeat(65));
    assert!(credential_text(&deep, false));
}

#[test]
fn symlinks_inside_selected_directories_are_rejected_at_export_time() {
    let fixture = Fixture::new();
    fixture.put("config/safe.json", b"{}");
    fixture.put("outside.txt", b"must not export");
    let preview = fixture.instance().preview;
    let link = fixture.0.join("config/linked.txt");
    #[cfg(unix)]
    std::os::unix::fs::symlink(fixture.0.join("outside.txt"), &link).unwrap();
    #[cfg(windows)]
    {
        let result = std::os::windows::fs::symlink_file(fixture.0.join("outside.txt"), &link);
        if let Err(error) = result {
            assert_eq!(error.raw_os_error(), Some(1314));
            let junction = fixture.0.join("config/junction");
            let status = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&junction)
                .arg(&fixture.0)
                .status()
                .unwrap();
            assert!(status.success());
            let rejected = content::selected_files(
                &fixture.0,
                &preview.files,
                &["@dir/config/config".into()],
                &CancellationToken::new(),
            )
            .is_err();
            std::fs::remove_dir(junction).unwrap();
            assert!(rejected);
            return;
        }
    }
    assert!(
        content::selected_files(
            &fixture.0,
            &preview.files,
            &["@dir/config/config".into()],
            &CancellationToken::new()
        )
        .is_err()
    );
}

#[test]
fn normal_mod_configuration_keys_and_placeholders_do_not_block_either_format() {
    let fixture = Fixture::new();
    let safe = [
        (
            "config/mod-client.json",
            r#"{"passwordRequired":true,"authorizationMode":"offline","apiKeyEnabled":false,"accessTokenLifetime":1200,"password":"","accessToken":null,"clientSecret":false,"apiKeys":[],"message":"Basic crafting","description":"password authorization api_key"}"#,
        ),
        (
            "config/recipe_tokens.toml",
            "# api_key = \"documentation example\"\napi_key = \"\"\npasswordRequired = true\nauthorizationMode = \"offline\"\n",
        ),
        (
            "config/mod-client.json5",
            "// accessToken = \"documentation example\"\n{ accessToken: '', passwordRequired: true, apiKey: '${API_KEY}', }",
        ),
        (
            "config/mod-client.yaml",
            "# password: documentation example\napi_key:\npasswordRequired: true\nauthorizationMode: offline\n",
        ),
        (
            "config/mod-client.properties",
            "# authorization=Bearer documentation\napi_key=YOUR_API_KEY_HERE\nrefresh_token=${REFRESH_TOKEN}\npassword=***\n",
        ),
        (
            "scripts/recipes.js",
            "// const client_secret = 'documentation only';\n/* accessToken = 'documentation only'; */\nconst passwordRequired = true;\nconst client_secret = process.env.CLIENT_SECRET;\nconst token = config.accessToken;\nconst item = 'example:password_token';\n",
        ),
    ];
    for (relative, text) in safe {
        fixture.put(relative, text);
        assert_eq!(
            group_for(relative).map(|(group, _)| group),
            Some(if relative.starts_with("scripts/") {
                "packData"
            } else {
                "config"
            })
        );
        reject_credentials(&fixture.0.join(relative), relative)
            .unwrap_or_else(|error| panic!("{relative}: {error}"));
    }
    let cancel = CancellationToken::new();
    for format in [ExportFormat::Mrpack, ExportFormat::Portable] {
        let name = if format == ExportFormat::Mrpack {
            "benign-standard"
        } else {
            "benign-portable-inner"
        };
        let workspace = fixture.0.join(name);
        std::fs::create_dir(&workspace).unwrap();
        let request = fixture.request(
            format,
            vec!["@dir/config/config".into(), "@dir/packData/scripts".into()],
        );
        let snapshot = snapshot_instance(
            fixture.instance(),
            &request,
            &workspace,
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        assert_eq!(snapshot.files.len(), safe.len());
        let archive = workspace.join("payload.mrpack");
        write_snapshot_payload(
            &snapshot,
            &archive,
            manifest(&snapshot.preview, &config(&request), &[]).unwrap(),
            &HashSet::new(),
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        assert_payload_bytes(&fixture, &snapshot, &archive, &workspace.join("imported"));
        fixture.record_archive(name, &snapshot, &request, &archive, None);
    }
}

#[test]
fn explicitly_selected_world_preserves_inventory_statistics_and_mod_data() {
    let fixture = Fixture::new();
    let world = "saves/Secret Workshop";
    let expected = [
        "level.dat",
        "level.dat_old",
        "region/r.0.0.mca",
        "DIM-1/region/r.0.0.mca",
        "entities/r.0.0.mca",
        "poi/r.0.0.mca",
        "playerdata/player.dat",
        "playerdata/player.dat_old",
        "stats/player.json",
        "advancements/player.json",
        "serverconfig/mod.toml",
        "data/cache/mod-state.dat",
        "data/exported/mod-state.dat",
        "datapacks/required/data/example/recipe.json",
    ];
    for relative in expected {
        fixture.put(
            &format!("{world}/{relative}"),
            if relative.ends_with(".json") {
                b"{\"progress\":42}".as_slice()
            } else if relative.ends_with(".toml") {
                b"enabled = true\n".as_slice()
            } else {
                b"world-owned\0\xff".as_slice()
            },
        );
    }
    fixture.put("saves/Other/playerdata/player.dat", b"other player");
    fixture.put("saves/Other/stats/player.json", b"{}");
    let instance = fixture.instance();
    assert_eq!(instance.preview.files.len(), 2);
    assert!(
        instance
            .preview
            .files
            .iter()
            .all(|entry| entry.group == "saves" && !entry.selected)
    );
    let defaults = content::selected_files(
        &fixture.0,
        &instance.preview.files,
        &[],
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(defaults.is_empty());
    let cancel = CancellationToken::new();
    for format in [ExportFormat::Mrpack, ExportFormat::Portable] {
        let name = if format == ExportFormat::Mrpack {
            "whole-world-standard"
        } else {
            "whole-world-portable-inner"
        };
        let workspace = fixture.0.join(name);
        std::fs::create_dir(&workspace).unwrap();
        let request = fixture.request(format, vec![format!("@dir/saves/{world}")]);
        let snapshot = snapshot_instance(
            fixture.instance(),
            &request,
            &workspace,
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        assert_eq!(snapshot.files.len(), expected.len());
        assert!(
            snapshot
                .files
                .iter()
                .all(|path| path.starts_with(&format!("{world}/")))
        );
        for relative in expected {
            assert!(snapshot.files.contains(&format!("{world}/{relative}")));
        }
        let archive = workspace.join("payload.mrpack");
        write_snapshot_payload(
            &snapshot,
            &archive,
            manifest(&snapshot.preview, &config(&request), &[]).unwrap(),
            &HashSet::new(),
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        assert_payload_bytes(&fixture, &snapshot, &archive, &workspace.join("imported"));
        fixture.record_archive(name, &snapshot, &request, &archive, None);
    }
}

#[cfg(windows)]
#[test]
#[ignore = "Real Wry portable writer; explicitly run single-threaded without opening a window"]
fn portable_zip_wraps_the_real_thousand_file_snapshot_and_preserves_world_bytes() {
    let fixture = thousand_file_fixture();
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().identifier = format!(
        "com.nickel.export-regression.t{}",
        crate::store::random_token()
    );
    let app = tauri::Builder::default()
        .any_thread()
        .build(context)
        .unwrap();
    let workspace = fixture.0.join("portable-stage");
    std::fs::create_dir(&workspace).unwrap();
    let request = fixture.request(ExportFormat::Portable, bulk_selections());
    let cancel = CancellationToken::new();
    let snapshot = snapshot_instance(
        fixture.instance(),
        &request,
        &workspace,
        &cancel,
        |_, total, _| {
            assert_eq!(total, 4805);
        },
    )
    .unwrap();
    fixture.put(
        "preview.json",
        serde_json::to_vec(&snapshot.preview).unwrap(),
    );
    let portable =
        crate::portable_pack::prepare_snapshot(app.handle(), &request, &workspace, &cancel)
            .unwrap();
    let payload = portable.payload_path();
    write_snapshot_payload(
        &snapshot,
        &payload,
        manifest(&snapshot.preview, &config(&request), &[]).unwrap(),
        &HashSet::new(),
        &cancel,
        |_, _, _| {},
    )
    .unwrap();
    let archive = workspace.join("portable.zip");
    let mut written = 0;
    portable
        .write(&archive, &cancel, |done, total, _| {
            written = done;
            assert_eq!(total, 3);
        })
        .unwrap();
    assert_eq!(written, 3);
    let marker = crate::portable_pack::MANIFEST_FILE;
    let manifest_bytes = read_zip_entry(&archive, marker, 64 * 1024, &cancel).unwrap();
    let manifest: crate::portable_pack::PortableManifest =
        serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest, portable.manifest);
    assert!(manifest.launcher.is_none() && manifest.personalization.is_none());
    let payload_bytes = read_zip_entry(
        &archive,
        &manifest.payload,
        ArchiveLimits::default().max_entry_bytes,
        &cancel,
    )
    .unwrap();
    assert_eq!(payload_bytes, std::fs::read(&payload).unwrap());
    assert!(read_zip_entry(&archive, "modrinth.index.json", 64 * 1024, &cancel).is_err());
    let relocated = workspace.join("relocated");
    std::fs::create_dir(&relocated).unwrap();
    std::fs::create_dir_all(relocated.join(&manifest.data_directory)).unwrap();
    std::fs::write(relocated.join(marker), &manifest_bytes).unwrap();
    std::fs::write(relocated.join(&manifest.payload), &payload_bytes).unwrap();
    let bootstrap = crate::portable_pack::load_bootstrap(&relocated)
        .unwrap()
        .unwrap();
    assert_eq!(bootstrap.manifest, manifest);
    let relocated_root = std::fs::canonicalize(&relocated).unwrap();
    assert_eq!(bootstrap.root, relocated_root);
    assert_eq!(
        bootstrap.game_directory,
        relocated_root.join(&manifest.game_directory)
    );
    assert_eq!(
        bootstrap.data_directory,
        relocated_root.join(&manifest.data_directory)
    );
    assert_eq!(
        bootstrap.payload_path,
        relocated_root.join(&manifest.payload)
    );
    assert_payload_bytes(
        &fixture,
        &snapshot,
        &bootstrap.payload_path,
        &workspace.join("imported"),
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let cancelled_zip = workspace.join("cancelled.zip");
    assert!(
        portable
            .write(&cancelled_zip, &cancelled, |_, _, _| {})
            .is_err()
    );
    assert!(!cancelled_zip.exists());
    fixture.record_archive(
        "portable-outer",
        &snapshot,
        &request,
        &archive,
        Some(&manifest.payload),
    );
}

#[test]
fn portable_comment_syntax_is_not_a_path_but_local_values_still_reject() {
    let fixture = Fixture::new();
    let safe = [
        (
            "config/commented.json5",
            "//description without whitespace after the marker\n{ quality: 'high' }",
        ),
        (
            "scripts/recipes.js",
            "const quality = 'high'; //ordinary comment\n/*ordinary block comment*/\nconst resource = 'example:textures/item.png';",
        ),
        (
            "scripts/recipes.lua",
            "-- ordinary comment\nlocal quality = 'high'\n",
        ),
        (
            "config/client.properties",
            "url=https://example.com/files?cache=/tmp/sample\nquality=high\n",
        ),
    ];
    for (relative, text) in safe {
        fixture.put(relative, text);
        reject_credentials(&fixture.0.join(relative), relative).unwrap();
        reject_portable_host_data(&fixture.0.join(relative), relative, &[])
            .unwrap_or_else(|error| panic!("{relative}: {error}"));
    }
    let unsafe_values = [
        ("config/client.properties", "folder=//server/share/game\n"),
        ("config/client.properties", "folder = //server/share/game\n"),
        (
            "config/client.properties",
            "folder=\\\\server\\share\\game\n",
        ),
        (
            "scripts/host.js",
            "const folder = '//server/share/game'; // ordinary comment\n",
        ),
        (
            "scripts/host.js",
            "const folder = 'C:/Users/private/game';\n",
        ),
        (
            "scripts/host.js",
            "// a host path must not leak even in comments: C:/Users/private/game\n",
        ),
        (
            "scripts/host.js",
            "/* a host path must not leak: /home/private/game */\n",
        ),
        ("config/client.json5", "{ folder: '/custom-root/game' }"),
        (
            "config/client.properties",
            "folder=file:///opt/jdk/bin/java\n",
        ),
        (
            "scripts/host.js",
            r#"const clean = /"/g; const folder = 'C:/Users/private/game';"#,
        ),
        (
            "scripts/host.js",
            "const clean = /[\"/]/g; // local path: /home/private/game\n",
        ),
        (
            "scripts/host.js",
            r#"const clean = /\\/g; const folder = '\\\\server\\share\\game';"#,
        ),
        (
            "scripts/host.js",
            r#"const clean = /"/g; const pre_launch_command = 'echo start';"#,
        ),
    ];
    for (relative, text) in unsafe_values {
        fixture.put(relative, text);
        assert!(
            reject_portable_host_data(&fixture.0.join(relative), relative, &[]).is_err(),
            "accepted {text}"
        );
        reject_credentials(&fixture.0.join(relative), relative).unwrap();
    }
}
