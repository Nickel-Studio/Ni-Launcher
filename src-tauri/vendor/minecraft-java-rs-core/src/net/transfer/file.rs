use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use reqwest::header::{CONTENT_RANGE, ETAG, HeaderMap, HeaderValue, IF_RANGE, RANGE};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;

use super::{
    AUTO_RESPONSE_DELAY, CancellationToken, IDLE_TIMEOUT, RETRY_ROUNDS, RequestCandidate,
    SourcePolicy, TransferContext, send,
};
use crate::utils::hash::HashAlgorithm;

pub const RANGE_THRESHOLD: u64 = 16 * 1024 * 1024;
pub const MIN_RANGE_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_FILE_RANGES: usize = 8;
const SLOW_SAMPLE_TIME: Duration = Duration::from_secs(2);
const SLOW_BYTES_PER_SECOND: u64 = 256 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default)]
pub struct FileHashes {
    pub sha1: Option<String>,
    pub sha512: Option<String>,
    pub sha256: Option<String>,
    pub md5: Option<String>,
}

impl FileHashes {
    fn entries(&self) -> [(HashAlgorithm, Option<&str>); 4] {
        [
            (HashAlgorithm::Sha1, self.sha1.as_deref()),
            (HashAlgorithm::Sha512, self.sha512.as_deref()),
            (HashAlgorithm::Sha256, self.sha256.as_deref()),
            (HashAlgorithm::Md5, self.md5.as_deref()),
        ]
    }
    fn has_hash(&self) -> bool {
        self.entries().iter().any(|(_, hash)| hash.is_some())
    }
}

#[derive(Clone)]
pub struct DownloadRequest {
    pub candidates: Vec<RequestCandidate>,
    pub destination: PathBuf,
    pub expected_size: Option<u64>,
    pub hashes: FileHashes,
    pub service: String,
}

#[derive(Clone, Debug)]
pub struct TransferProgress {
    pub transferred_bytes: u64,
    pub completed_bytes: u64,
    pub total_bytes: Option<u64>,
}

pub type ProgressCallback = Arc<dyn Fn(TransferProgress) + Send + Sync>;
pub type FileValidator = Arc<dyn Fn(&Path, &CancellationToken) -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
struct Validation {
    max_bytes: u64,
    validator: Option<FileValidator>,
}

#[derive(Debug)]
enum AttemptError {
    Remote(String),
    Local(String),
    RangeUnsupported,
}
impl std::fmt::Display for AttemptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Remote(message) | Self::Local(message) => f.write_str(message),
            Self::RangeUnsupported => f.write_str("Range not supported"),
        }
    }
}
impl From<std::io::Error> for AttemptError {
    fn from(error: std::io::Error) -> Self {
        Self::Local(error.to_string())
    }
}

struct TemporaryFile {
    path: PathBuf,
}
impl TemporaryFile {
    fn new(destination: &Path) -> Self {
        let id = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut name = destination.as_os_str().to_owned();
        name.push(format!(".{}.{}.part", std::process::id(), id));
        Self {
            path: PathBuf::from(name),
        }
    }
}
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct ReadyFile {
    temp: TemporaryFile,
    size: u64,
}

