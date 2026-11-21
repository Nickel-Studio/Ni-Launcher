//! 完整错误报告归档：只收集诊断及日志，有界保留跨行字段上下文后脱敏写入 ZIP。
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use minecraft_java_rs_core::net::transfer::CancellationToken;
use minecraft_java_rs_core::utils::archive::{ArchiveLimits, write_zip_files};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::launch_diagnostics::DiagnosticReport;
use crate::launch_log_store::{
    DEFAULT_MAX_ARCHIVES, LOG_FILE_NAME, redact_sensitive_text, redact_sensitive_text_checked,
};

const DIAGNOSTIC_FILE: &str = "nickel-diagnostic.json";
const FAILURE_FILE: &str = "nickel-launch-failure.txt";
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FILES: usize = 128;
const MAX_LINE_BYTES: usize = 1024 * 1024;

pub fn begin_session(directory: &Path) -> Result<(), String> {
    let logs = directory.join("logs");
    crate::instances::reject_links(&logs)?;
    let _lease = crate::launch_output::reserve_finished(&logs.join(LOG_FILE_NAME))?;
    for name in [DIAGNOSTIC_FILE, FAILURE_FILE] {
        let path = logs.join(name);
        crate::instances::reject_links(&path)?;
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("无法清理上一轮启动诊断，请检查日志目录权限".into()),
        }
    }
    Ok(())
}

pub fn store_diagnostic(directory: &Path, report: &DiagnosticReport) -> Result<(), String> {
    let text = serde_json::to_string_pretty(report).map_err(|_| "无法生成启动诊断")?;
    store_failure_text(directory, DIAGNOSTIC_FILE, &text)
}

fn store_failure_text(directory: &Path, name: &str, text: &str) -> Result<(), String> {
    let logs = directory.join("logs");
    crate::instances::reject_links(&logs)?;
    fs::create_dir_all(&logs).map_err(|_| "无法创建错误报告目录")?;
    crate::named_profiles::atomic_write(&logs.join(name), redact_sensitive_text(text).as_bytes())
}

pub fn record_launch_failure(app: &AppHandle, root: Option<&str>, version: &str, reason: &str) {
    let directory = root
        .map(PathBuf::from)
        .or_else(|| crate::app_paths::default_game_dir(app).ok());
    if let Some(root) = directory {
        if let Ok(directory) = crate::runtime_settings::resource_directory(app, &root, version) {
            let _ = store_failure_text(&directory, FAILURE_FILE, reason);
            let _ = crate::launch_log_store::LaunchLogStore::instance(&directory)
                .append_text(&format!("[启动失败] {reason}"));
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportManifest {
    product: String,
    version: String,
    generated_at: u64,
    platform: String,
    architecture: String,
    complete: bool,
    files: Vec<String>,
    notes: Vec<String>,
}

struct Sources {
    paths: Vec<(String, PathBuf)>,
    names: HashSet<String>,
    total: u64,
    notes: Vec<String>,
}

impl Sources {
    fn new() -> Self {
        Self {
            paths: Vec::new(),
            names: HashSet::new(),
            total: 0,
            notes: Vec::new(),
        }
    }

    fn add(&mut self, name: String, path: PathBuf) -> Result<(), String> {
        crate::instances::reject_links(&path)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(format!("无法读取报告文件：{name}")),
        };
        if !metadata.is_file() {
            return Err(format!("报告来源不是普通文件：{name}"));
        }
        if !self.names.insert(name.to_lowercase()) {
            return Err("报告文件存在大小写冲突".into());
        }
        self.total = self
            .total
            .checked_add(metadata.len())
            .ok_or("错误报告总大小溢出")?;
        if metadata.len() > MAX_FILE_BYTES
            || self.total > MAX_TOTAL_BYTES
            || self.paths.len() >= MAX_FILES
        {
            return Err("日志过大，无法完整导出报告；请先备份日志并移除无关历史报告后重试".into());
        }
        self.paths.push((name, path));
        Ok(())
    }

    fn collect(&mut self, directory: &Path) -> Result<(), String> {
        crate::instances::reject_links(directory)?;
        let logs = directory.join("logs");
        for name in [
            LOG_FILE_NAME,
            "latest.log",
            "debug.log",
            DIAGNOSTIC_FILE,
            FAILURE_FILE,
        ] {
            self.add(format!("logs/{name}"), logs.join(name))?;
        }
        for index in 1..=DEFAULT_MAX_ARCHIVES {
            let name = format!("{LOG_FILE_NAME}.{index}");
            self.add(format!("logs/{name}"), logs.join(name))?;
        }
        let crashes = directory.join("crash-reports");
        crate::instances::reject_links(&crashes)?;
        match fs::read_dir(&crashes) {
            Ok(entries) => {
                let mut paths = Vec::new();
                for entry in entries {
                    let entry = entry.map_err(|_| "无法读取崩溃报告列表")?;
                    if entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("txt"))
                    {
                        paths.push(entry.path());
                    }
                }
                paths.sort();
                for path in paths {
                    let name = path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .ok_or("崩溃报告名称无效")?;
                    self.add(format!("crash-reports/{name}"), path.clone())?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.notes
                    .push("游戏没有生成独立崩溃报告，已收集现有日志。".into());
            }
            Err(_) => return Err("无法读取崩溃报告目录，请检查权限后重试".into()),
        }
        if self.paths.is_empty() {
            self.notes
                .push("未找到游戏日志，报告仅包含启动错误及运行环境。".into());
        }
        Ok(())
    }
}

fn sanitized_copy(source: &Path, destination: &Path) -> Result<(), String> {
    crate::instances::reject_links(source)?;
    let file = File::open(source).map_err(|_| "无法读取完整日志，请检查权限后重试")?;
    let expected = file.metadata().map_err(|_| "无法读取日志大小")?.len();
    if expected > MAX_FILE_BYTES {
        return Err("日志超过完整报告配额".into());
    }
    // Keep one bounded source document until field boundaries are known. Flushing
    // at newlines loses the sensitive-key context of pretty-printed collections.
    let mut reader = BufReader::new(file);
    let mut document = Vec::new();
    let mut line_bytes = 0;
    loop {
        let buffer = reader.fill_buf().map_err(|_| "无法读取日志内容")?;
        if buffer.is_empty() {
            break;
        }
        let count = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |index| index + 1);
        line_bytes += count;
        if line_bytes > MAX_LINE_BYTES {
            return Err("日志单行过长，无法安全生成完整报告".into());
        }
        if document.len() as u64 + count as u64 > MAX_FILE_BYTES {
            return Err("日志在导出时持续增长，请等待游戏退出后重试".into());
        }
        if buffer[count - 1] == b'\n' {
            line_bytes = 0;
        }
        document.extend_from_slice(&buffer[..count]);
        reader.consume(count);
    }
    if document.len() as u64 != expected {
        return Err("日志在导出期间发生变化，请等待游戏退出后重试".into());
    }
    crate::instances::reject_links(source)?;
    if fs::metadata(source).map_err(|_| "日志已不可读取")?.len() != expected {
        return Err("日志在导出期间发生变化，请重试".into());
    }
    let sanitized = redact_sensitive_text_checked(&String::from_utf8_lossy(&document))?;
    fs::create_dir_all(destination.parent().ok_or("报告文件缺少目录")?)
        .map_err(|_| "无法创建报告临时目录")?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| "无法写入报告临时文件")?;
    output
        .write_all(sanitized.as_bytes())
        .map_err(|_| "无法写入完整报告")?;
    output
        .sync_all()
        .map_err(|_| "错误报告未能完整写入".to_owned())
}

