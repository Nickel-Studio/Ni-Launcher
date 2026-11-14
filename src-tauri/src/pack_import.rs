//! 本地标准整合包的原生选择与预览；确认名称后由 install 事务安装。
use minecraft_java_rs_core::net::transfer::CancellationToken;
use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPackPreview {
    pub path: String,
    pub name: String,
    pub minecraft_version: String,
    pub loader: Option<crate::modpacks::PackLoader>,
}

#[tauri::command]
pub async fn pack_import_pick(app: AppHandle) -> Result<Option<LocalPackPreview>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("导入整合包")
        .add_filter("Minecraft 整合包", &["mrpack", "zip"])
        .pick_file(move |path| {
            let _ = tx.send(path);
        });
    let Some(path) = rx.await.map_err(|_| "文件选择对话框已关闭，请重试")? else {
        return Ok(None);
    };
    let path = path
        .into_path()
        .map_err(|e| format!("请选择本地整合包：{e}"))?;
    tokio::task::spawn_blocking(move || {
        let (_, manifest) = crate::modpacks::local_manifest(&path, &CancellationToken::new())?;
        Ok(Some(LocalPackPreview {
            path: path.to_string_lossy().into_owned(),
            name: manifest.name,
            minecraft_version: manifest.minecraft_version,
            loader: manifest.loader,
        }))
    })
    .await
    .map_err(|e| format!("读取整合包失败：{e}"))?
}
