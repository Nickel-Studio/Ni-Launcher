//! 主动内存优化。
//!
//! Windows 遍历全部可访问进程回收工作集，可额外清空待命内存列表；
//! Unix 平台回收系统缓存，不按进程统计释放量。
//!
//! 权限模型：
//! - Windows：完整优化需要管理员权限。未提权时用 `runas` 拉起自身子进程并弹 UAC；
//!   子进程完成优化后把结果 JSON 写回临时文件，父进程读取后展示。
//! - macOS / Linux：按 normal → sudo → root → 跳过的顺序尝试。sudo / root 需要密码时
//!   由前端在应用内弹窗收集；密码只写入子进程 stdin，不落盘、不回传、不写日志。
//!
//! 设计约束：
//! - 任何失败都返回结构化结果，绝不 panic；调用方据此决定 toast 还是仅记录警告。
//! - 同一时刻只允许一次优化，避免连点触发多个 UAC 弹窗；提权线程仍在等待 UAC 时
//!   也视为忙碌（`ELEVATION_PENDING`），避免超时重试叠加出第二个授权弹窗。
//! - 超时是 best-effort：循环边界与子进程轮询点会检查截止时间，但 UAC 弹窗、单个
//!   Win32 / shell 调用无法被强制中断，实际耗时可能略超预算。超时后调用方立即返回，
//!   UAC 仍需用户在桌面上处理；提权子线程会在 UAC 返回后回收仍存活的子进程。

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::path::Path;
#[cfg(any(windows, unix))]
use std::time::Instant;

/// 优化执行状态。`Busy` 表示已有一次优化在进行中，本次请求被忽略。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MemoryOptimizeStatus {
    Success,
    /// 用户拒绝了 UAC，或没有任何可用的提权途径。
    PermissionDenied,
    /// 未获得提权能力时按「跳过并继续」处理（启动前优化专用）。
    Skipped,
    Failed,
    Busy,
}

/// 内存优化结果。提权子进程通过 JSON 把该结构写回父进程。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MemoryOptimizeReport {
    pub status: MemoryOptimizeStatus,
    /// 枚举到的进程总数（Unix 上为 0：清理的是系统页缓存而非进程工作集）。
    pub total_processes: usize,
    /// 成功回收工作集的进程数。
    pub succeeded: usize,
    /// 因权限不足无法打开的进程数。受保护进程即使管理员也无法打开，因此不会归零。
    pub access_denied: usize,
    /// 已退出或无需处理的进程数。
    pub skipped: usize,
    /// 系统可用物理内存的变化量（字节），可能为负；这是用户能在任务管理器里核对的数字。
    pub available_delta_bytes: i64,
    /// 是否成功清空待命内存列表（仅 Windows）。
    pub standby_purged: bool,
    pub elapsed_ms: u64,
    /// 实际使用的权限路径：normal / sudo / root / uac / skipped。
    pub elevation: String,
    /// 失败或跳过时的中文说明；成功时为 None。
    pub message: Option<String>,
}

impl MemoryOptimizeReport {
    fn blank(status: MemoryOptimizeStatus, elevation: &str) -> Self {
        Self {
            status,
            total_processes: 0,
            succeeded: 0,
            access_denied: 0,
            skipped: 0,
            available_delta_bytes: 0,
            standby_purged: false,
            elapsed_ms: 0,
            elevation: elevation.to_string(),
            message: None,
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            ..Self::blank(MemoryOptimizeStatus::Failed, "skipped")
        }
    }

    fn permission_denied(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            ..Self::blank(MemoryOptimizeStatus::PermissionDenied, "skipped")
        }
    }

    /// macOS / Linux 与移动端使用；Windows 构建保留该构造函数以共享同一套结果结构。
    #[allow(dead_code)]
    fn skipped(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            ..Self::blank(MemoryOptimizeStatus::Skipped, "skipped")
        }
    }

    fn busy() -> Self {
        Self::blank(MemoryOptimizeStatus::Busy, "skipped")
    }
}

/// 当前平台的提权能力，用于前端决定「直接执行 / 弹密码框 / 直接提示无法运行」。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MemoryOptimizeCapability {
    /// windows / macos / linux / other。
    pub platform: String,
    /// Windows：当前进程已提权；Unix：euid == 0。
    pub elevated: bool,
    /// 需要用户输入 sudo / root 密码才能真正执行。
    pub needs_password: bool,
    pub has_sudo: bool,
    /// 只有 Windows 支持清空待命内存列表。
    pub supports_standby_purge: bool,
    /// 当前平台是否存在可用的完整优化路径。
    pub available: bool,
    pub message: Option<String>,
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// UAC 子线程是否仍在运行（从发起提权到该线程退出）。
///
/// `RUNNING` 会在父线程超时返回时释放，但此时子线程可能仍阻塞在 `ShellExecuteExW`
/// 的 UAC 弹窗上；若只看 `RUNNING`，用户超时后再点一次就会叠加第二个 UAC。
static ELEVATION_PENDING: AtomicBool = AtomicBool::new(false);

/// 重入保护：同时只允许一次优化，避免连点产生多个 UAC / 密码请求。
struct BusyGuard;

impl Drop for BusyGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Release);
    }
}

/// 提权线程的兜底清理：无论线程从哪个分支退出（含 panic），都清除 pending 标记。
#[cfg(windows)]
struct ElevationPendingGuard;

#[cfg(windows)]
impl Drop for ElevationPendingGuard {
    fn drop(&mut self) {
        ELEVATION_PENDING.store(false, Ordering::Release);
    }
}

fn acquire() -> Option<BusyGuard> {
    // 先抢占 RUNNING，再检查提权线程：两个静态量分别覆盖「本次执行」与
    // 「上一次提权还在等 UAC」两种忙碌状态。
    RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .ok()?;
    if ELEVATION_PENDING.load(Ordering::Acquire) {
        // 本次没有获得执行权，必须回滚 RUNNING，否则之后的请求会被永久拒绝。
        RUNNING.store(false, Ordering::Release);
        return None;
    }
    Some(BusyGuard)
}

/// 判断是否已到达截止时间；`None` 表示本次调用不设总超时。
#[cfg(any(windows, unix))]
fn deadline_exceeded(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|deadline| Instant::now() >= deadline)
}

/// 把相对超时换算为绝对截止时间。
///
/// 极端大的 `Duration` 溢出时返回 `None`（不设截止），绝不 panic；正常的 40s/120s
/// 预算不会触发该分支。超时本身是 best-effort：只能在循环边界与子进程轮询点检查。
#[cfg(any(windows, unix))]
fn deadline_after(timeout: Duration) -> Option<Instant> {
    Instant::now().checked_add(timeout)
}

/// 可选的取消检查回调：返回 true 表示调用方要求尽快结束本次优化。
///
/// 使用回调而不是 `AtomicBool`，是因为启动流程的取消令牌是核心库的
/// `CancellationToken`（内部标志不对外暴露）；`None` 表示调用方不响应取消。
pub(crate) type CancelCheck<'a> = Option<&'a dyn Fn() -> bool>;

/// 结果文件名前缀（父进程与提权子进程通过 argv 约定的临时文件）。
#[cfg(any(windows, test))]
const RESULT_FILE_PREFIX: &str = "nowhere-memory-optimize-";
/// 结果文件名随机后缀的字节数。16 字节（128 位）已足够抵抗猜测；
/// 取 24 字节是为了让 `%TEMP%` 里的文件名也足够不透明。
#[cfg(any(windows, test))]
const RESULT_FILE_RANDOM_BYTES: usize = 24;

/// 生成密码学安全的随机文件名后缀（小写 hex）。
///
/// 为什么不用 `store::random_token()`：它由时间 / PID / 计数器混合而成，只保证
/// 唯一、不保证不可预测。提权子进程的结果路径如果可被本地攻击者预测，对方就能
/// 提前放置符号链接 / 硬链接抢占路径。这里直接复用 `aes-gcm`（已是直接依赖）
/// 转出的 `OsRng` —— 与 `vault.rs` 使用同一密码学随机源，无需新增 crate。
#[cfg(any(windows, test))]
fn random_result_suffix() -> String {
    use aes_gcm::aead::{OsRng, rand_core::RngCore};

    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0u8; RESULT_FILE_RANDOM_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let mut out = String::with_capacity(RESULT_FILE_RANDOM_BYTES * 2);
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// 提权子进程结果文件的路径：每次生成新的随机文件名。
#[cfg(any(windows, test))]
fn result_file_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "{RESULT_FILE_PREFIX}{}.json",
        random_result_suffix()
    ))
}

