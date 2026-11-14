//! Forge/NeoForge 的受管安装处理器：依赖和映射由 Rust 下载，Java 只执行本地转换。
//! 调用方先准备 install_profile 的 libraries / data，再运行客户端处理器；
//! 取消必须终止子进程，失败不能把提前写出的版本清单当作完成标记。
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::Sender;

use crate::error::LoaderError;
use crate::launcher::events::LaunchEvent;
use crate::models::loader::{ForgeProfile, LoaderType};
use crate::utils::archive::{get_file_from_archive, ArchiveQueryResult};
use crate::utils::paths::get_path_libraries;

// ── Config ────────────────────────────────────────────────────────────────────

/// Runtime paths needed to resolve processor arguments.
///
/// Mirrors the `Config` interface from patcher.ts:
/// `{ java, minecraft, minecraftJson }` + the base `options.path`.
pub struct PatchConfig<'a> {
    /// Path to the Java binary used to run processors.
    pub java_path: &'a str,
    /// Absolute path to the vanilla Minecraft client JAR (`{MINECRAFT_JAR}`).
    pub minecraft_jar: &'a str,
    /// Absolute path to the vanilla Minecraft version JSON (`{MINECRAFT_VERSION}`).
    pub minecraft_json: &'a str,
    /// Main game data directory — used for `{ROOT}`, `{INSTALLER}`, `{LIBRARY_DIR}`.
    /// In the TS this is `options.path`; in Rust pass `options.path`.
    pub game_path: &'a Path,
}

// ── Patcher ───────────────────────────────────────────────────────────────────

/// Runs Forge/NeoForge install processors one by one.
///
/// Equivalent to the TypeScript `ForgePatcher` class.
pub struct ForgePatcher {
    /// Directory where the loader installed its libraries
    /// (e.g. `<options.path>/loader/forge/`).
    pub loader_base: PathBuf,
    /// Loader variant — used to locate the "universal" library in the profile.
    pub loader_type: LoaderType,
}

impl ForgePatcher {
    pub fn new(loader_base: PathBuf, loader_type: LoaderType) -> Self {
        Self {
            loader_base,
            loader_type,
        }
    }

    /// Check whether patching has already been applied.
    ///
    /// Iterates every client-side processor's arguments, collects the output
    /// file paths referenced via `profile.data`, and verifies each exists on
    /// disk. Returns `true` when all files are present (skip patching).
    ///
    /// Mirrors `ForgePatcher.check()` from patcher.ts.
    pub fn check(&self, profile: &ForgeProfile) -> bool {
        let processors: Vec<_> = profile.processors.as_deref().unwrap_or(&[]).iter()
            .filter(|processor| is_client_side(processor.sides.as_deref())).collect();
        if processors.is_empty() { return true; }
        let Some(data) = &profile.data else { return false; };
        let mut tracked = false;
        for processor in processors {
            for argument in &processor.args {
                let Some(entry) = data.get(strip_braces(argument)) else { continue; };
                let Some(coordinate) = entry.client.strip_prefix('[').and_then(|value| value.strip_suffix(']')) else { continue; };
                let Ok(info) = get_path_libraries(coordinate, None, None) else { return false; };
                let path = self.loader_base.join("libraries").join(info.path).join(info.name);
                if !path.is_file() { return false; }
                tracked = true;
            }
        }
        // 只有明确追踪到文件时才能跳过，不能把缺少 data 当作已经完成。
        tracked
    }

