use super::*;
use std::time::Instant;
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path};

fn context(policy: SourcePolicy) -> TransferContext {
    let mut context = TransferContext::new(policy, policy, CancellationToken::new());
    context.controller = Arc::new(TransferController::new(4, 0));
    context
}

#[tokio::test]
async fn child_context_shares_measurements_and_budget_but_not_upstream_cancellation() {
    let parent = context(SourcePolicy::OfficialAuto);
    let child = parent.child();
    assert!(Arc::ptr_eq(&parent.controller, &child.controller));
    child.list_mirrors.lock().unwrap().insert("list".into());
    child.file_mirrors.lock().unwrap().insert("file".into());
    assert!(parent.list_mirrors.lock().unwrap().contains("list"));
    assert!(parent.file_mirrors.lock().unwrap().contains("file"));
    child.cancellation().cancel();
    assert!(!parent.cancellation().is_cancelled());
    let sibling = parent.child();
    assert!(!sibling.cancellation().is_cancelled());
    let grandchild = sibling.child();
    parent.cancellation().cancel();
    tokio::time::timeout(Duration::from_millis(100), grandchild.cancellation().cancelled()).await.unwrap();
    assert!(sibling.cancellation().is_cancelled());
}

#[tokio::test]
async fn parsing_failure_falls_back_before_retrying_for_all_policies() {
    for policy in [SourcePolicy::OfficialFirst, SourcePolicy::OfficialAuto, SourcePolicy::MirrorFirst] {
        let server = MockServer::start().await;
        let first = if policy == SourcePolicy::MirrorFirst { "/mirror" } else { "/official" };
        let second = if policy == SourcePolicy::MirrorFirst { "/official" } else { "/mirror" };
        Mock::given(path(first)).respond_with(ResponseTemplate::new(200).set_body_string("not-json")).expect(1).mount(&server).await;
        Mock::given(path(second)).respond_with(ResponseTemplate::new(200).set_body_string("{\"ok\":true}")).expect(1).mount(&server).await;
        let candidates = [RequestCandidate::official(format!("{}/official", server.uri())), RequestCandidate::mirror(format!("{}/mirror", server.uri()))];
        let value: serde_json::Value = fetch_json_candidates(&reqwest::Client::new(), &context(policy), "test", &candidates, 1024).await.unwrap();
        assert_eq!(value["ok"], true);
    }
}

#[tokio::test]
async fn auto_races_valid_mirror_after_four_seconds_and_new_context_retests() {
    let server = MockServer::start().await;
    Mock::given(path("/official")).respond_with(ResponseTemplate::new(200).set_body_string("{\"source\":0}").set_delay(Duration::from_secs(8))).mount(&server).await;
    Mock::given(path("/mirror")).respond_with(ResponseTemplate::new(200).set_body_string("{\"source\":1}")).mount(&server).await;
    let candidates = [RequestCandidate::official(format!("{}/official", server.uri())), RequestCandidate::mirror(format!("{}/mirror", server.uri()))];
    let client = reqwest::Client::new();
    let ctx = context(SourcePolicy::OfficialAuto);
    let start = Instant::now();
    let value: serde_json::Value = fetch_json_candidates(&client, &ctx, "test", &candidates, 1024).await.unwrap();
    assert_eq!(value["source"], 1);
    assert!(start.elapsed() >= AUTO_RESPONSE_DELAY);
    assert!(start.elapsed() < Duration::from_secs(7));
    let start = Instant::now();
    let _: serde_json::Value = fetch_json_candidates(&client, &ctx, "test", &candidates, 1024).await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!context(SourcePolicy::OfficialAuto).list_mirrors.lock().unwrap().contains("test"));
}

