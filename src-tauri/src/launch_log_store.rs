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

const TOKEN_KEYS: &[&str] = &[
    "accessToken",
    "access_token",
    "access-token",
    "clientToken",
    "client_token",
    "client-token",
    "refreshToken",
    "refresh_token",
    "refresh-token",
    "sessionToken",
    "session_token",
    "authToken",
    "auth_token",
    "authorization",
    "token",
];

const FULL_KEYS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    // OAuth 客户端密钥的常见 camelCase / snake_case / kebab-case / 紧凑写法。
    "clientSecret",
    "client_secret",
    "client-secret",
    "clientsecret",
    "inviteCode",
    "invite_code",
    "invite-code",
    "invitationCode",
    "invitation_code",
    "apiKey",
    "api_key",
    "x-api-key",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaskKind {
    Full,
    Token,
}

#[derive(Debug, Clone, Copy)]
struct RedactionSpan {
    start: usize,
    end: usize,
    kind: MaskKind,
}

/// 对日志文本做保守脱敏：
/// - accessToken / clientToken / refreshToken / Authorization 等保留首尾少量字符
/// - password / secret / 邀请码 / API key 完全打码
/// - `C:\Users\<name>`、`/Users/<name>`、`/home/<name>` 只遮蔽用户名段
/// - URL 的 userinfo 只保留协议和主机
///
/// 返回值仍是可读日志，键名、路径层级和错误信息都会保留。
pub fn redact_sensitive_text(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut spans = Vec::new();
    collect_key_value_spans(text, &mut spans);
    collect_user_path_spans(text, &mut spans);
    collect_invite_path_spans(text, &mut spans);
    collect_url_userinfo_spans(text, &mut spans);
    apply_redaction_spans(text, spans)
}

fn collect_key_value_spans(text: &str, spans: &mut Vec<RedactionSpan>) {
    for key in TOKEN_KEYS {
        let authorization = key.eq_ignore_ascii_case("authorization");
        let kind = if authorization {
            MaskKind::Full
        } else {
            MaskKind::Token
        };
        collect_key_spans(
            text,
            key,
            kind,
            !key.eq_ignore_ascii_case("token"),
            authorization,
            spans,
        );
    }
    for key in FULL_KEYS {
        collect_key_spans(text, key, MaskKind::Full, false, false, spans);
    }
}

fn collect_key_spans(
    text: &str,
    key: &str,
    kind: MaskKind,
    allow_short_implicit: bool,
    authorization: bool,
    spans: &mut Vec<RedactionSpan>,
) {
    let mut from = 0;
    while let Some(position) = find_key(text, key, from) {
        if let Some((start, end)) = parse_key_value(
            text,
            position + key.len(),
            kind,
            allow_short_implicit,
            authorization,
        ) {
            spans.push(RedactionSpan { start, end, kind });
        }
        from = position + key.len().max(1);
    }
}

