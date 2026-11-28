//! 皮肤原图从 Yggdrasil 档案的 `textures.SKIN.url` 获取；旧账号缺少 UUID 时，
//! 先通过 `/api/profiles/minecraft` 按角色名查询，再回退站点的渲染头像。
//! LittleSkin 使用档案纹理接口，不支持 `/api/skin/{name}`。
//! 渲染头像可能是 WebP，且挂在站点根；API 根与站点子目录须分别保留。
//! 皮肤上传由站点角色页处理，本模块只获取图片。

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::Serialize;

use crate::http;
use crate::store;

/// 一张皮肤图：`kind` 决定前端是否还需要裁头部。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkinPng {
    /// 图片字节的 base64（不含 `data:` 前缀）。
    pub data_base64: String,
    /// 实际 MIME：皮肤站的渲染头像可能是 `image/webp`。
    pub mime: String,
    /// `skin` = 完整皮肤（需裁头部）；`avatar` = 已渲染头像（直接用）。
    pub kind: String,
}

/// 已取到的图片 + 用途标记。
struct FetchedSkin {
    bytes: Vec<u8>,
    mime: String,
    kind: &'static str,
}

/// PNG 魔数校验。
fn is_png(bytes: &[u8]) -> bool {
    bytes.len() > 8 && bytes[0..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
}

/// 按魔数判断图片类型（响应缺少 Content-Type 时兜底）。
fn sniff_mime(bytes: &[u8]) -> &'static str {
    let webp = bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP";
    if webp { "image/webp" } else { "image/png" }
}


/// `textures` 属性是 base64 编码的 JSON；缺失或损坏时交给调用方回退。
fn extract_texture_url(profile: &serde_json::Value) -> Option<String> {
    let props = profile.get("properties")?.as_array()?;
    for p in props {
        if p.get("name").and_then(|v| v.as_str()) != Some("textures") {
            continue;
        }
        let value = p.get("value").and_then(|v| v.as_str())?;
        let decoded = STANDARD.decode(value).ok()?;
        let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
        if let Some(url) = json.pointer("/textures/SKIN/url").and_then(|v| v.as_str()) {
            return Some(url.to_string());
        }
    }
    None
}

/// 按角色名换 UUID（Blessing Skin 的 Yggdrasil 扩展）。
async fn lookup_uuid(api: &str, name: &str) -> Option<String> {
    let url = store::join(api, "/api/profiles/minecraft");
    let body = serde_json::json!([name]);
    let (status, value) = http::post_json(&url, &body, None).await.ok()?;
    if status != 200 {
        return None;
    }
    value
        .as_array()?
        .first()?
        .get("id")?
        .as_str()
        .map(str::to_owned)
}

/// 读 Yggdrasil 档案里的皮肤纹理并下载原图。
async fn fetch_profile_skin(api: &str, uuid: &str) -> Option<Vec<u8>> {
    let url = format!(
        "{}/sessionserver/session/minecraft/profile/{}",
        api.trim_end_matches('/'),
        store::undash_uuid(uuid)
    );
    let (status, profile) = http::get_json(&url, None).await.ok()?;
    if status != 200 {
        return None;
    }
    let texture = extract_texture_url(&profile)?;
    let (status, _mime, bytes) = http::get_bytes(&texture).await.ok()?;
    if status == 200 && is_png(&bytes) {
        Some(bytes)
    } else {
        None
    }
}

/// 皮肤站渲染头像（已经是方块头像，不需要裁切）。
async fn fetch_site_avatar(site: &str, name: &str) -> Option<(Vec<u8>, String)> {
    let url = format!(
        "{}/avatar/player/{}?size=256",
        site.trim_end_matches('/'),
        http::urlencode(name)
    );
    let (status, mime, bytes) = http::get_bytes(&url).await.ok()?;
    if status != 200 || bytes.is_empty() {
        return None;
    }
    let mime = mime
        .filter(|value| value.starts_with("image/"))
        .unwrap_or_else(|| sniff_mime(&bytes).to_string());
    Some((bytes, mime))
}

async fn fetch_littleskin(site: &str, name: &str, uuid: Option<String>) -> Option<FetchedSkin> {
    let api = crate::skin_auth::skin_site_describe(site.to_owned(), None).await.api;
    let id = match uuid
        .map(|value| store::undash_uuid(&value))
        .filter(|value| !value.is_empty())
    {
        Some(id) => Some(id),
        None => lookup_uuid(&api, name).await,
    };
    if let Some(id) = id {
        if let Some(bytes) = fetch_profile_skin(&api, &id).await {
            return Some(FetchedSkin {
                bytes,
                mime: "image/png".to_string(),
                kind: "skin",
            });
        }
    }
    let site_root = store::site_base(site);
    fetch_site_avatar(&site_root, name)
        .await
        .map(|(bytes, mime)| FetchedSkin {
            bytes,
            mime,
            kind: "avatar",
        })
}

