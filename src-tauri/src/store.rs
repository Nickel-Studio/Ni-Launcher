//! 账号 / 游戏设置的本地持久化（写入 `app.config_dir()`）。
//!
//! 文件布局：
//! - `accounts.json`        —— 账号数组（**含**敏感字段，仅后端落盘，绝不回传前端）
//! - `active_account.json`  —— 当前激活账号 id
//! - `game.json`            —— 游戏 / 版本设置（纯 JSON 文本透传）
//! - `instances.json`       —— 实例（版本根目录）注册表（纯 JSON 文本透传）
//! - `skin-oauth-clients.json` —— 各站点/实例的加密 OAuth 应用配置（仅后端落盘，不回传前端）
//!
//! 本模块为纯逻辑（不涉及网络），便于单元测试；文件损坏时安全回退为默认值，
//! 不 panic。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use minecraft_java_rs_core::utils::auth::offline_uuid;

/// 默认的 LittleSkin 皮肤站地址（站点根，不含 Yggdrasil 路径）。
pub const DEFAULT_SERVER: &str = "https://littleskin.cn";

/// 用户输入了非空、但无法解析为合法站点地址时的提示。
const INVALID_SERVER_MESSAGE: &str =
    "皮肤站地址无效，请填写完整地址，例如 https://littleskin.cn";

/// 已落盘账号保存的地址非法时的提示：引导用户重新登录，而不是静默改投默认站点。
const INVALID_STORED_SERVER_MESSAGE: &str =
    "账号保存的皮肤站地址无效，请在账号设置中重新登录并填写正确地址";

/// Blessing Skin / LittleSkin 把 Yggdrasil 挂在站点根的该路径下。
/// 皮肤接口走站点根（`/api/skin`），认证接口必须带此前缀，
/// 否则 `POST https://littleskin.cn/authserver/authenticate` 只会得到 404。
///
/// 已知的 API 根后缀只有 [`YGGDRASIL_SUFFIX`] 与 `/yggdrasil`；其它路径一律按站点根处理。
pub const YGGDRASIL_SUFFIX: &str = "/api/yggdrasil";

/// 兼容的 Yggdrasil API 根后缀（长后缀在前，避免 `/api/yggdrasil` 只剥掉 `/yggdrasil`）。
const YGGDRASIL_SUFFIXES: [&str; 2] = [YGGDRASIL_SUFFIX, "/yggdrasil"];

/// 返回给前端的账号（**不含**任何敏感字段）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    #[serde(rename = "type")]
    pub account_type: String,
    pub name: String,
    pub uuid: String,
    pub email: Option<String>,
    pub server: Option<String>,
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub auth_method: Option<String>,
}

/// 本机加密后的敏感字符串。
///
/// 序列化时保持透明（JSON 里就是密文字符串），`Debug` 固定输出占位符，
/// 避免账号结构被无意 `{:?}` 到日志时泄露密文或明文。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SealedSecret(String);

impl SealedSecret {
    pub fn new(ciphertext: impl Into<String>) -> Self {
        Self(ciphertext.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SealedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SealedSecret(<redacted>)")
    }
}

/// 持久化的账号（**含**敏感字段，绝不直接返回前端）。
///
/// 不派生 `Debug`：手写实现会把令牌 / 密码密文统一替换为 `<redacted>`，
/// 避免账号结构被无意 `{:?}` 到日志时泄露敏感字段。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAccount {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default = "default_type")]
    pub account_type: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,

    // ── 敏感字段：仅落盘 ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// 皮肤站密码（[`crate::vault`] 加密后的密文）。
    ///
    /// 仅用于「切换角色」时静默重新认证：Yggdrasil Connect 绑定过角色的令牌
    /// 无法枚举 / 改绑，服务端要求重新 `authenticate`。明文不落盘、不回传前端。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<SealedSecret>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<SealedSecret>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_api: Option<String>,
}