#[tokio::test]
async fn cancellation_interrupts_body_and_preserves_destination() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_string("new").set_delay(Duration::from_secs(5))).mount(&server).await;
    let ctx = context(SourcePolicy::OfficialFirst);
    let cancel = ctx.cancel.clone();
    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join("file"); std::fs::write(&dest, b"existing").unwrap();
    let request = DownloadRequest { candidates: vec![RequestCandidate::official(server.uri())], destination: dest.clone(), expected_size: Some(3), hashes: FileHashes::default(), service: "test".into() };
    let client = reqwest::Client::new();
    let start = Instant::now();
    let (result, _) = tokio::join!(download(&client, &ctx, &request, None), async { tokio::time::sleep(Duration::from_millis(100)).await; cancel.cancel(); });
    assert!(result.is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(std::fs::read(&dest).unwrap(), b"existing");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn shared_budget_and_rate_do_not_multiply_per_consumer() {
    let controller = Arc::new(TransferController::new(2, 65536));
    let cancel = CancellationToken::new();
    let first = controller.acquire(&cancel).await.unwrap();
    let second = controller.acquire(&cancel).await.unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(30), controller.acquire(&cancel)).await.is_err());
    drop(first); drop(second);
    let start = Instant::now();
    let (a, b) = tokio::join!(controller.throttle(65536, &cancel), controller.throttle(65536, &cancel));
    a.unwrap(); b.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(1900));
    controller.configure(1, 0);
    let before = Instant::now(); controller.throttle(65536, &cancel).await.unwrap();
    assert!(before.elapsed() < Duration::from_millis(100));
}

#[tokio::test]
async fn sha512_and_size_failures_fall_back_without_completed_inflation() {
    use sha2::Digest;
    let server = MockServer::start().await;
    Mock::given(path("/bad")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"bad")).mount(&server).await;
    Mock::given(path("/good")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"yes")).mount(&server).await;
    let sha512 = format!("{:x}", sha2::Sha512::digest(b"yes"));
    let dir = tempfile::TempDir::new().unwrap();
    let request = DownloadRequest { candidates: vec![RequestCandidate::official(format!("{}/bad", server.uri())), RequestCandidate::mirror(format!("{}/good", server.uri()))], destination: dir.path().join("file"), expected_size: Some(3), hashes: FileHashes { sha512: Some(sha512), ..FileHashes::default() }, service: "test".into() };
    let events = Arc::new(Mutex::new(Vec::new())); let capture = events.clone();
    download(&reqwest::Client::new(), &context(SourcePolicy::OfficialFirst), &request, Some(Arc::new(move |event| capture.lock().unwrap().push(event)))).await.unwrap();
    let events = events.lock().unwrap();
    assert_eq!(events.last().unwrap().completed_bytes, 3);
    assert_eq!(events.last().unwrap().transferred_bytes, 6);
    assert!(events.iter().all(|event| event.completed_bytes <= 3));
}

#[tokio::test]
async fn authorization_is_never_forwarded_to_redirect_or_mirror() {
    let server = MockServer::start().await;
    Mock::given(path("/redirect")).respond_with(ResponseTemplate::new(302).insert_header("Location", format!("{}/leak", server.uri()))).mount(&server).await;
    Mock::given(path("/leak")).respond_with(ResponseTemplate::new(200).set_body_string("{}")).expect(0).mount(&server).await;
    let mut candidate = RequestCandidate::official(format!("{}/redirect", server.uri()));
    candidate.headers.insert("x-api-key", "test-key".parse().unwrap());
    assert!(fetch_json_candidates::<serde_json::Value>(&reqwest::Client::new(), &context(SourcePolicy::OfficialFirst), "test", &[candidate.clone()], 1024).await.is_err());
    candidate.is_mirror = true;
    assert!(send(&reqwest::Client::new(), &candidate, HeaderMap::new(), &CancellationToken::new()).await.is_err());
}


struct RangeResponder { body: Arc<Vec<u8>>, mode: u8 }
impl wiremock::Respond for RangeResponder {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let range = request.headers.get("range").and_then(|value| value.to_str().ok());
        let Some(range) = range else { return ResponseTemplate::new(200).set_body_bytes(self.body.as_ref().clone()).insert_header("ETag", "\"stable\""); };
        let (start, end) = range.strip_prefix("bytes=").unwrap().split_once('-').unwrap();
        let start: usize = start.parse().unwrap(); let end: usize = end.parse().unwrap();
        if self.mode == 1 || (self.mode == 3 && start > 0) {
            return ResponseTemplate::new(200).set_body_bytes(self.body.as_ref().clone());
        }
        let actual_start = if self.mode == 2 { start + 1 } else { start };
        let etag = if self.mode == 4 && start > 0 { "\"different\"" } else { "\"stable\"" };
        ResponseTemplate::new(206).insert_header("Content-Range", format!("bytes {actual_start}-{end}/{}", self.body.len()))
            .insert_header("ETag", etag).set_body_bytes(self.body[start..=end].to_vec())
    }
}

