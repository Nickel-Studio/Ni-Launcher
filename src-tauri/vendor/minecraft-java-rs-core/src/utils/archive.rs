use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::LaunchError;

#[path = "pack_archive.rs"]
mod pack_archive;
pub use pack_archive::{extract_zip_subtrees, extract_zip_subtrees_with_existing, read_zip_entry, validate_relative_path, ArchiveLimits};

#[path = "pack_export.rs"]
mod pack_export;
pub use pack_export::write_zip_files;

/// 同步解压循环共用有界复制，阻塞任务显式携带取消令牌。
pub(crate) fn copy_archive_entry(reader: &mut impl Read, writer: &mut impl std::io::Write, limit: u64, cancel: &crate::net::transfer::CancellationToken) -> Result<u64, LaunchError> {
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        cancel.check().map_err(LaunchError::Archive)?;
        let count = reader.read(&mut buffer)?;
        if count == 0 { break; }
        total = total.checked_add(count as u64).ok_or_else(|| LaunchError::Archive("解压大小溢出".into()))?;
        if total > limit { return Err(LaunchError::Archive("解压超过大小限制".into())); }
        writer.write_all(&buffer[..count])?;
    }
    Ok(total)
}

pub(crate) fn check_extraction_target(path: &Path) -> Result<(), LaunchError> {
    pack_archive::reject_links(path).map_err(LaunchError::Archive)
}


// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ArchiveEntry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

/// Polymorphic result of `get_file_from_archive`.
///
/// - `file = Some(name)` → `FileData` (or `NotFound`)
/// - `file = None, include_dirs = true` → `Entries` with full metadata
/// - `file = None, include_dirs = false` → `Names` of file entries only
///
/// When `prefix` is provided it filters by entry name prefix in all list modes.
#[derive(Debug)]
pub enum ArchiveQueryResult {
    /// Raw bytes of the requested file.
    FileData(Vec<u8>),
    /// Full entry listing (includes directories when `include_dirs = true`).
    Entries(Vec<ArchiveEntry>),
    /// Entry names only (file-only when `include_dirs = false`).
    Names(Vec<String>),
    /// The requested `file` was not found.
    NotFound,
}

// ── ZIP / JAR ─────────────────────────────────────────────────────────────────

const ZIP_CENTRAL_HEADER_LEN: usize = 46;
// Name, extra-field and comment lengths in the ZIP central header.
const ZIP_CENTRAL_LENGTH_OFFSETS: [usize; 3] = [28, 30, 32];