impl std::fmt::Debug for StoredAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let secret = |value: &Option<String>| value.as_ref().map(|_| "<redacted>");
        f.debug_struct("StoredAccount")
            .field("id", &self.id)
            .field("type", &self.account_type)
            .field("name", &self.name)
            .field("uuid", &self.uuid)
            .field("email", &self.email)
            .field("server", &self.server)
            .field("avatar_url", &self.avatar_url)
            .field("access_token", &secret(&self.access_token))
            .field("client_token", &secret(&self.client_token))
            .field("refresh_token", &secret(&self.refresh_token))
            .field("expires_at", &self.expires_at)
            .field(
                "password",
                &self.password.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl Default for StoredAccount {
    fn default() -> Self {
        Self {
            id: String::new(),
            account_type: default_type(),
            name: String::new(),
            uuid: String::new(),
            email: None,
            server: None,
            avatar_url: None,
            access_token: None,
            client_token: None,
            refresh_token: None,
            expires_at: None,
            password: None,
            oauth: None,
            auth_api: None,
        }
    }
}

fn default_type() -> String {
    "offline".to_string()
}

impl StoredAccount {
    /// 投影为前端可安全接收的账号（剥离敏感字段）。
    pub fn to_public(&self) -> Account {
        Account {
            id: self.id.clone(),
            account_type: self.account_type.clone(),
            name: self.name.clone(),
            uuid: self.uuid.clone(),
            email: self.email.clone(),
            server: self.server.clone(),
            avatar_url: self.avatar_url.clone(),
            auth_method: (self.account_type == "littleskin")
                .then(|| if self.oauth.is_some() { "oauth2" } else { "classic" }.to_string()),
        }
    }

    /// 规范化：补全 id / type / uuid，保证落盘数据自洽。
    pub fn normalize(mut self) -> Self {
        if self.id.trim().is_empty() {
            self.id = new_id();
        }
        if self.account_type.trim().is_empty() {
            self.account_type = default_type();
        }
        if self.uuid.trim().is_empty() && !self.name.trim().is_empty() {
            self.uuid = offline_uuid(self.name.trim());
        }
        self
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 进程内文件锁：串行化「读-改-写」，避免并发命令互相覆盖。
// ─────────────────────────────────────────────────────────────────────────────

static FILE_LOCK: Mutex<()> = Mutex::new(());

pub fn lock() -> MutexGuard<'static, ()> {
    FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ─────────────────────────────────────────────────────────────────────────────
// 路径
// ─────────────────────────────────────────────────────────────────────────────

fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    crate::app_paths::config_dir(app)
}

pub fn accounts_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir(app)?.join("accounts.json"))
}

pub fn active_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir(app)?.join("active_account.json"))
}

pub fn game_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir(app)?.join("game.json"))
}

pub fn instances_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir(app)?.join("instances.json"))
}

// ─────────────────────────────────────────────────────────────────────────────
// 账号读写
// ─────────────────────────────────────────────────────────────────────────────

/// 解析账号数组文本；空 / 损坏时安全回退为空列表（不 panic）。
pub fn parse_accounts(text: &str) -> Vec<StoredAccount> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    serde_json::from_str::<Vec<StoredAccount>>(text).unwrap_or_default()
}

pub fn load_accounts(app: &AppHandle) -> Vec<StoredAccount> {
    let path = match accounts_path(app) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => parse_accounts(&s),
        Err(_) => Vec::new(),
    }
}

pub fn save_accounts(app: &AppHandle, accounts: &[StoredAccount]) -> Result<(), String> {
    let path = accounts_path(app)?;
    let text =
        serde_json::to_string_pretty(accounts).map_err(|e| format!("序列化账号失败: {e}"))?;
    write_accounts_atomic(&path, &text)
}

/// 原子写入账号文件：同目录临时文件 + rename 覆盖目标。
///
/// 账号文件含 accessToken / clientToken / 密码密文，直接 `fs::write` 一旦在写入中途
/// 崩溃或断电，就会留下半截 JSON；虽然读取会安全回退为空列表，但用户所有账号也会
/// 一起丢失。临时文件写完后 rename 是原子替换，目标要么是旧内容、要么是完整新内容。
fn write_accounts_atomic(path: &Path, text: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "无法定位账号文件所在目录".to_string())?;
    let temp = parent.join(format!("accounts.json.tmp-{}", random_token()));

    if let Err(error) = write_account_file(&temp, text) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("写入账号失败: {error}"));
    }

    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("保存账号失败: {error}"));
    }
    Ok(())
}

