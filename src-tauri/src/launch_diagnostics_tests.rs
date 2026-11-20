//! 诊断与日志存储的独立测试。
//!
//! 本文件既可以由 `lib.rs` 的 `#[cfg(test)] mod launch_diagnostics_tests;`
//! 引入，也可以由 `tests/launch_diagnostics_standalone.rs` 在模块尚未接入
//! 生产入口时独立编译，保证测试不会因接线顺序而失效。

use crate::launch_diagnostics::{
    DiagnosticCategory, DiagnosticInput, DiagnosticReport, DiagnosticSeverity, DiagnosticSource,
    MAX_CRASH_REPORT_BYTES, MAX_LINES_PER_SOURCE, analyze,
};
use crate::launch_log_store::{LaunchLogStore, redact_sensitive_text, resolve_log_paths};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "nickel-diagnostics-{tag}-{}-{stamp}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| {
            panic!("创建测试目录失败 {}：{error}", path.display());
        });
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn base_input() -> DiagnosticInput {
    DiagnosticInput::new(Some(1), "1.20.1", "forge")
}

fn analyze_category(input: &DiagnosticInput) -> (DiagnosticReport, DiagnosticCategory) {
    let report = analyze(input);
    (report.clone(), report.category)
}

#[test]
fn classifies_out_of_memory_and_redacts_evidence() {
    let input = base_input().with_process_output(
        "",
        "java.lang.OutOfMemoryError: Java heap space accessToken=abcdef1234567890",
    );
    let (report, category) = analyze_category(&input);

    assert_eq!(category, DiagnosticCategory::OutOfMemory);
    assert_eq!(report.severity, DiagnosticSeverity::Critical);
    assert!(report.summary.contains("内存"));
    assert!(report.advice.contains("内存"));

    let evidence = report.evidence.first().expect("应包含证据");
    assert_eq!(evidence.source, DiagnosticSource::Stderr);
    assert_eq!(evidence.line, 1);
    assert!(evidence.excerpt.contains("OutOfMemoryError"));
    assert!(!evidence.excerpt.contains("abcdef1234567890"));
    assert_eq!(report.context.exit_code, Some(1));
}

#[test]
fn classifies_unrecognized_jvm_option() {
    let input = base_input().with_process_output("", "Unrecognized option: -XX:+UseG1GC");
    let (report, category) = analyze_category(&input);

    assert_eq!(category, DiagnosticCategory::JvmArgumentError);
    assert_eq!(report.severity, DiagnosticSeverity::Critical);
    assert!(report.advice.contains("JVM"));
}

#[test]
fn classifies_unsupported_class_file_version() {
    let input = base_input().with_latest_log(
        "java.lang.UnsupportedClassVersionError: example/Mod has been compiled by a more recent version of the Java Runtime (class file version 61.0)",
    );
    let (report, category) = analyze_category(&input);

    assert_eq!(category, DiagnosticCategory::JavaVersionIncompatible);
    assert!(report.summary.contains("Java"));
    assert!(report.advice.contains("Java"));
}

#[test]
fn classifies_forge_java_incompatibility() {
    let input = base_input().with_process_output(
        "",
        "java.lang.NoSuchMethodError: sun.security.util.ManifestEntryVerifier",
    );
    let (report, category) = analyze_category(&input);

    assert_eq!(category, DiagnosticCategory::ForgeJavaIncompatible);
    assert!(report.advice.contains("Forge"));
}

#[test]
fn classifies_mixin_failure() {
    let input = base_input().with_latest_log(
        "Mixin apply failed example.mixins.json:example.mixin.ExampleMixin from mod examplemod",
    );
    let (report, category) = analyze_category(&input);

    assert_eq!(category, DiagnosticCategory::MixinFailure);
    assert!(report.summary.contains("Mixin"));
}

#[test]
fn classifies_optifine_forge_incompatibility() {
    let input = base_input()
        .with_optifine("HD U H9")
        .with_latest_log(
            "java.lang.NoSuchMethodError: 'void net.minecraft.client.renderer.texture.SpriteContents.<init>(...)'\nnet.minecraftforge event bus",
        );
    let (report, category) = analyze_category(&input);

    assert_eq!(category, DiagnosticCategory::OptiFineForgeIncompatible);
    assert!(report.advice.contains("OptiFine"));
}

#[test]
fn classifies_missing_mod_dependency() {
    let input = base_input().with_latest_log(
        "Missing or unsupported mandatory dependencies:\n\tmodid:create requires any version of flywheel",
    );
    let (report, category) = analyze_category(&input);

    assert_eq!(category, DiagnosticCategory::MissingModDependency);
    assert!(report.summary.contains("前置"));
    assert!(!report.evidence.is_empty());
}

