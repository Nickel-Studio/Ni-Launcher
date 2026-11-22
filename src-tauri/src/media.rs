//! Validated local images and bounded video streaming for backgrounds and portable export.
//! Arbitrary files, SVG documents and credential filenames must never reach the webview.
use std::io::Read;
use std::path::{Component, Path};

use base64::Engine as _;

const MAX_BYTES: u64 = 32 * 1024 * 1024;
const INVALID_IMAGE: &str = "请选择有效的 PNG、JPEG、WebP、GIF、BMP 或 AVIF 本地图片";

pub(crate) fn raster_mime(path: &Path, bytes: &[u8]) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => Some("image/png"),
        "jpg" | "jpeg" if bytes.starts_with(&[0xff, 0xd8, 0xff]) => Some("image/jpeg"),
        "gif" if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") => Some("image/gif"),
        "bmp" if bytes.starts_with(b"BM") => Some("image/bmp"),
        "webp" if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) => Some("image/webp"),
        "avif" if bytes.get(4..8) == Some(b"ftyp".as_slice())
            && bytes[8..bytes.len().min(32)].windows(4)
                .any(|brand| brand == b"avif" || brand == b"avis") => Some("image/avif"),
        _ => None,
    }
}

/// `invalid` is the media-specific message, so a rejected video never reports an image-format error.
fn check_local_path(path: &Path, invalid: &str) -> Result<(), String> {
    if !path.is_absolute() || path.to_string_lossy().len() > 32768 {
        return Err(invalid.into());
    }
    for component in path.components() {
        match component {
            Component::ParentDir => return Err(invalid.into()),
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                if name.chars().any(char::is_control) || name.contains(':')
                    || ["secrets.local.json", "credentials.key", "accounts.json", "active_account.json"]
                        .iter().any(|blocked| name.eq_ignore_ascii_case(blocked)) {
                    return Err(invalid.into());
                }
            }
            // Network shares stay supported (the old asset scope allowed them): mapped drives
            // canonicalize to `\\?\UNC\...`. Device namespaces such as `\\.\` remain rejected.
            #[cfg(windows)]
            Component::Prefix(prefix) => {
                use std::path::Prefix;
                if !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_) | Prefix::UNC(..) | Prefix::VerbatimUNC(..)) {
                    return Err(invalid.into());
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn read_image(path: &Path) -> Result<(&'static str, Vec<u8>), String> {
    check_local_path(path, INVALID_IMAGE)?;
    // Resolve ancestor redirects (e.g. macOS /var, OneDrive); reject a linked file itself.
    crate::instances::reject_links_local(path).map_err(|_| INVALID_IMAGE.to_string())?;
    let canonical = std::fs::canonicalize(path).map_err(|_| "文件不存在或不是文件".to_string())?;
    check_local_path(&canonical, INVALID_IMAGE)?;
    let mut file = std::fs::File::open(&canonical).map_err(|_| INVALID_IMAGE.to_string())?;
    let metadata = file.metadata().map_err(|_| INVALID_IMAGE.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_BYTES {
        return Err("图片无效或超过 32 MiB 限制".into());
    }
    // A single read() may return short on network shares or filter drivers; fill the header.
    let mut header = Vec::with_capacity(32);
    (&mut file).take(32).read_to_end(&mut header).map_err(|_| INVALID_IMAGE.to_string())?;
    let mime = raster_mime(&canonical, &header).ok_or(INVALID_IMAGE)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    bytes.extend_from_slice(&header);
    file.take(MAX_BYTES + 1 - header.len() as u64).read_to_end(&mut bytes)
        .map_err(|_| INVALID_IMAGE.to_string())?;
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > MAX_BYTES {
        return Err("图片文件已变化，请重新选择".into());
    }
    Ok((mime, bytes))
}

#[tauri::command]
pub fn image_data_url(path: String) -> Result<String, String> {
    let (mime, bytes) = read_image(Path::new(&path))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

fn protocol_response(request: tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    let image = (|| {
        if request.method() != tauri::http::Method::GET || request.uri().query().is_some() {
            return Err(INVALID_IMAGE.to_string());
        }
        let encoded = request.uri().path().strip_prefix('/').ok_or(INVALID_IMAGE)?;
        let path = percent_encoding::percent_decode_str(encoded).decode_utf8()
            .map_err(|_| INVALID_IMAGE.to_string())?;
        read_image(Path::new(path.as_ref()))
    })();
    let (status, mime, body) = match image {
        Ok((mime, bytes)) => (200, mime, bytes),
        Err(_) => (404, "text/plain", Vec::new()),
    };
    tauri::http::Response::builder()
        .status(status)
        .header("content-type", mime)
        .header("x-content-type-options", "nosniff")
        .header("cache-control", "no-store")
        .header("content-security-policy", "default-src 'none'; sandbox")
        .body(body).expect("static image response headers")
}

pub(crate) const MAX_VIDEO_BYTES: u64 = 512 * 1024 * 1024;
const MAX_VIDEO_CHUNK: u64 = 4 * 1024 * 1024;
const INVALID_VIDEO: &str = "请选择有效的 MP4、M4V、WebM 或 OGV 本地视频";

pub(crate) fn video_mime(path: &Path, bytes: &[u8]) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "mp4" | "m4v" if bytes.get(4..8) == Some(b"ftyp".as_slice()) => {
            let box_length = u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
            if box_length < 16 || box_length > bytes.len() { return None; }
            let brands = &bytes[8..box_length];
            let compatible = brands.chunks_exact(4).enumerate().any(|(index, brand)| {
                index != 1 && matches!(brand, b"isom" | b"iso2" | b"iso3" | b"iso4" | b"iso5" | b"iso6" | b"mp41" | b"mp42" | b"avc1" | b"dash" | b"M4V " | b"M4VH" | b"M4VP")
            });
            compatible.then_some("video/mp4")
        }
        "webm" if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3])
            && bytes.windows(7).any(|value| value == b"\x42\x82\x84webm") => Some("video/webm"),
        "ogv" if bytes.starts_with(b"OggS")
            && bytes.windows(7).any(|value| value == b"\x80theora") => Some("video/ogg"),
        _ => None,
    }
}

