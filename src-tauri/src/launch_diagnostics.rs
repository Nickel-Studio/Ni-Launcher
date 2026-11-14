//! Minecraft 启动失败的纯逻辑诊断。
//!
//! 输入由 runtime-core 在进程退出后收集：退出码、stdout/stderr 尾部、
//! `logs/latest.log`、`crash-reports` 文本，以及版本 / 加载器 / OptiFine /
//! Java 信息。本模块只做文本分析与分类，不读文件、不发事件、不 panic。
//!
//! 设计取舍：
//! - 优先匹配明确错误特征，再组合上下文；未命中时返回 `Unknown` 并记录回退原因。
//! - 每个输入源都限制字节数和行数；非 UTF-8 由调用方通过 `*_bytes`
//!   方法或 `decode_lossy_*` 安全转换。
//! - 证据片段统一经过 `launch_log_store::redact_sensitive_text`，避免把
//!   token / 密码 / 用户名路径带进事件或前端。

use serde::{Deserialize, Serialize};

use crate::launch_log_store::redact_sensitive_text;

/// 单段 stdout/stderr 的最大分析字节数。
pub const MAX_STREAM_BYTES: usize = 128 * 1024;
/// `logs/latest.log` 的最大分析字节数。
pub const MAX_LATEST_LOG_BYTES: usize = 256 * 1024;
/// 崩溃报告的最大分析字节数。
pub const MAX_CRASH_REPORT_BYTES: usize = 512 * 1024;
/// 每个输入源保留的最大行数。
pub const MAX_LINES_PER_SOURCE: usize = 4_000;
/// 单条诊断最多展示的证据数量。
pub const MAX_EVIDENCE_PER_FINDING: usize = 3;
/// 单次报告最多包含的分类数量（主分类 + 附加分类）。
pub const MAX_FINDINGS: usize = 4;

const MAX_EVIDENCE_CHARS: usize = 320;
const HEAD_TAIL_MARKER: &str = "[...日志已截断...]";

/// 崩溃 / 退出的结构化分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticCategory {
    /// 内存不足。
    OutOfMemory,
    /// JVM 参数无法识别或虚拟机初始化失败。
    JvmArgumentError,
    /// Java 版本与游戏 / Mod 的 class 文件版本不兼容。
    JavaVersionIncompatible,
    /// 旧版 Forge 与当前 Java 不兼容。
    ForgeJavaIncompatible,
    /// Mixin 准备、应用或转换失败。
    MixinFailure,
    /// OptiFine 与 Forge 版本不匹配。
    OptiFineForgeIncompatible,
    /// 缺少 Mod 前置或依赖版本不匹配。
    MissingModDependency,
    /// Mod 之间的兼容性冲突。
    ModConflict,
    /// Mod 初始化或加载器抛出异常。
    ModInitializationFailure,
    /// 同一个 Mod 被重复安装。
    DuplicateMod,
    /// Forge 安装文件不完整或版本 JSON 异常。
    ForgeInstallIncomplete,
    /// 显卡驱动或 OpenGL 能力不足。
    GraphicsDriverIssue,
    /// 未匹配到已知特征。
    Unknown,
}

impl DiagnosticCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::OutOfMemory => "内存不足",
            Self::JvmArgumentError => "JVM 参数错误",
            Self::JavaVersionIncompatible => "Java 版本不兼容",
            Self::ForgeJavaIncompatible => "Forge 与 Java 不兼容",
            Self::MixinFailure => "Mixin 失败",
            Self::OptiFineForgeIncompatible => "OptiFine 与 Forge 不兼容",
            Self::MissingModDependency => "缺少 Mod 前置依赖",
            Self::ModConflict => "Mod 冲突",
            Self::ModInitializationFailure => "Mod 初始化失败",
            Self::DuplicateMod => "重复安装 Mod",
            Self::ForgeInstallIncomplete => "Forge 安装不完整",
            Self::GraphicsDriverIssue => "显卡驱动或 OpenGL 问题",
            Self::Unknown => "未知原因",
        }
    }

    fn rank(self) -> u16 {
        match self {
            Self::OutOfMemory => 10,
            Self::JvmArgumentError => 20,
            Self::JavaVersionIncompatible => 30,
            Self::ForgeJavaIncompatible => 40,
            Self::MixinFailure => 50,
            Self::OptiFineForgeIncompatible => 60,
            Self::MissingModDependency => 70,
            Self::ModConflict => 80,
            Self::ModInitializationFailure => 90,
            Self::DuplicateMod => 100,
            Self::ForgeInstallIncomplete => 110,
            Self::GraphicsDriverIssue => 120,
            Self::Unknown => 1_000,
        }
    }
}

