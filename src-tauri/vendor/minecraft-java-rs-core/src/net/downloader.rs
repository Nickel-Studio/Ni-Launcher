use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{stream, StreamExt};
use crate::error::LaunchError;
use crate::launcher::events::LaunchEvent;
use crate::net::transfer::{self, DownloadRequest, FileHashes, ProgressCallback, TransferContext};

#[derive(Debug, Clone)]
pub struct DownloadItem {
    pub url: String,
    pub path: PathBuf,
    pub folder: PathBuf,
    pub name: String,
    pub size: u64,
    pub r#type: Option<String>,
    pub sha1: Option<String>,
}

pub struct Downloader {
    client: Result<reqwest::Client, String>,
    concurrency: usize,
    context: TransferContext,
}

impl Downloader {
    pub fn new(timeout_secs: u64, concurrency: u32, force_ipv4: bool, dns: Option<std::net::IpAddr>) -> Self {
        Self::with_context(timeout_secs, concurrency, force_ipv4, dns, TransferContext::current())
    }

    pub fn with_context(timeout_secs: u64, concurrency: u32, force_ipv4: bool, dns: Option<std::net::IpAddr>, context: TransferContext) -> Self {
        Self {
            client: crate::net::client::build_transfer_client(timeout_secs, force_ipv4, dns).map_err(|e| e.to_string()),
            concurrency: adaptive_concurrency(concurrency), context,
        }
    }

    pub async fn download_file(&self, item: &DownloadItem) -> Result<(), LaunchError> {
        self.download_item(item, None).await
    }

    async fn download_item(&self, item: &DownloadItem, progress: Option<ProgressCallback>) -> Result<(), LaunchError> {
        let request = DownloadRequest {
            candidates: transfer::core_candidates(&item.url), destination: item.path.clone(),
            expected_size: (item.size > 0).then_some(item.size),
            hashes: FileHashes { sha1: item.sha1.clone(), ..FileHashes::default() },
            service: crate::net::http::service_key(&item.url),
        };
        let describe = |error: String| {
            if self.context.cancellation().is_cancelled() {
                return LaunchError::InvalidData("操作已取消".into());
            }
            let candidates = request.candidates.iter().map(|candidate| candidate.diagnostic_label()).collect::<Vec<_>>().join(", ");
            LaunchError::InvalidData(format!("下载失败：{}；目标：{}；策略：{:?}；候选：[{}]；原因：{}",
                item.name, item.path.display(), self.context.file_policy(), candidates, error))
        };
        let client = self.client.as_ref().map_err(|error| describe(error.clone()))?;
        transfer::download(client, &self.context, &request, progress).await.map_err(describe)
    }

