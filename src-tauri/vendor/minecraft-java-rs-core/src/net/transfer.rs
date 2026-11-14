//! 操作级源快照、可取消请求与进程级传输预算。

use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use reqwest::header::{AUTHORIZATION, HeaderMap};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::sync::{Mutex as AsyncMutex, Notify};

mod file;
pub use file::{
    DownloadRequest, FileHashes, FileValidator, ProgressCallback, TransferProgress, download,
    download_validated,
};

pub const DEFAULT_MAX_THREADS: u32 = 10;
pub const MAX_THREADS: u32 = 64;
pub const AUTO_RESPONSE_DELAY: Duration = Duration::from_secs(4);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_LIST_LIMIT: usize = 32 * 1024 * 1024;
const RETRY_ROUNDS: usize = 3;
const RATE_SLICE_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcePolicy {
    MirrorFirst,
    #[default]
    OfficialAuto,
    OfficialFirst,
}

#[derive(Default, Debug)]
struct CancellationState {
    cancelled: AtomicBool,
    notify: Notify,
    parents: Vec<Arc<CancellationState>>,
}

#[derive(Clone, Default, Debug)]
pub struct CancellationToken(Arc<CancellationState>);

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.0.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
            || self
                .0
                .parents
                .iter()
                .any(|parent| parent.cancelled.load(Ordering::Acquire))
    }

    fn child(&self) -> Self {
        let mut parents = self.0.parents.clone();
        parents.push(self.0.clone());
        Self(Arc::new(CancellationState {
            parents,
            ..Default::default()
        }))
    }

    pub fn check(&self) -> Result<(), String> {
        if self.is_cancelled() {
            Err("操作已取消".into())
        } else {
            Ok(())
        }
    }

    pub async fn cancelled(&self) {
        let mut signals = FuturesUnordered::new();
        for state in std::iter::once(&self.0).chain(self.0.parents.iter()) {
            signals.push(async move {
                loop {
                    let notified = state.notify.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    if state.cancelled.load(Ordering::Acquire) {
                        return;
                    }
                    notified.await;
                }
            });
        }
        signals.next().await;
    }
}

struct BudgetState {
    active: u32,
    limit: u32,
}

pub(super) struct TransferController {
    state: Mutex<BudgetState>,
    admission: AsyncMutex<()>,
    changed: Notify,
    rate: AtomicU64,
    rate_gate: AsyncMutex<()>,
    rate_changed: Notify,
}

impl TransferController {
    fn new(limit: u32, rate: u64) -> Self {
        Self {
            state: Mutex::new(BudgetState {
                active: 0,
                limit: limit.clamp(1, MAX_THREADS),
            }),
            admission: AsyncMutex::new(()),
            changed: Notify::new(),
            rate: AtomicU64::new(rate),
            rate_gate: AsyncMutex::new(()),
            rate_changed: Notify::new(),
        }
    }

    fn configure(&self, limit: u32, rate: u64) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).limit = limit.clamp(1, MAX_THREADS);
        self.rate.store(rate, Ordering::Release);
        self.changed.notify_waiters();
        self.rate_changed.notify_waiters();
    }

    pub(super) async fn acquire(
        self: &Arc<Self>,
        cancel: &CancellationToken,
    ) -> Result<TransferPermit, String> {
        let _queue = tokio::select! {
            _ = cancel.cancelled() => return Err("操作已取消".into()),
            guard = self.admission.lock() => guard,
        };
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            cancel.check()?;
            {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.active < state.limit {
                    state.active += 1;
                    return Ok(TransferPermit(self.clone()));
                }
            }
            tokio::select! {
                _ = cancel.cancelled() => return Err("操作已取消".into()),
                _ = notified => {},
            }
        }
    }

    pub(super) async fn throttle(
        &self,
        bytes: usize,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        for slice in (0..bytes).step_by(RATE_SLICE_BYTES) {
            cancel.check()?;
            if self.rate.load(Ordering::Acquire) == 0 {
                return Ok(());
            }
            let _queue = tokio::select! {
                _ = cancel.cancelled() => return Err("操作已取消".into()),
                guard = self.rate_gate.lock() => guard,
            };
            let amount = (bytes - slice).min(RATE_SLICE_BYTES);
            loop {
                let notified = self.rate_changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let rate = self.rate.load(Ordering::Acquire);
                if rate == 0 {
                    break;
                }
                let delay = Duration::from_secs_f64(amount as f64 / rate as f64);
                tokio::select! {
                    _ = cancel.cancelled() => return Err("操作已取消".into()),
                    _ = tokio::time::sleep(delay) => break,
                    _ = notified => {},
                }
            }
        }
        Ok(())
    }
}