#[test]
fn classifies_mod_conflict_and_initialization_failure() {
    let conflict = base_input().with_latest_log("Incompatible mods found!");
    assert_eq!(analyze(&conflict).category, DiagnosticCategory::ModConflict);

    let init = base_input().with_latest_log("Failed to create mod instance. ModID: example");
    assert_eq!(
        analyze(&init).category,
        DiagnosticCategory::ModInitializationFailure
    );
}

#[test]
fn classifies_duplicate_mod() {
    let input = base_input().with_latest_log("Found a duplicate mod example.jar");
    let (_, category) = analyze_category(&input);

    assert_eq!(category, DiagnosticCategory::DuplicateMod);
}

#[test]
fn classifies_forge_install_incomplete() {
    let input = base_input().with_latest_log(
        "Cannot find launch target fmlclient, unable to launch\nFound multiple arguments for option fml.forgeVersion",
    );
    let (_, category) = analyze_category(&input);

    assert_eq!(category, DiagnosticCategory::ForgeInstallIncomplete);
}

#[test]
fn classifies_graphics_driver_access_violation() {
    let input = base_input().with_crash_report(
        "EXCEPTION_ACCESS_VIOLATION\n# C  [nvoglv64.dll+0x1234]\nThe driver does not appear to support OpenGL",
    );
    let (_, category) = analyze_category(&input);

    assert_eq!(category, DiagnosticCategory::GraphicsDriverIssue);
}

#[test]
fn empty_input_returns_explainable_unknown() {
    let report = analyze(&DiagnosticInput::new(Some(1), "1.20.1", "fabric"));

    assert_eq!(report.category, DiagnosticCategory::Unknown);
    assert_eq!(report.severity, DiagnosticSeverity::Error);
    assert!(report.fallback_reason.is_some());
    assert!(report.summary.contains("没有可分析"));
    assert!(report.evidence.is_empty());
}

#[test]
fn bad_utf8_input_is_lossy_not_panicking() {
    let mut stderr = vec![0xff, 0xfe, 0x00, b'\n'];
    stderr.extend_from_slice(b"java.lang.OutOfMemoryError: Java heap space\n");
    let input = base_input().with_process_output_bytes(&[], &stderr);

    assert!(input.stderr_tail.contains('\u{fffd}'));
    assert_eq!(analyze(&input).category, DiagnosticCategory::OutOfMemory);
}

#[test]
fn truncated_crash_report_bytes_sets_flag() {
    let trailer = b"java.lang.OutOfMemoryError: Java heap space";
    let mut crash_report = vec![b'a'; MAX_CRASH_REPORT_BYTES + 128];
    let start = crash_report.len() - trailer.len();
    crash_report[start..].copy_from_slice(trailer);

    let input = base_input().with_crash_report_bytes(&crash_report);
    assert!(input.input_truncated);

    let report = analyze(&input);
    assert_eq!(report.category, DiagnosticCategory::OutOfMemory);
    assert!(report.input_truncated);
    assert!(report.evidence[0].excerpt.contains("OutOfMemoryError"));
}

#[test]
fn huge_log_is_bounded_and_still_classified() {
    let mut latest_log = String::new();
    for index in 0..(MAX_LINES_PER_SOURCE + 2_000) {
        latest_log.push_str(&format!("line {index}\n"));
    }
    latest_log.push_str("java.lang.OutOfMemoryError: Java heap space\n");

    let report = analyze(&base_input().with_latest_log(latest_log));

    assert_eq!(report.category, DiagnosticCategory::OutOfMemory);
    assert!(report.input_truncated);
    assert!(!report.evidence.is_empty());
    assert!(report.evidence[0].line <= MAX_LINES_PER_SOURCE);
}

#[test]
fn unknown_result_is_stable_and_has_reason() {
    let input = base_input().with_process_output("unrelated stdout", "unrelated stderr");
    let first = analyze(&input);
    let second = analyze(&input);

    assert_eq!(first, second);
    assert_eq!(first.category, DiagnosticCategory::Unknown);
    assert!(first.fallback_reason.is_some());
    assert!(!first.evidence.is_empty());
}

#[test]
fn json_projection_uses_camel_case() {
    let input = base_input().with_process_output("", "java.lang.OutOfMemoryError: Java heap space");
    let value = serde_json::to_value(analyze(&input)).expect("诊断报告应可序列化");

    assert_eq!(value["category"].as_str(), Some("outOfMemory"));
    assert_eq!(value["severity"].as_str(), Some("critical"));
    assert_eq!(value["context"]["exitCode"].as_i64(), Some(1));
    assert!(value.get("fallbackReason").is_some());
    assert!(value.get("inputTruncated").is_some());
    assert_eq!(value["evidence"][0]["source"].as_str(), Some("stderr"));
}