/// Validate an in-memory executable JAR, including every entry's size and CRC.
///
/// Decompression is bounded by `max_uncompressed_bytes`; only the manifest is
/// retained in memory. Async callers should run this inside `spawn_blocking`.
/// `required_entries` adds installer-specific files without restricting other JAR users.
pub fn validate_executable_jar(
    bytes: &[u8],
    max_uncompressed_bytes: u64,
    required_entries: &[&str],
) -> Result<(), LaunchError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| LaunchError::Archive(e.to_string()))?;
    let mut remaining = max_uncompressed_bytes;
    let mut manifest = None;
    let mut central_offset = archive.central_directory_start();

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| LaunchError::Archive(e.to_string()))?;
        // zip indexes entries by name and hides earlier duplicates. Reject such
        // directories rather than leaving a shadowed entry's contents unchecked.
        if entry.central_header_start() != central_offset {
            return Err(LaunchError::Archive(
                "Duplicate or unordered ZIP entries".into(),
            ));
        }
        let header = usize::try_from(central_offset)
            .ok()
            .and_then(|start| bytes.get(start..)?.get(..ZIP_CENTRAL_HEADER_LEN))
            .ok_or_else(|| LaunchError::Archive("Truncated ZIP central directory".into()))?;
        let variable_length: u64 = ZIP_CENTRAL_LENGTH_OFFSETS
            .iter()
            .map(|&offset| u16::from_le_bytes([header[offset], header[offset + 1]]) as u64)
            .sum();
        central_offset += ZIP_CENTRAL_HEADER_LEN as u64 + variable_length;

        let expected_size = entry.size();
        if expected_size > remaining {
            return Err(LaunchError::Archive(
                "JAR exceeds decompressed size limit".into(),
            ));
        }
        let is_manifest = entry
            .name()
            .map_err(|e| LaunchError::Archive(format!("Invalid ZIP entry name: {e}")))?
            == "META-INF/MANIFEST.MF"
            && !entry.is_dir();
        // Reading one byte beyond the budget both detects false size metadata
        // and reaches EOF (where zip verifies CRC) for an exact-budget entry.
        let mut limited = (&mut entry).take(remaining.saturating_add(1));
        let actual_size = if is_manifest {
            let mut data = Vec::new();
            let size = limited.read_to_end(&mut data)? as u64;
            manifest = Some(data);
            size
        } else {
            std::io::copy(&mut limited, &mut std::io::sink())?
        };
        if actual_size > remaining {
            return Err(LaunchError::Archive(
                "JAR exceeds decompressed size limit".into(),
            ));
        }
        if actual_size != expected_size {
            return Err(LaunchError::Archive(
                "ZIP entry size does not match its contents".into(),
            ));
        }
        remaining -= actual_size;
    }
    if usize::try_from(central_offset)
        .ok()
        .and_then(|start| bytes.get(start..))
        .is_some_and(|tail| tail.starts_with(b"PK\x01\x02"))
    {
        return Err(LaunchError::Archive(
            "ZIP contains unindexed entries".into(),
        ));
    }

    let manifest = manifest
        .ok_or_else(|| LaunchError::Archive("JAR is missing META-INF/MANIFEST.MF".into()))?;
    let main_class = manifest_main_class(&manifest)?;
    let class_entry = format!("{}.class", main_class.replace('.', "/"));
    for name in std::iter::once(class_entry.as_str()).chain(required_entries.iter().copied()) {
        let entry = archive
            .by_name(name)
            .map_err(|_| LaunchError::Archive(format!("JAR is missing required entry: {name}")))?;
        if entry.is_dir() || entry.size() == 0 {
            return Err(LaunchError::Archive(format!(
                "JAR entry is not a nonempty file: {name}"
            )));
        }
    }
    Ok(())
}

fn manifest_main_class(manifest: &[u8]) -> Result<String, LaunchError> {
    let mut main_class: Option<Vec<u8>> = None;
    let mut has_header = false;
    let mut is_main_class = false;
    for line in manifest.split(|&byte| byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        // Only the main section defines the executable entry point.
        if line.is_empty() {
            break;
        }
        if let Some(continuation) = line.strip_prefix(b" ") {
            if !has_header {
                return Err(LaunchError::Archive(
                    "Manifest continuation has no header".into(),
                ));
            }
            if is_main_class {
                if let Some(value) = main_class.as_mut() {
                    value.extend_from_slice(continuation);
                }
            }
            continue;
        }
        let separator = line
            .iter()
            .position(|&byte| byte == b':')
            .ok_or_else(|| LaunchError::Archive("Malformed manifest header".into()))?;
        let value = line[separator + 1..]
            .strip_prefix(b" ")
            .ok_or_else(|| LaunchError::Archive("Malformed manifest header value".into()))?;
        has_header = true;
        is_main_class = line[..separator].eq_ignore_ascii_case(b"Main-Class");
        if is_main_class {
            if main_class.is_some() {
                return Err(LaunchError::Archive(
                    "Manifest has duplicate Main-Class headers".into(),
                ));
            }
            main_class = Some(value.to_vec());
        }
    }
    let value =
        main_class.ok_or_else(|| LaunchError::Archive("JAR manifest has no Main-Class".into()))?;
    // Unfold before UTF-8 decoding: a continuation can split a multibyte character.
    let value = std::str::from_utf8(&value)
        .map_err(|e| LaunchError::Archive(format!("Invalid Main-Class: {e}")))?
        .trim();
    if value.split('.').any(|part| {
        part.is_empty()
            || part
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | '\0'))
    }) {
        return Err(LaunchError::Archive(
            "JAR manifest has an invalid Main-Class".into(),
        ));
    }
    Ok(value.to_string())
}