#[tokio::test]
async fn large_file_ranges_validate_length_identity_and_full_stream_fallback() {
    use sha2::Digest;
    let body = Arc::new(vec![37u8; file::RANGE_THRESHOLD as usize]);
    for mode in 0..=4 {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(RangeResponder { body: body.clone(), mode }).mount(&server).await;
        let dir = tempfile::TempDir::new().unwrap();
        let request = DownloadRequest { candidates: vec![RequestCandidate::official(server.uri())], destination: dir.path().join("large"),
            expected_size: Some(body.len() as u64), hashes: FileHashes { sha256: Some(format!("{:x}", sha2::Sha256::digest(body.as_slice()))), ..FileHashes::default() }, service: "range".into() };
        let result = download(&reqwest::Client::new(), &context(SourcePolicy::OfficialFirst), &request, None).await;
        if mode == 2 || mode == 4 {
            assert!(result.is_err(), "mode {mode}"); assert!(!request.destination.exists());
        } else {
            result.unwrap(); assert_eq!(std::fs::read(&request.destination).unwrap(), body.as_ref().clone());
        }
        if mode == 0 {
            let requests = server.received_requests().await.unwrap();
            assert_eq!(requests.len(), 4);
            for request in requests {
                let range = request.headers.get("range").unwrap().to_str().unwrap();
                let (start, end) = range.strip_prefix("bytes=").unwrap().split_once('-').unwrap();
                let start: u64 = start.parse().unwrap(); let end: u64 = end.parse().unwrap();
                assert!(end - start + 1 >= file::MIN_RANGE_BYTES);
            }
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), usize::from(result_file_exists(mode)));
    }
}

fn result_file_exists(mode: u8) -> bool { mode != 2 && mode != 4 }

#[tokio::test]
async fn unknown_length_discovers_large_file_and_uses_valid_ranges() {
    let server = MockServer::start().await;
    let body = Arc::new(vec![53u8; file::RANGE_THRESHOLD as usize]);
    Mock::given(method("GET")).respond_with(RangeResponder { body: body.clone(), mode: 0 }).mount(&server).await;
    let dir = tempfile::TempDir::new().unwrap();
    let request = DownloadRequest { candidates: vec![RequestCandidate::official(server.uri())], destination: dir.path().join("large"), expected_size: None, hashes: FileHashes::default(), service: "range".into() };
    download(&reqwest::Client::new(), &context(SourcePolicy::OfficialFirst), &request, None).await.unwrap();
    assert_eq!(std::fs::read(&request.destination).unwrap(), body.as_ref().clone());
    assert_eq!(server.received_requests().await.unwrap().len(), 5);
}

#[tokio::test]
async fn cancelled_queued_transfers_and_processes_terminate_promptly() {
    let controller = Arc::new(TransferController::new(1, 1));
    let cancel = CancellationToken::new();
    let _permit = controller.acquire(&cancel).await.unwrap();
    let (queued, _) = tokio::join!(controller.acquire(&cancel), async { tokio::task::yield_now().await; cancel.cancel(); });
    assert!(queued.is_err());
    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("ping"); command.args(["-n", "30", "127.0.0.1"]); command
    } else {
        let mut command = tokio::process::Command::new("sleep"); command.arg("30"); command
    };
    let mut child = command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true).spawn().unwrap();
    assert!(wait_child(&mut child, &cancel).await.is_err());
    assert!(child.try_wait().unwrap().is_some());
}

#[tokio::test]
async fn auto_single_slot_releases_slow_request_before_switching() {
    let server = MockServer::start().await;
    Mock::given(path("/slow")).respond_with(ResponseTemplate::new(200).set_body_string("old").set_delay(Duration::from_secs(9))).mount(&server).await;
    Mock::given(path("/fast")).respond_with(ResponseTemplate::new(200).set_body_string("new")).mount(&server).await;
    let ctx = context(SourcePolicy::OfficialAuto); ctx.controller.configure(1, 0);
    let dir = tempfile::TempDir::new().unwrap();
    let request = DownloadRequest { candidates: vec![RequestCandidate::official(format!("{}/slow", server.uri())), RequestCandidate::mirror(format!("{}/fast", server.uri()))], destination: dir.path().join("file"), expected_size: Some(3), hashes: FileHashes::default(), service: "test".into() };
    let start = Instant::now();
    download(&reqwest::Client::new(), &ctx, &request, None).await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(7));
    assert_eq!(std::fs::read(request.destination).unwrap(), b"new");
    assert_eq!(ctx.controller.state.lock().unwrap().active, 0);
}