/// 诊断严重度，供前端决定提示层级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticSeverity {
    Critical,
    Error,
    Warning,
    Info,
}

/// 证据所在的输入源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticSource {
    Stdout,
    Stderr,
    LatestLog,
    CrashReport,
}

impl DiagnosticSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::LatestLog => "logs/latest.log",
            Self::CrashReport => "crash-reports",
        }
    }
}

/// 诊断输入。字段公开，runtime-core 可以直接构造或用 builder 方法逐步补齐。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticInput {
    pub exit_code: Option<i32>,
    pub minecraft_version: String,
    pub loader: String,
    pub optifine: Option<String>,
    pub java_major: Option<u32>,
    pub java_vendor: Option<String>,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub latest_log: Option<String>,
    pub crash_report: Option<String>,
    /// `*_bytes` builder 在入口处截断过原始字节时置为 true。
    pub input_truncated: bool,
}

impl DiagnosticInput {
    pub fn new(
        exit_code: Option<i32>,
        minecraft_version: impl Into<String>,
        loader: impl Into<String>,
    ) -> Self {
        Self {
            exit_code,
            minecraft_version: minecraft_version.into(),
            loader: loader.into(),
            ..Self::default()
        }
    }

    pub fn with_exit_code(mut self, exit_code: i32) -> Self {
        self.exit_code = Some(exit_code);
        self
    }

    pub fn with_process_output(
        mut self,
        stdout_tail: impl Into<String>,
        stderr_tail: impl Into<String>,
    ) -> Self {
        self.stdout_tail = stdout_tail.into();
        self.stderr_tail = stderr_tail.into();
        self
    }

    pub fn with_process_output_bytes(mut self, stdout_tail: &[u8], stderr_tail: &[u8]) -> Self {
        self.input_truncated |=
            stdout_tail.len() > MAX_STREAM_BYTES || stderr_tail.len() > MAX_STREAM_BYTES;
        self.stdout_tail = decode_lossy_tail(stdout_tail, MAX_STREAM_BYTES);
        self.stderr_tail = decode_lossy_tail(stderr_tail, MAX_STREAM_BYTES);
        self
    }

    pub fn with_latest_log(mut self, latest_log: impl Into<String>) -> Self {
        self.latest_log = Some(latest_log.into());
        self
    }

    pub fn with_latest_log_bytes(mut self, latest_log: &[u8]) -> Self {
        self.input_truncated |= latest_log.len() > MAX_LATEST_LOG_BYTES;
        self.latest_log = Some(decode_lossy_tail(latest_log, MAX_LATEST_LOG_BYTES));
        self
    }

    pub fn with_crash_report(mut self, crash_report: impl Into<String>) -> Self {
        self.crash_report = Some(crash_report.into());
        self
    }

    pub fn with_crash_report_bytes(mut self, crash_report: &[u8]) -> Self {
        self.input_truncated |= crash_report.len() > MAX_CRASH_REPORT_BYTES;
        self.crash_report = Some(decode_lossy_head_tail(crash_report, MAX_CRASH_REPORT_BYTES));
        self
    }

    pub fn with_optifine(mut self, version: impl Into<String>) -> Self {
        self.optifine = Some(version.into());
        self
    }

    pub fn with_java(mut self, major: u32, vendor: impl Into<String>) -> Self {
        self.java_major = Some(major);
        self.java_vendor = Some(vendor.into());
        self
    }
}

/// 一条可定位的证据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEvidence {
    pub source: DiagnosticSource,
    /// 在本次有界分析文本中的 1-based 行号。
    pub line: usize,
    /// 已脱敏并截断的证据片段。
    pub excerpt: String,
    /// 证据摘要，例如“匹配内存不足特征”。
    pub summary: String,
}

/// 一条诊断结论。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticFinding {
    pub category: DiagnosticCategory,
    pub severity: DiagnosticSeverity,
    pub summary: String,
    pub advice: String,
    pub evidence: Vec<DiagnosticEvidence>,
}

/// 与运行环境相关的上下文，原样带入报告供前端展示。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticContext {
    pub exit_code: Option<i32>,
    pub minecraft_version: String,
    pub loader: String,
    pub optifine: Option<String>,
    pub java_major: Option<u32>,
    pub java_vendor: Option<String>,
}

