use serde::de::DeserializeOwned;
use crate::net::transfer::{self, TransferContext, DEFAULT_LIST_LIMIT};

/// 兼容入口：复用当前操作快照，不临时修改全局下载源。
pub async fn fetch_json<T: DeserializeOwned>(client: &reqwest::Client, url: &str) -> Result<T, String> {
    fetch_json_with_context(client, url, &TransferContext::current()).await
}

pub async fn fetch_json_with_context<T: DeserializeOwned>(
    client: &reqwest::Client, url: &str, context: &TransferContext,
) -> Result<T, String> {
    let service = service_key(url);
    transfer::fetch_json_candidates(client, context, &service, &transfer::core_candidates(url), DEFAULT_LIST_LIMIT).await
}

pub async fn fetch_text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    fetch_text_with_context(client, url, &TransferContext::current()).await
}

pub async fn fetch_text_with_context(
    client: &reqwest::Client, url: &str, context: &TransferContext,
) -> Result<String, String> {
    let service = service_key(url);
    transfer::fetch_text_candidates(client, context, &service, &transfer::core_candidates(url), DEFAULT_LIST_LIMIT).await
}

pub(crate) fn service_key(url: &str) -> String {
    reqwest::Url::parse(url).map(|url| url.origin().ascii_serialization()).unwrap_or_else(|_| url.to_owned())
}