#[test]
fn redaction_covers_tokens_passwords_paths_and_invites() {
    let raw = concat!(
        "accessToken=abcdef1234567890 ",
        "clientToken: client-secret-123 ",
        "password=hunter2\n",
        "--accessToken cli-token-abcdef123456\n",
        "C:\\Users\\alice\\AppData\\Local\n",
        "https://example.com/invite/ABC123XYZ\n",
        "Authorization: Bearer bearer-token-123456\n",
        "https://user:pass@example.com/private\n"
    );
    let redacted = redact_sensitive_text(raw);

    for secret in [
        "abcdef1234567890",
        "client-secret-123",
        "hunter2",
        "cli-token-abcdef123456",
        "alice",
        "ABC123XYZ",
        "bearer-token-123456",
        "pass@example.com",
    ] {
        assert!(!redacted.contains(secret), "未脱敏内容：{secret}");
    }
    assert!(redacted.contains("accessToken="));
    assert!(redacted.contains("clientToken:"));
    assert!(redacted.contains("example.com"));
    assert!(redacted.contains("***"));
}

#[test]
fn redaction_covers_client_secret_key_variants() {
    for key in [
        "clientSecret",
        "client_secret",
        "client-secret",
        "clientsecret",
        "CLIENT_SECRET",
    ] {
        let secret = format!("{key}-value-should-not-leak");
        for raw in [
            format!("{key}={secret}"),
            format!("{{\"{key}\":\"{secret}\"}}"),
        ] {
            let redacted = redact_sensitive_text(&raw);
            assert!(!redacted.contains(&secret), "{key}: {redacted}");
            assert!(redacted.contains(key), "{key}: {redacted}");
        }
    }
}

#[test]
fn redaction_is_idempotent() {
    let once = redact_sensitive_text(
        "accessToken=abcdef1234567890 password=hunter2 C:\\Users\\alice\\AppData",
    );
    let twice = redact_sensitive_text(&once);
    assert_eq!(once, twice);
}

#[test]
fn quoted_redaction_respects_delimiters_escapes_and_line_boundaries() {
    for (raw, expected) in [
        (
            r#"{"password":"probe'secret-tail"}"#,
            r#"{"password":"***"}"#,
        ),
        (
            r#"password='probe"secret-tail' next=visible"#,
            "password='***' next=visible",
        ),
        (
            r#"{"clientSecret":"probe\"secret-tail"}"#,
            r#"{"clientSecret":"***"}"#,
        ),
        (
            r#"{"apiKey":"probe\\secret-tail","next":"visible"}"#,
            r#"{"apiKey":"***","next":"visible"}"#,
        ),
        (r#"password='probe\'secret-tail'"#, "password='***'"),
        (
            r#"password=`probe'"secret-tail` next=visible"#,
            "password=`***` next=visible",
        ),
        (
            "password=\"probe'secret-tail\nnext=visible",
            "password=\"***\nnext=visible",
        ),
        (
            r#"{"password":"probe\\","next":"visible"}"#,
            r#"{"password":"***","next":"visible"}"#,
        ),
    ] {
        let redacted = redact_sensitive_text(raw);
        assert_eq!(redacted, expected, "{raw}");
        assert_eq!(redact_sensitive_text(&redacted), redacted);
    }
}

#[test]
fn windows_user_paths_keep_plain_and_json_separator_widths() {
    for user in ["PrivateReviewName", "测试用户", "User Name"] {
        for prefix in [r"C:\Users\", r"C:\Documents and Settings\", r"c:\uSeRs\"] {
            let raw = format!(r"{prefix}{user}\game");
            let expected = format!(r"{prefix}***\game");
            assert_eq!(redact_sensitive_text(&raw), expected);
            let json = serde_json::json!({ "path": raw, "other": "visible" }).to_string();
            let expected_json = serde_json::json!({ "path": expected, "other": "visible" });
            let redacted = redact_sensitive_text(&json);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&redacted).unwrap(),
                expected_json
            );
            assert_eq!(redact_sensitive_text(&redacted), redacted);
        }
    }
}