/// Query a ZIP or JAR archive.
///
/// All I/O is done inside `spawn_blocking` so this is safe to `.await` from
/// an async context without blocking the Tokio runtime.
pub async fn get_file_from_archive(
    path: PathBuf,
    file: Option<String>,
    prefix: Option<String>,
    include_dirs: bool,
) -> Result<ArchiveQueryResult, LaunchError> {
    let cancel = crate::net::transfer::TransferContext::current().cancellation().clone();
    tokio::task::spawn_blocking(move || {
        query_zip_sync(&path, file.as_deref(), prefix.as_deref(), include_dirs, &cancel)
    })
    .await
    .map_err(|e| LaunchError::Archive(e.to_string()))?
}

fn query_zip_sync(
    path: &Path,
    file: Option<&str>,
    prefix: Option<&str>,
    include_dirs: bool,
    cancel: &crate::net::transfer::CancellationToken,
) -> Result<ArchiveQueryResult, LaunchError> {
    cancel.check().map_err(LaunchError::Archive)?;
    let f = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| LaunchError::Archive(e.to_string()))?;

    if let Some(name) = file {
        return match archive.by_name(name) {
            Ok(mut entry) => {
                let mut data = Vec::new();
                copy_archive_entry(&mut entry, &mut data, ArchiveLimits::default().max_entry_bytes, cancel)?;
                Ok(ArchiveQueryResult::FileData(data))
            }
            Err(zip::result::ZipError::FileNotFound) => Ok(ArchiveQueryResult::NotFound),
            Err(e) => Err(LaunchError::Archive(e.to_string())),
        };
    }

    if include_dirs {
        let mut entries = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            cancel.check().map_err(LaunchError::Archive)?;
            let entry = archive
                .by_index(i)
                .map_err(|e| LaunchError::Archive(e.to_string()))?;
            let name = entry
                .name()
                .map_err(|e| LaunchError::Archive(format!("Invalid ZIP entry name: {e}")))?
                .into_owned();
            if let Some(p) = prefix {
                if !name.starts_with(p) {
                    continue;
                }
            }
            entries.push(ArchiveEntry {
                is_dir: entry.is_dir(),
                size: entry.size(),
                name,
            });
        }
        Ok(ArchiveQueryResult::Entries(entries))
    } else {
        let mut names = Vec::new();
        for i in 0..archive.len() {
            cancel.check().map_err(LaunchError::Archive)?;
            let entry = archive
                .by_index(i)
                .map_err(|e| LaunchError::Archive(e.to_string()))?;
            if entry.is_dir() {
                continue;
            }
            let name = entry
                .name()
                .map_err(|e| LaunchError::Archive(format!("Invalid ZIP entry name: {e}")))?
                .into_owned();
            if let Some(p) = prefix {
                if !name.starts_with(p) {
                    continue;
                }
            }
            names.push(name);
        }
        Ok(ArchiveQueryResult::Names(names))
    }
}

// ── TAR.GZ ────────────────────────────────────────────────────────────────────

/// Extract a `.tar.gz` archive to `dest`, optionally stripping leading
/// path components (e.g. `strip_components = 1` removes the top-level
/// directory that most JDK tarballs include).
///
/// All I/O is done inside `spawn_blocking`.
pub async fn extract_tar_gz(
    src: PathBuf,
    dest: PathBuf,
    strip_components: usize,
) -> Result<(), LaunchError> {
    let cancel = crate::net::transfer::TransferContext::current().cancellation().clone();
    tokio::task::spawn_blocking(move || extract_tar_gz_sync(&src, &dest, strip_components, &cancel))
        .await
        .map_err(|e| LaunchError::Archive(e.to_string()))?
}

