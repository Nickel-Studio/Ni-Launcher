//! 「今日人品」：按本地日期生成 1~100 的稳定值。
//!
//! 日期键由前端按本地时区生成，避免跨时区计算出不同日期。
//! `yyyyMMdd` 的 FNV-1a 32 位哈希按无符号回绕计算，结果须跨平台一致。

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JrrpReport {
    /// 1~100，同一天内恒定。
    pub value: u32,
    /// 与数值分档对应的评语。
    pub comment: &'static str,
}

/// 只校验 8 位数字与月、日范围；本地日期由调用方生成。
fn valid_date_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    if bytes.len() != 8 || !bytes.iter().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let month = (bytes[4] - b'0') * 10 + (bytes[5] - b'0');
    let day = (bytes[6] - b'0') * 10 + (bytes[7] - b'0');
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

/// 同一日期键必须得到相同结果，整数溢出按 32 位回绕处理。
pub fn value_for_date_key(key: &str) -> u32 {
    let mut hash: u32 = 2_166_136_261;
    for byte in key.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash % 100 + 1
}

pub fn comment_for(value: u32) -> &'static str {
    match value {
        100 => "满分！你就是被选中的孩子！",
        90..=99 => "大吉！今天适合开箱出货！",
        75..=89 => "吉！运气不错！",
        55..=74 => "中吉，平平淡淡才是真。",
        35..=54 => "小凶，建议小心行事。",
        15..=34 => "凶！今日不宜冒险。",
        _ => "大凶！建议躺平休息……",
    }
}

/// 以本地日期键 `yyyyMMdd` 计算今日人品；格式不合法时返回可执行提示。
#[tauri::command]
pub fn jrrp_compute(date_key: String) -> Result<JrrpReport, String> {
    if !valid_date_key(&date_key) {
        return Err("日期格式必须为 yyyyMMdd".into());
    }
    let value = value_for_date_key(&date_key);
    Ok(JrrpReport {
        value,
        comment: comment_for(value),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_date_always_returns_the_same_value() {
        assert_eq!(value_for_date_key("20260101"), value_for_date_key("20260101"));
        assert_ne!(value_for_date_key("20260101"), value_for_date_key("20260102"));
    }

    #[test]
    fn value_matches_fnv1a_reference() {
        assert_eq!(value_for_date_key("20261114"), 61);
        assert_eq!(value_for_date_key("20260101"), 20);
        assert_eq!(value_for_date_key("20241231"), 47);
        assert_eq!(value_for_date_key("20240229"), 7);
        assert_eq!(value_for_date_key("20200101"), 54);
    }

    #[test]
    fn every_value_stays_in_range() {
        for year in [2024, 2025, 2026] {
            for month in 1..=12 {
                for day in 1..=28 {
                    let key = format!("{year}{month:02}{day:02}");
                    let value = value_for_date_key(&key);
                    assert!((1..=100).contains(&value), "{key} -> {value}");
                }
            }
        }
    }

    #[test]
    fn comments_follow_score_thresholds() {
        assert_eq!(comment_for(100), "满分！你就是被选中的孩子！");
        assert_eq!(comment_for(90), "大吉！今天适合开箱出货！");
        assert_eq!(comment_for(89), "吉！运气不错！");
        assert_eq!(comment_for(75), "吉！运气不错！");
        assert_eq!(comment_for(74), "中吉，平平淡淡才是真。");
        assert_eq!(comment_for(55), "中吉，平平淡淡才是真。");
        assert_eq!(comment_for(54), "小凶，建议小心行事。");
        assert_eq!(comment_for(35), "小凶，建议小心行事。");
        assert_eq!(comment_for(34), "凶！今日不宜冒险。");
        assert_eq!(comment_for(15), "凶！今日不宜冒险。");
        assert_eq!(comment_for(14), "大凶！建议躺平休息……");
        assert_eq!(comment_for(0), "大凶！建议躺平休息……");
    }

    #[test]
    fn command_rejects_malformed_dates() {
        for key in ["", "2026-1-1", "2026013", "202601011", "20261301", "20260132", "abcdefgh"] {
            assert!(jrrp_compute(key.into()).is_err(), "{key}");
        }
        let report = jrrp_compute("20261114".into()).unwrap();
        assert_eq!(report.value, 61);
        assert_eq!(report.comment, "中吉，平平淡淡才是真。");
    }
}
