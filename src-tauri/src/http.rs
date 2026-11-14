//! 共享 HTTP 辅助。
//!
//! **依赖策略**：应用 `Cargo.toml` 不直接声明 `reqwest`。这里复用核心库
//! `minecraft_java_rs_core::net::client::build_client(...)` 返回的 `reqwest::Client`
//! （返回类型由类型推断得到，源码中无需书写 `reqwest` 路径），再调用其
//! `.post()/.get()/.put()` 等固有方法。因此请求所用的 reqwest 与核心库**完全同版本**，
//! 不会引入第二个 reqwest 副本，也不会造成版本冲突。
//!
//! 注意：这些辅助**不走**核心库的 `mirror::rewrite_url` 下载源重写——认证/皮肤接口
//! 不应被镜像重定向。

use minecraft_java_rs_core::net::client::{
    Client, ClientBuilder, build_client, describe_reqwest_error,
};

/// 共享 HTTP 超时（秒）。install 模块自建 client 时也复用同一值。
pub const TIMEOUT_SECS: u64 = 30;

/// 微软链路持有同一客户端快照，不改变皮肤/Yggdrasil 或浏览器的 TLS 行为。
#[derive(Clone)]
pub struct MicrosoftClient(Client);

fn microsoft_client_builder(verify: bool) -> ClientBuilder {
    microsoft_client_builder_with_system_trust(verify, |builder| builder)
}

// 系统信任装配只属于普通模式；测试可注入隔离根，生产仍由 native TLS 读取系统根。
fn microsoft_client_builder_with_system_trust(
    verify: bool,
    system_trust: impl FnOnce(ClientBuilder) -> ClientBuilder,
) -> ClientBuilder {
    let builder = Client::builder().timeout(std::time::Duration::from_secs(TIMEOUT_SECS));
    if verify {
        // 明确选择 rustls + 固定公共根，不能因为 Cargo 默认 TLS 后端而加载系统根。
        builder
            .use_rustls_tls()
            .tls_built_in_root_certs(false)
            .tls_built_in_webpki_certs(true)
    } else {
        // reqwest 0.12 的 default-tls 已选择系统 TLS；use_native_tls 另受 native-tls feature 限制。
        // 保留正常证书链、主机名与有效期校验；没有 trust-all 分支。
        system_trust(builder.tls_built_in_root_certs(true))
    }
}

impl MicrosoftClient {
    pub fn new(verify: bool) -> Result<Self, String> {
        microsoft_client_builder(verify)
            .build()
            .map(Self)
            .map_err(|error| describe_reqwest_error(&error))
    }
    pub async fn post_form(
        &self,
        url: &str,
        fields: &[(&str, &str)],
    ) -> Result<(u16, serde_json::Value), String> {
        let response = self
            .0
            .post(url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .body(encode_form(fields))
            .send()
            .await
            .map_err(|error| describe_reqwest_error(&error))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|error| describe_reqwest_error(&error))?;
        Ok((status, parse_json_or_null(&text)))
    }
    pub async fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<(u16, serde_json::Value), String> {
        let response = self
            .0
            .post(url)
            .header("Accept", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|error| describe_reqwest_error(&error))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|error| describe_reqwest_error(&error))?;
        Ok((status, parse_json_or_null(&text)))
    }
    pub async fn get_json(
        &self,
        url: &str,
        bearer: &str,
    ) -> Result<(u16, serde_json::Value), String> {
        let response = self
            .0
            .get(url)
            .header("Accept", "application/json")
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|error| describe_reqwest_error(&error))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|error| describe_reqwest_error(&error))?;
        Ok((status, parse_json_or_null(&text)))
    }
}

fn parse_json_or_null(text: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(text).unwrap_or(serde_json::Value::Null)
}

