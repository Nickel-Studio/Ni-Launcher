//! Skin-site identity, effective login policy and per-site OAuth application settings.
use std::{collections::HashMap, sync::{Arc, LazyLock, Mutex}, time::{Duration, Instant}};
use minecraft_java_rs_core::net::client::reqwest::{Client, Url, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;
use crate::{store, vault};

pub const CALLBACK: &str = "nickel://auth";
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClientConfig { pub client_id: String, pub client_secret: String }
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Site { pub server: String, pub api: String, pub name: String, pub homepage: String, pub register: String }
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Context { pub exclusive: bool, pub method: String, pub site: Site, pub root: Option<String>, pub version_id: Option<String> }

type SiteCell = Arc<tokio::sync::OnceCell<Result<Site, ()>>>;
struct SiteEntry { created: Instant, cell: SiteCell, previous: Option<Site> }
static SITES: LazyLock<Mutex<HashMap<String, SiteEntry>>> = LazyLock::new(Default::default);
static CREDENTIAL_LOCK: Mutex<()> = Mutex::new(());

pub fn identity(raw: &str) -> Result<String, String> {
    let base = store::validate_server(Some(raw.to_owned()))?;
    let mut url = Url::parse(&base).map_err(|_| "皮肤站地址无效")?;
    url.set_fragment(None);
    Ok(url.to_string().trim_end_matches('/').to_owned())
}
pub fn same_site(a: &str, b: &str) -> bool { identity(a).ok().zip(identity(b).ok()).is_some_and(|(a,b)| a == b) }
pub fn client() -> Result<Client, String> {
    Client::builder().timeout(Duration::from_secs(20)).redirect(Policy::none()).build().map_err(|_| "无法建立认证连接".into())
}
pub async fn response_json(mut response: minecraft_java_rs_core::net::client::reqwest::Response) -> Result<Value, String> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "读取认证响应失败")? {
        if bytes.len() + chunk.len() > 1024 * 1024 { return Err("认证响应过大".into()); }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "服务器未返回兼容的认证数据".into())
}
fn display_name(raw: &str) -> String {
    raw.trim().chars().filter(|c| !c.is_control()).take(80).collect::<String>().trim().to_owned()
}
fn fallback(server: String, legacy: &str) -> Site {
    let legacy = display_name(legacy);
    let name = if legacy.is_empty() { Url::parse(&server).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_else(|| "皮肤站".into()) } else { legacy };
    Site { api: store::yggdrasil_base(&server), homepage: store::site_base(&server), register: String::new(), name, server }
}
fn resolved_name(mut site: Site, previous: Option<&Site>, legacy: &str) -> Site {
    if site.name.is_empty() {
        site.name = previous.filter(|old| same_site(&old.server, &site.server) && !old.name.is_empty()).map(|old| old.name.clone()).unwrap_or_else(|| fallback(site.server.clone(), legacy).name);
    }
    site
}
async fn discover(server: &str) -> Result<Site, String> {
    let mut site = fallback(server.to_owned(), "");
    let client = client()?;
    let mut url = Url::parse(server).map_err(|_| "皮肤站地址无效")?;
    let mut visited = std::collections::HashSet::new();
    for step in 0..6 {
        if !visited.insert(url.to_string()) { return Err("站点地址循环".into()); }
        let response = client.get(url.clone()).send().await.map_err(|_| "站点名称暂不可用")?;
        let target = response.headers().get("x-authlib-injector-api-location").or_else(|| if response.status().is_redirection() { response.headers().get("location") } else { None }).and_then(|h| h.to_str().ok()).map(str::to_owned);
        if let Some(target) = target {
            let next = url.join(&target).map_err(|_| "站点地址无效")?;
            if !matches!(next.scheme(), "https" | "http") || (url.scheme() == "https" && next.scheme() != "https") || !next.username().is_empty() || next.password().is_some() { return Err("站点地址无效".into()); }
            url = next;
            continue;
        }
        let value = response_json(response).await.unwrap_or(Value::Null);
        if value["meta"].is_object() {
            site.api = url.to_string().trim_end_matches('/').into();
            site.name = display_name(value["meta"]["serverName"].as_str().unwrap_or(""));
            for (key, out) in [("homepage", &mut site.homepage), ("register", &mut site.register)] {
                if let Some(link) = value["meta"]["links"][key].as_str().and_then(|s| url.join(s).ok()).filter(|u| matches!(u.scheme(), "http" | "https")) { *out = link.to_string(); }
            }
            return Ok(site);
        }
        if step == 0 { url = Url::parse(&site.api).map_err(|_| "站点地址无效")?; } else { break; }
    }
    Err("站点名称暂不可用".into())
}
#[tauri::command]
pub async fn skin_site_describe(server: String, legacy_name: Option<String>) -> Site {
    let Ok(server) = identity(&server) else { return Site { server, api: String::new(), name: "皮肤站".into(), homepage: String::new(), register: String::new() }; };
    let (cell, previous) = {
        let mut cache = SITES.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= 128 && !cache.contains_key(&server) {
            if let Some(oldest) = cache.iter().min_by_key(|(_, entry)| entry.created).map(|(key, _)| key.clone()) { cache.remove(&oldest); }
        }
        let entry = cache.entry(server.clone()).or_insert_with(|| SiteEntry { created: Instant::now(), cell: Arc::new(tokio::sync::OnceCell::new()), previous: None });
        let ttl = if matches!(entry.cell.get(), Some(Err(()))) { 30 } else { 1800 };
        if entry.created.elapsed().as_secs() >= ttl {
            if let Some(Ok(site)) = entry.cell.get().filter(|result| result.as_ref().is_ok_and(|site| !site.name.is_empty())) { entry.previous = Some(site.clone()); }
            entry.created = Instant::now();
            entry.cell = Arc::new(tokio::sync::OnceCell::new());
        }
        (entry.cell.clone(), entry.previous.clone())
    };
    match cell.get_or_init(|| async { tokio::time::timeout(Duration::from_secs(8), discover(&server)).await.ok().and_then(Result::ok).ok_or(()) }).await {
        Ok(site) => resolved_name(site.clone(), previous.as_ref(), legacy_name.as_deref().unwrap_or("")),
        Err(()) => previous.unwrap_or_else(|| fallback(server, legacy_name.as_deref().unwrap_or(""))),
    }
}
pub fn policy(app: &AppHandle, root: Option<&str>, version_id: Option<&str>) -> Result<(bool, String, String, String), String> {
    let global: Value = serde_json::from_str(&crate::settings_get(app.clone())?).unwrap_or_default();
    let game: Value = serde_json::from_str(&crate::game::game_settings_get(app.clone())?).unwrap_or_default();
    let local: Value = match (root, version_id) {
        (Some(root), Some(version)) => serde_json::from_str(&crate::version_settings::version_settings_get(root.into(), version.into())?).unwrap_or_default(),
        _ => Value::Null,
    };
    let setting = |key: &str| local[key].as_str().or_else(|| game[key].as_str()).unwrap_or("").to_owned();
    let exclusive = setting("serverLogin") == "thirdParty";
    let server = if exclusive { setting("authServer") } else { global["littleskinServer"].as_str().unwrap_or(store::DEFAULT_SERVER).to_owned() };
    let method = local["skinAuthMethod"].as_str().filter(|m| matches!(*m, "classic" | "oauth2")).or_else(|| global["skinAuthMethod"].as_str()).unwrap_or("classic");
    Ok((exclusive, server, if method == "oauth2" { "oauth2" } else { "classic" }.into(), setting("serverName")))
}
#[tauri::command]
pub async fn skin_auth_context(app: AppHandle, root: Option<String>, version_id: Option<String>) -> Result<Context, String> {
    let (exclusive, server, method, legacy) = policy(&app, root.as_deref(), version_id.as_deref())?;
    let site = if exclusive && server.trim().is_empty() { Site { server, api: String::new(), name: "皮肤站".into(), homepage: String::new(), register: String::new() } } else { fallback(identity(&server).unwrap_or(server), &legacy) };
    Ok(Context { exclusive, method, site, root, version_id })
}
fn scope(root: Option<&str>, version: Option<&str>, server: &str) -> Result<String, String> {
    Ok(serde_json::to_string(&(root.map(|r| r.replace('\\', "/").trim_end_matches('/').to_owned()), version, identity(server)?)).unwrap())
}
fn credentials_path(app: &AppHandle) -> Result<std::path::PathBuf, String> { Ok(crate::app_paths::config_dir(app)?.join("skin-oauth-clients.json")) }
fn read_clients(app: &AppHandle) -> Result<HashMap<String, String>, String> {
    match std::fs::read_to_string(credentials_path(app)?) {
        Ok(s) => serde_json::from_str(&s).map_err(|_| "OAuth 应用配置损坏".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(_) => Err("无法读取 OAuth 应用配置".into()),
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientView { pub client_id: String, pub client_secret: String, pub source: String, pub redirect_uri: &'static str }
fn builtin_site(server: &str) -> Result<bool, String> { Ok(identity(server)? == store::DEFAULT_SERVER) }
fn load_client(app: &AppHandle, server: &str, root: Option<&str>, version: Option<&str>) -> Result<(Option<ClientConfig>, &'static str), String> {
    // Built-in credentials never fall back to a saved custom override, even if unavailable.
    if builtin_site(server)? { return Ok((crate::secrets::littleskin_config(app), "builtin")); }
    let map = read_clients(app)?;
    for (key, source) in [(scope(root, version, server)?, "custom"), (scope(None, None, server)?, "global")] {
        if let Some(sealed) = map.get(&key) {
            let config = serde_json::from_str(&vault::open(app, sealed)?).map_err(|_| "OAuth 应用配置损坏")?;
            return Ok((Some(config), source));
        }
    }
    Ok((None, "empty"))
}
#[tauri::command]
pub fn skin_oauth_config_get(app: AppHandle, server: String, root: Option<String>, version_id: Option<String>) -> Result<ClientView, String> {
    // Do not decrypt or serialize built-in credentials across the IPC boundary.
    let (config, source) = if builtin_site(&server)? { (None, "builtin") } else { load_client(&app, &server, root.as_deref(), version_id.as_deref())? };
    let config = config.unwrap_or(ClientConfig { client_id: String::new(), client_secret: String::new() });
    Ok(ClientView { client_id: config.client_id, client_secret: config.client_secret, source: source.into(), redirect_uri: CALLBACK })
}
#[tauri::command]
pub fn skin_oauth_config_set(app: AppHandle, server: String, root: Option<String>, version_id: Option<String>, config: Option<ClientConfig>) -> Result<(), String> {
    if builtin_site(&server)? { return Err("LittleSkin 使用程序内置的 OAuth 应用，不允许自定义凭据".into()); }
    let _guard = CREDENTIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let key = scope(root.as_deref(), version_id.as_deref(), &server)?;
    let mut map = read_clients(&app)?;
    if let Some(config) = config {
        if config.client_id.len() > 8192 || config.client_secret.len() > 8192 { return Err("OAuth 应用配置过长".into()); }
        map.insert(key, vault::seal(&app, &serde_json::to_string(&config).map_err(|_| "OAuth 配置无效")?)?);
    } else { map.remove(&key); }
    crate::named_profiles::atomic_write(&credentials_path(&app)?, &serde_json::to_vec(&map).map_err(|_| "OAuth 配置无效")?)?;
    crate::skin_oauth::cancel_all();
    Ok(())
}
pub fn resolve_client(app: &AppHandle, server: &str, root: Option<String>, version_id: Option<String>) -> Result<ClientConfig, String> {
    let (config, source) = load_client(app, server, root.as_deref(), version_id.as_deref())?;
    let error = if source == "builtin" { "程序内置的 LittleSkin OAuth 配置不可用，请更新或重新安装启动器" } else { "请填写该皮肤站的 Client ID 和 Client Secret" };
    config.filter(|c| !c.client_id.trim().is_empty() && !c.client_secret.is_empty()).ok_or_else(|| error.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn builtin_protection_matches_only_canonical_littleskin() {
        for server in ["", "  ", "littleskin.cn", "https://LITTLESKIN.CN:443/", "https://littleskin.cn/api/yggdrasil/"] {
            assert!(builtin_site(server).unwrap());
        }
        for server in ["http://littleskin.cn", "https://littleskin.cn:8443", "https://littleskin.cn/other", "https://littleskin.cn.example"] {
            assert!(!builtin_site(server).unwrap());
        }
        for server in ["https://littleskin.cn?x=1", "https://user@littleskin.cn", "https://"] {
            assert!(builtin_site(server).is_err());
        }
    }
    #[test] fn site_identity_preserves_subdirectories_and_separates_sites() {
        assert!(same_site("https://littleskin.cn/api/yggdrasil/", "https://littleskin.cn"));
        assert!(!same_site("https://a.example/skin", "https://a.example/other"));
        assert!(!same_site("https://a.example", "https://b.example"));
        assert!(identity("https://user:pass@a.example").is_err());
    }
    #[test] fn scopes_separate_instance_and_site() {
        assert_ne!(scope(None,None,"https://a.example").unwrap(), scope(Some("D:/Games"),Some("v"),"https://a.example").unwrap());
        assert_ne!(scope(None,None,"https://a.example").unwrap(), scope(None,None,"https://b.example").unwrap());
    }
    #[test] fn missing_metadata_name_uses_same_site_cache_then_legacy_then_hostname() {
        let mut unnamed = fallback("https://a.example".into(), "");
        unnamed.name.clear();
        unnamed.api = "https://a.example/custom-api".into();
        let cached = fallback("https://a.example".into(), "Cached");
        let other = fallback("https://b.example".into(), "Other");
        let resolved = resolved_name(unnamed.clone(), Some(&cached), "Legacy");
        assert_eq!(resolved.name, "Cached");
        assert_eq!(resolved.api, unnamed.api);
        assert_eq!(resolved_name(unnamed.clone(), Some(&other), "Legacy").name, "Legacy");
        assert_eq!(resolved_name(unnamed.clone(), None, "").name, "a.example");
        assert_eq!(resolved_name(unnamed, None, "Second legacy").name, "Second legacy");
        assert_eq!(resolved_name(fallback("https://a.example".into(), "Metadata"), Some(&cached), "Legacy").name, "Metadata");
        assert_eq!(display_name(" \n\t "), "");
    }
}
