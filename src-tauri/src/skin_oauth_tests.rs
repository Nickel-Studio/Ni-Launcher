//! Isolated native storage + loopback HTTP contract tests; no real login or protocol registration.
use super::*;
use std::{io::{Read, Write}, net::TcpListener, sync::atomic::AtomicUsize};


const FIRST: &str = "11111111111111111111111111111111";
const SECOND: &str = "22222222222222222222222222222222";

#[derive(Default)]
struct Fixture {
    requests: Mutex<Vec<(String, String, String)>>,
    count: AtomicUsize,
    fail_issue: AtomicBool,
    slow: AtomicBool,
    unnamed: AtomicBool,
    stop: AtomicBool,
}
struct Server { base: String, state: Arc<Fixture>, thread: Option<std::thread::JoinHandle<()>> }
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Fixture::default());
        state.count.store(1, Ordering::SeqCst);
        let shared = state.clone();
        let thread = std::thread::spawn(move || {
            while !shared.stop.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else { std::thread::sleep(Duration::from_millis(5)); continue; };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut bytes = Vec::new();
                let (headers, body) = loop {
                    let mut chunk = [0; 4096];
                    let n = stream.read(&mut chunk).unwrap();
                    if n == 0 { break (String::new(), String::new()); }
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(at) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..at]).into_owned();
                        let length = headers.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").and_then(|n| n.trim().parse::<usize>().ok())).unwrap_or(0);
                        if bytes.len() >= at + 4 + length { break (headers, String::from_utf8_lossy(&bytes[at+4..at+4+length]).into_owned()); }
                    }
                };
                let path = headers.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or("").to_owned();
                let lower = headers.to_ascii_lowercase();
                shared.requests.lock().unwrap().push((path.clone(), lower.clone(), body.clone()));
                if shared.slow.load(Ordering::SeqCst) { std::thread::sleep(Duration::from_millis(180)); }
                let mut status = "200 OK";
                let mut extra = String::new();
                let value = if path.ends_with("/oauth/token") {
                    assert!(lower.contains("application/x-www-form-urlencoded"));
                    let fields: HashMap<_, _> = Url::parse(&format!("http://fixture/?{body}")).unwrap().query_pairs().map(|(k,v)|(k.into_owned(),v.into_owned())).collect();
                    assert_eq!(fields.get("client_id").unwrap(), "fixture-client");
                    assert_eq!(fields.get("client_secret").unwrap(), "fixture-secret");
                    if fields.get("grant_type").unwrap() == "refresh_token" {
                        assert_eq!(fields.get("refresh_token").unwrap(), "fixture-refresh");
                        json!({"access_token":"fixture-rotated", "refresh_token":"fixture-next-refresh", "expires_in":3600})
                    } else {
                        assert_eq!(fields.get("redirect_uri").unwrap(), skin_auth::CALLBACK);
                        assert_eq!(fields.get("code").unwrap(), "fixture-code");
                        json!({"access_token":"fixture-access", "refresh_token":"fixture-refresh", "expires_in":3600})
                    }
                } else if path.ends_with("/sessionserver/session/minecraft/profile") {
                    assert!(lower.contains("authorization: bearer fixture-"));
                    let mut items = vec![json!({"id":FIRST, "name":"First"})];
                    if shared.count.load(Ordering::SeqCst) == 2 { items.push(json!({"id":SECOND,"name":"Second"})); }
                    if shared.count.load(Ordering::SeqCst) == 0 { items.clear(); }
                    Value::Array(items)
                } else if path.ends_with("/authserver/oauth") {
                    assert!(lower.contains("authorization: bearer fixture-"));
                    if shared.fail_issue.load(Ordering::SeqCst) {
                        status = "503 Service Unavailable";
                        json!({"error":"invalid_scope", "error_description":"fixture-secret must never be displayed"})
                    } else {
                        let uuid = serde_json::from_str::<Value>(&body).unwrap()["uuid"].as_str().unwrap().to_owned();
                        json!({"accessToken":"fixture-minecraft", "clientToken":"fixture-game-client", "selectedProfile":{"id":uuid,"name":if uuid==FIRST {"First"} else {"Second"}}})
                    }
                } else if path.ends_with("/api-root") {
                    assert!(!lower.contains("authorization:"));
                    json!({"meta":{"serverName":if shared.unnamed.load(Ordering::SeqCst) { "" } else { "Fixture Skin" }, "links":{"homepage":"/skin","register":"/skin/auth/register"}}})
                } else {
                    extra = "X-Authlib-Injector-API-Location: /api-root\r\n".into();
                    json!({})
                };
                let text = value.to_string();
                let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{text}", text.len());
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { base, state, thread: Some(thread) }
    }
    fn calls(&self, suffix: &str) -> usize { self.state.requests.lock().unwrap().iter().filter(|r| r.0.ends_with(suffix)).count() }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() { thread.join().unwrap(); }
    }
}

