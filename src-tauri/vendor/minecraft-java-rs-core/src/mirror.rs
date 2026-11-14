//! 官方源与 BMCLAPI 镜像的地址映射。
//!
//! 每个操作由 TransferContext 固定列表与文件策略，任务开始后不得切换全局源。
//! 全局开关仅供旧 API 使用；失败回退、请求竞速与进程级预算由 net::transfer 负责。
//! 认证与皮肤接口不得调用下载镜像映射。
//!
//! 已知下载主机按 BMCLAPI 文档（<https://bmclapidoc.bangbang93.com/>）映射：
//!
//! | 官方主机 | 镜像路径 |
//! |---|---|
//! | `launchermeta.mojang.com` / `launcher.mojang.com` / `piston-meta.mojang.com` / `piston-data.mojang.com` | `bmclapi2.bangbang93.com`（路径不变） |
//! | `resources.download.minecraft.net` | `bmclapi2.bangbang93.com/assets`（前缀） |
//! | `libraries.minecraft.net` / `maven.fabricmc.net` / `maven.minecraftforge.net` | `bmclapi2.bangbang93.com/maven`（前缀） |
//! | `files.minecraftforge.net` | `bmclapi2.bangbang93.com/maven`（去掉可选 `/maven` 后再加） |
//! | `maven.neoforged.net` | `bmclapi2.bangbang93.com/maven`（去掉 `/releases`） |
//! | `maven.quiltmc.org` | `bmclapi2.bangbang93.com/maven`（去掉 `/repository/release`） |
//! | `meta.fabricmc.net` | `bmclapi2.bangbang93.com/fabric-meta`（前缀） |
//! | `meta.quiltmc.org` | `bmclapi2.bangbang93.com/quilt-meta`（前缀） |
//!
//! 未列入的主机（如 `meta.legacyfabric.net`、`api.adoptium.net`）保持原样 ——
//! BMCLAPI 未提供对应镜像。

use std::sync::atomic::{AtomicU8, Ordering};
pub use crate::net::transfer::SourcePolicy;

/// 纯地址映射，不读取全局开关。用于操作级候选快照。
pub fn mirror_url(url: &str) -> String {
    rewrite_for(true, url)
}

/// 镜像清单可能把内部文件地址也改成镜像，按路径恢复对应官方候选。
/// Maven 公共代理按坐标域划分上游，避免只剩同一个镜像地址可重试。
pub fn official_urls(url: &str) -> Vec<String> {
    let Ok(parsed) = reqwest::Url::parse(url) else { return Vec::new(); };
    if !matches!(parsed.host_str(), Some("bmclapi2.bangbang93.com" | "bmclapi.bangbang93.com")) {
        return Vec::new();
    }
    let path = parsed.path();
    let query = parsed.query().map(|query| format!("?{query}")).unwrap_or_default();
    let official = if let Some(path) = path.strip_prefix("/assets/") {
        format!("https://resources.download.minecraft.net/{path}{query}")
    } else if let Some(path) = path.strip_prefix("/fabric-meta/") {
        format!("https://meta.fabricmc.net/{path}{query}")
    } else if let Some(path) = path.strip_prefix("/quilt-meta/") {
        format!("https://meta.quiltmc.org/{path}{query}")
    } else if let Some(path) = path.strip_prefix("/maven/") {
        let base = if path.starts_with("net/neoforged/") { "https://maven.neoforged.net/releases" }
            else if path.starts_with("net/minecraftforge/") || path.starts_with("de/oceanlabs/") { "https://maven.minecraftforge.net" }
            else if path.starts_with("net/fabricmc/") { "https://maven.fabricmc.net" }
            else if path.starts_with("org/quiltmc/") { "https://maven.quiltmc.org/repository/release" }
            else { "https://libraries.minecraft.net" };
        format!("{base}/{path}{query}")
    } else if path.starts_with("/v1/packages/") || path == "/mc/game/version_manifest.json" || path == "/mc/game/version_manifest_v2.json" {
        format!("https://piston-meta.mojang.com{path}{query}")
    } else if path.starts_with("/v1/objects/") {
        format!("https://piston-data.mojang.com{path}{query}")
    } else {
        return Vec::new();
    };
    let mut urls = vec![official];
    if let Some(path) = path.strip_prefix("/maven/") {
        urls.push(format!("https://repo1.maven.org/maven2/{path}{query}"));
    }
    urls
}


