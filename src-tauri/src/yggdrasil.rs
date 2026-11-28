//! Yggdrasil 认证（LittleSkin / Blessing Skin 型皮肤站，默认 `https://littleskin.cn`）。
//!
//! 真实实现 `POST {site}/api/yggdrasil/authserver/authenticate|refresh|validate`：
//! - 账号落库保存**站点根**（皮肤接口 `{site}/api/skin` 用它），认证时再推导 Yggdrasil API 根；
//! - clientToken 由后端生成并随账号缓存
//! - agent 固定 `{ name: "Minecraft", version: 1 }`（`name` 大小写敏感，LittleSkin 拒绝小写）
//! - 错误按 `errorMessage` 映射为可读中文
//! - 皮肤站切换到 OAuth2 时，角色选择仍走 `yggdrasil_select_profile`；`oauth_*` 会话转交 `skin_oauth`，密码认证仍是默认路径
//!
//! ## 多角色账号
//! 同一账号在皮肤站下可能有多个角色（`availableProfiles`）。此时**不能**擅自挑一个就落库，
//! 否则用户永远切不到另一个角色。流程改为两步：
//! 1. `yggdrasil_login` 返回 `{"status":"select", "sessionId":"...", "profiles":[{id,name}]}`，
//!    并把本次登录的敏感上下文（accessToken / clientToken / 服务器 / 用户名 / 角色列表）
//!    **只**暂存在进程内的 `PENDING_LOGINS`，带 10 分钟 TTL：**不落盘、不回传前端**；
//! 2. 前端让用户选择后调用 `yggdrasil_select_profile(sessionId, profileId)` 完成落库；
//!    或调用 `yggdrasil_cancel(sessionId)` 放弃。
//!
//! 会话表不跨进程持久化——它是「本次登录的临时中间态」，进程重启即失效，比落盘更安全。
//!
//! 单角色账号仍沿用旧行为：直接落库并返回 `{"status":"account", "account":{...}}`，
//! 避免无意义的二次交互（绝大多数账号只有一个角色）。
//!
//! ## 切换角色（已登录账号）
//!
//! LittleSkin 使用 Yggdrasil Connect：绑定过角色的令牌既不能枚举其它角色，也不能
//! 改绑（`refresh` 会返回「请求的角色与令牌绑定的角色不一致」）。因此
//! [`yggdrasil_profiles`] 会用本机加密保存的密码重新 `authenticate`，把完整
//! `availableProfiles` 与「未绑定令牌」放进同一个待选会话；选角后
//! [`yggdrasil_select_profile`] 复用原账号 id 绑定，取消则用该令牌回绑原角色。
//! 密码明文只存在于本次请求的内存中，落盘的是 [`crate::vault`] 的密文。

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::http;
use crate::store::{self, Account, SealedSecret, StoredAccount};

/// 待选择角色的登录会话有效期。
const PENDING_TTL: Duration = Duration::from_secs(10 * 60);

/// 角色条目（`id` 为无连字符 UUID）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthResponse {
    access_token: String,
    #[serde(default)]
    client_token: Option<String>,
    #[serde(default)]
    selected_profile: Option<Profile>,
    /// 账号下的全部角色；只有部分皮肤站在多角色时返回。
    #[serde(default)]
    available_profiles: Vec<Profile>,
}

impl std::fmt::Debug for AuthResponse {
    /// 手写 `Debug`：结构体含访问令牌，不能派生后让它出现在日志里。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let secret = |value: &Option<String>| value.as_ref().map(|_| "<redacted>");
        f.debug_struct("AuthResponse")
            .field("access_token", &"<redacted>")
            .field("client_token", &secret(&self.client_token))
            .field("selected_profile", &self.selected_profile)
            .field("available_profiles", &self.available_profiles)
            .finish()
    }
}

/// 登录结果。序列化时以 `status` 为判别字段（前端按此分支）。
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LoginOutcome {
    /// 只有一个角色：已直接落库，返回脱敏账号。
    Account { account: Account },
    /// 多角色：需要用户选择，`sessionId` 用于后续 select / cancel（**不含任何 token**）。
    Select {
        session_id: String,
        profiles: Vec<Profile>,
    },
}

