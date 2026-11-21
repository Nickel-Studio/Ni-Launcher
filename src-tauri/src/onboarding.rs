//! 首次使用确认与每版本更新说明；独立存储，不参与外观或整合包迁移。
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

// Internal consent revision, not an asserted publication date of the official terms.
pub const EULA_VERSION: &str = "minecraft-eula-v1";
pub const EULA_URL: &str = "https://www.minecraft.net/zh-hans/eula";
const MAX_BYTES: u64 = 16 * 1024;
const REJECTION_DELAY: Duration = Duration::from_secs(5);
static EXIT_PENDING: AtomicBool = AtomicBool::new(false);
static STARTUP_DISPATCHED: AtomicBool = AtomicBool::new(false);

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Record {
    eula_version: Option<String>,
    eula_accepted_at: Option<u64>,
    changelog_seen_version: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub eula_version: &'static str,
    pub eula_url: &'static str,
    pub eula_accepted_at: Option<u64>,
    pub current_version: &'static str,
    pub needs_eula: bool,
    pub needs_changelog: bool,
    pub exiting: bool,
}

fn path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::app_paths::config_dir(app)?.join("onboarding.json"))
}

fn read(path: &Path) -> Result<Record, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Record::default()),
        Err(_) => return Err("无法读取首次启动记录，请检查配置目录权限后重试".into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取首次启动记录，请重试")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Ok(Record::default());
    }
    Ok(serde_json::from_slice(&bytes).unwrap_or_default())
}

fn valid_timestamp(time: u64) -> bool {
    // IPC uses JavaScript numbers; match Number.isSafeInteger in onboarding.ts.
    (1..=9_007_199_254_740_991).contains(&time)
}

fn accepted(record: &Record) -> bool {
    record.eula_version.as_deref() == Some(EULA_VERSION)
        && record.eula_accepted_at.is_some_and(valid_timestamp)
}

fn status(record: &Record) -> Status {
    let is_accepted = accepted(record);
    Status {
        eula_version: EULA_VERSION,
        eula_url: EULA_URL,
        eula_accepted_at: if is_accepted {
            record.eula_accepted_at
        } else {
            None
        },
        current_version: env!("CARGO_PKG_VERSION"),
        needs_eula: !is_accepted,
        needs_changelog: record.changelog_seen_version.as_deref()
            != Some(env!("CARGO_PKG_VERSION")),
        exiting: EXIT_PENDING.load(Ordering::Acquire),
    }
}

