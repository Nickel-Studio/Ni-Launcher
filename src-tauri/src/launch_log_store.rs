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
pub(crate) const REDACTION_FAILURE: &str = "[日志含无法安全脱敏的敏感字段]";

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
        self.append_safe_text(&redacted)
    }

    /// 按行追加；行之间补 `\n`。
    pub fn append_lines(&self, lines: &[String]) -> Result<(), String> {
        if lines.is_empty() {
            return Ok(());
        }
        self.append_text(&lines.join("\n"))
    }

    /// Only accepts text already redacted as a complete document or session output.
    pub(crate) fn append_safe_text(&self, text: &str) -> Result<(), String> {
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
        let mut append = cap_append_bytes(&normalized, append_limit);
        let mut failed = normalized.contains(REDACTION_FAILURE);
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
            // Carry a failure into the new file before its old marker can rotate
            // out of the retained archives. Inspection is bounded and fail-closed.
            failed |= if current_len > MAX_READ_BYTES as u64 {
                true
            } else if current_len > 0 {
                let mut prior = Vec::new();
                File::open(&self.paths.log_file)
                    .and_then(|file| file.take(MAX_READ_BYTES as u64).read_to_end(&mut prior))
                    .map_err(|_| "无法检查上一份启动日志的脱敏状态")?;
                prior
                    .windows(REDACTION_FAILURE.len())
                    .any(|bytes| bytes == REDACTION_FAILURE.as_bytes())
            } else {
                false
            };
            self.rotate_locked()?;
        }
        if failed && !append.contains(REDACTION_FAILURE) {
            let marker = format!("{REDACTION_FAILURE}\n");
            append = marker.clone()
                + &cap_append_bytes(&append, append_limit.saturating_sub(marker.len()));
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

pub(crate) const STREAM_LINE_MAX_BYTES: usize = 128 * 1024;
const STREAM_PENDING_MAX_BYTES: usize = MAX_COLLECTION_BYTES + STREAM_LINE_MAX_BYTES;
const STREAM_PENDING_MAX_LINES: usize = 16_384;
const STREAM_PENDING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug)]
struct QuoteBoundary {
    start: usize,
    index: usize,
    quote: u8,
    width: usize,
    escaped: bool,
}

impl QuoteBoundary {
    fn new(text: &str, start: usize, width: usize) -> Self {
        Self {
            start,
            index: start + width,
            quote: text.as_bytes()[start],
            width,
            escaped: false,
        }
    }

    fn advance(&mut self, text: &str) -> Result<Option<(usize, usize)>, ScanFailure> {
        let bytes = text.as_bytes();
        while self.index < bytes.len() {
            if self.index - self.start >= MAX_COLLECTION_BYTES {
                return Err(ScanFailure::Invalid("日志敏感字段超过安全处理大小"));
            }
            if !self.escaped
                && bytes[self.index] == self.quote
                && bytes
                    .get(self.index..self.index + self.width)
                    .is_some_and(|s| s.iter().all(|b| *b == self.quote))
            {
                let mut end = self.index + self.width;
                if self.width == 3 {
                    while end < bytes.len() && bytes[end] == self.quote {
                        end += 1;
                        if end - self.start > MAX_COLLECTION_BYTES {
                            return Err(ScanFailure::Invalid("日志敏感字段超过安全处理大小"));
                        }
                    }
                } else if self.quote == b'\'' && bytes.get(end) == Some(&self.quote) {
                    self.index = end + 1;
                    continue;
                }
                if end - self.start > MAX_COLLECTION_BYTES {
                    return Err(ScanFailure::Invalid("日志敏感字段超过安全处理大小"));
                }
                return Ok(Some((self.index, end)));
            }
            self.escaped = bytes[self.index] == b'\\'
                && !self.escaped
                && !(self.quote == b'\'' && self.width == 3);
            self.index += 1;
        }
        Ok(None)
    }
}

#[derive(Debug)]
struct CollectionBoundary {
    start: usize,
    index: usize,
    stack: Vec<u8>,
    quote: Option<QuoteBoundary>,
    line_comment: bool,
    block_comment: bool,
}

