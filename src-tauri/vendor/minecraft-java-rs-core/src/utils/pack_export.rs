//! 有界 ZIP 写出；调用方负责内容策略与最终原子提交，本层不跟随链接。
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use super::pack_archive::{ArchiveLimits, reject_links, validate_relative_path};
use crate::net::transfer::CancellationToken;

fn register_path(paths: &mut HashMap<String, (String, bool)>, name: &str) -> Result<(), String> {
    let normalized = validate_relative_path(name)?;
    if normalized != name {
        return Err("导出条目必须是文件路径".into());
    }
    let parts: Vec<_> = name.split('/').collect();
    for index in 0..parts.len() {
        let path = parts[..=index].join("/");
        let directory = index + 1 < parts.len();
        let key = path.to_lowercase();
        if let Some((previous, previous_directory)) = paths.get(&key) {
            if previous != &path || !directory || !previous_directory {
                return Err(format!("导出路径重复、大小写或目录冲突：{name}"));
            }
        } else {
            paths.insert(key, (path, directory));
        }
    }
    Ok(())
}

/// files 为 (ZIP 内路径, root 内相对文件路径)；全部预检通过后才创建 destination。
pub fn write_zip_files(
    destination: &Path,
    root: &Path,
    files: &[(String, String)],
    inline_entries: &[(String, Vec<u8>)],
    limits: &ArchiveLimits,
    cancel: &CancellationToken,
    mut progress: impl FnMut(usize, usize, &str),
) -> Result<(), String> {
    cancel.check()?;
    reject_links(root)?;
    reject_links(destination)?;
    let count = files
        .len()
        .checked_add(inline_entries.len())
        .ok_or("导出条目数量溢出")?;
    if count > limits.max_entries {
        return Err("导出条目数量超过上限".into());
    }
    let mut names = HashMap::new();
    let mut sizes = Vec::new();
    let mut total = 0_u64;
    for (name, relative) in files {
        cancel.check()?;
        register_path(&mut names, name)?;
        validate_relative_path(relative)?;
        let path = root.join(relative);
        reject_links(&path)?;
        let metadata = std::fs::metadata(&path).map_err(|e| format!("读取导出文件失败：{e}"))?;
        if !metadata.is_file() {
            return Err(format!("导出条目不是文件：{relative}"));
        }
        if metadata.len() > limits.max_entry_bytes {
            return Err(format!("导出文件超过配额：{relative}"));
        }
        total = total.checked_add(metadata.len()).ok_or("导出总大小溢出")?;
        sizes.push(metadata.len());
    }
    for (name, bytes) in inline_entries {
        register_path(&mut names, name)?;
        if bytes.len() as u64 > limits.max_entry_bytes {
            return Err("导出清单超过配额".into());
        }
        total = total
            .checked_add(bytes.len() as u64)
            .ok_or("导出总大小溢出")?;
    }
    if total > limits.max_total_bytes {
        return Err("导出总大小超过配额".into());
    }
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|e| format!("创建导出临时文件失败：{e}"))?;
    let result = (|| {
        let mut archive = zip::ZipWriter::new(output);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut done = 0;
        for (name, bytes) in inline_entries {
            cancel.check()?;
            archive
                .start_file(name, options)
                .map_err(|e| e.to_string())?;
            archive.write_all(bytes).map_err(|e| e.to_string())?;
            done += 1;
            progress(done, count, name);
        }
        let mut buffer = [0_u8; 64 * 1024];
        for ((name, relative), size) in files.iter().zip(sizes) {
            cancel.check()?;
            let path = root.join(relative);
            reject_links(&path)?;
            let mut input = File::open(&path).map_err(|e| format!("读取导出文件失败：{e}"))?;
            if !input.metadata().map_err(|e| e.to_string())?.is_file() {
                return Err("导出文件已变化".into());
            }
            archive
                .start_file(name, options.large_file(size >= u32::MAX as u64))
                .map_err(|e| e.to_string())?;
            let mut written = 0_u64;
            loop {
                cancel.check()?;
                let bytes = input.read(&mut buffer).map_err(|e| e.to_string())?;
                if bytes == 0 {
                    break;
                }
                written = written
                    .checked_add(bytes as u64)
                    .ok_or("导出文件长度溢出")?;
                if written > size {
                    return Err(format!("导出期间文件发生变化，请重试：{relative}"));
                }
                archive
                    .write_all(&buffer[..bytes])
                    .map_err(|e| e.to_string())?;
            }
            if written != size {
                return Err(format!("导出期间文件发生变化，请重试：{relative}"));
            }
            done += 1;
            progress(done, count, relative);
        }
        cancel.check()?;
        archive
            .finish()
            .map_err(|e| e.to_string())?
            .sync_all()
            .map_err(|e| e.to_string())
    })();
    if result.is_err() {
        if let Err(cleanup) = std::fs::remove_file(destination) {
            return Err(format!(
                "{}；清理临时归档失败：{cleanup}",
                result.err().unwrap_or_default()
            ));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_writes_only_selected_files() {
        let root = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(root.path().join("mods")).unwrap();
        std::fs::write(root.path().join("mods/a.jar"), b"selected").unwrap();
        std::fs::write(root.path().join("secret.txt"), b"excluded").unwrap();
        let output = root.path().join("test.mrpack");
        let cancel = CancellationToken::new();
        write_zip_files(
            &output,
            root.path(),
            &[("overrides/mods/a.jar".into(), "mods/a.jar".into())],
            &[("modrinth.index.json".into(), b"{}".to_vec())],
            &ArchiveLimits::default(),
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        assert_eq!(
            super::super::pack_archive::read_zip_entry(
                &output,
                "overrides/mods/a.jar",
                64,
                &cancel
            )
            .unwrap(),
            b"selected"
        );
        assert!(
            super::super::pack_archive::read_zip_entry(&output, "secret.txt", 64, &cancel).is_err()
        );
    }

    #[test]
    fn cancellation_conflicts_and_limits_never_create_output() {
        let root = tempfile::TempDir::new().unwrap();
        let output = root.path().join("test.mrpack");
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(
            write_zip_files(
                &output,
                root.path(),
                &[],
                &[],
                &ArchiveLimits::default(),
                &cancel,
                |_, _, _| {}
            )
            .is_err()
        );
        for names in [
            vec!["../escape"],
            vec!["A", "a"],
            vec!["a", "a/b"],
            vec!["a/b", "A/c"],
        ] {
            let entries = names
                .into_iter()
                .map(|name| (name.into(), vec![1]))
                .collect::<Vec<_>>();
            assert!(
                write_zip_files(
                    &output,
                    root.path(),
                    &[],
                    &entries,
                    &ArchiveLimits::default(),
                    &CancellationToken::new(),
                    |_, _, _| {}
                )
                .is_err()
            );
        }
        let limits = ArchiveLimits {
            max_total_bytes: 1,
            ..ArchiveLimits::default()
        };
        assert!(
            write_zip_files(
                &output,
                root.path(),
                &[],
                &[("large".into(), vec![1, 2])],
                &limits,
                &CancellationToken::new(),
                |_, _, _| {}
            )
            .is_err()
        );
        assert!(!output.exists());
    }

    #[test]
    fn existing_archive_is_not_overwritten() {
        let root = tempfile::TempDir::new().unwrap();
        let output = root.path().join("test.mrpack");
        std::fs::write(&output, b"existing").unwrap();
        assert!(
            write_zip_files(
                &output,
                root.path(),
                &[],
                &[],
                &ArchiveLimits::default(),
                &CancellationToken::new(),
                |_, _, _| {}
            )
            .is_err()
        );
        assert_eq!(std::fs::read(output).unwrap(), b"existing");
    }
}