struct TemporaryDirectory(PathBuf);
impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn export_report(
    directory: &Path,
    destination: &Path,
    failure: &str,
) -> Result<ReportManifest, String> {
    if failure.len() > 32 * 1024 {
        return Err("错误描述过长，请重新打开错误窗口后重试".into());
    }
    crate::instances::reject_links(destination)?;
    if destination.exists() {
        return Err("同名报告已存在，请换一个文件名，原文件不会被覆盖".into());
    }
    let _lease =
        crate::launch_output::reserve_finished(&directory.join("logs").join(LOG_FILE_NAME))?;
    let mut sources = Sources::new();
    sources.collect(directory)?;
    let stage = std::env::temp_dir().join(format!(
        "nickel-error-report-{}",
        crate::store::random_token()
    ));
    fs::create_dir(&stage).map_err(|_| "无法创建错误报告临时目录")?;
    let stage = TemporaryDirectory(stage);
    let mut files = Vec::new();
    for (name, source) in &sources.paths {
        sanitized_copy(source, &stage.0.join(name))?;
        files.push((name.clone(), name.clone()));
    }
    let manifest = ReportManifest {
        product: "Nickel".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        generated_at: crate::store::now_secs(),
        platform: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        complete: true,
        files: sources.paths.iter().map(|(name, _)| name.clone()).collect(),
        notes: sources.notes,
    };
    let index = serde_json::to_vec_pretty(&manifest).map_err(|_| "无法生成错误报告清单")?;
    let notice = "Nickel 完整错误报告\n\n寻求帮助时，请发送此 ZIP 文件，不要只发送崩溃弹窗截图，也不要只挑选其中一份日志。\n报告包含现有完整启动日志、游戏日志及崩溃报告；敏感字段已自动脱敏。未收集账号配置、密钥文件、世界存档或资源包。\n发送前仍建议检查日志中由 Mod 自行写入的个人内容。\n";
    let failure = redact_sensitive_text_checked(failure)?;
    let limits = ArchiveLimits {
        max_entries: MAX_FILES + 3,
        max_entry_bytes: MAX_FILE_BYTES,
        max_total_bytes: MAX_TOTAL_BYTES,
        ..Default::default()
    };
    write_zip_files(
        destination,
        &stage.0,
        &files,
        &[
            ("report.json".into(), index),
            ("READ-ME.txt".into(), notice.as_bytes().to_vec()),
            ("failure.txt".into(), failure.into_bytes()),
        ],
        &limits,
        &CancellationToken::new(),
        |_, _, _| {},
    )?;
    Ok(manifest)
}

