//! 本地图片 → dataURL。
//!
//! 背景图有两条加载路径：
//!   1. Tauri asset 协议（前端 `convertFileSrc`）—— 高效、零拷贝；
//!   2. 本命令兜底 —— 当 asset 协议不可用（未启用 / 路径不在 scope 内 / CSP 限制）时，
//!      由后端直接读文件并返回 `data:<mime>;base64,...`，保证背景图一定渲染得出来。
//!
//! 前端在 `useBackgroundSrc` 中先试 1，`:onerror` 后自动回退到 2。

use std::path::Path;

use base64::Engine as _;

/// 单文件大小上限（32MB）：误选超大图时不至于把内存打爆。
const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// 按扩展名推断 MIME 类型。
fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        Some("avif") => "image/avif",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// 读取本地图片并返回 dataURL。
///
/// 路径不存在 / 不是文件 / 超过上限 / 读取失败，都返回可读的中文错误
/// （前端会静默忽略，不影响主流程）。
#[tauri::command]
pub fn image_data_url(path: String) -> Result<String, String> {
    let p = Path::new(&path);
    if !p.is_file() {
        return Err(format!("文件不存在或不是文件：{path}"));
    }
    let meta = std::fs::metadata(p).map_err(|e| format!("无法读取文件信息：{e}"))?;
    if meta.len() > MAX_BYTES {
        return Err(format!(
            "图片过大（约 {} MB），请选择小于 32MB 的图片",
            meta.len() / 1024 / 1024
        ));
    }
    let bytes = std::fs::read(p).map_err(|e| format!("读取失败：{e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{};base64,{}", mime_for(p), b64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_by_extension_is_case_insensitive() {
        assert_eq!(mime_for(Path::new("a.PNG")), "image/png");
        assert_eq!(mime_for(Path::new("wallpaper.jpeg")), "image/jpeg");
        assert_eq!(mime_for(Path::new("a.webp")), "image/webp");
        assert_eq!(mime_for(Path::new("noext")), "application/octet-stream");
    }

    #[test]
    fn missing_path_returns_readable_error() {
        let err = image_data_url("Z:/nickel/definitely-missing.png".into());
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("文件不存在"));
    }
}