/// 一次「等待选择角色」的登录 / 切换上下文（**含敏感字段**，只存在于进程内存）。
struct PendingLogin {
    created_at: Instant,
    access_token: String,
    client_token: String,
    server: String,
    username: String,
    /// 登录密码的密文：选角落库时直接写入账号，进程内不保留明文。
    password: Option<SealedSecret>,
    /// 本次会话要更新的既有账号 id；`None` 表示登录流程要新建账号。
    account_id: Option<String>,
    /// 切换角色前的角色：取消选角时用它回绑，避免账号令牌悬空。
    current_profile: Option<Profile>,
    profiles: Vec<Profile>,
}

/// 进程级待选角色会话表：sessionId → 上下文。
static PENDING_LOGINS: LazyLock<Mutex<HashMap<String, PendingLogin>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn pending_lock() -> MutexGuard<'static, HashMap<String, PendingLogin>> {
    PENDING_LOGINS.lock().unwrap_or_else(|e| e.into_inner())
}

/// 清理过期会话（纯函数，便于测试）。每次访问会话表时顺手调用，避免陈旧 token 长期驻留内存。
fn purge_expired(map: &mut HashMap<String, PendingLogin>, now: Instant) {
    map.retain(|_, pending| now.duration_since(pending.created_at) < PENDING_TTL);
}

/// 登录路由决策（纯逻辑）：单角色直接落库，多角色交给用户选择。
#[derive(Debug, PartialEq)]
enum LoginRoute {
    Direct(Profile),
    Select(Vec<Profile>),
    NoProfile,
}

fn decide_login_route(selected: Option<Profile>, available: Vec<Profile>) -> LoginRoute {
    // 多角色：无法自动决定用户想要哪个，必须走 select。
    if available.len() > 1 {
        return LoginRoute::Select(available);
    }
    // 单角色（或仅返回 selectedProfile）：直接落库，省掉一次多余交互。
    if let Some(profile) = selected {
        return LoginRoute::Direct(profile);
    }
    match available.into_iter().next() {
        Some(profile) => LoginRoute::Direct(profile),
        None => LoginRoute::NoProfile,
    }
}

/// 将 Yggdrasil 错误映射为可读中文（纯逻辑，可测）。
pub fn yggdrasil_error_message(
    error: Option<&str>,
    message: Option<&str>,
    status: u16,
) -> String {
    let msg = message.unwrap_or("").trim();
    let err = error.unwrap_or("").trim().to_lowercase();
    let lower = msg.to_lowercase();

    if lower.contains("invalid credentials")
        || lower.contains("invalid username or password")
        || lower.contains("密码")
        || lower.contains("incorrect")
    {
        return "用户名或密码错误".to_string();
    }
    if lower.contains("locked") || lower.contains("封禁") || lower.contains("banned") {
        return "账号已被锁定或封禁".to_string();
    }
    // LittleSkin 对「未绑定角色的令牌 + 不带 selectedProfile 的刷新」返回这条中英双语错误。
    // 正常流程已带 selectedProfile 绑定；这里只作为旧账号 / 异常响应的可读兜底。
    if lower.contains("no profile assigned") || msg.contains("未绑定角色") {
        return "登录凭证尚未绑定角色，请重新登录并选择角色".to_string();
    }
    if lower.contains("token") && (lower.contains("invalid") || lower.contains("expired")) {
        return "登录凭证已失效，请重新登录".to_string();
    }
    if err.contains("forbiddenoperationexception") && msg.is_empty() {
        return "认证被拒绝（用户名或密码错误）".to_string();
    }

    if !msg.is_empty() {
        return msg.to_string();
    }
    if !err.is_empty() {
        return err;
    }
    format!("认证失败（HTTP {status}）")
}

fn extract_error(value: &serde_json::Value) -> (Option<String>, Option<String>) {
    let error = value
        .get("error")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let message = value
        .get("errorMessage")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (error, message)
}

/// 登录态构造的 `selectedProfile`（Yggdrasil 刷新需要）。
fn selected_profile_json(account: &StoredAccount) -> serde_json::Value {
    serde_json::json!({
        "id": store::undash_uuid(&account.uuid),
        "name": account.name,
    })
}

/// 单个角色 → `selectedProfile` 请求体（刷新 / 绑定共用）。
fn profile_json(profile: &Profile) -> serde_json::Value {
    serde_json::json!({
        "id": store::undash_uuid(&profile.id),
        "name": profile.name,
    })
}

/// 「切换角色」的返回值：完整角色列表 + 本次待选会话。
///
/// 会话让用户选角时不必再次输入密码；不选而关闭时，后端会用会话里的
/// 未绑定令牌把账号回绑到原角色，保证令牌始终可用。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileList {
    pub session_id: String,
    pub profiles: Vec<Profile>,
}