fn open_video(path: &Path) -> Result<(std::fs::File, &'static str, u64), String> {
    check_local_path(path, INVALID_VIDEO)?;
    crate::instances::reject_links_local(path).map_err(|_| INVALID_VIDEO.to_string())?;
    let canonical = std::fs::canonicalize(path).map_err(|_| INVALID_VIDEO.to_string())?;
    check_local_path(&canonical, INVALID_VIDEO)?;
    if !std::fs::metadata(&canonical).map_err(|_| INVALID_VIDEO.to_string())?.is_file() {
        return Err(INVALID_VIDEO.into());
    }
    let mut file = std::fs::File::open(&canonical).map_err(|_| INVALID_VIDEO.to_string())?;
    let metadata = file.metadata().map_err(|_| INVALID_VIDEO.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_VIDEO_BYTES {
        return Err("视频无效或超过 512 MiB 限制".into());
    }
    // Fill the header despite short reads; every later read seeks to its own start.
    let mut header = Vec::with_capacity(4096);
    (&mut file).take(4096).read_to_end(&mut header).map_err(|_| INVALID_VIDEO.to_string())?;
    let mime = video_mime(&canonical, &header).ok_or(INVALID_VIDEO)?;
    Ok((file, mime, metadata.len()))
}

pub(crate) fn validate_background_file(path: &Path) -> Result<(), String> {
    if path.extension().and_then(|value| value.to_str()).is_some_and(|value| {
        ["mp4", "m4v", "webm", "ogv"].contains(&value.to_ascii_lowercase().as_str())
    }) {
        open_video(path).map(|_| ())
    } else {
        read_image(path).map(|_| ())
    }
}

#[tauri::command]
pub async fn background_media_validate(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || validate_background_file(Path::new(&path)))
        .await.map_err(|_| "无法校验背景文件，请重试".to_string())?
}