fn extract_tar_gz_sync(
    src: &Path,
    dest: &Path,
    strip_components: usize,
    cancel: &crate::net::transfer::CancellationToken,
) -> Result<(), LaunchError> {
    cancel.check().map_err(LaunchError::Archive)?;
    let file = std::fs::File::open(src)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    // Java 运行时 TAR 允许仅指向提取根内部的链接，文件全部写完后才创建。
    let mut links: Vec<(PathBuf, PathBuf, bool)> = Vec::new();
    let mut names = std::collections::HashSet::new();
    let limits = ArchiveLimits::default();
    let mut total = 0u64;

    for entry in archive
        .entries()
        .map_err(|e| LaunchError::Archive(e.to_string()))?
    {
        cancel.check().map_err(LaunchError::Archive)?;
        let mut entry = entry.map_err(|e| LaunchError::Archive(e.to_string()))?;

        let raw_path = entry
            .path()
            .map_err(|e| LaunchError::Archive(e.to_string()))?
            .into_owned();

        if raw_path.components().any(|component| !matches!(component, std::path::Component::Normal(_) | std::path::Component::CurDir)) {
            return Err(LaunchError::Archive("Archive path escapes the destination".into()));
        }
        let stripped: PathBuf = raw_path.components().skip(strip_components).collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }

        let out = dest.join(&stripped);
        pack_archive::reject_links(&out).map_err(LaunchError::Archive)?;
        if !names.insert(stripped.clone()) || names.len() > limits.max_entries {
            return Err(LaunchError::Archive("归档包含重复条目或条目数量超限".into()));
        }
        total = total.checked_add(entry.size()).ok_or_else(|| LaunchError::Archive("解压大小溢出".into()))?;
        if entry.size() > limits.max_entry_bytes || total > limits.max_total_bytes {
            return Err(LaunchError::Archive("归档解压超过配额".into()));
        }

        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&out)?;
        } else if entry.header().entry_type().is_file() {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut output = std::fs::File::create(&out)?;
            copy_archive_entry(&mut entry, &mut output, ArchiveLimits::default().max_entry_bytes, cancel)?;
            #[cfg(unix)] {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(mode) = entry.header().mode() { std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o777))?; }
            }
        } else if entry.header().entry_type().is_symlink() || entry.header().entry_type().is_hard_link() {
            let target = entry.link_name().map_err(|e| LaunchError::Archive(e.to_string()))?
                .ok_or_else(|| LaunchError::Archive("归档链接缺少目标".into()))?.into_owned();
            let symbolic = entry.header().entry_type().is_symlink();
            let base = if symbolic { stripped.parent().unwrap_or(Path::new("")).to_path_buf() } else { PathBuf::new() };
            let target = if symbolic { target } else { target.components().skip(strip_components).collect() };
            let resolved = resolve_runtime_link(&base, &target)?;
            links.push((out, dest.join(resolved), symbolic));
        } else {
            return Err(LaunchError::Archive("归档包含不支持的特殊文件".into()));
        }
    }
    for (out, target, symbolic) in links {
        cancel.check().map_err(LaunchError::Archive)?;
        pack_archive::reject_links(&out).map_err(LaunchError::Archive)?;
        pack_archive::reject_links(&target).map_err(LaunchError::Archive)?;
        if let Some(parent) = out.parent() { std::fs::create_dir_all(parent)?; }
        if symbolic {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &out)?;
            #[cfg(not(unix))]
            std::fs::copy(&target, &out)?;
        } else { std::fs::hard_link(&target, &out)?; }
    }

    Ok(())
}

fn resolve_runtime_link(base: &Path, target: &Path) -> Result<PathBuf, LaunchError> {
    let mut resolved = base.to_path_buf();
    for component in target.components() {
        match component {
            std::path::Component::Normal(part) => resolved.push(part),
            std::path::Component::CurDir => {},
            std::path::Component::ParentDir if resolved.pop() => {},
            _ => return Err(LaunchError::Archive("运行时链接越过解压目录".into())),
        }
    }
    if resolved.as_os_str().is_empty() { return Err(LaunchError::Archive("运行时链接指向解压根目录".into())); }
    Ok(resolved)
}