/// 由角色 + 登录凭证构造落库账号（登录与选择角色共用，避免两处逻辑漂移）。
fn build_account(
    profile: &Profile,
    access_token: &str,
    client_token: String,
    server: &str,
    username: &str,
    password: Option<SealedSecret>,
) -> StoredAccount {
    StoredAccount {
        id: store::new_id(),
        account_type: "littleskin".to_string(),
        name: profile.name.clone(),
        uuid: store::dash_uuid(&profile.id),
        email: Some(username.to_string()),
        server: Some(server.to_string()),
        avatar_url: None,
        access_token: Some(access_token.to_string()),
        client_token: Some(client_token),
        refresh_token: None,
        expires_at: None,
        password,
        oauth: None,
        auth_api: None,
    }
    .normalize()
}

/// 用邮箱 + 密码换取未绑定 / 已绑定的访问令牌。
///
/// 登录与「切换角色」共用：只有 `authenticate` 能拿到完整的 `availableProfiles`，
/// 绑定后的令牌刷新只会返回当前角色。
async fn authenticate(
    server: &str,
    username: &str,
    password: &str,
) -> Result<(AuthResponse, String), String> {
    let client_token = store::random_token();
    let body = serde_json::json!({
        "username": username,
        "password": password,
        "clientToken": client_token,
        "agent": { "name": "Minecraft", "version": 1 },
        "requestUser": true,
    });

    let auth_base = store::yggdrasil_base(server);
    let url = store::join(&auth_base, "/authserver/authenticate");
    let (status, value) = http::post_json(&url, &body, None).await?;

    if status != 200 {
        let (error, message) = extract_error(&value);
        return Err(yggdrasil_error_message(
            error.as_deref(),
            message.as_deref(),
            status,
        ));
    }

    let resp: AuthResponse =
        serde_json::from_value(value).map_err(|e| format!("解析登录响应失败: {e}"))?;
    Ok((resp, client_token))
}

/// 用户名 + 密码登录皮肤站。
///
/// 单角色 → `{"status":"account", ...}`；多角色 → `{"status":"select", ...}`（token 不落盘、不回传）。
#[tauri::command]
pub async fn yggdrasil_login(
    app: AppHandle,
    server: Option<String>,
    username: String,
    password: String,
) -> Result<LoginOutcome, String> {
    // 认证路径必须严格校验用户输入：非空畸形地址不能回退默认站点。
    let site = store::validate_server(server)?;
    if username.trim().is_empty() {
        return Err("请输入邮箱 / 用户名".to_string());
    }
    if password.is_empty() {
        return Err("请输入密码".to_string());
    }

    let (resp, requested_client) = authenticate(&site, username.trim(), &password).await?;
    // 密码到此为止：立刻换成密文，后续流程不再持有明文。
    let sealed = crate::vault::seal(&app, &password)?;

    match decide_login_route(resp.selected_profile, resp.available_profiles) {
        LoginRoute::NoProfile => {
            Err("服务器未返回角色信息（该账号可能未创建角色）".to_string())
        }
        LoginRoute::Direct(profile) => {
            let account = build_account(
                &profile,
                &resp.access_token,
                resp.client_token.unwrap_or(requested_client),
                &site,
                username.trim(),
                Some(SealedSecret::new(sealed)),
            );
            store::upsert_and_save(&app, account.clone())?;
            Ok(LoginOutcome::Account {
                account: account.to_public(),
            })
        }
        LoginRoute::Select(profiles) => {
            let session_id = format!("sess_{}", store::random_token());
            {
                let mut map = pending_lock();
                let now = Instant::now();
                purge_expired(&mut map, now);
                map.insert(
                    session_id.clone(),
                    PendingLogin {
                        created_at: now,
                        access_token: resp.access_token,
                        client_token: resp
                            .client_token
                            .unwrap_or_else(|| requested_client.clone()),
                        server: site,
                        username: username.trim().to_string(),
                        password: Some(SealedSecret::new(sealed)),
                        account_id: None,
                        current_profile: None,
                        profiles: profiles.clone(),
                    },
                );
            }
            Ok(LoginOutcome::Select {
                session_id,
                profiles,
            })
        }
    }
}

