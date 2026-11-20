//! 自动内存规划：按有效资源根中的 Mod 数量与可用物理内存分段分配。
//!
//! 自动值不得超过物理内存上限；手动值超限须报错，不得静默截断用户配置。
//! 本模块只生成规划值，进程启动与运行时内存读取由调用方负责。

use std::path::Path;

pub const MIN_MEMORY_MB: u64 = 512;
const MB_PER_GIB: f64 = 1024.0;
const DECIGIB_STEP: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionKind {
    Regular,
    OptiFine,
    Moddable,
}

impl VersionKind {
    fn targets(self, mod_count: u64) -> (f64, f64, f64, f64) {
        let mods = mod_count as f64;
        match self {
            Self::Moddable => (
                0.5 + mods / 150.0,
                1.5 + mods / 90.0,
                2.7 + mods / 50.0,
                4.5 + mods / 25.0,
            ),
            Self::OptiFine => (0.5, 1.5, 3.0, 5.0),
            Self::Regular => (0.5, 1.5, 2.5, 4.0),
        }
    }
}

/// 从版本名、命名实例加载器与版本 JSON 特征中判断内存档位。
///
/// 加载器优先于 OptiFine：Forge + OptiFine 仍按可装 Mod 的实例计算。
pub fn classify_version(version_id: &str, loader: Option<&str>, metadata_hint: &str) -> VersionKind {
    let loader = loader.unwrap_or_default().to_ascii_lowercase();
    let all = format!(
        "{} {} {}",
        version_id.to_ascii_lowercase(),
        loader,
        metadata_hint.to_ascii_lowercase()
    );
    if ["forge", "neoforge", "fabric", "quilt", "liteloader"]
        .iter()
        .any(|needle| all.contains(needle))
    {
        VersionKind::Moddable
    } else if all.contains("optifine") {
        VersionKind::OptiFine
    } else {
        VersionKind::Regular
    }
}

/// 读取版本描述作为加载器 / OptiFine 的弱特征；失败时返回空串。
///
/// 这里不参与启动正确性，只用于自动内存档位。真正的加载器安装仍由核心
/// 版本元数据决定；文件缺失或损坏时回退普通版本。
pub fn version_metadata_hint(root: &Path, version_id: &str) -> String {
    let directory = root.join("versions").join(version_id);
    let direct = directory.join(format!("{version_id}.json"));
    let json = std::fs::read_to_string(direct)
        .ok()
        .or_else(|| {
            std::fs::read_dir(&directory)
                .ok()?
                .flatten()
                .map(|entry| entry.path())
                .find(|path| {
                    path.extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
                })
                .and_then(|path| std::fs::read_to_string(path).ok())
        })
        .unwrap_or_default();
    if json.len() > 1024 * 1024 {
        return String::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return String::new();
    };
    let mut hint = String::new();
    if let Some(id) = value.get("id").and_then(serde_json::Value::as_str) {
        hint.push_str(id);
    }
    if let Some(main_class) = value.get("mainClass").and_then(serde_json::Value::as_str) {
        hint.push(' ');
        hint.push_str(main_class);
    }
    if let Some(inherits) = value
        .get("inheritsFrom")
        .and_then(serde_json::Value::as_str)
    {
        hint.push(' ');
        hint.push_str(inherits);
    }
    if let Some(libraries) = value.get("libraries").and_then(serde_json::Value::as_array) {
        for library in libraries.iter().take(512) {
            if let Some(name) = library.get("name").and_then(serde_json::Value::as_str) {
                hint.push(' ');
                hint.push_str(name);
            }
        }
    }
    hint
}

/// 只统计普通 jar / zip / litemod 文件，禁用后缀与子目录不计入内存规划。
pub fn count_mods(mods_directory: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(mods_directory) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    matches!(
                        extension.to_ascii_lowercase().as_str(),
                        "jar" | "zip" | "litemod"
                    )
                })
        })
        .count() as u64
}

/// 由实例身份与有效资源根解析 LiteLoader 等加载器目录后再统计。
pub fn count_mods_for_instance(
    root: &Path,
    version_id: &str,
    resource_root: &Path,
) -> Result<u64, String> {
    let mods = crate::resources::instance_paths::for_instance(root, version_id, resource_root)?;
    Ok(count_mods(&mods))
}

/// 分段分配并限制物理上限；可用内存未知时回退 4 GiB。
pub fn automatic_memory_mb(
    available_mb: Option<u64>,
    total_mb: Option<u64>,
    mod_count: u64,
    kind: VersionKind,
) -> u64 {
    let Some(available_mb) = available_mb else {
        return 4096;
    };
    let total_mb = total_mb.unwrap_or(available_mb.max(4096));
    let mut available = (available_mb.min(total_mb) as f64 / MB_PER_GIB * 10.0).round() / 10.0;
    let (minimum, target1, target2, target3) = kind.targets(mod_count);
    let mut give = 0.0;

    // 阶段一保留 100%，之后分别只取新增区间的 70% / 40% / 15%。
    let mut delta = target1;
    give += available.min(delta);
    available -= delta;
    if available >= 0.1 {
        delta = target2 - target1;
        give += (available * 0.7).min(delta);
        available -= delta / 0.7;
    }
    if available >= 0.1 {
        delta = target3 - target2;
        give += (available * 0.4).min(delta);
        available -= delta / 0.4;
    }
    if available >= 0.1 {
        delta = target3;
        give += (available * 0.15).min(delta);
    }

    let rounded = ((give.max(minimum) * DECIGIB_STEP).round() / DECIGIB_STEP * MB_PER_GIB)
        .round() as u64;
    rounded.clamp(MIN_MEMORY_MB, total_mb.max(MIN_MEMORY_MB))
}

