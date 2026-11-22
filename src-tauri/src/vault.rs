//! 皮肤站密码的持久化加密。
//!
//! LittleSkin 已绑定角色的令牌不能枚举或改绑角色，切换时需要重新 `authenticate`。
//! 保存的密码须经 AES-256-GCM 加密后写入 `accounts.json`，每次使用独立随机 nonce。
//! 密钥首次创建为 32 字节随机数，位于配置目录的 `credentials.key`（Unix 权限 0600）。
//! 密文格式保持 `v1:<base64(nonce || ciphertext || tag)>`，读取时须检查版本前缀。
//! 解密不得创建或覆盖密钥；错误信息不得包含明文、密文内容或密钥。

use std::path::{Path, PathBuf};
use std::time::Duration;

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use tauri::AppHandle;

/// 密钥文件名（与 `accounts.json` 同目录）。
const KEY_FILE_NAME: &str = "credentials.key";
/// AES-256 密钥长度。
const KEY_LEN: usize = 32;
/// 并发落位后读取已有密钥的重试次数。
const KEY_READ_ATTEMPTS: usize = 3;
/// 每次重试的等待时间：让先落盘的进程有机会把数据 flush 到磁盘。
const KEY_READ_RETRY_DELAY: Duration = Duration::from_millis(15);
/// GCM nonce 长度。
const NONCE_LEN: usize = 12;
/// 密文版本前缀。
const CIPHER_PREFIX: &str = "v1:";

/// 加密一段明文，返回可直接落盘的密文字符串。
pub fn seal(app: &AppHandle, plaintext: &str) -> Result<String, String> {
    let key = load_or_create_key(&key_path(app)?)?;
    seal_with_key(&key, plaintext)
}

/// 解密 [`seal`] 产生的密文。
///
/// 密钥缺失 / 损坏 / 不匹配时返回可执行错误，由调用方决定是否要求重新登录。
pub fn open(app: &AppHandle, sealed: &str) -> Result<String, String> {
    // 解密只允许使用已有密钥：缺失时生成新密钥会让历史密文全部失效，
    // 用户看到的是「密钥不匹配」，却不知道真正的修复方式是重新登录。
    let key = load_key(&key_path(app)?)?;
    open_with_key(&key, sealed)
}

fn key_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::app_paths::config_dir(app)?.join(KEY_FILE_NAME))
}

/// 读取已有密钥，绝不创建。解密路径必须用它，密钥缺失 / 损坏时提示重新登录。
fn load_key(path: &Path) -> Result<[u8; KEY_LEN], String> {
    match read_key_with_half_write_retry(path)? {
        Some(key) => Ok(key),
        None => Err(format!(
            "本地凭据密钥缺失，请重新登录（{}）",
            path.display()
        )),
    }
}

/// 读取已有密钥；不存在时生成并原子写入。并发创建时以先写入者为准。
fn load_or_create_key(path: &Path) -> Result<[u8; KEY_LEN], String> {
    match read_key_with_half_write_retry(path)? {
        Some(key) => Ok(key),
        None => create_key(path),
    }
}

fn read_key(path: &Path) -> Result<Option<[u8; KEY_LEN]>, String> {
    match std::fs::read(path) {
        Ok(bytes) => key_from_bytes(bytes, path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("读取本地凭据密钥失败: {error}")),
    }
}

/// 读取已有密钥：不存在 → `None`；读取 / 长度异常 → 短暂重试后再判定。
///
/// 不支持 hard_link 的文件系统上，并发 `create_new` 的进程可能只写了部分字节，
/// 立即读取会得到长度异常。这里复用 [`read_existing_key_with_retry`] 的短重试；
/// 仍失败时保留首次错误，绝不新建 / 覆盖（先落盘者为准）。
fn read_key_with_half_write_retry(path: &Path) -> Result<Option<[u8; KEY_LEN]>, String> {
    match read_key(path) {
        Ok(found) => Ok(found),
        Err(error) => match read_existing_key_with_retry(path) {
            Some(existing) => Ok(Some(existing)),
            None => Err(error),
        },
    }
}