struct CancelAttempt(CancellationToken);
impl Drop for CancelAttempt {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[derive(Clone)]
struct ProgressState {
    transferred: Arc<AtomicU64>,
    callback_gate: Arc<std::sync::Mutex<()>>,
    callback: Option<ProgressCallback>,
    total: Option<u64>,
}
impl ProgressState {
    fn transferred(&self, bytes: u64) {
        let _guard = self.callback_gate.lock().unwrap_or_else(|e| e.into_inner());
        let transferred = self.transferred.fetch_add(bytes, Ordering::Relaxed) + bytes;
        if let Some(callback) = &self.callback {
            callback(TransferProgress {
                transferred_bytes: transferred,
                completed_bytes: 0,
                total_bytes: self.total,
            });
        }
    }
    fn complete(&self, bytes: u64) {
        let _guard = self.callback_gate.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(callback) = &self.callback {
            callback(TransferProgress {
                transferred_bytes: self.transferred.load(Ordering::Relaxed),
                completed_bytes: bytes,
                total_bytes: self.total.or(Some(bytes)),
            });
        }
    }
}

fn signal_slow(context: &TransferContext, sender: &mpsc::UnboundedSender<()>) {
    // 限速期间不能把本地主动等待当成远端慢速证据。
    if context.controller.rate.load(Ordering::Acquire) == 0 {
        let _ = sender.send(());
    }
}

async fn send_measured(
    client: &reqwest::Client,
    context: &TransferContext,
    candidate: &RequestCandidate,
    headers: HeaderMap,
    slow: &mpsc::UnboundedSender<()>,
) -> Result<reqwest::Response, AttemptError> {
    let request = send(client, candidate, headers, &context.cancel);
    tokio::pin!(request);
    tokio::select! {
        result = &mut request => result.map_err(AttemptError::Remote),
        _ = tokio::time::sleep(AUTO_RESPONSE_DELAY) => {
            let _ = slow.send(());
            request.await.map_err(AttemptError::Remote)
        }
    }
}

async fn write_response(
    mut response: reqwest::Response,
    file: &mut tokio::fs::File,
    expected: Option<u64>,
    context: &TransferContext,
    slow: &mpsc::UnboundedSender<()>,
    progress: &ProgressState,
    validation: &Validation,
) -> Result<u64, AttemptError> {
    if response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v != "identity")
    {
        return Err(AttemptError::Remote("文件响应不能使用内容压缩".into()));
    }
    let declared = response.content_length();
    if declared.is_some_and(|size| size > validation.max_bytes) {
        return Err(AttemptError::Remote("File exceeds size limit".into()));
    }
    if let (Some(expected), Some(declared)) = (expected, declared) {
        if expected != declared {
            return Err(AttemptError::Remote("响应长度与文件清单不一致".into()));
        }
    }
    let expected = expected.or(declared);
    let mut received = 0u64;
    let mut network_time = Duration::ZERO;
    loop {
        let read_started = Instant::now();
        let next = tokio::time::timeout(IDLE_TIMEOUT, response.chunk());
        tokio::pin!(next);
        let chunk = tokio::select! {
            _ = context.cancel.cancelled() => return Err(AttemptError::Remote("操作已取消".into())),
            result = &mut next => result,
            _ = tokio::time::sleep(if received == 0 { AUTO_RESPONSE_DELAY } else { SLOW_SAMPLE_TIME }) => {
                signal_slow(context, slow);
                tokio::select! {
                    _ = context.cancel.cancelled() => return Err(AttemptError::Remote("操作已取消".into())),
                    result = &mut next => result,
                }
            }
        }.map_err(|_| AttemptError::Remote("文件读取无进展，请重试".into()))?
            .map_err(|e| AttemptError::Remote(crate::net::client::describe_reqwest_error(&e)))?;
        network_time += read_started.elapsed();
        let Some(chunk) = chunk else {
            break;
        };
        received = received.saturating_add(chunk.len() as u64);
        context.record_transferred(chunk.len() as u64);
        progress.transferred(chunk.len() as u64);
        if received > validation.max_bytes || expected.is_some_and(|size| received > size) {
            return Err(AttemptError::Remote("响应体超过声明长度".into()));
        }
        let net_elapsed = network_time;
        if net_elapsed >= SLOW_SAMPLE_TIME
            && received as f64 / net_elapsed.as_secs_f64() < SLOW_BYTES_PER_SECOND as f64
        {
            signal_slow(context, slow);
        }
        context
            .controller
            .throttle(chunk.len(), &context.cancel)
            .await
            .map_err(AttemptError::Remote)?;
        file.write_all(&chunk).await?;
    }
    if expected.is_some_and(|size| size != received) {
        return Err(AttemptError::Remote("文件响应被截断".into()));
    }
    file.flush().await?;
    Ok(received)
}

fn content_range(
    response: &reqwest::Response,
    start: u64,
    end: u64,
    total: u64,
) -> Result<(), AttemptError> {
    if response.status() == reqwest::StatusCode::OK {
        return Err(AttemptError::RangeUnsupported);
    }
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(AttemptError::Remote("服务器未返回 206 分块响应".into()));
    }
    let expected = format!("bytes {start}-{end}/{total}");
    let actual = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok());
    if actual != Some(expected.as_str()) {
        return Err(AttemptError::Remote("Content-Range 与请求不匹配".into()));
    }
    if response
        .content_length()
        .is_some_and(|size| size != end - start + 1)
    {
        return Err(AttemptError::Remote("分块 Content-Length 不匹配".into()));
    }
    Ok(())
}