/// 独占创建结果文件并写入。
///
/// 为什么用 `create_new`：父进程以管理员权限拉起的子进程会写这个路径。如果路径
/// 预先被符号链接 / 硬链接 / Windows 重解析点占用，普通 `fs::write` 会跟随链接
/// 覆盖链接目标（典型攻击是指向系统文件）。`create_new` 在 Unix 上映射为
/// `O_CREAT | O_EXCL`（目标已存在、含符号链接时直接 EEXIST，不跟随），在 Windows
/// 上映射为 `CREATE_NEW`（任何已存在的路径都会失败）。
fn write_result_exclusive(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(text.as_bytes())
}

/// 读取结果文件；路径自身是符号链接 / 重解析点时拒绝。
///
/// 子进程正常写入的是普通文件；若读取时目标已变成链接，说明 `%TEMP%` 下的文件
/// 被替换，宁可退化为「已完成但无统计」也不能跟随链接读取任意文件。
#[cfg(any(windows, test))]
fn read_result_file(path: &Path) -> Option<MemoryOptimizeReport> {
    // 复用实例模块的单点判定，避免符号链接 / Windows 重解析点出现第二套实现。
    crate::instances::reject_links_local(path).ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<MemoryOptimizeReport>(&text).ok()
}

/// 提权前的取消判断：`ShellExecuteExW` 会立刻弹出 UAC，调用前必须再确认一次。
///
/// `Acquire` 与父线程写入时的 `Release` 配对，确保读到最新的取消标记。
#[cfg(any(windows, test))]
fn cancelled_before_elevation(cancelled: &AtomicBool) -> bool {
    cancelled.load(Ordering::Acquire)
}

/// 提权子进程入口：`--memory-optimize <结果文件> [--standby]`。
///
/// 返回 true 表示本次进程只负责执行内存优化，调用方应立即返回（不要初始化 Tauri）。
pub fn handle_cli_request() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let Some(index) = args.iter().position(|arg| arg == "--memory-optimize") else {
        return false;
    };
    let result_path = args.get(index + 1).cloned().unwrap_or_default();
    let purge_standby = args.iter().any(|arg| arg == "--standby");
    let report = if result_path.is_empty() {
        MemoryOptimizeReport::failed("提权子进程缺少结果文件参数")
    } else {
        // 120 秒既是父进程的等待上限，也是子进程内 `optimize_all` 的截止预算；
        // 父进程超时会回收本进程，因此这里的 deadline 只需覆盖正常完成路径。
        run_with_timeout(purge_standby, None, false, Duration::from_secs(120), None)
    };
    if !result_path.is_empty() {
        // 子进程无需感知写盘失败：父进程读不到结果时会退化为「已完成但无统计」。
        if let Ok(text) = serde_json::to_string(&report) {
            let _ = write_result_exclusive(Path::new(&result_path), &text);
        }
    }
    true
}

/// 完整内存优化（手动触发，允许提权交互）。
pub fn run(purge_standby: bool, password: Option<&str>, allow_prompt: bool) -> MemoryOptimizeReport {
    run_with_timeout(
        purge_standby,
        password,
        allow_prompt,
        Duration::from_secs(120),
        None,
    )
}

/// 启动前优化：Windows 仍会请求 UAC；Unix 只尝试免密路径，失败则跳过并继续。
///
/// `cancel` 用于响应启动取消：优化循环与 Unix 的每次提权尝试都会检查它。
/// 它无法关闭已经出现的系统 UAC 弹窗——那一步只能由用户在桌面上处理。
pub fn run_before_launch(
    purge_standby: bool,
    cancel: CancelCheck<'_>,
) -> MemoryOptimizeReport {
    run_with_timeout(purge_standby, None, true, Duration::from_secs(40), cancel)
}

fn run_with_timeout(
    purge_standby: bool,
    password: Option<&str>,
    allow_prompt: bool,
    timeout: Duration,
    cancel: CancelCheck<'_>,
) -> MemoryOptimizeReport {
    let Some(_guard) = acquire() else {
        return MemoryOptimizeReport::busy();
    };
    imp::run(purge_standby, password, allow_prompt, timeout, cancel)
}

pub fn capability() -> MemoryOptimizeCapability {
    imp::capability()
}

/// Tauri 命令：返回当前平台的提权能力。
#[tauri::command]
pub fn memory_optimize_capability() -> MemoryOptimizeCapability {
    capability()
}

/// Tauri 命令：立即执行一次内存优化。
///
/// `password` 仅在 macOS / Linux 需要 sudo / root 时由前端传入，后端只写入子进程 stdin。
#[tauri::command]
pub async fn memory_optimize_run(
    purge_standby: bool,
    password: Option<String>,
) -> MemoryOptimizeReport {
    tokio::task::spawn_blocking(move || run(purge_standby, password.as_deref(), true))
        .await
        .unwrap_or_else(|error| MemoryOptimizeReport::failed(format!("内存优化线程异常：{error}")))
}

