//! 微软正版登录（OAuth 2.0 Device Code Flow）。
//!
//! 流程：Device Code → 轮询 token（MSA token）→ XBL → XSTS → Minecraft 登录 →
//! `/minecraft/profile`（拿 uuid / name）。
//!
//! `clientId` 由前端传入，绝不硬编码；为空时返回明确错误。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tauri::AppHandle;

#[derive(Clone)]
struct LoginSession {
    client_id: String,
    client: http::MicrosoftClient,
    expires: Instant,
}
static SESSIONS: LazyLock<Mutex<HashMap<String, LoginSession>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const MAX_LOGIN_SESSIONS: usize = 32;

use crate::http;
use crate::store::{self, Account, StoredAccount};

const DEVICE_CODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const SCOPE: &str = "XboxLive.signin offline_access";

const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

const NO_CLIENT_ID: &str = "请在设置中填入 Azure 应用 Client ID";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodeStart {
    pub user_code: String,
    pub device_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollResult {
    pub status: String,
    pub account: Option<Account>,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    user_code: String,
    device_code: String,
    verification_uri: String,
    #[serde(default)]
    interval: Option<u64>,
    #[serde(default)]
    expires_in: u64,
}

/// XSTS 错误码 → 可读中文（纯逻辑，可测）。
pub fn xsts_error_message(code: i64) -> String {
    match code {
        2148916233 => "该微软账号尚未注册 Xbox 账号，请先前往 xbox.com 创建".to_string(),
        2148916238 => "该账号为未成年人账号，需由成年家庭成员邀请加入家庭组后登录".to_string(),
        2148916235 => "该账号所在国家 / 地区不支持 Xbox Live".to_string(),
        2148916227 => "该微软账号已被封禁".to_string(),
        2148916236 | 2148916237 => "该账号需要在 Xbox 官网完成验证后才能登录".to_string(),
        _ => format!("Xbox 验证失败（错误码 {code}）"),
    }
}

fn msa_error(value: &serde_json::Value, status: u16) -> String {
    if let Some(desc) = value.get("error_description").and_then(|v| v.as_str()) {
        return desc.to_string();
    }
    if let Some(err) = value.get("error").and_then(|v| v.as_str()) {
        return err.to_string();
    }
    format!("HTTP {status}")
}

fn require_client_id(client_id: &str) -> Result<&str, String> {
    let cid = client_id.trim();
    if cid.is_empty() {
        Err(NO_CLIENT_ID.to_string())
    } else {
        Ok(cid)
    }
}

/// 启动设备码登录：返回 userCode / deviceCode / 验证地址 / 轮询间隔 / 有效期。
#[tauri::command]
pub async fn msa_device_start(client_id: String) -> Result<DeviceCodeStart, String> {
    let cid = require_client_id(&client_id)?;
    let client =
        http::MicrosoftClient::new(crate::network_settings::snapshot().verify_microsoft_ssl)?;
    let (status, value) = client
        .post_form(DEVICE_CODE_URL, &[("client_id", cid), ("scope", SCOPE)])
        .await?;
    if status != 200 {
        return Err(format!("获取设备代码失败：{}", msa_error(&value, status)));
    }
    let response: DeviceCodeResponse =
        serde_json::from_value(value).map_err(|e| format!("解析设备代码响应失败: {e}"))?;
    let mut sessions = SESSIONS.lock().unwrap_or_else(|error| error.into_inner());
    sessions.retain(|_, session| session.expires > Instant::now());
    if sessions.len() >= MAX_LOGIN_SESSIONS {
        return Err("登录会话过多，请等待现有设备代码过期后重试".into());
    }
    sessions.insert(
        response.device_code.clone(),
        LoginSession {
            client_id: cid.into(),
            client,
            expires: Instant::now() + Duration::from_secs(response.expires_in.min(3600)),
        },
    );
    Ok(DeviceCodeStart {
        user_code: response.user_code,
        device_code: response.device_code,
        verification_uri: response.verification_uri,
        interval: response.interval.unwrap_or(5),
        expires_in: response.expires_in,
    })
}

/// 轮询设备码登录结果。
#[tauri::command]
pub async fn msa_device_poll(
    app: AppHandle,
    client_id: String,
    device_code: String,
) -> Result<PollResult, String> {
    let cid = require_client_id(&client_id)?;
    if device_code.trim().is_empty() {
        return Err("缺少 deviceCode".to_string());
    }

    let session = {
        let mut sessions = SESSIONS.lock().unwrap_or_else(|error| error.into_inner());
        sessions.retain(|_, session| session.expires > Instant::now());
        sessions
            .get(&device_code)
            .filter(|session| session.client_id == cid)
            .cloned()
            .ok_or("登录会话已过期，请重新开始登录")?
    };
    let client = &session.client;
    let (status, value) = client
        .post_form(
            TOKEN_URL,
            &[
                ("client_id", cid),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", &device_code),
            ],
        )
        .await?;

    if status != 200 {
        let err = value.get("error").and_then(|v| v.as_str()).unwrap_or("");
        return match err {
            "authorization_pending" | "slow_down" => Ok(PollResult {
                status: "pending".to_string(),
                account: None,
            }),
            "authorization_declined" => Err("用户已取消登录".to_string()),
            "expired_token" => Err("设备代码已过期，请重新开始登录".to_string()),
            "bad_verification_code" => Err("设备代码无效，请重新开始登录".to_string()),
            _ => Err(format!("登录失败：{}", msa_error(&value, status))),
        };
    }

    let msa_token = value
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "微软未返回访问令牌".to_string())?
        .to_string();
    let refresh_token = value
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let (id, name, mc_token) = full_microsoft_login(client, &msa_token).await?;
    SESSIONS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&device_code);

    let uuid = store::dash_uuid(&id);
    // 先读现有账号以复用同一微软账号的 id：每次登录都新建条目会积累重复账号。
    // `load_accounts` 内部不加锁，而 `upsert_and_save` 会加 `store::lock()` —— 这里
    // 不能在外层再锁一次，否则同一把非重入 Mutex 会自死锁。
    let existing = store::load_accounts(&app);
    let existing_id = existing_microsoft_account_id(&existing, &uuid);
    let email = existing_id
        .as_ref()
        .and_then(|id| existing.iter().find(|account| &account.id == id))
        .and_then(|account| account.email.clone());

    let account = StoredAccount {
        id: existing_id.unwrap_or_else(store::new_id),
        account_type: "microsoft".to_string(),
        name,
        uuid,
        email,
        server: None,
        avatar_url: None,
        access_token: Some(mc_token),
        client_token: None,
        refresh_token,
        expires_at: Some(store::now_secs() + 86_000),
        password: None,
        oauth: None,
        auth_api: None,
    }
    .normalize();

    store::upsert_and_save(&app, account.clone())?;
    Ok(PollResult {
        status: "success".to_string(),
        account: Some(account.to_public()),
    })
}