/// 从待选会话中选定角色并落库，返回脱敏账号。
#[tauri::command]
pub async fn yggdrasil_select_profile(
    app: AppHandle,
    session_id: String,
    profile_id: String,
) -> Result<Account, String> {
    if session_id.starts_with("oauth_") { return crate::skin_oauth::select(app, session_id, profile_id).await; }
    // 会话表锁必须限制在这个块内：后面要 await 刷新，MutexGuard 不能跨 await。
    let (profile, pending) = {
        let mut map = pending_lock();
        let now = Instant::now();
        purge_expired(&mut map, now);

        // 先校验再移除：用户选错角色时仍可重试，不必重新输入密码。
        let profile = match map.get(&session_id) {
            Some(pending) => match pending.profiles.iter().find(|p| p.id == profile_id) {
                Some(found) => found.clone(),
                None => return Err("所选角色不存在，请重新选择".to_string()),
            },
            None => return Err("登录会话已过期，请重新登录".to_string()),
        };
        let pending = match map.remove(&session_id) {
            Some(pending) => pending,
            None => return Err("登录会话已过期，请重新登录".to_string()),
        };
        (profile, pending)
    };

    // 切换角色时复用原账号 id：否则每换一次角色就会多出一条账号记录。
    let mut account = match pending
        .account_id
        .as_deref()
        .and_then(|id| store::find_account(&app, id))
    {
        Some(mut existing) => {
            existing.account_type = "littleskin".to_string();
            existing.name = profile.name.clone();
            existing.uuid = store::dash_uuid(&profile.id);
            existing.email = Some(pending.username.clone());
            existing.server = Some(pending.server.clone());
            existing.access_token = Some(pending.access_token.clone());
            existing.client_token = Some(pending.client_token.clone());
            if pending.password.is_some() {
                existing.password = pending.password.clone();
            }
            existing
        }
        None => build_account(
            &profile,
            &pending.access_token,
            pending.client_token.clone(),
            &pending.server,
            &pending.username,
            pending.password.clone(),
        ),
    };

    // authenticate 对多角色账号返回的是「未绑定角色」的访问令牌；必须在选择角色时
    // 带 selectedProfile 刷新一次，把令牌绑定到该角色。否则之后任何不带角色的刷新
    // 都会被 LittleSkin 拒绝（The access token has no profile assigned）。
    match refresh_tokens(&account, Some(profile_json(&profile))).await {
        Ok((resp, client_token)) => {
            merge_refresh(&mut account, &resp, client_token, Some(&profile));
            store::upsert_and_save(&app, account.clone())?;
            Ok(account.to_public())
        }
        Err(error) => {
            // 刷新失败（网络抖动 / 临时 5xx）时把会话放回，允许用户直接重试，
            // 不用重新输入密码；成功路径已经移除了会话，保持单次使用。
            let mut map = pending_lock();
            map.entry(session_id).or_insert(pending);
            Err(error)
        }
    }
}

/// 放弃本次「等待选择角色」的会话（幂等：会话不存在也直接返回）。
///
/// 切换角色时 `authenticate` 可能已让账号原令牌失效，因此取消时必须用会话里的
/// 未绑定令牌回绑原角色，保证账号还能直接启动。回绑失败不阻塞关闭弹窗 ——
/// 下次启动会走刷新 / 重新登录兜底。
#[tauri::command]
pub async fn yggdrasil_cancel(app: AppHandle, session_id: String) -> Result<(), String> {
    if session_id.starts_with("oauth_") { crate::skin_oauth::skin_oauth_cancel(Some(session_id)); return Ok(()); }
    let pending = {
        let mut map = pending_lock();
        purge_expired(&mut map, Instant::now());
        map.remove(&session_id)
    };
    let Some(pending) = pending else {
        return Ok(());
    };
    let (Some(account_id), Some(current)) = (
        pending.account_id.clone(),
        pending.current_profile.clone(),
    ) else {
        return Ok(());
    };
    let Some(mut account) = store::find_account(&app, &account_id) else {
        return Ok(());
    };

    let session_account = StoredAccount {
        access_token: Some(pending.access_token.clone()),
        client_token: Some(pending.client_token.clone()),
        server: Some(pending.server.clone()),
        ..Default::default()
    };
    if let Ok((resp, client_token)) =
        refresh_tokens(&session_account, Some(profile_json(&current))).await
    {
        merge_refresh(&mut account, &resp, client_token, Some(&current));
        let _ = store::upsert_and_save(&app, account);
    }
    Ok(())
}