/* ============================================================================
   Windows 实现
   ========================================================================== */

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::Instant;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_SET_QUOTA: u32 = 0x0100;
    const TOKEN_QUERY: u32 = 0x0008;
    const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
    const TOKEN_ELEVATION: u32 = 20;
    const SE_PRIVILEGE_ENABLED: u32 = 0x0002;
    const ERROR_NOT_ALL_ASSIGNED: i32 = 1300;
    const ERROR_CANCELLED: i32 = 1223;
    const SNAP_PROCESS: u32 = 0x0000_0002;
    /// `SHELLEXECUTEINFOW.fMask` 是 32 位 `ULONG`；写成 `usize` 会让 x64 上
    /// 后续所有字段整体后移 8 字节（cbSize 变成 120，正确值 112），
    /// `ShellExecuteExW` 会直接拒绝执行，`runas` 根本不会弹 UAC。
    const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
    const SW_HIDE: i32 = 0;
    const SYSTEM_MEMORY_LIST_INFORMATION: i32 = 0x50;
    const MEMORY_PURGE_STANDBY_LIST: i32 = 4;
    /// 清空待命列表需要 Windows 8.1（build 9600）或更高版本。
    const MIN_BUILD_FOR_STANDBY_PURGE: u32 = 9600;
    /// 工作集页面移入待命列表是异步的，立即读数会低估释放量。
    const SETTLE_MS: u64 = 400;
    /// 等待提权子进程的分片长度：决定取消 / 超时的响应粒度。
    const WAIT_SLICE_MS: u32 = 100;
    /// `WaitForSingleObject` 的返回码。
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 258;

    #[repr(C)]
    struct ProcessEntry {
        size: u32,
        usage: u32,
        pid: u32,
        heap: usize,
        module: u32,
        threads: u32,
        parent: u32,
        priority: i32,
        flags: u32,
        executable: [u16; 260],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Luid {
        low: u32,
        high: i32,
    }

    #[repr(C)]
    struct TokenPrivileges {
        count: u32,
        luid: Luid,
        attributes: u32,
    }

    #[repr(C)]
    struct TokenElevationInfo {
        token_is_elevated: u32,
    }

    #[repr(C)]
    struct ShellExecuteInfoW {
        cb_size: u32,
        f_mask: u32,
        hwnd: *mut c_void,
        lp_verb: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show: i32,
        h_inst_app: *mut c_void,
        lp_id_list: *mut c_void,
        lp_class: *const u16,
        h_key_class: *mut c_void,
        dw_hot_key: u32,
        h_icon_or_monitor: *mut c_void,
        h_process: *mut c_void,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn K32EmptyWorkingSet(process: *mut c_void) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
        fn Process32FirstW(snapshot: *mut c_void, entry: *mut ProcessEntry) -> i32;
        fn Process32NextW(snapshot: *mut c_void, entry: *mut ProcessEntry) -> i32;
        fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn TerminateProcess(process: *mut c_void, exit_code: u32) -> i32;
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
        fn GetTokenInformation(
            token: *mut c_void,
            class: u32,
            info: *mut c_void,
            len: u32,
            out_len: *mut u32,
        ) -> i32;
        fn LookupPrivilegeValueW(system: *const u16, name: *const u16, luid: *mut Luid) -> i32;
        fn AdjustTokenPrivileges(
            token: *mut c_void,
            disable_all: i32,
            new_state: *mut TokenPrivileges,
            len: u32,
            previous: *mut c_void,
            return_len: *mut u32,
        ) -> i32;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteExW(info: *mut ShellExecuteInfoW) -> i32;
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtSetSystemInformation(class: i32, info: *mut c_void, len: u32) -> i32;
    }

    fn wide(text: &str) -> Vec<u16> {
        OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect()
    }

    /// 当前进程是否已获得管理员令牌。
    pub(super) fn elevated() -> bool {
        // SAFETY: 只查询当前进程令牌；句柄在返回前关闭。
        unsafe {
            let mut token: *mut c_void = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut info = TokenElevationInfo { token_is_elevated: 0 };
            let mut size = 0u32;
            let ok = GetTokenInformation(
                token,
                TOKEN_ELEVATION,
                &mut info as *mut TokenElevationInfo as *mut c_void,
                std::mem::size_of::<TokenElevationInfo>() as u32,
                &mut size,
            );
            CloseHandle(token);
            ok != 0 && info.token_is_elevated != 0
        }
    }

    fn enable_privilege(name: &str) -> bool {
        let name = wide(name);
        // SAFETY: LUID / TOKEN_PRIVILEGES 按 Win32 布局初始化，令牌句柄在返回前关闭。
        unsafe {
            let mut token: *mut c_void = std::ptr::null_mut();
            if OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES,
                &mut token,
            ) == 0
            {
                return false;
            }
            let mut luid = Luid { low: 0, high: 0 };
            if LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) == 0 {
                CloseHandle(token);
                return false;
            }
            let mut state = TokenPrivileges {
                count: 1,
                luid,
                attributes: SE_PRIVILEGE_ENABLED,
            };
            let adjusted = AdjustTokenPrivileges(
                token,
                0,
                &mut state,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) != 0;
            // 部分权限未授予时 AdjustTokenPrivileges 仍返回 TRUE，必须再核对错误码。
            let granted = std::io::Error::last_os_error().raw_os_error()
                != Some(ERROR_NOT_ALL_ASSIGNED);
            CloseHandle(token);
            adjusted && granted
        }
    }

    fn available_memory_bytes() -> Option<u64> {
        crate::runtime_settings::physical_memory_bytes().1
    }

    fn snapshot_pids() -> Vec<u32> {
        let mut pids = Vec::new();
        // SAFETY: 快照句柄在枚举后立即关闭；结构体按 Win32 布局初始化。
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(SNAP_PROCESS, 0);
            if snapshot as isize == -1 {
                return pids;
            }
            let mut entry: ProcessEntry = std::mem::zeroed();
            entry.size = std::mem::size_of::<ProcessEntry>() as u32;
            let mut present = Process32FirstW(snapshot, &mut entry) != 0;
            while present {
                pids.push(entry.pid);
                present = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        pids
    }

    /// 清空系统待命内存列表（分页回收）。失败时静默返回 false。
    fn try_purge_standby() -> bool {
        if crate::runtime_settings::windows_build()
            .is_some_and(|build| build < MIN_BUILD_FOR_STANDBY_PURGE)
        {
            return false;
        }
        if !enable_privilege("SeProfileSingleProcessPrivilege") {
            return false;
        }
        let mut command = MEMORY_PURGE_STANDBY_LIST;
        // SAFETY: MemoryPurgeStandbyList 只读取一个 int；失败时返回 NTSTATUS。
        let status = unsafe {
            NtSetSystemInformation(
                SYSTEM_MEMORY_LIST_INFORMATION,
                &mut command as *mut i32 as *mut c_void,
                std::mem::size_of::<i32>() as u32,
            )
        };
        status >= 0
    }

    /// 优化循环被什么原因提前终止。
    #[derive(Clone, Copy)]
    enum StopReason {
        Deadline,
        Cancelled,
    }

    /// 提前终止时保留已完成统计，只把状态与说明改成失败。
    fn mark_stopped(report: &mut MemoryOptimizeReport, reason: StopReason) {
        report.status = MemoryOptimizeStatus::Failed;
        let detail = format!(
            "已处理 {} 个进程（成功 {}，权限不足 {}，跳过 {}）",
            report.succeeded + report.access_denied + report.skipped,
            report.succeeded,
            report.access_denied,
            report.skipped
        );
        report.message = Some(match reason {
            StopReason::Deadline => format!("内存优化超时：{detail}，剩余步骤已停止"),
            StopReason::Cancelled => format!("启动已取消，内存优化中止：{detail}"),
        });
    }

    /// 回收全部进程的工作集，可选清空待命列表。
    ///
    /// 超时是 best-effort：只在循环检查点（每 32 个 PID）、清空待命列表前比较
    /// 截止时间，单个 `OpenProcess` / `K32EmptyWorkingSet` 调用不可中断，实际耗时
    /// 可能略超预算；超时/取消时保留已有统计并停止后续处理。
    fn optimize_all(
        purge_standby: bool,
        elevation: &str,
        deadline: Option<Instant>,
        cancel: CancelCheck<'_>,
    ) -> MemoryOptimizeReport {
        let started = Instant::now();
        let before = available_memory_bytes();
        // 调试权限用于处理以其他账户运行的进程；未提权时通常会失败，属正常情况。
        let _ = enable_privilege("SeDebugPrivilege");

        let pids = snapshot_pids();
        let mut report = MemoryOptimizeReport::blank(MemoryOptimizeStatus::Success, elevation);
        report.total_processes = pids.len();
        let mut stopped = None;

        for (index, pid) in pids.into_iter().enumerate() {
            // 每 32 个 PID 检查一次截止时间与取消标记：逐进程检查会引入不必要的
            // 时钟读取，间隔过大又会让超时 / 取消拖尾。
            if index % 32 == 0 {
                if cancel.is_some_and(|check| check()) {
                    stopped = Some(StopReason::Cancelled);
                    break;
                }
                if deadline_exceeded(deadline) {
                    stopped = Some(StopReason::Deadline);
                    break;
                }
            }
            // PID 0（Idle）与 PID 4（System）无法打开，直接跳过以免污染统计。
            if pid <= 4 {
                report.skipped += 1;
                continue;
            }
            // SAFETY: 只打开快照中存在的进程；句柄在每次循环结束前关闭。
            unsafe {
                let handle = OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA,
                    0,
                    pid,
                );
                if handle.is_null() {
                    if std::io::Error::last_os_error().raw_os_error() == Some(5) {
                        report.access_denied += 1;
                    } else {
                        report.skipped += 1;
                    }
                    continue;
                }
                if K32EmptyWorkingSet(handle) != 0 {
                    report.succeeded += 1;
                } else {
                    report.skipped += 1;
                }
                CloseHandle(handle);
            }
        }

        if stopped.is_none() && purge_standby {
            // 清空待命列表可能较慢，执行前再确认一次预算。
            if deadline_exceeded(deadline) {
                stopped = Some(StopReason::Deadline);
            } else {
                report.standby_purged = try_purge_standby();
            }
        }

        // 工作集页面移入待命列表是异步的，等待后读数更接近真实释放量；
        // 被提前终止时不再等待，此时读数会低估释放量。
        if stopped.is_none() {
            std::thread::sleep(Duration::from_millis(SETTLE_MS));
        }
        if let Some(reason) = stopped {
            mark_stopped(&mut report, reason);
        }
        if let (Some(before), Some(after)) = (before, available_memory_bytes()) {
            report.available_delta_bytes = after as i64 - before as i64;
        }
        report.elapsed_ms = started.elapsed().as_millis() as u64;
        report
    }

    /// 终止仍存活的提权子进程，并等待句柄进入已退出状态（最多 5 秒）。
    ///
    /// 返回 true 表示已确认进程退出（或此前就已退出），false 表示无法回收。
    /// 句柄由调用方关闭；`TerminateProcess` 对已退出进程会返回失败，属正常情况，
    /// 因此失败后还要用 0 超时等待确认它确实不在运行。
    fn terminate_child(handle: *mut c_void) -> bool {
        // SAFETY: handle 来自 ShellExecuteExW，在本次调用期间保持有效。
        unsafe {
            if TerminateProcess(handle, 1) != 0 {
                return WaitForSingleObject(handle, 5_000) == WAIT_OBJECT_0;
            }
            // 进程可能已经退出：TerminateProcess 失败不代表回收失败。
            WaitForSingleObject(handle, 0) == WAIT_OBJECT_0
        }
    }

    /// 删除提权子进程的结果文件；不存在视为成功。
    ///
    /// 所有提前返回路径（UAC 取消、超时、等待失败）都必须调用它，避免在
    /// `%TEMP%/nowhere-memory-optimize-*.json` 残留上一次的优化结果。
    fn remove_result_file(path: &Path) -> bool {
        match std::fs::remove_file(path) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(_) => false,
        }
    }

    /// 以管理员权限拉起自身子进程执行优化，并读取它写回的结果。
    ///
    /// 为什么把整个提权流程放进子线程：`ShellExecuteExW` 在用户处理 UAC 之前不会返回，
    /// 若在调用线程上直接等待，超时只覆盖之后的 `WaitForSingleObject`，用户把 UAC 弹窗
    /// 晾在一边时调用方会无限期阻塞。这里改为子线程执行 + `recv_timeout` 控制总时长。
    ///
    /// 超时后的语义（重要）：
    /// - 调用方立即返回失败结果，但**不会**替用户关闭系统 UAC 弹窗；弹窗仍需用户在
    ///   桌面上批准或取消，取消标记只能让子线程在 `ShellExecuteExW` 返回后尽快回收
    ///   提权进程。
    /// - `ELEVATION_PENDING` 从派生线程开始一直保持到该线程结束：等待期间新的优化
    ///   请求一律返回忙碌，避免超时后重复点击叠加出第二个 UAC。
    /// - 提权进程句柄由子线程持有，因此由它在 UAC 返回后终止并回收仍存活的子进程；
    ///   父线程无法直接关闭这个句柄。
    ///
    /// `cancel` 在派生线程前与等待期间都会检查：系统 UAC 弹窗本身无法被进程内
    /// 标记关闭，但一旦取消，子线程会在 UAC 返回后立即回收提权进程。
    fn elevate_and_run(
        purge_standby: bool,
        timeout: Duration,
        cancel: CancelCheck<'_>,
    ) -> MemoryOptimizeReport {
        // 启动流程已取消时不要再请求提权，避免用户已经点了取消还弹 UAC。
        if cancel.is_some_and(|check| check()) {
            return MemoryOptimizeReport::failed("启动已取消，内存优化未执行");
        }
        let result_path = result_file_path();
        let executable = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                let _ = remove_result_file(&result_path);
                return MemoryOptimizeReport::failed(format!("无法定位启动器可执行文件：{error}"));
            }
        };
        let parameters = if purge_standby {
            format!(
                "\"--memory-optimize\" \"{}\" \"--standby\"",
                result_path.display()
            )
        } else {
            format!("\"--memory-optimize\" \"{}\"", result_path.display())
        };

        // `ShellExecuteInfoW` 及其中的原始指针不是 Send：结构体必须在子线程内构建，
        // 跨线程只能传递拥有的 String / PathBuf / bool / Duration。
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel::<MemoryOptimizeReport>();
        // 先置位再创建线程：如果在线程内部置位，父线程超时释放 RUNNING 与子线程置位
        // 之间会留下一个窗口，让重复点击绕过忙碌保护。
        ELEVATION_PENDING.store(true, Ordering::Release);
        {
            let cancelled = Arc::clone(&cancelled);
            // 闭包需要 'static：克隆一份路径给线程，外层的 result_path 留给错误分支清理。
            let thread_result_path = result_path.clone();
            let spawned = std::thread::Builder::new()
                .name("memory-optimize-uac".into())
                .spawn(move || {
                    // 无论线程从哪条路径退出（含 panic），都必须清除 pending，
                    // 否则之后的优化请求会被永久拒绝。
                    let _pending = ElevationPendingGuard;
                    let report = run_elevated(
                        &executable,
                        &parameters,
                        &thread_result_path,
                        &cancelled,
                        timeout,
                    );
                    // 父线程超时后已经放弃接收：发送失败属预期情况，直接丢弃报告。
                    let _ = sender.send(report);
                });
            if let Err(error) = spawned {
                // 线程没创建成功：自行回滚 pending，并清理临时结果文件路径。
                ELEVATION_PENDING.store(false, Ordering::Release);
                let _ = remove_result_file(&result_path);
                return MemoryOptimizeReport::failed(format!("无法创建内存优化线程：{error}"));
            }
        }

        // 分片等待：既能在启动取消时提前返回（UAC 弹窗无法关闭，但可以先置位
        // 取消标记，让子线程在 UAC 返回后立即回收提权进程），也能累计到总超时。
        let Some(wait_deadline) = deadline_after(timeout) else {
            // 极端超时值（正常 40s/120s 不会走到）：仍要置位取消，让子线程在 UAC
            // 返回后回收提权进程，不能因为换算失败留下一个等待授权的孤儿进程。
            cancelled.store(true, Ordering::Release);
            let _ = remove_result_file(&result_path);
            return MemoryOptimizeReport::failed("等待时间超出范围");
        };
        loop {
            if cancel.is_some_and(|check| check()) {
                cancelled.store(true, Ordering::Release);
                let _ = remove_result_file(&result_path);
                return MemoryOptimizeReport::failed("启动已取消，内存优化中止");
            }
            let remaining = wait_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                // 子线程可能还阻塞在 UAC 上：置位取消标记，它会在 ShellExecuteExW 返回后
                // 立刻终止并回收提权子进程（句柄只存在于子线程，父线程无法直接关闭）。
                // 注意：这里只能返回，UAC 弹窗仍需用户自行处理后才会消失。
                cancelled.store(true, Ordering::Release);
                // 父线程返回前先清一次结果文件：子进程此刻还没写回，之后子线程还会再清一次；
                // 提前删除可保证调用方拿到超时结果时不会残留上一次的旧文件。
                let _ = remove_result_file(&result_path);
                return MemoryOptimizeReport::failed("等待管理员授权或优化完成超时");
            }
            let slice = remaining.min(Duration::from_millis(WAIT_SLICE_MS.into()));
            match receiver.recv_timeout(slice) {
                Ok(report) => return report,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // 子线程 panic / 提前退出：句柄已随线程结束释放，只需清理结果文件。
                    cancelled.store(true, Ordering::Release);
                    let _ = remove_result_file(&result_path);
                    return MemoryOptimizeReport::failed("内存优化线程异常退出");
                }
            }
        }
    }

    /// 子线程入口：请求 UAC、等待提权子进程并读取结果文件。
    ///
    /// 这里的 Win32 指针全部只在本线程内创建和使用；父线程唯一能影响它的是
    /// `cancelled` 标记。
    ///
    /// 回收语义：`ShellExecuteExW` 成功返回后，提权进程句柄只存在于本线程，因此
    /// 取消 / 超时 / 等待失败都必须在这里终止并回收子进程。`terminate_child` 返回
    /// false 时用明确的中文说明告知调用方「无法回收」，不能静默当成成功。
    fn run_elevated(
        executable: &Path,
        parameters: &str,
        result_path: &Path,
        cancelled: &AtomicBool,
        timeout: Duration,
    ) -> MemoryOptimizeReport {
        let verb = wide("runas");
        let file: Vec<u16> = executable
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let params = wide(parameters);
        let mut info = ShellExecuteInfoW {
            cb_size: std::mem::size_of::<ShellExecuteInfoW>() as u32,
            f_mask: SEE_MASK_NOCLOSEPROCESS,
            hwnd: std::ptr::null_mut(),
            lp_verb: verb.as_ptr(),
            lp_file: file.as_ptr(),
            lp_parameters: params.as_ptr(),
            lp_directory: std::ptr::null(),
            n_show: SW_HIDE,
            h_inst_app: std::ptr::null_mut(),
            lp_id_list: std::ptr::null_mut(),
            lp_class: std::ptr::null(),
            h_key_class: std::ptr::null_mut(),
            dw_hot_key: 0,
            h_icon_or_monitor: std::ptr::null_mut(),
            h_process: std::ptr::null_mut(),
        };

        // 父线程可能在派生前最后一刻取消：UAC 一旦弹出就无法由进程关闭，
        // 因此必须在真正调用 ShellExecuteExW 之前再确认一次取消标记。
        if cancelled_before_elevation(cancelled) {
            let _ = remove_result_file(result_path);
            return MemoryOptimizeReport::failed("启动已取消，内存优化未执行");
        }

        // SAFETY: 结构体字段全部初始化；仅在返回后使用 h_process。
        let launched = unsafe { ShellExecuteExW(&mut info) } != 0;

        if cancelled.load(Ordering::Acquire) {
            // UAC 阶段已被父线程判超时或取消：即使进程已拉起也要立刻回收，
            // 不能留下一个还在等管理员授权的孤儿进程。
            if !info.h_process.is_null() {
                let reclaimed = terminate_child(info.h_process);
                // SAFETY: 句柄由 ShellExecuteExW 返回，关闭后不再使用。
                unsafe { CloseHandle(info.h_process) };
                if !reclaimed {
                    let _ = remove_result_file(result_path);
                    return MemoryOptimizeReport::failed(
                        "等待管理员授权或优化完成超时，且无法回收提权子进程，请稍后重试",
                    );
                }
            }
            let _ = remove_result_file(result_path);
            return MemoryOptimizeReport::failed("等待管理员授权或优化完成超时");
        }

        if !launched {
            let code = std::io::Error::last_os_error();
            let _ = remove_result_file(result_path);
            return if code.raw_os_error() == Some(ERROR_CANCELLED) {
                MemoryOptimizeReport::permission_denied("已取消管理员授权，内存优化未执行")
            } else {
                MemoryOptimizeReport::failed(format!("无法请求管理员权限：{code}"))
            };
        }
        if info.h_process.is_null() {
            let _ = remove_result_file(result_path);
            return MemoryOptimizeReport::failed("提权子进程未启动");
        }

        match wait_child(info.h_process, timeout, cancelled) {
            ChildWait::Exited => {}
            ChildWait::Timeout | ChildWait::Cancelled => {
                // 子进程仍可能卡在 UAC / 优化中：必须终止并回收，不能留下孤儿进程。
                let reclaimed = terminate_child(info.h_process);
                // SAFETY: 终止尝试后句柄仍然有效，此处关闭一次。
                unsafe { CloseHandle(info.h_process) };
                let _ = remove_result_file(result_path);
                return MemoryOptimizeReport::failed(if reclaimed {
                    "等待管理员授权或优化完成超时".to_string()
                } else {
                    "等待管理员授权或优化完成超时，且无法回收提权子进程，请稍后重试".to_string()
                });
            }
            ChildWait::Failed(code) => {
                // 等待本身失败时同样要尝试回收，不能只关句柄让进程继续运行。
                let reclaimed = terminate_child(info.h_process);
                // SAFETY: 同上，等待失败后也必须回收句柄。
                unsafe { CloseHandle(info.h_process) };
                let _ = remove_result_file(result_path);
                return MemoryOptimizeReport::failed(if reclaimed {
                    format!("等待提权子进程失败（代码 {code}）")
                } else {
                    format!(
                        "等待提权子进程失败（代码 {code}），且无法回收提权子进程，请稍后重试"
                    )
                });
            }
        }

        let mut exit_code = 0u32;
        // SAFETY: 同一有效句柄；进程已退出，查询退出码后立即关闭。
        // `GetExitCodeProcess` 失败时不能把 0 当成成功退出码，否则会把「拿不到状态」
        // 误报成「优化完成」。
        let exit_code_known = unsafe { GetExitCodeProcess(info.h_process, &mut exit_code) } != 0;
        // SAFETY: 查询结束后句柄不再使用，关闭一次。
        unsafe { CloseHandle(info.h_process) };
        let report = read_result_file(result_path);
        let _ = remove_result_file(result_path);
        match report {
            Some(report) => report,
            None if !exit_code_known => {
                MemoryOptimizeReport::failed("无法读取内存优化子进程的退出码，请稍后重试")
            }
            None if exit_code == 0 => MemoryOptimizeReport {
                elevation: "uac".into(),
                ..MemoryOptimizeReport::blank(MemoryOptimizeStatus::Success, "uac")
            },
            None => MemoryOptimizeReport::failed(format!(
                "内存优化子进程异常退出（代码 {exit_code}）"
            )),
        }
    }

    /// 计算下一次分片等待的毫秒数：至少 1ms（避免 0ms 忙等），最多 100ms（保证取消响应）。
    fn wait_slice_ms(remaining: Duration) -> u32 {
        remaining
            .min(Duration::from_millis(WAIT_SLICE_MS.into()))
            .as_millis()
            .max(1) as u32
    }

    /// 分片等待提权子进程：每 100ms 检查一次取消标记，累计到 `timeout` 判超时。
    ///
    /// 不能把整个 timeout 交给一次 `WaitForSingleObject`：那样父线程取消后
    /// 子线程还要把剩余时间等完才会回收句柄。
    fn wait_child(handle: *mut c_void, timeout: Duration, cancelled: &AtomicBool) -> ChildWait {
        let Some(deadline) = deadline_after(timeout) else {
            // 换算溢出时按超时处理：调用方会终止并回收子进程，语义不变。
            return ChildWait::Timeout;
        };
        loop {
            if cancelled.load(Ordering::Acquire) {
                return ChildWait::Cancelled;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return ChildWait::Timeout;
            }
            let slice_ms = wait_slice_ms(remaining);
            // SAFETY: handle 来自 ShellExecuteExW，调用期间保持有效。
            match unsafe { WaitForSingleObject(handle, slice_ms) } {
                // WAIT_OBJECT_0 = 已退出；WAIT_TIMEOUT = 当前片段到期，继续累计。
                WAIT_OBJECT_0 => return ChildWait::Exited,
                WAIT_TIMEOUT => {}
                other => return ChildWait::Failed(other),
            }
        }
    }

    /// 分片等待结果。用枚举而不是裸 `u32`，避免把 WAIT_TIMEOUT 与失败码混在一起比较。
    enum ChildWait {
        Exited,
        Timeout,
        Cancelled,
        Failed(u32),
    }

    pub(super) fn capability() -> MemoryOptimizeCapability {
        let admin = elevated();
        MemoryOptimizeCapability {
            platform: "windows".into(),
            elevated: admin,
            needs_password: false,
            has_sudo: false,
            supports_standby_purge: true,
            available: true,
            message: if admin {
                None
            } else {
                Some("执行完整内存优化时 Windows 会弹出管理员授权（UAC）。".into())
            },
        }
    }

    pub(super) fn run(
        purge_standby: bool,
        _password: Option<&str>,
        allow_prompt: bool,
        timeout: Duration,
        cancel: CancelCheck<'_>,
    ) -> MemoryOptimizeReport {
        // 已提权时同步执行；把总超时换算成绝对截止时间传入优化循环，
        // 否则这里会无视 timeout 一直跑到全部进程处理完。
        let deadline = deadline_after(timeout);
        if elevated() {
            return optimize_all(purge_standby, "normal", deadline, cancel);
        }
        if !allow_prompt {
            return MemoryOptimizeReport::permission_denied("完整内存优化需要管理员权限");
        }
        elevate_and_run(purge_standby, timeout, cancel)
    }

    /// 保留给单元测试：Windows 上判断一个「子进程结果 JSON」是否可解析。
    #[cfg(test)]
    pub(super) fn parse_report(text: &str) -> Option<MemoryOptimizeReport> {
        serde_json::from_str(text).ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 锁定 Win32 ABI：`cbSize` 必须等于结构体大小，`fMask` 必须是 32 位。
        /// 曾因把 `f_mask` 写成 `usize` 导致 x64 上字段整体错位，`runas` 退化为默认动词，
        /// 点击「内存优化」完全不弹 UAC。
        #[test]
        fn shell_execute_info_matches_win32_layout() {
            let size = std::mem::size_of::<ShellExecuteInfoW>();
            let pointer = std::mem::size_of::<*mut c_void>();
            assert_eq!(std::mem::offset_of!(ShellExecuteInfoW, cb_size), 0);
            assert_eq!(std::mem::offset_of!(ShellExecuteInfoW, f_mask), 4);
            assert_eq!(std::mem::offset_of!(ShellExecuteInfoW, hwnd), 8);
            if pointer == 8 {
                assert_eq!(size, 112, "x64 SHELLEXECUTEINFOW 必须是 112 字节");
                assert_eq!(std::mem::offset_of!(ShellExecuteInfoW, lp_verb), 16);
                assert_eq!(std::mem::offset_of!(ShellExecuteInfoW, n_show), 48);
                assert_eq!(std::mem::offset_of!(ShellExecuteInfoW, h_process), 104);
            } else {
                assert_eq!(size, 60, "x86 SHELLEXECUTEINFOW 必须是 60 字节");
            }
        }

        #[test]
        fn report_json_roundtrip_uses_camel_case() {
            let mut report =
                MemoryOptimizeReport::blank(MemoryOptimizeStatus::Success, "uac");
            report.total_processes = 120;
            report.available_delta_bytes = 32 * 1024 * 1024;
            let text = serde_json::to_string(&report).unwrap();
            assert!(text.contains("\"availableDeltaBytes\":33554432"));
            assert!(text.contains("\"standbyPurged\":false"));
            assert!(text.contains("\"totalProcesses\":120"));
            let parsed = parse_report(&text).unwrap();
            assert_eq!(parsed.status, MemoryOptimizeStatus::Success);
            assert_eq!(parsed.elevation, "uac");
        }

        #[test]
        fn result_file_cleanup_handles_missing_and_existing_files() {
            let path = std::env::temp_dir().join(format!(
                "nowhere-memory-optimize-test-{}.json",
                crate::store::random_token()
            ));
            std::fs::write(&path, b"{}").unwrap();
            assert!(remove_result_file(&path));
            assert!(!path.exists());
            // 已不存在时仍视为清理成功，确保所有提前返回路径可以无条件调用。
            assert!(remove_result_file(&path));
        }

        #[test]
        fn permission_denied_status_is_camel_case() {
            let report = MemoryOptimizeReport::permission_denied("已取消管理员授权");
            let value = serde_json::to_value(&report).unwrap();
            assert_eq!(value["status"], "permissionDenied");
            assert_eq!(value["message"], "已取消管理员授权");
        }

        #[test]
        fn wait_slice_is_bounded_and_never_zero() {
            // 分片必须带上限：否则取消 / 超时要等到一个长等待结束才生效。
            assert_eq!(wait_slice_ms(Duration::from_secs(10)), WAIT_SLICE_MS);
            assert_eq!(wait_slice_ms(Duration::from_millis(5)), 5);
            // 剩余不足 1ms 时也传 1ms，避免 0ms 导致忙等。
            assert_eq!(wait_slice_ms(Duration::from_micros(200)), 1);
        }

        #[test]
        fn stopped_report_keeps_partial_counts() {
            let mut report = MemoryOptimizeReport::blank(MemoryOptimizeStatus::Success, "normal");
            report.total_processes = 6;
            report.succeeded = 3;
            report.access_denied = 2;
            report.skipped = 1;
            mark_stopped(&mut report, StopReason::Deadline);
            assert_eq!(report.status, MemoryOptimizeStatus::Failed);
            // 超时不能把已完成的统计清零，前端要能展示「做到了哪一步」。
            assert_eq!(report.total_processes, 6);
            assert_eq!(report.succeeded, 3);
            assert_eq!(report.access_denied, 2);
            assert_eq!(report.skipped, 1);
            assert!(report
                .message
                .as_deref()
                .is_some_and(|message| message.contains("超时")));
        }

    }
}