fn range_headers(
    start: u64,
    end: u64,
    etag: Option<&HeaderValue>,
) -> Result<HeaderMap, AttemptError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        RANGE,
        HeaderValue::from_str(&format!("bytes={start}-{end}"))
            .map_err(|e| AttemptError::Remote(e.to_string()))?,
    );
    if let Some(etag) = etag {
        headers.insert(IF_RANGE, etag.clone());
    }
    Ok(headers)
}

async fn validate_file(
    path: &Path,
    hashes: &FileHashes,
    cancel: &CancellationToken,
    temporary: bool,
    validation: &Validation,
) -> Result<(), AttemptError> {
    // 校验按块执行并检查取消，不让大文件哈希阻塞 async worker。
    let path = path.to_owned();
    let cleanup_path = path.clone();
    let cleanup_cancel = cancel.clone();
    let hashes = hashes.clone();
    let validation = validation.clone();
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || {
        let result = (|| {
            for (algorithm, expected) in hashes.entries() {
                cancel.check().map_err(AttemptError::Remote)?;
                if let Some(expected) = expected {
                    let actual =
                        crate::utils::hash::get_file_hash_cancellable(&path, algorithm, &cancel)
                            .map_err(AttemptError::Local)?;
                    if !actual.eq_ignore_ascii_case(expected) {
                        return Err(AttemptError::Remote("文件校验不一致，请重新下载".into()));
                    }
                }
            }
            if let Some(validator) = validation.validator {
                validator(&path, &cancel).map_err(AttemptError::Remote)?;
            }
            Ok(())
        })();
        // 败者 future 被 drop 后，阻塞哈希仍可能持有文件；关闭句柄后再清理。
        if temporary && cleanup_cancel.is_cancelled() {
            let _ = std::fs::remove_file(cleanup_path);
        }
        result
    })
    .await
    .map_err(|e| AttemptError::Local(e.to_string()))?
}

async fn candidate_attempt(
    client: &reqwest::Client,
    context: &TransferContext,
    request: &DownloadRequest,
    candidate: &RequestCandidate,
    slow: mpsc::UnboundedSender<()>,
    progress: &ProgressState,
    validation: &Validation,
) -> Result<ReadyFile, AttemptError> {
    let mut attempt_context = context.clone();
    attempt_context.cancel = context.cancel.child();
    let _guard = CancelAttempt(attempt_context.cancel.clone());
    let context = &attempt_context;
    match candidate_attempt_inner(
        client,
        context,
        request,
        candidate,
        slow.clone(),
        progress,
        validation,
        true,
    )
    .await
    {
        Err(AttemptError::RangeUnsupported) => {
            candidate_attempt_inner(
                client, context, request, candidate, slow, progress, validation, false,
            )
            .await
        }
        result => result,
    }
}