/// XBL 认证 → (uhs, xbl_token)。
async fn xbox_login(
    client: &http::MicrosoftClient,
    msa_token: &str,
) -> Result<(String, String), String> {
    let body = serde_json::json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": format!("d={msa_token}"),
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT",
    });
    let (status, value) = client.post_json(XBL_URL, &body).await?;
    if status != 200 {
        return Err(format!("Xbox Live 认证失败（HTTP {status}）"));
    }
    let token = value
        .get("Token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Xbox Live 未返回令牌".to_string())?
        .to_string();
    let uhs = value
        .pointer("/DisplayClaims/xui/0/uhs")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Xbox Live 未返回用户哈希".to_string())?
        .to_string();
    Ok((uhs, token))
}

/// XSTS 授权 → (uhs, xsts_token)。
async fn xsts_authorize(
    client: &http::MicrosoftClient,
    xbl_token: &str,
) -> Result<(String, String), String> {
    let body = serde_json::json!({
        "Properties": {
            "SandboxId": "RETAIL",
            "UserTokens": [xbl_token],
        },
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT",
    });
    let (status, value) = client.post_json(XSTS_URL, &body).await?;
    if status != 200 {
        let code = value.get("XErr").and_then(|v| v.as_i64()).unwrap_or(0);
        return Err(xsts_error_message(code));
    }
    let token = value
        .get("Token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "XSTS 未返回令牌".to_string())?
        .to_string();
    let uhs = value
        .pointer("/DisplayClaims/xui/0/uhs")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "XSTS 未返回用户哈希".to_string())?
        .to_string();
    Ok((uhs, token))
}