/// 写入账号临时文件；Unix 下限制为仅当前用户可读写。
fn write_account_file(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // 账号文件保存令牌与密码密文，只允许当前用户访问；rename 后权限保留到目标文件。
        options.mode(0o600);
    }

    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    // rename 前把内容刷到磁盘，避免断电后出现「文件名已替换、内容仍为空」的账号文件。
    file.sync_all()
}

pub fn find_account(app: &AppHandle, id: &str) -> Option<StoredAccount> {
    load_accounts(app).into_iter().find(|a| a.id == id)
}

pub fn upsert_into(list: &mut Vec<StoredAccount>, account: StoredAccount) {
    match list.iter().position(|a| a.id == account.id) {
        Some(pos) => list[pos] = account,
        None => list.push(account),
    }
}

/// 读取-合并-写回（内部加锁），供登录流程直接落盘。
pub fn upsert_and_save(app: &AppHandle, account: StoredAccount) -> Result<(), String> {
    let _g = lock();
    let mut list = load_accounts(app);
    upsert_into(&mut list, account);
    save_accounts(app, &list)
}

pub fn set_active(app: &AppHandle, id: &str) -> Result<(), String> {
    let path = active_path(app)?;
    let text = serde_json::to_string(&serde_json::json!({ "id": id }))
        .map_err(|e| format!("序列化激活账号失败: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写入激活账号失败: {e}"))
}

// ─────────────────────────────────────────────────────────────────────────────
// 纯工具
// ─────────────────────────────────────────────────────────────────────────────

/// 生成随机 token（用于 Yggdrasil clientToken 等）。非密码学安全，仅需唯一。
pub fn random_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut x = nanos
        ^ counter.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (std::process::id() as u64).wrapping_mul(0x1000_0000_1B3);

    let mut out = String::with_capacity(32);
    for _ in 0..4 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.push_str(&format!("{:08x}", (x & 0xffff_ffff) as u32));
    }
    out
}