pub(super) struct TransferPermit(Arc<TransferController>);
impl Drop for TransferPermit {
    fn drop(&mut self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active -= 1;
        self.0.changed.notify_waiters();
    }
}

fn controller() -> Arc<TransferController> {
    static CONTROLLER: OnceLock<Arc<TransferController>> = OnceLock::new();
    CONTROLLER
        .get_or_init(|| Arc::new(TransferController::new(DEFAULT_MAX_THREADS, 0)))
        .clone()
}
pub fn configure_transfer_limits(max_threads: u32, bytes_per_second: u64) {
    controller().configure(max_threads, bytes_per_second);
}

/// 安装器/处理器取消时终止并回收子进程；Command 同时应设置 kill_on_drop(true)。
pub async fn wait_child(
    child: &mut tokio::process::Child,
    cancel: &CancellationToken,
) -> Result<std::process::ExitStatus, String> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            child.kill().await.map_err(|e| format!("终止安装进程失败：{e}"))?;
            Err("操作已取消".into())
        },
        result = child.wait() => result.map_err(|e| e.to_string()),
    }
}

#[derive(Clone)]
pub struct TransferContext {
    list_policy: SourcePolicy,
    file_policy: SourcePolicy,
    cancel: CancellationToken,
    controller: Arc<TransferController>,
    list_mirrors: Arc<Mutex<HashSet<String>>>,
    file_mirrors: Arc<Mutex<HashSet<String>>>,
    transferred: Arc<AtomicU64>,
}

tokio::task_local! { static CURRENT_CONTEXT: TransferContext; }

impl TransferContext {
    pub fn new(
        list_policy: SourcePolicy,
        file_policy: SourcePolicy,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            list_policy,
            file_policy,
            cancel,
            controller: controller(),
            list_mirrors: Arc::new(Mutex::new(HashSet::new())),
            file_mirrors: Arc::new(Mutex::new(HashSet::new())),
            transferred: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn current() -> Self {
        CURRENT_CONTEXT.try_with(Clone::clone).unwrap_or_else(|_| {
            let policy = if crate::mirror::is_mirror() {
                SourcePolicy::MirrorFirst
            } else {
                SourcePolicy::OfficialFirst
            };
            Self::new(policy, policy, CancellationToken::new())
        })
    }

    /// 子操作共享源测速结果、字节统计与预算；取消子操作不会取消父操作。
    pub fn child(&self) -> Self {
        let mut child = self.clone();
        child.cancel = self.cancel.child();
        child
    }

    /// 实际文件流量，包含分块、候选回退和重试，不包含磁盘缓存复用。
    pub fn transferred_bytes(&self) -> u64 {
        self.transferred.load(Ordering::Relaxed)
    }
    pub(super) fn record_transferred(&self, bytes: u64) {
        self.transferred.fetch_add(bytes, Ordering::Relaxed);
    }
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancel
    }
    pub fn list_policy(&self) -> SourcePolicy {
        self.list_policy
    }
    pub fn file_policy(&self) -> SourcePolicy {
        self.file_policy
    }

    pub async fn scope<F: Future>(&self, future: F) -> F::Output {
        CURRENT_CONTEXT.scope(self.clone(), future).await
    }