    /// Run all client-side processors from the install profile sequentially.
    ///
    /// For each processor:
    /// 1. Resolve arguments via `set_argument` + `compute_path`.
    /// 2. Build a classpath from the processor JAR and its declared dependencies.
    /// 3. Read `Main-Class` from `META-INF/MANIFEST.MF` inside the processor JAR.
    /// 4. Spawn `java -classpath <cp> <MainClass> <args>`.
    /// 5. Stream stdout/stderr as `LaunchEvent::Patch` events.
    ///
    /// Mirrors `ForgePatcher.patcher()` from patcher.ts.
    pub async fn patch(
        &self,
        profile: &ForgeProfile,
        config: &PatchConfig<'_>,
        _neo_forge_old: bool,
        event_tx: &Sender<LaunchEvent>,
    ) -> Result<(), LoaderError> {
        let processors = profile.processors.as_ref().ok_or(LoaderError::ProfileNotFound)?;
        let context = crate::net::transfer::TransferContext::current();
        for processor in processors {
            if !is_client_side(processor.sides.as_deref()) { continue; }
            context.cancellation().check().map_err(LoaderError::ApiError)?;
            let jar_info = get_path_libraries(&processor.jar, None, None)
                .map_err(|error| LoaderError::ApiError(format!("处理器坐标无效：{error}")))?;
            let jar_path = self.loader_base.join("libraries").join(&jar_info.path).join(&jar_info.name);
            let args: Vec<String> = processor.args.iter()
                .map(|argument| self.resolve_argument(argument, profile, config)).collect();
            for argument in &args {
                let path = Path::new(argument);
                if path.starts_with(self.loader_base.join("libraries")) {
                    if let Some(parent) = path.parent() { tokio::fs::create_dir_all(parent).await?; }
                }
            }
            let mut cp_entries = vec![jar_path.to_string_lossy().into_owned()];
            for coordinate in &processor.classpath {
                let info = get_path_libraries(coordinate, None, None)
                    .map_err(|error| LoaderError::ApiError(format!("处理器依赖坐标无效：{error}")))?;
                cp_entries.push(self.loader_base.join("libraries").join(info.path).join(info.name).to_string_lossy().into_owned());
            }
            let main_class = read_jar_manifest(&jar_path).await?
                .ok_or_else(|| LoaderError::Archive(format!("处理器缺少 Main-Class：{}", jar_path.display())))?;
            let label = processor.jar.clone();
            let _ = event_tx.try_send(LaunchEvent::Patch(format!("[patcher] Running: {label}")));
            let mut child = tokio::process::Command::new(config.java_path)
                .arg("-classpath").arg(cp_entries.join(cp_separator())).arg(main_class).args(&args)
                .current_dir(&self.loader_base)
                .stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true)
                .spawn().map_err(LoaderError::Io)?;
            if let Some(stdout) = child.stdout.take() {
                let tx = event_tx.clone();
                let mut lines = BufReader::new(stdout).lines();
                tokio::spawn(async move {
                    while let Ok(Some(line)) = lines.next_line().await { let _ = tx.try_send(LaunchEvent::Patch(line)); }
                });
            }
            if let Some(stderr) = child.stderr.take() {
                let tx = event_tx.clone();
                let mut lines = BufReader::new(stderr).lines();
                tokio::spawn(async move {
                    while let Ok(Some(line)) = lines.next_line().await { let _ = tx.try_send(LaunchEvent::Patch(line)); }
                });
            }
            let status = crate::net::transfer::wait_child(&mut child, context.cancellation()).await.map_err(LoaderError::ApiError)?;
            if !status.success() { return Err(LoaderError::ProcessorFailed { processor: label, code: status.code() }); }
        }
        Ok(())
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// data 的单引号表示字面量，斜线表示安装器内部条目，方括号表示 Maven 坐标。
    /// 替换嵌入参数中的占位符，不能把版本 ID 误替换成版本 JSON 路径。
    fn set_argument(
        &self,
        arg: &str,
        profile: &ForgeProfile,
        config: &PatchConfig<'_>,
        _neo_forge_old: bool,
    ) -> String {
        let libs_dir = self.loader_base.join("libraries");
        let mut result = arg.to_owned();
        if let Some(data) = &profile.data {
            for (key, entry) in data {
                let value = &entry.client;
                let replacement = if let Some(literal) = value.strip_prefix('\'').and_then(|value| value.strip_suffix('\'')) {
                    literal.to_owned()
                } else if let Some(relative) = value.strip_prefix('/') {
                    libs_dir.join(relative).to_string_lossy().into_owned()
                } else {
                    value.clone()
                };
                result = result.replace(&format!("{{{key}}}"), &replacement);
            }
        }
        let version = Path::new(config.minecraft_json).file_stem().and_then(|name| name.to_str()).unwrap_or_default();
        result.replace("{SIDE}", "client")
            .replace("{ROOT}", &config.game_path.to_string_lossy())
            .replace("{MINECRAFT_JAR}", config.minecraft_jar)
            .replace("{MINECRAFT_VERSION}", version)
            .replace("{INSTALLER}", &libs_dir.to_string_lossy())
            .replace("{LIBRARY_DIR}", &libs_dir.to_string_lossy())
    }