/// 手动上限必须能放进本机物理内存；保留用户保存值，超限时明确报错而非截断。
pub fn manual_memory_mb(saved_mb: u64, total_mb: Option<u64>) -> Result<u64, String> {
    if saved_mb < MIN_MEMORY_MB {
        return Err(format!(
            "手动内存 {saved_mb} MB 低于最低 {MIN_MEMORY_MB} MB，请调高后重试"
        ));
    }
    if let Some(total_mb) = total_mb.filter(|total| *total > 0) {
        if saved_mb > total_mb {
            return Err(format!(
                "手动内存 {saved_mb} MB 超过本机物理内存 {total_mb} MB，请降低分配后重试"
            ));
        }
    }
    Ok(saved_mb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_memory_matrix_tracks_mods_and_available_memory() {
        for (available, total, mods, kind, expected) in [
            (4096, 8192, 0, VersionKind::Regular, 2970),
            (8192, 16384, 0, VersionKind::Regular, 4301),
            (16384, 32768, 200, VersionKind::Moddable, 10138),
            (32768, 32768, 50, VersionKind::OptiFine, 8704),
            (4096, 4096, 200, VersionKind::Moddable, 3994),
        ] {
            assert_eq!(
                automatic_memory_mb(Some(available), Some(total), mods, kind),
                expected,
                "available={available} total={total} mods={mods} kind={kind:?}"
            );
        }
    }

    #[test]
    fn automatic_memory_full_axis_matrix_stays_bounded_and_monotonic() {
        for kind in [
            VersionKind::Regular,
            VersionKind::OptiFine,
            VersionKind::Moddable,
        ] {
            for available in [4096, 8192, 16384, 32768] {
                let none = automatic_memory_mb(Some(available), Some(32768), 0, kind);
                let fifty = automatic_memory_mb(Some(available), Some(32768), 50, kind);
                let two_hundred =
                    automatic_memory_mb(Some(available), Some(32768), 200, kind);
                assert!(none >= MIN_MEMORY_MB && none <= available);
                assert!(fifty >= none, "mods should not reduce requested heap");
                assert!(two_hundred >= fifty, "mods should not reduce requested heap");
                let larger = automatic_memory_mb(
                    Some((available * 2).min(32768)),
                    Some(32768),
                    200,
                    kind,
                );
                assert!(larger >= two_hundred, "more available memory should not reduce heap");
            }
        }
    }

    #[test]
    fn manual_memory_matrix_tracks_physical_capacity() {
        for total in [4096, 8192, 16384, 32768] {
            assert_eq!(manual_memory_mb(2048, Some(total)).unwrap(), 2048);
            assert!(manual_memory_mb(total, Some(total)).is_ok());
            assert!(manual_memory_mb(total + 1, Some(total)).is_err());
        }
    }

    #[test]
    fn automatic_memory_never_exceeds_physical_or_drops_below_minimum() {
        for (available, total) in [(50, 50), (300, 1024), (1024, 512), (0, 0)] {
            let value = automatic_memory_mb(
                Some(available),
                Some(total),
                10_000,
                VersionKind::Moddable,
            );
            assert!(value >= MIN_MEMORY_MB);
            if total > 0 {
                assert!(value <= total.max(MIN_MEMORY_MB));
            }
        }
        assert_eq!(
            automatic_memory_mb(None, None, 0, VersionKind::Regular),
            4096
        );
    }

    #[test]
    fn manual_memory_is_not_silently_clamped() {
        assert_eq!(manual_memory_mb(8192, Some(16384)).unwrap(), 8192);
        assert!(manual_memory_mb(16384, Some(8192))
            .unwrap_err()
            .contains("超过本机物理内存"));
        assert_eq!(manual_memory_mb(16384, None).unwrap(), 16384);
        assert!(manual_memory_mb(128, Some(16384)).is_err());
    }

    #[test]
    fn version_kind_prefers_loader_then_optifine() {
        assert_eq!(
            classify_version("1.20.1", Some("Forge"), "OptiFine"),
            VersionKind::Moddable
        );
        assert_eq!(
            classify_version("1.20.1-OptiFine_HD_U_I6", None, ""),
            VersionKind::OptiFine
        );
        assert_eq!(
            classify_version("1.21.4", Some("Fabric"), ""),
            VersionKind::Moddable
        );
        assert_eq!(
            classify_version("1.21.4", None, "net.minecraft.client.main.Main"),
            VersionKind::Regular
        );
    }

    #[test]
    fn mod_count_ignores_disabled_and_unrelated_files() {
        let root = std::env::temp_dir().join(format!("nickel-memory-{}", crate::store::random_token()));
        let mods = root.join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        for name in ["a.jar", "b.ZIP", "c.litemod", "d.jar.disabled", "readme.txt"] {
            std::fs::write(mods.join(name), b"x").unwrap();
        }
        std::fs::create_dir_all(mods.join("nested.jar")).unwrap();
        assert_eq!(count_mods(&mods), 3);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mod_count_accepts_an_explicit_loader_directory() {
        let root = std::env::temp_dir().join(format!(
            "nickel-memory-lite-{}",
            crate::store::random_token()
        ));
        let mods = root.join("mods/1.12.2");
        std::fs::create_dir_all(&mods).unwrap();
        for name in ["a.jar", "b.litemod", "disabled.jar.disabled"] {
            std::fs::write(mods.join(name), b"x").unwrap();
        }
        assert_eq!(count_mods(&mods), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