fn key_from_bytes(bytes: Vec<u8>, path: &Path) -> Result<[u8; KEY_LEN], String> {
    if bytes.len() != KEY_LEN {
        return Err(format!(
            "本地凭据密钥长度异常（{}），请删除 {} 后重新登录",
            bytes.len(),
            path.display()
        ));
    }
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(&bytes);
    Ok(key)
}

fn create_key(path: &Path) -> Result<[u8; KEY_LEN], String> {
    create_key_with(path, read_key_with_half_write_retry)
}

/// 创建密钥的可注入版本：测试可在「首次读取后、hard_link 前」模拟并发落位，
/// 稳定复现目标已存在但内容半写的窗口。
fn create_key_with(
    path: &Path,
    mut read_existing: impl FnMut(&Path) -> Result<Option<[u8; KEY_LEN]>, String>,
) -> Result<[u8; KEY_LEN], String> {
    // 已有密钥（含并发创建）时直接复用，绝不覆盖；损坏文件也走读取阶段的报错。
    // 半写窗口与初始读取一致：先短重试，仍读不到完整密钥再报错。
    if let Some(existing) = read_existing(path)? {
        return Ok(existing);
    }

    let mut key = [0u8; KEY_LEN];
    getrandom::fill(&mut key).map_err(|_| "生成本地凭据密钥失败".to_string())?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建凭据密钥目录失败: {e}"))?;
    }

    // 先写入同目录临时文件，再用 hard_link 原子“占位”到最终路径：
    // 目标是创建而非覆盖，两个进程同时初始化时只有一个能成功，另一个读取先落盘的密钥。
    // 不能直接 rename —— 在 Unix 上 rename 会覆盖已有文件，导致先写入者的密文作废。
    let temp = key_temp_path(path);
    if let Err(error) = write_key_file(&temp, &key) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }

    let persisted = std::fs::hard_link(&temp, path);
    match persisted {
        Ok(()) => {
            let _ = std::fs::remove_file(&temp);
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(&temp);
            reuse_existing_key(path, error)
        }
        Err(_) => {
            // 不支持硬链接的文件系统（部分网络盘 / FAT32 / 容器挂载）无法用 hard_link
            // 完成原子占位，退化为 create_new + 直写：仍然保证不覆盖已有密钥，
            // 但「创建」与「写入完成」之间仍存在窗口，无法完全保证原子性。
            // 至少保证两点：并发进程已落位时复用它；本调用写坏时清理半写文件，
            // 否则下次启动会读到 0..32 字节的坏密钥，且永远无法自行恢复。
            let fallback = create_key_at(path, &key);
            let _ = std::fs::remove_file(&temp);
            match fallback {
                Ok(()) => Ok(key),
                // 错误优先级：写入错误优先。create_new 失败通常是并发进程已先落位，
                // 读回对方密钥即可；读取本身失败（损坏 / 权限）也不能覆盖写入错误，
                // 否则用户看到的是无法执行的「密钥长度异常」，而不是真正的原因。
                Err(write_error) => match read_existing_key_with_retry(path) {
                    Some(existing) => Ok(existing),
                    None => Err(write_error),
                },
            }
        }
    }
}

/// 目标已存在（并发落位）时复用先写入者的密钥。
///
/// 读取失败不能覆盖写入错误：`read_existing_key_with_retry` 会为半写文件做短重试，
/// 仍读不到完整密钥时把真正的 `write_error` 返回给用户，而不是无法执行的
/// 「密钥长度异常」。
fn reuse_existing_key(path: &Path, write_error: std::io::Error) -> Result<[u8; KEY_LEN], String> {
    match read_existing_key_with_retry(path) {
        Some(existing) => Ok(existing),
        None => Err(format!("创建本地凭据密钥失败: {write_error}")),
    }
}