async fn candidate_attempt_inner(
    client: &reqwest::Client,
    context: &TransferContext,
    request: &DownloadRequest,
    candidate: &RequestCandidate,
    slow: mpsc::UnboundedSender<()>,
    progress: &ProgressState,
    validation: &Validation,
    allow_ranges: bool,
) -> Result<ReadyFile, AttemptError> {
    let temp = TemporaryFile::new(&request.destination);
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&temp.path)
        .await?;
    let permit = context
        .controller
        .acquire(&context.cancel)
        .await
        .map_err(AttemptError::Remote)?;
    let mut range_size = request
        .expected_size
        .filter(|size| allow_ranges && *size >= RANGE_THRESHOLD);
    let headers = if range_size.is_some() {
        range_headers(0, MIN_RANGE_BYTES - 1, None)?
    } else {
        HeaderMap::new()
    };
    let mut response = send_measured(client, context, candidate, headers, &slow).await?;
    if response
        .content_length()
        .is_some_and(|size| size > validation.max_bytes)
    {
        return Err(AttemptError::Remote("File exceeds size limit".into()));
    }
    if allow_ranges && range_size.is_none() && response.status() == reqwest::StatusCode::OK {
        if let Some(total) = response
            .content_length()
            .filter(|size| *size >= RANGE_THRESHOLD)
        {
            if request.hashes.has_hash()
                || response
                    .headers()
                    .get(ETAG)
                    .is_some_and(|tag| tag.to_str().is_ok_and(|s| !s.starts_with("W/")))
            {
                let etag = response.headers().get(ETAG).cloned();
                drop(response);
                response = send_measured(
                    client,
                    context,
                    candidate,
                    range_headers(0, MIN_RANGE_BYTES - 1, etag.as_ref())?,
                    &slow,
                )
                .await?;
                range_size = Some(total);
            }
        }
    }
    let size;
    if let Some(total) =
        range_size.filter(|_| response.status() == reqwest::StatusCode::PARTIAL_CONTENT)
    {
        if total > validation.max_bytes {
            return Err(AttemptError::Remote("File exceeds size limit".into()));
        }
        content_range(&response, 0, MIN_RANGE_BYTES - 1, total)?;
        let etag = response
            .headers()
            .get(ETAG)
            .filter(|value| value.to_str().is_ok_and(|s| !s.starts_with("W/")))
            .cloned();
        if etag.is_none() && !request.hashes.has_hash() {
            // 没有稳定文件身份，丢弃探测响应并改为单流；不会拼接不明实体。
            drop(response);
            let response =
                send_measured(client, context, candidate, HeaderMap::new(), &slow).await?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(AttemptError::Remote("完整文件响应必须为 200".into()));
            }
            size = write_response(
                response,
                &mut file,
                Some(total),
                context,
                &slow,
                progress,
                validation,
            )
            .await?;
            drop(permit);
        } else {
            file.set_len(total).await?;
            write_response(
                response,
                &mut file,
                Some(MIN_RANGE_BYTES),
                context,
                &slow,
                progress,
                validation,
            )
            .await?;
            drop(permit);
            let remaining = total - MIN_RANGE_BYTES;
            let count = (remaining / MIN_RANGE_BYTES).clamp(1, (MAX_FILE_RANGES - 1) as u64);
            let chunk_size = remaining / count;
            let mut parts = FuturesUnordered::new();
            for index in 0..count {
                let start = MIN_RANGE_BYTES + index * chunk_size;
                let end = if index + 1 == count {
                    total - 1
                } else {
                    start + chunk_size - 1
                };
                let etag = etag.clone();
                let path = &temp.path;
                let slow = slow.clone();
                parts.push(async move {
                    let _permit = context
                        .controller
                        .acquire(&context.cancel)
                        .await
                        .map_err(AttemptError::Remote)?;
                    let response = send_measured(
                        client,
                        context,
                        candidate,
                        range_headers(start, end, etag.as_ref())?,
                        &slow,
                    )
                    .await?;
                    content_range(&response, start, end, total)?;
                    if etag
                        .as_ref()
                        .is_some_and(|tag| response.headers().get(ETAG) != Some(tag))
                    {
                        return Err(AttemptError::Remote("分块文件 ETag 已改变".into()));
                    }
                    let mut part_file =
                        tokio::fs::OpenOptions::new().write(true).open(path).await?;
                    part_file.seek(std::io::SeekFrom::Start(start)).await?;
                    write_response(
                        response,
                        &mut part_file,
                        Some(end - start + 1),
                        context,
                        &slow,
                        progress,
                        validation,
                    )
                    .await?;
                    Ok::<(), AttemptError>(())
                });
            }
            while let Some(result) = parts.next().await {
                result?;
            }
            size = total;
        }
    } else {
        if response.status() != reqwest::StatusCode::OK {
            return Err(AttemptError::Remote("完整文件响应必须为 200".into()));
        }
        size = write_response(
            response,
            &mut file,
            request.expected_size,
            context,
            &slow,
            progress,
            validation,
        )
        .await?;
        drop(permit);
    }
    context.cancel.check().map_err(AttemptError::Remote)?;
    file.sync_all().await?;
    drop(file);
    validate_file(
        &temp.path,
        &request.hashes,
        &context.cancel,
        true,
        validation,
    )
    .await?;
    Ok(ReadyFile { temp, size })
}

