//! 启动日志落盘、脱敏、轮转与有界读取。
//!
//! 本模块不读取 Tauri 全局状态：所有路径都由调用方传入的 `base` 推导。
//! 约定布局：
//! - `<base>/logs/nickel-launcher.log`：本启动器捕获的游戏输出
//! - `<base>/logs/latest.log`：Minecraft 自己写入的最新日志（只读）
//! - `<base>/crash-reports/`：Minecraft 崩溃报告目录（只读）
//!
//! 写入前统一脱敏；超过上限后按 `.1`..`.N` 轮转。权限、损坏或 IO 失败
//! 一律返回 `Err`，不 panic。`base` 可以是应用配置目录，也可以是实例根目录，
//! 由 runtime-core 根据日志归属选择。

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// 日志目录名，位于调用方给定的 base 之下。
pub const LOG_DIRECTORY: &str = "logs";
/// 启动器捕获日志的文件名。
pub const LOG_FILE_NAME: &str = "nickel-launcher.log";
/// Minecraft 自己维护的最新日志文件名。
pub const LATEST_GAME_LOG_NAME: &str = "latest.log";
/// Minecraft 崩溃报告目录名。
pub const CRASH_REPORT_DIRECTORY: &str = "crash-reports";

/// 单份启动日志的默认上限（4 MiB）。
pub const DEFAULT_MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;
/// 默认保留的历史日志份数。
pub const DEFAULT_MAX_ARCHIVES: usize = 3;
/// 默认读取尾部行数。
pub const DEFAULT_TAIL_LINES: usize = 2_000;
/// 默认读取尾部字节数。
pub const DEFAULT_TAIL_BYTES: usize = 256 * 1024;
/// 单次读取的硬上限，防止调用方误传超大值。
pub const MAX_READ_BYTES: usize = 8 * 1024 * 1024;
/// 单次读取的硬行数上限。
pub const MAX_READ_LINES: usize = 10_000;
/// 单次追加的硬上限，超出时只保留尾部。
pub const MAX_APPEND_BYTES: usize = 512 * 1024;

const MIN_LOG_BYTES: u64 = 1024;
const TRUNCATION_MARKER: &str = "[日志片段已截断]\n";
const REDACTION_MASK: &str = "***";
const REDACTION_FAILURE: &str = "[日志含无法安全脱敏的敏感字段]";

static LOG_WRITE_LOCK: Mutex<()> = Mutex::new(());

fn write_lock() -> MutexGuard<'static, ()> {
    LOG_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 与日志相关的全部可定位路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchLogPaths {
    pub base: PathBuf,
    pub directory: PathBuf,
    pub log_file: PathBuf,
    pub latest_game_log: PathBuf,
    pub crash_reports: PathBuf,
}

impl LaunchLogPaths {
    /// 返回轮转归档文件路径，例如 `nickel-launcher.log.1`。
    pub fn archive_file(&self, index: usize) -> PathBuf {
        let mut name = self
            .log_file
            .file_name()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| OsString::from(LOG_FILE_NAME));
        name.push(format!(".{index}"));
        self.log_file.with_file_name(name)
    }
}

/// 解析任意 base（应用配置目录或实例根目录）下的日志路径。
pub fn resolve_log_paths(base: impl AsRef<Path>) -> LaunchLogPaths {
    let base = base.as_ref().to_path_buf();
    let directory = base.join(LOG_DIRECTORY);
    LaunchLogPaths {
        log_file: directory.join(LOG_FILE_NAME),
        latest_game_log: directory.join(LATEST_GAME_LOG_NAME),
        crash_reports: base.join(CRASH_REPORT_DIRECTORY),
        base,
        directory,
    }
}

/// 应用配置目录下的日志路径。
pub fn app_log_paths(config_dir: impl AsRef<Path>) -> LaunchLogPaths {
    resolve_log_paths(config_dir)
}

/// 实例根目录下的日志路径。
pub fn instance_log_paths(instance_root: impl AsRef<Path>) -> LaunchLogPaths {
    resolve_log_paths(instance_root)
}

/// 有界读取的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredLog {
    pub path: PathBuf,
    pub text: String,
    /// 文件或行数超过读取上限时为 true。
    pub truncated: bool,
}

/// 启动日志存储句柄。只做文件操作，不触发任何 UI。
#[derive(Debug, Clone)]
pub struct LaunchLogStore {
    paths: LaunchLogPaths,
    max_bytes: u64,
    max_archives: usize,
}

impl LaunchLogStore {
    pub fn new(base: impl AsRef<Path>) -> Self {
        Self::with_limits(base, DEFAULT_MAX_LOG_BYTES, DEFAULT_MAX_ARCHIVES)
    }

    pub fn app(config_dir: impl AsRef<Path>) -> Self {
        Self::new(config_dir)
    }

    pub fn instance(instance_root: impl AsRef<Path>) -> Self {
        Self::new(instance_root)
    }