fn key_temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(KEY_FILE_NAME);
    path.with_file_name(format!(".{name}.tmp-{}", crate::store::random_token()))
}

/// 并发落位后短暂重试读取已有密钥。
///
/// 另一个进程刚 `create_new` 成功时，数据可能还停留在它的页缓存里尚未 flush，
/// 立即读取会得到「不存在」或半写内容；重试几次可以避开这个窗口。
/// 读取错误一律按「没有可用密钥」处理：调用方需要保留自己手里的写入错误。
fn read_existing_key_with_retry(path: &Path) -> Option<[u8; KEY_LEN]> {
    retry_key_read(|| read_key(path))
}

/// 短重试读取密钥；`read` 可注入，便于在测试中稳定模拟「先半写、后完整」的窗口。
fn retry_key_read(
    mut read: impl FnMut() -> Result<Option<[u8; KEY_LEN]>, String>,
) -> Option<[u8; KEY_LEN]> {
    for attempt in 0..KEY_READ_ATTEMPTS {
        match read() {
            Ok(Some(existing)) => return Some(existing),
            Ok(None) | Err(_) => {}
        }
        if attempt + 1 < KEY_READ_ATTEMPTS {
            std::thread::sleep(KEY_READ_RETRY_DELAY);
        }
    }
    None
}

/// 回退路径：直接以 `create_new` 在目标路径写入密钥。
///
/// `create_new` 成功说明目标文件由本次调用创建，因此写失败时删除它是安全的，
/// 不会误删并发进程先落位的密钥；`create_new` 本身失败（`AlreadyExists`）时不删除
/// 任何文件，交给调用方读取对方密钥。
fn create_key_at(path: &Path, key: &[u8; KEY_LEN]) -> Result<(), String> {
    let mut file = open_key_file(path)?;
    if let Err(error) = write_key_contents(&mut file, key) {
        // 先释放句柄：Windows 上仍打开的文件句柄会阻止删除。
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

fn open_key_file(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // 仅当前用户可读写，避免同机其它用户直接取走密钥。
        options.mode(0o600);
    }

    options
        .open(path)
        .map_err(|e| format!("创建本地凭据密钥失败: {e}"))
}

fn write_key_contents(file: &mut std::fs::File, key: &[u8; KEY_LEN]) -> Result<(), String> {
    use std::io::Write;
    file.write_all(key)
        .map_err(|e| format!("写入本地凭据密钥失败: {e}"))?;
    file.sync_all()
        .map_err(|e| format!("写入本地凭据密钥失败: {e}"))
}

fn write_key_file(path: &Path, key: &[u8; KEY_LEN]) -> Result<(), String> {
    let mut file = open_key_file(path)?;
    write_key_contents(&mut file, key)
}

fn seal_with_key(key: &[u8; KEY_LEN], plaintext: &str) -> Result<String, String> {
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| "初始化本地加密失败".to_string())?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce_bytes).map_err(|_| "生成本地加密随机数失败".to_string())?;

    let ciphertext = cipher
        .encrypt(&nonce_bytes.into(), plaintext.as_bytes())
        .map_err(|_| "加密本地凭据失败".to_string())?;

    let mut payload = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    payload.extend_from_slice(&nonce_bytes);
    payload.extend_from_slice(&ciphertext);

    Ok(format!("{CIPHER_PREFIX}{}", BASE64.encode(payload)))
}