fn write(path: &Path, record: &Record) -> Result<(), String> {
    let parent = path.parent().ok_or("无法定位首次启动记录目录")?;
    std::fs::create_dir_all(parent).map_err(|_| "无法创建首次启动记录目录")?;
    let temporary = parent.join(format!(
        "onboarding.json.tmp-{}",
        crate::store::random_token()
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| "无法保存首次启动记录，请检查配置目录写入权限")?;
        let text = serde_json::to_vec(record).map_err(|_| "无法生成首次启动记录")?;
        file.write_all(&text)
            .and_then(|_| file.sync_all())
            .map_err(|_| "首次启动记录未能完整写入，请重试")?;
        drop(file);
        std::fs::rename(&temporary, path).map_err(|_| "无法保存首次启动记录，请重试")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn require_record_accepted(record: &Record, exiting: bool) -> Result<(), String> {
    if exiting {
        return Err("正在退出，无法开始新的操作".into());
    }
    if !accepted(record) {
        return Err("请先阅读并同意 Minecraft EULA 后继续".into());
    }
    Ok(())
}

fn accept_at(
    path: &Path,
    checked: bool,
    eula_version: &str,
    now: u64,
    exit_pending: &AtomicBool,
) -> Result<Record, String> {
    let _guard = crate::store::lock();
    if !checked || eula_version != EULA_VERSION || exit_pending.load(Ordering::Acquire) {
        return Err("请勾选阅读声明并确认当前 Minecraft EULA 后继续".into());
    }
    let mut record = read(path)?;
    // Preserve the original signature when a lost IPC reply is retried.
    if !accepted(&record) {
        if !valid_timestamp(now) {
            return Err("无法获取有效的签署时间，请检查系统时间后重试".into());
        }
        record.eula_version = Some(EULA_VERSION.into());
        record.eula_accepted_at = Some(now);
        write(path, &record)?;
    }
    Ok(record)
}

fn acknowledge_at(path: &Path, version: &str, exit_pending: &AtomicBool) -> Result<Record, String> {
    let _guard = crate::store::lock();
    let mut record = read(path)?;
    require_record_accepted(&record, exit_pending.load(Ordering::Acquire))?;
    if version != env!("CARGO_PKG_VERSION") {
        return Err("更新说明版本已变化，请重新打开启动器".into());
    }
    if record.changelog_seen_version.as_deref() != Some(version) {
        record.changelog_seen_version = Some(version.into());
        write(path, &record)?;
    }
    Ok(record)
}

fn finish_at(
    path: &Path,
    exit_pending: &AtomicBool,
    startup_dispatched: &AtomicBool,
    dispatch: impl FnOnce(),
) -> Result<(), String> {
    let _guard = crate::store::lock();
    let record = read(path)?;
    require_record_accepted(&record, exit_pending.load(Ordering::Acquire))?;
    if record.changelog_seen_version.as_deref() != Some(env!("CARGO_PKG_VERSION")) {
        return Err("请先确认本版本更新说明".into());
    }
    if !startup_dispatched.swap(true, Ordering::AcqRel) {
        dispatch();
    }
    Ok(())
}

fn begin_rejection(exit_pending: &AtomicBool) -> Option<Instant> {
    // Rejection and consent writes share a lock, so refusal cannot race a late signature.
    let _guard = crate::store::lock();
    if exit_pending.swap(true, Ordering::AcqRel) {
        None
    } else {
        Some(Instant::now() + REJECTION_DELAY)
    }
}

async fn wait_for_rejection(deadline: Instant) {
    tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
}

pub fn require_accepted(app: &AppHandle) -> Result<(), String> {
    require_record_accepted(&read(&path(app)?)?, EXIT_PENDING.load(Ordering::Acquire))
}

#[tauri::command]
pub fn onboarding_get(app: AppHandle) -> Result<Status, String> {
    Ok(status(&read(&path(&app)?)?))
}

#[tauri::command]
pub fn onboarding_accept_eula(
    app: AppHandle,
    checked: bool,
    eula_version: String,
) -> Result<Status, String> {
    let record = accept_at(
        &path(&app)?,
        checked,
        &eula_version,
        crate::store::now_secs(),
        &EXIT_PENDING,
    )?;
    Ok(status(&record))
}

#[tauri::command]
pub fn onboarding_ack_changelog(app: AppHandle, version: String) -> Result<Status, String> {
    let record = acknowledge_at(&path(&app)?, &version, &EXIT_PENDING)?;
    Ok(status(&record))
}

#[tauri::command]
pub fn onboarding_finish(app: AppHandle) -> Result<(), String> {
    finish_at(&path(&app)?, &EXIT_PENDING, &STARTUP_DISPATCHED, || {
        crate::launch_from_startup_arguments(&app);
    })
}

#[tauri::command]
pub fn onboarding_reject(app: AppHandle) {
    let Some(deadline) = begin_rejection(&EXIT_PENDING) else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        wait_for_rejection(deadline).await;
        app.exit(0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "nickel-onboarding-{}",
                crate::store::random_token()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn record(&self) -> PathBuf {
            self.0.join("onboarding.json")
        }

        fn retain(&self, name: &str) {
            if let Some(directory) = std::env::var_os("NICKEL_STARTUP_TEST_EVIDENCE") {
                let directory = PathBuf::from(directory);
                std::fs::create_dir_all(&directory).unwrap();
                std::fs::copy(self.record(), directory.join(format!("{name}.json"))).unwrap();
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn eula_precedes_changelog_and_version_ack_is_scoped() {
        let mut record = Record::default();
        assert!(status(&record).needs_eula);
        assert!(status(&record).needs_changelog);
        record.eula_version = Some(EULA_VERSION.into());
        record.eula_accepted_at = Some(123);
        assert!(!status(&record).needs_eula);
        assert!(status(&record).needs_changelog);
        record.changelog_seen_version = Some(env!("CARGO_PKG_VERSION").into());
        assert!(!status(&record).needs_changelog);
        record.changelog_seen_version = Some("previous-version".into());
        assert!(status(&record).needs_changelog);
        record.eula_version = Some("obsolete-terms".into());
        assert!(status(&record).needs_eula);
        assert_eq!(status(&record).eula_accepted_at, None);
    }

    #[test]
    fn acceptance_roundtrips_without_account_or_appearance_data() {
        let fixture = Fixture::new();
        let pending = AtomicBool::new(false);
        let before = crate::store::now_secs();
        let record = accept_at(&fixture.record(), true, EULA_VERSION, before, &pending).unwrap();
        assert!(accepted(&record));
        let record =
            acknowledge_at(&fixture.record(), env!("CARGO_PKG_VERSION"), &pending).unwrap();
        assert_eq!(record.eula_accepted_at, Some(before));
        assert!(!status(&read(&fixture.record()).unwrap()).needs_changelog);
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.record()).unwrap()).unwrap();
        let mut keys: Vec<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["changelogSeenVersion", "eulaAcceptedAt", "eulaVersion"]
        );
        assert_eq!(value["eulaAcceptedAt"], before);
        fixture.retain("accepted-current-release-real-record");
    }

    #[test]
    fn missing_record_is_first_run_without_implicit_acceptance() {
        let fixture = Fixture::new();
        assert!(status(&read(&fixture.record()).unwrap()).needs_eula);
        assert!(!fixture.record().exists());
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 0);
    }

    #[test]
    fn malformed_records_reask_instead_of_inventing_consent() {
        let fixture = Fixture::new();
        for bytes in [
            b"broken".as_slice(),
            b"null",
            b"[]",
            b"{}",
            b"\xff\xfe",
            br#"{"eulaVersion":"minecraft-eula-v1","eulaAcceptedAt":"123"}"#,
            br#"{"eulaVersion":"minecraft-eula-v1","eulaAcceptedAt":-1}"#,
            br#"{"eulaVersion":"minecraft-eula-v1","eulaAcceptedAt":1.5}"#,
            br#"{"eulaVersion":"minecraft-eula-v1","eulaAcceptedAt":true}"#,
            br#"{"eulaVersion":"minecraft-eula-v1","eulaAcceptedAt":0}"#,
            br#"{"eulaVersion":"minecraft-eula-v1"}"#,
            br#"{"eulaAcceptedAt":123}"#,
        ] {
            std::fs::write(fixture.record(), bytes).unwrap();
            let record = read(&fixture.record()).unwrap();
            assert!(!accepted(&record));
            assert_eq!(status(&record).eula_accepted_at, None);
        }
    }

    #[test]
    fn timestamp_boundaries_match_frontend_and_corrupt_records_can_be_resigned() {
        let fixture = Fixture::new();
        let pending = AtomicBool::new(false);
        for time in [0, 1, 9_007_199_254_740_991, 9_007_199_254_740_992, u64::MAX] {
            let record = Record {
                eula_version: Some(EULA_VERSION.into()),
                eula_accepted_at: Some(time),
                changelog_seen_version: Some(env!("CARGO_PKG_VERSION").into()),
            };
            write(&fixture.record(), &record).unwrap();
            let current = read(&fixture.record()).unwrap();
            let valid = time == 1 || time == 9_007_199_254_740_991;
            assert_eq!(accepted(&current), valid);
            assert_eq!(status(&current).needs_eula, !valid);
            assert_eq!(status(&current).eula_accepted_at, valid.then_some(time));
            assert_eq!(require_record_accepted(&current, false).is_ok(), valid);
            let resigned = accept_at(&fixture.record(), true, EULA_VERSION, 123, &pending).unwrap();
            assert_eq!(
                resigned.eula_accepted_at,
                Some(if valid { time } else { 123 })
            );
            assert!(accepted(&read(&fixture.record()).unwrap()));
            assert!(!status(&resigned).needs_changelog);
        }
    }

    #[test]
    fn invalid_current_time_never_writes_consent() {
        let fixture = Fixture::new();
        let pending = AtomicBool::new(false);
        for time in [0, 9_007_199_254_740_992, u64::MAX] {
            assert!(accept_at(&fixture.record(), true, EULA_VERSION, time, &pending).is_err());
            assert!(!fixture.record().exists());
        }
    }

    #[test]
    fn oversized_record_is_bounded_and_fails_closed() {
        let fixture = Fixture::new();
        let record = Record {
            eula_version: Some(EULA_VERSION.into()),
            eula_accepted_at: Some(123),
            changelog_seen_version: None,
        };
        let mut bytes = serde_json::to_vec(&record).unwrap();
        bytes.resize(MAX_BYTES as usize + 1, b' ');
        std::fs::write(fixture.record(), &bytes).unwrap();
        assert!(!accepted(&read(&fixture.record()).unwrap()));
        assert_eq!(
            std::fs::metadata(fixture.record()).unwrap().len(),
            MAX_BYTES + 1
        );
    }

    #[test]
    fn checked_revision_and_exiting_are_validated_before_any_write() {
        let fixture = Fixture::new();
        let pending = AtomicBool::new(false);
        assert!(accept_at(&fixture.record(), false, EULA_VERSION, 123, &pending).is_err());
        assert!(accept_at(&fixture.record(), true, "obsolete-terms", 123, &pending).is_err());
        pending.store(true, Ordering::Release);
        assert!(accept_at(&fixture.record(), true, EULA_VERSION, 123, &pending).is_err());
        assert!(!fixture.record().exists());
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 0);
    }

    #[test]
    fn invalid_system_clock_does_not_record_a_signature_and_can_retry() {
        let fixture = Fixture::new();
        let pending = AtomicBool::new(false);
        assert!(accept_at(&fixture.record(), true, EULA_VERSION, 0, &pending).is_err());
        assert!(!fixture.record().exists());
        assert!(accepted(
            &accept_at(&fixture.record(), true, EULA_VERSION, 123, &pending).unwrap()
        ));
    }

    #[test]
    fn duplicate_acceptance_preserves_original_signature_and_seen_version() {
        let fixture = Fixture::new();
        let pending = AtomicBool::new(false);
        accept_at(&fixture.record(), true, EULA_VERSION, 123, &pending).unwrap();
        acknowledge_at(&fixture.record(), env!("CARGO_PKG_VERSION"), &pending).unwrap();
        let before = std::fs::read(fixture.record()).unwrap();
        let record = accept_at(&fixture.record(), true, EULA_VERSION, 456, &pending).unwrap();
        assert_eq!(record.eula_accepted_at, Some(123));
        assert!(!status(&record).needs_changelog);
        assert_eq!(std::fs::read(fixture.record()).unwrap(), before);
        fixture.retain("idempotent-signature-retry");
    }

    #[test]
    fn changelog_requires_current_consent_and_current_release() {
        let fixture = Fixture::new();
        let pending = AtomicBool::new(false);
        assert!(acknowledge_at(&fixture.record(), env!("CARGO_PKG_VERSION"), &pending).is_err());
        assert!(!fixture.record().exists());
        accept_at(&fixture.record(), true, EULA_VERSION, 123, &pending).unwrap();
        let before = std::fs::read(fixture.record()).unwrap();
        assert!(acknowledge_at(&fixture.record(), "previous-version", &pending).is_err());
        assert_eq!(std::fs::read(fixture.record()).unwrap(), before);
        pending.store(true, Ordering::Release);
        assert!(acknowledge_at(&fixture.record(), env!("CARGO_PKG_VERSION"), &pending).is_err());
        assert_eq!(std::fs::read(fixture.record()).unwrap(), before);
    }

    #[test]
    fn version_upgrade_changes_only_changelog_acknowledgment() {
        let fixture = Fixture::new();
        let pending = AtomicBool::new(false);
        let old = Record {
            eula_version: Some(EULA_VERSION.into()),
            eula_accepted_at: Some(123),
            changelog_seen_version: Some("1.0.1".into()),
        };
        write(&fixture.record(), &old).unwrap();
        assert!(!status(&read(&fixture.record()).unwrap()).needs_eula);
        assert!(status(&read(&fixture.record()).unwrap()).needs_changelog);
        let current =
            acknowledge_at(&fixture.record(), env!("CARGO_PKG_VERSION"), &pending).unwrap();
        assert_eq!(current.eula_accepted_at, Some(123));
        assert!(!status(&current).needs_changelog);
        fixture.retain("upgraded-release-real-record");
    }

    #[test]
    fn startup_dispatch_waits_for_both_prompts_and_is_idempotent() {
        let fixture = Fixture::new();
        let pending = AtomicBool::new(false);
        let dispatched = AtomicBool::new(false);
        let count = AtomicUsize::new(0);
        let dispatch = || {
            count.fetch_add(1, Ordering::AcqRel);
        };
        assert!(finish_at(&fixture.record(), &pending, &dispatched, dispatch).is_err());
        accept_at(&fixture.record(), true, EULA_VERSION, 123, &pending).unwrap();
        assert!(finish_at(&fixture.record(), &pending, &dispatched, dispatch).is_err());
        assert_eq!(count.load(Ordering::Acquire), 0);
        acknowledge_at(&fixture.record(), env!("CARGO_PKG_VERSION"), &pending).unwrap();
        finish_at(&fixture.record(), &pending, &dispatched, dispatch).unwrap();
        finish_at(&fixture.record(), &pending, &dispatched, dispatch).unwrap();
        assert_eq!(count.load(Ordering::Acquire), 1);
        pending.store(true, Ordering::Release);
        assert!(finish_at(&fixture.record(), &pending, &dispatched, dispatch).is_err());
        assert_eq!(count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn refusal_prevents_new_writes_and_restart_still_needs_eula() {
        let fixture = Fixture::new();
        let pending = AtomicBool::new(false);
        let dispatched = AtomicBool::new(false);
        assert!(begin_rejection(&pending).is_some());
        assert!(begin_rejection(&pending).is_none());
        assert!(accept_at(&fixture.record(), true, EULA_VERSION, 123, &pending).is_err());
        assert!(acknowledge_at(&fixture.record(), env!("CARGO_PKG_VERSION"), &pending).is_err());
        assert!(
            finish_at(&fixture.record(), &pending, &dispatched, || panic!(
                "refusal dispatched startup"
            ))
            .is_err()
        );
        assert!(!fixture.record().exists());
        assert!(status(&read(&fixture.record()).unwrap()).needs_eula);
    }

    #[test]
    fn concurrent_signatures_and_finish_commands_remain_idempotent() {
        let fixture = Fixture::new();
        let file = fixture.record();
        let pending = AtomicBool::new(false);
        let dispatched = AtomicBool::new(false);
        let count = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..12 {
                scope.spawn(|| {
                    accept_at(&file, true, EULA_VERSION, 123, &pending).unwrap();
                });
            }
        });
        std::thread::scope(|scope| {
            for _ in 0..12 {
                scope.spawn(|| {
                    acknowledge_at(&file, env!("CARGO_PKG_VERSION"), &pending).unwrap();
                });
            }
        });
        std::thread::scope(|scope| {
            for _ in 0..12 {
                scope.spawn(|| {
                    finish_at(&file, &pending, &dispatched, || {
                        count.fetch_add(1, Ordering::AcqRel);
                    })
                    .unwrap();
                });
            }
        });
        assert_eq!(count.load(Ordering::Acquire), 1);
        assert_eq!(read(&file).unwrap().eula_accepted_at, Some(123));
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    #[test]
    fn failed_replacement_cleans_temporary_file_and_keeps_previous_record() {
        let fixture = Fixture::new();
        let destination = fixture.record();
        std::fs::create_dir_all(&destination).unwrap();
        let record = Record {
            eula_version: Some(EULA_VERSION.into()),
            eula_accepted_at: Some(123),
            changelog_seen_version: None,
        };
        assert!(write(&destination, &record).is_err());
        assert!(destination.is_dir());
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
        std::fs::remove_dir(&destination).unwrap();
        write(&destination, &record).unwrap();
        assert!(accepted(&read(&destination).unwrap()));
    }

    #[cfg(windows)]
    #[test]
    fn windows_locked_record_failure_then_retry_preserves_acceptance() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = Fixture::new();
        let pending = AtomicBool::new(false);
        accept_at(&fixture.record(), true, EULA_VERSION, 123, &pending).unwrap();
        let before = std::fs::read(fixture.record()).unwrap();
        let blocker = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2)
            .open(fixture.record())
            .unwrap();
        assert!(acknowledge_at(&fixture.record(), env!("CARGO_PKG_VERSION"), &pending).is_err());
        assert_eq!(std::fs::read(fixture.record()).unwrap(), before);
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
        drop(blocker);
        acknowledge_at(&fixture.record(), env!("CARGO_PKG_VERSION"), &pending).unwrap();
        assert_eq!(read(&fixture.record()).unwrap().eula_accepted_at, Some(123));
        fixture.retain("windows-locked-save-retry");
    }

    #[test]
    fn unwritable_parent_failure_does_not_create_partial_acceptance() {
        let fixture = Fixture::new();
        let blocker = fixture.0.join("not-a-directory");
        std::fs::write(&blocker, "fixture sentinel").unwrap();
        let record = Record {
            eula_version: Some(EULA_VERSION.into()),
            eula_accepted_at: Some(123),
            changelog_seen_version: None,
        };
        assert!(write(&blocker.join("onboarding.json"), &record).is_err());
        assert_eq!(
            std::fs::read_to_string(&blocker).unwrap(),
            "fixture sentinel"
        );
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn rejection_uses_real_five_second_deadline_without_rearming() {
        let pending = AtomicBool::new(false);
        let started = Instant::now();
        let deadline = begin_rejection(&pending).unwrap();
        assert!(begin_rejection(&pending).is_none());
        assert!(deadline.duration_since(started) >= REJECTION_DELAY);
        wait_for_rejection(deadline).await;
        assert!(started.elapsed() >= Duration::from_secs(5));
    }
}
