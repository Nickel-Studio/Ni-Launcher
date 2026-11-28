//! 账号存储命令（持久化到 `app_config_dir/accounts.json`）。
//!
//! 所有命令均只回传**脱敏**后的 `Account`；accessToken 等敏感字段仅落盘。

use tauri::AppHandle;

use crate::store::{self, Account, StoredAccount};

/// 列出全部账号（脱敏）。
#[tauri::command]
pub fn accounts_list(app: AppHandle) -> Result<Vec<Account>, String> {
    Ok(store::load_accounts(&app).iter().map(|a| a.to_public()).collect())
}

/// 新增或更新账号（按 id 匹配）。返回更新后的账号列表（脱敏）。
///
/// 前端只传公共字段即可；若同 id 已存在且本次未提供敏感字段，则保留原敏感字段。
#[tauri::command]
pub fn accounts_upsert(app: AppHandle, account: StoredAccount) -> Result<Vec<Account>, String> {
    let _g = store::lock();
    let mut incoming = account;

    let mut list = store::load_accounts(&app);
    if let Some(existing) = list.iter().find(|a| a.id == incoming.id && !a.id.is_empty()) {
        // 保留既有的敏感字段（前端不会回传它们）。
        if incoming.access_token.is_none() {
            incoming.access_token = existing.access_token.clone();
        }
        if incoming.client_token.is_none() {
            incoming.client_token = existing.client_token.clone();
        }
        if incoming.refresh_token.is_none() {
            incoming.refresh_token = existing.refresh_token.clone();
        }
        if incoming.expires_at.is_none() {
            incoming.expires_at = existing.expires_at.clone();
        }
        // 密码密文只由登录 / 选角流程写入；前端表单不会携带它，不能在此丢失。
        if incoming.oauth.is_none() { incoming.oauth = existing.oauth.clone(); }
        if incoming.auth_api.is_none() { incoming.auth_api = existing.auth_api.clone(); }
        if incoming.password.is_none() {
            incoming.password = existing.password.clone();
        }
        if incoming.server.is_none() {
            incoming.server = existing.server.clone();
        }
        if incoming.email.is_none() {
            incoming.email = existing.email.clone();
        }
        if incoming.uuid.trim().is_empty() {
            incoming.uuid = existing.uuid.clone();
        }
    }

    let incoming = incoming.normalize();
    store::upsert_into(&mut list, incoming);
    store::save_accounts(&app, &list)?;
    Ok(list.iter().map(|a| a.to_public()).collect())
}

/// 删除账号。返回更新后的账号列表（脱敏）。
#[tauri::command]
pub fn accounts_remove(app: AppHandle, id: String) -> Result<Vec<Account>, String> {
    let _g = store::lock();
    let mut list = store::load_accounts(&app);
    list.retain(|a| a.id != id);
    store::save_accounts(&app, &list)?;
    Ok(list.iter().map(|a| a.to_public()).collect())
}

/// 设置当前激活账号（持久化到 `active_account.json`）。
#[tauri::command]
pub fn accounts_set_active(app: AppHandle, id: String) -> Result<(), String> {
    let _g = store::lock();
    store::set_active(&app, &id)
}