/// 官方源标识。
pub const SOURCE_OFFICIAL: &str = "official";
/// BMCLAPI 国内镜像源标识。
pub const SOURCE_BMCLAPI: &str = "bmclapi";

const OFFICIAL: u8 = 0;
const BMCLAPI: u8 = 1;

/// BMCLAPI 主入口。
pub const BMCLAPI_BASE: &str = "https://bmclapi2.bangbang93.com";

static CURRENT_SOURCE: AtomicU8 = AtomicU8::new(OFFICIAL);

/// 设置当前下载源。取值 `"official"` 或 `"bmclapi"`（大小写不敏感）；
/// 其他任何值一律回退为官方源。
pub fn set_download_source(source: &str) {
    let flag = match source.trim().to_ascii_lowercase().as_str() {
        "bmclapi" | "bmclapi2" | "mirror" | "mirror2" | "cn" => BMCLAPI,
        _ => OFFICIAL,
    };
    CURRENT_SOURCE.store(flag, Ordering::Relaxed);
}

/// 返回当前下载源标识（`"official"` 或 `"bmclapi"`）。
pub fn download_source() -> &'static str {
    if is_mirror() {
        SOURCE_BMCLAPI
    } else {
        SOURCE_OFFICIAL
    }
}

/// 当前是否启用了 BMCLAPI 镜像。
pub fn is_mirror() -> bool {
    CURRENT_SOURCE.load(Ordering::Relaxed) == BMCLAPI
}

/// 将官方 URL 重写为镜像 URL。
///
/// 官方模式、未知主机、或非 http(s) 的 URL 一律原样返回。
pub fn rewrite_url(url: &str) -> String {
    rewrite_for(is_mirror(), url)
}

/// 返回给定官方 URL 应尝试的地址列表（按优先级顺序）。
///
/// - 官方模式、或未知主机（无镜像映射）：仅返回原 URL。
/// - 镜像模式且存在映射：返回 `[镜像 URL, 官方 URL]`，以便镜像
///   404 / 超时 / 校验失败时**自动回退官方源**。
pub fn candidate_urls(url: &str) -> Vec<String> {
    candidate_urls_for(is_mirror(), url)
}

/// [`candidate_urls`] 的纯函数实现（不依赖全局状态，便于单元测试）。
fn candidate_urls_for(mirror: bool, url: &str) -> Vec<String> {
    if !mirror {
        return vec![url.to_string()];
    }
    let mirror_url = rewrite_for(true, url);
    if mirror_url == url {
        vec![url.to_string()]
    } else {
        vec![mirror_url, url.to_string()]
    }
}