#[tokio::test]
async fn auto_single_slot_restores_slow_official_after_mirror_failure() {
    let server = MockServer::start().await;
    Mock::given(path("/official")).respond_with(ResponseTemplate::new(200).set_body_string("official").set_delay(Duration::from_secs(6))).expect(2).mount(&server).await;
    Mock::given(path("/mirror")).respond_with(ResponseTemplate::new(404)).expect(1).mount(&server).await;
    let ctx = context(SourcePolicy::OfficialAuto);
    ctx.controller.configure(1, 0);
    let dir = tempfile::TempDir::new().unwrap();
    let request = DownloadRequest {
        candidates: vec![RequestCandidate::official(format!("{}/official", server.uri())), RequestCandidate::mirror(format!("{}/mirror", server.uri()))],
        destination: dir.path().join("file"), expected_size: Some(8), hashes: FileHashes::default(), service: "restored".into(),
    };
    let client = reqwest::Client::new();
    let transfer = download(&client, &ctx, &request, None);
    tokio::pin!(transfer);
    let mut ticks = tokio::time::interval(Duration::from_millis(10));
    let deadline = tokio::time::sleep(Duration::from_secs(14));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            result = &mut transfer => { result.unwrap(); break; },
            _ = ticks.tick() => assert!(ctx.controller.state.lock().unwrap().active <= 1),
            _ = &mut deadline => panic!("restored official request was starved"),
        }
    }
    assert_eq!(std::fs::read(&request.destination).unwrap(), b"official");
    assert_eq!(ctx.controller.state.lock().unwrap().active, 0);
    assert!(!ctx.file_mirrors.lock().unwrap().contains("restored"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}


#[tokio::test]
async fn throttling_does_not_select_mirror_or_limit_lists() {
    let server = MockServer::start().await;
    Mock::given(path("/file")).respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8; 32768])).mount(&server).await;
    Mock::given(path("/mirror")).respond_with(ResponseTemplate::new(200).set_body_bytes(vec![2u8; 32768])).expect(0).mount(&server).await;
    Mock::given(path("/list")).respond_with(ResponseTemplate::new(200).set_body_string("{}")).mount(&server).await;
    let ctx = context(SourcePolicy::OfficialAuto); ctx.controller.configure(2, 8192);
    let dir = tempfile::TempDir::new().unwrap();
    let request = DownloadRequest { candidates: vec![RequestCandidate::official(format!("{}/file", server.uri())), RequestCandidate::mirror(format!("{}/mirror", server.uri()))], destination: dir.path().join("file"), expected_size: Some(32768), hashes: FileHashes::default(), service: "test".into() };
    let client = reqwest::Client::new();
    let (file, list) = tokio::join!(download(&client, &ctx, &request, None), async {
        let start = Instant::now();
        let value = fetch_json_candidates::<serde_json::Value>(&client, &ctx, "list", &[RequestCandidate::official(format!("{}/list", server.uri()))], 1024).await;
        assert!(start.elapsed() < Duration::from_secs(1)); value
    });
    file.unwrap(); list.unwrap();
}

#[tokio::test]
async fn file_failures_switch_before_retry_and_contexts_never_change_each_other() {
    let server = MockServer::start().await;
    Mock::given(path("/bad")).respond_with(ResponseTemplate::new(404)).expect(1).mount(&server).await;
    Mock::given(path("/ok")).respond_with(ResponseTemplate::new(200).set_body_string("ok")).expect(1).mount(&server).await;
    let ctx = context(SourcePolicy::MirrorFirst);
    let opposite = context(SourcePolicy::OfficialFirst);
    let (a, b) = tokio::join!(ctx.scope(async { TransferContext::current().file_policy() }), opposite.scope(async { tokio::task::yield_now().await; TransferContext::current().file_policy() }));
    assert_eq!(a, SourcePolicy::MirrorFirst); assert_eq!(b, SourcePolicy::OfficialFirst);
    let dir = tempfile::TempDir::new().unwrap();
    let request = DownloadRequest { candidates: vec![RequestCandidate::official(format!("{}/ok", server.uri())), RequestCandidate::mirror(format!("{}/bad", server.uri()))], destination: dir.path().join("file"), expected_size: Some(2), hashes: FileHashes::default(), service: "test".into() };
    download(&reqwest::Client::new(), &ctx, &request, None).await.unwrap();
}