/// 用登录凭证换新 token；`selected` 为空时只刷新并返回账号下的角色列表。
async fn refresh_tokens(
    account: &StoredAccount,
    selected: Option<serde_json::Value>,
) -> Result<(AuthResponse, String), String> {
    let access_token = account
        .access_token
        .clone()
        .ok_or_else(|| "该账号缺少访问令牌，请重新登录".to_string())?;
    let client_token = account
        .client_token
        .clone()
        .unwrap_or_else(store::random_token);
    // 已落盘账号的地址非法时拒绝刷新，绝不把令牌发往默认站点。
    let site = store::validate_stored_server(account.server.clone())?;
    let auth_base = store::yggdrasil_base(&site);

    let mut body = serde_json::json!({
        "accessToken": access_token,
        "clientToken": client_token,
        "requestUser": true,
    });
    if let Some(profile) = selected {
        body["selectedProfile"] = profile;
    }

    let url = store::join(&auth_base, "/authserver/refresh");
    let (status, value) = http::post_json(&url, &body, None).await?;

    if status != 200 {
        let (error, message) = extract_error(&value);
        return Err(yggdrasil_error_message(
            error.as_deref(),
            message.as_deref(),
            status,
        ));
    }

    let resp: AuthResponse =
        serde_json::from_value(value).map_err(|e| format!("解析刷新响应失败: {e}"))?;
    Ok((resp, client_token))
}

/// 把刷新结果合并进账号；服务端没回 `selectedProfile` 时使用请求的角色作为回退。
fn merge_refresh(
    account: &mut StoredAccount,
    resp: &AuthResponse,
    client_token: String,
    fallback: Option<&Profile>,
) {
    account.access_token = Some(resp.access_token.clone());
    account.client_token = Some(resp.client_token.clone().unwrap_or(client_token));
    if let Some(profile) = resp.selected_profile.as_ref().or(fallback) {
        account.name = profile.name.clone();
        account.uuid = store::dash_uuid(&profile.id);
    }
}

/// 把刷新结果写回账号。
fn apply_refresh(
    app: &AppHandle,
    account: &StoredAccount,
    resp: &AuthResponse,
    client_token: String,
    fallback: Option<&Profile>,
) -> Result<Account, String> {
    let mut updated = account.clone();
    merge_refresh(&mut updated, resp, client_token, fallback);
    if updated.server.is_none() {
        updated.server = Some(store::normalize_server(account.server.clone()));
    }
    store::upsert_and_save(app, updated.clone())?;
    Ok(updated.to_public())
}

/// 刷新皮肤站登录凭证（accessToken）。
#[tauri::command]
pub async fn yggdrasil_refresh(app: AppHandle, id: String) -> Result<Account, String> {
    if id.trim().is_empty() {
        return Err("缺少账号 id".to_string());
    }
    let account = store::find_account(&app, &id).ok_or_else(|| "账号不存在".to_string())?;
    if account.oauth.is_some() { return crate::skin_oauth::refresh(app, account).await; }
    let (resp, client_token) =
        refresh_tokens(&account, Some(selected_profile_json(&account))).await?;
    apply_refresh(&app, &account, &resp, client_token, None)
}