/// [`rewrite_url`] 的纯函数实现：按给定开关重写，不依赖全局状态（便于单元测试）。
fn rewrite_for(mirror: bool, url: &str) -> String {
    if !mirror {
        return url.to_string();
    }

    let rest = match url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        Some(r) => r,
        None => return url.to_string(),
    };

    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };

    match host {
        // 版本清单 / 版本 JSON / 客户端 jar / Java 运行时清单等：路径保持不变。
        "launchermeta.mojang.com"
        | "launcher.mojang.com"
        | "piston-meta.mojang.com"
        | "piston-data.mojang.com" => format!("{BMCLAPI_BASE}{path}"),

        // 游戏资源对象：resources.download.minecraft.net/xx/hash -> /assets/xx/hash
        "resources.download.minecraft.net" => format!("{BMCLAPI_BASE}/assets{path}"),

        // Maven 仓库（路径前缀）
        "libraries.minecraft.net" | "maven.fabricmc.net" | "maven.minecraftforge.net" => {
            format!("{BMCLAPI_BASE}/maven{path}")
        }
        "files.minecraftforge.net" => {
            let p = path.strip_prefix("/maven").unwrap_or(path);
            format!("{BMCLAPI_BASE}/maven{p}")
        }
        "maven.neoforged.net" => {
            let p = path.strip_prefix("/releases").unwrap_or(path);
            format!("{BMCLAPI_BASE}/maven{p}")
        }
        "maven.quiltmc.org" => {
            let p = path.strip_prefix("/repository/release").unwrap_or(path);
            format!("{BMCLAPI_BASE}/maven{p}")
        }

        // 加载器元数据 API
        "meta.fabricmc.net" => format!("{BMCLAPI_BASE}/fabric-meta{path}"),
        "meta.quiltmc.org" => format!("{BMCLAPI_BASE}/quilt-meta{path}"),

        // 未知主机：保持原样。
        _ => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_mode_is_noop() {
        let url = "https://libraries.minecraft.net/a/b/1.0/b-1.0.jar";
        assert_eq!(rewrite_for(false, url), url);
    }

    #[test]
    fn bmclapi_rewrites_known_hosts() {
        assert_eq!(
            rewrite_for(true, "https://piston-meta.mojang.com/v1/packages/ab/1.21.1.json"),
            "https://bmclapi2.bangbang93.com/v1/packages/ab/1.21.1.json"
        );
        assert_eq!(
            rewrite_for(true, "https://resources.download.minecraft.net/ab/abcdef"),
            "https://bmclapi2.bangbang93.com/assets/ab/abcdef"
        );
        assert_eq!(
            rewrite_for(true, "https://libraries.minecraft.net/a/b/1.0/b-1.0.jar"),
            "https://bmclapi2.bangbang93.com/maven/a/b/1.0/b-1.0.jar"
        );
        assert_eq!(
            rewrite_for(
                true,
                "https://files.minecraftforge.net/net/minecraftforge/forge/maven-metadata.json"
            ),
            "https://bmclapi2.bangbang93.com/maven/net/minecraftforge/forge/maven-metadata.json"
        );
        assert_eq!(
            rewrite_for(
                true,
                "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml"
            ),
            "https://bmclapi2.bangbang93.com/maven/net/neoforged/neoforge/maven-metadata.xml"
        );
        assert_eq!(
            rewrite_for(true, "https://meta.fabricmc.net/v2/versions"),
            "https://bmclapi2.bangbang93.com/fabric-meta/v2/versions"
        );
        // 无对应镜像的主机保持原样。
        assert_eq!(
            rewrite_for(true, "https://api.adoptium.net/v3/assets/latest"),
            "https://api.adoptium.net/v3/assets/latest"
        );
    }

    #[test]
    fn global_switch_roundtrip() {
        set_download_source("bmclapi");
        assert!(is_mirror());
        assert_eq!(download_source(), SOURCE_BMCLAPI);
        set_download_source("official");
        assert!(!is_mirror());
        assert_eq!(download_source(), SOURCE_OFFICIAL);
        // 未知取值回退为官方源。
        set_download_source("whatever");
        assert!(!is_mirror());
    }

    #[test]
    fn candidate_urls_falls_back_to_official() {
        // 镜像模式：已知主机 → [镜像, 官方] 两条候选。
        assert_eq!(
            candidate_urls_for(true, "https://libraries.minecraft.net/a/b/1.0/b-1.0.jar"),
            vec![
                "https://bmclapi2.bangbang93.com/maven/a/b/1.0/b-1.0.jar".to_string(),
                "https://libraries.minecraft.net/a/b/1.0/b-1.0.jar".to_string(),
            ]
        );
        // 镜像模式：无映射主机 → 仅原 URL（不会误重写）。
        assert_eq!(
            candidate_urls_for(true, "https://api.adoptium.net/v3/assets/latest"),
            vec!["https://api.adoptium.net/v3/assets/latest".to_string()]
        );
        // 官方模式：仅原 URL。
        assert_eq!(
            candidate_urls_for(false, "https://libraries.minecraft.net/a/b/1.0/b-1.0.jar"),
            vec!["https://libraries.minecraft.net/a/b/1.0/b-1.0.jar".to_string()]
        );
    }
}