    pub async fn download_multiple(&self, items: Vec<DownloadItem>, event_tx: tokio::sync::mpsc::Sender<LaunchEvent>) -> Result<(), LaunchError> {
        if items.is_empty() { return Ok(()); }
        let total_bytes: u64 = items.iter().map(|item| item.size).sum();
        let total_count = items.len() as u64;
        let transferred = Arc::new(AtomicU64::new(0));
        let completed_bytes = Arc::new(AtomicU64::new(0));
        // 不 spawn 隐式丢失 task-local；每个请求持有构造时的 context 快照。
        let mut pending = stream::iter(items.into_iter().map(|item| {
            let transferred = transferred.clone();
            let completed_bytes = completed_bytes.clone();
            async move {
                let previous = Arc::new(AtomicU64::new(0));
                let callback: ProgressCallback = Arc::new(move |progress| {
                    let old = previous.swap(progress.transferred_bytes, Ordering::Relaxed);
                    transferred.fetch_add(progress.transferred_bytes.saturating_sub(old), Ordering::Relaxed);
                    if progress.completed_bytes > 0 { completed_bytes.fetch_add(progress.completed_bytes, Ordering::Relaxed); }
                });
                self.download_item(&item, Some(callback)).await?;
                Ok::<_, LaunchError>(item)
            }
        })).buffer_unordered(self.concurrency);
        let mut done = 0u64;
        let mut ticks = tokio::time::interval(Duration::from_millis(250));
        let mut window = VecDeque::from([(Instant::now(), 0u64)]);
        loop {
            tokio::select! {
                _ = self.context.cancellation().cancelled() => return Err(LaunchError::InvalidData("操作已取消".into())),
                result = pending.next() => {
                    let Some(result) = result else { break; };
                    let item = result?;
                    done += 1;
                    let _ = event_tx.try_send(LaunchEvent::Progress { downloaded: done, total: total_count, kind: item.r#type.unwrap_or_default() });
                },
                _ = ticks.tick() => {
                    let now = Instant::now();
                    let bytes = transferred.load(Ordering::Relaxed);
                    window.push_back((now, bytes));
                    while window.len() > 2 && window.front().is_some_and(|(time, _)| now.duration_since(*time) > Duration::from_secs(5)) { window.pop_front(); }
                    if let Some((time, previous)) = window.front() {
                        let elapsed = now.duration_since(*time).as_secs_f64();
                        if elapsed > 0.1 {
                            let speed = bytes.saturating_sub(*previous) as f64 / elapsed;
                            let _ = event_tx.try_send(LaunchEvent::Speed(speed));
                            if speed > 0.0 && total_bytes > 0 {
                                let remaining = total_bytes.saturating_sub(completed_bytes.load(Ordering::Relaxed));
                                let _ = event_tx.try_send(LaunchEvent::Estimated(remaining as f64 / speed));
                            }
                        }
                    }
                },
            }
        }
        Ok(())
    }

    pub async fn check_url(&self, url: &str) -> bool {
        let Ok(client) = &self.client else { return false; };
        for candidate in transfer::core_candidates(url) {
            let result = tokio::select! {
                _ = self.context.cancellation().cancelled() => return false,
                result = client.head(&candidate.url).send() => result,
            };
            if result.is_ok_and(|response| response.status().is_success()) { return true; }
        }
        false
    }

    pub async fn check_mirror(&self, mirrors: &[&str], path: &str) -> Option<String> {
        for mirror in mirrors {
            let url = format!("{}/{}", mirror.trim_end_matches('/'), path.trim_start_matches('/'));
            if self.check_url(&url).await { return Some(url); }
        }
        None
    }
}

fn adaptive_concurrency(requested: u32) -> usize {
    requested.clamp(1, transfer::MAX_THREADS) as usize
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    fn make_downloader() -> Downloader {
        Downloader::new(5, 4, false, None)
    }

    #[test]
    fn adaptive_concurrency_clamps_high_value() {
        // Whatever the CPU count is, 400 must be reduced.
        assert!(adaptive_concurrency(400) <= 64);
    }

    #[test]
    fn adaptive_concurrency_preserves_low_value() {
        assert_eq!(adaptive_concurrency(2), 2);
        assert_eq!(adaptive_concurrency(1), 1);
    }

    #[test]
    fn adaptive_concurrency_floors_at_one() {
        assert_eq!(adaptive_concurrency(0), 1);
    }

    #[tokio::test]
    async fn download_multiple_empty_list() {
        let d = make_downloader();
        let (tx, _rx) = mpsc::channel(16);
        d.download_multiple(vec![], tx).await.unwrap();
    }

    #[tokio::test]
    async fn download_file_bad_url_returns_error() {
        let dir = TempDir::new().unwrap();
        let item = DownloadItem {
            url: "http://127.0.0.1:1/nonexistent".into(),
            path: dir.path().join("out.bin"),
            folder: dir.path().to_path_buf(),
            name: "out.bin".into(),
            size: 0,
            r#type: None,
            sha1: None,
        };
        let d = Downloader::new(1, 1, false, None); // 1-second timeout
        let result = d.download_file(&item).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn check_url_unreachable_returns_false() {
        let d = Downloader::new(1, 1, false, None);
        assert!(!d.check_url("http://127.0.0.1:1/test").await);
    }

    #[tokio::test]
    async fn check_mirror_all_bad_returns_none() {
        let d = Downloader::new(1, 1, false, None);
        let result = d
            .check_mirror(&["http://127.0.0.1:1"], "/some/path.jar")
            .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn download_multiple_bad_url_propagates_error() {
        let dir = TempDir::new().unwrap();
        let item = DownloadItem {
            url: "http://127.0.0.1:1/nonexistent?token=private-query#private-fragment".into(),
            path: dir.path().join("out.bin"),
            folder: dir.path().to_path_buf(),
            name: "out.bin".into(),
            size: 0,
            r#type: Some("test".into()),
            sha1: None,
        };
        let destination = item.path.display().to_string();
        let context = TransferContext::new(transfer::SourcePolicy::OfficialFirst, transfer::SourcePolicy::OfficialFirst, transfer::CancellationToken::new());
        let d = Downloader::with_context(1, 1, false, None, context);
        let (tx, _rx) = mpsc::channel(16);
        let error = d.download_multiple(vec![item], tx).await.unwrap_err().to_string();
        assert!(error.contains("out.bin"));
        assert!(error.contains(&destination));
        assert!(error.contains("OfficialFirst"));
        assert!(error.contains("official http://127.0.0.1:1"));
        assert!(!error.contains("private-query"));
        assert!(!error.contains("private-fragment"));
    }

    #[tokio::test]
    async fn no_tmp_file_left_after_failed_download() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.bin");
        let item = DownloadItem {
            url: "http://127.0.0.1:1/nonexistent".into(),
            path: path.clone(),
            folder: dir.path().to_path_buf(),
            name: "out.bin".into(),
            size: 0,
            r#type: None,
            sha1: None,
        };
        let d = Downloader::new(1, 1, false, None);
        let _ = d.download_file(&item).await;

        let tmp = {
            let mut s = path.as_os_str().to_owned();
            s.push(".tmp");
            PathBuf::from(s)
        };
        assert!(
            !tmp.exists(),
            ".tmp file should be cleaned up after failure"
        );
    }
}