    /// `max_bytes` 至少为 1 KiB，`max_archives` 至少为 1，避免产生无界文件。
    pub fn with_limits(base: impl AsRef<Path>, max_bytes: u64, max_archives: usize) -> Self {
        Self {
            paths: resolve_log_paths(base),
            max_bytes: max_bytes.max(MIN_LOG_BYTES),
            max_archives: max_archives.max(1),
        }
    }

    pub fn paths(&self) -> &LaunchLogPaths {
        &self.paths
    }

    pub fn log_path(&self) -> &Path {
        &self.paths.log_file
    }

    /// 交给前端或 opener 插件打开目录时使用；本模块不执行打开动作。
    pub fn directory_to_open(&self) -> &Path {
        &self.paths.directory
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub fn max_archives(&self) -> usize {
        self.max_archives
    }

    /// 脱敏后追加一段文本。内容为空时直接返回成功。
    pub fn append_text(&self, text: &str) -> Result<(), String> {
        let redacted = redact_sensitive_text(text);
        self.append_redacted(&redacted)
    }

    /// 按行追加；行之间补 `\n`。
    pub fn append_lines(&self, lines: &[String]) -> Result<(), String> {
        if lines.is_empty() {
            return Ok(());
        }
        self.append_text(&lines.join("\n"))
    }

    fn append_redacted(&self, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }

        let _guard = write_lock();
        fs::create_dir_all(&self.paths.directory).map_err(|error| {
            format!(
                "创建日志目录失败 {}：{error}",
                self.paths.directory.display()
            )
        })?;

        let normalized = ensure_trailing_newline(&normalize_newlines(text));
        let append_limit = self.max_bytes.min(MAX_APPEND_BYTES as u64).max(1) as usize;
        let append = cap_append_bytes(&normalized, append_limit);

        let current_len = match fs::metadata(&self.paths.log_file) {
            Ok(metadata) => {
                if !metadata.is_file() {
                    return Err(format!(
                        "日志路径不是普通文件：{}",
                        self.paths.log_file.display()
                    ));
                }
                metadata.len()
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => {
                return Err(format!(
                    "读取日志文件信息失败 {}：{error}",
                    self.paths.log_file.display()
                ));
            }
        };

        if current_len.saturating_add(append.len() as u64) > self.max_bytes {
            self.rotate_locked()?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.paths.log_file)
            .map_err(|error| {
                format!(
                    "打开日志文件失败 {}：{error}",
                    self.paths.log_file.display()
                )
            })?;
        file.write_all(append.as_bytes()).map_err(|error| {
            format!(
                "写入日志文件失败 {}：{error}",
                self.paths.log_file.display()
            )
        })
    }

    fn rotate_locked(&self) -> Result<(), String> {
        let metadata = match fs::metadata(&self.paths.log_file) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(format!(
                    "读取日志文件信息失败 {}：{error}",
                    self.paths.log_file.display()
                ));
            }
        };
        if !metadata.is_file() {
            return Err(format!(
                "日志路径不是普通文件：{}",
                self.paths.log_file.display()
            ));
        }
        if metadata.len() == 0 {
            return Ok(());
        }

        let oldest = self.paths.archive_file(self.max_archives);
        remove_if_exists(&oldest)?;

        for index in (1..self.max_archives).rev() {
            let from = self.paths.archive_file(index);
            let to = self.paths.archive_file(index + 1);
            if !from.exists() {
                continue;
            }
            remove_if_exists(&to)?;
            fs::rename(&from, &to).map_err(|error| {
                format!(
                    "轮转日志失败 {} -> {}：{error}",
                    from.display(),
                    to.display()
                )
            })?;
        }