/// 生成账号 id。
pub fn new_id() -> String {
    format!("acc_{}", random_token())
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 补全 scheme 与去尾部斜杠；空输入或只有 scheme（`https:`）时返回 `None`。
///
/// `normalize_server` / `yggdrasil_base` / `site_base` 共用这段归一化，
/// 避免多份规则漂移（尤其是 `https://` 去掉尾部斜杠后只剩 `https:` 的边界）。
fn normalized_base(server: &str) -> Option<String> {
    let mut s = server.trim().trim_end_matches('/').to_string();
    if s.is_empty() {
        return None;
    }
    // 只写了 scheme：按站点根补全会拼出 `https://https:` 这种畸形地址。
    if !s.contains("://") && s.ends_with(':') {
        return None;
    }
    if !s.contains("://") {
        // 缺少协议的站点地址统一使用 HTTPS。
        s = format!("https://{s}");
    }
    let s = s.trim_end_matches('/').to_string();
    // `https://` / `http:///` 这类输入没有主机名，继续拼接只会得到畸形地址。
    if !has_site_root(&s) {
        return None;
    }
    Some(s)
}

/// 归一化皮肤站地址：空 → 默认 LittleSkin；补全 `https://`；去尾部 `/`；
/// 剥离历史数据里误填的 `/api/yggdrasil`（[`yggdrasil_base`] 会按站点根重新追加它），
/// 保证落库的 `server` 是站点根（皮肤接口依赖它）。
///
/// 自定义 API 根需以 `/api/yggdrasil` 或 `/yggdrasil` 结尾；其它路径（含站点子目录，
/// 如 `https://example.com/mc`）一律按站点根处理。`/yggdrasil` 形式保留原样，
/// 以保留真正的自定义 API 根。
///
/// 该函数保留「解析失败回退默认站点」的容错，仅供展示 / 皮肤 URL 等非认证路径；
/// 认证路径必须走 [`validate_server`] / [`validate_stored_server`]。
pub fn normalize_server(server: Option<String>) -> String {
    let Some(s) = normalized_base(&server.unwrap_or_default()) else {
        return DEFAULT_SERVER.to_string();
    };
    site_root_from_base(&s)
}

/// 把已补全 scheme 的地址收敛为站点根（剥离已知 Yggdrasil 后缀）。
fn site_root_from_base(base: &str) -> String {
    let s = base.strip_suffix(YGGDRASIL_SUFFIX).unwrap_or(base);
    let s = s.trim_end_matches('/');
    if s.is_empty() {
        DEFAULT_SERVER.to_string()
    } else {
        s.to_string()
    }
}

/// 校验地址在补全 scheme 后是否为合法站点根。
///
/// 认证请求会把密码 / 令牌发到这里，不能像 [`normalize_server`] 那样在解析失败时
/// 静默回退默认站点：`https://`、`ftp://x` 之类的输入一旦回退，就可能把凭据
/// 发往用户并未指定的服务器。
fn checked_base(server: &str) -> Option<String> {
    let trimmed = server.trim();
    let lower = trimmed.to_ascii_lowercase();
    // `https:/host` / `https:host` 缺少 `//`，补全逻辑会把它误当成域名；
    // 认证路径直接拒绝，避免发往 `https://https:` 之类的畸形主机。
    if (lower.starts_with("http:") || lower.starts_with("https:")) && !lower.contains("://") {
        return None;
    }
    let base = normalized_base(trimmed)?;
    let url = minecraft_java_rs_core::net::client::reqwest::Url::parse(&base).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.host_str().map(str::is_empty).unwrap_or(true) {
        return None;
    }
    // 拒绝 `user:pass@host` 形式的 userinfo，避免把凭据写进 URL。
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    if url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    Some(base)
}

/// 校验用户输入的皮肤站地址并归一化为站点根。
///
/// `None` / 全空白保持默认 LittleSkin 行为；非空但非法的地址必须报错，
/// 不能回退默认站点，否则密码 / 令牌可能被发往非预期服务器。
pub fn validate_server(server: Option<String>) -> Result<String, String> {
    let raw = server.unwrap_or_default();
    if raw.trim().is_empty() {
        return Ok(DEFAULT_SERVER.to_string());
    }
    let Some(base) = checked_base(&raw) else {
        return Err(INVALID_SERVER_MESSAGE.to_string());
    };
    Ok(site_root_from_base(&base))
}

/// 校验已落盘账号的皮肤站地址。
///
/// 与 [`validate_server`] 的区别只有错误文案：已有账号的地址来自历史数据，
/// 提示用户去账号设置重新登录，而不是让刷新 / 校验流程悄悄改投默认站点。
pub fn validate_stored_server(server: Option<String>) -> Result<String, String> {
    match server {
        None => Ok(DEFAULT_SERVER.to_string()),
        Some(raw) if raw.trim().is_empty() => Err(INVALID_STORED_SERVER_MESSAGE.to_string()),
        Some(raw) => {
            validate_server(Some(raw)).map_err(|_| INVALID_STORED_SERVER_MESSAGE.to_string())
        }
    }
}

/// 是否具备可用的 `scheme://host`（scheme 与 host 均非空）。
fn has_site_root(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    !scheme.is_empty() && !rest.split('/').next().unwrap_or_default().is_empty()
}

/// 识别 URL 末尾的已知 Yggdrasil API 根后缀（长后缀优先）。
///
/// 只有 `/api/yggdrasil` 与 `/yggdrasil` 被视为 API 根；其它路径（含站点子目录）
/// 一律按站点根处理，由 [`yggdrasil_base`] 追加固定后缀。剥离后必须仍保有站点根，
/// 避免把主机名恰好叫 `yggdrasil` 的 `https://yggdrasil` 误判成 API 路径。
fn trailing_yggdrasil_suffix(url: &str) -> Option<&'static str> {
    YGGDRASIL_SUFFIXES
        .into_iter()
        .find(|suffix| url.strip_suffix(suffix).is_some_and(has_site_root))
}

/// 剥掉末尾已知的 Yggdrasil API 根后缀并去掉尾部 `/`；无法识别时返回 `None`。
fn strip_yggdrasil_suffix(url: &str) -> Option<String> {
    let suffix = trailing_yggdrasil_suffix(url)?;
    let rest = url.strip_suffix(suffix)?;
    Some(rest.trim_end_matches('/').to_string())
}