impl CollectionBoundary {
    fn new(start: usize) -> Self {
        Self {
            start,
            index: start,
            stack: Vec::new(),
            quote: None,
            line_comment: false,
            block_comment: false,
        }
    }

    fn advance(&mut self, text: &str) -> Result<Option<usize>, ScanFailure> {
        let bytes = text.as_bytes();
        while self.index < bytes.len() {
            if self.index - self.start >= MAX_COLLECTION_BYTES {
                return Err(ScanFailure::Invalid("日志敏感集合超过安全处理大小"));
            }
            if let Some(quote) = self.quote.as_mut() {
                let Some((_, end)) = quote.advance(text)? else {
                    return Ok(None);
                };
                self.index = end;
                self.quote = None;
                if end - self.start > MAX_COLLECTION_BYTES {
                    return Err(ScanFailure::Invalid("日志敏感集合超过安全处理大小"));
                }
                continue;
            }
            let byte = bytes[self.index];
            if self.line_comment {
                self.line_comment = !matches!(byte, b'\r' | b'\n');
                self.index += 1;
                continue;
            }
            if self.block_comment {
                if bytes[self.index..].starts_with(b"*/") {
                    self.block_comment = false;
                    self.index += 2;
                } else {
                    self.index += 1;
                }
                continue;
            }
            if matches!(byte, b'"' | b'\'' | b'`') {
                let width = quote_width(bytes, self.index);
                self.quote = Some(QuoteBoundary::new(text, self.index, width));
                continue;
            }
            // URL fragments inside a map are data, not comments.
            let comment_boundary = self.index == self.start
                || bytes[self.index - 1].is_ascii_whitespace()
                || matches!(bytes[self.index - 1], b'[' | b'{' | b',');
            if comment_boundary && (byte == b'#' || bytes[self.index..].starts_with(b"//")) {
                self.line_comment = true;
                self.index += 1;
                continue;
            }
            if comment_boundary && bytes[self.index..].starts_with(b"/*") {
                self.block_comment = true;
                self.index += 2;
                continue;
            }
            match byte {
                b'[' | b'{' => {
                    if self.stack.len() >= MAX_COLLECTION_DEPTH {
                        return Err(ScanFailure::Invalid("日志敏感集合嵌套超过安全处理深度"));
                    }
                    self.stack.push(if byte == b'[' { b']' } else { b'}' });
                }
                b']' | b'}' => {
                    if self.stack.pop() != Some(byte) {
                        return Err(ScanFailure::Invalid(
                            "日志敏感集合括号不匹配，无法安全生成完整报告",
                        ));
                    }
                    if self.stack.is_empty() {
                        return Ok(Some(self.index + 1));
                    }
                }
                _ => {}
            }
            self.index += 1;
        }
        Ok(None)
    }
}

fn quote_width(bytes: &[u8], start: usize) -> usize {
    if bytes[start] != b'`'
        && bytes
            .get(start..start + 3)
            .is_some_and(|s| s.iter().all(|b| *b == bytes[start]))
    {
        3
    } else {
        1
    }
}

#[derive(Debug)]
enum PendingBoundary {
    Header,
    Quote(QuoteBoundary),
    Collection(CollectionBoundary),
    Indented { indent: usize, cursor: usize },
}

impl PendingBoundary {
    fn new(text: &str, key_start: usize, value_start: Option<usize>) -> Self {
        let Some(start) = value_start else {
            return Self::Header;
        };
        match text.as_bytes()[start] {
            b'[' | b'{' => Self::Collection(CollectionBoundary::new(start)),
            b'"' | b'\'' | b'`' => Self::Quote(QuoteBoundary::new(
                text,
                start,
                quote_width(text.as_bytes(), start),
            )),
            _ => Self::Indented {
                indent: line_indent(text, key_start),
                cursor: scan_line_end(text, start),
            },
        }
    }