fn find_key(text: &str, key: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let needle = key.as_bytes();
    if needle.is_empty() || from >= bytes.len() || bytes.len() < needle.len() {
        return None;
    }
    let mut index = from;
    while index + needle.len() <= bytes.len() {
        if bytes[index..index + needle.len()].eq_ignore_ascii_case(needle)
            && key_boundary(bytes, index, needle.len())
        {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn key_boundary(bytes: &[u8], start: usize, length: usize) -> bool {
    let before_ok = start == 0 || !is_key_byte(bytes[start - 1]);
    let after = start + length;
    let after_ok = after >= bytes.len() || !is_key_byte(bytes[after]);
    before_ok && after_ok
}
fn is_key_byte(byte: u8) -> bool {
    // 不把 `-` 算作 key 边界：日志里常见 `--accessToken <value>`。
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn parse_key_value(
    text: &str,
    after_key: usize,
    kind: MaskKind,
    allow_short_implicit: bool,
    authorization: bool,
) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut index = skip_spaces(text, after_key);

    let mut quote = None;
    if let Some(byte) = bytes.get(index).copied() {
        if byte == b'"' || byte == b'\'' {
            let after_quote = skip_spaces(text, index + 1);
            if matches!(bytes.get(after_quote), Some(b':') | Some(b'=')) {
                index = after_quote;
            } else {
                // accessToken 后直接跟引号时，引号开始值而非包围键。
                index += 1;
                quote = Some(byte);
            }
        }
    }

    let had_separator = match bytes.get(index).copied() {
        Some(b':') | Some(b'=') => {
            index = skip_spaces(text, index + 1);
            true
        }
        _ => false,
    };

    if !had_separator && kind == MaskKind::Full && !allow_short_implicit {
        return None;
    }

    if quote.is_none() {
        if let Some(byte) = bytes.get(index).copied() {
            if byte == b'"' || byte == b'\'' || byte == b'`' {
                index += 1;
                quote = Some(byte);
            }
        }
    }
    index = skip_spaces(text, index);

    let mut bearer_value = false;
    if authorization && starts_with_bearer(&bytes[index..]) {
        index += "bearer".len();
        if matches!(bytes.get(index), Some(byte) if byte.is_ascii_whitespace()) {
            index = skip_spaces(text, index + 1);
        }
        bearer_value = true;
    }

    let start = index;
    let end = if authorization && !bearer_value {
        // Basic / Digest 等认证头没有统一的 token 边界，整行打码更安全。
        scan_line_end(text, start)
    } else if let Some(quote) = quote {
        scan_quoted_value_end(text, start, quote)
    } else {
        scan_value_end(text, start)
    };
    if end <= start {
        return None;
    }

    // 无分隔符时只接受较长的 token，避免把 `token is invalid` 中的 `is` 打码。
    if !had_separator && quote.is_none() && !allow_short_implicit && end - start < 8 {
        return None;
    }

    Some((start, end))
}

fn scan_line_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = start;
    while let Some(byte) = bytes.get(index).copied() {
        if byte == b'\n' || byte == b'\r' {
            break;
        }
        index += 1;
    }
    index
}

fn skip_spaces(text: &str, mut index: usize) -> usize {
    let bytes = text.as_bytes();
    while matches!(bytes.get(index), Some(b' ') | Some(b'\t')) {
        index += 1;
    }
    index
}

fn starts_with_bearer(bytes: &[u8]) -> bool {
    bytes.len() >= "bearer".len() && bytes[.."bearer".len()].eq_ignore_ascii_case(b"bearer")
}

fn scan_value_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = start;
    while let Some(byte) = bytes.get(index).copied() {
        if byte.is_ascii_whitespace()
            || matches!(
                byte,
                b'"' | b'\'' | b'`' | b',' | b';' | b'&' | b'}' | b']' | b')' | b'<' | b'>'
            )
        {
            break;
        }
        index += 1;
    }
    index
}

fn scan_quoted_value_end(text: &str, start: usize, quote: u8) -> usize {
    let bytes = text.as_bytes();
    let mut index = start;
    let mut escaped = false;
    while let Some(byte) = bytes.get(index).copied() {
        if byte == b'\n' || byte == b'\r' || (byte == quote && !escaped) {
            break;
        }
        escaped = byte == b'\\' && !escaped;
        index += 1;
    }
    index
}

fn collect_user_path_spans(text: &str, spans: &mut Vec<RedactionSpan>) {
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
                spans.push(RedactionSpan {
                    start,
                    end,
                    kind: MaskKind::Full,
                });
            }
            from = position + marker.len().max(1);
        }
    }
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

fn collect_invite_path_spans(text: &str, spans: &mut Vec<RedactionSpan>) {
    for marker in ["/invite/", "/join/"] {
        let mut from = 0;
        while let Some(position) = find_ascii_insensitive(text, marker, from) {
            let start = position + marker.len();
            let end = scan_invite_segment_end(text, start);
            if end > start && &text[start..end] != REDACTION_MASK {
                spans.push(RedactionSpan {
                    start,
                    end,
                    kind: MaskKind::Full,
                });
            }
            from = position + marker.len().max(1);
        }
    }
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

fn collect_url_userinfo_spans(text: &str, spans: &mut Vec<RedactionSpan>) {
    let mut from = 0;
    while let Some(position) = find_ascii_insensitive(text, "://", from) {
        let authority_start = position + 3;
        let authority_end = scan_authority_end(text, authority_start);
        if let Some(userinfo_end) = text[authority_start..authority_end].rfind('@') {
            let at = authority_start + userinfo_end;
            if text[authority_start..at].contains(':') {
                spans.push(RedactionSpan {
                    start: authority_start,
                    end: at,
                    kind: MaskKind::Full,
                });
            }
        }
        from = authority_start.max(position + 1);
    }
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
    if left == MaskKind::Full || right == MaskKind::Full {
        MaskKind::Full
    } else {
        MaskKind::Token
    }
}

fn mask_value(value: &str, kind: MaskKind) -> String {
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