/// Minecraft 服务登录 → MC access_token。
async fn minecraft_login(
    client: &http::MicrosoftClient,
    uhs: &str,
    xsts_token: &str,
) -> Result<String, String> {
    let body = serde_json::json!({
        "identityToken": format!("XBL3.0 x={uhs};{xsts_token}"),
    });
    let (status, value) = client.post_json(MC_LOGIN_URL, &body).await?;
    if status != 200 {
        return Err(format!("Minecraft 登录失败（HTTP {status}）"));
    }
    value
        .get("access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "Minecraft 未返回访问令牌".to_string())
}

/// 拉取 Minecraft 档案 → (uuid, name)。
async fn minecraft_profile(
    client: &http::MicrosoftClient,
    mc_token: &str,
) -> Result<(String, String), String> {
    let (status, value) = client.get_json(MC_PROFILE_URL, mc_token).await?;
    if status == 404 {
        return Err("该微软账号未拥有 Minecraft: Java 版".to_string());
    }
    if status != 200 {
        return Err(format!("获取 Minecraft 档案失败（HTTP {status}）"));
    }
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Minecraft 档案缺少 id".to_string())?
        .to_string();
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Minecraft 档案缺少 name".to_string())?
        .to_string();
    Ok((id, name))
}

/// 完整微软链路 → (uuid, name, mc_access_token)。
async fn full_microsoft_login(
    client: &http::MicrosoftClient,
    msa_token: &str,
) -> Result<(String, String, String), String> {
    let (_xbl_uhs, xbl_token) = xbox_login(client, msa_token).await?;
    let (xsts_uhs, xsts_token) = xsts_authorize(client, &xbl_token).await?;
    let mc_token = minecraft_login(client, &xsts_uhs, &xsts_token).await?;
    let (id, name) = minecraft_profile(client, &mc_token).await?;
    Ok((id, name, mc_token))
}

/// UUID 归一化：去掉连字符并转小写。
///
/// Mojang 返回的 `id` 是无连字符的 32 位十六进制，但登录档案 / 旧数据可能带
/// 连字符或大小写差异；直接字符串比较会把同一账号当成两个。
fn normalize_uuid(raw: &str) -> String {
    store::undash_uuid(raw).to_ascii_lowercase()
}

/// 在已有账号中查找同一微软账号（按归一化 uuid 匹配），返回可复用的账号 id。
///
/// 只匹配 `account_type == "microsoft"`：离线 / 皮肤站账号即使 uuid 相同也不应
/// 被微软登录覆盖；空 id 的损坏条目不参与匹配，交给 `StoredAccount::normalize` 补全。
fn existing_microsoft_account_id(accounts: &[StoredAccount], uuid: &str) -> Option<String> {
    let target = normalize_uuid(uuid);
    accounts
        .iter()
        .find(|account| {
            account.account_type == "microsoft"
                && !account.id.trim().is_empty()
                && normalize_uuid(&account.uuid) == target
        })
        .map(|account| account.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xsts_codes_map_to_readable_chinese() {
        assert!(xsts_error_message(2148916233).contains("尚未注册 Xbox"));
        assert!(xsts_error_message(2148916238).contains("未成年人"));
        assert!(xsts_error_message(2148916235).contains("不支持 Xbox Live"));
        assert!(xsts_error_message(999).contains("999"));
    }

    #[test]
    fn empty_client_id_is_rejected() {
        assert_eq!(require_client_id(""), Err(NO_CLIENT_ID.to_string()));
        assert_eq!(require_client_id("   "), Err(NO_CLIENT_ID.to_string()));
        assert_eq!(require_client_id("abc"), Ok("abc"));
    }

    #[test]
    fn poll_result_serializes_status_and_account() {
        let pending = PollResult {
            status: "pending".into(),
            account: None,
        };
        let v = serde_json::to_value(&pending).unwrap();
        assert_eq!(v.get("status").and_then(|s| s.as_str()), Some("pending"));
        assert_eq!(v.get("account"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn device_code_struct_uses_camel_case() {
        let d = DeviceCodeStart {
            user_code: "ABCD".into(),
            device_code: "dev".into(),
            verification_uri: "https://microsoft.com/devicelogin".into(),
            interval: 5,
            expires_in: 900,
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v.get("userCode").and_then(|s| s.as_str()), Some("ABCD"));
        assert_eq!(v.get("deviceCode").and_then(|s| s.as_str()), Some("dev"));
        assert_eq!(
            v.get("verificationUri").and_then(|s| s.as_str()),
            Some("https://microsoft.com/devicelogin")
        );
        assert_eq!(v.get("expiresIn").and_then(|s| s.as_u64()), Some(900));
    }

    fn microsoft_account(id: &str, uuid: &str) -> StoredAccount {
        StoredAccount {
            id: id.into(),
            account_type: "microsoft".into(),
            uuid: uuid.into(),
            ..StoredAccount::default()
        }
    }

    #[test]
    fn microsoft_account_dedupes_by_normalized_uuid() {
        let accounts = vec![
            microsoft_account("acc_existing", "069A79F4-44E9-4726-A5BE-FCA90E38AFC0"),
            StoredAccount {
                id: "acc_offline".into(),
                account_type: "offline".into(),
                uuid: "069a79f444e94726a5befca90e38afc0".into(),
                ..StoredAccount::default()
            },
        ];

        // 大小写与连字符差异仍视为同一账号。
        assert_eq!(
            existing_microsoft_account_id(&accounts, "069a79f444e94726a5befca90e38afc0"),
            Some("acc_existing".into())
        );
        assert_eq!(
            existing_microsoft_account_id(&accounts, "069A79F4-44E9-4726-A5BE-FCA90E38AFC0"),
            Some("acc_existing".into())
        );
        // 不同 uuid → 新建；离线账号即使 uuid 相同也不参与匹配。
        assert_eq!(
            existing_microsoft_account_id(&accounts, "11111111111111111111111111111111"),
            None
        );
        assert_eq!(
            existing_microsoft_account_id(&accounts[1..], "069a79f444e94726a5befca90e38afc0"),
            None
        );
        assert_eq!(
            normalize_uuid("069A79F4-44E9-4726-A5BE-FCA90E38AFC0"),
            "069a79f444e94726a5befca90e38afc0"
        );
    }
}