        let first = self.paths.archive_file(1);
        remove_if_exists(&first)?;
        fs::rename(&self.paths.log_file, &first).map_err(|error| {
            format!(
                "轮转日志失败 {} -> {}：{error}",
                self.paths.log_file.display(),
                first.display()
            )
        })
    }

    /// 读取本启动器日志尾部，默认 2000 行 / 256 KiB。
    pub fn read_tail(&self, max_lines: usize) -> Result<Vec<String>, String> {
        self.read_tail_limited(max_lines, DEFAULT_TAIL_BYTES)
    }

    /// 读取本启动器日志尾部；两个上限都会被夹到硬上限内。
    pub fn read_tail_limited(
        &self,
        max_lines: usize,
        max_bytes: usize,
    ) -> Result<Vec<String>, String> {
        let max_lines = max_lines.min(MAX_READ_LINES);
        if max_lines == 0 {
            return Ok(Vec::new());
        }
        let max_bytes = max_bytes.clamp(1024, MAX_READ_BYTES);
        match read_path_tail(&self.paths.log_file, max_bytes, max_lines)? {
            Some(log) => Ok(log.text.lines().map(str::to_owned).collect()),
            None => Ok(Vec::new()),
        }
    }

    /// 读取 `<base>/logs/latest.log` 尾部；文件不存在返回 `Ok(None)`。
    pub fn read_latest_game_log(&self, max_bytes: usize) -> Result<Option<StoredLog>, String> {
        let max_bytes = max_bytes.clamp(1024, MAX_READ_BYTES);
        read_path_tail(&self.paths.latest_game_log, max_bytes, MAX_READ_LINES)
    }

    /// 读取最新的崩溃报告；目录不存在返回 `Ok(None)`。
    pub fn read_latest_crash_report(&self, max_bytes: usize) -> Result<Option<StoredLog>, String> {
        let max_bytes = max_bytes.clamp(1024, MAX_READ_BYTES);
        for path in crash_report_paths(&self.paths.crash_reports)? {
            if let Some(log) = read_path_head_tail(&path, max_bytes)? {
                return Ok(Some(log));
            }
        }
        Ok(None)
    }

    /// 按修改时间倒序读取最多 `limit` 份崩溃报告。
    pub fn read_crash_reports(
        &self,
        limit: usize,
        max_bytes: usize,
    ) -> Result<Vec<StoredLog>, String> {
        let limit = limit.min(16);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let max_bytes = max_bytes.clamp(1024, MAX_READ_BYTES);
        let mut logs = Vec::new();
        for path in crash_report_paths(&self.paths.crash_reports)?
            .into_iter()
            .take(limit)
        {
            if let Some(log) = read_path_head_tail(&path, max_bytes)? {
                logs.push(log);
            }
        }
        Ok(logs)
    }
}

fn normalize_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\0' => {}
            '\n' | '\t' => out.push(ch),
            other if other.is_control() => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

fn ensure_trailing_newline(text: &str) -> String {
    if text.ends_with('\n') {
        text.to_string()
    } else {
        let mut out = String::with_capacity(text.len() + 1);
        out.push_str(text);
        out.push('\n');
        out
    }
}

fn cap_append_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let budget = max_bytes.saturating_sub(TRUNCATION_MARKER.len()).max(1);
    let mut out = String::with_capacity(TRUNCATION_MARKER.len() + budget);
    out.push_str(TRUNCATION_MARKER);
    out.push_str(tail_at_char_boundary(text, budget));
    out
}

fn tail_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn remove_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("删除日志归档失败 {}：{error}", path.display())),
    }
}

fn read_path_tail(
    path: &Path,
    max_bytes: usize,
    max_lines: usize,
) -> Result<Option<StoredLog>, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!("打开日志文件失败 {}：{error}", path.display()));
        }
    };
    let length = file
        .metadata()
        .map_err(|error| format!("读取日志文件信息失败 {}：{error}", path.display()))?
        .len();
    let start = length.saturating_sub(max_bytes as u64);
    file.seek(SeekFrom::Start(start))
        .map_err(|error| format!("定位日志文件失败 {}：{error}", path.display()))?;

    let mut buffer = Vec::with_capacity((length - start).min(max_bytes as u64) as usize);
    file.take(max_bytes as u64)
        .read_to_end(&mut buffer)
        .map_err(|error| format!("读取日志文件失败 {}：{error}", path.display()))?;

    let mut text = String::from_utf8_lossy(&buffer).into_owned();
    if start > 0 {
        // 从中间截断时首行可能是半个字符 / 半行，丢弃它以免误导证据行号。
        if let Some(newline) = text.find('\n') {
            text.drain(..=newline);
        } else {
            text.clear();
        }
    }

    let line_count = text.lines().count();
    let line_truncated = line_count > max_lines;
    if line_truncated {
        let keep_from = line_count - max_lines;
        text = text.lines().skip(keep_from).collect::<Vec<_>>().join("\n");
    }

    Ok(Some(StoredLog {
        path: path.to_path_buf(),
        text,
        truncated: start > 0 || line_truncated,
    }))
}

fn read_path_head_tail(path: &Path, max_bytes: usize) -> Result<Option<StoredLog>, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!("打开日志文件失败 {}：{error}", path.display()));
        }
    };
    let length = file
        .metadata()
        .map_err(|error| format!("读取日志文件信息失败 {}：{error}", path.display()))?
        .len();
    if length <= max_bytes as u64 {
        let mut buffer = Vec::with_capacity(length as usize);
        file.read_to_end(&mut buffer)
            .map_err(|error| format!("读取日志文件失败 {}：{error}", path.display()))?;
        return Ok(Some(StoredLog {
            path: path.to_path_buf(),
            text: String::from_utf8_lossy(&buffer).into_owned(),
            truncated: false,
        }));
    }

    let marker = "\n[...日志已截断...]\n";
    let budget = max_bytes.saturating_sub(marker.len()).max(1);
    let head_len = budget / 3;
    let tail_len = budget - head_len;
    let mut head = vec![0_u8; head_len];
    file.read_exact(&mut head)
        .map_err(|error| format!("读取日志文件失败 {}：{error}", path.display()))?;

    let mut tail = vec![0_u8; tail_len];
    file.seek(SeekFrom::Start(length - tail_len as u64))
        .map_err(|error| format!("定位日志文件失败 {}：{error}", path.display()))?;
    file.read_exact(&mut tail)
        .map_err(|error| format!("读取日志文件失败 {}：{error}", path.display()))?;

    let mut text = String::with_capacity(marker.len() + head_len + tail_len);
    text.push_str(&String::from_utf8_lossy(&head));
    text.push_str(marker);
    text.push_str(&String::from_utf8_lossy(&tail));
    Ok(Some(StoredLog {
        path: path.to_path_buf(),
        text,
        truncated: true,
    }))
}