/// 普通 JAR/ZIP 完整性验证，不要求可执行 Main-Class，也不做整合包提取。
pub fn validate_zip_file(path: &Path, cancel: &crate::net::transfer::CancellationToken) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("下载文件不是有效 ZIP：{e}"))?;
    let limits = ArchiveLimits::default();
    if archive.len() > limits.max_entries { return Err("ZIP 条目数量超限".into()); }
    let mut remaining = limits.max_total_bytes;
    for index in 0..archive.len() {
        cancel.check()?;
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let expected = entry.size();
        let actual = copy_archive_entry(&mut entry, &mut std::io::sink(), remaining.min(limits.max_entry_bytes), cancel).map_err(|e| e.to_string())?;
        if expected != actual { return Err("ZIP 内容长度与声明不一致".into()); }
        remaining -= actual;
    }
    Ok(())
}



// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    // Build an in-memory ZIP and write it to a temp file.
    fn make_test_zip() -> NamedTempFile {
        use zip::write::SimpleFileOptions;

        let mut tmp = NamedTempFile::new().unwrap();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            let opts = SimpleFileOptions::default();

            w.add_directory("META-INF/", opts).unwrap();
            w.start_file("META-INF/MANIFEST.MF", opts).unwrap();
            w.write_all(b"Manifest-Version: 1.0\n").unwrap();

            w.start_file("data/hello.txt", opts).unwrap();
            w.write_all(b"hello world").unwrap();

            w.start_file("data/world.txt", opts).unwrap();
            w.write_all(b"world hello").unwrap();

            let finished = w.finish().unwrap();
            tmp.write_all(finished.get_ref()).unwrap();
        }
        tmp
    }

    const JAR_TEST_LIMIT: u64 = 1024 * 1024;
    const TEST_MANIFEST: &[u8] = b"Manifest-Version: 1.0\r\nMain-Class: example.Main\r\n\r\n";
    // Java 8 example.Main with public static void main(String[] args) { return; }.
    const TEST_CLASS: &[u8] = b"\xca\xfe\xba\xbe\x00\x00\x00\x34\x00\x08\x01\x00\x0cexample/Main\x07\x00\x01\x01\x00\x10java/lang/Object\x07\x00\x03\x01\x00\x04main\x01\x00\x16([Ljava/lang/String;)V\x01\x00\x04Code\x00\x21\x00\x02\x00\x04\x00\x00\x00\x00\x00\x01\x00\x09\x00\x05\x00\x06\x00\x01\x00\x07\x00\x00\x00\x0d\x00\x00\x00\x01\x00\x00\x00\x01\xb1\x00\x00\x00\x00\x00\x00";

    fn make_jar(entries: &[(&str, &[u8])], compression: zip::CompressionMethod) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(compression);
        for &(name, data) in entries {
            writer.start_file(name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn executable_jar(manifest: &[u8], extras: &[(&str, &[u8])]) -> Vec<u8> {
        let mut entries = vec![
            ("META-INF/MANIFEST.MF", manifest),
            ("example/Main.class", TEST_CLASS),
        ];
        entries.extend_from_slice(extras);
        make_jar(&entries, zip::CompressionMethod::Stored)
    }

    #[test]
    fn executable_jar_rejects_empty_html_and_truncated_zip() {
        for bytes in [b"".as_slice(), b"<html>error</html>", b"PK\x03\x04test"] {
            assert!(validate_executable_jar(bytes, JAR_TEST_LIMIT, &[]).is_err());
        }
        let bytes = executable_jar(TEST_MANIFEST, &[]);
        let archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        for end in [archive.central_directory_start() as usize, bytes.len() - 1] {
            assert!(validate_executable_jar(&bytes[..end], JAR_TEST_LIMIT, &[]).is_err());
        }
    }

    #[test]
    fn executable_jar_accepts_main_section_and_folded_crlf_or_lf() {
        for manifest in [
            TEST_MANIFEST,
            b"Manifest-Version: 1.0\nMain-Class: example.Main\n\n",
            b"Manifest-Version: 1.0\r\nMain-Class: example.\r\n Main\r\n\r\n",
            b"Manifest-Version: 1.0\nmain-class: example.\n Ma\n in\n\n",
            b"Manifest-Version: 1.0\nMain-Class: example.Main\n\nName: other\nMain-Class: absent.Main\n\n",
        ] {
            let bytes = executable_jar(manifest, &[]);
            validate_executable_jar(&bytes, JAR_TEST_LIMIT, &[]).unwrap();
        }
    }

    #[test]
    fn executable_jar_rejects_non_executable_archives() {
        let no_manifest = make_jar(
            &[("example/Main.class", TEST_CLASS)],
            zip::CompressionMethod::Stored,
        );
        let no_class = make_jar(
            &[("META-INF/MANIFEST.MF", TEST_MANIFEST)],
            zip::CompressionMethod::Stored,
        );
        for bytes in [
            no_manifest,
            no_class,
            make_jar(&[], zip::CompressionMethod::Stored),
        ] {
            assert!(validate_executable_jar(&bytes, JAR_TEST_LIMIT, &[]).is_err());
        }
        for manifest in [
            b"Manifest-Version: 1.0\n\n".as_slice(),
            b"Manifest-Version: 1.0\nMain-Class: \n\n",
            b"Manifest-Version: 1.0\n\nName: other\nMain-Class: example.Main\n\n",
            b"Manifest-Version: 1.0\nMain-Class: missing.Main\n\n",
            b"Manifest-Version: 1.0\nMain-Class: example.Main\nMain-Class: example.Main\n\n",
        ] {
            assert!(
                validate_executable_jar(&executable_jar(manifest, &[]), JAR_TEST_LIMIT, &[])
                    .is_err()
            );
        }
    }

    #[test]
    fn executable_jar_requires_profile_only_when_requested() {
        let bytes = executable_jar(TEST_MANIFEST, &[]);
        validate_executable_jar(&bytes, JAR_TEST_LIMIT, &[]).unwrap();
        assert!(
            validate_executable_jar(&bytes, JAR_TEST_LIMIT, &["install_profile.json"]).is_err()
        );
        let with_profile = executable_jar(TEST_MANIFEST, &[("install_profile.json", b"{}")]);
        validate_executable_jar(&with_profile, JAR_TEST_LIMIT, &["install_profile.json"]).unwrap();
    }

    #[test]
    fn executable_jar_rejects_crc_corruption_in_unrelated_entry() {
        let mut bytes = executable_jar(TEST_MANIFEST, &[("payload.bin", b"payload")]);
        let offset = {
            let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
            let entry = archive.by_name("payload.bin").unwrap();
            // zip 9 reports the data offset only once the local header has been read.
            entry.data_start().expect("payload data offset") as usize
        };
        bytes[offset] ^= 1;
        let error = validate_executable_jar(&bytes, JAR_TEST_LIMIT, &[]).unwrap_err();
        assert!(error.to_string().contains("checksum"), "{error}");
    }

    #[test]
    fn executable_jar_limits_total_decompression_including_forged_sizes() {
        let payload = vec![0; 16 * 1024];
        let entries = [
            ("META-INF/MANIFEST.MF", TEST_MANIFEST),
            ("example/Main.class", TEST_CLASS),
            ("payload.bin", payload.as_slice()),
        ];
        let mut bytes = make_jar(&entries, zip::CompressionMethod::Deflated);
        let total = entries
            .iter()
            .map(|(_, data)| data.len() as u64)
            .sum::<u64>();
        validate_executable_jar(&bytes, total, &[]).unwrap();
        let error = validate_executable_jar(&bytes, total - 1, &[]).unwrap_err();
        assert!(error.to_string().contains("size limit"), "{error}");

        let offset = {
            let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
            let entry = archive.by_name("payload.bin").unwrap();
            entry.central_header_start() as usize
        };
        // Forge the central directory's uncompressed size; the streaming budget
        // must still reject data larger than the declared total. zip 9 also enforces
        // the declared size while inflating, so the forged entry may fail inside zip
        // before our own byte count runs. Either rejection is fine; what must never
        // happen is accepting the entry.
        bytes[offset + 24..offset + 28].copy_from_slice(&0u32.to_le_bytes());
        for (budget, ours) in [(total - 1, "size limit"), (total, "size does not match")] {
            let error = validate_executable_jar(&bytes, budget, &[]).unwrap_err().to_string();
            assert!(
                error.contains(ours) || error.contains("declared uncompressed size"),
                "{error}"
            );
        }
    }

    #[test]
    fn executable_jar_rejects_duplicate_names_that_hide_entries() {
        let mut bytes = executable_jar(
            TEST_MANIFEST,
            &[("payload-one", b"one"), ("payload-two", b"two")],
        );
        let offset = {
            let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
            let entry = archive.by_name("payload-two").unwrap();
            entry.central_header_start() as usize
        };
        bytes[offset + 46..offset + 46 + "payload-one".len()].copy_from_slice(b"payload-one");
        assert!(validate_executable_jar(&bytes, JAR_TEST_LIMIT, &[]).is_err());
    }

    #[tokio::test]
    async fn read_specific_file() {
        let zip_file = make_test_zip();
        let result = get_file_from_archive(
            zip_file.path().to_path_buf(),
            Some("META-INF/MANIFEST.MF".into()),
            None,
            false,
        )
        .await
        .unwrap();

        match result {
            ArchiveQueryResult::FileData(data) => {
                assert_eq!(data, b"Manifest-Version: 1.0\n");
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_file_returns_not_found() {
        let zip_file = make_test_zip();
        let result = get_file_from_archive(
            zip_file.path().to_path_buf(),
            Some("does_not_exist.txt".into()),
            None,
            false,
        )
        .await
        .unwrap();

        assert!(matches!(result, ArchiveQueryResult::NotFound));
    }

    #[tokio::test]
    async fn list_all_files_no_dirs() {
        let zip_file = make_test_zip();
        let result = get_file_from_archive(zip_file.path().to_path_buf(), None, None, false)
            .await
            .unwrap();

        match result {
            ArchiveQueryResult::Names(names) => {
                assert!(names.contains(&"META-INF/MANIFEST.MF".to_string()));
                assert!(names.contains(&"data/hello.txt".to_string()));
                assert!(names.contains(&"data/world.txt".to_string()));
                // directory entries excluded
                assert!(!names.iter().any(|n| n == "META-INF/"));
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_with_prefix() {
        let zip_file = make_test_zip();
        let result = get_file_from_archive(
            zip_file.path().to_path_buf(),
            None,
            Some("data/".into()),
            false,
        )
        .await
        .unwrap();

        match result {
            ArchiveQueryResult::Names(names) => {
                assert_eq!(names.len(), 2);
                assert!(names.contains(&"data/hello.txt".to_string()));
                assert!(names.contains(&"data/world.txt".to_string()));
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_all_entries_include_dirs() {
        let zip_file = make_test_zip();
        let result = get_file_from_archive(zip_file.path().to_path_buf(), None, None, true)
            .await
            .unwrap();

        match result {
            ArchiveQueryResult::Entries(entries) => {
                assert!(entries.iter().any(|e| e.is_dir && e.name == "META-INF/"));
                assert!(entries
                    .iter()
                    .any(|e| !e.is_dir && e.name == "data/hello.txt"));
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn extract_tar_gz_strips_root() {
        // Build a simple .tar.gz with one nested file: root/file.txt
        let dest = TempDir::new().unwrap();
        let src = {
            use flate2::write::GzEncoder;
            use flate2::Compression;

            let mut tar_data = Vec::new();
            {
                let enc = GzEncoder::new(&mut tar_data, Compression::fast());
                let mut builder = tar::Builder::new(enc);

                let content = b"tar content";
                let mut header = tar::Header::new_gnu();
                header.set_path("jdk-21/file.txt").unwrap();
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append(&header, content.as_ref()).unwrap();
                builder.finish().unwrap();
            }

            let mut f = NamedTempFile::new().unwrap();
            f.write_all(&tar_data).unwrap();
            f
        };

        extract_tar_gz(src.path().to_path_buf(), dest.path().to_path_buf(), 1)
            .await
            .unwrap();

        let out = dest.path().join("file.txt");
        assert!(out.exists(), "file.txt should exist after extraction");
        assert_eq!(std::fs::read(&out).unwrap(), b"tar content");
    }
}