    fn advance(&mut self, text: &str) -> Result<bool, ScanFailure> {
        match self {
            Self::Header => Ok(true),
            Self::Quote(quote) => Ok(quote.advance(text)?.is_some()),
            Self::Collection(collection) => Ok(collection.advance(text)?.is_some()),
            Self::Indented { indent, cursor } => {
                while *cursor < text.len() {
                    while matches!(text.as_bytes().get(*cursor), Some(b'\r' | b'\n')) {
                        *cursor += 1;
                    }
                    if *cursor == text.len() {
                        break;
                    }
                    let end = scan_line_end(text, *cursor);
                    let content = skip_spaces(text, *cursor);
                    if content < end && content - *cursor <= *indent {
                        return Ok(true);
                    }
                    *cursor = end;
                }
                Ok(false)
            }
        }
    }
}

/// A stream keeps only an unfinished field; complete ordinary lines are released immediately.
/// Invalid or discarded input quarantines that stream until EOF, never its sibling stream.
#[derive(Debug, Default)]
pub(crate) struct StreamRedactor {
    pending: String,
    boundary: Option<PendingBoundary>,
    started: Option<std::time::Instant>,
    pending_lines: usize,
    quarantined: bool,
    finished: bool,
}

impl StreamRedactor {
    pub(crate) fn push_line(&mut self, line: &str, discarded: bool) -> String {
        if self.finished || self.quarantined {
            return String::new();
        }
        if discarded || line.len() > STREAM_LINE_MAX_BYTES {
            return self.reject();
        }
        let text = ensure_trailing_newline(&normalize_newlines(line));
        if self.pending.is_empty() {
            return self.accept(text);
        }
        if self.pending.len().saturating_add(text.len()) > STREAM_PENDING_MAX_BYTES
            || self.pending_lines >= STREAM_PENDING_MAX_LINES
            || self
                .started
                .is_some_and(|start| start.elapsed() >= STREAM_PENDING_TIMEOUT)
        {
            return self.reject();
        }
        self.pending.push_str(&text);
        self.pending_lines += 1;
        let completed_value = !matches!(self.boundary, Some(PendingBoundary::Header));
        if !completed_value && self.pending.len() > 4096 {
            return self.reject();
        }
        match self.boundary.as_mut().unwrap().advance(&self.pending) {
            Ok(false) => String::new(),
            Ok(true) => {
                let text = std::mem::take(&mut self.pending);
                self.boundary = None;
                if completed_value {
                    self.started = None;
                }
                self.pending_lines = 0;
                self.accept(text)
            }
            Err(_) => self.reject(),
        }
    }

    fn accept(&mut self, text: String) -> String {
        match redact_document(&text, false) {
            Ok(safe) => {
                self.started = None;
                self.pending_lines = 0;
                safe
            }
            Err(ScanFailure::IncompleteField {
                key_start,
                value_start,
            }) => {
                let line_start = text[..key_start].rfind('\n').map_or(0, |n| n + 1);
                let (safe, pending_start) = match redact_document(&text[..line_start], true) {
                    Ok(safe) => (safe, line_start),
                    // A previous value can close on the next field's opening line.
                    // Retain that complete prefix too instead of cutting its boundary.
                    Err(_) => (String::new(), 0),
                };
                self.pending = text[pending_start..].to_owned();
                self.boundary = Some(PendingBoundary::new(
                    &self.pending,
                    key_start - pending_start,
                    value_start.map(|n| n - pending_start),
                ));
                if self
                    .boundary
                    .as_mut()
                    .unwrap()
                    .advance(&self.pending)
                    .is_err()
                {
                    return self.reject();
                }
                if self.started.is_none() {
                    self.started = Some(std::time::Instant::now());
                }
                self.pending_lines = self.pending.lines().count();
                safe
            }
            Err(_) => self.reject(),
        }
    }

    pub(crate) fn expire(&mut self) -> String {
        if self
            .started
            .is_some_and(|start| start.elapsed() >= STREAM_PENDING_TIMEOUT)
        {
            self.reject()
        } else {
            String::new()
        }
    }

    pub(crate) fn finish(&mut self, failed: bool) -> String {
        if self.finished {
            return String::new();
        }
        self.finished = true;
        if failed && !self.quarantined {
            return self.reject();
        }
        if self.quarantined {
            return String::new();
        }
        let text = std::mem::take(&mut self.pending);
        self.boundary = None;
        self.started = None;
        redact_document(&text, true).unwrap_or_else(|_| self.reject())
    }

    pub(crate) fn failed(&self) -> bool {
        self.quarantined
    }