fn crash_report_paths(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "读取崩溃报告目录失败 {}：{error}",
                directory.display()
            ));
        }
    };

    let mut files = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_file() || !is_crash_report_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        files.push(entry.path());
    }

    files.sort_by(|left, right| {
        let left_modified = fs::metadata(left)
            .and_then(|metadata| metadata.modified())
            .ok();
        let right_modified = fs::metadata(right)
            .and_then(|metadata| metadata.modified())
            .ok();
        right_modified
            .cmp(&left_modified)
            .then_with(|| right.file_name().cmp(&left.file_name()))
    });
    Ok(files)
}

fn is_crash_report_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("crash-") || lower.ends_with(".txt") || lower.ends_with(".log")
}

// ─────────────────────────────────────────────────────────────────────────────
// 脱敏
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaskKind {
    Full,
    Token,
    Collection,
}

#[derive(Debug, Clone, Copy)]
struct RedactionSpan {
    start: usize,
    end: usize,
    kind: MaskKind,
}

const MAX_REDACTION_SPANS: usize = 262_144;
const MAX_COLLECTION_BYTES: usize = 1024 * 1024;
const MAX_COLLECTION_DEPTH: usize = 64;
const MAX_KEY_BYTES: usize = 256;

#[derive(Debug)]
enum ScanFailure {
    Incomplete,
    Invalid(&'static str),
}
impl ScanFailure {
    fn message(&self) -> String {
        match self {
            Self::Incomplete => "日志敏感字段未闭合，无法安全生成完整报告".into(),
            Self::Invalid(message) => (*message).into(),
        }
    }
}

/// Complete documents and streaming sessions share the same field boundaries.
/// Uncertain value boundaries fail closed rather than releasing a continuation.
pub fn redact_sensitive_text(text: &str) -> String {
    redact_sensitive_text_checked(text).unwrap_or_else(|_| REDACTION_FAILURE.into())
}
pub(crate) fn redact_sensitive_text_checked(text: &str) -> Result<String, String> {
    redact_document(text, true).map_err(|error| error.message())
}
fn redact_document(text: &str, final_input: bool) -> Result<String, ScanFailure> {
    if text.contains(REDACTION_FAILURE) {
        return Err(ScanFailure::Invalid(
            "日志包含先前无法安全脱敏的片段，无法生成完整报告",
        ));
    }
    let mut spans = Vec::new();
    collect_key_value_spans(text, &mut spans, final_input)?;
    collect_user_path_spans(text, &mut spans)
        .map_err(|_| ScanFailure::Invalid("日志敏感字段数量超过安全处理上限"))?;
    collect_invite_path_spans(text, &mut spans)
        .map_err(|_| ScanFailure::Invalid("日志敏感字段数量超过安全处理上限"))?;
    collect_url_userinfo_spans(text, &mut spans)
        .map_err(|_| ScanFailure::Invalid("日志敏感字段数量超过安全处理上限"))?;
    Ok(apply_redaction_spans(text, spans))
}
fn push_redaction_span(spans: &mut Vec<RedactionSpan>, span: RedactionSpan) -> Result<(), String> {
    if spans.len() >= MAX_REDACTION_SPANS {
        return Err("日志敏感字段数量超过安全处理上限".into());
    }
    spans.push(span);
    Ok(())
}

fn log_key_kind(key: &str) -> Option<MaskKind> {
    if key.len() > MAX_KEY_BYTES {
        return None;
    }
    let folded: String = key
        .bytes()
        .filter(|b| !matches!(b, b'_' | b'-'))
        .map(|b| (b as char).to_ascii_lowercase())
        .collect();
    match folded.as_str() {
        "accesstoken" | "clienttoken" | "refreshtoken" | "sessiontoken" | "authtoken"
        | "bearertoken" | "apitoken" | "token" => Some(MaskKind::Token),
        "password" | "passwords" | "passwd" | "pwd" | "secret" | "secrets" | "clientsecret"
        | "authorization" | "invitecode" | "invitationcode" | "apikey" | "apikeys" | "xapikey"
        | "privatekey" | "secretkey" => Some(MaskKind::Full),
        _ => None,
    }
}
fn key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}
fn skip_spaces(text: &str, mut index: usize) -> usize {
    while matches!(text.as_bytes().get(index), Some(b' ' | b'\t')) {
        index += 1;
    }
    index
}
fn skip_field_whitespace(text: &str, mut index: usize) -> usize {
    while text
        .as_bytes()
        .get(index)
        .is_some_and(u8::is_ascii_whitespace)
    {
        index += 1;
    }
    index
}
fn scan_line_end(text: &str, start: usize) -> usize {
    text.as_bytes()[start..]
        .iter()
        .position(|b| matches!(b, b'\r' | b'\n'))
        .map_or(text.len(), |n| start + n)
}

