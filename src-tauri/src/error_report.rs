//! 完整错误报告归档：只收集诊断及日志，逐行脱敏后写入 ZIP。
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use minecraft_java_rs_core::net::transfer::CancellationToken;
use minecraft_java_rs_core::utils::archive::{ArchiveLimits, write_zip_files};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::launch_diagnostics::DiagnosticReport;
use crate::launch_log_store::{DEFAULT_MAX_ARCHIVES, LOG_FILE_NAME, redact_sensitive_text};

const DIAGNOSTIC_FILE: &str = "nickel-diagnostic.json";
const FAILURE_FILE: &str = "nickel-launch-failure.txt";
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FILES: usize = 128;
const MAX_LINE_BYTES: usize = 1024 * 1024;

pub fn begin_session(directory: &Path) -> Result<(), String> {
    let logs = directory.join("logs");
    crate::instances::reject_links(&logs)?;
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
    fs::create_dir_all(destination.parent().ok_or("报告文件缺少目录")?)
        .map_err(|_| "无法创建报告临时目录")?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| "无法写入报告临时文件")?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut read = 0_u64;
    loop {
        let buffer = reader.fill_buf().map_err(|_| "无法读取日志内容")?;
        if buffer.is_empty() {
            if !line.is_empty() {
                output
                    .write_all(redact_sensitive_text(&String::from_utf8_lossy(&line)).as_bytes())
                    .map_err(|_| "无法写入完整报告")?;
            }
            break;
        }
        let count = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |index| index + 1);
        let ended = buffer[count - 1] == b'\n';
        if line.len() + count > MAX_LINE_BYTES {
            return Err("日志单行过长，无法安全生成完整报告".into());
        }
        line.extend_from_slice(&buffer[..count]);
        reader.consume(count);
        read += count as u64;
        if read > MAX_FILE_BYTES {
            return Err("日志在导出时持续增长，请等待游戏退出后重试".into());
        }
        if ended {
            output
                .write_all(redact_sensitive_text(&String::from_utf8_lossy(&line)).as_bytes())
                .map_err(|_| "无法写入完整报告")?;
            line.clear();
        }
    }
    if read != expected {
        return Err("日志在导出期间发生变化，请等待游戏退出后重试".into());
    }
    crate::instances::reject_links(source)?;
    if fs::metadata(source).map_err(|_| "日志已不可读取")?.len() != expected {
        return Err("日志在导出期间发生变化，请重试".into());
    }
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
    let failure = redact_sensitive_text(failure);
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