/// 列出账号下的全部角色（「切换角色」弹窗）。
///
/// 绑定过角色的令牌只能看到当前角色，因此这里用本机加密保存的密码重新
/// `authenticate` 拿完整 `availableProfiles`，并把「未绑定令牌」放进待选会话，
/// 供 [`yggdrasil_select_profile`] 绑定 —— 用户无需再输一次密码。
#[tauri::command]
pub async fn yggdrasil_profiles(app: AppHandle, id: String) -> Result<ProfileList, String> {
    if id.trim().is_empty() {
        return Err("缺少账号 id".to_string());
    }
    let account = store::find_account(&app, &id).ok_or_else(|| "账号不存在".to_string())?;
    if account.oauth.is_some() { return crate::skin_oauth::list_profiles(app, account).await; }
    // 已落盘账号的地址非法时拒绝重新认证，绝不把密码发往默认站点。
    let site = store::validate_stored_server(account.server.clone())?;

    let username = account
        .email
        .clone()
        .unwrap_or_default()
        .trim()
        .to_string();
    if username.is_empty() || !username.contains('@') {
        // LittleSkin 允许用角色名登录，但那种令牌只会绑定该角色，无法枚举其它角色。
        return Err("该账号不是用邮箱登录的，无法读取全部角色；请退出后用邮箱重新登录".to_string());
    }

    let sealed = account
        .password
        .as_ref()
        .ok_or_else(|| "该账号未保存登录凭据，请重新登录后再切换角色".to_string())?;
    let password = crate::vault::open(&app, sealed.as_str())
        .map_err(|_| "本地登录凭据已失效，请重新登录后再切换角色".to_string())?;

    let (resp, requested_client) = authenticate(&site, &username, &password).await?;

    let mut profiles = resp.available_profiles.clone();
    if profiles.is_empty() {
        if let Some(selected) = &resp.selected_profile {
            profiles.push(selected.clone());
        }
    }
    if profiles.is_empty() {
        return Err("服务器未返回角色信息（该账号可能未创建角色）".to_string());
    }

    let current = if account.uuid.trim().is_empty() {
        None
    } else {
        Some(Profile {
            id: store::undash_uuid(&account.uuid),
            name: account.name.clone(),
        })
    };

    let session_id = format!("sess_{}", store::random_token());
    {
        let mut map = pending_lock();
        let now = Instant::now();
        purge_expired(&mut map, now);
        map.insert(
            session_id.clone(),
            PendingLogin {
                created_at: now,
                access_token: resp.access_token,
                client_token: resp.client_token.unwrap_or(requested_client),
                server: site,
                username,
                password: account.password.clone(),
                account_id: Some(account.id.clone()),
                current_profile: current,
                profiles: profiles.clone(),
            },
        );
    }
    Ok(ProfileList {
        session_id,
        profiles,
    })
}