fn quoted_key(text: &str, start: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let delimiter = bytes[start];
    let mut escaped = false;
    for end in start + 1..bytes.len().min(start + MAX_KEY_BYTES + 2) {
        let byte = bytes[end];
        if matches!(byte, b'\r' | b'\n') {
            return None;
        }
        if byte == delimiter && !escaped {
            let raw = &text[start..=end];
            let key = if delimiter == b'"' {
                serde_json::from_str::<String>(raw).ok()?
            } else {
                let inner = &text[start + 1..end];
                serde_json::from_str::<String>(&format!(
                    "\"{}\"",
                    inner.replace("\\'", "'").replace('"', "\\\"")
                ))
                .ok()?
            };
            return Some((key, end + 1));
        }
        escaped = byte == b'\\' && !escaped;
    }
    None
}

fn collect_key_value_spans(
    text: &str,
    spans: &mut Vec<RedactionSpan>,
    final_input: bool,
) -> Result<(), ScanFailure> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let key_start = index;
        let (key, after_key, quoted) = if matches!(bytes[index], b'"' | b'\'') {
            if let Some((key, end)) = quoted_key(text, index) {
                (key, end, true)
            } else {
                index += 1;
                continue;
            }
        } else if key_byte(bytes[index]) {
            let start = index;
            while index < bytes.len() && key_byte(bytes[index]) {
                index += 1;
            }
            if index - start > MAX_KEY_BYTES {
                continue;
            }
            (text[start..index].to_owned(), index, false)
        } else {
            index += 1;
            continue;
        };
        let separator = skip_field_whitespace(text, after_key);
        let explicit = matches!(bytes.get(separator), Some(b':' | b'='));
        // A quoted diagnostic message can itself contain an assignment. Only
        // consume it as an opaque key when its closing quote has a separator.
        index = if quoted && !explicit {
            key_start + 1
        } else {
            after_key
        };
        let cli = !quoted
            && key.starts_with("--")
            && key.len() > 2
            && (key_start == 0 || bytes[key_start - 1].is_ascii_whitespace());
        let Some(kind) = log_key_kind(key.trim_start_matches('-')) else {
            continue;
        };
        if !explicit && !cli {
            if !final_input
                && separator == text.len()
                && (quoted || text[..key_start].trim().is_empty())
            {
                return Err(ScanFailure::Incomplete);
            }
            continue;
        }
        if explicit && matches!(bytes.get(separator + 1), Some(b'=' | b'>')) {
            continue;
        }
        let start = if explicit {
            skip_field_whitespace(text, separator + 1)
        } else {
            skip_spaces(text, after_key)
        };
        if start == text.len() {
            if !final_input {
                return Err(ScanFailure::Incomplete);
            }
            continue;
        }
        let block_value = explicit
            && bytes[separator] == b':'
            && !matches!(
                bytes[start],
                b'[' | b'{' | b'"' | b'\'' | b'`' | b'|' | b'>'
            )
            && (text[separator + 1..start].contains('\n')
                && line_indent(text, start) > line_indent(text, key_start)
                || yaml_field_start(text, key_start));
        let (span, consumed) = if block_value {
            let end = scan_indented_end(
                text,
                key_start,
                start,
                scan_line_end(text, start),
                final_input,
            )?;
            (
                RedactionSpan {
                    start,
                    end,
                    kind: MaskKind::Collection,
                },
                end,
            )
        } else {
            scan_sensitive_value(
                text,
                key_start,
                start,
                kind,
                key.trim_start_matches('-')
                    .eq_ignore_ascii_case("authorization"),
                final_input,
            )?
        };
        index = consumed.max(start + 1);
        if span.end > span.start {
            push_redaction_span(spans, span)
                .map_err(|_| ScanFailure::Invalid("日志敏感字段数量超过安全处理上限"))?;
        }
    }
    Ok(())
}

fn yaml_field_start(text: &str, key_start: usize) -> bool {
    let line_start = text[..key_start].rfind('\n').map_or(0, |n| n + 1);
    matches!(text[line_start..key_start].trim(), "" | "-")
}