/* ============================================================================
   macOS / Linux 实现
   ========================================================================== */

#[cfg(unix)]
mod imp {
    use super::*;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::OnceLock;
    use std::time::Instant;

    /// 一次提权尝试的超时；避免 sudo / su 卡在密码提示上永久阻塞。
    const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);

    fn cached_uid() -> Option<u32> {
        static UID: OnceLock<Option<u32>> = OnceLock::new();
        *UID.get_or_init(|| {
            let output = Command::new("id").arg("-u").output().ok()?;
            String::from_utf8_lossy(&output.stdout).trim().parse().ok()
        })
    }

    fn is_root() -> bool {
        cached_uid() == Some(0)
    }

    /// 可执行文件判定：存在、是普通文件、且带执行位。
    ///
    /// 只看 `is_file()` 会把没有执行权限的同名文件当成 sudo，点击后必然失败。
    fn is_executable(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    fn resolve_program(name: &str, absolute_candidates: &[&str]) -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = absolute_candidates
            .iter()
            .map(|path| PathBuf::from(*path))
            .collect();
        if let Some(paths) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&paths).map(|dir| dir.join(name)));
        }
        candidates.into_iter().find(|path| is_executable(path))
    }

    /// 解析 sudo：先查常见绝对路径，再查 PATH。
    ///
    /// 图形会话（Finder / .desktop）拿到的 PATH 往往不含 sbin，只扫 PATH 会把
    /// 明明装了 sudo 的系统误判成「没有 sudo」，前端于是连密码框都不弹。
    fn sudo_program() -> Option<PathBuf> {
        resolve_program(
            "sudo",
            &["/usr/bin/sudo", "/bin/sudo", "/usr/local/bin/sudo", "/opt/homebrew/bin/sudo"],
        )
    }

    /// 解析 su（Linux 常见于 /bin，macOS 在 /usr/bin）。
    fn su_program() -> Option<PathBuf> {
        resolve_program("su", &["/bin/su", "/usr/bin/su", "/sbin/su", "/usr/sbin/su"])
    }

    fn has_sudo() -> bool {
        sudo_program().is_some()
    }

    /// macOS 的 purge 通常位于 /usr/sbin；没有时退回 PATH 查找。
    ///
    /// 与 sudo / su 一样要求真实执行位：GUI 会话的 PATH 往往不含 sbin。
    #[cfg(target_os = "macos")]
    fn purge_program() -> Option<PathBuf> {
        resolve_program(
            "purge",
            &[
                "/usr/sbin/purge",
                "/usr/bin/purge",
                "/bin/purge",
                "/usr/local/bin/purge",
            ],
        )
    }

    /// 平台对应的「回收系统内存」shell 命令。
    fn shell_command() -> String {
        #[cfg(target_os = "linux")]
        {
            // drop_caches 的 3 同时回收页缓存、dentry 与 inode，执行前先 sync。
            "sync; echo 3 > /proc/sys/vm/drop_caches".into()
        }
        #[cfg(target_os = "macos")]
        {
            // shell_command 只返回 shell 脚本字符串；macOS 用解析后的绝对路径，
            // 避免 GUI 会话 PATH 不含 /usr/sbin 时找不到 purge。
            purge_program()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            String::new()
        }
    }

    /// 解析 /proc/meminfo 的 MemAvailable（kB → 字节）。
    fn parse_mem_available(text: &str) -> Option<u64> {
        text.lines().find_map(|line| {
            let rest = line.strip_prefix("MemAvailable:")?;
            let kb = rest.split_whitespace().next()?.parse::<u64>().ok()?;
            Some(kb * 1024)
        })
    }

    /// 解析 vm_stat：free + inactive + speculative 视为可用内存。
    fn parse_vm_stat(text: &str) -> Option<u64> {
        let page_size = text
            .lines()
            .next()
            .and_then(|line| line.split("page size of ").nth(1))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(4096);
        let mut pages = 0u64;
        let mut found = false;
        for line in text.lines() {
            let Some((name, rest)) = line.split_once(':') else {
                continue;
            };
            if matches!(
                name.trim(),
                "Pages free" | "Pages inactive" | "Pages speculative"
            ) {
                let value = rest.trim().trim_end_matches('.').parse::<u64>().unwrap_or(0);
                pages = pages.saturating_add(value);
                found = true;
            }
        }
        found.then_some(pages.saturating_mul(page_size))
    }

    fn available_memory_bytes() -> Option<u64> {
        #[cfg(target_os = "linux")]
        {
            std::fs::read_to_string("/proc/meminfo")
                .ok()
                .and_then(|text| parse_mem_available(&text))
        }
        #[cfg(target_os = "macos")]
        {
            let output = Command::new("vm_stat").output().ok()?;
            parse_vm_stat(&String::from_utf8_lossy(&output.stdout))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            None
        }
    }

    /// 单次提权尝试可用的截止时间：`min(剩余总预算, ATTEMPT_TIMEOUT)`。
    ///
    /// `None` 表示总预算已经耗尽（或时间换算溢出），本次尝试应直接放弃。
    fn attempt_deadline(deadline: Option<Instant>) -> Option<Instant> {
        let budget = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return None;
                }
                remaining.min(ATTEMPT_TIMEOUT)
            }
            None => ATTEMPT_TIMEOUT,
        };
        Instant::now().checked_add(budget)
    }

    /// 运行一个可能要求密码的子进程；密码只写入 stdin，从不进入日志。
    ///
    /// 超时是 best-effort：总预算与单次 30 秒上限取较小值，轮询循环每 50ms 检查一次
    /// 截止时间与取消标记；杀进程之后的系统调用无法被中断，但不会无限等待。
    fn run_command(
        program: &Path,
        args: &[String],
        password: Option<&str>,
        deadline: Option<Instant>,
        cancel: CancelCheck<'_>,
    ) -> bool {
        // 预算已耗尽时不要再派生新进程。
        let Some(attempt_ends_at) = attempt_deadline(deadline) else {
            return false;
        };
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let Ok(mut child) = command.spawn() else {
            return false;
        };
        if let Some(password) = password {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(password.as_bytes());
                let _ = stdin.write_all(b"\n");
                // stdin 在此释放，向 sudo / su 发出 EOF。
            }
        } else {
            drop(child.stdin.take());
        }
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) => {}
                Err(_) => return false,
            }
            // 取消或超时都要立刻回收子进程，避免 sudo 仍在密码提示上占用时间。
            if cancel.is_some_and(|check| check()) || Instant::now() >= attempt_ends_at {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// 直接执行（normal），无提权。
    fn try_normal(deadline: Option<Instant>, cancel: CancelCheck<'_>) -> bool {
        let script = shell_command();
        if script.is_empty() {
            return false;
        }
        run_command(Path::new("sh"), &["-c".into(), script], None, deadline, cancel)
    }

    /// sudo：`None` 表示只尝试免密路径。
    fn try_sudo(
        password: Option<&str>,
        deadline: Option<Instant>,
        cancel: CancelCheck<'_>,
    ) -> bool {
        let script = shell_command();
        if script.is_empty() {
            return false;
        }
        let Some(program) = sudo_program() else {
            return false;
        };
        let mut args = if password.is_some() {
            vec![
                "-S".to_string(),
                "-k".to_string(),
                "-p".to_string(),
                String::new(),
            ]
        } else {
            vec!["-n".to_string()]
        };
        args.push("sh".into());
        args.push("-c".into());
        args.push(script);
        run_command(&program, &args, password, deadline, cancel)
    }

    /// root：su 的密码校验通常要求 TTY，部分系统会直接失败——按「跳过并继续」处理。
    fn try_root(password: &str, deadline: Option<Instant>, cancel: CancelCheck<'_>) -> bool {
        let script = shell_command();
        if script.is_empty() {
            return false;
        }
        let Some(program) = su_program() else {
            return false;
        };
        #[cfg(target_os = "linux")]
        let args = vec!["-c".to_string(), script, "root".to_string()];
        #[cfg(target_os = "macos")]
        let args = vec!["root".to_string(), "-c".to_string(), script];
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let args = Vec::new();
        run_command(&program, &args, Some(password), deadline, cancel)
    }

    /// 依次尝试的提权方式，用枚举而不是嵌套 if，便于在每次尝试前统一检查截止时间。
    #[derive(Clone, Copy)]
    enum Attempt {
        Normal,
        SudoPasswordless,
        SudoPassword,
        Root,
    }

    /// 纯逻辑构造能力信息，便于覆盖 root / sudo / su 各分支。
    fn capability_for(
        platform: &str,
        root: bool,
        sudo: bool,
        su: bool,
    ) -> MemoryOptimizeCapability {
        MemoryOptimizeCapability {
            platform: platform.into(),
            elevated: root,
            needs_password: !root && (sudo || su),
            has_sudo: sudo,
            supports_standby_purge: false,
            available: root || sudo || su,
            message: if root || sudo {
                None
            } else if su {
                // 仅剩 su 时前端必须弹密码框，try_root 才可达。
                Some("当前系统没有可用的 sudo，将尝试 su/root 密码。".into())
            } else {
                Some("当前系统没有可用的 sudo 或 su，无法运行完整内存优化。".into())
            },
        }
    }

    pub(super) fn capability() -> MemoryOptimizeCapability {
        let platform = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            "other"
        };
        capability_for(
            platform,
            is_root(),
            has_sudo(),
            su_program().is_some(),
        )
    }

    pub(super) fn run(
        _purge_standby: bool,
        password: Option<&str>,
        _allow_prompt: bool,
        timeout: Duration,
        cancel: CancelCheck<'_>,
    ) -> MemoryOptimizeReport {
        let started = Instant::now();
        let before = available_memory_bytes();
        // 超时是 best-effort：只在两次尝试之间与子进程轮询点检查截止时间，
        // 单次尝试最多 30 秒（见 attempt_deadline），运行中的系统调用无法被强制中断。
        let deadline = deadline_after(timeout);
        let attempts: &[Attempt] = if password.is_some() {
            &[
                Attempt::Normal,
                Attempt::SudoPasswordless,
                Attempt::SudoPassword,
                Attempt::Root,
            ]
        } else {
            &[Attempt::Normal, Attempt::SudoPasswordless]
        };

        let mut attempted = 0usize;
        let mut elevation = None;
        let mut stopped = None;
        for attempt in attempts {
            if cancel.is_some_and(|check| check()) {
                stopped = Some("启动已取消，内存优化中止");
                break;
            }
            if deadline_exceeded(deadline) {
                stopped = Some("内存优化超时");
                break;
            }
            attempted += 1;
            let succeeded = match *attempt {
                Attempt::Normal => try_normal(deadline, cancel),
                Attempt::SudoPasswordless => try_sudo(None, deadline, cancel),
                Attempt::SudoPassword => try_sudo(password, deadline, cancel),
                Attempt::Root => try_root(password.unwrap_or_default(), deadline, cancel),
            };
            if succeeded {
                elevation = Some(match *attempt {
                    Attempt::Normal => "normal",
                    Attempt::SudoPasswordless | Attempt::SudoPassword => "sudo",
                    Attempt::Root => "root",
                });
                break;
            }
        }
        // 最后一次尝试可能刚好耗尽预算或被取消，退出循环后再确认一次。
        if stopped.is_none() && elevation.is_none() {
            if cancel.is_some_and(|check| check()) {
                stopped = Some("启动已取消，内存优化中止");
            } else if deadline_exceeded(deadline) {
                stopped = Some("内存优化超时");
            }
        }

        let mut report = if let Some(reason) = stopped {
            let mut report = MemoryOptimizeReport::blank(MemoryOptimizeStatus::Failed, "skipped");
            report.message = Some(format!(
                "{reason}：已尝试 {attempted} 种提权方式，剩余步骤已停止"
            ));
            report
        } else if let Some(elevation) = elevation {
            MemoryOptimizeReport::blank(MemoryOptimizeStatus::Success, elevation)
        } else {
            return MemoryOptimizeReport::skipped(
                "无法运行完整内存优化：未获得 sudo / root 权限",
            );
        };
        if let (Some(before), Some(after)) = (before, available_memory_bytes()) {
            report.available_delta_bytes = after as i64 - before as i64;
        }
        report.elapsed_ms = started.elapsed().as_millis() as u64;
        report
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// sudo 探测必须看执行位：图形会话的 PATH 里可能有同名但不可执行的文件，
        /// 只按 is_file() 判断会让前端误以为能提权。
        #[test]
        fn executable_detection_requires_exec_bit() {
            let root = std::env::temp_dir().join(format!(
                "nowhere-sudo-probe-{}",
                crate::store::random_token()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let plain = root.join("sudo");
            std::fs::write(&plain, b"#!/bin/sh\n").unwrap();
            assert!(!is_executable(&plain), "没有执行位不能算可执行");

            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&plain).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&plain, permissions).unwrap();
            assert!(is_executable(&plain));

            // 目录不算可执行文件，即使带执行位。
            assert!(!is_executable(&root));

            // 绝对路径候选优先，且只接受真实可执行的文件；
            // 不存在的程序名不会凭空「解析成功」。
            assert!(resolve_program("sudo", &[plain.to_str().unwrap()]).is_some());
            assert!(resolve_program("nowhere-missing-program", &[]).is_none());

            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn parses_mem_available_from_proc_meminfo() {
            let text = "MemTotal:       16384000 kB\nMemFree:         123456 kB\n\
                        MemAvailable:    8123456 kB\nBuffers:            1024 kB\n";
            assert_eq!(parse_mem_available(text), Some(8_123_456 * 1024));
            assert_eq!(parse_mem_available("MemTotal: 1 kB\n"), None);
        }

        #[test]
        fn parses_free_inactive_and_speculative_pages() {
            let text = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
                        Pages free:                                1000.\n\
                        Pages active:                              2000.\n\
                        Pages inactive:                            3000.\n\
                        Pages speculative:                          500.\n\
                        Pages wired down:                           800.\n";
            assert_eq!(parse_vm_stat(text), Some(4500 * 16384));
            assert_eq!(parse_vm_stat("nothing here"), None);
        }

        #[test]
        fn capability_counts_su_as_available_password_path() {
            let root = capability_for("linux", true, false, false);
            assert!(root.available && !root.needs_password && !root.has_sudo);

            let sudo = capability_for("linux", false, true, false);
            assert!(sudo.available && sudo.needs_password && sudo.has_sudo);

            // 仅 su：前端必须收到「需要密码 + 可用」，否则 try_root 永远不可达。
            let su_only = capability_for("linux", false, false, true);
            assert!(su_only.available, "仅 su 也必须视为可用");
            assert!(su_only.needs_password);
            assert!(!su_only.has_sudo, "has_sudo 只能表示 sudo");
            assert!(su_only
                .message
                .as_deref()
                .is_some_and(|text| text.contains("su")));

            let none = capability_for("linux", false, false, false);
            assert!(!none.available && !none.needs_password && !none.has_sudo);
            assert!(none
                .message
                .as_deref()
                .is_some_and(|text| text.contains("sudo 或 su")));
        }

        #[test]
        fn skipped_report_carries_reason() {
            let report = MemoryOptimizeReport::skipped("无法运行完整内存优化：未获得 sudo 权限");
            assert_eq!(report.status, MemoryOptimizeStatus::Skipped);
            assert_eq!(report.elevation, "skipped");
            assert!(report.message.unwrap().contains("sudo"));
        }
    }
}