/// 由站点地址推导 Yggdrasil API 根。
///
/// 本项目账号类型是 `littleskin`（Blessing Skin 系），默认的 Yggdrasil 挂在
/// `/api/yggdrasil`。规则：
/// - 以 `/api/yggdrasil` 或 `/yggdrasil` 结尾 → 视为完整 API 根，原样返回；
/// - 其它路径一律按站点根处理，追加 `/api/yggdrasil`（保留站点子目录，
///   如 `https://example.com/mc` → `https://example.com/mc/api/yggdrasil`）。
pub fn yggdrasil_base(server: &str) -> String {
    let Some(s) = normalized_base(server) else {
        return join(DEFAULT_SERVER, YGGDRASIL_SUFFIX);
    };
    if trailing_yggdrasil_suffix(&s).is_some() {
        return s;
    }
    join(&s, YGGDRASIL_SUFFIX)
}

/// 由（可能带 Yggdrasil 路径的）地址推导站点根。
///
/// 站点级接口（如 `/avatar/player/...`）必须挂在站点根，不能带 Yggdrasil 后缀。
/// 规则：只剥掉已知的 `/api/yggdrasil` / `/yggdrasil` 后缀并保留前面的子目录
/// （`https://example.com/mc/api/yggdrasil` → `https://example.com/mc`）；
/// 无法识别时原样返回，不退回 origin，也不丢弃站点子目录。
pub fn site_base(server: &str) -> String {
    let Some(s) = normalized_base(server) else {
        return DEFAULT_SERVER.to_string();
    };
    match strip_yggdrasil_suffix(&s) {
        Some(root) => root,
        None => s,
    }
}

/// 拼接 base 与 path，避免双斜杠。
pub fn join(base: &str, path: &str) -> String {
    let b = base.trim_end_matches('/');
    let p = path.trim_start_matches('/');
    format!("{b}/{p}")
}

/// 32 位无连字符 UUID → 带连字符形式；否则原样返回。
pub fn dash_uuid(raw: &str) -> String {
    let s = raw.trim();
    if s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        format!(
            "{}-{}-{}-{}-{}",
            &s[0..8],
            &s[8..12],
            &s[12..16],
            &s[16..20],
            &s[20..32]
        )
    } else {
        s.to_string()
    }
}