    fn ordered<'a>(
        &self,
        candidates: &'a [RequestCandidate],
        service: &str,
        file: bool,
    ) -> Vec<&'a RequestCandidate> {
        let policy = if file {
            self.file_policy
        } else {
            self.list_policy
        };
        let measured = if file {
            &self.file_mirrors
        } else {
            &self.list_mirrors
        };
        let mirror_first = policy == SourcePolicy::MirrorFirst
            || (policy == SourcePolicy::OfficialAuto
                && measured
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains(service));
        let mut ordered: Vec<_> = candidates.iter().collect();
        ordered.sort_by_key(|candidate| candidate.is_mirror != mirror_first);
        ordered.dedup_by(|a, b| a.url == b.url);
        ordered
    }

    fn remember_mirror(&self, service: &str, file: bool) {
        let measured = if file {
            &self.file_mirrors
        } else {
            &self.list_mirrors
        };
        measured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(service.to_owned());
    }
}

#[derive(Clone)]
pub struct RequestCandidate {
    pub url: String,
    pub is_mirror: bool,
    pub headers: HeaderMap,
}

impl RequestCandidate {
    pub fn official(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            is_mirror: false,
            headers: HeaderMap::new(),
        }
    }
    pub fn mirror(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            is_mirror: true,
            headers: HeaderMap::new(),
        }
    }

    pub(crate) fn diagnostic_label(&self) -> String {
        // 只记录来源，不输出 URL 凭据、签名参数、片段或请求头。
        let origin = reqwest::Url::parse(&self.url)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .map(|url| url.origin().ascii_serialization())
            .unwrap_or_else(|| "invalid-url".into());
        let kind = if self.is_mirror { "mirror" } else { "official" };
        format!("{kind} {origin}")
    }
}

pub fn core_candidates(url: &str) -> Vec<RequestCandidate> {
    let official = crate::mirror::official_urls(url);
    if !official.is_empty() {
        let mut candidates: Vec<_> = official
            .into_iter()
            .map(RequestCandidate::official)
            .collect();
        candidates.push(RequestCandidate::mirror(url));
        return candidates;
    }
    let mirror = crate::mirror::mirror_url(url);
    let mut candidates = vec![RequestCandidate::official(url)];
    if mirror != url {
        candidates.push(RequestCandidate::mirror(mirror));
    }
    candidates
}

pub(super) async fn send(
    client: &reqwest::Client,
    candidate: &RequestCandidate,
    extra_headers: HeaderMap,
    cancel: &CancellationToken,
) -> Result<reqwest::Response, String> {
    cancel.check()?;
    let parsed = reqwest::Url::parse(&candidate.url).map_err(|_| "下载地址无效".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("下载地址必须使用 HTTP 或 HTTPS，且不能包含凭据".into());
    }
    let sensitive = candidate.headers.contains_key(AUTHORIZATION)
        || candidate.headers.contains_key("x-api-key");
    if sensitive && candidate.is_mirror {
        return Err("镜像请求不能携带认证凭据".into());
    }
    // 带平台凭据的请求不跟随重定向，避免自定义 Key 泄漏到新域。
    let strict_client;
    let client = if sensitive {
        strict_client = reqwest::Client::builder()
            .use_rustls_tls()
            .tls_built_in_root_certs(false)
            .tls_built_in_webpki_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(IDLE_TIMEOUT)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .build()
            .map_err(|e| e.to_string())?;
        &strict_client
    } else {
        client
    };
    let request = client
        .get(parsed)
        .headers(candidate.headers.clone())
        .headers(extra_headers)
        .header(reqwest::header::ACCEPT_ENCODING, "identity");
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err("操作已取消".into()),
        response = tokio::time::timeout(IDLE_TIMEOUT, request.send()) =>
            response.map_err(|_| "连接超时，请重试".to_string())?
                .map_err(|e| crate::net::client::describe_reqwest_error(&e))?,
    };
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    Ok(response)
}

