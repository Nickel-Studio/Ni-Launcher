//! 构建通道在编译期固化，运行时环境变量不改变请求策略。
//! `NICKEL_CHANNEL` 未设置时使用开发通道。
//! 生产通道直接请求 CurseForge 官方接口，缺少本地 Key 时仍按实际 HTTP 响应处理。

const PRODUCTION: &str = "production";
const DEVELOPMENT: &str = "development";

fn resolve_channel(value: Option<&str>) -> &'static str {
    match value {
        Some(value) if value.eq_ignore_ascii_case(PRODUCTION) => PRODUCTION,
        _ => DEVELOPMENT,
    }
}

pub fn current() -> &'static str {
    resolve_channel(option_env!("NICKEL_CHANNEL"))
}

pub fn is_production() -> bool {
    current() == PRODUCTION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_is_one_of_two_known_values() {
        assert!(matches!(current(), PRODUCTION | DEVELOPMENT));
        assert_eq!(is_production(), current() == PRODUCTION);
    }

    #[test]
    fn channel_selection_defaults_to_development() {
        assert_eq!(resolve_channel(Some("PRODUCTION")), PRODUCTION);
        for value in [None, Some(""), Some(" "), Some("unknown"), Some(DEVELOPMENT)] {
            assert_eq!(resolve_channel(value), DEVELOPMENT);
        }
    }
}
