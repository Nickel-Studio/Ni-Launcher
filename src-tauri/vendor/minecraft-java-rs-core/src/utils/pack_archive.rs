//! 整合包读取与受限子树提取；不执行归档中的任何程序。
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::net::transfer::CancellationToken;

#[derive(Clone, Debug)]
pub struct ArchiveLimits {
    pub max_entries: usize,
    pub max_entry_bytes: u64,
    pub max_total_bytes: u64,
}
impl Default for ArchiveLimits {
    fn default() -> Self {
        Self { max_entries: 100_000, max_entry_bytes: 512 * 1024 * 1024, max_total_bytes: 8 * 1024 * 1024 * 1024 }
    }
}

pub fn validate_relative_path(name: &str) -> Result<String, String> {
    if name.is_empty() || name.starts_with('/') || name.contains('\\') || name.chars().any(|c| matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*')) || name.chars().any(char::is_control) {
        return Err(format!("归档路径无效：{name}"));
    }
    let normalized = name.strip_suffix('/').unwrap_or(name);
    for component in normalized.split('/') {
        if component.is_empty() || component == "." || component == ".." || component.ends_with(['.', ' ']) {
            return Err(format!("归档路径越界或不规范：{name}"));
        }
        let stem = component.split('.').next().unwrap_or_default().to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && matches!(stem.as_bytes()[3], b'1'..=b'9')) {
            return Err(format!("归档路径包含系统设备名：{name}"));
        }
    }
    Ok(normalized.to_owned())
}

struct EntryInfo { index: usize, name: String, size: u64, is_dir: bool }

fn inspect(archive: &mut zip::ZipArchive<File>, source: &Path, limits: &ArchiveLimits, cancel: &CancellationToken) -> Result<Vec<EntryInfo>, String> {
    if archive.len() > limits.max_entries { return Err("归档条目数量超限".into()); }
    let mut central = File::open(source).map_err(|e| e.to_string())?;
    let mut position = archive.central_directory_start();
    let mut names: HashSet<String> = HashSet::new();
    let mut components: HashMap<String, (String, bool)> = HashMap::new();
    let mut total = 0u64;
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        cancel.check()?;
        let entry = archive.by_index(index).map_err(|e| e.to_string())?;
        // zip 按名称索引条目，会隐藏同名的早期条目；直接核对中央目录确保未漏检。
        if entry.central_header_start() != position { return Err("归档包含重复条目".into()); }
        central.seek(SeekFrom::Start(position)).map_err(|e| e.to_string())?;
        let mut header = [0u8; 46];
        central.read_exact(&mut header).map_err(|e| e.to_string())?;
        if &header[..4] != b"PK\x01\x02" { return Err("归档中央目录损坏".into()); }
        position = position.checked_add(46 + [28, 30, 32].iter().map(|&offset| u16::from_le_bytes([header[offset], header[offset + 1]]) as u64).sum::<u64>())
            .ok_or_else(|| "归档目录长度溢出".to_string())?;
        let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
        if mode != 0 && mode != 0o100000 && mode != 0o040000 { return Err("归档不能包含符号链接或特殊文件".into()); }
        // 名称既不是 UTF-8 也无法按 CP437 解码时整包拒绝，不跳过单个条目。
        let name = validate_relative_path(&entry.name().map_err(|e| format!("归档条目名称无效：{e}"))?)?;
        if !names.insert(name.to_lowercase()) { return Err("归档存在重复或大小写冲突条目".into()); }
        register_components(&mut components, &name, entry.is_dir())?;
        total = total.checked_add(entry.size()).ok_or_else(|| "归档大小溢出".to_string())?;
        if entry.size() > limits.max_entry_bytes || total > limits.max_total_bytes { return Err("归档解压大小超过配额".into()); }
        entries.push(EntryInfo { index, name, size: entry.size(), is_dir: entry.is_dir() });
    }
    central.seek(SeekFrom::Start(position)).map_err(|e| e.to_string())?;
    let mut signature = [0u8; 4];
    if central.read_exact(&mut signature).is_ok() && &signature == b"PK\x01\x02" {
        return Err("归档包含未索引的重复条目".into());
    }
    Ok(entries)
}

