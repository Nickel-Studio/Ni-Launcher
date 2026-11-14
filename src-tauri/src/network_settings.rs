//! 网络配置由 settings.json 迁移得到；每个操作读取独立快照，不临时改写全局源。
pub use minecraft_java_rs_core::net::transfer::SourcePolicy;
use minecraft_java_rs_core::net::transfer::{
    CancellationToken, TransferContext, configure_transfer_limits,
};
use serde::Serialize;
use std::sync::{LazyLock, RwLock};

pub const DEFAULT_DOWNLOAD_THREADS: usize = 10;
pub const MAX_DOWNLOAD_THREADS: usize = 64;
pub const MAX_DOWNLOAD_LIMIT_BPS: u64 = 128 * 1024 * 1024;
pub const MODRINTH_API_MIRROR: &str = "https://mod.mcimirror.top/modrinth/v2/";
pub const CURSEFORGE_API_MIRROR: &str = "https://mod.mcimirror.top/curseforge/v1/";
pub const COMMUNITY_FILE_MIRROR: &str = "https://mod.mcimirror.top/";

pub fn parse_source_policy(value: &str) -> Result<SourcePolicy, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "mirror_first" | "bmclapi" => Ok(SourcePolicy::MirrorFirst),
        "official_auto" => Ok(SourcePolicy::OfficialAuto),
        "official_first" | "official" => Ok(SourcePolicy::OfficialFirst),
        _ => Err("下载源策略无效，请在设置中重新选择".into()),
    }
}
pub fn policy_name(policy: SourcePolicy) -> &'static str {
    match policy {
        SourcePolicy::MirrorFirst => "mirror_first",
        SourcePolicy::OfficialAuto => "official_auto",
        SourcePolicy::OfficialFirst => "official_first",
    }
}
impl NetworkSettings {
    pub fn transfer(&self, source: Option<&str>) -> Result<TransferContext, String> {
        Ok(TransferContext::new(
            self.version_list_source,
            source
                .map(parse_source_policy)
                .transpose()?
                .unwrap_or(self.file_download_source),
            CancellationToken::new(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSettings {
    pub version_list_source: SourcePolicy,
    pub file_download_source: SourcePolicy,
    pub community_api_source: SourcePolicy,
    pub max_download_threads: usize,
    pub download_limit_bps: u64,
    pub verify_microsoft_ssl: bool,
    pub modrinth_api_mirror: String,
    pub curseforge_api_mirror: String,
    pub community_file_mirror: String,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            version_list_source: SourcePolicy::OfficialAuto,
            file_download_source: SourcePolicy::OfficialAuto,
            community_api_source: SourcePolicy::OfficialAuto,
            max_download_threads: DEFAULT_DOWNLOAD_THREADS,
            download_limit_bps: 0,
            verify_microsoft_ssl: true,
            modrinth_api_mirror: MODRINTH_API_MIRROR.into(),
            curseforge_api_mirror: CURSEFORGE_API_MIRROR.into(),
            community_file_mirror: COMMUNITY_FILE_MIRROR.into(),
        }
    }
}

#[cfg(test)]
fn normalize_base(value: &str) -> Result<String, String> {
    // 后端不依赖前端 URL 校验；解析器由既有 reqwest 重导出。
    let url = minecraft_java_rs_core::net::client::reqwest::Url::parse(value.trim())
        .map_err(|_| "镜像地址必须是有效的 HTTPS 基址".to_string())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("镜像地址须使用 HTTPS，且不包含账号、查询参数或片段".into());
    }
    Ok(format!("{}/", url.as_str().trim_end_matches('/')))
}

pub fn from_value(raw: &serde_json::Value) -> Result<NetworkSettings, String> {
    let mut out = NetworkSettings::default();
    let legacy = raw
        .get("downloadSource")
        .and_then(|v| v.as_str())
        .and_then(|v| parse_source_policy(v).ok());
    let policy = |key: &str, fallback: SourcePolicy| -> Result<SourcePolicy, String> {
        match raw.get(key).and_then(|v| v.as_str()) {
            Some(value) => parse_source_policy(value),
            None => Ok(fallback),
        }
    };
    out.version_list_source = policy("versionListSource", legacy.unwrap_or_default())?;
    out.file_download_source = policy("fileDownloadSource", legacy.unwrap_or_default())?;
    out.community_api_source = policy("communityApiSource", SourcePolicy::OfficialAuto)?;
    out.max_download_threads = raw
        .get("maxDownloadThreads")
        .and_then(|v| v.as_u64())
        .map(|v| v.clamp(1, MAX_DOWNLOAD_THREADS as u64) as usize)
        .unwrap_or(DEFAULT_DOWNLOAD_THREADS);
    out.download_limit_bps = raw
        .get("downloadLimitBps")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .min(MAX_DOWNLOAD_LIMIT_BPS);
    out.verify_microsoft_ssl = raw
        .get("verifyMicrosoftSsl")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    // 镜像基址固定为服务常量；旧配置地址不覆盖它们，三项来源策略独立读取。
    Ok(out)
}

static CURRENT: LazyLock<RwLock<NetworkSettings>> =
    LazyLock::new(|| RwLock::new(NetworkSettings::default()));

pub fn snapshot() -> NetworkSettings {
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn apply(settings: NetworkSettings) {
    configure_transfer_limits(
        settings.max_download_threads as u32,
        settings.download_limit_bps,
    );
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = settings;
}

pub fn set_legacy_source(source: &str) -> Result<(), String> {
    let policy = parse_source_policy(source)?;
    let mut settings = CURRENT.write().unwrap_or_else(|e| e.into_inner());
    settings.version_list_source = policy;
    settings.file_download_source = policy;
    Ok(())
}

pub fn list_policy(source: Option<&str>) -> Result<SourcePolicy, String> {
    source
        .map(parse_source_policy)
        .unwrap_or_else(|| Ok(snapshot().version_list_source))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migrates_legacy_without_overwriting_new_independent_policies() {
        let fresh = from_value(&serde_json::json!({})).unwrap();
        assert_eq!(fresh.version_list_source, SourcePolicy::OfficialAuto);
        assert_eq!(fresh.max_download_threads, 10);
        assert!(fresh.verify_microsoft_ssl);
        for (old, expected) in [
            ("official", SourcePolicy::OfficialFirst),
            ("bmclapi", SourcePolicy::MirrorFirst),
        ] {
            let migrated = from_value(&serde_json::json!({"downloadSource":old})).unwrap();
            assert_eq!(migrated.version_list_source, expected);
            assert_eq!(migrated.file_download_source, expected);
            assert_eq!(migrated.community_api_source, SourcePolicy::OfficialAuto);
        }
        let mixed = from_value(&serde_json::json!({"downloadSource":"bmclapi", "versionListSource":"official_first", "fileDownloadSource":"official_auto"})).unwrap();
        assert_eq!(mixed.version_list_source, SourcePolicy::OfficialFirst);
        assert_eq!(mixed.file_download_source, SourcePolicy::OfficialAuto);
    }
    #[test]
    fn validates_mirrors_and_budget() {
        for invalid in [
            "http://example.com/",
            "https://user:secret@example.com/",
            "https://example.com/?key=x",
            "https://example.com/#x",
        ] {
            assert!(normalize_base(invalid).is_err());
        }
        assert_eq!(
            normalize_base("https://example.com/base").unwrap(),
            "https://example.com/base/"
        );
        let normalized = from_value(&serde_json::json!({"maxDownloadThreads":1000,"downloadLimitBps":9999999999_u64,"verifyMicrosoftSsl":false})).unwrap();
        assert_eq!(normalized.max_download_threads, MAX_DOWNLOAD_THREADS);
        assert_eq!(normalized.download_limit_bps, MAX_DOWNLOAD_LIMIT_BPS);
        assert!(!normalized.verify_microsoft_ssl);
    }

    #[test]
    fn transfer_snapshot_keeps_list_and_file_policies_independent() {
        let mut settings = NetworkSettings {
            version_list_source: SourcePolicy::MirrorFirst,
            file_download_source: SourcePolicy::OfficialAuto,
            community_api_source: SourcePolicy::OfficialFirst,
            ..NetworkSettings::default()
        };
        let original = settings.transfer(None).unwrap();
        let override_file = settings.transfer(Some("official")).unwrap();
        settings.version_list_source = SourcePolicy::OfficialFirst;
        settings.file_download_source = SourcePolicy::MirrorFirst;
        assert_eq!(original.list_policy(), SourcePolicy::MirrorFirst);
        assert_eq!(original.file_policy(), SourcePolicy::OfficialAuto);
        assert_eq!(override_file.list_policy(), SourcePolicy::MirrorFirst);
        assert_eq!(override_file.file_policy(), SourcePolicy::OfficialFirst);
        assert_eq!(settings.community_api_source, SourcePolicy::OfficialFirst);
        assert!(settings.transfer(Some("invalid")).is_err());
        let minimum = from_value(&serde_json::json!({"maxDownloadThreads":0})).unwrap();
        assert_eq!(minimum.max_download_threads, 1);
    }
}