async fn commit(
    ready: ReadyFile,
    request: &DownloadRequest,
    context: &TransferContext,
    progress: &ProgressState,
) -> Result<(), String> {
    context.cancel.check()?;
    tokio::fs::rename(&ready.temp.path, &request.destination)
        .await
        .map_err(|e| format!("提交下载文件失败：{e}"))?;
    progress.complete(ready.size);
    Ok(())
}

pub async fn download(
    client: &reqwest::Client,
    context: &TransferContext,
    request: &DownloadRequest,
    progress: Option<ProgressCallback>,
) -> Result<(), String> {
    let validator: Option<FileValidator> = request
        .destination
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| matches!(ext.to_ascii_lowercase().as_str(), "jar" | "zip" | "mrpack"))
        .map(|_| Arc::new(crate::utils::archive::validate_zip_file) as FileValidator);
    download_inner(
        client,
        context,
        request,
        progress,
        &Validation {
            max_bytes: u64::MAX,
            validator,
        },
    )
    .await
}

pub async fn download_validated(
    client: &reqwest::Client,
    context: &TransferContext,
    request: &DownloadRequest,
    max_bytes: u64,
    validator: FileValidator,
    progress: Option<ProgressCallback>,
) -> Result<(), String> {
    download_inner(
        client,
        context,
        request,
        progress,
        &Validation {
            max_bytes,
            validator: Some(validator),
        },
    )
    .await
}