/// 校验皮肤站凭证是否仍有效。
#[tauri::command]
pub async fn yggdrasil_validate(app: AppHandle, id: String) -> Result<bool, String> {
    if id.trim().is_empty() {
        return Err("缺少账号 id".to_string());
    }
    let account = store::find_account(&app, &id).ok_or_else(|| "账号不存在".to_string())?;
    let access_token = match account.access_token.clone() {
        Some(t) => t,
        None => return Ok(false),
    };
    let client_token = account
        .client_token
        .clone()
        .unwrap_or_else(store::random_token);
    // 校验路径同样不能回退默认站点，否则令牌会被发往非预期服务器。
    let site = store::validate_stored_server(account.server.clone())?;
    let auth_base = store::yggdrasil_base(&site);

    let body = serde_json::json!({
        "accessToken": access_token,
        "clientToken": client_token,
    });

    let url = store::join(&auth_base, "/authserver/validate");
    let (status, _value) = http::post_json(&url, &body, None).await?;
    // Yggdrasil 规范：校验成功返回 204（无内容），失败返回 403。
    Ok(status == 204 || status == 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, name: &str) -> Profile {
        Profile {
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    fn pending(age: Duration) -> PendingLogin {
        PendingLogin {
            created_at: Instant::now()
                .checked_sub(age)
                .unwrap_or_else(Instant::now),
            access_token: "access".to_string(),
            client_token: "client".to_string(),
            server: store::DEFAULT_SERVER.to_string(),
            username: "user@example.com".to_string(),
            password: None,
            account_id: None,
            current_profile: None,
            profiles: vec![profile("1", "A")],
        }
    }

    #[test]
    fn maps_invalid_credentials() {
        assert_eq!(
            yggdrasil_error_message(
                Some("ForbiddenOperationException"),
                Some("Invalid credentials. Invalid username or password."),
                403
            ),
            "用户名或密码错误"
        );
    }

    /// LittleSkin 实际返回的中英双语错误体（2026-11-13 实测）。
    #[test]
    fn maps_littleskin_bilingual_credentials_error() {
        assert_eq!(
            yggdrasil_error_message(
                Some("ForbiddenOperationException"),
                Some("用户名或密码错误。Incorrect username or password."),
                403
            ),
            "用户名或密码错误"
        );
    }

    #[test]
    fn maps_locked_account() {
        assert_eq!(
            yggdrasil_error_message(
                Some("ForbiddenOperationException"),
                Some("Account has been locked."),
                403
            ),
            "账号已被锁定或封禁"
        );
    }

    #[test]
    fn maps_token_expired() {
        assert_eq!(
            yggdrasil_error_message(
                Some("ForbiddenOperationException"),
                Some("Invalid token."),
                403
            ),
            "登录凭证已失效，请重新登录"
        );
    }

    /// LittleSkin 对「未绑定角色的令牌 + 不带 selectedProfile 的刷新」返回的中英双语错误。
    #[test]
    fn maps_unbound_profile_refresh_error() {
        assert_eq!(
            yggdrasil_error_message(
                Some("ForbiddenOperationException"),
                Some("访问令牌未绑定角色，请在刷新时指定角色。The access token has no profile assigned, please select a profile."),
                403
            ),
            "登录凭证尚未绑定角色，请重新登录并选择角色"
        );
    }

    /// 刷新结果必须能覆盖令牌，并在服务端未回 selectedProfile 时用请求角色回退。
    #[test]
    fn merge_refresh_binds_fallback_profile() {
        let mut account = StoredAccount {
            access_token: Some("old-access".into()),
            client_token: Some("old-client".into()),
            ..Default::default()
        };
        let resp = AuthResponse {
            access_token: "new-access".into(),
            client_token: None,
            selected_profile: None,
            available_profiles: vec![],
        };
        let fallback = profile("069a79f444e94726a5befca90e38aaf5", "Keyi");

        merge_refresh(&mut account, &resp, "new-client".into(), Some(&fallback));

        assert_eq!(account.access_token.as_deref(), Some("new-access"));
        assert_eq!(account.client_token.as_deref(), Some("new-client"));
        assert_eq!(account.name, "Keyi");
        assert_eq!(account.uuid, "069a79f4-44e9-4726-a5be-fca90e38aaf5");
    }

    #[test]
    fn falls_back_to_raw_message_then_status() {
        assert_eq!(
            yggdrasil_error_message(None, Some("自定义错误"), 500),
            "自定义错误"
        );
        assert_eq!(
            yggdrasil_error_message(None, None, 500),
            "认证失败（HTTP 500）"
        );
    }

    #[test]
    fn selected_profile_uses_undashed_uuid() {
        let acc = StoredAccount {
            name: "Keyi".into(),
            uuid: "069a79f4-44e9-4726-a5be-fca90e38aaf5".into(),
            ..Default::default()
        };
        let v = selected_profile_json(&acc);
        assert_eq!(
            v.get("id").and_then(|s| s.as_str()),
            Some("069a79f444e94726a5befca90e38aaf5")
        );
        assert_eq!(v.get("name").and_then(|s| s.as_str()), Some("Keyi"));
    }

    /// 绑定 / 取消回绑都必须把角色 UUID 转成无连字符形式（服务端只认这种）。
    #[test]
    fn profile_json_uses_undashed_uuid() {
        let selected = profile_json(&profile("069a79f4-44e9-4726-a5be-fca90e38aaf5", "Keyi"));
        assert_eq!(
            selected.get("id").and_then(|s| s.as_str()),
            Some("069a79f444e94726a5befca90e38aaf5")
        );
        assert_eq!(selected.get("name").and_then(|s| s.as_str()), Some("Keyi"));
    }

    /// 前后端线格式契约：判别字段必须是 `status`，变体与字段名必须与前端解析器一致。
    #[test]
    fn login_outcome_wire_contract() {
        let account = Account {
            id: "acc_1".into(),
            account_type: "littleskin".into(),
            name: "Keyi".into(),
            uuid: "069a79f4-44e9-4726-a5be-fca90e38aaf5".into(),
            email: Some("user@example.com".into()),
            server: Some(store::DEFAULT_SERVER.into()),
            avatar_url: None,
            auth_method: Some("classic".into()),
        };
        let value = serde_json::to_value(LoginOutcome::Account {
            account: account.clone(),
        })
        .unwrap();
        assert_eq!(value.get("status").and_then(|v| v.as_str()), Some("account"));
        let public = value.get("account").expect("必须内联 account 字段");
        assert_eq!(
            public.get("type").and_then(|v| v.as_str()),
            Some("littleskin")
        );
        assert_eq!(public.get("name").and_then(|v| v.as_str()), Some("Keyi"));
        assert_eq!(
            public.get("avatarUrl"),
            Some(&serde_json::Value::Null),
            "avatarUrl 字段必须存在（可为 null）"
        );
        // 敏感字段绝不回传
        assert!(public.get("accessToken").is_none());
        assert!(public.get("clientToken").is_none());

        let value = serde_json::to_value(LoginOutcome::Select {
            session_id: "sess_abc".into(),
            profiles: vec![profile("1", "A"), profile("2", "B")],
        })
        .unwrap();
        assert_eq!(value.get("status").and_then(|v| v.as_str()), Some("select"));
        assert_eq!(
            value.get("sessionId").and_then(|v| v.as_str()),
            Some("sess_abc")
        );
        let profiles = value
            .get("profiles")
            .and_then(|v| v.as_array())
            .expect("profiles 必须是数组");
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].get("id").and_then(|v| v.as_str()), Some("1"));
        assert_eq!(profiles[0].get("name").and_then(|v| v.as_str()), Some("A"));
        // 选择阶段绝不能带出任何 token
        assert!(value.get("accessToken").is_none());
        assert!(value.get("clientToken").is_none());
    }

    /// AuthResponse 含访问令牌，Debug 输出必须脱敏（日志 / panic 信息都可能带出）。
    #[test]
    fn auth_response_debug_redacts_tokens() {
        let resp = AuthResponse {
            access_token: "ACCESS-TOKEN-VALUE".into(),
            client_token: Some("CLIENT-TOKEN-VALUE".into()),
            selected_profile: Some(profile("1", "Keyi")),
            available_profiles: vec![profile("1", "Keyi")],
        };
        let debug = format!("{resp:?}");
        assert!(!debug.contains("ACCESS-TOKEN-VALUE"), "{debug}");
        assert!(!debug.contains("CLIENT-TOKEN-VALUE"), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");
        // 非敏感字段仍可读，便于排障。
        assert!(debug.contains("Keyi"), "{debug}");
    }

    /// `availableProfiles` / `selectedProfile` 的各种组合都要能解析。
    #[test]
    fn parses_auth_response_profile_shapes() {
        // 只有 selectedProfile（旧皮肤站）
        let resp: AuthResponse = serde_json::from_value(serde_json::json!({
            "accessToken": "a",
            "clientToken": "c",
            "selectedProfile": { "id": "1", "name": "A" }
        }))
        .unwrap();
        assert_eq!(resp.available_profiles.len(), 0);
        assert_eq!(resp.selected_profile.as_ref().map(|p| p.name.as_str()), Some("A"));
        assert_eq!(resp.client_token.as_deref(), Some("c"));

        // 只有 availableProfiles，且无 selectedProfile（多角色）
        let resp: AuthResponse = serde_json::from_value(serde_json::json!({
            "accessToken": "a",
            "availableProfiles": [
                { "id": "1", "name": "A" },
                { "id": "2", "name": "B" }
            ]
        }))
        .unwrap();
        assert!(resp.selected_profile.is_none());
        assert!(resp.client_token.is_none());
        assert_eq!(resp.available_profiles.len(), 2);

        // 两者都有（单角色）
        let resp: AuthResponse = serde_json::from_value(serde_json::json!({
            "accessToken": "a",
            "selectedProfile": { "id": "1", "name": "A" },
            "availableProfiles": [{ "id": "1", "name": "A" }]
        }))
        .unwrap();
        assert_eq!(resp.available_profiles.len(), 1);
        assert_eq!(resp.selected_profile.as_ref().map(|p| p.id.as_str()), Some("1"));
    }

    #[test]
    fn routes_single_and_multi_profile_accounts() {
        // 单角色（仅有 selectedProfile）→ 直接落库
        assert_eq!(
            decide_login_route(Some(profile("1", "A")), vec![]),
            LoginRoute::Direct(profile("1", "A"))
        );
        // 单角色（仅有 availableProfiles）→ 直接落库
        assert_eq!(
            decide_login_route(None, vec![profile("1", "A")]),
            LoginRoute::Direct(profile("1", "A"))
        );
        // 多角色 → 交给用户选择
        assert_eq!(
            decide_login_route(
                Some(profile("1", "A")),
                vec![profile("1", "A"), profile("2", "B")]
            ),
            LoginRoute::Select(vec![profile("1", "A"), profile("2", "B")])
        );
        // 无角色 → 明确报错
        assert_eq!(decide_login_route(None, vec![]), LoginRoute::NoProfile);
    }

    #[test]
    fn purges_expired_pending_sessions() {
        let now = Instant::now();
        let mut map = HashMap::new();
        map.insert("fresh".to_string(), pending(Duration::from_secs(60)));
        map.insert("stale".to_string(), pending(PENDING_TTL + Duration::from_secs(1)));

        purge_expired(&mut map, now);

        assert!(map.contains_key("fresh"), "未过期会话应保留");
        assert!(!map.contains_key("stale"), "过期会话应被清理");
    }
}