/* ============================================================================
   其它平台（移动端等）：没有可用的提权路径
   ========================================================================== */

#[cfg(not(any(windows, unix)))]
mod imp {
    use super::*;

    pub(super) fn capability() -> MemoryOptimizeCapability {
        MemoryOptimizeCapability {
            platform: "other".into(),
            elevated: false,
            needs_password: false,
            has_sudo: false,
            supports_standby_purge: false,
            available: false,
            message: Some("当前平台不支持完整内存优化。".into()),
        }
    }

    pub(super) fn run(
        _purge_standby: bool,
        _password: Option<&str>,
        _allow_prompt: bool,
        _timeout: Duration,
        _cancel: CancelCheck<'_>,
    ) -> MemoryOptimizeReport {
        MemoryOptimizeReport::skipped("当前平台不支持完整内存优化")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// `acquire` 直接读写进程级静态量：相关测试必须串行，并在结束时恢复初始状态，
    /// 否则并行执行的其他测试会看到脏标记。
    static ACQUIRE_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 测试期间持有串行锁，Drop 时把两个静态标记恢复到初始值（断言失败也不会污染后续测试）。
    struct AcquireTestGuard(Option<MutexGuard<'static, ()>>);

    impl AcquireTestGuard {
        fn new() -> Self {
            let lock = ACQUIRE_TEST_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            RUNNING.store(false, Ordering::Release);
            ELEVATION_PENDING.store(false, Ordering::Release);
            Self(Some(lock))
        }
    }

    impl Drop for AcquireTestGuard {
        fn drop(&mut self) {
            RUNNING.store(false, Ordering::Release);
            ELEVATION_PENDING.store(false, Ordering::Release);
            self.0.take();
        }
    }

    #[test]
    fn busy_report_has_no_message_and_skipped_elevation() {
        let report = MemoryOptimizeReport::busy();
        assert_eq!(report.status, MemoryOptimizeStatus::Busy);
        assert!(report.message.is_none());
        assert_eq!(report.elevation, "skipped");
    }

    #[test]
    fn failed_report_keeps_reason_and_status() {
        let report = MemoryOptimizeReport::failed("无法请求管理员权限");
        assert_eq!(report.status, MemoryOptimizeStatus::Failed);
        assert_eq!(report.message.as_deref(), Some("无法请求管理员权限"));
    }

    #[test]
    fn capability_is_serializable_for_frontend() {
        let capability = capability();
        let value = serde_json::to_value(&capability).unwrap();
        for key in [
            "platform",
            "elevated",
            "needsPassword",
            "hasSudo",
            "supportsStandbyPurge",
            "available",
        ] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn acquire_is_busy_while_another_run_is_active() {
        let _state = AcquireTestGuard::new();
        let first = acquire().expect("首次请求应成功");
        assert!(acquire().is_none(), "已有优化在执行时必须返回 busy");
        drop(first);
        assert!(!RUNNING.load(Ordering::Acquire));
    }

    #[test]
    fn acquire_is_busy_while_elevation_thread_is_pending() {
        let _state = AcquireTestGuard::new();
        ELEVATION_PENDING.store(true, Ordering::Release);
        assert!(acquire().is_none(), "提权线程仍在等 UAC 时必须返回 busy");
        // 关键回归点：失败路径必须回滚 RUNNING，否则之后的请求会被永久拒绝。
        assert!(!RUNNING.load(Ordering::Acquire));
        ELEVATION_PENDING.store(false, Ordering::Release);
        drop(acquire().expect("UAC 结束后应重新可用"));
        assert!(!RUNNING.load(Ordering::Acquire));
    }

    /// 超时是 best-effort：这里只锁定截止时间换算与比较的基本行为。
    #[cfg(any(windows, unix))]
    #[test]
    fn deadline_helpers_do_not_report_premature_timeout() {
        assert!(!deadline_exceeded(None));
        let deadline = deadline_after(Duration::from_secs(60)).expect("正常超时不应溢出");
        assert!(!deadline_exceeded(Some(deadline)));
        assert!(deadline_exceeded(Some(Instant::now())));
    }

    /// 极端超时值不能让 `Instant + Duration` panic；换算出 None 时按不设截止处理。
    #[cfg(any(windows, unix))]
    #[test]
    fn deadline_after_rejects_extreme_duration_without_panicking() {
        assert!(deadline_after(Duration::MAX).is_none());
    }

    /// 结果文件名必须每次不同，且不会凭空命中已有文件（防临时目录残留误读）。
    #[test]
    fn result_file_path_uses_random_suffix() {
        let first = result_file_path();
        let second = result_file_path();
        assert_ne!(first, second);
        let name = first
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        assert!(name.starts_with(RESULT_FILE_PREFIX), "{name}");
        assert!(name.ends_with(".json"), "{name}");
        assert_eq!(
            name.len(),
            RESULT_FILE_PREFIX.len() + RESULT_FILE_RANDOM_BYTES * 2 + ".json".len()
        );
        assert!(!first.exists());
    }

    /// 结果文件必须独占创建：路径已存在（硬链接 / 普通文件）时写入失败，
    /// 且原目标内容不能被覆盖。
    #[test]
    fn write_result_exclusive_refuses_existing_path() {
        let dir = std::env::temp_dir().join(format!(
            "nowhere-memory-result-{}",
            crate::store::random_token()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let victim = dir.join("victim.json");
        let target = dir.join("result.json");
        std::fs::write(&victim, b"original").unwrap();
        // 硬链接让 target 与 victim 指向同一 inode：create_new 必须失败。
        std::fs::hard_link(&victim, &target).unwrap();

        assert!(write_result_exclusive(&target, "overwritten").is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"original");

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Unix：预先放置指向 victim 的符号链接，写入必须失败且不跟随链接。
    #[cfg(unix)]
    #[test]
    fn write_result_exclusive_does_not_follow_symlink() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join(format!(
            "nowhere-memory-result-link-{}",
            crate::store::random_token()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let victim = dir.join("victim.json");
        let target = dir.join("result.json");
        std::fs::write(&victim, b"original").unwrap();
        symlink(&victim, &target).unwrap();

        assert!(write_result_exclusive(&target, "overwritten").is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"original");
        // 链接本身仍然存在，说明失败发生在 open 阶段而不是误删链接。
        assert!(
            std::fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 父进程读取前必须拒绝被替换成符号链接的结果文件。
    #[cfg(unix)]
    #[test]
    fn read_result_file_refuses_symlink() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join(format!(
            "nowhere-memory-read-link-{}",
            crate::store::random_token()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let victim = dir.join("victim.json");
        let target = dir.join("result.json");
        std::fs::write(
            &victim,
            br#"{"status":"success","totalProcesses":0,"succeeded":0,"accessDenied":0,"skipped":0,"availableDeltaBytes":0,"standbyPurged":false,"elapsedMs":0,"elevation":"uac","message":null}"#,
        )
        .unwrap();
        symlink(&victim, &target).unwrap();

        assert!(read_result_file(&target).is_none());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancellation_is_visible_before_elevation() {
        let flag = AtomicBool::new(false);
        assert!(!cancelled_before_elevation(&flag));
        flag.store(true, Ordering::Release);
        assert!(cancelled_before_elevation(&flag));
    }
}