fn register_components(map: &mut HashMap<String, (String, bool)>, name: &str, directory: bool) -> Result<(), String> {
    let parts: Vec<_> = name.split('/').collect();
    for index in 0..parts.len() {
        let path = parts[..=index].join("/");
        let is_dir = index + 1 < parts.len() || directory;
        let key = path.to_lowercase();
        if let Some((existing, previous_is_dir)) = map.get(&key) {
            if existing != &path || *previous_is_dir != is_dir { return Err("归档存在目录/文件或大小写冲突".into()); }
        } else { map.insert(key, (path, is_dir)); }
    }
    Ok(())
}

fn copy_bounded(reader: &mut impl Read, writer: &mut impl Write, limit: u64, cancel: &CancellationToken) -> Result<u64, String> {
    let mut buffer = [0u8; 64 * 1024];
    let mut written = 0u64;
    loop {
        cancel.check()?;
        let count = reader.read(&mut buffer).map_err(|e| format!("归档 CRC 或内容无效：{e}"))?;
        if count == 0 { break; }
        written = written.checked_add(count as u64).ok_or_else(|| "解压大小溢出".to_string())?;
        if written > limit { return Err("归档解压超过配额".into()); }
        writer.write_all(&buffer[..count]).map_err(|e| e.to_string())?;
    }
    Ok(written)
}

pub fn read_zip_entry(archive: &Path, entry: &str, max_bytes: u64, cancel: &CancellationToken) -> Result<Vec<u8>, String> {
    let requested = validate_relative_path(entry)?;
    let mut zip = zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let entries = inspect(&mut zip, archive, &ArchiveLimits::default(), cancel)?;
    let info = entries.iter().find(|info| info.name == requested && !info.is_dir).ok_or_else(|| format!("归档缺少 {entry}"))?;
    if info.size > max_bytes { return Err("归档清单超过大小限制".into()); }
    let mut data = Vec::new();
    let mut file = zip.by_index(info.index).map_err(|e| e.to_string())?;
    let count = copy_bounded(&mut file, &mut data, max_bytes, cancel)?;
    if count != info.size { return Err("归档条目长度与目录声明不一致".into()); }
    Ok(data)
}

fn protected(path: &str, extra: &[String]) -> bool {
    const RESERVED: &[&str] = &["nickel.profile.json", "nickel.instance.json", "nickel.pack.json", "versions", "libraries", "assets", "runtime", "gamedata.json", "launcher_profiles.json", "nickel.display.json"];
    let path = path.to_lowercase();
    RESERVED.iter().copied().chain(extra.iter().map(String::as_str)).any(|item| {
        let item = item.to_lowercase();
        path == item || path.starts_with(&format!("{item}/"))
    })
}

pub(super) fn reject_links(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        if let Ok(meta) = std::fs::symlink_metadata(ancestor) {
            if meta.file_type().is_symlink() { return Err("提取目标包含符号链接".into()); }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
                if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 { return Err("提取目标包含重解析点".into()); }
            }
        }
    }
    Ok(())
}

pub fn extract_zip_subtrees(
    archive: &Path, destination: &Path, subtrees: &[String], protected_paths: &[String],
    limits: &ArchiveLimits, cancel: &CancellationToken,
) -> Result<Vec<PathBuf>, String> {
    extract_zip_subtrees_with_existing(archive, destination, subtrees, protected_paths, &[], limits, cancel)
}