async fn list_attempt<T, V: Fn(&[u8]) -> Result<T, String>>(
    client: &reqwest::Client,
    context: &TransferContext,
    candidate: &RequestCandidate,
    max_bytes: usize,
    validate: &V,
) -> Result<T, String> {
    let mut response = send(client, candidate, HeaderMap::new(), &context.cancel).await?;
    if response
        .content_length()
        .is_some_and(|len| len > max_bytes as u64)
    {
        return Err("清单超过大小限制".into());
    }
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = context.cancel.cancelled() => return Err("操作已取消".into()),
            chunk = tokio::time::timeout(IDLE_TIMEOUT, response.chunk()) =>
                chunk.map_err(|_| "读取清单超时，请重试".to_string())?.map_err(|e| e.to_string())?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err("清单超过大小限制".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    context.cancel.check()?;
    validate(&bytes)
}

pub async fn fetch_validated_candidates<T, V: Fn(&[u8]) -> Result<T, String>>(
    client: &reqwest::Client,
    context: &TransferContext,
    service: &str,
    candidates: &[RequestCandidate],
    max_bytes: usize,
    validate: V,
) -> Result<T, String> {
    let ordered = context.ordered(candidates, service, false);
    let Some(first) = ordered.first() else {
        return Err("没有可用的请求地址".into());
    };
    let mut last_error = String::new();
    for round in 0..RETRY_ROUNDS {
        context.cancel.check()?;
        if round > 0 {
            tokio::select! {
                _ = context.cancel.cancelled() => return Err("操作已取消".into()),
                _ = tokio::time::sleep(Duration::from_millis(250 * round as u64)) => {},
            }
        }
        let first_request = list_attempt(client, context, first, max_bytes, &validate);
        tokio::pin!(first_request);
        let race = context.list_policy == SourcePolicy::OfficialAuto
            && !first.is_mirror
            && ordered.len() > 1;
        if race {
            tokio::select! {
                result = &mut first_request => match result {
                    Ok(value) => return Ok(value),
                    Err(error) => last_error = error,
                },
                _ = tokio::time::sleep(AUTO_RESPONSE_DELAY) => {
                    let mut alternatives = FuturesUnordered::new();
                    for candidate in ordered.iter().skip(1) {
                        let validate = &validate;
                        alternatives.push(async move { (candidate.is_mirror, list_attempt(client, context, candidate, max_bytes, validate).await) });
                    }
                    loop {
                        tokio::select! {
                            result = &mut first_request => {
                                match result { Ok(value) => return Ok(value), Err(error) => last_error = error }
                                break;
                            },
                            alternative = alternatives.next(), if !alternatives.is_empty() => {
                                if let Some((mirror, result)) = alternative {
                                    match result {
                                        Ok(value) => { if mirror { context.remember_mirror(service, false); } return Ok(value); },
                                        Err(_) => {}, // 官方仍在途，最终错误由最后失败的请求提供。
                                    }
                                }
                            },
                        }
                    }
                    while let Some((mirror, result)) = alternatives.next().await {
                        match result {
                            Ok(value) => { if mirror { context.remember_mirror(service, false); } return Ok(value); },
                            Err(error) => last_error = error,
                        }
                    }
                    continue;
                }
            }
        } else {
            match first_request.await {
                Ok(value) => return Ok(value),
                Err(error) => last_error = error,
            }
        }
        for candidate in ordered.iter().skip(1) {
            context.cancel.check()?;
            match list_attempt(client, context, candidate, max_bytes, &validate).await {
                Ok(value) => return Ok(value),
                Err(error) => last_error = error,
            }
        }
    }
    context.cancel.check()?;
    Err(last_error)
}

pub async fn fetch_json_candidates<T: DeserializeOwned>(
    client: &reqwest::Client,
    context: &TransferContext,
    service: &str,
    candidates: &[RequestCandidate],
    max_bytes: usize,
) -> Result<T, String> {
    fetch_validated_candidates(client, context, service, candidates, max_bytes, |bytes| {
        serde_json::from_slice(bytes).map_err(|error| format!("清单 JSON 无效：{error}"))
    })
    .await
}

pub async fn fetch_text_candidates(
    client: &reqwest::Client,
    context: &TransferContext,
    service: &str,
    candidates: &[RequestCandidate],
    max_bytes: usize,
) -> Result<String, String> {
    fetch_validated_candidates(client, context, service, candidates, max_bytes, |bytes| {
        String::from_utf8(bytes.to_vec()).map_err(|_| "清单不是有效 UTF-8".into())
    })
    .await
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "transfer_socket_tests.rs"]
mod socket_tests;
