//! 无网络的提供方粗筛与 API 版本归属。路径安全由调用方独立校验。

const NEOFORGE_FIRST_DAY: &str = "2023-06-11";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryPlan {
    pub forge: bool,
    pub neoforge: bool,
    pub fabric: bool,
    pub liteloader: bool,
}

fn traditional_version(mc: &str) -> Option<(u32, u32)> {
    let mut parts = mc.strip_prefix("1.")?.split('.');
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    parts.next().is_none().then_some((minor, patch))
}

pub fn query_plan(mc: &str, release_time: Option<&str>) -> QueryPlan {
    let traditional = traditional_version(mc);
    let legacy_fabric = traditional.is_some_and(|(minor, _)| minor <= 13);
    let before_neoforge = release_time
        .and_then(manifest_day)
        .map(|day| day < NEOFORGE_FIRST_DAY)
        .unwrap_or_else(|| traditional.is_some_and(|version| version < (20, 1)));
    // 明确的 Alpha/Beta 身份不适用 Forge；未知/新格式交给支持索引确认。
    let ancient = mc.starts_with("a1.")
        || mc.starts_with("b1.")
        || mc.starts_with("rd-")
        || mc.starts_with("c0.")
        || mc.starts_with("inf-");
    QueryPlan {
        forge: !ancient,
        neoforge: !before_neoforge,
        fabric: !legacy_fabric,
        // LiteLoader 的最后一条产品线停在整个 1.13.x 之前；未知/新格式不做猜测。
        liteloader: traditional.is_some_and(|(minor, _)| minor < 13),
    }
}

fn manifest_day(value: &str) -> Option<&str> {
    let day = value.get(..10)?;
    let bytes = day.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(i, c)| i != 4 && i != 7 && !c.is_ascii_digit())
    {
        return None;
    }
    let year = day[..4].parse::<u32>().ok()?;
    let month = day[5..7].parse::<u32>().ok()?;
    let date = day[8..].parse::<u32>().ok()?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => return None,
    };
    (date > 0 && date <= days).then_some(day)
}

pub fn fabric_api_id(mc: &str) -> String {
    match mc {
        "Combat Test 7c" => "1.16_combat-3".to_string(),
        _ => mc.replace('∞', "infinite"),
    }
}

pub fn forge_api_id(mc: &str) -> String {
    mc.replace('-', "_")
}

/// NeoForge 1.20.1 的 `47.1.82` 在官方 Maven 中缺少可用安装器；
/// 目录与安装入口必须共同过滤，避免“能选却下不到”。
pub fn is_known_broken_neoforge(api_name: &str) -> bool {
    let raw = api_name.trim();
    let build = raw.strip_prefix("1.20.1-").unwrap_or(raw);
    build == "47.1.82"
}

/// Maven 全量索引无法说明每条文件类型；该判断只保留为历史回归测试。
#[cfg(test)]
pub fn forge_installer_era(mc: &str) -> bool {
    !traditional_version(mc).is_some_and(|(minor, _)| minor < 6)
}

/// 返回构建实际面向的 MC ID，不以 starts_with 将同号快照混入正式版本。
pub fn neoforge_minecraft_id(raw: &str) -> Option<String> {
    if raw.starts_with("1.20.1-") {
        return Some("1.20.1".into());
    }
    if let Some(special) = raw.strip_prefix("0.") {
        let (mc, build) = special.split_once('.')?;
        if !mc.is_empty() && build.starts_with(|c: char| c.is_ascii_digit()) {
            return Some(mc.into());
        }
        return None;
    }
    let (base, suffix) = raw.split_once('+').unwrap_or((raw, ""));
    let numeric = base.split('-').next()?;
    let parts = numeric
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let major = *parts.first()?;
    let minor = *parts.get(1)?;
    let mc = if major >= 26 && parts.len() == 4 {
        let patch = parts[2];
        if patch == 0 {
            format!("{major}.{minor}")
        } else {
            format!("{major}.{minor}.{patch}")
        }
    } else if (20..26).contains(&major) && parts.len() == 3 {
        if minor == 0 {
            format!("1.{major}")
        } else {
            format!("1.{major}.{minor}")
        }
    } else {
        return None;
    };
    if suffix.is_empty() {
        return Some(mc);
    }
    // 新命名的 snapshot/pre/rc 归属由 API 后缀给出；保留原构建本身用于安装地址。
    if ["snapshot-", "pre-", "rc-"].iter().any(|prefix| {
        suffix
            .strip_prefix(prefix)
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
    }) {
        Some(format!("{mc}-{suffix}"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_versions_do_not_request_modern_providers() {
        for mc in ["1.2.1", "1.12.2", "1.13", "1.13.1", "1.13.2"] {
            let plan = query_plan(mc, Some("2019-04-22T00:00:00+00:00"));
            assert!(plan.forge, "{mc}");
            assert!(!plan.fabric, "{mc}");
            assert!(!plan.neoforge, "{mc}");
        }
        assert!(!query_plan("1.2.1", None).neoforge);
        assert!(!query_plan("23w18a", Some("2023-05-03T10:00:00Z")).neoforge);
        assert!(query_plan("future-special", Some("2023-06-11T00:00:00Z")).neoforge);
        assert!(query_plan("future-special", None).neoforge);
        assert!(query_plan("26.1-snapshot-1", None).fabric);
        assert!(!query_plan("b1.7.3", None).forge);
        assert!(query_plan("1.12.2", None).liteloader);
        assert!(!query_plan("1.13", None).liteloader);
        assert!(!query_plan("1.13.2", None).liteloader);
        assert!(!query_plan("26.1", None).liteloader);
    }

    #[test]
    fn aliases_are_provider_specific() {
        assert_eq!(fabric_api_id("20w14∞"), "20w14infinite");
        assert_eq!(fabric_api_id("Combat Test 7c"), "1.16_combat-3");
        assert_eq!(fabric_api_id("26.1-snapshot-1"), "26.1-snapshot-1");
        assert_eq!(forge_api_id("1.7.10-pre4"), "1.7.10_pre4");
        assert!(!forge_installer_era("1.5.2"));
        assert!(forge_installer_era("1.12.2"));
    }

    #[test]
    fn neoforge_exact_game_mapping_preserves_special_releases() {
        for (build, mc) in [
            ("1.20.1-47.1.99", "1.20.1"),
            ("20.4.30-beta", "1.20.4"),
            ("21.0.1", "1.21"),
            ("26.1.0.1-beta", "26.1"),
            ("26.1.2.10", "26.1.2"),
            ("0.25w14craftmine.3-beta", "25w14craftmine"),
            ("26.1.0.0-alpha.1+snapshot-1", "26.1-snapshot-1"),
            ("26.1.2.0-alpha.1+snapshot-1", "26.1.2-snapshot-1"),
        ] {
            assert_eq!(neoforge_minecraft_id(build).as_deref(), Some(mc), "{build}");
        }
        assert_eq!(neoforge_minecraft_id("26.1.2-beta"), None);
        assert_eq!(neoforge_minecraft_id("garbage"), None);
        assert_eq!(neoforge_minecraft_id("26.1.0.0-alpha+unknown"), None);
    }
}