fn scan_sensitive_value(
    text: &str,
    key_start: usize,
    start: usize,
    kind: MaskKind,
    authorization: bool,
    final_input: bool,
) -> Result<(RedactionSpan, usize), ScanFailure> {
    let bytes = text.as_bytes();
    if matches!(bytes[start], b'[' | b'{') {
        let end = scan_collection_end(text, start)?;
        return Ok((
            RedactionSpan {
                start,
                end,
                kind: MaskKind::Collection,
            },
            end,
        ));
    }
    if matches!(bytes[start], b'"' | b'\'' | b'`') {
        let width = if bytes[start] != b'`'
            && bytes
                .get(start..start + 3)
                .is_some_and(|s| s.iter().all(|b| *b == bytes[start]))
        {
            3
        } else {
            1
        };
        let (inner_end, end) = scan_quote_end(text, start, width)?;
        if width == 3 || text[start..end].contains(['\n', '\r']) {
            return Ok((
                RedactionSpan {
                    start,
                    end,
                    kind: MaskKind::Collection,
                },
                end,
            ));
        }
        return Ok((
            RedactionSpan {
                start: start + 1,
                end: inner_end,
                kind,
            },
            end,
        ));
    }
    if matches!(bytes[start], b'|' | b'>') {
        let line_end = scan_line_end(text, start);
        let header = text[start + 1..line_end]
            .split('#')
            .next()
            .unwrap_or("")
            .trim();
        if header
            .bytes()
            .all(|b| matches!(b, b'+' | b'-' | b'1'..=b'9'))
        {
            let end = scan_indented_end(text, key_start, start, line_end, final_input)?;
            return Ok((
                RedactionSpan {
                    start,
                    end,
                    kind: MaskKind::Collection,
                },
                end,
            ));
        }
    }
    let end = if authorization
        && text[start..]
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("bearer "))
    {
        scan_value_end(text, skip_spaces(text, start + 7))
    } else if authorization {
        scan_line_end(text, start)
    } else {
        scan_value_end(text, start)
    };
    Ok((RedactionSpan { start, end, kind }, end))
}

fn line_indent(text: &str, position: usize) -> usize {
    let start = text[..position].rfind('\n').map_or(0, |n| n + 1);
    text.as_bytes()[start..]
        .iter()
        .take_while(|b| matches!(b, b' ' | b'\t'))
        .count()
}

fn scan_indented_end(
    text: &str,
    key_start: usize,
    value_start: usize,
    first_line_end: usize,
    final_input: bool,
) -> Result<usize, ScanFailure> {
    if first_line_end - value_start > MAX_COLLECTION_BYTES {
        return Err(ScanFailure::Invalid("日志敏感字段超过安全处理大小"));
    }
    let bytes = text.as_bytes();
    let indent = line_indent(text, key_start);
    let mut cursor = first_line_end;
    let mut value_end = first_line_end;
    while cursor < bytes.len() {
        while matches!(bytes.get(cursor), Some(b'\r' | b'\n')) {
            cursor += 1;
        }
        if cursor == bytes.len() {
            break;
        }
        let end = scan_line_end(text, cursor);
        let content = skip_spaces(text, cursor);
        if content < end && content - cursor <= indent {
            return Ok(value_end);
        }
        if end - value_start > MAX_COLLECTION_BYTES {
            return Err(ScanFailure::Invalid("日志敏感字段超过安全处理大小"));
        }
        value_end = end;
        cursor = end;
    }
    if !final_input {
        return Err(ScanFailure::Incomplete);
    }
    Ok(value_end)
}

fn scan_quote_end(text: &str, start: usize, width: usize) -> Result<(usize, usize), ScanFailure> {
    let bytes = text.as_bytes();
    let quote = bytes[start];
    let mut index = start + width;
    let mut escaped = false;
    while index < bytes.len() {
        if index - start >= MAX_COLLECTION_BYTES {
            return Err(ScanFailure::Invalid("日志敏感字段超过安全处理大小"));
        }
        if !escaped
            && bytes[index] == quote
            && bytes
                .get(index..index + width)
                .is_some_and(|s| s.iter().all(|b| *b == quote))
        {
            let mut end = index + width;
            if width == 3 {
                while end < bytes.len() && bytes[end] == quote {
                    end += 1;
                    if end - start > MAX_COLLECTION_BYTES {
                        return Err(ScanFailure::Invalid("日志敏感字段超过安全处理大小"));
                    }
                }
            } else if quote == b'\'' && bytes.get(end) == Some(&quote) {
                index = end + 1;
                continue;
            }
            if end - start > MAX_COLLECTION_BYTES {
                return Err(ScanFailure::Invalid("日志敏感字段超过安全处理大小"));
            }
            return Ok((index, end));
        }
        escaped = bytes[index] == b'\\' && !escaped && !(quote == b'\'' && width == 3);
        index += 1;
    }
    Err(ScanFailure::Incomplete)
}

