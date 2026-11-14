use std::time::Duration;

/// Probe connectivity for the selected source; request errors and timeouts return false.
pub async fn check_internet() -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    // Probe the mirror directly when selected; an unreachable gstatic endpoint must not
    // suppress downloads that the mirror can serve.
    let probe = if crate::mirror::is_mirror() {
        "https://bmclapi2.bangbang93.com/mc/game/version_manifest_v2.json"
    } else {
        "https://connectivitycheck.gstatic.com/generate_204"
    };

    client.get(probe).send().await.is_ok()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unreachable_host_returns_false() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        let result = client
            .get("http://192.0.2.1/generate_204") // TEST-NET-1, RFC 5737
            .send()
            .await;
        assert!(result.is_err());
    }
}