/// 极简 percent-encoding（仅保留 RFC3986 未保留字符）。
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn encode_form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// POST JSON 请求体，返回 (HTTP 状态码, 解析后的 JSON / Null)。
pub async fn post_json(
    url: &str,
    body: &serde_json::Value,
    bearer: Option<&str>,
) -> Result<(u16, serde_json::Value), String> {
    let client = build_client(TIMEOUT_SECS, false, None).map_err(|e| describe_reqwest_error(&e))?;
    let mut req = client
        .post(url)
        .header("Accept", "application/json")
        .json(body);
    if let Some(token) = bearer {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let resp = req.send().await.map_err(|e| describe_reqwest_error(&e))?;
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?;
    Ok((status, parse_json_or_null(&text)))
}

/// GET JSON，可选 Bearer，返回 (状态码, JSON / Null)。
pub async fn get_json(url: &str, bearer: Option<&str>) -> Result<(u16, serde_json::Value), String> {
    let client = build_client(TIMEOUT_SECS, false, None).map_err(|e| describe_reqwest_error(&e))?;
    let mut req = client.get(url).header("Accept", "application/json");
    if let Some(token) = bearer {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let resp = req.send().await.map_err(|e| describe_reqwest_error(&e))?;
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?;
    Ok((status, parse_json_or_null(&text)))
}

/// GET 二进制，返回 (状态码, Content-Type, 字节)。
pub async fn get_bytes(url: &str) -> Result<(u16, Option<String>, Vec<u8>), String> {
    let client = build_client(TIMEOUT_SECS, false, None).map_err(|e| describe_reqwest_error(&e))?;
    let resp = client
        .get(url)
        .header("Accept", "image/png,*/*")
        .send()
        .await
        .map_err(|e| describe_reqwest_error(&e))?;
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?
        .to_vec();
    Ok((status, content_type, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_microsoft_tls_modes_build_without_changing_shared_client() {
        assert!(MicrosoftClient::new(true).is_ok());
        assert!(MicrosoftClient::new(false).is_ok());
        assert!(build_client(TIMEOUT_SECS, false, None).is_ok());
    }

    /// 仅显式执行：使用本机已有 OpenSSL，在临时目录创建测试 CA，不写系统证书库。
    #[tokio::test]
    #[ignore = "需要本机已有 OpenSSL；不安装 CA 或新增 Rust TLS 测试依赖"]
    async fn microsoft_tls_rejects_untrusted_expired_and_wrong_host() {
        use minecraft_java_rs_core::net::client::Certificate;
        use std::path::{Path, PathBuf};
        use std::process::{Child, Command, Stdio};
        use std::time::Duration;

        struct Fixture {
            directory: PathBuf,
            servers: Vec<Child>,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                for server in &mut self.servers {
                    let _ = server.kill();
                    let _ = server.wait();
                }
                let _ = std::fs::remove_dir_all(&self.directory);
            }
        }
        fn openssl(directory: &Path, args: &[&str]) {
            let output = Command::new("openssl")
                .current_dir(directory)
                .args(args)
                .stdin(Stdio::null())
                .output()
                .expect("请先配置已安装 OpenSSL 的 PATH");
            assert!(
                output.status.success(),
                "OpenSSL 证书构造失败：{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        async fn server(fixture: &mut Fixture, certificate: &str) -> u16 {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            let child = Command::new("openssl")
                .current_dir(&fixture.directory)
                .args([
                    "s_server",
                    "-accept",
                    &format!("127.0.0.1:{port}"),
                    "-cert",
                    certificate,
                    "-key",
                    "server.key",
                    "-www",
                    "-quiet",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            fixture.servers.push(child);
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if tokio::net::TcpStream::connect(("127.0.0.1", port))
                        .await
                        .is_ok()
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("本地 TLS 服务未就绪");
            port
        }
        let directory =
            std::env::temp_dir().join(format!("nowhere-tls-{}", crate::store::random_token()));
        std::fs::create_dir(&directory).unwrap();
        let mut fixture = Fixture {
            directory,
            servers: Vec::new(),
        };
        openssl(
            &fixture.directory,
            &[
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "root.key",
                "-out",
                "root.pem",
                "-days",
                "2",
                "-subj",
                "/CN=NoWheRE-test-CA",
                "-addext",
                "basicConstraints=critical,CA:TRUE",
            ],
        );
        openssl(
            &fixture.directory,
            &[
                "req",
                "-new",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "server.key",
                "-out",
                "server.csr",
                "-subj",
                "/CN=localhost",
                "-addext",
                "subjectAltName=DNS:localhost",
            ],
        );
        for (name, end) in [
            ("valid.pem", "20990101000000Z"),
            ("expired.pem", "20210101000000Z"),
        ] {
            openssl(
                &fixture.directory,
                &[
                    "x509",
                    "-req",
                    "-in",
                    "server.csr",
                    "-CA",
                    "root.pem",
                    "-CAkey",
                    "root.key",
                    "-CAcreateserial",
                    "-copy_extensions",
                    "copy",
                    "-not_before",
                    "20200101000000Z",
                    "-not_after",
                    end,
                    "-out",
                    name,
                ],
            );
        }
        let certificate =
            Certificate::from_pem(&std::fs::read(fixture.directory.join("root.pem")).unwrap())
                .unwrap();
        let valid = server(&mut fixture, "valid.pem").await;
        let expired = server(&mut fixture, "expired.pem").await;
        let client = |verify, trust: bool| {
            let mut builder = microsoft_client_builder(verify)
                .no_proxy()
                .timeout(Duration::from_secs(5));
            if trust {
                builder = builder.add_root_certificate(certificate.clone());
            }
            builder.build().unwrap()
        };
        // 两种生产策略均拒绝注入的非公共 CA；正常系统模式也不是 trust-all。
        for verify in [true, false] {
            assert!(
                client(verify, false)
                    .get(format!("https://localhost:{valid}/"))
                    .send()
                    .await
                    .is_err()
            );
        }
        // 仅测试客户端信任隔离 CA，用成功对照确认错误来自主机名/有效期校验。
        for verify in [true, false] {
            assert!(
                client(verify, true)
                    .get(format!("https://localhost:{valid}/"))
                    .send()
                    .await
                    .unwrap()
                    .status()
                    .is_success()
            );
            assert!(
                client(verify, true)
                    .get(format!("https://127.0.0.1:{valid}/"))
                    .send()
                    .await
                    .is_err()
            );
            assert!(
                client(verify, true)
                    .get(format!("https://localhost:{expired}/"))
                    .send()
                    .await
                    .is_err()
            );
        }
        // 在系统信任装配点注入额外根，模拟机器已信任该 CA；不修改真实系统证书库。
        // 同一输入经过生产策略分支：普通模式接受，公共根模式必须忽略该额外根。
        for verify in [false, true] {
            let system_trust_used = std::cell::Cell::new(false);
            let isolated = microsoft_client_builder_with_system_trust(verify, |builder| {
                system_trust_used.set(true);
                builder.add_root_certificate(certificate.clone())
            })
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
            assert_eq!(system_trust_used.get(), !verify);
            let response = isolated
                .get(format!("https://localhost:{valid}/"))
                .send()
                .await;
            if verify {
                assert!(response.is_err(), "公共根模式不得继承模拟系统额外根");
            } else {
                assert!(response.unwrap().status().is_success());
                assert!(
                    isolated
                        .get(format!("https://127.0.0.1:{valid}/"))
                        .send()
                        .await
                        .is_err()
                );
                assert!(
                    isolated
                        .get(format!("https://localhost:{expired}/"))
                        .send()
                        .await
                        .is_err()
                );
            }
            eprintln!(
                "[tls-isolated-system] verify={verify} extra_root_loaded={} valid_chain_accepted={}",
                system_trust_used.get(),
                !verify
            );
        }
        eprintln!(
            "[tls-isolated-system] local TLS handshake; injected trust source; OS certificate store unchanged"
        );
    }

    #[tokio::test]
    #[ignore = "需要访问 Microsoft 官方 HTTPS；离线 CI 不执行"]
    async fn microsoft_tls_accepts_official_certificate_chain() {
        for verify in [true, false] {
            let client = microsoft_client_builder(verify).build().unwrap();
            let response = client.get("https://login.microsoftonline.com/common/v2.0/.well-known/openid-configuration")
                .send().await.unwrap();
            assert!(response.status().is_success());
        }
    }

    #[test]
    fn urlencode_escapes_reserved() {
        assert_eq!(
            urlencode("XboxLive.signin offline_access"),
            "XboxLive.signin%20offline_access"
        );
        assert_eq!(urlencode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(urlencode("a/b"), "a%2Fb");
    }

    #[test]
    fn encode_form_builds_query() {
        assert_eq!(encode_form(&[("a", "1"), ("b", "x y")]), "a=1&b=x%20y");
    }

    #[test]
    fn parse_json_or_null_falls_back() {
        assert_eq!(parse_json_or_null(""), serde_json::Value::Null);
        assert_eq!(parse_json_or_null("not json"), serde_json::Value::Null);
        assert_eq!(parse_json_or_null("{\"a\":1}")["a"], 1);
    }
}