#[test]
fn embedded_mirror_urls_recover_official_candidates_without_global_mutation() {
    for (mirror, official) in [
        ("https://bmclapi2.bangbang93.com/assets/ab/abc", "https://resources.download.minecraft.net/ab/abc"),
        ("https://bmclapi2.bangbang93.com/v1/objects/abc/client.jar", "https://piston-data.mojang.com/v1/objects/abc/client.jar"),
        ("https://bmclapi2.bangbang93.com/maven/net/neoforged/neoforge/a.jar", "https://maven.neoforged.net/releases/net/neoforged/neoforge/a.jar"),
    ] {
        let candidates = core_candidates(mirror);
        assert!(candidates.iter().any(|candidate| !candidate.is_mirror && candidate.url == official));
        assert!(candidates.iter().any(|candidate| candidate.is_mirror && candidate.url == mirror));
    }
    let auth = core_candidates("https://api.minecraftservices.com/minecraft/profile");
    assert_eq!(auth.len(), 1); assert!(!auth[0].is_mirror);
}

#[tokio::test]
async fn managed_client_does_not_timeout_while_deliberately_throttled() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 32768])).expect(1).mount(&server).await;
    let dir = tempfile::TempDir::new().unwrap();
    let ctx = context(SourcePolicy::OfficialFirst); ctx.controller.configure(2, 8192);
    let client = crate::net::client::build_transfer_client(1, false, None).unwrap();
    let request = DownloadRequest { candidates: vec![RequestCandidate::official(server.uri())], destination: dir.path().join("file"), expected_size: Some(32768), hashes: FileHashes::default(), service: "limit".into() };
    download(&client, &ctx, &request, None).await.unwrap();
}

#[tokio::test]
async fn child_cancel_does_not_cancel_sibling_and_parent_wakes_every_child() {
    let parent = CancellationToken::new(); let a = parent.child(); let b = parent.child();
    a.cancel(); assert!(a.is_cancelled()); assert!(!parent.is_cancelled()); assert!(!b.is_cancelled());
    parent.cancel(); assert!(b.is_cancelled());
    tokio::time::timeout(Duration::from_millis(100), b.cancelled()).await.unwrap();
}


#[tokio::test]
async fn bounded_validator_failure_switches_candidate_without_retries_or_commit() {
    let server = MockServer::start().await;
    Mock::given(path("/html")).respond_with(ResponseTemplate::new(200).set_body_string("<html>bad</html>")).expect(1).mount(&server).await;
    Mock::given(path("/good")).respond_with(ResponseTemplate::new(200).set_body_string("good")).expect(1).mount(&server).await;
    let dir = tempfile::TempDir::new().unwrap();
    let request = DownloadRequest { candidates: vec![RequestCandidate::official(format!("{}/html", server.uri())), RequestCandidate::mirror(format!("{}/good", server.uri()))], destination: dir.path().join("file"), expected_size: None, hashes: FileHashes::default(), service: "validator".into() };
    let ctx = context(SourcePolicy::OfficialFirst);
    let validator: FileValidator = Arc::new(|path, cancel| { cancel.check()?; if std::fs::read(path).map_err(|e| e.to_string())? == b"good" { Ok(()) } else { Err("validation failed".into()) } });
    download_validated(&reqwest::Client::new(), &ctx, &request, 64 * 1024 * 1024, validator, None).await.unwrap();
    assert_eq!(std::fs::read(&request.destination).unwrap(), b"good");
    assert_eq!(ctx.controller.state.lock().unwrap().active, 0);
}