/// 诊断报告。`category` / `severity` / `summary` / `advice` / `evidence`
/// 是主分类的扁平投影，`additional` 保留次要分类。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReport {
    pub category: DiagnosticCategory,
    pub severity: DiagnosticSeverity,
    pub summary: String,
    pub advice: String,
    pub evidence: Vec<DiagnosticEvidence>,
    pub additional: Vec<DiagnosticFinding>,
    pub fallback_reason: Option<String>,
    pub input_truncated: bool,
    pub context: DiagnosticContext,
}

/// 安全解码尾部字节；从中间截断产生的半个 UTF-8 字符会变成替换字符。
pub fn decode_lossy_tail(bytes: &[u8], max_bytes: usize) -> String {
    let max_bytes = max_bytes.max(1);
    let start = bytes.len().saturating_sub(max_bytes);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

/// 安全解码崩溃报告的首尾字节，用于保留头部的 Description 和尾部的堆栈。
pub fn decode_lossy_head_tail(bytes: &[u8], max_bytes: usize) -> String {
    if bytes.len() <= max_bytes {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let marker = format!("\n{HEAD_TAIL_MARKER}\n");
    let budget = max_bytes.saturating_sub(marker.len()).max(1);
    let head_len = budget / 3;
    let tail_len = budget - head_len;
    let head_end = head_len.min(bytes.len());
    let tail_start = bytes.len().saturating_sub(tail_len);

    let mut out = String::with_capacity(marker.len() + head_end + tail_len);
    out.push_str(&String::from_utf8_lossy(&bytes[..head_end]));
    out.push_str(&marker);
    out.push_str(&String::from_utf8_lossy(&bytes[tail_start..]));
    out
}

/// 分析一次启动失败。没有命中任何规则时仍返回可解释的 `Unknown` 报告。
pub fn analyze(input: &DiagnosticInput) -> DiagnosticReport {
    let sources = build_sources(input);
    let mut findings = Vec::new();

    for rule in RULES {
        let evidence = collect_evidence(&sources, rule.patterns, rule.category);
        if !evidence.is_empty() {
            push_finding(
                &mut findings,
                DiagnosticFinding {
                    category: rule.category,
                    severity: rule.severity,
                    summary: rule.summary.to_string(),
                    advice: rule.advice.to_string(),
                    evidence,
                },
            );
        }
    }

    add_contextual_findings(input, &sources, &mut findings);
    findings.sort_by_key(|finding| finding.category.rank());
    findings.dedup_by(|left, right| left.category == right.category);
    findings.truncate(MAX_FINDINGS);

    let input_truncated = sources.iter().any(|source| source.truncated);
    let fallback_reason = if findings.is_empty() {
        Some(fallback_text(input, &sources))
    } else {
        None
    };

    let mut findings = findings.into_iter();
    let primary = match findings.next() {
        Some(finding) => finding,
        None => unknown_finding(input, &sources),
    };
    let additional: Vec<DiagnosticFinding> = findings.collect();

    DiagnosticReport {
        category: primary.category,
        severity: primary.severity,
        summary: primary.summary,
        advice: primary.advice,
        evidence: primary.evidence,
        additional,
        fallback_reason,
        input_truncated,
        context: context_from_input(input),
    }
}

struct Rule {
    category: DiagnosticCategory,
    severity: DiagnosticSeverity,
    patterns: &'static [&'static str],
    summary: &'static str,
    advice: &'static str,
}

// 规则顺序就是主分类优先级；精确日志匹配优先于上下文组合判断。
const RULES: &[Rule] = &[
    Rule {
        category: DiagnosticCategory::OutOfMemory,
        severity: DiagnosticSeverity::Critical,
        patterns: &[
            "java.lang.outofmemoryerror",
            "outofmemoryerror",
            "out of memory error",
            "the system is out of physical ram or swap space",
            "could not reserve enough space for object heap",
            "there is insufficient memory for the java runtime environment to continue",
            "native memory allocation (malloc) failed",
            "paging file is too small",
            "unable to create new native thread",
            "gc overhead limit exceeded",
            "java heap space",
        ],
        summary: "游戏因内存不足退出。",
        advice: "关闭其他占用内存的程序；在版本设置中提高最大内存，或减少 Mod / 高清材质；确认使用的是 64 位 Java。",
    },
    Rule {
        category: DiagnosticCategory::JvmArgumentError,
        severity: DiagnosticSeverity::Critical,
        patterns: &[
            "unrecognized option:",
            "unrecognized vm option",
            "error: could not create the java virtual machine",
            "error occurred during initialization of vm",
            "improperly specified vm option",
            "invalid maximum heap size",
            "invalid initial heap size",
            "too small maximum heap",
            "missing or invalid option",
            "could not find or load main class net.minecraft",
        ],
        summary: "JVM 参数无法识别或虚拟机初始化失败。",
        advice: "检查版本设置中的 JVM 参数：删除无法识别的选项，确认 -Xmx / -Xms 数值合法且不超过物理内存。",
    },
    Rule {
        category: DiagnosticCategory::JavaVersionIncompatible,
        severity: DiagnosticSeverity::Critical,
        patterns: &[
            "unsupported class file major version",
            "unsupported class file version",
            "unsupportedclassversionerror",
            "has been compiled by a more recent version of the java runtime",
            "java.lang.nosuchfieldexception: ucp",
            "because module java.base does not export",
            "classnotfoundexception: jdk.nashorn.api.scripting.nashornscriptenginefactory",
            "classnotfoundexception: java.lang.invoke.lambdametafactory",
            "the requested compatibility level java_11 could not be set",
            "no such method: sun.misc.unsafe.defineanonymousclass",
            "java.lang.noclassdeffounderror: jdk/nashorn",
        ],
        summary: "Java 版本与游戏或 Mod 不兼容。",
        advice: "在版本设置中改用与游戏和加载器匹配的 Java：1.16.5 及以下常用 Java 8，1.17 常用 Java 16/17，1.18-1.20.4 常用 Java 17，1.20.5 及以上常用 Java 21。",
    },
    Rule {
        category: DiagnosticCategory::ForgeJavaIncompatible,
        severity: DiagnosticSeverity::Critical,
        patterns: &[
            "nosuchmethoderror: sun.security.util.manifestentryverifier",
            "nosuchmethoderror: 'void sun.security.util.manifestentryverifier",
            "java.lang.classcastexception: java.base/jdk",
            "java.lang.classcastexception: class jdk.",
        ],
        summary: "Forge 与当前 Java 版本不兼容。",
        advice: "把 Forge 更新到支持当前 Java 的构建，或改用该 Forge 版本要求的 Java；例如 1.12.2 优先使用 Java 8。",
    },
    Rule {
        category: DiagnosticCategory::MixinFailure,
        severity: DiagnosticSeverity::Error,
        patterns: &[
            "mixin prepare failed",
            "mixin apply failed",
            "mixinapplyerror",
            "mixintransformererror",
            "mixin.injection.throwables.",
            ".json] failed during",
            "org.spongepowered.asm.launch.mixintweaker",
        ],
        summary: "Mixin 注入失败，游戏在加载阶段退出。",
        advice: "查看报错中的 mod id，先禁用最近安装或与加载器版本不匹配的 Mod；缺少 MixinBootstrap 时安装对应版本后重试。",
    },
    Rule {
        category: DiagnosticCategory::OptiFineForgeIncompatible,
        severity: DiagnosticSeverity::Error,
        patterns: &[
            "spritecontents.<init>",
            "rendersystem.getbackenddescription",
            "bakedquad.<init>",
            "forgegui.renderselecteditemname",
            "font.ellipsize",
            "shaders mod detected. please remove it, optifine has built-in support for shaders",
        ],
        summary: "OptiFine 与 Forge 版本不兼容。",
        advice: "换成与当前 Forge 构建匹配的 OptiFine，或移除 OptiFine 后用等价渲染 Mod 启动。",
    },
    Rule {
        category: DiagnosticCategory::MissingModDependency,
        severity: DiagnosticSeverity::Error,
        patterns: &[
            "missing or unsupported mandatory dependencies:",
            "missing dependency",
            "mod resolution failed",
            "requires any version of",
            "requires a version of",
        ],
        summary: "缺少 Mod 前置依赖或依赖版本不匹配。",
        advice: "按日志列出的 mod id 安装缺失前置，并确认前置版本与 Minecraft / 加载器匹配。",
    },
    Rule {
        category: DiagnosticCategory::ModConflict,
        severity: DiagnosticSeverity::Error,
        patterns: &[
            "incompatible mods found!",
            "mod resolution encountered an incompatible mod set",
            "some of your mods are incompatible",
        ],
        summary: "检测到 Mod 之间的兼容性冲突。",
        advice: "按日志列出的冲突 Mod 逐个禁用排查，并更新到与当前 Minecraft / 加载器兼容的版本。",
    },
    Rule {
        category: DiagnosticCategory::ModInitializationFailure,
        severity: DiagnosticSeverity::Error,
        patterns: &[
            "failed to create mod instance",
            "caught exception from ",
            "loaderexceptionmodcrash: caught exception from ",
            "due to errors, provided by ",
        ],
        summary: "Mod 初始化或加载器抛出异常。",
        advice: "禁用日志中提到的 Mod，逐个排查；把该 Mod 更新到与当前加载器兼容的版本。",
    },
    Rule {
        category: DiagnosticCategory::DuplicateMod,
        severity: DiagnosticSeverity::Error,
        patterns: &[
            "duplicatemodsfoundexception",
            "found a duplicate mod",
            "found duplicate mods",
            "modresolutionexception: duplicate",
            "multiple entries with same key:",
        ],
        summary: "检测到重复安装的 Mod。",
        advice: "删除 mods 目录中重复的同一个 Mod，只保留一个版本后重新启动。",
    },
    Rule {
        category: DiagnosticCategory::ForgeInstallIncomplete,
        severity: DiagnosticSeverity::Error,
        patterns: &[
            "cannot find launch target fmlclient",
            "invalid paths argument, contained no existing paths",
            "found multiple arguments for option fml.forgeversion",
        ],
        summary: "Forge 安装文件不完整或版本 JSON 异常。",
        advice: "重新安装一次相同版本的 Forge，确认 libraries 中对应文件完整后启动。",
    },
    Rule {
        category: DiagnosticCategory::GraphicsDriverIssue,
        severity: DiagnosticSeverity::Error,
        patterns: &[
            "the driver does not appear to support opengl",
            "couldn't set pixel format",
            "pixel format not accelerated",
            "1282: invalid operation",
            "maybe try a lower resolution resourcepack?",
        ],
        summary: "显卡驱动或 OpenGL 能力不足。",
        advice: "更新或回退显卡驱动，改用独立显卡启动；仍失败时降低分辨率或移除光影后排查。",
    },
];

fn build_sources(input: &DiagnosticInput) -> Vec<BoundedSource> {
    let mut sources = Vec::new();
    if let Some(crash_report) = input.crash_report.as_deref() {
        if !crash_report.trim().is_empty() {
            sources.push(bound_source(
                DiagnosticSource::CrashReport,
                crash_report,
                MAX_CRASH_REPORT_BYTES,
                true,
            ));
        }
    }
    if let Some(latest_log) = input.latest_log.as_deref() {
        if !latest_log.trim().is_empty() {
            sources.push(bound_source(
                DiagnosticSource::LatestLog,
                latest_log,
                MAX_LATEST_LOG_BYTES,
                false,
            ));
        }
    }
    if !input.stderr_tail.trim().is_empty() {
        sources.push(bound_source(
            DiagnosticSource::Stderr,
            &input.stderr_tail,
            MAX_STREAM_BYTES,
            false,
        ));
    }
    if !input.stdout_tail.trim().is_empty() {
        sources.push(bound_source(
            DiagnosticSource::Stdout,
            &input.stdout_tail,
            MAX_STREAM_BYTES,
            false,
        ));
    }
    for source in &mut sources {
        source.truncated |= input.input_truncated;
    }
    sources
}

struct BoundedSource {
    source: DiagnosticSource,
    lines: Vec<String>,
    text: String,
    truncated: bool,
}

fn bound_source(
    source: DiagnosticSource,
    raw: &str,
    max_bytes: usize,
    keep_head: bool,
) -> BoundedSource {
    let mut truncated = raw.len() > max_bytes;
    let bounded = if truncated {
        if keep_head {
            head_tail_text(raw, max_bytes)
        } else {
            tail_at_char_boundary(raw, max_bytes).to_string()
        }
    } else {
        raw.to_string()
    };
    let normalized = normalize_source(&bounded);
    let mut lines: Vec<String> = normalized.lines().map(str::to_owned).collect();

    if lines.len() > MAX_LINES_PER_SOURCE {
        truncated = true;
        if keep_head {
            let head_count = MAX_LINES_PER_SOURCE / 4;
            let tail_count = MAX_LINES_PER_SOURCE - head_count;
            let mut kept = Vec::with_capacity(MAX_LINES_PER_SOURCE);
            kept.extend_from_slice(&lines[..head_count]);
            kept.extend_from_slice(&lines[lines.len() - tail_count..]);
            lines = kept;
        } else {
            lines = lines[lines.len() - MAX_LINES_PER_SOURCE..].to_vec();
        }
    }

    let text = lines.join("\n");
    BoundedSource {
        source,
        lines,
        text,
        truncated,
    }
}

fn normalize_source(text: &str) -> String {
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

fn head_tail_text(text: &str, max_bytes: usize) -> String {
    let marker = format!("\n{HEAD_TAIL_MARKER}\n");
    let budget = max_bytes.saturating_sub(marker.len()).max(1);
    let head_budget = budget / 3;
    let tail_budget = budget - head_budget;
    let head_end = floor_char_boundary(text, head_budget);
    let tail_start = ceil_char_boundary(text, text.len().saturating_sub(tail_budget));

    let mut out = String::with_capacity(marker.len() + head_end + (text.len() - tail_start));
    out.push_str(&text[..head_end]);
    out.push_str(&marker);
    out.push_str(&text[tail_start..]);
    out
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn collect_evidence(
    sources: &[BoundedSource],
    patterns: &[&str],
    category: DiagnosticCategory,
) -> Vec<DiagnosticEvidence> {
    let mut evidence = Vec::new();
    'sources: for source in sources {
        for (index, line) in source.lines.iter().enumerate() {
            let lower = line.to_ascii_lowercase();
            let matched_pattern = patterns
                .iter()
                .copied()
                .find(|pattern| !pattern.is_empty() && lower.contains(pattern));
            if let Some(pattern) = matched_pattern {
                let excerpt = evidence_excerpt(line, pattern);
                if excerpt.is_empty() {
                    continue;
                }
                evidence.push(DiagnosticEvidence {
                    source: source.source,
                    line: index + 1,
                    excerpt,
                    summary: format!("匹配{}特征", category.label()),
                });
                if evidence.len() >= MAX_EVIDENCE_PER_FINDING {
                    break 'sources;
                }
            }
        }
    }
    evidence
}

fn evidence_excerpt(line: &str, needle: &str) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let total_chars = trimmed.chars().count();
    let (start_char, end_char) = if total_chars <= MAX_EVIDENCE_CHARS {
        (0, total_chars)
    } else {
        let lower = trimmed.to_ascii_lowercase();
        let match_byte = if needle.is_empty() {
            0
        } else {
            lower.find(needle).unwrap_or(0)
        };
        let match_char = trimmed[..match_byte].chars().count();
        let mut start = match_char.saturating_sub(MAX_EVIDENCE_CHARS / 2);
        let mut end = (start + MAX_EVIDENCE_CHARS).min(total_chars);
        if end == total_chars {
            start = total_chars.saturating_sub(MAX_EVIDENCE_CHARS);
            end = total_chars;
        }
        (start, end)
    };

    let mut excerpt: String = trimmed
        .chars()
        .skip(start_char)
        .take(end_char - start_char)
        .collect();
    if start_char > 0 {
        excerpt.insert_str(0, "...");
    }
    if end_char < total_chars {
        excerpt.push_str("...");
    }
    redact_sensitive_text(&excerpt)
}

fn add_contextual_findings(
    input: &DiagnosticInput,
    sources: &[BoundedSource],
    findings: &mut Vec<DiagnosticFinding>,
) {
    let combined = sources
        .iter()
        .map(|source| source.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    let loader = input.loader.to_ascii_lowercase();
    let has_optifine = input
        .optifine
        .as_ref()
        .is_some_and(|version| !version.trim().is_empty())
        || combined.contains("optifine");
    let has_forge = loader.contains("forge")
        || combined.contains("net.minecraftforge")
        || combined.contains("forge");
    let has_no_such_member =
        combined.contains("nosuchmethoderror") || combined.contains("nosuchfielderror");

    if has_optifine && has_forge && has_no_such_member {
        let evidence = collect_evidence(
            sources,
            &[
                "optifine",
                "net.minecraftforge",
                "nosuchmethoderror",
                "nosuchfielderror",
            ],
            DiagnosticCategory::OptiFineForgeIncompatible,
        );
        push_finding(
            findings,
            DiagnosticFinding {
                category: DiagnosticCategory::OptiFineForgeIncompatible,
                severity: DiagnosticSeverity::Error,
                summary: "检测到 OptiFine 与 Forge 同时出现方法签名不匹配。".into(),
                advice:
                    "换成与当前 Forge 构建匹配的 OptiFine，或移除 OptiFine 后用等价渲染 Mod 启动。"
                        .into(),
                evidence,
            },
        );
    }

    let has_access_violation = combined.contains("exception_access_violation");
    let has_driver_frame =
        combined.contains("nvoglv") || combined.contains("atio") || combined.contains("# c  [ig");
    if has_access_violation && has_driver_frame {
        let evidence = collect_evidence(
            sources,
            &["exception_access_violation", "nvoglv", "atio", "# c  [ig"],
            DiagnosticCategory::GraphicsDriverIssue,
        );
        push_finding(
            findings,
            DiagnosticFinding {
                category: DiagnosticCategory::GraphicsDriverIssue,
                severity: DiagnosticSeverity::Error,
                summary: "显卡驱动在 OpenGL 调用中触发访问冲突。".into(),
                advice:
                    "更新或回退显卡驱动，改用独立显卡启动；仍失败时降低分辨率或移除光影后排查。"
                        .into(),
                evidence,
            },
        );
    }
}

fn push_finding(findings: &mut Vec<DiagnosticFinding>, finding: DiagnosticFinding) {
    if let Some(existing) = findings
        .iter_mut()
        .find(|item| item.category == finding.category)
    {
        for evidence in finding.evidence {
            if existing.evidence.len() >= MAX_EVIDENCE_PER_FINDING {
                break;
            }
            if !existing
                .evidence
                .iter()
                .any(|item| item.source == evidence.source && item.line == evidence.line)
            {
                existing.evidence.push(evidence);
            }
        }
        return;
    }
    findings.push(finding);
}

fn fallback_text(input: &DiagnosticInput, sources: &[BoundedSource]) -> String {
    if sources.is_empty() {
        return "没有捕获到游戏输出、logs/latest.log 或 crash-reports".into();
    }
    match input.exit_code {
        Some(code) if code != 0 => {
            format!("退出码 {code} 非零，但日志未匹配已知崩溃特征")
        }
        _ => "日志未匹配已知崩溃特征".into(),
    }
}

fn unknown_finding(input: &DiagnosticInput, sources: &[BoundedSource]) -> DiagnosticFinding {
    let no_sources = sources.is_empty();
    let severity = match input.exit_code {
        Some(code) if code != 0 => DiagnosticSeverity::Error,
        _ if no_sources => DiagnosticSeverity::Warning,
        _ => DiagnosticSeverity::Info,
    };
    DiagnosticFinding {
        category: DiagnosticCategory::Unknown,
        severity,
        summary: if no_sources {
            "没有可分析的游戏输出或崩溃报告。".into()
        } else {
            "日志未匹配已知崩溃特征。".into()
        },
        advice: if no_sources {
            "先确认 Java 路径与 JVM 参数；启动后在 logs/latest.log 或 crash-reports 生成记录时再分析。"
                .into()
        } else {
            "查看 crash-reports 与 logs/latest.log 的完整堆栈；先禁用最近安装的 Mod，并确认 Java 版本与加载器匹配后重试。"
                .into()
        },
        evidence: unknown_evidence(sources),
    }
}

fn unknown_evidence(sources: &[BoundedSource]) -> Vec<DiagnosticEvidence> {
    let mut evidence = Vec::new();
    for source in sources {
        if let Some((index, line)) = source
            .lines
            .iter()
            .enumerate()
            .rev()
            .find(|(_, line)| !line.trim().is_empty())
        {
            let excerpt = evidence_excerpt(line, "");
            if !excerpt.is_empty() {
                evidence.push(DiagnosticEvidence {
                    source: source.source,
                    line: index + 1,
                    excerpt,
                    summary: "最后一段输出".into(),
                });
            }
        }
        if evidence.len() >= MAX_EVIDENCE_PER_FINDING {
            break;
        }
    }
    evidence
}

fn context_from_input(input: &DiagnosticInput) -> DiagnosticContext {
    DiagnosticContext {
        exit_code: input.exit_code,
        minecraft_version: input.minecraft_version.clone(),
        loader: input.loader.clone(),
        optifine: input.optifine.clone(),
        java_major: input.java_major,
        java_vendor: input.java_vendor.clone(),
    }
}