async fn fetch_mojang(name: &str) -> Option<Vec<u8>> {
    let lookup = format!("https://api.mojang.com/users/profiles/minecraft/{name}");
    let (status, value) = http::get_json(&lookup, None).await.ok()?;
    if status != 200 {
        return None;
    }
    let id = value.get("id").and_then(|v| v.as_str())?.to_string();

    let profile_url = format!("https://sessionserver.mojang.com/session/minecraft/profile/{id}");
    let (pstatus, profile) = http::get_json(&profile_url, None).await.ok()?;
    if pstatus != 200 {
        return None;
    }
    let texture_url = extract_texture_url(&profile)?;
    // Mojang 部分历史纹理仍是 http，换成 https 避免明文下载。
    let texture_url = texture_url.replace("http://", "https://");

    let (tstatus, _mime, bytes) = http::get_bytes(&texture_url).await.ok()?;
    if tstatus == 200 && is_png(&bytes) {
        Some(bytes)
    } else {
        None
    }
}

/// 未找到皮肤或请求失败时返回 `null`；非法站点地址在请求前返回错误。
#[tauri::command]
pub async fn skin_fetch(
    username: String,
    source: Option<String>,
    uuid: Option<String>,
    server: Option<String>,
) -> Result<Option<SkinPng>, String> {
    let name = username.trim();
    if name.is_empty() {
        return Ok(None);
    }
    let src = source.unwrap_or_default().trim().to_lowercase();
    let fetched = match src.as_str() {
        // 离线账号没有远程皮肤，不要拿名字去皮肤站碰运气。
        "offline" => None,
        "mojang" | "official" | "microsoft" => fetch_mojang(name).await.map(|bytes| FetchedSkin {
            bytes,
            mime: "image/png".to_string(),
            kind: "skin",
        }),
        _ => {
            // 皮肤站来源必须校验地址：非法输入不能静默回退默认站点，
            // 否则会把用户名 / UUID 发到用户未指定的服务器。
            let site = store::validate_server(server)?;
            fetch_littleskin(&site, name, uuid).await
        }
    };
    Ok(fetched.map(|skin| SkinPng {
        data_base64: STANDARD.encode(skin.bytes),
        mime: skin.mime,
        kind: skin.kind.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_magic_detection() {
        assert!(is_png(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00]));
        assert!(!is_png(b"not a png"));
        assert!(!is_png(&[0x89]));
    }

    /// 皮肤站头像接口返回 WebP；MIME 缺失时必须靠魔数识别，否则前端拿到错误类型。
    #[test]
    fn sniff_mime_recognizes_webp_and_png() {
        let webp = b"RIFF\x1c\x06\x00\x00WEBPVP8X";
        assert_eq!(sniff_mime(webp), "image/webp");
        assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\n\x00\x00"), "image/png");
    }

    #[test]
    fn extract_texture_url_parses_profile() {
        let inner = serde_json::json!({
            "textures": { "SKIN": { "url": "http://textures.minecraft.net/texture/abc" } }
        });
        let encoded = STANDARD.encode(serde_json::to_vec(&inner).unwrap());
        let profile = serde_json::json!({
            "id": "x",
            "properties": [ { "name": "textures", "value": encoded } ]
        });
        assert_eq!(
            extract_texture_url(&profile).as_deref(),
            Some("http://textures.minecraft.net/texture/abc")
        );
        assert!(extract_texture_url(&serde_json::json!({})).is_none());
    }

    /// 非法站点地址必须在发起任何网络请求前拒绝（前端会回退内置头像）。
    #[tokio::test]
    async fn rejects_invalid_littleskin_server_before_network() {
        let error = skin_fetch(
            "Steve".into(),
            Some("littleskin".into()),
            Some("uuid".into()),
            Some("https://user:pass@host".into()),
        )
        .await
        .unwrap_err();
        assert!(error.contains("皮肤站地址无效"), "{error}");
    }

    /// 缺少 UUID 时仍须获取原图或渲染头像；此测试通过 `--ignored` 访问 littleskin.cn。
    #[tokio::test]
    #[ignore = "需要访问 littleskin.cn"]
    async fn littleskin_returns_real_skin_or_avatar() {
        let skin = skin_fetch(
            "Keyi_1307".into(),
            Some("littleskin".into()),
            None,
            Some("https://littleskin.cn".into()),
        )
        .await
        .unwrap()
        .expect("应能拿到皮肤或渲染头像");
        assert!(skin.data_base64.len() > 100);
        assert!(skin.mime.starts_with("image/"));
        assert!(matches!(skin.kind.as_str(), "skin" | "avatar"));
        eprintln!(
            "[skin] kind={} mime={} base64_len={}",
            skin.kind,
            skin.mime,
            skin.data_base64.len()
        );
    }
}