#[test]
fn log_store_append_and_read_tail_redacts() {
    let dir = TestDir::new("log-tail");
    let store = LaunchLogStore::new(dir.path());
    store
        .append_lines(&[
            "one".to_string(),
            "accessToken=abcdef1234567890".to_string(),
            "three".to_string(),
        ])
        .expect("追加日志");

    let tail = store.read_tail(2).expect("读取尾部");
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0], "accessToken=abcd***7890");
    assert_eq!(tail[1], "three");

    let on_disk = fs::read_to_string(store.log_path()).expect("读取日志文件");
    assert!(!on_disk.contains("abcdef1234567890"));
    assert_eq!(
        store.directory_to_open().to_path_buf(),
        dir.path().join("logs")
    );
}

#[test]
fn log_store_rotates_when_limit_exceeded() {
    let dir = TestDir::new("log-rotate");
    let store = LaunchLogStore::with_limits(dir.path(), 1024, 2);
    let line = "x".repeat(300);

    for _ in 0..8 {
        store.append_text(&line).expect("追加日志");
    }

    let current_len = fs::metadata(store.log_path())
        .expect("读取当前日志信息")
        .len();
    assert!(current_len <= 1024);
    assert!(store.paths().archive_file(1).exists());
    assert!(store.paths().archive_file(2).exists());
    assert!(!store.read_tail(10).expect("读取尾部").is_empty());
}

#[test]
fn log_store_reads_non_utf8_file() {
    let dir = TestDir::new("log-utf8");
    let paths = resolve_log_paths(dir.path());
    fs::create_dir_all(&paths.directory).expect("创建日志目录");
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend_from_slice(b" OutOfMemoryError\n");
    fs::write(&paths.log_file, bytes).expect("写入非 UTF-8 日志");

    let store = LaunchLogStore::new(dir.path());
    let tail = store.read_tail(10).expect("读取尾部");
    assert!(tail.iter().any(|line| line.contains("OutOfMemoryError")));
    assert!(tail.join("\n").contains('\u{fffd}'));
}

#[test]
fn log_store_returns_error_for_directory_at_log_path() {
    let dir = TestDir::new("log-directory");
    let store = LaunchLogStore::new(dir.path());
    fs::create_dir_all(store.log_path()).expect("创建冲突目录");

    let error = store.append_text("hello").expect_err("目录不能当日志文件");
    assert!(error.contains("日志路径不是普通文件"));
}

#[test]
fn log_store_reads_latest_crash_report_and_paths() {
    let dir = TestDir::new("crash-report");
    let paths = resolve_log_paths(dir.path());
    assert_eq!(
        paths.log_file,
        dir.path().join("logs").join("nickel-launcher.log")
    );
    assert_eq!(
        paths.latest_game_log,
        dir.path().join("logs").join("latest.log")
    );
    assert_eq!(paths.crash_reports, dir.path().join("crash-reports"));

    fs::create_dir_all(&paths.crash_reports).expect("创建崩溃报告目录");
    fs::write(
        paths.crash_reports.join("crash-2026-01-01.txt"),
        "Description: test\njava.lang.OutOfMemoryError\n",
    )
    .expect("写入崩溃报告");

    let store = LaunchLogStore::new(dir.path());
    let latest = store
        .read_latest_crash_report(64 * 1024)
        .expect("读取崩溃报告")
        .expect("应找到崩溃报告");
    assert!(latest.text.contains("OutOfMemoryError"));
    assert!(!latest.truncated);
    assert!(
        store
            .read_latest_game_log(4096)
            .expect("读取游戏日志")
            .is_none()
    );
    assert_eq!(
        store
            .read_crash_reports(5, 64 * 1024)
            .expect("读取崩溃报告列表")
            .len(),
        1
    );
}

#[test]
fn log_store_crash_report_keeps_head_and_tail_when_truncated() {
    let dir = TestDir::new("crash-head-tail");
    let paths = resolve_log_paths(dir.path());
    fs::create_dir_all(&paths.crash_reports).expect("创建崩溃报告目录");

    let mut report = vec![b'a'; 4096];
    let prefix = b"Description: crash";
    report[..prefix.len()].copy_from_slice(prefix);
    let trailer = b"java.lang.OutOfMemoryError";
    let start = report.len() - trailer.len();
    report[start..].copy_from_slice(trailer);
    fs::write(paths.crash_reports.join("crash-large.txt"), report).expect("写入崩溃报告");

    let store = LaunchLogStore::new(dir.path());
    let latest = store
        .read_latest_crash_report(1024)
        .expect("读取崩溃报告")
        .expect("应找到崩溃报告");

    assert!(latest.truncated);
    assert!(latest.text.starts_with("Description: crash"));
    assert!(latest.text.ends_with("java.lang.OutOfMemoryError"));
    assert!(latest.text.contains("[...日志已截断...]"));
}