// Exercise the actual plugin window, not a direct Rust call to handle_callback.
fn send_windows_callback(identifier: &str, raw: &str, legacy: bool) -> Result<(), String> {
    use std::ffi::c_void;
    #[repr(C)]
    struct CopyData { tag: usize, length: u32, data: *const u8 }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> *mut c_void;
        fn GetWindowThreadProcessId(window: *mut c_void, pid: *mut u32) -> u32;
        fn SendMessageTimeoutW(window: *mut c_void, message: u32, wparam: usize, lparam: isize, flags: u32, timeout: u32, result: *mut usize) -> isize;
    }
    let wide = |text: String| text.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let class = wide(format!("{identifier}-sic"));
    let title = wide(format!("{identifier}-siw"));
    let window = unsafe { FindWindowW(class.as_ptr(), title.as_ptr()) };
    let mut owner = 0;
    if !window.is_null() { unsafe { GetWindowThreadProcessId(window, &mut owner); } }
    if owner != std::process::id() { return Err("fixture callback window is missing or belongs to another process".into()); }
    let separator = if legacy { '|' } else { '\0' };
    let mut payload = format!("C:\\oauth-fixture{separator}fixture.exe{separator}{raw}");
    if legacy { payload.push('\0'); }
    let data = CopyData { tag: if legacy { 1542 } else { 1543 }, length: payload.len() as u32, data: payload.as_ptr() };
    let mut reply = 0;
    let sent = unsafe { SendMessageTimeoutW(window, 0x4a, 0, &data as *const _ as isize, 2, 3000, &mut reply) };
    if sent == 0 || reply != 1 { return Err("fixture WM_COPYDATA delivery failed".into()); }
    Ok(())
}

async fn finish_via_windows_callback(app: &mut tauri::App, id: String, raw: String, legacy: bool) -> Result<LoginOutcome, String> {
    let finish = skin_oauth_finish(app.handle().clone(), id);
    tokio::pin!(finish);
    // Prove that finish is already awaiting its receiver before native delivery.
    if tokio::time::timeout(Duration::from_millis(10), &mut finish).await.is_ok() {
        return Err("fixture login did not wait for the native callback".into());
    }
    let identifier = app.config().identifier.clone();
    let sender = std::thread::spawn(move || send_windows_callback(&identifier, &raw, legacy));
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut finish => break result,
                _ = tokio::time::sleep(Duration::from_millis(5)) => {
                    #[allow(deprecated)]
                    app.run_iteration(|_, event| {
                        if let tauri::RunEvent::ExitRequested { api, .. } = event { api.prevent_exit(); }
                    });
                }
            }
        }
    }).await;
    sender.join().map_err(|_| "fixture callback sender panicked")??;
    result.map_err(|_| "native callback did not wake the pending login")?
}