fn video_range(header: Option<&str>, length: u64) -> Result<(u64, u64), ()> {
    if length == 0 { return Err(()); }
    let Some(header) = header else { return Ok((0, length.min(MAX_VIDEO_CHUNK) - 1)); };
    let value = header.strip_prefix("bytes=").ok_or(())?;
    let (start, end) = value.split_once('-').ok_or(())?;
    let number = |value: &str| -> Result<u64, ()> {
        if value.is_empty() || !value.bytes().all(|value| value.is_ascii_digit()) { return Err(()); }
        value.parse().map_err(|_| ())
    };
    let (start, end) = if start.is_empty() {
        let suffix = number(end)?;
        if suffix == 0 { return Err(()); }
        (length.saturating_sub(suffix), length - 1)
    } else {
        let start = number(start)?;
        let end = if end.is_empty() { length - 1 } else { number(end)?.min(length - 1) };
        if start >= length || start > end { return Err(()); }
        (start, end)
    };
    Ok((start, end.min(start.saturating_add(MAX_VIDEO_CHUNK - 1))))
}

fn video_protocol_response(request: tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    use std::io::{Seek, SeekFrom};
    use tauri::http::{Method, Response, StatusCode};
    let response = |status, mime| Response::builder()
        .status(status)
        .header("content-type", mime)
        .header("x-content-type-options", "nosniff")
        .header("cache-control", "no-store")
        .header("content-security-policy", "default-src 'none'; sandbox");
    let opened = (|| {
        if !matches!(*request.method(), Method::GET | Method::HEAD) || request.uri().query().is_some() {
            return Err(INVALID_VIDEO.to_string());
        }
        let encoded = request.uri().path().strip_prefix('/').ok_or(INVALID_VIDEO)?;
        let path = percent_encoding::percent_decode_str(encoded).decode_utf8()
            .map_err(|_| INVALID_VIDEO.to_string())?;
        open_video(Path::new(path.as_ref()))
    })();
    let Ok((mut file, mime, length)) = opened else {
        return response(StatusCode::NOT_FOUND, "text/plain").body(Vec::new()).expect("static media headers");
    };
    if request.method() == Method::HEAD {
        return response(StatusCode::OK, mime)
            .header("accept-ranges", "bytes")
            .header("content-length", length)
            .body(Vec::new()).expect("static video metadata headers");
    }
    let range = request.headers().get("range");
    let parsed = range.map(|value| value.to_str()).transpose()
        .map_err(|_| ()).and_then(|value| video_range(value, length));
    let Ok((start, end)) = parsed else {
        return response(StatusCode::RANGE_NOT_SATISFIABLE, "text/plain")
            .header("content-range", format!("bytes */{length}"))
            .body(Vec::new()).expect("static invalid range headers");
    };
    // Deliberate RFC 9110 deviation: a request without Range still receives at most
    // MAX_VIDEO_CHUNK bytes as 206, so a plain GET can never pull a whole video into IPC memory.
    let partial = range.is_some() || end + 1 < length;
    let mut builder = response(if partial { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK }, mime)
        .header("accept-ranges", "bytes")
        .header("content-length", end - start + 1);
    if partial {
        builder = builder.header("content-range", format!("bytes {start}-{end}/{length}"));
    }
    // Each request reads a bounded segment, never an entire user video into IPC memory.
    let mut body = vec![0; (end - start + 1) as usize];
    let read = file.seek(SeekFrom::Start(start)).and_then(|_| file.read_exact(&mut body));
    if read.is_err() || file.metadata().map(|value| value.len()).ok() != Some(length) {
        return response(StatusCode::NOT_FOUND, "text/plain").body(Vec::new()).expect("static media headers");
    }
    builder.body(body).expect("validated video response headers")
}