/// 去掉连字符（Yggdrasil / Mojang 接受无连字符 UUID）。
pub fn undash_uuid(raw: &str) -> String {
    raw.chars().filter(|c| *c != '-').collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dash_and_undash_uuid_roundtrip() {
        let raw = "069a79f444e94726a5befca90e38aaf5";
        let dashed = dash_uuid(raw);
        assert_eq!(dashed, "069a79f4-44e9-4726-a5be-fca90e38aaf5");
        assert_eq!(undash_uuid(&dashed), raw);
        assert_eq!(dash_uuid(&dashed), dashed);
        assert_eq!(dash_uuid("not-a-uuid"), "not-a-uuid");
    }

    #[test]
    fn to_public_hides_secrets() {
        let stored = StoredAccount {
            id: "acc_1".into(),
            account_type: "littleskin".into(),
            name: "Keyi".into(),
            uuid: "uuid-1".into(),
            email: Some("a@b.c".into()),
            server: Some(DEFAULT_SERVER.into()),
            avatar_url: None,
            access_token: Some("ACCESS-TOKEN-VALUE".into()),
            client_token: Some("CLIENT-TOKEN-VALUE".into()),
            refresh_token: Some("REFRESH-TOKEN-VALUE".into()),
            expires_at: Some(123),
            password: Some(SealedSecret::new("v1:CIPHER-VALUE")),
            oauth: Some(SealedSecret::new("v1:OAUTH-CIPHER")),
            auth_api: None,
        };
        let public = stored.to_public();
        let value = serde_json::to_value(&public).unwrap();
        assert!(value.get("accessToken").is_none());
        assert!(value.get("clientToken").is_none());
        assert!(value.get("refreshToken").is_none());
        assert!(value.get("oauth").is_none());
        assert!(value.get("password").is_none(), "密码密文不得回传前端");
        assert_eq!(
            value.get("type").and_then(|v| v.as_str()),
            Some("littleskin")
        );
        assert_eq!(value.get("avatarUrl"), Some(&serde_json::Value::Null));

        // 落盘结构保留密文（切换角色时需要），但 Debug 必须脱敏。
        let persisted = serde_json::to_value(&stored).unwrap();
        assert_eq!(
            persisted.get("password").and_then(|v| v.as_str()),
            Some("v1:CIPHER-VALUE")
        );
        let debug = format!("{stored:?}");
        for secret in [
            "ACCESS-TOKEN-VALUE",
            "CLIENT-TOKEN-VALUE",
            "REFRESH-TOKEN-VALUE",
            "CIPHER-VALUE",
        ] {
            assert!(!debug.contains(secret), "Debug 泄露敏感字段 {secret}：{debug}");
        }
        assert!(debug.contains("<redacted>"), "{debug}");
        assert!(debug.contains("acc_1") && debug.contains("Keyi"), "{debug}");
    }

    #[test]
    fn parse_accounts_handles_corruption() {
        assert!(parse_accounts("").is_empty());
        assert!(parse_accounts("   ").is_empty());
        assert!(parse_accounts("{not json").is_empty());
        assert_eq!(parse_accounts("[{\"id\":\"x\"}]").len(), 1);
    }

    #[test]
    fn public_auth_method_comes_from_stored_credentials() {
        let mut stored = StoredAccount {
            account_type: "littleskin".into(),
            ..Default::default()
        };
        assert_eq!(stored.to_public().auth_method.as_deref(), Some("classic"));
        stored.oauth = Some(SealedSecret::new("fixture-sealed-oauth"));
        let public = serde_json::to_value(stored.to_public()).unwrap();
        assert_eq!(public["authMethod"], "oauth2");
        assert!(!public.to_string().contains("fixture-sealed-oauth"));
        stored.account_type = "microsoft".into();
        assert_eq!(stored.to_public().auth_method, None);
        let legacy: Account = serde_json::from_value(serde_json::json!({
            "id": "legacy", "type": "littleskin", "name": "Player", "uuid": "uuid"
        })).unwrap();
        assert_eq!(legacy.auth_method, None);
    }


    #[test]
    fn normalize_fills_defaults() {
        let a = StoredAccount {
            name: "Steve".into(),
            account_type: String::new(),
            ..Default::default()
        }
        .normalize();
        assert!(a.id.starts_with("acc_"));
        assert_eq!(a.account_type, "offline");
        assert!(!a.uuid.is_empty());
    }

    #[test]
    fn server_normalization_and_join() {
        assert_eq!(normalize_server(None), DEFAULT_SERVER);
        assert_eq!(normalize_server(Some("   ".into())), DEFAULT_SERVER);
        assert_eq!(
            normalize_server(Some("https://littleskin.cn/".into())),
            "https://littleskin.cn"
        );
        // 仅域名输入也须归一化为 HTTPS。
        assert_eq!(
            normalize_server(Some("littleskin.cn".into())),
            "https://littleskin.cn"
        );
        // 误把 Yggdrasil 根填进站点地址时，落库仍回到站点根（皮肤接口需要它）。
        assert_eq!(
            normalize_server(Some("https://littleskin.cn/api/yggdrasil/".into())),
            "https://littleskin.cn"
        );
        // 站点子目录必须保留：认证路径由 yggdrasil_base 在站点根后追加。
        assert_eq!(
            normalize_server(Some("https://example.com/mc".into())),
            "https://example.com/mc"
        );
        assert_eq!(
            normalize_server(Some("https://example.com/mc/api/yggdrasil/".into())),
            "https://example.com/mc"
        );
        assert_eq!(
            normalize_server(Some("example.com:8443/mc".into())),
            "https://example.com:8443/mc"
        );
        assert_eq!(
            join("https://littleskin.cn/", "/authserver/authenticate"),
            "https://littleskin.cn/authserver/authenticate"
        );
    }

    #[test]
    fn validate_server_accepts_and_normalizes() {
        assert_eq!(validate_server(None).unwrap(), DEFAULT_SERVER);
        assert_eq!(validate_server(Some(String::new())).unwrap(), DEFAULT_SERVER);
        assert_eq!(
            validate_server(Some("   ".into())).unwrap(),
            DEFAULT_SERVER
        );

        for (raw, expected) in [
            ("littleskin.cn", "https://littleskin.cn"),
            ("https://example.com/mc", "https://example.com/mc"),
            (
                "https://example.com/yggdrasil",
                "https://example.com/yggdrasil",
            ),
            (
                "https://example.com:8443/mc",
                "https://example.com:8443/mc",
            ),
            ("https://littleskin.cn/api/yggdrasil/", DEFAULT_SERVER),
        ] {
            assert_eq!(
                validate_server(Some(raw.into())).unwrap(),
                expected,
                "输入 {raw:?}"
            );
        }
    }

    #[test]
    fn validate_server_rejects_malformed_addresses() {
        for raw in [
            "https:",
            "https://",
            "http:///",
            "https:/host",
            "http:example.com",
            "ftp://x",
            "https://user:pass@host",
            "https://host?x=1",
            "https://host#frag",
        ] {
            let error = validate_server(Some(raw.into())).unwrap_err();
            assert_eq!(
                error,
                "皮肤站地址无效，请填写完整地址，例如 https://littleskin.cn",
                "输入 {raw:?}"
            );
        }
    }

    #[test]
    fn validate_stored_server_uses_account_specific_error() {
        assert_eq!(validate_stored_server(None).unwrap(), DEFAULT_SERVER);
        assert_eq!(
            validate_stored_server(Some("  ".into())).unwrap_err(),
            INVALID_STORED_SERVER_MESSAGE
        );
        assert_eq!(
            validate_stored_server(Some("littleskin.cn".into())).unwrap(),
            "https://littleskin.cn"
        );

        let error = validate_stored_server(Some("https://".into())).unwrap_err();
        assert_eq!(
            error,
            "账号保存的皮肤站地址无效，请在账号设置中重新登录并填写正确地址"
        );
    }

    #[test]
    fn save_accounts_writes_atomically_without_temp_leftovers() {
        let dir = temp_dir("accounts-atomic");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("accounts.json");

        let accounts = vec![StoredAccount {
            id: "acc_1".into(),
            account_type: "littleskin".into(),
            name: "Keyi".into(),
            ..Default::default()
        }];
        let text = serde_json::to_string_pretty(&accounts).unwrap();
        write_accounts_atomic(&path, &text).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        assert_eq!(parse_accounts(&text).len(), 1);

        // 覆盖写必须完整替换，不能留下旧内容或半截 JSON。
        write_accounts_atomic(&path, "[]").unwrap();
        assert!(parse_accounts(&std::fs::read_to_string(&path).unwrap()).is_empty());

        // 临时文件必须在 rename 后消失。
        assert_no_account_temp_files(&dir);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "账号文件必须仅当前用户可读写");
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_accounts_reports_rename_failure_and_cleans_temp() {
        let dir = temp_dir("accounts-rename-fail");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("accounts.json");
        // 目标被目录占用，rename 必然失败：必须返回中文错误且不残留临时文件。
        std::fs::create_dir(&path).unwrap();

        let error = write_accounts_atomic(&path, "[]").unwrap_err();
        assert!(error.contains("保存账号失败"), "{error}");
        assert_no_account_temp_files(&dir);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 测试用唯一临时目录（`tempfile` 未作为依赖引入）。
    fn temp_dir(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "nickel_store_{}_{}_{}",
            std::process::id(),
            random_token(),
            tag
        ));
        path
    }

    fn assert_no_account_temp_files(dir: &Path) {
        let leftovers: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("accounts.json.tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件: {leftovers:?}");
    }

    #[test]
    fn yggdrasil_base_only_treats_known_suffixes_as_api_root() {
        // 站点根 → Blessing Skin 的固定 Yggdrasil 路径（LittleSkin 认证接口的唯一正确入口）。
        assert_eq!(
            yggdrasil_base("https://littleskin.cn"),
            "https://littleskin.cn/api/yggdrasil"
        );
        assert_eq!(
            yggdrasil_base("littleskin.cn/api/yggdrasil"),
            "https://littleskin.cn/api/yggdrasil"
        );
        // 已显式写成 API 根的地址保持原样，避免拼出 `/yggdrasil/api/yggdrasil`。
        assert_eq!(
            yggdrasil_base("https://example.org/yggdrasil"),
            "https://example.org/yggdrasil"
        );
        // 站点子目录必须保留，只在末尾追加固定的 Yggdrasil 路径。
        assert_eq!(
            yggdrasil_base("https://example.com/mc"),
            "https://example.com/mc/api/yggdrasil"
        );
        assert_eq!(
            yggdrasil_base("https://example.com/mc/api/yggdrasil"),
            "https://example.com/mc/api/yggdrasil"
        );
        assert_eq!(
            yggdrasil_base("https://example.com/mc/yggdrasil"),
            "https://example.com/mc/yggdrasil"
        );
        assert_eq!(
            yggdrasil_base("example.com/mc"),
            "https://example.com/mc/api/yggdrasil"
        );
        assert_eq!(
            yggdrasil_base("https://example.com:8443/mc"),
            "https://example.com:8443/mc/api/yggdrasil"
        );
        // 无法识别为 API 后缀的路径按站点根处理（不再当作自定义 API 根）。
        assert_eq!(
            yggdrasil_base("https://example.com/custom/auth"),
            "https://example.com/custom/auth/api/yggdrasil"
        );
        // 裸站点根（含尾部斜杠）仍然追加 Blessing Skin 的固定路径。
        assert_eq!(
            yggdrasil_base("https://littleskin.cn/"),
            "https://littleskin.cn/api/yggdrasil"
        );
        assert_eq!(
            yggdrasil_base("https://littleskin.cn/api/yggdrasil"),
            "https://littleskin.cn/api/yggdrasil"
        );
        assert_eq!(
            join(&yggdrasil_base("https://littleskin.cn"), "/authserver/authenticate"),
            "https://littleskin.cn/api/yggdrasil/authserver/authenticate"
        );
        assert_eq!(
            join(&yggdrasil_base("https://example.com/mc"), "/authserver/authenticate"),
            "https://example.com/mc/api/yggdrasil/authserver/authenticate"
        );
    }

    #[test]
    fn yggdrasil_base_falls_back_for_incomplete_scheme() {
        let fallback = join(DEFAULT_SERVER, YGGDRASIL_SUFFIX);
        // 只有 scheme 的输入不能拼成 `https://https:` 之类的畸形地址。
        for raw in ["https:", "https://", "   ", "http:///"] {
            assert_eq!(yggdrasil_base(raw), fallback, "输入 {raw:?}");
        }
    }

    #[test]
    fn site_base_strips_known_yggdrasil_suffixes_only() {
        // 裸站点根原样返回（补全协议、去掉尾部斜杠）。
        assert_eq!(site_base("https://littleskin.cn"), DEFAULT_SERVER);
        assert_eq!(site_base("littleskin.cn/"), DEFAULT_SERVER);
        // 误填 Yggdrasil 根时仍回到站点根；头像接口不能挂在 API 路径下。
        assert_eq!(
            site_base(&join(DEFAULT_SERVER, YGGDRASIL_SUFFIX)),
            DEFAULT_SERVER
        );
        assert_eq!(site_base("https://littleskin.cn/yggdrasil"), DEFAULT_SERVER);
        // 站点子目录必须保留，只剥掉已知的 Yggdrasil 后缀。
        assert_eq!(
            site_base("https://example.com/mc"),
            "https://example.com/mc"
        );
        assert_eq!(
            site_base("https://example.com/mc/api/yggdrasil"),
            "https://example.com/mc"
        );
        assert_eq!(
            site_base("https://example.com/mc/yggdrasil"),
            "https://example.com/mc"
        );
        assert_eq!(
            site_base("https://example.com:8443/mc/api/yggdrasil/"),
            "https://example.com:8443/mc"
        );
        // 无法识别为 Yggdrasil 后缀的路径不是 API 根：原样返回，不退回 origin。
        assert_eq!(
            site_base("https://example.com/custom/auth"),
            "https://example.com/custom/auth"
        );
        assert_eq!(
            site_base("https://example.com:8443/custom/auth"),
            "https://example.com:8443/custom/auth"
        );
        // 空 / 只有 scheme 的输入回退默认站点根。
        assert_eq!(site_base("   "), DEFAULT_SERVER);
        assert_eq!(site_base("https:"), DEFAULT_SERVER);
    }

}