async fn download_inner(
    client: &reqwest::Client,
    context: &TransferContext,
    request: &DownloadRequest,
    progress: Option<ProgressCallback>,
    validation: &Validation,
) -> Result<(), String> {
    context.cancel.check()?;
    if request
        .expected_size
        .is_some_and(|size| size > validation.max_bytes)
    {
        return Err("File exceeds size limit".into());
    }
    let progress = ProgressState {
        transferred: Arc::new(AtomicU64::new(0)),
        callback_gate: Arc::new(std::sync::Mutex::new(())),
        callback: progress,
        total: request.expected_size,
    };
    if request.hashes.has_hash() && request.destination.is_file() {
        if let Ok(metadata) = tokio::fs::metadata(&request.destination).await {
            if metadata.len() <= validation.max_bytes
                && request
                    .expected_size
                    .is_none_or(|size| metadata.len() == size)
                && validate_file(
                    &request.destination,
                    &request.hashes,
                    &context.cancel,
                    false,
                    validation,
                )
                .await
                .is_ok()
            {
                progress.complete(metadata.len());
                return Ok(());
            }
        }
    }
    if let Some(parent) = request
        .destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    let ordered = context.ordered(&request.candidates, &request.service, true);
    let Some(first) = ordered.first() else {
        return Err("没有可用的下载地址".into());
    };
    let mut failures = Vec::new();
    let mut allow_slow_switch = true;
    for round in 0..RETRY_ROUNDS {
        context.cancel.check()?;
        if round > 0 {
            tokio::select! {
                _ = context.cancel.cancelled() => return Err("操作已取消".into()),
                _ = tokio::time::sleep(Duration::from_millis(250 * round as u64)) => {},
            }
        }
        let (slow_tx, mut slow_rx) = mpsc::unbounded_channel();
        let mut first_attempt = Box::pin(candidate_attempt(
            client, context, request, first, slow_tx, &progress, validation,
        ));
        let auto = allow_slow_switch
            && context.file_policy == SourcePolicy::OfficialAuto
            && !first.is_mirror
            && ordered.len() > 1;
        if auto {
            tokio::select! {
                result = &mut first_attempt => match result {
                    Ok(ready) => return commit(ready, request, context, &progress).await,
                    Err(AttemptError::Local(error)) => return Err(error),
                    Err(error) => record_failure(&mut failures, first, error),
                },
                Some(_) = slow_rx.recv() => {
                    let saturated = {
                        let state = context.controller.state.lock().unwrap_or_else(|e| e.into_inner());
                        state.active >= state.limit
                    };
                    if saturated {
                        // 预算满时先释放慢流的连接，候选切换不能突破用户的总并发限制。
                        drop(first_attempt);
                        allow_slow_switch = false;
                        for candidate in ordered.iter().skip(1) {
                            let (slow, _) = mpsc::unbounded_channel();
                            match candidate_attempt(client, context, request, candidate, slow, &progress, validation).await {
                                Ok(ready) => { if candidate.is_mirror { context.remember_mirror(&request.service, true); } return commit(ready, request, context, &progress).await; },
                                Err(AttemptError::Local(error)) => return Err(error),
                                Err(error) => record_failure(&mut failures, candidate, error),
                            }
                        }
                        // 镜像全部失败后恢复官方，让它按空闲超时完成；同文件不再因测速阈值反复取消。
                        let (slow, _) = mpsc::unbounded_channel();
                        match candidate_attempt(client, context, request, first, slow, &progress, validation).await {
                            Ok(ready) => return commit(ready, request, context, &progress).await,
                            Err(AttemptError::Local(error)) => return Err(error),
                            Err(error) => record_failure(&mut failures, first, error),
                        }
                        continue;
                    }
                    let mut alternatives = FuturesUnordered::new();
                    for candidate in ordered.iter().skip(1) {
                        let (slow, _) = mpsc::unbounded_channel();
                        let progress = &progress;
                        alternatives.push(async move { (*candidate, candidate_attempt(client, context, request, candidate, slow, progress, validation).await) });
                    }
                    loop {
                        tokio::select! {
                            result = &mut first_attempt => {
                                match result {
                                    Ok(ready) => {
                                        drop(alternatives);
                                        return commit(ready, request, context, &progress).await;
                                    },
                                    Err(AttemptError::Local(error)) => return Err(error),
                                    Err(error) => record_failure(&mut failures, first, error),
                                }
                                break;
                            },
                            alternative = alternatives.next(), if !alternatives.is_empty() => {
                                if let Some((candidate, result)) = alternative {
                                    match result {
                                        Ok(ready) => {
                                            if candidate.is_mirror { context.remember_mirror(&request.service, true); }
                                            drop(first_attempt);
                                            drop(alternatives);
                                            return commit(ready, request, context, &progress).await;
                                        },
                                        Err(AttemptError::Local(error)) => return Err(error),
                                        Err(error) => record_failure(&mut failures, candidate, error),
                                    }
                                }
                            },
                        }
                    }
                    while let Some((candidate, result)) = alternatives.next().await {
                        match result {
                            Ok(ready) => { if candidate.is_mirror { context.remember_mirror(&request.service, true); } return commit(ready, request, context, &progress).await; },
                            Err(AttemptError::Local(error)) => return Err(error),
                            Err(error) => record_failure(&mut failures, candidate, error),
                        }
                    }
                    continue;
                }
            }
        } else {
            match first_attempt.await {
                Ok(ready) => return commit(ready, request, context, &progress).await,
                Err(AttemptError::Local(error)) => return Err(error),
                Err(error) => record_failure(&mut failures, first, error),
            }
        }
        for candidate in ordered.iter().skip(1) {
            context.cancel.check()?;
            let (slow, _) = mpsc::unbounded_channel();
            match candidate_attempt(
                client, context, request, candidate, slow, &progress, validation,
            )
            .await
            {
                Ok(ready) => return commit(ready, request, context, &progress).await,
                Err(AttemptError::Local(error)) => return Err(error),
                Err(error) => record_failure(&mut failures, candidate, error),
            }
        }
    }
    context.cancel.check()?;
    Err(failures.join("; "))
}

fn record_failure(failures: &mut Vec<String>, candidate: &RequestCandidate, error: AttemptError) {
    let failure = format!("{}: {error}", candidate.diagnostic_label());
    if !failures.contains(&failure) {
        failures.push(failure);
    }
}

#[cfg(test)]
mod byte_metrics_tests {
    use super::*;
    use sha2::Digest;
    use std::sync::Mutex;
    use tokio::io::AsyncReadExt;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn context() -> TransferContext {
        let mut context = TransferContext::new(
            SourcePolicy::OfficialFirst,
            SourcePolicy::OfficialFirst,
            CancellationToken::new(),
        );
        context.controller = Arc::new(super::super::TransferController::new(4, 0));
        context
    }