pub fn protocol_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("nickel-images")
        .register_asynchronous_uri_scheme_protocol("nickel-image", |_context, request, responder| {
            tauri::async_runtime::spawn_blocking(move || responder.respond(protocol_response(request)));
        })
        .register_asynchronous_uri_scheme_protocol("nickel-video", |_context, request, responder| {
            tauri::async_runtime::spawn_blocking(move || responder.respond(video_protocol_response(request)));
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("nickel-image-{}", crate::store::random_token()));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn file(&self, name: &str, bytes: &[u8]) -> std::path::PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn raster_header_matches_extension_and_case() {
        for (name, bytes, mime) in [
            ("a.PNG", b"\x89PNG\r\n\x1a\n".as_slice(), "image/png"),
            ("a.jpeg", &[0xff, 0xd8, 0xff], "image/jpeg"),
            ("a.gif", b"GIF89a".as_slice(), "image/gif"),
            ("a.webp", b"RIFFxxxxWEBP".as_slice(), "image/webp"),
            ("a.bmp", b"BM".as_slice(), "image/bmp"),
            ("a.avif", b"\0\0\0\x18ftypavif\0\0\0\0avif".as_slice(), "image/avif"),
        ] {
            assert_eq!(raster_mime(Path::new(name), bytes), Some(mime));
        }
        for name in ["secrets.local.json", "credentials.key", "image.svg", "image.png"] {
            assert!(raster_mime(Path::new(name), b"{\"apiKey\":\"private-fixture\"}").is_none());
        }
        assert!(raster_mime(Path::new("a.avif"), b"ftyp").is_none());
    }

    #[test]
    fn ipc_and_protocol_reject_credentials_renamed_files_and_svg_without_values() {
        let fixture = Fixture::new();
        for name in ["secrets.local.json", "credentials.key", "accounts.json", "image.png", "image.svg"] {
            let path = fixture.file(name, b"{\"apiKey\":\"private-fixture\"}");
            let error = image_data_url(path.to_string_lossy().into_owned()).unwrap_err();
            assert!(!error.contains("private-fixture"));
            assert!(!error.contains(&path.to_string_lossy().to_string()));
            let encoded = percent_encoding::utf8_percent_encode(&path.to_string_lossy(), percent_encoding::NON_ALPHANUMERIC).to_string();
            let response = protocol_response(tauri::http::Request::builder()
                .uri(format!("nickel-image://localhost/{encoded}")).body(Vec::new()).unwrap());
            assert_eq!(response.status(), 404);
            assert!(response.body().is_empty());
        }
        assert!(image_data_url("relative.png".into()).is_err());
    }

    #[test]
    fn ipc_and_protocol_serve_the_same_validated_image_with_nosniff() {
        let fixture = Fixture::new();
        let bytes = b"\x89PNG\r\n\x1a\nimage-fixture";
        let path = fixture.file("wall paper.PNG", bytes);
        let data = image_data_url(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(data, format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)));
        let encoded = percent_encoding::utf8_percent_encode(&path.to_string_lossy(), percent_encoding::NON_ALPHANUMERIC).to_string();
        let response = protocol_response(tauri::http::Request::builder()
            .uri(format!("http://nickel-image.localhost/{encoded}")).body(Vec::new()).unwrap());
        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), bytes);
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert!(!response.headers().contains_key("access-control-allow-origin"));
    }

    #[test]
    fn oversized_files_traversal_and_non_get_requests_are_rejected() {
        let fixture = Fixture::new();
        let path = fixture.file("large.png", b"\x89PNG\r\n\x1a\n");
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(MAX_BYTES + 1).unwrap();
        assert!(image_data_url(path.to_string_lossy().into_owned()).is_err());
        assert!(check_local_path(&fixture.0.join("other/../image.png"), INVALID_IMAGE).is_err());
        let response = protocol_response(tauri::http::Request::builder().method("POST")
            .uri("nickel-image://localhost/image.png").body(Vec::new()).unwrap());
        assert_eq!(response.status(), 404);
    }

    #[test]
    fn rejected_videos_report_the_video_message_instead_of_the_image_one() {
        let fixture = Fixture::new();
        assert_eq!(open_video(Path::new("relative.mp4")).unwrap_err(), INVALID_VIDEO);
        assert_eq!(open_video(&fixture.0.join("other/../wall.mp4")).unwrap_err(), INVALID_VIDEO);
        assert_eq!(read_image(Path::new("relative.png")).unwrap_err(), INVALID_IMAGE);
    }

    #[cfg(windows)]
    #[test]
    fn network_share_prefixes_are_allowed_and_device_namespaces_are_rejected() {
        for path in [r"\\server\share\wall.png", r"\\?\UNC\server\share\wall.mp4", r"C:\wall.png", r"\\?\C:\wall.webm"] {
            assert!(check_local_path(Path::new(path), INVALID_IMAGE).is_ok(), "{path}");
        }
        for path in [r"\\.\pipe\wall.png", r"\\.\C:\wall.png", r"\\server\share\secrets.local.json"] {
            assert!(check_local_path(Path::new(path), INVALID_IMAGE).is_err(), "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn final_symlink_to_private_file_is_rejected_but_image_ancestor_redirect_is_supported() {
        let fixture = Fixture::new();
        let secret = fixture.file("secrets.local.json", b"private-fixture");
        let link = fixture.0.join("linked.png");
        std::os::unix::fs::symlink(secret, &link).unwrap();
        assert!(image_data_url(link.to_string_lossy().into_owned()).is_err());
        fixture.file("image.png", b"\x89PNG\r\n\x1a\nfixture");
        let ancestor = fixture.0.join("redirect");
        std::os::unix::fs::symlink(&fixture.0, &ancestor).unwrap();
        assert!(image_data_url(ancestor.join("image.png").to_string_lossy().into_owned()).is_ok());
    }
    fn video_request(path: &Path, method: &str, range: Option<&str>) -> tauri::http::Request<Vec<u8>> {
        let encoded = percent_encoding::utf8_percent_encode(&path.to_string_lossy(), percent_encoding::NON_ALPHANUMERIC).to_string();
        let mut request = tauri::http::Request::builder().method(method)
            .uri(format!("http://nickel-video.localhost/{encoded}"));
        if let Some(range) = range { request = request.header("range", range); }
        request.body(Vec::new()).unwrap()
    }

    const MP4: &[u8] = b"\0\0\0\x18ftypisom\0\0\0\0isommp42video-fixture";

    #[test]
    fn video_headers_require_matching_extensions_and_real_container_brands() {
        for (name, bytes, mime) in [
            ("a.MP4", MP4, "video/mp4"),
            ("a.m4v", MP4, "video/mp4"),
            ("a.webm", b"\x1a\x45\xdf\xa3\x42\x82\x84webmfixture".as_slice(), "video/webm"),
            ("a.ogv", b"OggSfixture\x80theora".as_slice(), "video/ogg"),
        ] { assert_eq!(video_mime(Path::new(name), bytes), Some(mime)); }
        for bytes in [b"private-credential".as_slice(), b"ftyp", b"\0\0\0\x18ftypavif\0\0\0\0avifavis", b"\0\0\0\xffftypisom"] {
            assert!(video_mime(Path::new("a.mp4"), bytes).is_none());
        }
        for name in ["a.png", "a.svg", "accounts.json", "credentials.key", "a.webm", "a.ogv"] {
            assert!(video_mime(Path::new(name), MP4).is_none());
        }
        assert!(video_mime(Path::new("a.webm"), b"\x1a\x45\xdf\xa3webm").is_none());
        assert!(video_mime(Path::new("a.ogv"), b"OggSOpusHead").is_none());
    }

    #[test]
    fn video_ranges_accept_bounded_open_and_suffix_ranges_and_reject_malformed_input() {
        assert!(video_range(None, 0).is_err());
        assert_eq!(video_range(Some("bytes=2-5"), 32), Ok((2, 5)));
        assert_eq!(video_range(Some("bytes=2-"), 32), Ok((2, 31)));
        assert_eq!(video_range(Some("bytes=-4"), 32), Ok((28, 31)));
        assert_eq!(video_range(Some("bytes=0-99"), 32), Ok((0, 31)));
        assert_eq!(video_range(None, MAX_VIDEO_BYTES), Ok((0, MAX_VIDEO_CHUNK - 1)));
        assert_eq!(video_range(Some("bytes=0-"), MAX_VIDEO_BYTES), Ok((0, MAX_VIDEO_CHUNK - 1)));
        for value in ["bytes=", "bytes=-", "bytes=-0", "bytes=32-", "bytes=6-2", "bytes=0-1,3-4", "items=0-1", "bytes=+1-2", "bytes= 1-2", "bytes=18446744073709551616-"] {
            assert!(video_range(Some(value), 32).is_err(), "{value}");
        }
    }

    #[test]
    fn video_protocol_supports_head_seek_and_loop_without_cors_or_private_values() {
        let fixture = Fixture::new();
        let path = fixture.file("video space.MP4", MP4);
        validate_background_file(&path).unwrap();
        let head = video_protocol_response(video_request(&path, "HEAD", None));
        assert_eq!(head.status(), 200);
        assert!(head.body().is_empty());
        assert_eq!(head.headers()["content-length"], MP4.len().to_string());
        for range in [None, Some("bytes=0-"), Some("bytes=4-11"), Some("bytes=-4")] {
            let response = video_protocol_response(video_request(&path, "GET", range));
            assert_eq!(response.status(), if range.is_some() { 206 } else { 200 });
            assert_eq!(response.headers()["content-type"], "video/mp4");
            assert_eq!(response.headers()["x-content-type-options"], "nosniff");
            assert_eq!(response.headers()["accept-ranges"], "bytes");
            assert!(!response.headers().contains_key("access-control-allow-origin"));
            let (start, end) = video_range(range, MP4.len() as u64).unwrap();
            assert_eq!(response.body(), &MP4[start as usize..=end as usize]);
            if range.is_some() {
                assert_eq!(response.headers()["content-range"], format!("bytes {start}-{end}/{}", MP4.len()));
            }
        }
    }

    #[test]
    fn large_video_requests_read_only_four_mib_and_invalid_ranges_return_416() {
        let fixture = Fixture::new();
        let path = fixture.file("large.mp4", MP4);
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(MAX_VIDEO_CHUNK * 3).unwrap();
        for range in [None, Some("bytes=0-"), Some("bytes=10-99999999")] {
            let response = video_protocol_response(video_request(&path, "GET", range));
            assert_eq!(response.status(), 206);
            assert_eq!(response.body().len() as u64, MAX_VIDEO_CHUNK);
        }
        // Intentional RFC deviation: a plain GET larger than one chunk is still a bounded 206.
        let plain = video_protocol_response(video_request(&path, "GET", None));
        assert_eq!(plain.status(), 206);
        assert_eq!(plain.headers()["content-range"], format!("bytes 0-{}/{}", MAX_VIDEO_CHUNK - 1, MAX_VIDEO_CHUNK * 3));
        assert_eq!(plain.headers()["content-length"], MAX_VIDEO_CHUNK.to_string());
        let response = video_protocol_response(video_request(&path, "GET", Some("bytes=99999999-")));
        assert_eq!(response.status(), 416);
        assert_eq!(response.headers()["content-range"], format!("bytes */{}", MAX_VIDEO_CHUNK * 3));
        assert!(response.body().is_empty());
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(MAX_VIDEO_BYTES + 1).unwrap();
        assert!(validate_background_file(&path).is_err());
        assert_eq!(video_protocol_response(video_request(&path, "GET", None)).status(), 404);
    }

    #[test]
    fn video_protocol_rejects_credentials_traversal_queries_svg_and_wrong_methods() {
        let fixture = Fixture::new();
        for name in ["secrets.local.json", "credentials.key", "accounts.json", "active_account.json", "renamed.mp4", "image.svg"] {
            let path = fixture.file(name, b"private-video-fixture");
            assert!(validate_background_file(&path).is_err());
            for method in ["GET", "HEAD", "POST"] {
                let response = video_protocol_response(video_request(&path, method, None));
                assert_eq!(response.status(), 404);
                assert!(response.body().is_empty());
            }
        }
        let path = fixture.file("valid.mp4", MP4);
        assert_eq!(video_protocol_response(video_request(&path, "POST", None)).status(), 404);
        let traversal = fixture.0.join("other/../valid.mp4");
        assert_eq!(video_protocol_response(video_request(&traversal, "GET", None)).status(), 404);
        let mut request = video_request(&path, "GET", None);
        *request.uri_mut() = format!("{}?extra=1", request.uri()).parse().unwrap();
        assert_eq!(video_protocol_response(request).status(), 404);
        assert!(open_video(Path::new("relative.mp4")).is_err());
        assert!(image_data_url(path.to_string_lossy().into_owned()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn video_file_symlinks_are_rejected_and_ancestor_redirects_remain_supported() {
        let fixture = Fixture::new();
        let path = fixture.file("valid.mp4", MP4);
        let link = fixture.0.join("linked.mp4");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(open_video(&link).is_err());
        let ancestor = fixture.0.join("redirect");
        std::os::unix::fs::symlink(&fixture.0, &ancestor).unwrap();
        assert!(open_video(&ancestor.join("valid.mp4")).is_ok());
    }

}