fn scan_collection_end(text: &str, start: usize) -> Result<usize, ScanFailure> {
    let bytes = text.as_bytes();
    let mut stack = Vec::new();
    let mut index = start;
    while index < bytes.len() {
        if index - start >= MAX_COLLECTION_BYTES {
            return Err(ScanFailure::Invalid("日志敏感集合超过安全处理大小"));
        }
        let byte = bytes[index];
        if matches!(byte, b'"' | b'\'' | b'`') {
            let width = if byte != b'`'
                && bytes
                    .get(index..index + 3)
                    .is_some_and(|s| s.iter().all(|b| *b == byte))
            {
                3
            } else {
                1
            };
            index = scan_quote_end(text, index, width)?.1;
            if index - start > MAX_COLLECTION_BYTES {
                return Err(ScanFailure::Invalid("日志敏感集合超过安全处理大小"));
            }
            continue;
        }
        // Comment markers are only comments at token boundaries; URLs and
        // unquoted map values may contain // or # as ordinary data.
        let comment_boundary = index == start
            || bytes[index - 1].is_ascii_whitespace()
            || matches!(bytes[index - 1], b'[' | b'{' | b',');
        if comment_boundary && (byte == b'#' || bytes[index..].starts_with(b"//")) {
            index = scan_line_end(text, index);
            continue;
        }
        if comment_boundary && bytes[index..].starts_with(b"/*") {
            let end = text[index + 2..]
                .find("*/")
                .ok_or(ScanFailure::Incomplete)?;
            index += end + 4;
            if index - start > MAX_COLLECTION_BYTES {
                return Err(ScanFailure::Invalid("日志敏感集合超过安全处理大小"));
            }
            continue;
        }
        match byte {
            b'[' | b'{' => {
                if stack.len() >= MAX_COLLECTION_DEPTH {
                    return Err(ScanFailure::Invalid("日志敏感集合嵌套超过安全处理深度"));
                }
                stack.push(if byte == b'[' { b']' } else { b'}' });
            }
            b']' | b'}' => {
                if stack.pop() != Some(byte) {
                    return Err(ScanFailure::Invalid(
                        "日志敏感集合括号不匹配，无法安全生成完整报告",
                    ));
                }
                if stack.is_empty() {
                    return Ok(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    Err(ScanFailure::Incomplete)
}
fn scan_value_end(text: &str, start: usize) -> usize {
    let mut index = start;
    while let Some(byte) = text.as_bytes().get(index).copied() {
        if byte.is_ascii_whitespace() || b"\"'`,;&}])<>".contains(&byte) {
            break;
        }
        index += 1;
    }
    index
}

fn collect_user_path_spans(text: &str, spans: &mut Vec<RedactionSpan>) -> Result<(), String> {
    const MARKERS: &[&str] = &[
        "\\\\Users\\\\",
        "\\\\Documents and Settings\\\\",
        "\\Users\\",
        "/Users/",
        "/home/",
        "\\Documents and Settings\\",
    ];
    for marker in MARKERS {
        let mut from = 0;
        while let Some(position) = find_ascii_insensitive(text, marker, from) {
            let start = position + marker.len();
            let end = scan_path_segment_end(text, start);
            if end > start && &text[start..end] != REDACTION_MASK {
                push_redaction_span(
                    spans,
                    RedactionSpan {
                        start,
                        end,
                        kind: MaskKind::Full,
                    },
                )?;
            }
            from = end.max(position + marker.len());
        }
    }
    Ok(())
}

fn scan_path_segment_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = start;
    while let Some(byte) = bytes.get(index).copied() {
        if matches!(byte, b'\\' | b'/' | b'"' | b'\'' | b'\n' | b'\r') {
            break;
        }
        index += 1;
    }
    index
}

fn collect_invite_path_spans(text: &str, spans: &mut Vec<RedactionSpan>) -> Result<(), String> {
    for marker in ["/invite/", "/join/"] {
        let mut from = 0;
        while let Some(position) = find_ascii_insensitive(text, marker, from) {
            let start = position + marker.len();
            let end = scan_invite_segment_end(text, start);
            if end > start && &text[start..end] != REDACTION_MASK {
                push_redaction_span(
                    spans,
                    RedactionSpan {
                        start,
                        end,
                        kind: MaskKind::Full,
                    },
                )?;
            }
            from = end.max(position + marker.len());
        }
    }
    Ok(())
}

fn scan_invite_segment_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = start;
    while let Some(byte) = bytes.get(index).copied() {
        if byte.is_ascii_whitespace()
            || matches!(
                byte,
                b'/' | b'?' | b'#' | b'"' | b'\'' | b'&' | b')' | b']' | b'}'
            )
        {
            break;
        }
        index += 1;
    }
    index
}

fn collect_url_userinfo_spans(text: &str, spans: &mut Vec<RedactionSpan>) -> Result<(), String> {
    let mut from = 0;
    while let Some(position) = find_ascii_insensitive(text, "://", from) {
        let authority_start = position + 3;
        let authority_end = scan_authority_end(text, authority_start);
        if let Some(userinfo_end) = text[authority_start..authority_end].rfind('@') {
            let at = authority_start + userinfo_end;
            if text[authority_start..at].contains(':') {
                push_redaction_span(
                    spans,
                    RedactionSpan {
                        start: authority_start,
                        end: at,
                        kind: MaskKind::Full,
                    },
                )?;
            }
        }
        from = authority_end.max(authority_start);
    }
    Ok(())
}

fn scan_authority_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = start;
    while let Some(byte) = bytes.get(index).copied() {
        if byte.is_ascii_whitespace() || matches!(byte, b'/' | b'?' | b'#' | b'"' | b'\'') {
            break;
        }
        index += 1;
    }
    index
}

fn find_ascii_insensitive(text: &str, needle: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() || from >= bytes.len() || bytes.len() < needle.len() {
        return None;
    }
    let mut index = from;
    while index + needle.len() <= bytes.len() {
        if bytes[index..index + needle.len()].eq_ignore_ascii_case(needle) {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn apply_redaction_spans(text: &str, mut spans: Vec<RedactionSpan>) -> String {
    spans.retain(|span| {
        span.start < span.end
            && span.end <= text.len()
            && text.is_char_boundary(span.start)
            && text.is_char_boundary(span.end)
    });
    spans.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| left.end.cmp(&right.end))
    });

    let mut merged: Vec<RedactionSpan> = Vec::new();
    for span in spans {
        if let Some(last) = merged.last_mut() {
            if span.start <= last.end {
                last.end = last.end.max(span.end);
                last.kind = merge_mask_kind(last.kind, span.kind);
                continue;
            }
        }
        merged.push(span);
    }

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for span in merged {
        if span.start > cursor {
            out.push_str(&text[cursor..span.start]);
        }
        out.push_str(&mask_value(&text[span.start..span.end], span.kind));
        cursor = span.end;
    }
    out.push_str(&text[cursor..]);
    out
}

fn merge_mask_kind(left: MaskKind, right: MaskKind) -> MaskKind {
    if left == MaskKind::Collection || right == MaskKind::Collection {
        MaskKind::Collection
    } else if left == MaskKind::Full || right == MaskKind::Full {
        MaskKind::Full
    } else {
        MaskKind::Token
    }
}

fn mask_value(value: &str, kind: MaskKind) -> String {
    if kind == MaskKind::Collection {
        // A string replacement is valid in JSON/YAML and leaks no token edges.
        return format!("\"{REDACTION_MASK}\"");
    }
    if kind == MaskKind::Full {
        return REDACTION_MASK.to_string();
    }
    let char_count = value.chars().count();
    if char_count < 10 {
        return REDACTION_MASK.to_string();
    }
    let first: String = value.chars().take(4).collect();
    let last: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{first}{REDACTION_MASK}{last}")
}

#[cfg(test)]
mod collection_redaction_tests {
    use super::*;

    #[test]
    fn collection_limits_are_inclusive_and_fail_closed() {
        let within = format!(
            "password={}\"bounded-secret\"{} next=visible",
            "[".repeat(MAX_COLLECTION_DEPTH),
            "]".repeat(MAX_COLLECTION_DEPTH)
        );
        assert_eq!(
            redact_sensitive_text_checked(&within).unwrap(),
            "password=\"***\" next=visible"
        );
        let oversized_depth = format!(
            "password={}{}",
            "[".repeat(MAX_COLLECTION_DEPTH + 1),
            "]".repeat(MAX_COLLECTION_DEPTH + 1)
        );
        assert!(redact_sensitive_text_checked(&oversized_depth).is_err());
        let exact_size = format!(
            "password=[{}] next=visible",
            " ".repeat(MAX_COLLECTION_BYTES - 2)
        );
        assert_eq!(
            redact_sensitive_text_checked(&exact_size).unwrap(),
            "password=\"***\" next=visible"
        );
        let over_size = format!("password=[{}]", " ".repeat(MAX_COLLECTION_BYTES - 1));
        assert!(redact_sensitive_text_checked(&over_size).is_err());
        let exact_spans = "password=x\n".repeat(MAX_REDACTION_SPANS);
        assert_eq!(
            redact_sensitive_text_checked(&exact_spans).unwrap(),
            "password=***\n".repeat(MAX_REDACTION_SPANS)
        );
        let too_many = format!("{exact_spans}password=x\n");
        assert!(redact_sensitive_text_checked(&too_many).is_err());
        assert!(!redact_sensitive_text(&too_many).contains("password=x"));
    }

    #[test]
    fn collection_contents_never_restart_key_scanning() {
        for raw in [
            r#"{"password":["apiKey=[not syntax", "bounded-secret"],"next":"visible"}"#,
            r#"{"password":[{"authorization":"Basic bounded-secret"}],"next":"visible"}"#,
            r#"{"apiKey":["C:\\Users\\PrivateName\\game", "bounded-secret"],"next":"visible"}"#,
            r#"{"password":["https://user:bounded-secret@example.com"],"next":"visible"}"#,
        ] {
            let redacted = redact_sensitive_text_checked(raw).unwrap();
            assert!(!redacted.contains("bounded-secret"));
            let value: serde_json::Value = serde_json::from_str(&redacted).unwrap();
            assert_eq!(value["next"], "visible");
            assert_eq!(redact_sensitive_text(&redacted), redacted);
        }
    }
}