    pub(crate) fn resolve_argument(&self, argument: &str, profile: &ForgeProfile, config: &PatchConfig<'_>) -> String {
        self.compute_path(&self.set_argument(argument, profile, config, false))
    }

    /// Convert a `[group:artifact:version]` reference to an absolute path.
    ///
    /// Arguments that don't start with `[` are returned unchanged.
    ///
    /// Mirrors `computePath()` from patcher.ts.
    fn compute_path(&self, arg: &str) -> String {
        if arg.starts_with('[') {
            let coord = arg.trim_matches(|c| c == '[' || c == ']');
            if let Ok(info) = get_path_libraries(coord, None, None) {
                return self
                    .loader_base
                    .join("libraries")
                    .join(&info.path)
                    .join(&info.name)
                    .to_string_lossy()
                    .into_owned();
            }
        }
        arg.to_owned()
    }
}

// ── Module-level helpers ──────────────────────────────────────────────────────

/// Returns `true` if the processor should run on the client side.
///
/// A processor with no `sides` field runs on both; one that lists sides must
/// include `"client"` to be selected.
pub(crate) fn is_client_side(sides: Option<&[String]>) -> bool {
    match sides {
        None => true,
        Some(sides) => sides.iter().any(|side| side == "client"),
    }
}

/// Strip outer `{` / `}` from a placeholder token.
///
/// `{BINPATCH}` → `BINPATCH`. Non-placeholder args pass through unchanged.
///
/// Uses `trim_start_matches`/`trim_end_matches` (mirrors the TS single-char
/// `replace('{', '').replace('}', '')` for the common single-placeholder case).
fn strip_braces(s: &str) -> &str {
    s.trim_start_matches('{').trim_end_matches('}')
}

/// Read the `Main-Class` value from a JAR's `META-INF/MANIFEST.MF`.
///
/// Handles both CRLF and LF line endings (fixes the latent TS bug where
/// `split('\r\n')` silently included `\n` on Unix-created manifests).
///
/// Mirrors `readJarManifest()` from patcher.ts.
async fn read_jar_manifest(jar_path: &Path) -> Result<Option<String>, LoaderError> {
    let result = get_file_from_archive(
        jar_path.to_path_buf(),
        Some("META-INF/MANIFEST.MF".into()),
        None,
        false,
    )
    .await
    .map_err(|e| LoaderError::Archive(e.to_string()))?;

    let bytes = match result {
        ArchiveQueryResult::FileData(b) => b,
        _ => return Ok(None),
    };

    // str::lines() handles \r\n, \r, and \n — no CRLF/LF ambiguity.
    let content = String::from_utf8_lossy(&bytes);
    for line in content.lines() {
        if let Some(class) = line.strip_prefix("Main-Class: ") {
            return Ok(Some(class.trim().to_owned()));
        }
    }

    Ok(None)
}