    #[tokio::test]
    async fn real_network_bytes_include_retries_and_parallel_children_but_not_cache() {
        let server = MockServer::start().await;
        Mock::given(path("/bad"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"bad"))
            .mount(&server)
            .await;
        Mock::given(path("/good"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"yes"))
            .mount(&server)
            .await;
        let directory = tempfile::TempDir::new().unwrap();
        let request = DownloadRequest {
            candidates: vec![
                RequestCandidate::official(format!("{}/bad", server.uri())),
                RequestCandidate::mirror(format!("{}/good", server.uri())),
            ],
            destination: directory.path().join("download"),
            expected_size: Some(3),
            hashes: FileHashes {
                sha256: Some(format!("{:x}", sha2::Sha256::digest(b"yes"))),
                ..FileHashes::default()
            },
            service: "byte-metrics".into(),
        };
        let parent = context();
        let child = parent.child();
        let client = reqwest::Client::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        download(
            &client,
            &child,
            &request,
            Some(Arc::new(move |progress| {
                captured.lock().unwrap().push(progress)
            })),
        )
        .await
        .unwrap();
        assert_eq!(parent.transferred_bytes(), 6);
        assert_eq!(child.transferred_bytes(), 6);
        assert_eq!(std::fs::read(&request.destination).unwrap(), b"yes");
        {
            let events = events.lock().unwrap();
            assert_eq!(events.last().unwrap().transferred_bytes, 6);
            assert_eq!(events.last().unwrap().completed_bytes, 3);
            assert!(
                events
                    .iter()
                    .any(|event| event.transferred_bytes > 0 && event.completed_bytes == 0)
            );
        }
        let cached = context();
        download(&client, &cached, &request, None).await.unwrap();
        assert_eq!(cached.transferred_bytes(), 0);
        assert_eq!(parent.transferred_bytes(), 6);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);

        let mut first = request.clone();
        first.candidates.remove(0);
        first.destination = directory.path().join("parallel-first");
        let mut second = first.clone();
        second.destination = directory.path().join("parallel-second");
        let sibling = parent.child();
        let (a, b) = tokio::join!(
            download(&client, &child, &first, None),
            download(&client, &sibling, &second, None)
        );
        a.unwrap();
        b.unwrap();
        assert_eq!(parent.transferred_bytes(), 12);
        assert_eq!(child.transferred_bytes(), 12);
        assert_eq!(sibling.transferred_bytes(), 12);
        assert_eq!(server.received_requests().await.unwrap().len(), 4);
        eprintln!("[transfer-bytes] retry=6; cache=0; parallel_children=6; parent_total=12");
    }

    #[tokio::test]
    async fn real_stream_bytes_are_visible_before_completion_and_cancel_cleans_partial_file() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let url = format!("http://{}/stream", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).await.unwrap();
                headers.push(byte[0]);
                assert!(headers.len() < 8192);
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 65536\r\nConnection: close\r\n\r\nfirst-chunk")
                .await
                .unwrap();
            let mut byte = [0];
            tokio::time::timeout(Duration::from_secs(5), socket.read(&mut byte))
                .await
                .unwrap()
        });
        let directory = tempfile::TempDir::new().unwrap();
        let destination = directory.path().join("stream");
        let request = DownloadRequest {
            candidates: vec![RequestCandidate::official(url)],
            destination: destination.clone(),
            expected_size: Some(65536),
            hashes: FileHashes::default(),
            service: "stream-metrics".into(),
        };
        let parent = context();
        let stream_context = parent.child();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let received = Arc::new(tokio::sync::Notify::new());
        let notify = received.clone();
        let download = tokio::spawn(async move {
            download(
                &reqwest::Client::new(),
                &stream_context,
                &request,
                Some(Arc::new(move |progress| {
                    let complete_chunk = progress.transferred_bytes >= b"first-chunk".len() as u64;
                    captured.lock().unwrap().push(progress);
                    if complete_chunk {
                        notify.notify_one();
                    }
                })),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(5), received.notified())
            .await
            .unwrap();
        assert_eq!(parent.transferred_bytes(), b"first-chunk".len() as u64);
        assert!(
            events
                .lock()
                .unwrap()
                .iter()
                .all(|event| event.completed_bytes == 0)
        );
        assert!(!destination.exists());
        parent.cancellation().cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), download)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_err());
        assert!(matches!(server.await.unwrap(), Ok(0) | Err(_)));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        eprintln!(
            "[transfer-bytes] streaming=11; completed=0; cancelled_socket_closed; partial_files=0"
        );
    }
}