#[test]
#[ignore = "isolated Wry storage and loopback OAuth; run explicitly with one test thread"]
fn native_oauth_lifecycle() -> Result<(), String> {
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().identifier = format!("studio.nickel.launcher.oauth-fixture.{}", store::random_token());
    use tauri_plugin_deep_link::DeepLinkExt;
    let forwarded = Arc::new(AtomicUsize::new(0));
    let received = Arc::new(AtomicUsize::new(0));
    let single_count = forwarded.clone();
    let mut app = tauri::Builder::default().any_thread()
        .plugin(tauri_plugin_single_instance::init(move |_, args, _| {
            single_count.fetch_add(1, Ordering::SeqCst);
            for argument in &args { if argument.starts_with("nickel:") { handle_callback(argument); } }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .build(context).map_err(|e| e.to_string())?;
    let deep_count = received.clone();
    app.deep_link().on_open_url(move |event| {
        deep_count.fetch_add(1, Ordering::SeqCst);
        for url in event.urls() { handle_callback(url.as_str()); }
    });
    let config_dir = crate::app_paths::config_dir(app.handle())?;
    let server = Server::new();
    let site = format!("{}/skin", server.base);
    let config = ClientConfig { client_id: "fixture-client".into(), client_secret: "fixture-secret".into() };
    let handle = app.handle().clone();
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let result = runtime.block_on(async {
        // Old user overrides must neither replace nor reveal the built-in application.
        let mut legacy = HashMap::new();
        let sealed = crate::vault::seal(&handle, &serde_json::to_string(&config).unwrap())?;
        for (root, version) in [(None, None), (Some("C:/BuiltinFixture"), Some("v"))] {
            let key = serde_json::to_string(&(root, version, store::DEFAULT_SERVER)).unwrap();
            legacy.insert(key, sealed.clone());
        }
        crate::named_profiles::atomic_write(&config_dir.join("skin-oauth-clients.json"), &serde_json::to_vec(&legacy).unwrap())?;
        let builtin = crate::secrets::littleskin_config(&handle);
        for server in [store::DEFAULT_SERVER, "https://LITTLESKIN.CN:443/api/yggdrasil/", ""] {
            for (root, version) in [(None, None), (Some("C:/BuiltinFixture".to_owned()), Some("v".to_owned()))] {
                let view = skin_auth::skin_oauth_config_get(handle.clone(), server.into(), root.clone(), version.clone())?;
                assert!(view.client_id.is_empty() && view.client_secret.is_empty());
                assert_eq!(view.source, "builtin");
                assert_eq!(view.redirect_uri, skin_auth::CALLBACK);
                assert!(skin_auth::skin_oauth_config_set(handle.clone(), server.into(), root.clone(), version.clone(), Some(config.clone())).is_err());
                assert!(skin_auth::skin_oauth_config_set(handle.clone(), server.into(), root.clone(), version.clone(), None).is_err());
                let resolved = skin_auth::resolve_client(&handle, server, root, version);
                if let Some(expected) = builtin.as_ref().filter(|c| !c.client_id.trim().is_empty() && !c.client_secret.is_empty()) {
                    assert!(resolved.as_ref().is_ok_and(|actual| actual == expected));
                } else { assert!(resolved.is_err()); }
            }
        }
        crate::settings_set(handle.clone(), json!({"littleskinServer":site,"skinAuthMethod":"oauth2"}).to_string())?;
        skin_auth::skin_oauth_config_set(handle.clone(), site.clone(), None, None, Some(config.clone()))?;
        let (a,b) = tokio::join!(skin_auth::skin_site_describe(site.clone(), None), skin_auth::skin_site_describe(site.clone(), None));
        assert_eq!(a.name,"Fixture Skin"); assert_eq!(a.api,format!("{}/api-root",server.base)); assert_eq!(a.api,b.api);
        assert_eq!(server.calls("/api-root"),1);
        assert_eq!(server.calls("/skin"),1);
        let _ = crate::skin::skin_fetch("First".into(), Some("littleskin".into()), Some(FIRST.into()), Some(site.clone())).await?;
        assert_eq!(server.calls(&format!("/api-root/sessionserver/session/minecraft/profile/{FIRST}")), 1);
        assert_eq!(server.calls("/skin"), 1);
        server.state.unnamed.store(true, Ordering::SeqCst);
        let unnamed_site = format!("{}/unnamed", server.base);
        let unnamed = skin_auth::skin_site_describe(unnamed_site.clone(), Some("Legacy name".into())).await;
        assert_eq!(unnamed.name, "Legacy name");
        assert_eq!(unnamed.api, a.api);
        let renamed = skin_auth::skin_site_describe(unnamed_site, Some("Other instance name".into())).await;
        assert_eq!(renamed.name, "Other instance name");
        assert_eq!(server.calls("/unnamed"), 1);
        server.state.unnamed.store(false, Ordering::SeqCst);
        let other = skin_auth::skin_oauth_config_get(handle.clone(), format!("{}/other",server.base), None, None)?;
        assert!(other.client_id.is_empty() && other.client_secret.is_empty());
        let root = config_dir.join("fixture-game");
        let root_text = root.to_string_lossy().into_owned();
        std::fs::create_dir_all(root.join("versions/v")).map_err(|e|e.to_string())?;
        std::fs::write(root.join("versions/v/nickel.settings.json"),json!({"serverLogin":"thirdParty","authServer":site,"skinAuthMethod":"oauth2"}).to_string()).map_err(|e|e.to_string())?;
        assert!(skin_auth::skin_auth_context(handle.clone(),Some(root_text.clone()),Some("v".into())).await?.exclusive);
        skin_auth::skin_oauth_config_set(handle.clone(),site.clone(),Some(root_text.clone()),Some("v".into()),Some(ClientConfig{client_id:"instance".into(),client_secret:String::new()}))?;
        assert!(skin_auth::resolve_client(&handle,&site,Some(root_text.clone()),Some("v".into())).is_err());
        skin_auth::skin_oauth_config_set(handle.clone(),site.clone(),Some(root_text.clone()),Some("v".into()),None)?;
        assert!(skin_auth::resolve_client(&handle,&site,Some(root_text),Some("v".into()))? == config);

        // Windows normalizes nickel://auth?... to nickel://auth/?... . Both plugin
        // payload versions must wake an already waiting finish, and double dispatch
        // (deep-link event plus single-instance callback) must exchange only once.
        for legacy in [false, true] {
            let (id, _) = prepare(&handle, None, None).await?;
            let before = server.calls("/oauth/token");
            let callback = format!("{}/?state={id}&code=fixture-code", skin_auth::CALLBACK);
            let outcome = finish_via_windows_callback(&mut app, id.clone(), callback, legacy).await?;
            assert!(matches!(outcome, LoginOutcome::Account { .. }));
            assert_eq!(server.calls("/oauth/token"), before + 1);
            assert!(!AUTH.lock().unwrap().contains_key(&id));
            assert!(!FLAGS.lock().unwrap().contains_key(&id));
        }
        let (id, _) = prepare(&handle, None, None).await?;
        let before = server.calls("/oauth/token");
        let error = finish_via_windows_callback(&mut app, id, "nickel://auth/?error=access_denied".into(), false).await.err().expect("native denial must fail");
        assert_eq!(error, "你已在网页中取消授权");
        assert_eq!(server.calls("/oauth/token"), before);
        assert_eq!(forwarded.load(Ordering::SeqCst), 3);
        assert_eq!(received.load(Ordering::SeqCst), 3);


        let (id,url) = prepare(&handle,None,None).await?;
        assert_eq!(url.path(),"/skin/oauth/authorize");
        assert!(!url.as_str().contains("fixture-secret"));
        let query: HashMap<_,_> = url.query_pairs().collect();
        assert!(!query.get("scope").unwrap().contains("PlayerProfiles.Select"));
        handle_callback(&format!("{}?state=unknown&code=fixture-code",skin_auth::CALLBACK));
        assert!(AUTH.lock().unwrap().get(&id).unwrap().sender.is_some());
        handle_callback(&format!("{}?state={id}&code=fixture-code",skin_auth::CALLBACK));
        handle_callback(&format!("{}?state={id}&code=must-not-win",skin_auth::CALLBACK));
        let LoginOutcome::Account { account } = skin_oauth_finish(handle.clone(),id.clone()).await? else { panic!("single profile must complete") };
        assert!(skin_oauth_finish(handle.clone(),id).await.is_err());
        assert!(serde_json::to_value(&account).unwrap().get("oauth").is_none());
        assert!(!serde_json::to_string(&account).unwrap().contains("fixture-access"));
        let stored = store::find_account(&handle,&account.id).unwrap();
        assert!(stored.password.is_none());
        assert_eq!(stored.access_token.as_deref(),Some("fixture-minecraft"));
        assert_eq!(stored.auth_api.as_deref(),Some(a.api.as_str()));
        store::set_active(&handle,&account.id)?;
        let mut options = crate::build_options(&crate::ResolvedLaunch {version:"1.20.4".into(),username:"Player".into(),memory_max:"1G".into(),game_dir:None},config_dir.clone());
        let settings = crate::runtime_settings::resolve(&json!({}),&json!({"serverLogin":"thirdParty","authServer":site}),None)?;
        assert_eq!(crate::apply_active_account(&handle,&mut options,&settings).await?.as_deref(),Some(a.api.as_str()));
        assert_eq!(options.authenticator.access_token,"fixture-minecraft");
        let wrong = crate::runtime_settings::resolve(&json!({}),&json!({"serverLogin":"thirdParty","authServer":"https://other.invalid"}),None)?;
        assert!(crate::apply_active_account(&handle,&mut options,&wrong).await.is_err());

        server.state.count.store(2,Ordering::SeqCst);
        let list = crate::yggdrasil::yggdrasil_profiles(handle.clone(),account.id.clone()).await?;
        assert_eq!(list.profiles.len(),2);
        server.state.fail_issue.store(true,Ordering::SeqCst);
        let error = select(handle.clone(),list.session_id.clone(),SECOND.into()).await.err().unwrap();
        assert!(error.contains("invalid_scope") && !error.contains("fixture-secret"));
        server.state.fail_issue.store(false,Ordering::SeqCst);
        let switched = crate::yggdrasil::yggdrasil_select_profile(handle.clone(),list.session_id,SECOND.into()).await?;
        assert_eq!(switched.id,account.id);
        assert_eq!(store::undash_uuid(&switched.uuid),SECOND);
        let list = list_profiles(handle.clone(),store::find_account(&handle,&account.id).unwrap()).await?;
        skin_oauth_cancel(Some(list.session_id.clone()));
        assert!(select(handle.clone(),list.session_id,FIRST.into()).await.is_err());

        let (id,_) = prepare(&handle,None,None).await?;
        handle_callback(&format!("{}?state={id}&code=fixture-code",skin_auth::CALLBACK));
        let LoginOutcome::Select {session_id,profiles} = skin_oauth_finish(handle.clone(),id).await? else { panic!("multiple profiles must select") };
        assert_eq!(profiles.len(),2);
        let selected = select(handle.clone(),session_id,FIRST.into()).await?;
        let mut stored = store::find_account(&handle,&selected.id).unwrap();
        let mut tokens: Tokens = serde_json::from_str(&crate::vault::open(&handle,stored.oauth.as_ref().unwrap().as_str())?).unwrap();
        tokens.expires = 0; stored.oauth = Some(seal(&handle,&tokens)?);
        store::upsert_and_save(&handle,stored.clone())?;
        skin_auth::skin_oauth_config_set(handle.clone(),site.clone(),None,None,Some(ClientConfig{client_id:"new-client".into(),client_secret:"new-secret".into()}))?;
        refresh(handle.clone(),stored).await?;
        let stored = store::find_account(&handle,&selected.id).unwrap();
        let restored: Tokens = serde_json::from_str(&crate::vault::open(&handle,stored.oauth.as_ref().unwrap().as_str())?).unwrap();
        assert_eq!(restored.access,"fixture-rotated"); assert_eq!(restored.refresh,"fixture-next-refresh");
        assert_eq!(restored.client.client_id,"fixture-client");
        skin_auth::skin_oauth_config_set(handle.clone(),site.clone(),None,None,Some(config.clone()))?;

        let (id,_) = prepare(&handle,None,None).await?;
        let before = server.calls("/oauth/token");
        handle_callback(&format!("{}?state={id}&error=access_denied",skin_auth::CALLBACK));
        assert!(skin_oauth_finish(handle.clone(),id).await.is_err());
        assert_eq!(server.calls("/oauth/token"),before);
        let (id,_) = prepare(&handle,None,None).await?;
        for invalid in ["nickel://auth?state=unknown&error=access_denied", "nickel://auth?code=unbound", "nickel://other?error=access_denied"] {
            handle_callback(invalid);
            assert!(AUTH.lock().unwrap().get(&id).unwrap().sender.is_some());
        }
        let deny = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            handle_callback("nickel://auth?error=access_denied&error_description=untrusted-secret");
            handle_callback("nickel://auth?error=server_error");
        };
        let finish = tokio::time::timeout(Duration::from_secs(2), skin_oauth_finish(handle.clone(), id.clone()));
        let (result, _) = tokio::join!(finish, deny);
        let error = result.expect("state-less denial must wake the pending login").err().expect("denial must fail");
        assert_eq!(error, "你已在网页中取消授权");
        assert!(!AUTH.lock().unwrap().contains_key(&id));
        assert!(!FLAGS.lock().unwrap().contains_key(&id));
        assert_eq!(server.calls("/oauth/token"), before);
        let (id,_) = prepare(&handle,None,None).await?;
        AUTH.lock().unwrap().get_mut(&id).unwrap().created = Instant::now() - TTL;
        assert!(skin_oauth_finish(handle.clone(),id).await.is_err());
        let (id,_) = prepare(&handle,None,None).await?;
        skin_oauth_cancel(Some(id.clone()));
        handle_callback(&format!("{}?state={id}&code=fixture-code",skin_auth::CALLBACK));
        assert!(skin_oauth_finish(handle.clone(),id).await.is_err());
        assert_eq!(server.calls("/oauth/token"),before);
        let (id,_) = prepare(&handle,None,None).await?;
        handle_callback(&format!("{}?state={id}&code=fixture-code",skin_auth::CALLBACK));
        server.state.slow.store(true,Ordering::SeqCst);
        let cancel = async { tokio::time::sleep(Duration::from_millis(40)).await; skin_oauth_cancel(Some(id.clone())); };
        let (result,_) = tokio::join!(skin_oauth_finish(handle.clone(),id.clone()),cancel);
        assert!(result.is_err());
        server.state.slow.store(false,Ordering::SeqCst);
        server.state.count.store(0,Ordering::SeqCst);
        let (id,_) = prepare(&handle,None,None).await?;
        handle_callback(&format!("{}?state={id}&code=fixture-code",skin_auth::CALLBACK));
        assert!(skin_oauth_finish(handle.clone(),id).await.is_err());
        assert!(!server.state.requests.lock().unwrap().iter().any(|r|r.0.ends_with("/authenticate")));
        eprintln!("OAuth fixture: ALI/cache, scoped clients, callback/state/denial/timeout/cancel, profiles/retry, sealed restore, rotated refresh, original client and launch policy passed");
        Ok(())
    });
    cancel_all();
    drop(runtime);
    app.cleanup_before_exit();
    drop(app);
    std::fs::remove_dir_all(config_dir).map_err(|e|e.to_string())?;
    result
}