    fn reject(&mut self) -> String {
        self.pending.clear();
        self.pending.shrink_to_fit();
        self.boundary = None;
        self.started = None;
        self.quarantined = true;
        format!("{REDACTION_FAILURE}\n")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 脱敏
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaskKind {
    Full,
    Token,
    Collection,
    EncodedCollection(usize),
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
    IncompleteField {
        key_start: usize,
        value_start: Option<usize>,
    },
    Invalid(&'static str),
}
impl ScanFailure {
    fn message(&self) -> String {
        match self {
            Self::Incomplete | Self::IncompleteField { .. } => {
                "日志敏感字段未闭合，无法安全生成完整报告".into()
            }
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

const MAX_QUOTED_MESSAGE_DEPTH: usize = 8;

fn decode_log_string(inner: &str, quote: u8) -> Option<(String, Vec<usize>)> {
    let mut decoded = String::new();
    let mut offsets = vec![0];
    let mut index = 0;
    while index < inner.len() {
        let start = index;
        let ch = inner[index..].chars().next()?;
        let value = if ch == '\\' {
            let escape = *inner.as_bytes().get(index + 1)?;
            let mut width = if escape == b'u' { 6 } else { 2 };
            if escape == b'u' {
                let code = u16::from_str_radix(inner.get(index + 2..index + 6)?, 16).ok()?;
                if (0xd800..=0xdbff).contains(&code) {
                    width = 12;
                }
            }
            index = index.checked_add(width)?;
            let raw = inner.get(start..index)?;
            if quote == b'\'' && escape == b'\'' {
                "'".to_owned()
            } else {
                serde_json::from_str::<String>(&format!("\"{raw}\"")).ok()?
            }
        } else if quote == b'\'' && inner.as_bytes().get(index..index + 2) == Some(b"''") {
            index += 2;
            "'".to_owned()
        } else {
            if quote == b'"' && ch.is_control() {
                return None;
            }
            index += ch.len_utf8();
            ch.to_string()
        };
        decoded.push_str(&value);
        offsets.extend(std::iter::repeat_n(start, value.len().saturating_sub(1)));
        offsets.push(index);
    }
    Some((decoded, offsets))
}

fn collect_quoted_message_spans(
    inner: &str,
    quote: u8,
    width: usize,
    depth: usize,
) -> Result<Vec<RedactionSpan>, ScanFailure> {
    if depth >= MAX_QUOTED_MESSAGE_DEPTH {
        return Err(ScanFailure::Invalid("日志引号消息嵌套超过安全处理深度"));
    }
    let decoded = (width == 1)
        .then(|| decode_log_string(inner, quote))
        .flatten();
    let text = decoded.as_ref().map_or(inner, |(text, _)| text.as_str());
    let mut spans = Vec::new();
    match collect_key_value_spans_bounded(text, &mut spans, true, depth + 1) {
        Ok(()) => {}
        Err(ScanFailure::Incomplete | ScanFailure::IncompleteField { .. }) => {
            // A complete outer string can safely hide an uncertain assignment in full.
            return Ok(vec![RedactionSpan {
                start: 0,
                end: inner.len(),
                kind: MaskKind::Full,
            }]);
        }
        Err(error) => return Err(error),
    }
    let Some((text, offsets)) = decoded else {
        return Ok(if spans.is_empty() {
            Vec::new()
        } else {
            vec![RedactionSpan {
                start: 0,
                end: inner.len(),
                kind: MaskKind::Full,
            }]
        });
    };
    // Map decoded boundaries back to the original escape sequences; never splice
    // an encoded quote, backslash or Unicode scalar in half.
    spans.retain(|span| text.is_char_boundary(span.start) && text.is_char_boundary(span.end));
    for span in &mut spans {
        let start = offsets[span.start];
        let end = offsets[span.end];
        span.kind = match span.kind {
            MaskKind::Collection => MaskKind::EncodedCollection(1),
            MaskKind::EncodedCollection(depth) => MaskKind::EncodedCollection(depth + 1),
            MaskKind::Token if inner[start..end] == text[span.start..span.end] => MaskKind::Token,
            _ => MaskKind::Full,
        };
        span.start = start;
        span.end = end;
    }
    Ok(spans)
}

fn collect_key_value_spans(
    text: &str,
    spans: &mut Vec<RedactionSpan>,
    final_input: bool,
) -> Result<(), ScanFailure> {
    collect_key_value_spans_bounded(text, spans, final_input, 0)
}

fn collect_key_value_spans_bounded(
    text: &str,
    spans: &mut Vec<RedactionSpan>,
    final_input: bool,
    depth: usize,
) -> Result<(), ScanFailure> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let key_start = index;
        let (key, after_key, quoted) = if matches!(bytes[index], b'"' | b'\'') {
            let key = quoted_key(text, index);
            let explicit = key.as_ref().is_some_and(|(_, end)| {
                matches!(
                    bytes.get(skip_field_whitespace(text, *end)),
                    Some(b':' | b'=')
                )
            });
            if !explicit {
                if let Some((key, end)) = &key {
                    if !final_input
                        && log_key_kind(key).is_some()
                        && skip_field_whitespace(text, *end) == text.len()
                    {
                        return Err(ScanFailure::IncompleteField {
                            key_start,
                            value_start: None,
                        });
                    }
                }
                let width = quote_width(bytes, index);
                match scan_quote_end(text, index, width) {
                    Ok((inner_end, end)) => {
                        for mut span in collect_quoted_message_spans(
                            &text[index + width..inner_end],
                            bytes[index],
                            width,
                            depth,
                        )? {
                            span.start += index + width;
                            span.end += index + width;
                            push_redaction_span(spans, span).map_err(|_| {
                                ScanFailure::Invalid("日志敏感字段数量超过安全处理上限")
                            })?;
                        }
                        index = end;
                        continue;
                    }
                    Err(ScanFailure::Incomplete) => {
                        if !collect_quoted_message_spans(
                            &text[index + width..],
                            bytes[index],
                            width,
                            depth,
                        )?
                        .is_empty()
                        {
                            return Err(ScanFailure::IncompleteField {
                                key_start,
                                value_start: Some(index),
                            });
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
            if let Some((key, end)) = key {
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
        index = after_key;
        let cli = !quoted
            && key.starts_with("--")
            && key.len() > 2
            && (key_start == 0 || bytes[key_start - 1].is_ascii_whitespace());
        let Some(kind) = log_key_kind(key.trim_start_matches('-')) else {
            continue;
        };
        if !explicit && !cli {
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
                return Err(ScanFailure::IncompleteField {
                    key_start,
                    value_start: None,
                });
            }
            continue;
        }
        let authorization = key
            .trim_start_matches('-')
            .eq_ignore_ascii_case("authorization");
        let inline_bearer = authorization
            && text[start..]
                .get(..7)
                .is_some_and(|value| value.eq_ignore_ascii_case("bearer "))
            && inline_field_follows(text, scan_value_end(text, skip_spaces(text, start + 7)));
        let block_value = explicit
            && bytes[separator] == b':'
            && !inline_bearer
            && !matches!(
                bytes[start],
                b'[' | b'{' | b'"' | b'\'' | b'`' | b'|' | b'>'
            )
            && (text[separator + 1..start].contains('\n')
                && line_indent(text, start) > line_indent(text, key_start)
                || yaml_field_start(text, key_start));
        let scanned = if block_value {
            let end = scan_indented_end(
                text,
                key_start,
                start,
                scan_line_end(text, start),
                final_input,
            );
            end.map(|end| {
                (
                    RedactionSpan {
                        start,
                        end,
                        kind: MaskKind::Collection,
                    },
                    end,
                )
            })
        } else {
            scan_sensitive_value(text, key_start, start, kind, authorization, final_input)
        };
        let (span, consumed) = scanned.map_err(|error| match error {
            ScanFailure::Incomplete => ScanFailure::IncompleteField {
                key_start,
                value_start: Some(start),
            },
            error => error,
        })?;
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

fn inline_field_follows(text: &str, start: usize) -> bool {
    let start = skip_spaces(text, start);
    let line_end = scan_line_end(text, start);
    let mut end = start;
    while end < line_end && key_byte(text.as_bytes()[end]) {
        end += 1;
    }
    if end == start || end - start > MAX_KEY_BYTES {
        return false;
    }
    let separator = skip_spaces(text, end);
    let key = &text[start..end];
    (matches!(text.as_bytes().get(separator), Some(b':' | b'='))
        && !matches!(text.as_bytes().get(separator + 1), Some(b'=' | b'>')))
        || (key.starts_with("--")
            && log_key_kind(key.trim_start_matches('-')).is_some()
            && separator > end
            && separator < line_end)
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
    QuoteBoundary::new(text, start, width)
        .advance(text)?
        .ok_or(ScanFailure::Incomplete)
}

fn scan_collection_end(text: &str, start: usize) -> Result<usize, ScanFailure> {
    CollectionBoundary::new(start)
        .advance(text)?
        .ok_or(ScanFailure::Incomplete)
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
    match (left, right) {
        (MaskKind::Collection, _) | (_, MaskKind::Collection) => MaskKind::Collection,
        (MaskKind::EncodedCollection(a), MaskKind::EncodedCollection(b)) => {
            MaskKind::EncodedCollection(a.min(b))
        }
        (MaskKind::EncodedCollection(depth), _) | (_, MaskKind::EncodedCollection(depth)) => {
            MaskKind::EncodedCollection(depth)
        }
        (MaskKind::Full, _) | (_, MaskKind::Full) => MaskKind::Full,
        _ => MaskKind::Token,
    }
}

fn mask_value(value: &str, kind: MaskKind) -> String {
    if let MaskKind::EncodedCollection(depth) = kind {
        let mut mask = format!("\"{REDACTION_MASK}\"");
        for _ in 0..depth {
            let encoded = serde_json::to_string(&mask).expect("a string always serializes");
            mask = encoded[1..encoded.len() - 1].to_owned();
        }
        return mask;
    }
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

#[cfg(test)]
mod quoted_message_redaction_tests {
    use super::*;

    #[test]
    fn quoted_messages_map_escaped_values_without_changing_neighbors() {
        for inner in [
            r#"prefix password="round03-secret" suffix visible"#,
            r#"prefix password="round03-secret\\tail\"more" suffix visible"#,
            r#"prefix {"pass\u0077ord":"round03-secret"} suffix visible"#,
            r#"prefix password="round03-secret\uD83D\uDE00" suffix visible"#,
            r#"prefix password=["round03-secret",{"v":"round03-secret"}] suffix visible"#,
            r#"prefix {"password":["round03-secret"],"other":"visible"} suffix visible"#,
        ] {
            let raw = serde_json::json!({"message":inner,"next":"visible"}).to_string();
            let safe = redact_sensitive_text_checked(&raw).unwrap();
            let value: serde_json::Value = serde_json::from_str(&safe).unwrap();
            assert!(!safe.contains("round03-secret"), "{safe}");
            assert_eq!(value["next"], "visible");
            let message = value["message"].as_str().unwrap();
            assert!(
                message.starts_with("prefix ") && message.ends_with(" suffix visible"),
                "{message}"
            );
            assert_eq!(redact_sensitive_text_checked(&safe).unwrap(), safe);
            let mut stream = StreamRedactor::default();
            assert_eq!(stream.push_line(&raw, false), format!("{safe}\n"));
            assert!(stream.finish(false).is_empty() && !stream.failed());
        }
        let raw = r#"{"message":"prefix pass\u0077ord=\u0022round03-secret\u0022 suffix visible","next":"visible"}"#;
        let safe = redact_sensitive_text_checked(raw).unwrap();
        assert!(!safe.contains("round03-secret"));
        assert!(serde_json::from_str::<serde_json::Value>(&safe).is_ok());
        assert_eq!(redact_sensitive_text_checked(&safe).unwrap(), safe);
    }

    #[test]
    fn quoted_messages_keep_normal_diagnostics_and_redacted_edges_stable() {
        for raw in [
            r#"{"message":"Unexpected token \" in JSON","next":"visible"}"#,
            r#"{"message":"path C:\\Program Files\\game and \uD83D\uDE00","next":"visible"}"#,
            r#"{"message":"password == expected and password => callback","next":"visible"}"#,
            r#"{"message":"password=\"***\" token=abcd***wxyz","next":"visible"}"#,
            r#"{"message":"password=[\"***\"]","next":"visible"}"#,
        ] {
            let safe = redact_sensitive_text_checked(raw).unwrap();
            assert!(serde_json::from_str::<serde_json::Value>(&safe).is_ok());
            assert_eq!(redact_sensitive_text_checked(&safe).unwrap(), safe);
            if !raw.contains("password=[") {
                assert_eq!(safe, raw);
            }
        }
        assert_eq!(
            redact_sensitive_text_checked(
                r#"message='prefix password="round03-secret" suffix visible' next=visible"#
            )
            .unwrap(),
            r#"message='prefix password="***" suffix visible' next=visible"#,
        );
        let raw = r#"{"message":"password=\"round03-secret","next":"visible"}"#;
        let safe = redact_sensitive_text_checked(raw).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&safe).unwrap()["message"],
            "***"
        );
        assert_eq!(redact_sensitive_text_checked(&safe).unwrap(), safe);
    }

    #[test]
    fn quoted_messages_bound_mapping_depth_and_unfinished_streams() {
        let mut inner = r#"password="round03-secret""#.to_owned();
        for _ in 0..MAX_QUOTED_MESSAGE_DEPTH {
            inner = serde_json::to_string(&inner).unwrap();
        }
        let safe = redact_sensitive_text_checked(&inner).unwrap();
        assert!(!safe.contains("round03-secret"));
        assert!(redact_sensitive_text_checked(&serde_json::to_string(&inner).unwrap()).is_err());
        let oversized = format!("\"{}\"", "x".repeat(MAX_COLLECTION_BYTES));
        assert!(redact_sensitive_text_checked(&oversized).is_err());
        let unfinished = r#"{"message":"prefix password=\"round03-secret\""#;
        let mut stream = StreamRedactor::default();
        let mut safe = stream.push_line(unfinished, false);
        safe.push_str(&stream.finish(false));
        assert!(!safe.contains("round03-secret"));
        assert!(stream.failed());
    }
}

#[cfg(test)]
mod stream_redaction_tests {
    use super::*;

    #[test]
    fn streams_use_the_document_lexer_without_publishing_continuations() {
        for raw in [
            "{\n\"password\": [\n],\n\"next\":\"visible\"\n}\n",
            "{\n\"pass\\u0077ord\"\n:\n[\n{\"value\":\"stream-secret\"}\n],\n\"next\":\"visible\"\n}\n",
            "password=\"\nstream-secret\n\"\nnext=visible\n",
            "password=\"\"\"\r\nstream-secret\r\n\"\"\"\r\nnext=visible\r\n",
            "password='''\nstream-secret\\tail\n'''\nnext=visible\n",
            "password: |2+ # comment\n  stream-secret\nnext: visible\n",
            "password:\n  primary: stream-secret\nnext: visible\n",
            "password: stream-secret\n  continuation\nnext: visible\n",
            "password=[/* ]\n } */ 'stream-secret',\n{value:https://example.invalid/stream-secret}]\nnext=visible\n",
            "password=[\n`stream-secret\\`}]`\n]\nnext=visible\n",
        ] {
            let mut stream = StreamRedactor::default();
            let mut output = String::new();
            for line in raw.split_terminator('\n') {
                let safe = stream.push_line(line, false);
                assert!(!safe.contains("stream-secret"), "{raw}");
                output.push_str(&safe);
            }
            output.push_str(&stream.finish(false));
            assert!(!output.contains(REDACTION_FAILURE), "{raw}: {output}");
            assert!(output.contains("visible"));
            assert_eq!(
                output,
                redact_sensitive_text_checked(&normalize_newlines(raw)).unwrap()
            );
            assert!(stream.finish(false).is_empty());
        }
    }

    #[test]
    fn ordinary_lines_are_not_delayed_by_unrelated_sensitive_words() {
        let mut stream = StreamRedactor::default();
        for line in [
            "SyntaxError: Unexpected token { in JSON",
            "token",
            "password confirmation failed",
            "UTF8 中文",
            "next=visible",
        ] {
            assert_eq!(stream.push_line(line, false), format!("{line}\n"));
            assert!(stream.pending.is_empty());
        }
        assert!(stream.finish(false).is_empty());
    }

    #[test]
    fn incomplete_values_flush_fail_closed_and_new_sessions_are_independent() {
        for prefix in ["password=[", "password=\"", "password='''", "password=`"] {
            let mut stream = StreamRedactor::default();
            assert!(stream.push_line(prefix, false).is_empty());
            assert!(stream.push_line("stream-secret", false).is_empty());
            assert_eq!(stream.finish(false), format!("{REDACTION_FAILURE}\n"));
            assert!(stream.push_line("stream-secret", false).is_empty());
            let mut next = StreamRedactor::default();
            assert_eq!(next.push_line("next=visible", false), "next=visible\n");
        }
        let mut yaml = StreamRedactor::default();
        assert!(yaml.push_line("password: |", false).is_empty());
        assert!(yaml.push_line("  stream-secret", false).is_empty());
        assert_eq!(yaml.finish(false), "password: \"***\"\n");
    }

    #[test]
    fn pending_bytes_lines_and_timeout_are_bounded() {
        let mut lines = StreamRedactor::default();
        assert!(lines.push_line("password=[", false).is_empty());
        for _ in 0..STREAM_PENDING_MAX_LINES {
            lines.push_line("", false);
        }
        assert!(lines.quarantined);
        assert!(lines.pending.is_empty());
        assert!(lines.push_line("stream-secret", false).is_empty());

        let mut bytes = StreamRedactor::default();
        bytes.push_line("password=[", false);
        for _ in 0..20 {
            let safe = bytes.push_line(&"stream-secret ".repeat(7000), false);
            assert!(!safe.contains("stream-secret"));
            assert!(bytes.pending.len() <= STREAM_PENDING_MAX_BYTES);
        }
        assert!(bytes.quarantined);

        let mut timeout = StreamRedactor::default();
        timeout.push_line("password=[", false);
        timeout.started = Some(std::time::Instant::now() - STREAM_PENDING_TIMEOUT);
        assert_eq!(timeout.expire(), format!("{REDACTION_FAILURE}\n"));
        assert!(timeout.expire().is_empty());

        let mut discarded = StreamRedactor::default();
        assert_eq!(
            discarded.push_line("", true),
            format!("{REDACTION_FAILURE}\n")
        );
        assert!(discarded.push_line("stream-secret", false).is_empty());
    }

    #[test]
    fn long_collections_advance_a_cursor_instead_of_rescanning_every_line() {
        let mut stream = StreamRedactor::default();
        stream.push_line("password=[", false);
        for _ in 0..2000 {
            assert!(stream.push_line("\"stream-secret\",", false).is_empty());
            let Some(PendingBoundary::Collection(boundary)) = &stream.boundary else {
                panic!("collection state");
            };
            assert_eq!(boundary.index, stream.pending.len());
        }
        let safe = stream.push_line("] next=visible", false);
        assert_eq!(safe, "password=\"***\" next=visible\n");
        assert!(stream.pending.is_empty());
        assert!(stream.finish(false).is_empty());
    }

    #[test]
    fn successive_fields_reset_timeouts_but_incomplete_headers_do_not() {
        let mut stream = StreamRedactor::default();
        assert!(stream.push_line("password=[", false).is_empty());
        let old = std::time::Instant::now() - STREAM_PENDING_TIMEOUT / 2;
        stream.started = Some(old);
        assert_eq!(stream.push_line("] password=[", false), "");
        assert!(stream.started.unwrap() > old);
        assert_eq!(
            stream.push_line("] next=visible", false),
            "password=\"***\" password=\"***\" next=visible\n"
        );
        assert!(!stream.failed());

        let mut header = StreamRedactor::default();
        assert!(header.push_line("\"password\"", false).is_empty());
        header.started = Some(old);
        assert!(header.push_line("", false).is_empty());
        assert_eq!(header.started, Some(old));
        assert!(header.push_line(":", false).is_empty());
        assert_eq!(header.started, Some(old));
        assert_eq!(
            header.push_line("[]", false),
            "\"password\"\n\n:\n\"***\"\n"
        );
        assert!(!header.failed());
    }
}
