use super::*;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct SocketServer {
    url: String,
    task: tokio::task::JoinHandle<()>,
    peak: Arc<AtomicUsize>,
    disconnected_slow: Arc<AtomicBool>,
}
impl Drop for SocketServer { fn drop(&mut self) { self.task.abort(); } }

async fn server(size: usize) -> SocketServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let disconnected_slow = Arc::new(AtomicBool::new(false));
    let server_peak = peak.clone();
    let disconnected = disconnected_slow.clone();
    let task = tokio::spawn(async move {
        let mut children = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                result = listener.accept() => {
                    let Ok((mut stream, _)) = result else { break; };
                    let active = active.clone(); let peak = server_peak.clone(); let disconnected = disconnected.clone();
                    children.spawn(async move {
                        let mut request = Vec::new();
                        while !request.ends_with(b"\r\n\r\n") {
                            let mut byte = [0u8; 1];
                            if stream.read_exact(&mut byte).await.is_err() { return; }
                            request.push(byte[0]);
                            if request.len() > 8192 { return; }
                        }
                        let request = String::from_utf8_lossy(&request);
                        let slow = request.starts_with("GET /slow ");
                        let hold = request.starts_with("GET /hold ");
                        let active_count = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(active_count, Ordering::SeqCst);
                        let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n");
                        let mut failed = stream.write_all(header.as_bytes()).await.is_err();
                        if hold { tokio::time::sleep(Duration::from_millis(150)).await; }
                        let chunk = vec![if slow { 1u8 } else { 2u8 }; 1024];
                        let mut sent = 0;
                        while !failed && sent < size {
                            let len = (size - sent).min(chunk.len());
                            failed = stream.write_all(&chunk[..len]).await.is_err();
                            sent += len;
                            if slow { tokio::time::sleep(Duration::from_millis(250)).await; }
                        }
                        if failed && slow { disconnected.store(true, Ordering::SeqCst); }
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                },
                _ = children.join_next(), if !children.is_empty() => {},
            }
        }
    });
    SocketServer { url, task, peak, disconnected_slow }
}

fn test_context(threads: u32, rate: u64, policy: SourcePolicy) -> TransferContext {
    let mut context = TransferContext::new(policy, policy, CancellationToken::new());
    context.controller = Arc::new(TransferController::new(threads, rate)); context
}

#[tokio::test]
async fn measured_slow_stream_races_and_cancels_real_losing_socket() {
    let server = server(128 * 1024).await;
    let dir = tempfile::TempDir::new().unwrap();
    let request = DownloadRequest { candidates: vec![RequestCandidate::official(format!("{}/slow", server.url)), RequestCandidate::mirror(format!("{}/fast", server.url))], destination: dir.path().join("file"), expected_size: Some(128 * 1024), hashes: FileHashes::default(), service: "stream".into() };
    let context = test_context(4, 0, SourcePolicy::OfficialAuto);
    let start = Instant::now();
    download(&reqwest::Client::new(), &context, &request, None).await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(std::fs::read(&request.destination).unwrap(), vec![2u8; 128 * 1024]);
    tokio::time::timeout(Duration::from_secs(3), async {
        while !server.disconnected_slow.load(Ordering::SeqCst) { tokio::time::sleep(Duration::from_millis(50)).await; }
    }).await.unwrap();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn actual_file_requests_respect_shared_process_admission() {
    let server = server(4096).await;
    let dir = tempfile::TempDir::new().unwrap();
    let context = test_context(2, 0, SourcePolicy::OfficialFirst);
    let client = reqwest::Client::new();
    let mut downloads = FuturesUnordered::new();
    for index in 0..8 {
        let client = &client; let context = &context; let destination = dir.path().join(format!("file-{index}")); let url = format!("{}/hold", server.url);
        downloads.push(async move {
            let request = DownloadRequest { candidates: vec![RequestCandidate::official(url)], destination, expected_size: Some(4096), hashes: FileHashes::default(), service: "budget".into() };
            download(client, context, &request, None).await
        });
    }
    while let Some(result) = downloads.next().await { result.unwrap(); }
    assert_eq!(server.peak.load(Ordering::SeqCst), 2);
    assert_eq!(context.controller.state.lock().unwrap().active, 0);
}

#[tokio::test]
async fn real_streams_share_one_total_bytes_per_second_limit() {
    let size = 32 * 1024;
    let server = server(size).await;
    let dir = tempfile::TempDir::new().unwrap();
    let context = test_context(4, 16 * 1024, SourcePolicy::OfficialFirst);
    let client = reqwest::Client::new();
    let first = DownloadRequest { candidates: vec![RequestCandidate::official(format!("{}/fast", server.url))], destination: dir.path().join("first"), expected_size: Some(size as u64), hashes: FileHashes::default(), service: "rate".into() };
    let mut second = first.clone(); second.destination = dir.path().join("second");
    let start = Instant::now();
    let (a, b) = tokio::join!(download(&client, &context, &first, None), download(&client, &context, &second, None));
    a.unwrap(); b.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(3900));
    assert!(start.elapsed() < Duration::from_secs(7));
}