#[tokio::test]
async fn declared_oversize_is_rejected_before_validator_and_target_is_unchanged() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8; 32])).mount(&server).await;
    let dir = tempfile::TempDir::new().unwrap(); let destination = dir.path().join("file"); std::fs::write(&destination, b"existing").unwrap();
    let request = DownloadRequest { candidates: vec![RequestCandidate::official(server.uri())], destination, expected_size: None, hashes: FileHashes::default(), service: "bounded".into() };
    let validator: FileValidator = Arc::new(|_, _| panic!("oversized file must not reach validator"));
    assert!(download_validated(&reqwest::Client::new(), &context(SourcePolicy::OfficialFirst), &request, 16, validator, None).await.is_err());
    assert_eq!(std::fs::read(&request.destination).unwrap(), b"existing");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn ordinary_jar_html_response_falls_back_to_valid_non_executable_zip() {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("ordinary.txt", zip::write::SimpleFileOptions::default()).unwrap(); zip.write_all(b"data").unwrap();
    let zip = zip.finish().unwrap().into_inner();
    let server = MockServer::start().await;
    Mock::given(path("/bad")).respond_with(ResponseTemplate::new(200).set_body_string("<html>bad</html>")).expect(1).mount(&server).await;
    Mock::given(path("/ok")).respond_with(ResponseTemplate::new(200).set_body_bytes(zip.clone())).expect(1).mount(&server).await;
    let dir = tempfile::TempDir::new().unwrap();
    let request = DownloadRequest { candidates: vec![RequestCandidate::official(format!("{}/bad", server.uri())), RequestCandidate::mirror(format!("{}/ok", server.uri()))], destination: dir.path().join("library.jar"), expected_size: None, hashes: FileHashes::default(), service: "jar".into() };
    download(&reqwest::Client::new(), &context(SourcePolicy::OfficialFirst), &request, None).await.unwrap();
    assert_eq!(std::fs::read(&request.destination).unwrap(), zip);
}

#[test]
fn candidate_diagnostics_exclude_credentials_paths_queries_and_headers() {
    let mut candidate = RequestCandidate::official("https://private-user:private-password@example.test/private-path?token=private-query#private-fragment");
    candidate.headers.insert("x-api-key", "private-key".parse().unwrap());
    assert_eq!(candidate.diagnostic_label(), "official https://example.test");
    assert_eq!(RequestCandidate::mirror("invalid private-secret").diagnostic_label(), "mirror invalid-url");
}

#[tokio::test]
async fn failed_download_reports_both_attempted_candidates() {
    let server = MockServer::start().await;
    Mock::given(path("/official")).respond_with(ResponseTemplate::new(503)).expect(3).mount(&server).await;
    Mock::given(path("/mirror")).respond_with(ResponseTemplate::new(404)).expect(3).mount(&server).await;
    let dir = tempfile::TempDir::new().unwrap();
    let request = DownloadRequest {
        candidates: vec![
            RequestCandidate::official(format!("{}/official?token=private-official", server.uri())),
            RequestCandidate::mirror(format!("{}/mirror?token=private-mirror", server.uri())),
        ],
        destination: dir.path().join("file"), expected_size: None, hashes: FileHashes::default(), service: "failures".into(),
    };
    let error = download(&reqwest::Client::new(), &context(SourcePolicy::OfficialFirst), &request, None).await.unwrap_err();
    assert!(error.contains(&format!("official {}: HTTP 503", server.uri())));
    assert!(error.contains(&format!("mirror {}: HTTP 404", server.uri())));
    assert!(!error.contains("private-official"));
    assert!(!error.contains("private-mirror"));
}

#[tokio::test]
async fn official_first_connection_failure_attempts_mirror_before_retry() {
    let server = MockServer::start().await;
    Mock::given(path("/mirror")).respond_with(ResponseTemplate::new(200).set_body_string("mirror")).expect(1).mount(&server).await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let dir = tempfile::TempDir::new().unwrap();
    let request = DownloadRequest {
        candidates: vec![RequestCandidate::official(format!("http://{address}/file")), RequestCandidate::mirror(format!("{}/mirror", server.uri()))],
        destination: dir.path().join("file"), expected_size: Some(6), hashes: FileHashes::default(), service: "fallback".into(),
    };
    download(&crate::net::client::build_transfer_client(1, false, None).unwrap(), &context(SourcePolicy::OfficialFirst), &request, None).await.unwrap();
    assert_eq!(std::fs::read(&request.destination).unwrap(), b"mirror");
}