fn open_with_key(key: &[u8; KEY_LEN], sealed: &str) -> Result<String, String> {
    let encoded = sealed
        .trim()
        .strip_prefix(CIPHER_PREFIX)
        .ok_or_else(|| "本地凭据格式无法识别，请重新登录".to_string())?;

    let payload = BASE64
        .decode(encoded)
        .map_err(|_| "本地凭据已损坏，请重新登录".to_string())?;

    if payload.len() <= NONCE_LEN {
        return Err("本地凭据已损坏，请重新登录".to_string());
    }

    let (nonce_bytes, ciphertext) = payload.split_at(NONCE_LEN);
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| "初始化本地解密失败".to_string())?;
    let nonce = Nonce::try_from(nonce_bytes)
        .map_err(|_| "本地凭据已损坏，请重新登录".to_string())?;

    let plaintext = cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|_| "本地凭据已失效或密钥不匹配，请重新登录".to_string())?;

    String::from_utf8(plaintext).map_err(|_| "本地凭据内容无法解析，请重新登录".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> [u8; KEY_LEN] {
        [seed; KEY_LEN]
    }

    fn temp_path(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "nickel_vault_{}_{}_{}.key",
            std::process::id(),
            crate::store::random_token(),
            tag
        ));
        path
    }

    #[test]
    fn round_trips_plaintext() {
        let k = key(7);
        let sealed = seal_with_key(&k, "p@ss word 密码").unwrap();
        assert!(sealed.starts_with(CIPHER_PREFIX));
        assert!(!sealed.contains("p@ss"));
        assert_eq!(open_with_key(&k, &sealed).unwrap(), "p@ss word 密码");
    }

    #[test]
    fn reads_existing_v1_standard_gcm_payload_after_crypto_upgrade() {
        // Independently generated AES-256-GCM data, never an account credential.
        let sealed = "v1:AwMDAwMDAwMDAwMDSZvEYjlRczRLbSAujjOaYplVgYOZ8/0IW6v11ACLjVIz/+X1QlhPOCY3PH4=";
        assert_eq!(open_with_key(&key(7), sealed).unwrap(), "legacy-v1-credential-fixture");
        assert!(open_with_key(&key(8), sealed).is_err());
    }

    #[test]
    fn produces_different_ciphertext_per_call() {
        let k = key(9);
        let first = seal_with_key(&k, "same").unwrap();
        let second = seal_with_key(&k, "same").unwrap();
        assert_ne!(first, second, "每次加密必须使用新的 nonce");
    }

    #[test]
    fn rejects_wrong_key() {
        let sealed = seal_with_key(&key(1), "secret").unwrap();
        let error = open_with_key(&key(2), &sealed).unwrap_err();
        assert!(error.contains("重新登录"), "{error}");
        assert!(!error.contains("secret"));
    }

    #[test]
    fn rejects_tampered_or_unknown_payload() {
        let k = key(3);
        let sealed = seal_with_key(&k, "secret").unwrap();

        let mut bytes = BASE64.decode(&sealed[CIPHER_PREFIX.len()..]).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        let tampered = format!("{CIPHER_PREFIX}{}", BASE64.encode(bytes));
        assert!(open_with_key(&k, &tampered).is_err());

        assert!(open_with_key(&k, "not-a-sealed-secret").is_err());
    }

    #[test]
    fn key_file_is_created_once_and_reused() {
        let path = temp_path("create");

        let first = load_or_create_key(&path).unwrap();
        let second = load_or_create_key(&path).unwrap();
        assert_eq!(first, second, "已有密钥必须复用，否则历史密文会全部失效");
        assert_eq!(std::fs::read(&path).unwrap().len(), KEY_LEN);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_corrupted_key_file() {
        let path = temp_path("corrupt");
        std::fs::write(&path, b"too-short").unwrap();

        let error = load_or_create_key(&path).unwrap_err();
        assert!(error.contains("长度异常"), "{error}");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_key_is_error_and_not_created() {
        let path = temp_path("missing");

        // 解密路径只能读取已有密钥：缺失时给出可执行的重新登录提示，且不得落盘新密钥。
        let error = load_key(&path).unwrap_err();
        assert!(error.contains("重新登录"), "{error}");
        assert!(!path.exists(), "读取 / 解密绝不能创建密钥文件");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn create_key_is_atomic_and_reuses_existing_key() {
        let path = temp_path("atomic");
        let first = create_key(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap().as_slice(), first.as_slice());

        // 已有密钥时再次创建必须复用，不能覆盖。
        let second = create_key(&path).unwrap();
        assert_eq!(first, second);

        // 同目录不得残留 `.tmp-*` 临时文件。
        let parent = path.parent().unwrap();
        let prefix = format!(
            ".{}.tmp-",
            path.file_name().unwrap().to_string_lossy()
        );
        let leftovers: Vec<String> = std::fs::read_dir(parent)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(&prefix))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        std::fs::remove_file(&path).ok();
    }

    /// 并发创建时目标已存在且内容半写：必须走短重试读取，
    /// 仍失败则保留写入错误，且绝不覆盖对方文件。
    #[test]
    fn create_key_keeps_write_error_for_half_written_concurrent_target() {
        let path = temp_path("race-half-written");
        let full = key(23);
        let partial = &full[..KEY_LEN / 2];
        let mut first_read = true;

        let error = create_key_with(&path, |candidate| {
            if first_read {
                first_read = false;
                // 首次读取时目标还不存在；随后并发进程 create_new 只写了一半。
                std::fs::write(candidate, partial).unwrap();
                Ok(None)
            } else {
                read_key_with_half_write_retry(candidate)
            }
        })
        .unwrap_err();

        assert!(error.contains("创建本地凭据密钥失败"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap().as_slice(), partial);

        // 同目录不得残留本次调用创建的 `.tmp-*` 文件。
        let parent = path.parent().unwrap();
        let prefix = format!(".{}.tmp-", path.file_name().unwrap().to_string_lossy());
        let leftovers: Vec<String> = std::fs::read_dir(parent)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(&prefix))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn direct_write_reports_error_without_deleting_existing_key() {
        let path = temp_path("direct");
        std::fs::write(&path, b"existing").unwrap();

        // create_new 失败说明目标文件属于并发进程：必须报错，且不能删除 / 覆盖对方数据。
        let error = create_key_at(&path, &key(5)).unwrap_err();
        assert!(error.contains("创建本地凭据密钥失败"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap().as_slice(), b"existing");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn retry_read_prefers_existing_key_and_swallows_read_errors() {
        let path = temp_path("retry");
        std::fs::write(&path, key(11)).unwrap();
        assert_eq!(read_existing_key_with_retry(&path), Some(key(11)));

        // 读取错误（损坏 / 权限）不得冒泡：调用方必须保留自己的写入错误。
        std::fs::write(&path, b"broken").unwrap();
        assert!(read_existing_key_with_retry(&path).is_none());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn retry_read_recovers_from_half_written_key() {
        // 不支持 hard_link 的文件系统上，并发 create_new 的进程可能只写了部分字节：
        // 前两次读到长度异常，第三次才读到完整 32 字节。用可注入的读取函数稳定模拟。
        let mut calls = 0;
        let recovered = retry_key_read(|| {
            calls += 1;
            if calls < KEY_READ_ATTEMPTS {
                Err(format!("本地凭据密钥长度异常（{}）", KEY_LEN / 2))
            } else {
                Ok(Some(key(13)))
            }
        });
        assert_eq!(recovered, Some(key(13)), "半写窗口后必须读回完整密钥");
        assert_eq!(calls, KEY_READ_ATTEMPTS);
    }

    #[test]
    fn load_or_create_key_does_not_overwrite_persistently_half_written_key() {
        let path = temp_path("half-written");
        let partial = &key(19)[..KEY_LEN / 2];
        std::fs::write(&path, partial).unwrap();
        assert!(read_key(&path).is_err(), "半写文件必须被识别为长度异常");

        // 重试后仍不完整：保留首次的长度异常错误，绝不新建 / 覆盖文件。
        let error = load_or_create_key(&path).unwrap_err();
        assert!(error.contains("长度异常"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap().as_slice(), partial);

        std::fs::remove_file(&path).ok();
    }
}