#[tauri::command]
pub async fn launch_error_report_export(
    app: AppHandle,
    dir: String,
    version_id: String,
    failure: String,
) -> Result<Option<String>, String> {
    if failure.len() > 32 * 1024 {
        return Err("错误描述过长，请重新打开错误窗口后重试".into());
    }
    let directory =
        crate::runtime_settings::resource_directory(&app, Path::new(&dir), &version_id)?;
    let name = format!("Nickel-错误报告-{}.zip", crate::store::now_secs());
    let Some(destination) =
        crate::pack_export::choose_file(&app, true, "导出完整错误报告", &name, &["zip"]).await?
    else {
        return Ok(None);
    };
    let destination = if destination
        .extension()
        .is_some_and(|value| value.eq_ignore_ascii_case("zip"))
    {
        destination
    } else {
        destination.with_extension("zip")
    };
    crate::instances::reject_links(&destination)?;
    let parent = fs::canonicalize(destination.parent().ok_or("请选择本地保存目录")?)
        .map_err(|_| "保存目录不可访问，请选择其他文件夹")?;
    let destination = parent.join(destination.file_name().ok_or("保存文件名无效")?);
    if destination.exists() {
        return Err("同名报告已存在，请换一个文件名，原文件不会被覆盖".into());
    }
    if destination.starts_with(directory.join("logs"))
        || destination.starts_with(directory.join("crash-reports"))
    {
        return Err("请将完整 ZIP 保存到日志目录之外，避免报告被再次收集".into());
    }
    let returned = destination.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || export_report(&directory, &destination, &failure))
        .await
        .map_err(|_| "生成错误报告时发生异常，请重试")??;
    Ok(Some(returned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use minecraft_java_rs_core::utils::archive::read_zip_entry;

    #[test]
    fn report_preserves_complete_logs_and_crashes_but_excludes_private_files() {
        let root = std::env::temp_dir().join(format!(
            "nickel-report-test-{}",
            crate::store::random_token()
        ));
        fs::create_dir_all(root.join("logs")).unwrap();
        fs::create_dir_all(root.join("crash-reports")).unwrap();
        let log = format!(
            "BEGIN\n{}\naccessToken=private-access-value\nclientSecret=private-secret-value\nC:\\Users\\PrivateName\\game\nEND\n",
            "ordinary log line\n".repeat(20_000)
        );
        fs::write(root.join("logs/latest.log"), &log).unwrap();
        fs::write(root.join("logs/debug.log"), "all debug details\n").unwrap();
        fs::write(
            root.join("logs/nickel-launcher.log.1"),
            "previous session\n",
        )
        .unwrap();
        fs::write(
            root.join("crash-reports/crash-2026.txt"),
            "full crash stack\npassword=private-password-value\n",
        )
        .unwrap();
        fs::write(root.join("secrets.local.json"), "do-not-export").unwrap();
        fs::write(root.join("accounts.json"), "do-not-export").unwrap();
        let output = root.join("report.zip");
        let manifest = export_report(&root, &output, "OutOfMemoryError").unwrap();
        assert!(manifest.complete);
        assert_eq!(manifest.files.len(), 4);
        let cancel = CancellationToken::new();
        let text = String::from_utf8(
            read_zip_entry(&output, "logs/latest.log", MAX_FILE_BYTES, &cancel).unwrap(),
        )
        .unwrap();
        assert!(text.starts_with("BEGIN\n") && text.ends_with("END\n"));
        assert_eq!(text.matches("ordinary log line").count(), 20_000);
        for value in [
            "private-access-value",
            "private-secret-value",
            "PrivateName",
        ] {
            assert!(!text.contains(value));
        }
        let crash = String::from_utf8(
            read_zip_entry(
                &output,
                "crash-reports/crash-2026.txt",
                MAX_FILE_BYTES,
                &cancel,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(crash.contains("full crash stack") && !crash.contains("private-password-value"));
        assert!(read_zip_entry(&output, "secrets.local.json", 1024, &cancel).is_err());
        assert!(read_zip_entry(&output, "accounts.json", 1024, &cancel).is_err());
        if let Some(evidence) = std::env::var_os("NICKEL_REPORT_TEST_OUTPUT") {
            let evidence = PathBuf::from(evidence);
            fs::create_dir_all(&evidence).unwrap();
            let target = evidence.join("complete-report.zip");
            assert!(!target.exists());
            fs::copy(&output, &target).unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn report_redacts_complete_quoted_secrets_and_json_user_paths() {
        let root = std::env::temp_dir().join(format!(
            "nickel-report-redaction-{}",
            crate::store::random_token()
        ));
        let raw = serde_json::json!({
            "password": "probe'secret-tail",
            "clientSecret": "probe\"secret-tail",
            "apiKey": "probe\\secret-tail",
            "path": r"C:\Users\PrivateReviewName\game",
            "message": "visible diagnostic"
        })
        .to_string();
        let expected = serde_json::json!({
            "password": "***", "clientSecret": "***", "apiKey": "***",
            "path": r"C:\Users\***\game", "message": "visible diagnostic"
        });
        let files = [
            "logs/latest.log",
            "logs/debug.log",
            "crash-reports/crash-quoted.txt",
        ];
        for relative in files {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, &raw).unwrap();
        }
        let output = root.join("report.zip");
        assert!(export_report(&root, &output, &raw).unwrap().complete);
        for relative in files {
            let bytes =
                read_zip_entry(&output, relative, MAX_FILE_BYTES, &CancellationToken::new())
                    .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value, expected, "{relative}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn report_zip_masks_nested_escaped_and_multiline_collections_without_losing_neighbors() {
        let root = std::env::temp_dir().join(format!(
            "nickel-report-collections-{}",
            crate::store::random_token()
        ));
        let root = TemporaryDirectory(root);
        fs::create_dir_all(root.0.join("logs")).unwrap();
        fs::create_dir_all(root.0.join("crash-reports")).unwrap();
        let files = [
            "logs/latest.log",
            "logs/debug.log",
            "crash-reports/crash-collections.txt",
        ];
        let cases = [
            (r#"{"password":["zip-secret"],"next":"visible"}"#.to_owned(), true),
            (r#"{"apiKey":{"value":[{"nested":"zip-secret"}]},"next":"visible"}"#.to_owned(), true),
            (r#"{"accessToken":["zip-secret"],"next":"visible"}"#.to_owned(), true),
            (r#"{"password":["zip-secret\"}]\\tail", {"password":["zip-secret"]}],"next":"visible"}"#.to_owned(), true),
            ("{\r\n\"password\"\r\n:\r\n[\r\n{\"v\":\"zip-secret\"}\r\n],\r\n\"next\":\"visible\"\r\n}".to_owned(), true),
            (format!("{{\"password\":[{}\"zip-secret\"],\"next\":\"visible\"}}", " \n".repeat(5000)), true),
            (r#"{"password":[],"next":"visible"}"#.to_owned(), true),
            (r#"{"password":{"primary":[null,false,"placeholder","${PASSWORD}"]},"next":"visible"}"#.to_owned(), true),
            ("password = [/* ] } */ 'zip-secret', {value: `zip-secret\\`}]`} ]\nnext=visible".to_owned(), false),
            ("password = [# ] }\n'zip-secret']\nnext=visible".to_owned(), false),
        ];
        for (index, (raw, json)) in cases.iter().enumerate() {
            for relative in files {
                fs::write(root.0.join(relative), raw).unwrap();
            }
            let target = root.0.join(format!("collections-{index}.zip"));
            assert!(export_report(&root.0, &target, raw).unwrap().complete);
            for relative in files.into_iter().chain(["failure.txt"]) {
                let bytes =
                    read_zip_entry(&target, relative, MAX_FILE_BYTES, &CancellationToken::new())
                        .unwrap();
                let redacted = String::from_utf8(bytes).unwrap();
                assert!(
                    !redacted.contains("zip-secret"),
                    "{index}: {relative}: {redacted}"
                );
                assert!(redacted.contains("visible"), "{index}: {redacted}");
                assert_eq!(redact_sensitive_text(&redacted), redacted);
                if *json {
                    let value: serde_json::Value = serde_json::from_str(&redacted).unwrap();
                    assert_eq!(value["next"], "visible");
                    let key = if raw.contains("\"apiKey\"") {
                        "apiKey"
                    } else if raw.contains("\"accessToken\"") {
                        "accessToken"
                    } else {
                        "password"
                    };
                    assert_eq!(value[key], "***");
                }
            }
            if let Some(evidence) = std::env::var_os("NICKEL_REPORT_TEST_OUTPUT") {
                let target_copy = PathBuf::from(evidence).join(format!("collections-{index}.zip"));
                fs::create_dir_all(target_copy.parent().unwrap()).unwrap();
                assert!(!target_copy.exists());
                fs::copy(&target, target_copy).unwrap();
            }
        }
    }

    #[test]
    fn report_zip_uses_structured_keys_and_complete_multiline_value_boundaries() {
        let root = TemporaryDirectory(std::env::temp_dir().join(format!(
            "nickel-report-fields-{}",
            crate::store::random_token()
        )));
        let files = [
            "logs/latest.log",
            "logs/debug.log",
            "crash-reports/crash-fields.txt",
        ];
        for file in files {
            fs::create_dir_all(root.0.join(file).parent().unwrap()).unwrap();
        }
        let cases = [
            r#"{"pass\u0077ord":["zip-secret"],"next":"visible"}"#.to_owned(),
            r#"{"apiKeys":["zip-secret"],"next":"visible"}"#.to_owned(),
            r#"{"password":"zip-secret","api_key":"zip-secret","next":"visible"}"#.to_owned(),
            r#"{"authorization":"Basic zip-secret","next":"visible"}"#.to_owned(),
            "password=\"\nzip-secret\n\"\nnext=visible".into(),
            "password=\"\"\"\nzip-secret\n\"\"\"\nnext=visible".into(),
            "password='''\r\nzip-secret\\tail\r\n'''\r\nnext=visible".into(),
            "password=\"\"\"\"zip-secret\"\"\"\"\nnext=visible".into(),
            "password='zip-secret''suffix'\nnext=visible".into(),
            "password: |\n  zip-secret\nnext: visible".into(),
            "password: >-\r\n  zip-secret\r\nnext: visible".into(),
            "auth:\n  password: |2+ # block\n    zip-secret\n  next: visible".into(),
            "password:\n  - zip-secret\nnext: visible".into(),
            "api_keys:\n  primary: zip-secret\nnext: visible".into(),
            "password={primary=https://example.invalid/zip-secret}\nnext=visible".into(),
            "--password \"zip-secret\" --next visible".into(),
            "--accessToken [\"zip-secret\"]\nnext=visible".into(),
            r#"{"\u0061pi_Keys":{"primary":"zip-secret"},"next":"visible"}"#.into(),
            r#"{"passwords":["zip-secret"],"secrets":["zip-secret"],"next":"visible"}"#.into(),
            "password: zip-secret\n  continuation\nnext: visible".into(),
            "--password=zip-secret --next=visible".into(),
            r#"{"message":"password=zip-secret","next":"visible"}"#.into(),
        ];
        for (index, raw) in cases.iter().enumerate() {
            for file in files {
                fs::write(root.0.join(file), raw).unwrap();
            }
            let zip = root.0.join(format!("fields-{index}.zip"));
            assert!(
                export_report(&root.0, &zip, raw).unwrap().complete,
                "{index}"
            );
            for file in files.into_iter().chain(["failure.txt"]) {
                let text = String::from_utf8(
                    read_zip_entry(&zip, file, MAX_FILE_BYTES, &CancellationToken::new()).unwrap(),
                )
                .unwrap();
                assert!(!text.contains("zip-secret"), "{index} {file}: {text}");
                assert!(text.contains("visible"), "{index} {file}: {text}");
                assert_eq!(redact_sensitive_text(&text), text, "{index}");
                if raw.starts_with('{') {
                    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
                    assert_eq!(json["next"], "visible");
                }
            }
            if let Some(output) = std::env::var_os("NICKEL_REPORT_TEST_OUTPUT") {
                let output = PathBuf::from(output);
                fs::create_dir_all(&output).unwrap();
                let target = output.join(format!("fields-{index}.zip"));
                assert!(!target.exists());
                fs::copy(&zip, target).unwrap();
            }
        }
    }

    #[test]
    fn reports_keep_non_assignment_diagnostics_and_reject_unterminated_values() {
        let root = TemporaryDirectory(std::env::temp_dir().join(format!(
            "nickel-report-syntax-{}",
            crate::store::random_token()
        )));
        fs::create_dir_all(root.0.join("logs")).unwrap();
        for (index, raw) in [
            "SyntaxError: Unexpected token { in JSON at position 0\n    at JSON.parse (<anonymous>)\nnext=visible\n",
            "Unexpected token [\naccessToken documentation\npassword confirmation failed\n",
            "{\"passwordRequired\":true,\"tokenCount\":42,\"message\":\"password documentation\"}\n",
        ].iter().enumerate() {
            fs::write(root.0.join("logs/latest.log"), raw).unwrap();
            let zip = root.0.join(format!("ordinary-{index}.zip"));
            assert!(export_report(&root.0, &zip, raw).unwrap().complete);
            for file in ["logs/latest.log", "failure.txt"] {
                assert_eq!(read_zip_entry(&zip, file, MAX_FILE_BYTES, &CancellationToken::new()).unwrap(), raw.as_bytes());
            }
        }
        for (index, raw) in [
            "password=\"\nzip-secret\n".to_owned(),
            "password='''\nzip-secret\n".into(),
            "password=`zip-secret\n".into(),
            "token={value:zip-secret".into(),
            "--password [zip-secret".into(),
            format!(
                "password=\"\"\"\n{}\n\"\"\"",
                "zip-secret\n".repeat(110_000)
            ),
            format!(
                "password: |\n{}next: visible",
                "  zip-secret\n".repeat(100_000)
            ),
        ]
        .iter()
        .enumerate()
        {
            fs::write(root.0.join("logs/latest.log"), raw).unwrap();
            let zip = root.0.join(format!("unclosed-{index}.zip"));
            assert!(export_report(&root.0, &zip, "test").is_err());
            assert!(!zip.exists());
            assert!(!redact_sensitive_text(raw).contains("zip-secret"));
        }
    }

    #[test]
    fn malformed_or_over_budget_collections_never_publish_a_report_zip() {
        let root = TemporaryDirectory(std::env::temp_dir().join(format!(
            "nickel-report-reject-{}",
            crate::store::random_token()
        )));
        fs::create_dir_all(root.0.join("logs")).unwrap();
        for (index, raw) in [
            "password=[\n'zip-secret'".to_owned(),
            "password=[{'value':'zip-secret']}]".to_owned(),
            "password=['zip-secret\"]".to_owned(),
            "password=[/* zip-secret */".to_owned(),
            format!(
                "password={}\"zip-secret\"{}",
                "[".repeat(65),
                "]".repeat(65)
            ),
            format!("password=[{}\"zip-secret\"]", " \n".repeat(512 * 1024)),
            format!("password=[\"zip-secret{}\"]", "x".repeat(MAX_LINE_BYTES)),
        ]
        .iter()
        .enumerate()
        {
            fs::write(root.0.join("logs/latest.log"), raw).unwrap();
            let target = root.0.join(format!("rejected-{index}.zip"));
            let error = export_report(&root.0, &target, "test").unwrap_err();
            assert!(!error.contains("zip-secret"));
            assert!(!target.exists(), "{index}");
            assert!(!redact_sensitive_text(raw).contains("zip-secret"));
        }
        fs::write(root.0.join("logs/latest.log"), "visible diagnostic").unwrap();
        let target = root.0.join("rejected-failure.zip");
        assert!(export_report(&root.0, &target, "password=[\n'zip-secret'").is_err());
        assert!(!target.exists());
    }

    #[test]
    fn pre_redacted_collection_fragments_cannot_hide_unsafe_report_continuations() {
        let root = TemporaryDirectory(std::env::temp_dir().join(format!(
            "nickel-report-fragments-{}",
            crate::store::random_token()
        )));
        let store = crate::launch_log_store::LaunchLogStore::instance(&root.0);
        for line in [
            "{",
            "\"password\": [",
            "\"zip-secret\"",
            "],",
            "\"next\": \"visible\"",
            "}",
        ] {
            // Historical stateless log fragments must still fail the checked export API.
            store.append_text(&redact_sensitive_text(line)).unwrap();
        }
        let stored = fs::read_to_string(store.log_path()).unwrap();
        assert!(stored.contains("zip-secret"));
        let target = root.0.join("rejected-fragments.zip");
        let error = export_report(&root.0, &target, "test").unwrap_err();
        assert!(!error.contains("zip-secret"));
        assert!(!target.exists());
        let redacted = redact_sensitive_text(&stored);
        assert!(!redacted.contains("zip-secret"));
        assert_eq!(redact_sensitive_text(&redacted), redacted);
    }

    fn stream_pump(
        store: crate::launch_log_store::LaunchLogStore,
    ) -> (
        tokio::sync::mpsc::Sender<minecraft_java_rs_core::launcher::events::LaunchEvent>,
        crate::launch_output::OutputPump,
        std::sync::Arc<std::sync::Mutex<crate::launch_output::OutputCapture>>,
        std::sync::Arc<std::sync::Mutex<Vec<crate::FrontendEvent>>>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        let capture = std::sync::Arc::new(std::sync::Mutex::new(
            crate::launch_output::OutputCapture::default(),
        ));
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let emitted = events.clone();
        let pump =
            crate::launch_output::OutputPump::start(store, rx, capture.clone(), move |event| {
                emitted.lock().unwrap().push(event)
            })
            .unwrap();
        (tx, pump, capture, events)
    }

    #[tokio::test]
    async fn report_zip_accepts_complete_line_by_line_process_streams() {
        use minecraft_java_rs_core::launcher::{events::ProcessStream, forward_process_output};
        use tokio::io::AsyncWriteExt;
        let cases = [
            "{\n\"password\": [\n],\n\"next\":\"visible\"\n}\n",
            "{\n\"apiKeys\": [\n\"stream-secret\"\n],\n\"next\":\"visible\"\n}\n",
            "{\n\"pass\\u0077ord\"\n:\n[\n{\"v\":\"stream-secret\"}\n],\n\"next\":\"visible\"\n}\n",
            "password=\"\nstream-secret\n\"\nnext=visible\n",
            "password=\"\"\"\r\nstream-secret\r\n\"\"\"\r\nnext=visible\r\n",
            "password='''\nstream-secret\\tail\n'''\nnext=visible\n",
            "password: |2+ # comment\n  stream-secret\nnext: visible\n",
            "password:\n  primary: stream-secret\nnext: visible\n",
            "password: stream-secret\n  continuation\nnext: visible\n",
            "password=[/* ]\n } */ 'stream-secret',\n{value:https://example.invalid/stream-secret}]\nnext=visible\n",
            "password=\"stream-secret\"\nnext=visible",
            "SyntaxError: Unexpected token { in JSON\nnext=visible\n",
        ];
        for (index, raw) in cases.iter().enumerate() {
            let root = TemporaryDirectory(
                std::env::temp_dir()
                    .join(format!("nickel-stream-{}", crate::store::random_token())),
            );
            crate::runtime_settings::start_launch_log(&root.0);
            let store = crate::launch_log_store::LaunchLogStore::instance(&root.0);
            let (tx, pump, capture, events) = stream_pump(store.clone());
            let (mut writer, reader) = tokio::io::duplex(7);
            let stdout = tokio::spawn(forward_process_output(
                reader,
                tx.clone(),
                ProcessStream::Stdout,
            ));
            let stderr = tokio::spawn(forward_process_output(
                &b"OutOfMemoryError visible-stderr\n"[..],
                tx.clone(),
                ProcessStream::Stderr,
            ));
            let bytes = raw.as_bytes().to_vec();
            let write = tokio::spawn(async move {
                for byte in bytes {
                    writer.write_all(&[byte]).await.unwrap();
                }
                writer.shutdown().await.unwrap();
            });
            drop(tx);
            write.await.unwrap();
            stdout.await.unwrap();
            stderr.await.unwrap();
            pump.finish().await;
            let stored = fs::read_to_string(store.log_path()).unwrap();
            assert!(!stored.contains("stream-secret"), "{index}: {stored}");
            assert!(!stored.contains("无法安全脱敏"), "{index}: {stored}");
            assert!(stored.contains("visible-stderr") && stored.contains("next"));
            assert!(!capture.lock().unwrap().text().contains("stream-secret"));
            assert!(
                !serde_json::to_string(&*events.lock().unwrap())
                    .unwrap()
                    .contains("stream-secret")
            );
            let zip = root.0.join("report.zip");
            assert!(
                export_report(&root.0, &zip, "test").unwrap().complete,
                "{index}"
            );
            let text = String::from_utf8(
                read_zip_entry(
                    &zip,
                    "logs/nickel-launcher.log",
                    MAX_FILE_BYTES,
                    &CancellationToken::new(),
                )
                .unwrap(),
            )
            .unwrap();
            assert!(
                !text.contains("stream-secret")
                    && text.contains("visible-stderr")
                    && text.contains("visible")
            );
            if let Some(output) = std::env::var_os("NICKEL_REPORT_TEST_OUTPUT") {
                let target = PathBuf::from(output).join(format!("stream-{index}.zip"));
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                assert!(!target.exists());
                fs::copy(&zip, target).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn quoted_json_messages_are_safe_in_storage_capture_events_and_raw_latest_zip() {
        use minecraft_java_rs_core::launcher::{events::ProcessStream, forward_process_output};
        use tokio::io::AsyncWriteExt;
        let mut cases = vec![
            r#"{"message":"password=\"round03-secret\"","next":"visible"}"#.to_owned(),
            r#"{"message":"prefix pass\u0077ord=\u0022round03-secret\u0022 suffix visible","next":"visible"}"#.to_owned(),
        ];
        for message in [
            r#"prefix password="round03-secret\\tail\"more" suffix visible"#,
            r#"prefix password="round03-secret\uD83D\uDE00" suffix visible"#,
            r#"prefix password=["round03-secret",{"v":"round03-secret"}] suffix visible"#,
            r#"prefix {"password":["round03-secret"],"other":"visible"} suffix visible"#,
            r#"Unexpected token \" in JSON and password == expected"#,
            r#"password="***" and token=abcd***wxyz"#,
        ] {
            cases.push(serde_json::json!({"message":message,"next":"visible"}).to_string());
        }
        for (index, raw) in cases.iter().enumerate() {
            let root = TemporaryDirectory(std::env::temp_dir().join(format!(
                "nickel-quoted-message-{}",
                crate::store::random_token(),
            )));
            let store = crate::launch_log_store::LaunchLogStore::instance(&root.0);
            let (tx, pump, capture, events) = stream_pump(store.clone());
            let (mut writer, reader) = tokio::io::duplex(7);
            let stdout = tokio::spawn(forward_process_output(
                reader,
                tx.clone(),
                ProcessStream::Stdout,
            ));
            let stderr = tokio::spawn(forward_process_output(
                &b"visible-stderr\n"[..],
                tx.clone(),
                ProcessStream::Stderr,
            ));
            let bytes = format!("{raw}\n").into_bytes();
            let write = tokio::spawn(async move {
                for byte in bytes {
                    writer.write_all(&[byte]).await.unwrap();
                }
                writer.shutdown().await.unwrap();
            });
            drop(tx);
            write.await.unwrap();
            stdout.await.unwrap();
            stderr.await.unwrap();
            pump.finish().await;
            let stored = fs::read_to_string(store.log_path()).unwrap();
            let captured = capture.lock().unwrap().text();
            let payload = serde_json::to_string(&*events.lock().unwrap()).unwrap();
            for text in [&stored, &captured, &payload] {
                assert!(
                    !text.contains("round03-secret") && !text.contains("无法安全脱敏"),
                    "{index}: {text}"
                );
                assert!(text.contains("visible-stderr") && text.contains("visible"));
            }
            let safe = stored.lines().find(|line| line.starts_with('{')).unwrap();
            let value: serde_json::Value = serde_json::from_str(safe).unwrap();
            assert_eq!(value["next"], "visible");
            assert_eq!(
                crate::launch_log_store::redact_sensitive_text_checked(safe).unwrap(),
                safe
            );
            for event in events.lock().unwrap().iter() {
                if let crate::FrontendEvent::Log { line } = event {
                    if line.starts_with('{') {
                        assert_eq!(line.trim_end(), safe);
                    }
                }
            }
            let stream_zip = root.0.join("stream.zip");
            assert!(
                export_report(&root.0, &stream_zip, "quoted message test")
                    .unwrap()
                    .complete
            );
            let stream_entry = read_zip_entry(
                &stream_zip,
                "logs/nickel-launcher.log",
                MAX_FILE_BYTES,
                &CancellationToken::new(),
            )
            .unwrap();
            assert!(
                !String::from_utf8(stream_entry)
                    .unwrap()
                    .contains("round03-secret")
            );
            fs::write(root.0.join("logs/latest.log"), format!("{raw}\n")).unwrap();
            let raw_zip = root.0.join("raw-latest.zip");
            assert!(
                export_report(&root.0, &raw_zip, "raw quoted message test")
                    .unwrap()
                    .complete
            );
            let raw_entry = read_zip_entry(
                &raw_zip,
                "logs/latest.log",
                MAX_FILE_BYTES,
                &CancellationToken::new(),
            )
            .unwrap();
            let raw_safe = String::from_utf8(raw_entry).unwrap();
            assert_eq!(raw_safe.trim_end(), safe);
            assert!(!raw_safe.contains("round03-secret"));
            if let Some(output) = std::env::var_os("NICKEL_REPORT_TEST_OUTPUT") {
                let output = PathBuf::from(output);
                fs::create_dir_all(&output).unwrap();
                for (label, path) in [("stream", &stream_zip), ("raw-latest", &raw_zip)] {
                    let target = output.join(format!("quoted-message-{index}-{label}.zip"));
                    assert!(!target.exists());
                    fs::copy(path, target).unwrap();
                }
                fs::write(
                    output.join(format!("quoted-message-{index}-storage.log")),
                    stored,
                )
                .unwrap();
                fs::write(
                    output.join(format!("quoted-message-{index}-capture.log")),
                    captured,
                )
                .unwrap();
                fs::write(
                    output.join(format!("quoted-message-{index}-events.json")),
                    payload,
                )
                .unwrap();
            }
        }
    }

    #[tokio::test]
    async fn user_paths_in_serialized_messages_are_safe_through_process_and_report_sources() {
        use crate::launch_log_store::{LaunchLogStore, redact_sensitive_text_checked};
        use minecraft_java_rs_core::launcher::{events::ProcessStream, forward_process_output};
        use tokio::io::AsyncWriteExt;

        let mut cases = Vec::new();
        for user in ["R04PrivateUser", "R04\u{6d4b}\u{8bd5} \u{1f600}"] {
            for (prefix, suffix) in [(r"C:\Users\", r"\.minecraft"), ("/home/", "/.minecraft")] {
                let mut raw = serde_json::json!({"path":format!("{prefix}{user}{suffix}"),"before":"visible-before","next":"visible"});
                let mut expected = serde_json::json!({"path":format!("{prefix}***{suffix}"),"before":"visible-before","next":"visible"});
                for _ in 0..3 {
                    cases.push((user, raw.to_string(), expected.clone()));
                    raw = serde_json::json!({"message":raw.to_string(),"next":"visible"});
                    expected = serde_json::json!({"message":expected.to_string(),"next":"visible"});
                }
            }
        }
        let mut evidence = Vec::new();
        for (index, (user, raw, expected)) in cases.into_iter().enumerate() {
            let root = TemporaryDirectory(
                std::env::temp_dir()
                    .join(format!("nickel-user-path-{}", crate::store::random_token())),
            );
            let safe = redact_sensitive_text_checked(&raw).unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&safe).unwrap(),
                expected
            );
            assert!(!safe.contains(user), "{index}: {safe}");
            assert_eq!(redact_sensitive_text_checked(&safe).unwrap(), safe);
            let document_store = LaunchLogStore::instance(root.0.join("document"));
            document_store.append_text(&raw).unwrap();
            assert_eq!(
                fs::read_to_string(document_store.log_path())
                    .unwrap()
                    .trim_end(),
                safe
            );

            let store = LaunchLogStore::instance(&root.0);
            let (tx, pump, capture, events) = stream_pump(store.clone());
            assert!(crate::launch_output::reserve_finished(store.log_path()).is_err());
            let (mut writer, reader) = tokio::io::duplex(7);
            let stdout = tokio::spawn(forward_process_output(
                reader,
                tx.clone(),
                ProcessStream::Stdout,
            ));
            let stderr = tokio::spawn(forward_process_output(
                &b"visible-stderr\n"[..],
                tx.clone(),
                ProcessStream::Stderr,
            ));
            let bytes = format!("{raw}\n").into_bytes();
            let write = tokio::spawn(async move {
                for chunk in bytes.chunks(3) {
                    writer.write_all(chunk).await.unwrap();
                }
                writer.shutdown().await.unwrap();
            });
            drop(tx);
            write.await.unwrap();
            stdout.await.unwrap();
            stderr.await.unwrap();
            pump.finish().await;
            assert!(crate::launch_output::reserve_finished(store.log_path()).is_ok());
            let stored = fs::read_to_string(store.log_path()).unwrap();
            let captured = capture.lock().unwrap().text();
            let payload = serde_json::to_string(&*events.lock().unwrap()).unwrap();
            for text in [&stored, &captured, &payload] {
                assert!(
                    !text.contains(user) && !text.contains("无法安全脱敏"),
                    "{index}: {text}"
                );
                assert!(text.contains("visible-stderr") && text.contains("visible-before"));
            }
            let stored_json = stored.lines().find(|line| line.starts_with('{')).unwrap();
            assert_eq!(stored_json, safe);
            for event in events.lock().unwrap().iter() {
                if let crate::FrontendEvent::Log { line } = event {
                    if line.starts_with('{') {
                        assert_eq!(line.trim_end(), safe);
                    }
                }
            }
            let stream_zip = root.0.join("stream.zip");
            assert!(
                export_report(&root.0, &stream_zip, "user path test")
                    .unwrap()
                    .complete
            );
            fs::write(root.0.join("logs/latest.log"), format!("{raw}\n")).unwrap();
            let raw_zip = root.0.join("raw-latest.zip");
            assert!(
                export_report(&root.0, &raw_zip, "raw user path test")
                    .unwrap()
                    .complete
            );
            for (zip, entry) in [
                (&stream_zip, "logs/nickel-launcher.log"),
                (&raw_zip, "logs/latest.log"),
            ] {
                let text = String::from_utf8(
                    read_zip_entry(zip, entry, MAX_FILE_BYTES, &CancellationToken::new()).unwrap(),
                )
                .unwrap();
                assert!(
                    !text.contains(user) && text.contains("visible-before"),
                    "{index}: {text}"
                );
                let json = text.lines().find(|line| line.starts_with('{')).unwrap();
                assert_eq!(json, safe);
                assert_eq!(redact_sensitive_text_checked(json).unwrap(), safe);
            }
            if let Some(output) = std::env::var_os("NICKEL_REPORT_TEST_OUTPUT") {
                let output = PathBuf::from(output);
                fs::create_dir_all(&output).unwrap();
                for (label, path) in [("stream", &stream_zip), ("raw-latest", &raw_zip)] {
                    let target = output.join(format!("user-path-{index}-{label}.zip"));
                    assert!(!target.exists());
                    fs::copy(path, target).unwrap();
                }
                for (label, text) in [
                    ("document", &safe),
                    ("storage", &stored),
                    ("capture", &captured),
                    ("events", &payload),
                ] {
                    let target = output.join(format!("user-path-{index}-{label}.log"));
                    assert!(!target.exists());
                    fs::write(target, text).unwrap();
                }
            }
            evidence.push(serde_json::json!({"index":index,"private":user,"expected":expected,"document":safe,"raw":raw}));
        }
        if let Some(output) = std::env::var_os("NICKEL_REPORT_TEST_OUTPUT") {
            let target = PathBuf::from(output).join("user-path-matrix.json");
            assert!(!target.exists());
            fs::write(target, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
        }
    }

    #[tokio::test]
    async fn stdout_pending_values_never_capture_stderr_or_another_session() {
        use minecraft_java_rs_core::launcher::events::{LaunchEvent, ProcessStream};
        let root = TemporaryDirectory(
            std::env::temp_dir().join(format!("nickel-isolated-{}", crate::store::random_token())),
        );
        let store = crate::launch_log_store::LaunchLogStore::instance(&root.0);
        for _ in 0..2 {
            crate::runtime_settings::start_launch_log(&root.0);
            let (tx, pump, capture, _) = stream_pump(store.clone());
            tx.send(LaunchEvent::ProcessOutput {
                stream: ProcessStream::Stdout,
                line: "password=[".into(),
                discarded: false,
            })
            .await
            .unwrap();
            tx.send(LaunchEvent::ProcessOutput {
                stream: ProcessStream::Stderr,
                line: "visible-stderr".into(),
                discarded: false,
            })
            .await
            .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while !capture.lock().unwrap().text().contains("visible-stderr") {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            for line in ["\"stream-secret\"", "]", "next=visible"] {
                tx.send(LaunchEvent::ProcessOutput {
                    stream: ProcessStream::Stdout,
                    line: line.into(),
                    discarded: false,
                })
                .await
                .unwrap();
            }
            for stream in [ProcessStream::Stdout, ProcessStream::Stderr] {
                tx.send(LaunchEvent::OutputClosed {
                    stream,
                    failed: false,
                })
                .await
                .unwrap();
            }
            drop(tx);
            pump.finish().await;
        }
        let zip = root.0.join("report.zip");
        assert!(export_report(&root.0, &zip, "test").unwrap().complete);
        let text = fs::read_to_string(store.log_path()).unwrap();
        assert_eq!(text.matches("next=visible").count(), 2);
        assert_eq!(text.matches("visible-stderr").count(), 2);
        assert!(!text.contains("stream-secret") && !text.contains("无法安全脱敏"));
    }

    #[tokio::test]
    async fn report_zip_rejects_failed_unclosed_and_discarded_process_input() {
        use minecraft_java_rs_core::launcher::events::{LaunchEvent, ProcessStream};
        for (index, prefix, discarded, failed) in [
            (0, "password=[", false, false),
            (1, "password=\"", false, false),
            (2, "password='''", false, false),
            (3, "password=`", false, false),
            (4, "password=[]", true, false),
            (5, "password=[]", false, true),
        ] {
            let root = TemporaryDirectory(std::env::temp_dir().join(format!(
                "nickel-unsafe-stream-{}",
                crate::store::random_token()
            )));
            let store = crate::launch_log_store::LaunchLogStore::instance(&root.0);
            let (tx, pump, capture, _) = stream_pump(store.clone());
            for line in [
                prefix,
                if failed {
                    "password=stream-secret"
                } else {
                    "stream-secret"
                },
            ] {
                tx.send(LaunchEvent::ProcessOutput {
                    stream: ProcessStream::Stdout,
                    line: line.into(),
                    discarded,
                })
                .await
                .unwrap();
            }
            tx.send(LaunchEvent::ProcessOutput {
                stream: ProcessStream::Stderr,
                line: "visible-stderr".into(),
                discarded: false,
            })
            .await
            .unwrap();
            tx.send(LaunchEvent::OutputClosed {
                stream: ProcessStream::Stdout,
                failed,
            })
            .await
            .unwrap();
            tx.send(LaunchEvent::OutputClosed {
                stream: ProcessStream::Stderr,
                failed: false,
            })
            .await
            .unwrap();
            drop(tx);
            pump.finish().await;
            let text = fs::read_to_string(store.log_path()).unwrap();
            assert!(
                text.contains("无法安全脱敏") && text.contains("visible-stderr"),
                "{index}: {text}"
            );
            assert!(!text.contains("stream-secret"));
            assert!(!capture.lock().unwrap().text().contains("stream-secret"));
            let zip = root.0.join("report.zip");
            assert!(export_report(&root.0, &zip, "test").is_err());
            assert!(!zip.exists());
        }
    }

    #[tokio::test]
    async fn rotation_and_utf8_caps_only_apply_after_stream_redaction() {
        use minecraft_java_rs_core::launcher::{events::ProcessStream, forward_process_output};
        let root = TemporaryDirectory(std::env::temp_dir().join(format!(
            "nickel-stream-rotation-{}",
            crate::store::random_token()
        )));
        let store = crate::launch_log_store::LaunchLogStore::with_limits(&root.0, 1024, 3);
        for session in 0..3 {
            let (tx, pump, capture, _) = stream_pump(store.clone());
            let raw = format!(
                "{}\npassword=[\n\"stream-secret\"\n]\nnext=visible-{session}\n",
                "中文".repeat(6000)
            );
            let stdout = forward_process_output(raw.as_bytes(), tx.clone(), ProcessStream::Stdout);
            let stderr =
                forward_process_output(&b"visible-stderr\n"[..], tx.clone(), ProcessStream::Stderr);
            drop(tx);
            tokio::join!(stdout, stderr);
            pump.finish().await;
            assert!(!capture.lock().unwrap().text().contains("stream-secret"));
        }
        assert!(store.paths().archive_file(1).exists());
        let zip = root.0.join("report.zip");
        let manifest = export_report(&root.0, &zip, "test").unwrap();
        assert!(manifest.complete);
        for name in manifest.files {
            let text = String::from_utf8(
                read_zip_entry(&zip, &name, MAX_FILE_BYTES, &CancellationToken::new()).unwrap(),
            )
            .unwrap();
            assert!(!text.contains("stream-secret") && !text.contains("无法安全脱敏"));
        }
    }

    #[test]
    fn log_pipeline_process_fixture() {
        let mode = std::env::var("NICKEL_LOG_PIPE_FIXTURE").unwrap_or_default();
        if !matches!(mode.as_str(), "round02" | "complete" | "terminate") {
            return;
        }
        use std::io::Write;
        let output: &[u8] = if mode == "terminate" {
            b"password=[\n\"stream-secret\"\n"
        } else {
            b"{\n\"password\": [\n\"stream-secret\"\n],\n\"next\":\"visible\"\n}\n"
        };
        std::io::stdout().write_all(output).unwrap();
        std::io::stdout().flush().unwrap();
        std::io::stderr()
            .write_all("OutOfMemoryError UTF8 中文 visible-stderr\n".as_bytes())
            .unwrap();
        std::io::stderr().flush().unwrap();
        if mode == "terminate" {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
        if mode != "complete" {
            panic!("fixture process exited unsuccessfully");
        }
    }

    #[tokio::test]
    async fn spawned_process_exit_flushes_real_pipe_storage_and_zip_pipeline() {
        use minecraft_java_rs_core::launcher::{events::ProcessStream, forward_process_output};
        for mode in ["round02", "complete"] {
            let root = TemporaryDirectory(std::env::temp_dir().join(format!(
                "nickel-real-pipes-{}",
                crate::store::random_token()
            )));
            let store = crate::launch_log_store::LaunchLogStore::instance(&root.0);
            let (tx, pump, capture, events) = stream_pump(store.clone());
            let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "error_report::tests::log_pipeline_process_fixture",
                    "--nocapture",
                ])
                .env("NICKEL_LOG_PIPE_FIXTURE", mode)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let stdout = tokio::spawn(forward_process_output(
                child.stdout.take().unwrap(),
                tx.clone(),
                ProcessStream::Stdout,
            ));
            let stderr = tokio::spawn(forward_process_output(
                child.stderr.take().unwrap(),
                tx.clone(),
                ProcessStream::Stderr,
            ));
            drop(tx);
            let status = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(status.success(), mode == "complete");
            pump.finish().await;
            stdout.await.unwrap();
            stderr.await.unwrap();
            assert!(capture.lock().unwrap().text().contains("visible-stderr"));
            assert!(
                !serde_json::to_string(&*events.lock().unwrap())
                    .unwrap()
                    .contains("stream-secret")
            );
            let zip = root.0.join("report.zip");
            assert!(
                export_report(&root.0, &zip, "process test")
                    .unwrap()
                    .complete
            );
            let text = String::from_utf8(
                read_zip_entry(
                    &zip,
                    "logs/nickel-launcher.log",
                    MAX_FILE_BYTES,
                    &CancellationToken::new(),
                )
                .unwrap(),
            )
            .unwrap();
            assert!(text.contains("visible-stderr") && text.contains("visible"));
            assert!(!text.contains("stream-secret") && !text.contains("无法安全脱敏"));
            if let Some(output) = std::env::var_os("NICKEL_REPORT_TEST_OUTPUT") {
                let target = PathBuf::from(output).join(format!("real-process-stream-{mode}.zip"));
                assert!(!target.exists());
                fs::copy(zip, target).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn terminated_process_flushes_unclosed_values_without_exporting_secrets() {
        use minecraft_java_rs_core::launcher::{events::ProcessStream, forward_process_output};
        let root = TemporaryDirectory(std::env::temp_dir().join(format!(
            "nickel-terminated-pipes-{}",
            crate::store::random_token()
        )));
        let store = crate::launch_log_store::LaunchLogStore::instance(&root.0);
        let (tx, pump, capture, events) = stream_pump(store.clone());
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "error_report::tests::log_pipeline_process_fixture",
                "--nocapture",
            ])
            .env("NICKEL_LOG_PIPE_FIXTURE", "terminate")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdout = tokio::spawn(forward_process_output(
            child.stdout.take().unwrap(),
            tx.clone(),
            ProcessStream::Stdout,
        ));
        let stderr = tokio::spawn(forward_process_output(
            child.stderr.take().unwrap(),
            tx.clone(),
            ProcessStream::Stderr,
        ));
        drop(tx);
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !capture.lock().unwrap().text().contains("visible-stderr") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // This is the exact child owned by the fixture, never a process-name kill.
        child.kill().await.unwrap();
        pump.finish().await;
        stdout.await.unwrap();
        stderr.await.unwrap();
        assert!(crate::launch_output::ensure_finished(store.log_path()).is_ok());
        let stored = fs::read_to_string(store.log_path()).unwrap();
        assert!(stored.contains("无法安全脱敏") && stored.contains("visible-stderr"));
        assert!(!stored.contains("stream-secret"));
        assert!(!capture.lock().unwrap().text().contains("stream-secret"));
        assert!(
            !serde_json::to_string(&*events.lock().unwrap())
                .unwrap()
                .contains("stream-secret")
        );
        let destination = root.0.join("report.zip");
        assert!(export_report(&root.0, &destination, "process terminated").is_err());
        assert!(!destination.exists());
        if let Some(output) = std::env::var_os("NICKEL_REPORT_TEST_OUTPUT") {
            let target = PathBuf::from(output).join("real-process-stream-terminate");
            fs::create_dir(&target).unwrap();
            fs::copy(store.log_path(), target.join(LOG_FILE_NAME)).unwrap();
            fs::write(target.join("capture.txt"), capture.lock().unwrap().text()).unwrap();
            fs::write(
                target.join("events.json"),
                serde_json::to_vec(&*events.lock().unwrap()).unwrap(),
            )
            .unwrap();
            fs::write(
                target.join("result.json"),
                serde_json::to_vec_pretty(&serde_json::json!({
                    "terminatedOwnedChild": true,
                    "reportRejected": true,
                    "destinationExists": destination.exists(),
                    "sessionFinished": crate::launch_output::ensure_finished(store.log_path()).is_ok(),
                }))
                .unwrap(),
            )
            .unwrap();
        }
    }

    async fn wait_for_stream_end(path: &Path) {
        tokio::time::timeout(std::time::Duration::from_secs(8), async {
            while crate::launch_output::ensure_finished(path).is_err() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn active_exports_and_new_sessions_wait_for_the_same_log_identity() {
        use minecraft_java_rs_core::launcher::events::{LaunchEvent, ProcessStream};
        let root = TemporaryDirectory(std::env::temp_dir().join(format!(
            "nickel-active-stream-{}",
            crate::store::random_token()
        )));
        fs::create_dir_all(&root.0).unwrap();
        let store = crate::launch_log_store::LaunchLogStore::instance(&root.0);
        let reservation = crate::launch_output::reserve_finished(store.log_path()).unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        assert!(
            crate::launch_output::OutputPump::start(store.clone(), rx, Default::default(), |_| {})
                .is_err()
        );
        drop(tx);
        drop(reservation);
        let (tx, pump, capture, _) = stream_pump(store.clone());
        let alias = root.0.join("logs/../logs").join(LOG_FILE_NAME);
        assert!(crate::launch_output::ensure_finished(&alias).is_err());
        let destination = root.0.join("report.zip");
        assert!(export_report(&root.0, &destination, "test").is_err());
        assert!(!destination.exists());
        assert!(begin_session(&root.0).is_err());
        for line in ["password=[", "\"stream-secret\"", "]", "next=visible"] {
            tx.send(LaunchEvent::ProcessOutput {
                stream: ProcessStream::Stdout,
                line: line.into(),
                discarded: false,
            })
            .await
            .unwrap();
        }
        for stream in [ProcessStream::Stdout, ProcessStream::Stderr] {
            tx.send(LaunchEvent::OutputClosed {
                stream,
                failed: false,
            })
            .await
            .unwrap();
        }
        drop(tx);
        pump.finish().await;
        assert!(crate::launch_output::ensure_finished(&alias).is_ok());
        assert!(!capture.lock().unwrap().text().contains("stream-secret"));
        assert!(
            export_report(&root.0, &destination, "test")
                .unwrap()
                .complete
        );
        begin_session(&root.0).unwrap();
    }

    #[tokio::test]
    async fn dropped_output_guards_drain_and_release_both_streams() {
        use minecraft_java_rs_core::launcher::{events::ProcessStream, forward_process_output};
        use tokio::io::AsyncWriteExt;
        let root = TemporaryDirectory(std::env::temp_dir().join(format!(
            "nickel-dropped-pump-{}",
            crate::store::random_token()
        )));
        let store = crate::launch_log_store::LaunchLogStore::instance(&root.0);
        let (tx, pump, capture, events) = stream_pump(store.clone());
        let (mut writer, reader) = tokio::io::duplex(32);
        let stdout = tokio::spawn(forward_process_output(
            reader,
            tx.clone(),
            ProcessStream::Stdout,
        ));
        let stderr = tokio::spawn(forward_process_output(
            &b"visible-stderr\n"[..],
            tx.clone(),
            ProcessStream::Stderr,
        ));
        writer.write_all(b"password=[\n").await.unwrap();
        drop(pump);
        writer
            .write_all(b"\"stream-secret\"\n]\nnext=visible")
            .await
            .unwrap();
        writer.shutdown().await.unwrap();
        drop(tx);
        stdout.await.unwrap();
        stderr.await.unwrap();
        wait_for_stream_end(store.log_path()).await;
        let stored = fs::read_to_string(store.log_path()).unwrap();
        assert!(stored.contains("next=visible") && stored.contains("visible-stderr"));
        assert!(!stored.contains("stream-secret") && !stored.contains("无法安全脱敏"));
        assert!(!capture.lock().unwrap().text().contains("stream-secret"));
        assert!(
            !serde_json::to_string(&*events.lock().unwrap())
                .unwrap()
                .contains("stream-secret")
        );
        assert!(
            export_report(&root.0, &root.0.join("report.zip"), "test")
                .unwrap()
                .complete
        );
    }

    #[tokio::test]
    async fn launcher_failure_before_pipes_does_not_poison_a_complete_report() {
        use minecraft_java_rs_core::launcher::events::LaunchEvent;
        let root = TemporaryDirectory(std::env::temp_dir().join(format!(
            "nickel-preparation-error-{}",
            crate::store::random_token()
        )));
        let store = crate::launch_log_store::LaunchLogStore::instance(&root.0);
        let (tx, pump, capture, _) = stream_pump(store.clone());
        tx.send(LaunchEvent::Data("preparing next=visible".into()))
            .await
            .unwrap();
        tx.send(LaunchEvent::Error(
            "Java could not start password=stream-secret".into(),
        ))
        .await
        .unwrap();
        drop(tx);
        pump.finish().await;
        store_failure_text(&root.0, FAILURE_FILE, "Java could not start").unwrap();
        assert!(!capture.lock().unwrap().text().contains("stream-secret"));
        let stored = fs::read_to_string(store.log_path()).unwrap();
        assert!(stored.contains("Java could not start") && stored.contains("next=visible"));
        assert!(!stored.contains("无法安全脱敏"));
        assert!(
            export_report(&root.0, &root.0.join("report.zip"), "test")
                .unwrap()
                .complete
        );
    }

    #[tokio::test]
    async fn drain_timeout_and_missing_ordered_eof_reject_incomplete_reports() {
        use minecraft_java_rs_core::launcher::events::{LaunchEvent, ProcessStream};
        for hold_sender in [false, true] {
            let root = TemporaryDirectory(std::env::temp_dir().join(format!(
                "nickel-missing-eof-{}",
                crate::store::random_token()
            )));
            let store = crate::launch_log_store::LaunchLogStore::instance(&root.0);
            let (tx, pump, capture, _) = stream_pump(store.clone());
            for line in ["password: |", "  stream-secret"] {
                tx.send(LaunchEvent::ProcessOutput {
                    stream: ProcessStream::Stdout,
                    line: line.into(),
                    discarded: false,
                })
                .await
                .unwrap();
            }
            tx.send(LaunchEvent::ProcessOutput {
                stream: ProcessStream::Stderr,
                line: "visible-stderr".into(),
                discarded: false,
            })
            .await
            .unwrap();
            tx.send(LaunchEvent::OutputClosed {
                stream: ProcessStream::Stderr,
                failed: false,
            })
            .await
            .unwrap();
            let held = if hold_sender {
                Some(tx)
            } else {
                drop(tx);
                None
            };
            tokio::time::timeout(std::time::Duration::from_secs(8), pump.finish())
                .await
                .unwrap();
            drop(held);
            assert!(crate::launch_output::ensure_finished(store.log_path()).is_ok());
            let stored = fs::read_to_string(store.log_path()).unwrap();
            assert!(stored.contains("无法安全脱敏") && stored.contains("visible-stderr"));
            assert!(!stored.contains("stream-secret"));
            assert!(!capture.lock().unwrap().text().contains("stream-secret"));
            let destination = root.0.join("report.zip");
            assert!(export_report(&root.0, &destination, "test").is_err());
            assert!(!destination.exists());
        }
    }

    #[tokio::test]
    async fn two_log_sessions_do_not_share_pending_boundaries() {
        use minecraft_java_rs_core::launcher::events::{LaunchEvent, ProcessStream};
        let a = TemporaryDirectory(
            std::env::temp_dir().join(format!("nickel-stream-a-{}", crate::store::random_token())),
        );
        let b = TemporaryDirectory(
            std::env::temp_dir().join(format!("nickel-stream-b-{}", crate::store::random_token())),
        );
        let store_a = crate::launch_log_store::LaunchLogStore::instance(&a.0);
        let (tx_a, pump_a, capture_a, _) = stream_pump(store_a.clone());
        let store_b = crate::launch_log_store::LaunchLogStore::instance(&b.0);
        let (tx_b, pump_b, _, _) = stream_pump(store_b.clone());
        tx_a.send(LaunchEvent::ProcessOutput {
            stream: ProcessStream::Stdout,
            line: "password=[".into(),
            discarded: false,
        })
        .await
        .unwrap();
        tx_b.send(LaunchEvent::ProcessOutput {
            stream: ProcessStream::Stdout,
            line: "next=visible-other-session".into(),
            discarded: false,
        })
        .await
        .unwrap();
        for stream in [ProcessStream::Stdout, ProcessStream::Stderr] {
            tx_b.send(LaunchEvent::OutputClosed {
                stream,
                failed: false,
            })
            .await
            .unwrap();
        }
        drop(tx_b);
        pump_b.finish().await;
        assert!(
            export_report(&b.0, &b.0.join("report.zip"), "test")
                .unwrap()
                .complete
        );
        for line in ["\"stream-secret\"", "]", "next=visible-a"] {
            tx_a.send(LaunchEvent::ProcessOutput {
                stream: ProcessStream::Stdout,
                line: line.into(),
                discarded: false,
            })
            .await
            .unwrap();
        }
        for stream in [ProcessStream::Stdout, ProcessStream::Stderr] {
            tx_a.send(LaunchEvent::OutputClosed {
                stream,
                failed: false,
            })
            .await
            .unwrap();
        }
        drop(tx_a);
        pump_a.finish().await;
        let stored_a = fs::read_to_string(store_a.log_path()).unwrap();
        assert!(stored_a.contains("next=visible-a"));
        assert!(!stored_a.contains("other-session") && !stored_a.contains("stream-secret"));
        assert!(!capture_a.lock().unwrap().text().contains("stream-secret"));
        assert!(
            export_report(&a.0, &a.0.join("report.zip"), "test")
                .unwrap()
                .complete
        );
    }

    #[test]
    fn redaction_failures_survive_caps_and_all_retained_rotations() {
        let root = TemporaryDirectory(std::env::temp_dir().join(format!(
            "nickel-failed-rotation-{}",
            crate::store::random_token()
        )));
        let store = crate::launch_log_store::LaunchLogStore::with_limits(&root.0, 1024, 1);
        store
            .append_safe_text(&format!(
                "{}\n{}",
                crate::launch_log_store::REDACTION_FAILURE,
                "visible ".repeat(1000)
            ))
            .unwrap();
        for _ in 0..10 {
            let stored = fs::read_to_string(store.log_path()).unwrap();
            assert!(stored.contains(crate::launch_log_store::REDACTION_FAILURE));
            assert!(stored.len() <= 1024);
            store
                .append_safe_text(&"visible-safe-output ".repeat(1000))
                .unwrap();
        }
        let destination = root.0.join("report.zip");
        assert!(export_report(&root.0, &destination, "test").is_err());
        assert!(!destination.exists());
    }

    #[tokio::test]
    async fn reader_byte_limits_and_io_errors_are_fail_closed_in_actual_storage() {
        use minecraft_java_rs_core::launcher::{events::ProcessStream, forward_process_output};
        use std::pin::Pin;
        use std::task::{Context, Poll};
        struct BrokenPipe(bool);
        impl tokio::io::AsyncRead for BrokenPipe {
            fn poll_read(
                mut self: Pin<&mut Self>,
                _: &mut Context<'_>,
                buffer: &mut tokio::io::ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                if self.0 {
                    Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "fixture read error",
                    )))
                } else {
                    self.0 = true;
                    buffer.put_slice(b"password: |\n  stream-secret");
                    Poll::Ready(Ok(()))
                }
            }
        }
        for scenario in 0..3 {
            let root = TemporaryDirectory(std::env::temp_dir().join(format!(
                "nickel-reader-limit-{}",
                crate::store::random_token()
            )));
            let store = crate::launch_log_store::LaunchLogStore::instance(&root.0);
            let (tx, pump, capture, events) = stream_pump(store.clone());
            if scenario == 2 {
                forward_process_output(BrokenPipe(false), tx.clone(), ProcessStream::Stdout).await;
            } else {
                let limit = crate::launch_log_store::STREAM_LINE_MAX_BYTES;
                let raw = if scenario == 0 {
                    format!("{}\nnext=visible\n", "x".repeat(limit))
                } else {
                    format!("{}\nstream-secret\n", "x".repeat(limit + 1))
                };
                forward_process_output(raw.as_bytes(), tx.clone(), ProcessStream::Stdout).await;
            }
            forward_process_output(&b"visible-stderr\n"[..], tx.clone(), ProcessStream::Stderr)
                .await;
            drop(tx);
            pump.finish().await;
            let stored = fs::read_to_string(store.log_path()).unwrap();
            assert!(stored.contains("visible-stderr"));
            assert!(!stored.contains("stream-secret"));
            assert!(!capture.lock().unwrap().text().contains("stream-secret"));
            assert!(
                !serde_json::to_string(&*events.lock().unwrap())
                    .unwrap()
                    .contains("stream-secret")
            );
            let destination = root.0.join("report.zip");
            if scenario == 0 {
                assert!(stored.contains("next=visible"));
                assert!(
                    export_report(&root.0, &destination, "test")
                        .unwrap()
                        .complete
                );
            } else {
                assert!(stored.contains("无法安全脱敏"));
                assert!(export_report(&root.0, &destination, "test").is_err());
                assert!(!destination.exists());
            }
        }
    }

    #[test]
    fn oversized_reports_fail_instead_of_silently_truncating() {
        let root = std::env::temp_dir().join(format!(
            "nickel-report-size-{}",
            crate::store::random_token()
        ));
        fs::create_dir_all(root.join("logs")).unwrap();
        File::create(root.join("logs/latest.log"))
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        let output = root.join("report.zip");
        assert!(export_report(&root, &output, "test").is_err());
        assert!(!output.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failure_without_game_logs_still_has_an_honest_manifest_and_zip_guidance() {
        let root = std::env::temp_dir().join(format!(
            "nickel-report-empty-{}",
            crate::store::random_token()
        ));
        fs::create_dir(&root).unwrap();
        let output = root.join("report.zip");
        let manifest = export_report(&root, &output, "Java could not start").unwrap();
        assert!(manifest.files.is_empty() && !manifest.notes.is_empty());
        let text = String::from_utf8(
            read_zip_entry(&output, "READ-ME.txt", 8192, &CancellationToken::new()).unwrap(),
        )
        .unwrap();
        assert!(text.contains("ZIP") && text.contains("截图"));
        let bytes = fs::read(&output).unwrap();
        assert!(export_report(&root, &output, "other").is_err());
        assert_eq!(fs::read(&output).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }
}
