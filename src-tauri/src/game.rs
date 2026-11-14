//! 游戏 / 版本设置持久化（`app_config_dir/game.json`）。
//!
//! 与既有 `settings.json` 一致：后端对业务结构无感知，纯 JSON 文本透传，
//! 仅校验合法性 + 大小上限。

use tauri::AppHandle;

use crate::store;

/// game.json 大小上限（256KB）。
const GAME_MAX_BYTES: usize = 256 * 1024;

/// 读取游戏设置。文件不存在 / 为空 / 损坏时返回 `"{}"`。
#[tauri::command]
pub fn game_settings_get(app: AppHandle) -> Result<String, String> {
    let path = store::game_path(&app)?;
    match std::fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => {
            // 简单校验：非合法 JSON 则回退默认，避免把坏数据透传给前端。
            if serde_json::from_str::<serde_json::Value>(&s).is_ok() {
                Ok(s)
            } else {
                Ok("{}".to_string())
            }
        }
        _ => Ok("{}".to_string()),
    }
}

/// 写入游戏设置。校验 JSON 合法性 + 大小上限。
#[tauri::command]
pub fn game_settings_set(app: AppHandle, json: String) -> Result<(), String> {
    if json.len() > GAME_MAX_BYTES {
        return Err(format!(
            "游戏设置内容过大（{} 字节，上限 {} 字节）",
            json.len(),
            GAME_MAX_BYTES
        ));
    }
    serde_json::from_str::<serde_json::Value>(&json).map_err(|e| format!("非法 JSON: {e}"))?;
    let path = store::game_path(&app)?;
    std::fs::write(&path, json).map_err(|e| format!("写入游戏设置失败: {e}"))
}