/// Classpath separator: `;` on Windows, `:` everywhere else.
fn cp_separator() -> &'static str {
    if cfg!(target_os = "windows") {
        ";"
    } else {
        ":"
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Write;
    use tempfile::NamedTempFile;

    use crate::models::loader::{ForgeProcessor, ProfileDataEntry};

    fn make_patcher() -> ForgePatcher {
        ForgePatcher {
            loader_base: PathBuf::from("/mc/loader/forge"),
            loader_type: LoaderType::Forge,
        }
    }

    fn make_config<'a>(game_path: &'a Path) -> PatchConfig<'a> {
        PatchConfig {
            java_path: "/usr/bin/java",
            minecraft_jar: "/mc/versions/1.20.1/1.20.1.jar",
            minecraft_json: "/mc/versions/1.20.1/1.20.1.json",
            game_path,
        }
    }

    #[test]
    fn installer_literals_embedded_paths_and_version_keep_profile_semantics() {
        let patcher = make_patcher();
        let game = PathBuf::from("/mc");
        let config = make_config(&game);
        let profile: ForgeProfile = serde_json::from_value(serde_json::json!({"data": {
            "BINPATCH": {"client": "/data/client.lzma"},
            "MCP_VERSION": {"client": "'20230612.114412'"},
            "INSTALLER": {"client": "'/downloads/forge installer.jar'"}
        }})).unwrap();
        assert_eq!(patcher.resolve_argument("{MINECRAFT_VERSION}", &profile, &config), "1.20.1");
        assert_eq!(patcher.resolve_argument("--version={MCP_VERSION}", &profile, &config), "--version=20230612.114412");
        assert_eq!(patcher.resolve_argument("{INSTALLER}", &profile, &config), "/downloads/forge installer.jar");
        assert_eq!(Path::new(&patcher.resolve_argument("{BINPATCH}", &profile, &config)), patcher.loader_base.join("libraries/data/client.lzma"));
    }

    #[test]
    fn processors_without_tracked_data_must_not_be_skipped() {
        let profile: ForgeProfile = serde_json::from_value(serde_json::json!({"processors": [
            {"jar":"net.minecraftforge:installertools:1.4.1", "classpath":[], "args":["--task","MCP_DATA"]}
        ]})).unwrap();
        assert!(!make_patcher().check(&profile));
    }

    // ── strip_braces ──────────────────────────────────────────────────────────

    #[test]
    fn strip_braces_removes_outer() {
        assert_eq!(strip_braces("{BINPATCH}"), "BINPATCH");
        assert_eq!(strip_braces("{SIDE}"), "SIDE");
        assert_eq!(strip_braces("--no-braces"), "--no-braces");
    }

    // ── is_client_side ────────────────────────────────────────────────────────

    #[test]
    fn no_sides_means_client() {
        assert!(is_client_side(None));
    }

    #[test]
    fn client_in_sides_passes() {
        let sides = vec!["client".to_owned()];
        assert!(is_client_side(Some(&sides)));
    }

    #[test]
    fn server_only_filtered() {
        let sides = vec!["server".to_owned()];
        assert!(!is_client_side(Some(&sides)));
    }

    // ── compute_path ──────────────────────────────────────────────────────────

    #[test]
    fn compute_path_plain_arg_unchanged() {
        let p = make_patcher();
        assert_eq!(p.compute_path("--some-flag"), "--some-flag");
    }

    #[test]
    fn compute_path_bracket_coord_resolved() {
        let p = make_patcher();
        let result = p.compute_path("[net.minecraftforge:forge:1.20.1-47.4.20]");
        assert!(result.contains("net/minecraftforge/forge"));
        assert!(result.contains("forge-1.20.1-47.4.20.jar"));
        assert!(Path::new(&result).starts_with(Path::new("/mc/loader/forge").join("libraries")));
    }

    // ── set_argument ──────────────────────────────────────────────────────────

    #[test]
    fn set_argument_resolves_data_entry() {
        let p = make_patcher();
        let game = PathBuf::from("/mc");

        let mut data = HashMap::new();
        data.insert(
            "MAPPINGS".to_owned(),
            ProfileDataEntry {
                client: "[net.minecraftforge:forge:1.20.1-47.4.20:client-mappings@txt]".to_owned(),
                server: None,
            },
        );
        let profile = ForgeProfile {
            data: Some(data),
            ..Default::default()
        };
        let config = make_config(&game);
        let result = p.set_argument("{MAPPINGS}", &profile, &config, true);
        assert_eq!(
            result,
            "[net.minecraftforge:forge:1.20.1-47.4.20:client-mappings@txt]"
        );
    }

    #[test]
    fn set_argument_fixed_side_placeholder() {
        let p = make_patcher();
        let game = PathBuf::from("/mc");
        let config = make_config(&game);
        let result = p.set_argument("{SIDE}", &ForgeProfile::default(), &config, true);
        assert_eq!(result, "client");
    }

    #[test]
    fn set_argument_minecraft_jar_placeholder() {
        let p = make_patcher();
        let game = PathBuf::from("/mc");
        let config = make_config(&game);
        let result = p.set_argument("{MINECRAFT_JAR}", &ForgeProfile::default(), &config, true);
        assert_eq!(result, "/mc/versions/1.20.1/1.20.1.jar");
    }

    #[test]
    fn set_argument_library_dir_placeholder() {
        let p = make_patcher();
        let game = PathBuf::from("/mc");
        let config = make_config(&game);
        let result = p.set_argument("{LIBRARY_DIR}", &ForgeProfile::default(), &config, true);
        assert_eq!(Path::new(&result), Path::new("/mc/loader/forge").join("libraries"));
    }

    // ── check ─────────────────────────────────────────────────────────────────

    #[test]
    fn check_returns_true_when_no_processors() {
        let p = make_patcher();
        let profile = ForgeProfile {
            processors: None,
            ..Default::default()
        };
        assert!(p.check(&profile));
    }

    #[test]
    fn check_returns_true_when_processors_empty() {
        let p = make_patcher();
        let profile = ForgeProfile {
            processors: Some(vec![]),
            ..Default::default()
        };
        assert!(p.check(&profile));
    }

    #[test]
    fn check_returns_false_when_output_file_missing() {
        let p = make_patcher();
        let mut data = HashMap::new();
        data.insert(
            "MC_SLIM".to_owned(),
            ProfileDataEntry {
                client: "[net.minecraftforge:forge:1.20.1-47.4.20:slim]".to_owned(),
                server: None,
            },
        );
        let profile = ForgeProfile {
            data: Some(data),
            processors: Some(vec![ForgeProcessor {
                jar: "cpw.mods:jarsplitter:1.1.4".to_owned(),
                classpath: vec![],
                args: vec!["{MC_SLIM}".to_owned()],
                sides: None,
            }]),
            ..Default::default()
        };
        // File doesn't exist on disk → check returns false.
        assert!(!p.check(&profile));
    }

    #[test]
    fn check_skips_server_side_processors() {
        let p = make_patcher();
        let mut data = HashMap::new();
        data.insert(
            "SERVER_EXTRA".to_owned(),
            ProfileDataEntry {
                client: "[some:artifact:1.0]".to_owned(),
                server: None,
            },
        );
        let profile = ForgeProfile {
            data: Some(data),
            processors: Some(vec![ForgeProcessor {
                jar: "some:tool:1.0".to_owned(),
                classpath: vec![],
                args: vec!["{SERVER_EXTRA}".to_owned()],
                // Server-only processor — skipped.
                sides: Some(vec!["server".to_owned()]),
            }]),
            ..Default::default()
        };
        // No client-side processors with tracked output → check returns true.
        assert!(p.check(&profile));
    }

    // ── read_jar_manifest ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn read_jar_manifest_finds_main_class() {
        use zip::write::SimpleFileOptions;

        let mut tmp = NamedTempFile::new().unwrap();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            let opts = SimpleFileOptions::default();
            w.start_file("META-INF/MANIFEST.MF", opts).unwrap();
            w.write_all(b"Manifest-Version: 1.0\r\nMain-Class: com.example.Main\r\n")
                .unwrap();
            let data = w.finish().unwrap();
            tmp.write_all(data.get_ref()).unwrap();
        }

        let result = read_jar_manifest(tmp.path()).await.unwrap();
        assert_eq!(result, Some("com.example.Main".to_owned()));
    }

    #[tokio::test]
    async fn read_jar_manifest_lf_line_endings() {
        use zip::write::SimpleFileOptions;

        let mut tmp = NamedTempFile::new().unwrap();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            let opts = SimpleFileOptions::default();
            w.start_file("META-INF/MANIFEST.MF", opts).unwrap();
            // LF only (the bug in TS) — Rust handles fine with str::lines().
            w.write_all(b"Manifest-Version: 1.0\nMain-Class: com.example.Main\n")
                .unwrap();
            let data = w.finish().unwrap();
            tmp.write_all(data.get_ref()).unwrap();
        }

        let result = read_jar_manifest(tmp.path()).await.unwrap();
        assert_eq!(result, Some("com.example.Main".to_owned()));
    }

    #[tokio::test]
    async fn read_jar_manifest_returns_none_when_no_main_class() {
        use zip::write::SimpleFileOptions;

        let mut tmp = NamedTempFile::new().unwrap();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            let opts = SimpleFileOptions::default();
            w.start_file("META-INF/MANIFEST.MF", opts).unwrap();
            w.write_all(b"Manifest-Version: 1.0\r\n").unwrap();
            let data = w.finish().unwrap();
            tmp.write_all(data.get_ref()).unwrap();
        }

        let result = read_jar_manifest(tmp.path()).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn read_jar_manifest_returns_none_when_no_manifest() {
        use zip::write::SimpleFileOptions;

        let mut tmp = NamedTempFile::new().unwrap();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            let opts = SimpleFileOptions::default();
            w.start_file("some/other/file.txt", opts).unwrap();
            w.write_all(b"hello").unwrap();
            let data = w.finish().unwrap();
            tmp.write_all(data.get_ref()).unwrap();
        }

        let result = read_jar_manifest(tmp.path()).await.unwrap();
        assert_eq!(result, None);
    }
}
