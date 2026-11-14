//! Java 选择纯策略：版本/加载器要求、自动候选排序与手动 Java 校验。
//!
//! 列表探测顺序只用于展示，不能把第一项宣称为默认选择。真正启动前必须经过
//! `select`，并在手动路径场景通过 `validate_manual` 检查位数与兼容区间。

use serde::{Deserialize, Serialize};

const JAVA_MIN_OLD: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaRequirement {
    pub min_major: u32,
    pub max_major: Option<u32>,
    pub preferred_major: u32,
    pub reason: String,
}

impl JavaRequirement {
    fn exactly(major: u32, reason: impl Into<String>) -> Self {
        Self {
            min_major: major,
            max_major: Some(major),
            preferred_major: major,
            reason: reason.into(),
        }
    }

    fn at_least(major: u32, reason: impl Into<String>) -> Self {
        Self {
            min_major: major,
            max_major: None,
            preferred_major: major,
            reason: reason.into(),
        }
    }

    fn range(
        min_major: u32,
        max_major: u32,
        preferred_major: u32,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            min_major,
            max_major: Some(max_major),
            preferred_major,
            reason: reason.into(),
        }
    }

    fn with_max(mut self, max_major: u32, reason: impl Into<String>) -> Self {
        self.max_major = Some(max_major);
        if self.preferred_major > max_major {
            self.preferred_major = max_major;
        }
        self.reason = reason.into();
        self
    }

    pub fn accepts(&self, major: u32) -> bool {
        major >= self.min_major && self.max_major.is_none_or(|max| major <= max)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaCandidate {
    pub path: String,
    pub version: String,
    pub vendor: String,
    pub major: u32,
    pub bits: u8,
    pub executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaSelection {
    pub path: Option<String>,
    /// `auto` / `manual` / `unavailable`
    pub source: String,
    pub version: String,
    pub vendor: String,
    pub major: Option<u32>,
    pub bits: Option<u8>,
    pub compatible: bool,
    pub needs_download: bool,
    pub reason: String,
    pub requirement: JavaRequirement,
}

fn traditional_parts(mc: &str) -> Option<(u32, u32, u32)> {
    let mut parts = mc.strip_prefix("1.")?.split('.');
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((1, minor, patch))
}

fn modern_parts(mc: &str) -> Option<(u32, u32, u32)> {
    if mc.starts_with(|ch: char| !ch.is_ascii_digit()) {
        return None;
    }
    let base = mc.split(['-', '+']).next()?;
    let mut parts = base.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    if major < 24 {
        return None;
    }
    let minor = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    Some((major, minor, patch))
}

fn at_least(actual: (u32, u32, u32), wanted: (u32, u32, u32)) -> bool {
    actual >= wanted
}

fn numeric_major_version(mc: &str) -> Option<u32> {
    mc.split('.').next()?.parse::<u32>().ok()
}

fn base_requirement(mc: &str, release_time: Option<&str>) -> JavaRequirement {
    if let Some(parts) = traditional_parts(mc) {
        let (_, minor, patch) = parts;
        if at_least(parts, (1, 20, 5)) {
            return JavaRequirement::at_least(21, "Minecraft 1.20.5 起需要 Java 21");
        }
        if at_least(parts, (1, 18, 0)) {
            return JavaRequirement::at_least(17, "Minecraft 1.18 起需要 Java 17");
        }
        if at_least(parts, (1, 17, 0)) {
            return JavaRequirement::at_least(16, "Minecraft 1.17 起需要 Java 16");
        }
        if minor < 6 || (minor == 5 && patch <= 2) {
            return JavaRequirement::range(8, 8, 8, "Minecraft 1.5.2 及更早版本使用 Java 8");
        }
        return JavaRequirement::at_least(8, "Minecraft 1.12 及更早版本至少需要 Java 8")
            .with_max(if minor <= 16 { 17 } else { 21 }, "旧版游戏使用 Java 8–17");
    }
    if let Some(parts) = modern_parts(mc) {
        if parts.0 >= 26 {
            return JavaRequirement::at_least(21, "现代 Minecraft 版本至少需要 Java 21");
        }
    }
    if numeric_major_version(mc).is_some_and(|major| major >= 26) {
        return JavaRequirement::at_least(21, "现代 Minecraft 版本至少需要 Java 21");
    }
    if release_time
        .and_then(|value| value.get(..10))
        .is_some_and(|day| day >= "2024-04-23")
    {
        return JavaRequirement::at_least(21, "2024 年 4 月后的版本至少需要 Java 21");
    }
    if release_time
        .and_then(|value| value.get(..10))
        .is_some_and(|day| day >= "2021-11-30")
    {
        return JavaRequirement::at_least(17, "该快照需要 Java 17");
    }
    JavaRequirement::at_least(8, "未识别的版本按 Java 8 以上处理")
}

fn forge_requirement(mc: &str, loader_version: Option<&str>) -> Option<JavaRequirement> {
    let parts = traditional_parts(mc)?;
    let (_, minor, patch) = parts;
    if minor == 6 && (1..=4).contains(&patch) || minor == 7 && patch <= 2 {
        return Some(JavaRequirement::exactly(7, "Forge 1.6.1–1.7.2 必须使用 Java 7"));
    }
    if minor <= 12 {
        return Some(JavaRequirement::exactly(8, "Forge 1.12 及更早版本必须使用 Java 8"));
    }
    if minor <= 14 {
        return Some(JavaRequirement::range(
            8,
            10,
            8,
            "Forge 1.13–1.14 使用 Java 8–10",
        ));
    }
    if minor == 15 {
        return Some(JavaRequirement::range(
            8,
            15,
            8,
            "Forge 1.15 使用 Java 8–15",
        ));
    }
    if minor == 16 && patch <= 5 {
        let build = loader_version.unwrap_or_default();
        if build.starts_with("34.") || build.starts_with("35.") || build.starts_with("36.2.25") {
            return Some(JavaRequirement::range(
                8,
                8,
                JAVA_MIN_OLD,
                "Forge 34.0.0–36.2.25 使用 Java 8",
            ));
        }
        if build.starts_with("36.") {
            return Some(JavaRequirement::range(
                8,
                23,
                17,
                "Forge 36.2.26+ 使用 Java 8–23",
            ));
        }
    }
    if minor == 17 && patch == 1 {
        let build = loader_version.unwrap_or_default();
        if build.starts_with("37.0.") {
            return Some(JavaRequirement::range(
                8,
                16,
                16,
                "Forge 37.0.x 使用 Java 8–16",
            ));
        }
    }
    None
}

fn optifine_requirement(mc: &str, requirement: JavaRequirement) -> JavaRequirement {
    let Some((_, minor, _)) = traditional_parts(mc) else {
        return requirement;
    };
    if minor <= 6 {
        JavaRequirement::range(8, 8, 8, "旧版 OptiFine 使用 Java 8")
    } else if (8..=11).contains(&minor) {
        JavaRequirement::exactly(8, "OptiFine 1.8–1.11 必须使用 Java 8")
    } else if minor == 12 {
        JavaRequirement::range(8, 8, 8, "OptiFine 1.12 使用 Java 8")
    } else {
        requirement
    }
}

/// 计算启动版本的要求；加载器约束与游戏本体的要求取交集。
pub fn requirement_for(
    mc: &str,
    loader: Option<&str>,
    loader_version: Option<&str>,
    release_time: Option<&str>,
) -> JavaRequirement {
    let mut requirement = base_requirement(mc, release_time);
    match loader.unwrap_or_default().trim().to_ascii_lowercase().as_str() {
        "forge" => {
            if let Some(forge) = forge_requirement(mc, loader_version) {
                requirement = forge;
            }
        }
        "optifine" => {
            requirement = optifine_requirement(mc, requirement);
        }
        "liteloader" => {
            if traditional_parts(mc).is_some_and(|(_, minor, _)| minor <= 12) {
                requirement =
                    JavaRequirement::exactly(8, "LiteLoader 支持范围使用 Java 8");
            }
        }
        _ => {}
    }
    requirement
}

fn invalid_reason(requirement: &JavaRequirement, candidate: &JavaCandidate) -> Option<String> {
    if !candidate.executable {
        return Some("Java 可执行文件不可用".into());
    }
    if candidate.major == 0 {
        return Some("无法读取 Java 主版本".into());
    }
    if candidate.bits < 64 {
        return Some("Minecraft 需要 64 位 Java".into());
    }
    if !requirement.accepts(candidate.major) {
        let range = requirement
            .max_major
            .map(|max| format!("Java {}-{}", requirement.min_major, max))
            .unwrap_or_else(|| format!("Java {} 以上", requirement.min_major));
        return Some(format!(
            "需要 {range}，当前为 Java {}",
            candidate.major
        ));
    }
    None
}

fn selection(
    requirement: JavaRequirement,
    candidate: Option<&JavaCandidate>,
    source: &str,
    needs_download: bool,
    reason: String,
) -> JavaSelection {
    JavaSelection {
        path: candidate.map(|candidate| candidate.path.clone()),
        source: source.into(),
        version: candidate.map(|candidate| candidate.version.clone()).unwrap_or_default(),
        vendor: candidate.map(|candidate| candidate.vendor.clone()).unwrap_or_default(),
        major: candidate.map(|candidate| candidate.major),
        bits: candidate.map(|candidate| candidate.bits),
        compatible: candidate.is_some(),
        needs_download,
        reason,
        requirement,
    }
}

/// 自动模式选择满足要求的 Java；绝不把输入列表顺序当作默认决策。
pub fn select(requirement: JavaRequirement, candidates: &[JavaCandidate]) -> JavaSelection {
    let mut compatible: Vec<&JavaCandidate> = candidates
        .iter()
        .filter(|candidate| invalid_reason(&requirement, candidate).is_none())
        .collect();
    compatible.sort_by(|a, b| {
        let a_score = (
            a.major == requirement.preferred_major,
            u32::MAX - a.major,
            a.path.to_ascii_lowercase(),
        );
        let b_score = (
            b.major == requirement.preferred_major,
            u32::MAX - b.major,
            b.path.to_ascii_lowercase(),
        );
        b_score.cmp(&a_score)
    });
    match compatible.first() {
        Some(candidate) => selection(
            requirement,
            Some(candidate),
            "auto",
            false,
            format!("自动选择 Java {}", candidate.major),
        ),
        None => {
            let reason = format!(
                "未找到满足要求的 Java，需要 Java {} 以上",
                requirement.min_major
            );
            selection(requirement, None, "unavailable", true, reason)
        }
    }
}

/// 手动模式必须显式校验兼容性；不因用户选择而跳过版本和位数检查。
pub fn validate_manual(
    requirement: &JavaRequirement,
    candidate: &JavaCandidate,
) -> Result<JavaSelection, String> {
    if let Some(reason) = invalid_reason(requirement, candidate) {
        return Err(format!("所选 Java 不兼容：{reason}"));
    }
    Ok(selection(
        requirement.clone(),
        Some(candidate),
        "manual",
        false,
        format!("手动选择 Java {}", candidate.major),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn java(path: &str, major: u32, bits: u8) -> JavaCandidate {
        JavaCandidate {
            path: path.into(),
            version: if major == 8 {
                format!("1.8.0_{major}")
            } else {
                format!("{major}.0.1")
            },
            vendor: "OpenJDK".into(),
            major,
            bits,
            executable: true,
        }
    }

    #[test]
    fn requirements_cover_old_boundaries_and_modern_releases() {
        assert_eq!(requirement_for("1.5.2", None, None, None).min_major, 8);
        assert_eq!(
            requirement_for("1.5.2", None, None, None).max_major,
            Some(8)
        );
        assert_eq!(requirement_for("1.12.2", None, None, None).min_major, 8);
        assert_eq!(requirement_for("1.17", None, None, None).min_major, 16);
        assert_eq!(requirement_for("1.18", None, None, None).min_major, 17);
        assert_eq!(requirement_for("1.20.4", None, None, None).min_major, 17);
        assert_eq!(requirement_for("1.20.5", None, None, None).min_major, 21);
        assert_eq!(requirement_for("26.1", None, None, None).min_major, 21);
        assert_eq!(
            requirement_for("24w14a", None, None, Some("2024-04-03T00:00:00Z")).min_major,
            17
        );
    }

    #[test]
    fn loader_constraints_are_intersected_with_game_constraints() {
        let forge = requirement_for("1.7.2", Some("forge"), Some("10.12.2.1121"), None);
        assert_eq!(forge.min_major, 7);
        assert_eq!(forge.max_major, Some(7));
        let optifine = requirement_for("1.10.2", Some("optifine"), Some("HD_U_E7"), None);
        assert_eq!(optifine.min_major, 8);
        assert_eq!(optifine.max_major, Some(8));
        let forge_116 = requirement_for("1.16.5", Some("forge"), Some("36.2.26"), None);
        assert_eq!(forge_116.max_major, Some(23));
        let lite = requirement_for("1.12.2", Some("liteloader"), Some("1.12.2"), None);
        assert_eq!(lite.max_major, Some(8));
    }

    #[test]
    fn selection_ignores_list_order_and_ranking_prefers_exact_major() {
        let requirement = requirement_for("1.20.5", None, None, None);
        let candidates = [
            java("C:/java8/bin/java.exe", 8, 64),
            java("C:/java21/bin/java.exe", 21, 64),
            java("C:/java25/bin/java.exe", 25, 64),
        ];
        let selected = select(requirement.clone(), &candidates);
        assert_eq!(selected.major, Some(21));
        assert_eq!(selected.source, "auto");
        assert!(!selected.needs_download);

        let unavailable = select(requirement, &candidates[..1]);
        assert_eq!(unavailable.path, None);
        assert!(unavailable.needs_download);
        assert_eq!(unavailable.source, "unavailable");
    }

    #[test]
    fn manual_java_checks_executable_bits_major_and_range() {
        let requirement = requirement_for("1.20.5", None, None, None);
        let valid = java("C:/java21/bin/java.exe", 21, 64);
        assert_eq!(
            validate_manual(&requirement, &valid).unwrap().source,
            "manual"
        );

        let mut invalid = valid.clone();
        invalid.bits = 32;
        assert!(validate_manual(&requirement, &invalid).is_err());

        invalid = valid.clone();
        invalid.executable = false;
        assert!(validate_manual(&requirement, &invalid).is_err());

        invalid = valid;
        invalid.major = 17;
        assert!(validate_manual(&requirement, &invalid).is_err());

        let old = requirement_for("1.7.2", Some("forge"), Some("10.12.2.1121"), None);
        assert!(validate_manual(&old, &java("C:/java8/bin/java.exe", 8, 64)).is_err());
        assert!(validate_manual(&old, &java("C:/java7/bin/java.exe", 7, 64)).is_ok());
    }
}