/// existing_files 是本任务已下载清单中的相对文件路径；预登记目录组件以统一跨阶段名称校验。
/// 仅完全同名的文件允许按子树顺序覆盖；调用方仍须独占暂存目录。
pub fn extract_zip_subtrees_with_existing(
    archive: &Path, destination: &Path, subtrees: &[String], protected_paths: &[String],
    existing_files: &[String], limits: &ArchiveLimits, cancel: &CancellationToken,
) -> Result<Vec<PathBuf>, String> {
    cancel.check()?;
    let mut zip = zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let entries = inspect(&mut zip, archive, limits, cancel)?;
    let mut selected = Vec::new();
    let mut destinations = HashMap::new();
    for file in existing_files {
        cancel.check()?;
        if file.ends_with('/') { return Err("已下载清单必须提供文件路径".into()); }
        let relative = validate_relative_path(file)?;
        register_components(&mut destinations, &relative, false)?;
    }
    let mut trees = HashSet::new();
    let mut total = 0u64;
    // 完成下载清单与整份选中目录校验后才开始写出；不同阶段仅允许完全同名覆盖。
    for subtree in subtrees {
        cancel.check()?;
        let subtree = validate_relative_path(subtree)?;
        if !trees.insert(subtree.to_lowercase()) { return Err("不能重复提取同一子树".into()); }
        let prefix = format!("{subtree}/");
        for info in &entries {
            cancel.check()?;
            let Some(relative) = info.name.strip_prefix(&prefix) else { continue; };
            validate_relative_path(relative)?;
            if protected(relative, protected_paths) { return Err(format!("整合包不能覆盖启动器控制文件：{relative}")); }
            register_components(&mut destinations, relative, info.is_dir)?;
            if !info.is_dir {
                total = total.checked_add(info.size).ok_or_else(|| "解压大小溢出".to_string())?;
                if total > limits.max_total_bytes { return Err("归档解压超过总配额".into()); }
            }
            selected.push((info, destination.join(relative)));
        }
    }
    reject_links(destination)?;
    let mut output = Vec::new();
    for (info, target) in selected {
        cancel.check()?;
        reject_links(&target)?;
        if info.is_dir { std::fs::create_dir_all(&target).map_err(|e| e.to_string())?; continue; }
        if let Some(parent) = target.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        let mut entry = zip.by_index(info.index).map_err(|e| e.to_string())?;
        let mut file = OpenOptions::new().create(true).write(true).truncate(true).open(&target).map_err(|e| e.to_string())?;
        let count = copy_bounded(&mut entry, &mut file, info.size.min(limits.max_entry_bytes), cancel)?;
        if count != info.size { return Err("归档条目长度不一致".into()); }
        file.flush().map_err(|e| e.to_string())?;
        output.push(target);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;
    fn pack(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(file.reopen().unwrap());
        for (path, body) in entries { zip.start_file(*path, SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap(); zip.write_all(body).unwrap(); }
        zip.finish().unwrap(); file
    }
    #[test]
    fn manifest_needs_no_main_class_and_client_overrides_wins() {
        let file = pack(&[("modrinth.index.json", b"{}"), ("overrides/config/a.txt", b"common"), ("client-overrides/config/a.txt", b"client"), ("server-overrides/a", b"ignored")]);
        let dest = tempfile::TempDir::new().unwrap();
        let cancel = CancellationToken::new();
        assert_eq!(read_zip_entry(file.path(), "modrinth.index.json", 16, &cancel).unwrap(), b"{}");
        extract_zip_subtrees(file.path(), dest.path(), &["overrides".into(), "client-overrides".into()], &[], &ArchiveLimits::default(), &cancel).unwrap();
        assert_eq!(std::fs::read(dest.path().join("config/a.txt")).unwrap(), b"client");
        assert!(!dest.path().join("a").exists());
    }
    #[test]
    fn unsafe_paths_are_rejected() {
        for path in ["../escape", "/absolute", "C:/drive", "//server/share", "\\\\server\\share", "a/../b", "a\\b", "NUL.txt", "a./b"] {
            assert!(validate_relative_path(path).is_err(), "{path}");
        }
    }
    #[test]
    fn case_conflicts_control_files_and_limits_are_rejected_before_writing() {
        let cancel = CancellationToken::new();
        for entries in [vec![("overrides/a", b"a".as_slice()), ("overrides/A", b"b".as_slice())], vec![("overrides/nickel.profile.json", b"{}".as_slice())], vec![("overrides/a", b"long".as_slice())]] {
            let file = pack(&entries); let dest = tempfile::TempDir::new().unwrap();
            let limits = ArchiveLimits { max_entry_bytes: 2, ..ArchiveLimits::default() };
            assert!(extract_zip_subtrees(file.path(), dest.path(), &["overrides".into()], &[], &limits, &cancel).is_err());
            assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
        }
    }
    #[test]
    fn cancelled_archive_does_not_write() {
        let file = pack(&[("overrides/a", b"data")]); let dest = tempfile::TempDir::new().unwrap();
        let cancel = CancellationToken::new(); cancel.cancel();
        assert!(extract_zip_subtrees(file.path(), dest.path(), &["overrides".into()], &[], &ArchiveLimits::default(), &cancel).is_err());
    }


    #[test]
    fn zip_symlinks_duplicate_names_and_hidden_entries_are_rejected() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(file.reopen().unwrap());
        zip.add_symlink("overrides/link", "../../outside", SimpleFileOptions::default()).unwrap();
        zip.finish().unwrap();
        let dest = tempfile::TempDir::new().unwrap();
        let cancel = CancellationToken::new();
        assert!(extract_zip_subtrees(file.path(), dest.path(), &["overrides".into()], &[], &ArchiveLimits::default(), &cancel).is_err());
        let file = pack(&[("overrides/a", b"one"), ("overrides/b", b"two")]);
        let bytes = std::fs::read(file.path()).unwrap();
        let replaced = bytes.windows(b"overrides/b".len()).enumerate().filter_map(|(i, bytes)| (bytes == b"overrides/b").then_some(i)).collect::<Vec<_>>();
        let mut bytes = bytes;
        for offset in replaced { bytes[offset..offset + b"overrides/a".len()].copy_from_slice(b"overrides/a"); }
        std::fs::write(file.path(), bytes).unwrap();
        assert!(extract_zip_subtrees(file.path(), dest.path(), &["overrides".into()], &[], &ArchiveLimits::default(), &cancel).is_err());
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
    }

    #[test]
    fn corrupt_crc_and_cross_layer_case_conflicts_are_rejected() {
        let file = pack(&[("overrides/a", b"unique-payload")]);
        let mut bytes = std::fs::read(file.path()).unwrap();
        let offset = bytes.windows(b"unique-payload".len()).position(|bytes| bytes == b"unique-payload").unwrap();
        bytes[offset] ^= 1; std::fs::write(file.path(), bytes).unwrap();
        let cancel = CancellationToken::new();
        assert!(read_zip_entry(file.path(), "overrides/a", 1024, &cancel).is_err());
        let file = pack(&[("overrides/config/a", b"one"), ("client-overrides/Config/a", b"two")]);
        let dest = tempfile::TempDir::new().unwrap();
        assert!(extract_zip_subtrees(file.path(), dest.path(), &["overrides".into(), "client-overrides".into()], &[], &ArchiveLimits::default(), &cancel).is_err());
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
    }

    #[test]
    fn existing_files_preflight_rejects_cross_stage_conflicts_without_writing() {
        for (existing, replacement) in [
            ("mods/A.jar", "overrides/mods/a.jar"),
            ("mods/a.jar", "overrides/mods/A.jar"),
            ("mods/a.jar", "overrides/Mods/b.jar"),
            ("mods/a.jar", "overrides/mods/a.jar/config"),
            ("mods/a.jar", "overrides/mods"),
        ] {
            let file = pack(&[("overrides/config/untouched", b"new"), (replacement, b"replacement")]);
            let dest = tempfile::TempDir::new().unwrap();
            let downloaded = dest.path().join(existing);
            std::fs::create_dir_all(downloaded.parent().unwrap()).unwrap();
            std::fs::write(&downloaded, b"downloaded").unwrap();
            let result = extract_zip_subtrees_with_existing(file.path(), dest.path(), &["overrides".into()], &[], &[existing.into()], &ArchiveLimits::default(), &CancellationToken::new());
            assert!(result.is_err(), "{existing} vs {replacement}");
            assert_eq!(std::fs::read(&downloaded).unwrap(), b"downloaded");
            assert!(!dest.path().join("config").exists());
        }
    }

    #[test]
    fn existing_files_allow_exact_overwrite_and_preserve_layer_priority() {
        let file = pack(&[("overrides/mods/A.jar", b"common"), ("client-overrides/mods/A.jar", b"client")]);
        let dest = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dest.path().join("mods")).unwrap();
        std::fs::write(dest.path().join("mods/A.jar"), b"downloaded").unwrap();
        std::fs::write(dest.path().join("mods/B.jar"), b"kept").unwrap();
        let paths = extract_zip_subtrees_with_existing(file.path(), dest.path(), &["overrides".into(), "client-overrides".into()], &[], &["mods/A.jar".into(), "mods/B.jar".into()], &ArchiveLimits::default(), &CancellationToken::new()).unwrap();
        assert_eq!(paths, vec![dest.path().join("mods/A.jar"); 2]);
        assert_eq!(std::fs::read(dest.path().join("mods/A.jar")).unwrap(), b"client");
        assert_eq!(std::fs::read(dest.path().join("mods/B.jar")).unwrap(), b"kept");
    }

    #[test]
    fn existing_file_names_are_validated_before_extracting() {
        let file = pack(&[("overrides/config/a", b"new")]);
        for existing in [vec!["../escape"], vec!["C:/drive"], vec!["mods/"], vec!["mods/A", "mods/a"], vec!["mods/A", "Mods/B"], vec!["mods", "mods/A"]] {
            let dest = tempfile::TempDir::new().unwrap();
            let existing = existing.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert!(extract_zip_subtrees_with_existing(file.path(), dest.path(), &["overrides".into()], &[], &existing, &ArchiveLimits::default(), &CancellationToken::new()).is_err());
            assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
        }
    }


}
