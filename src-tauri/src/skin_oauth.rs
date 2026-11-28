//! Authorization-code OAuth for Blessing Skin / Yggdrasil Connect.
use std::{collections::HashMap, sync::{Arc, LazyLock, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use minecraft_java_rs_core::net::client::reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use crate::{skin_auth::{self, ClientConfig, Site}, store::{self, Account, SealedSecret, StoredAccount}, yggdrasil::{LoginOutcome, Profile, ProfileList}};

const TTL: Duration = Duration::from_secs(600);
#[derive(Clone, Serialize, Deserialize)]
struct Tokens { site: Site, client: ClientConfig, access: String, refresh: String, expires: u64 }
struct Authorization {
    created: Instant, site: Site, client: ClientConfig, root: Option<String>, version: Option<String>,
    cancelled: Arc<AtomicBool>, sender: Option<tokio::sync::oneshot::Sender<Result<String, String>>>,
    receiver: Option<tokio::sync::oneshot::Receiver<Result<String, String>>>,
}
struct Selection { created: Instant, tokens: Tokens, profiles: Vec<Profile>, account: Option<StoredAccount>, cancelled: Arc<AtomicBool> }
static AUTH: LazyLock<Mutex<HashMap<String, Authorization>>> = LazyLock::new(Default::default);
static SELECTIONS: LazyLock<Mutex<HashMap<String, Selection>>> = LazyLock::new(Default::default);
// Retain cancellation flags during network work, including single-profile completion.
static FLAGS: LazyLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = LazyLock::new(Default::default);
static REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn random_state() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| "无法创建授权会话")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn live(flag: &AtomicBool) -> Result<(), String> { if flag.load(Ordering::SeqCst) { Err("授权已取消".into()) } else { Ok(()) } }
pub fn cancel_all() {
    let mut flags = FLAGS.lock().unwrap_or_else(|e| e.into_inner());
    for flag in flags.values() { flag.store(true, Ordering::SeqCst); }
    flags.clear();
    AUTH.lock().unwrap_or_else(|e| e.into_inner()).clear();
    SELECTIONS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}
#[tauri::command]
pub fn skin_oauth_cancel(session_id: Option<String>) {
    let Some(id) = session_id else { cancel_all(); return; };
    let mut flags = FLAGS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(flag) = flags.remove(&id) { flag.store(true, Ordering::SeqCst); }
    AUTH.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    SELECTIONS.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
}
fn endpoint(site: &Site, path: &str) -> String { store::join(&store::site_base(&site.server), path) }
async fn request(url: &str, body: Option<Value>, bearer: Option<&str>, form: bool) -> Result<Value, String> {
    let client = skin_auth::client()?;
    let mut req = if body.is_some() { client.post(url) } else { client.get(url) }.header("Accept", "application/json");
    if let Some(token) = bearer { req = req.bearer_auth(token); }
    if let Some(body) = body {
        req = if form {
            let fields: Vec<(&str, &str)> = body.as_object().ok_or("OAuth 参数无效")?.iter().filter_map(|(k,v)| v.as_str().map(|v|(k.as_str(),v))).collect();
            req.header("Content-Type", "application/x-www-form-urlencoded").body(crate::http::encode_form(&fields))
        } else { req.json(&body) };
    }
    let response = req.send().await.map_err(|_| "无法连接皮肤站认证服务")?;
    let status = response.status();
    let value = skin_auth::response_json(response).await?;
    if !status.is_success() {
        // Only protocol error identifiers; never echo a response containing credentials.
        let kind = value["error"].as_str().filter(|s| s.len() < 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')).unwrap_or("authentication_failed");
        return Err(format!("皮肤站认证失败（{}，{}）", status.as_u16(), kind));
    }
    Ok(value)
}
fn token_response(value: &Value, site: Site, client: ClientConfig) -> Result<Tokens, String> {
    let access = value["access_token"].as_str().filter(|s| !s.is_empty()).ok_or("未收到 OAuth 访问令牌")?.to_owned();
    let refresh = value["refresh_token"].as_str().filter(|s| !s.is_empty()).ok_or("未收到 OAuth 刷新令牌")?.to_owned();
    Ok(Tokens { site, client, access, refresh, expires: store::now_secs().saturating_add(value["expires_in"].as_u64().unwrap_or(3600)) })
}
fn seal(app: &AppHandle, tokens: &Tokens) -> Result<SealedSecret, String> {
    Ok(SealedSecret::new(crate::vault::seal(app, &serde_json::to_string(tokens).map_err(|_| "无法保存授权")?)?))
}
async fn account_tokens(app: &AppHandle, account: &mut StoredAccount) -> Result<Tokens, String> {
    let sealed = account.oauth.as_ref().ok_or("该账号不是 OAuth 账号")?;
    let mut tokens: Tokens = serde_json::from_str(&crate::vault::open(app, sealed.as_str())?).map_err(|_| "授权数据不可用，请重新登录")?;
    if !skin_auth::same_site(account.server.as_deref().unwrap_or(""), &tokens.site.server) { return Err("账号认证站点不一致".into()); }
    if tokens.expires <= store::now_secs() + 60 {
        let value = request(&endpoint(&tokens.site, "/oauth/token"), Some(json!({"grant_type":"refresh_token", "refresh_token":tokens.refresh, "client_id":tokens.client.client_id, "client_secret":tokens.client.client_secret})), None, true).await?;
        tokens = token_response(&value, tokens.site, tokens.client)?;
        account.oauth = Some(seal(app, &tokens)?);
        // Persist rotated OAuth tokens before the next request can fail.
        store::upsert_and_save(app, account.clone())?;
    }
    Ok(tokens)
}
async fn profiles(tokens: &Tokens) -> Result<Vec<Profile>, String> {
    let value = request(&store::join(&tokens.site.api, "/sessionserver/session/minecraft/profile"), None, Some(&tokens.access), false).await?;
    let profiles: Vec<Profile> = serde_json::from_value(value).map_err(|_| "皮肤站未提供兼容的角色列表")?;
    let profiles: Vec<_> = profiles.into_iter().filter(|p| !p.name.is_empty() && store::undash_uuid(&p.id).len() == 32 && store::undash_uuid(&p.id).bytes().all(|b| b.is_ascii_hexdigit())).collect();
    if profiles.is_empty() { return Err("该皮肤站账号尚未创建角色".into()); }
    Ok(profiles)
}
async fn issue(app: &AppHandle, tokens: &Tokens, profile: &Profile, existing: Option<StoredAccount>, flag: &AtomicBool) -> Result<Account, String> {
    live(flag)?;
    let value = request(&store::join(&tokens.site.api, "/authserver/oauth"), Some(json!({"uuid":store::undash_uuid(&profile.id)})), Some(&tokens.access), false).await?;
    live(flag)?;
    let access = value["accessToken"].as_str().filter(|s| !s.is_empty()).ok_or("皮肤站未签发 Minecraft 令牌")?;
    let client = value["clientToken"].as_str().filter(|s| !s.is_empty()).ok_or("皮肤站未签发 Minecraft clientToken")?;
    if value["selectedProfile"]["id"].as_str().map(store::undash_uuid).as_deref() != Some(store::undash_uuid(&profile.id).as_str()) { return Err("皮肤站返回了不匹配的角色".into()); }
    let mut account = existing.or_else(|| store::load_accounts(app).into_iter().find(|a| a.account_type == "littleskin" && skin_auth::same_site(a.server.as_deref().unwrap_or(""), &tokens.site.server) && store::undash_uuid(&a.uuid) == store::undash_uuid(&profile.id))).unwrap_or_else(|| StoredAccount { id: store::new_id(), ..Default::default() });
    account.account_type = "littleskin".into();
    account.name = profile.name.clone(); account.uuid = store::dash_uuid(&profile.id);
    account.server = Some(tokens.site.server.clone()); account.auth_api = Some(tokens.site.api.clone());
    account.access_token = Some(access.into()); account.client_token = Some(client.into());
    account.password = None; account.oauth = Some(seal(app, tokens)?);
    account.expires_at = None;
    account = account.normalize();
    let _commit_guard = FLAGS.lock().unwrap_or_else(|e| e.into_inner());
    live(flag)?;
    store::upsert_and_save(app, account.clone())?;
    Ok(account.to_public())
}
#[tauri::command]
pub async fn skin_oauth_begin(app: AppHandle, root: Option<String>, version_id: Option<String>) -> Result<String, String> {
    let (id, url) = prepare(&app, root, version_id).await?;
    diagnostic("session_prepared");
    let result = (|| {
        #[cfg(any(windows, target_os = "linux"))] {
            use tauri_plugin_deep_link::DeepLinkExt;
            app.deep_link().register_all().map_err(|_| "无法注册授权回调协议")?;
        }
        let flags = FLAGS.lock().unwrap_or_else(|e| e.into_inner());
        live(flags.get(&id).ok_or("授权已取消")?)?;
        app.opener().open_url(url.as_str(), None::<&str>).map_err(|_| "无法打开授权页面".to_owned())
    })();
    if let Err(error) = result {
        diagnostic("browser_open_failed");
        skin_oauth_cancel(Some(id));
        return Err(error);
    }
    diagnostic("browser_opened");
    Ok(id)
}
async fn prepare(app: &AppHandle, root: Option<String>, version_id: Option<String>) -> Result<(String, Url), String> {
    cancel_all();
    let id = format!("oauth_{}", random_state()?);
    let flag = Arc::new(AtomicBool::new(false));
    FLAGS.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), flag.clone());
    let result = async {
        let mut context = skin_auth::skin_auth_context(app.clone(), root.clone(), version_id.clone()).await?;
        if context.site.api.is_empty() || (context.exclusive && context.site.server.is_empty()) { return Err("请先配置实例的皮肤站地址".into()); }
        context.site = skin_auth::skin_site_describe(context.site.server.clone(), Some(context.site.name.clone())).await;
        let config = skin_auth::resolve_client(app, &context.site.server, root.clone(), version_id.clone())?;
        let mut url = Url::parse(&endpoint(&context.site, "/oauth/authorize")).map_err(|_| "皮肤站地址无效")?;
        url.query_pairs_mut().extend_pairs([("client_id",config.client_id.as_str()),("redirect_uri",skin_auth::CALLBACK),("response_type","code"),("scope","Yggdrasil.PlayerProfiles.Read Yggdrasil.MinecraftToken.Create"),("state",id.as_str())]);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let _guard = FLAGS.lock().unwrap_or_else(|e| e.into_inner());
        live(&flag)?;
        AUTH.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), Authorization { created: Instant::now(), site: context.site, client: config, root, version: version_id, cancelled: flag, sender: Some(sender), receiver: Some(receiver) });
        Ok((id.clone(), url))
    }.await;
    if result.is_err() { skin_oauth_cancel(Some(id)); }
    result
}
fn authorization_error(kind: &str) -> &'static str {
    match kind {
        "access_denied" => "你已在网页中取消授权",
        "invalid_client" => "OAuth 应用未注册或未获准使用",
        "invalid_request" => "OAuth 授权请求参数无效",
        "invalid_scope" => "OAuth 授权权限无效",
        "server_error" | "temporarily_unavailable" => "皮肤站授权服务暂不可用，请稍后重试",
        _ => "皮肤站授权失败，请重试",
    }
}
fn callback_parts(raw: &str) -> Result<(Option<String>, Result<String, String>), String> {
    let url = Url::parse(raw).map_err(|_| "授权回调无效")?;
    let expected = Url::parse(skin_auth::CALLBACK).unwrap();
    // Windows IUri adds '/' to an authority-only URI before protocol activation.
    // Accept only that root-path equivalence, never an arbitrary trailing slash.
    let same_path = url.path() == expected.path()
        || (expected.path().is_empty() && url.path() == "/");
    if url.scheme() != expected.scheme() || url.host_str() != expected.host_str() || !same_path || url.port() != expected.port() || !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() { return Err("授权回调不匹配".into()); }
    let mut pairs = HashMap::new();
    for (key, value) in url.query_pairs() { if pairs.insert(key.into_owned(), value.into_owned()).is_some() { return Err("授权回调包含重复参数".into()); } }
    let state = pairs.remove("state");
    if state.as_ref().is_some_and(|s| s.is_empty()) { return Err("授权回调 state 无效".into()); }
    if let Some(error) = pairs.remove("error") {
        if error.is_empty() || pairs.contains_key("code") { return Err("授权回调无效".into()); }
        // Never echo error_description: providers may include credentials or callback URLs.
        return Ok((state, Err(authorization_error(&error).into())));
    }
    let state = state.ok_or("授权回调缺少 state")?;
    let code = pairs.remove("code").filter(|s| !s.is_empty() && s.len() < 16384).ok_or("授权回调缺少代码")?;
    Ok((Some(state), Ok(code)))
}
pub(crate) fn diagnostic(stage: &'static str) {
    if cfg!(debug_assertions) || crate::diagnostics_enabled(std::env::var_os("NICKEL_DIAG").as_deref()) {
        use std::io::Write;
        // Callers supply fixed stage labels only, never URLs, IDs or error payloads.
        let _ = writeln!(std::io::stderr().lock(), "[skin-oauth] pid={} {stage}", std::process::id());
    }
}
pub fn handle_callback(raw: &str) {
    diagnostic("callback_received");
    let Ok((state, code)) = callback_parts(raw) else {
        diagnostic("callback_rejected_invalid_parameters");
        return;
    };
    let mut sessions = AUTH.lock().unwrap_or_else(|e| e.into_inner());
    let waiting = |session: &Authorization| session.created.elapsed() < TTL && !session.cancelled.load(Ordering::SeqCst) && session.sender.is_some();
    let session = if let Some(state) = state {
        sessions.get_mut(&state).filter(|session| waiting(session))
    } else {
        // LittleSkin documents error redirects without state. Only fail the sole pending
        // request; a successful callback must still match its cryptographic state exactly.
        if code.is_ok() { return; }
        let mut pending = sessions.values_mut().filter(|session| waiting(session));
        let first = pending.next();
        if pending.next().is_some() {
            diagnostic("callback_rejected_ambiguous_session");
            return;
        }
        first
    };
    if let Some(session) = session {
        if let Some(sender) = session.sender.take() {
            let stage = if code.is_ok() { "callback_accepted_code" } else { "callback_accepted_error" };
            diagnostic(if sender.send(code).is_ok() { stage } else { "callback_receiver_closed" });
        }
    } else {
        diagnostic("callback_ignored_no_pending_session");
    }
}
#[tauri::command]
pub async fn skin_oauth_finish(app: AppHandle, session_id: String) -> Result<LoginOutcome, String> {
    let (receiver, created) = {
        let mut map = AUTH.lock().unwrap_or_else(|e| e.into_inner());
        let session = map.get_mut(&session_id).ok_or("授权会话已取消")?;
        (session.receiver.take().ok_or("授权会话已使用")?, session.created)
    };
    diagnostic("waiting_for_callback");
    let code = match tokio::time::timeout(TTL.saturating_sub(created.elapsed()), receiver).await {
        Ok(Ok(Ok(code))) => code,
        result => {
            diagnostic("authorization_rejected_cancelled_or_timed_out");
            skin_oauth_cancel(Some(session_id));
            return Err(match result { Ok(Ok(Err(error))) => error, _ => "授权已取消或超时".into() });
        }
    };
    diagnostic("callback_consumed");
    let session = AUTH.lock().unwrap_or_else(|e| e.into_inner()).remove(&session_id).ok_or("授权会话已取消")?;
    let result = async {
        live(&session.cancelled)?;
        let (_, current, method, _) = skin_auth::policy(&app, session.root.as_deref(), session.version.as_deref())?;
        if !skin_auth::same_site(&current, &session.site.server) || method != "oauth2" || skin_auth::resolve_client(&app, &current, session.root.clone(), session.version.clone())? != session.client { return Err("认证配置已改变，请重新登录".into()); }
        diagnostic("token_exchange_started");
        let value = request(&endpoint(&session.site, "/oauth/token"), Some(json!({"grant_type":"authorization_code", "client_id":session.client.client_id, "client_secret":session.client.client_secret, "redirect_uri":skin_auth::CALLBACK, "code":code})), None, true).await?;
        live(&session.cancelled)?;
        let tokens = token_response(&value, session.site, session.client)?;
        diagnostic("profiles_requested");
        let items = profiles(&tokens).await?;
        live(&session.cancelled)?;
        if items.len() == 1 {
            let account = issue(&app, &tokens, &items[0], None, &session.cancelled).await?;
            Ok(LoginOutcome::Account { account })
        } else {
            SELECTIONS.lock().unwrap_or_else(|e| e.into_inner()).insert(session_id.clone(), Selection { created: Instant::now(), tokens, profiles: items.clone(), account: None, cancelled: session.cancelled });
            Ok(LoginOutcome::Select { session_id: session_id.clone(), profiles: items })
        }
    }.await;
    diagnostic(match &result {
        Ok(LoginOutcome::Account { .. }) => "account_ready",
        Ok(LoginOutcome::Select { .. }) => "profile_selection_required",
        Err(_) => "login_failed_after_callback",
    });
    if !matches!(result, Ok(LoginOutcome::Select { .. })) { FLAGS.lock().unwrap_or_else(|e| e.into_inner()).remove(&session_id); }
    result
}
pub async fn select(app: AppHandle, session_id: String, profile_id: String) -> Result<Account, String> {
    let selection = {
        let mut map = SELECTIONS.lock().unwrap_or_else(|e| e.into_inner());
        let item = map.get(&session_id).ok_or("选角会话已过期")?;
        if item.created.elapsed() >= TTL || !item.profiles.iter().any(|p| p.id == profile_id) { return Err("选角会话无效，请重新登录".into()); }
        map.remove(&session_id).unwrap()
    };
    let profile = selection.profiles.iter().find(|p| p.id == profile_id).unwrap();
    let result = issue(&app, &selection.tokens, profile, selection.account.clone(), &selection.cancelled).await;
    if result.is_err() && live(&selection.cancelled).is_ok() { SELECTIONS.lock().unwrap_or_else(|e| e.into_inner()).insert(session_id, selection); }
    else { FLAGS.lock().unwrap_or_else(|e| e.into_inner()).remove(&session_id); }
    result
}
pub async fn list_profiles(app: AppHandle, mut account: StoredAccount) -> Result<ProfileList, String> {
    let id = format!("oauth_{}", random_state()?);
    let flag = Arc::new(AtomicBool::new(false));
    FLAGS.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), flag.clone());
    let result = async {
        let _guard = REFRESH_LOCK.lock().await;
        live(&flag)?;
        account = store::find_account(&app, &account.id).ok_or("账号不存在")?;
        let tokens = account_tokens(&app, &mut account).await?;
        let items = profiles(&tokens).await?;
        let _commit = FLAGS.lock().unwrap_or_else(|e| e.into_inner());
        live(&flag)?;
        SELECTIONS.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), Selection { created: Instant::now(), tokens, profiles: items.clone(), account: Some(account), cancelled: flag });
        Ok(ProfileList { session_id: id.clone(), profiles: items })
    }.await;
    if result.is_err() { skin_oauth_cancel(Some(id)); }
    result
}
pub async fn refresh(app: AppHandle, account: StoredAccount) -> Result<Account, String> {
    let _guard = REFRESH_LOCK.lock().await;
    let mut account = store::find_account(&app, &account.id).ok_or("账号不存在")?;
    let tokens = account_tokens(&app, &mut account).await?;
    let profile = Profile { id: store::undash_uuid(&account.uuid), name: account.name.clone() };
    issue(&app, &tokens, &profile, Some(account), &AtomicBool::new(false)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn callbacks_accept_windows_root_path_without_relaxing_target_or_state() {
        for root in ["nickel://auth", "nickel://auth/"] {
            let (state, code) = callback_parts(&format!("{root}?state=x&code=y")).unwrap();
            assert_eq!(state.as_deref(), Some("x"));
            assert_eq!(code.unwrap(), "y");
            assert!(callback_parts(&format!("{root}?code=y")).is_err());
            assert!(callback_parts(&format!("{root}?state=&code=y")).is_err());
            assert!(callback_parts(&format!("{root}?state=x&state=y&code=z")).is_err());
            assert!(callback_parts(&format!("{root}?state=x&code=y&code=z")).is_err());
            assert!(callback_parts(&format!("{root}?state=x&code=y#fragment")).is_err());
        }
        for root in ["nickel://other", "nickel://auth/other", "nickel://auth//", "nickel://auth/%2F", "nickel://auth:80/", "nickel://user@auth/", "nickel://auth.evil/", "https://auth/"] {
            assert!(callback_parts(&format!("{root}?state=x&code=y")).is_err());
        }
        assert!(callback_parts("nickel://auth/?state=x&error=access_denied").unwrap().1.is_err());
        assert!(callback_parts("nickel://auth/?error=access_denied").unwrap().0.is_none());
    }
    #[test] fn rejected_authorization_is_not_a_code() { assert!(callback_parts("nickel://auth?state=x&error=access_denied").unwrap().1.is_err()); }
    #[test] fn error_callbacks_allow_missing_state_but_never_codes_or_untrusted_descriptions() {
        let (state, result) = callback_parts("nickel://auth?error=access_denied&error_description=secret-must-not-leak").unwrap();
        assert!(state.is_none());
        assert_eq!(result.unwrap_err(), "你已在网页中取消授权");
        assert_eq!(callback_parts("nickel://auth?state=x&error=invalid_client").unwrap().1.unwrap_err(), "OAuth 应用未注册或未获准使用");
        for raw in ["nickel://auth?error=access_denied&code=x", "nickel://auth?error=access_denied&state=", "nickel://auth?error=", "nickel://auth?error=access_denied&error=server_error", "nickel://other?error=access_denied", "nickel://auth?code=x"] {
            assert!(callback_parts(raw).is_err());
        }
    }
    #[test] fn random_states_are_unique_and_crypto_sized() { let a=random_state().unwrap(); assert_eq!(a.len(),64); assert_ne!(a,random_state().unwrap()); }
}

#[cfg(all(test, windows))]
#[path = "skin_oauth_tests.rs"]
mod native_tests;
