//! 构建通道在编译期固化，运行时环境变量不改变请求策略。
//! `NICKEL_CHANNEL` 优先于兼容别名 `NOWHERE_CHANNEL`，未设置时使用开发通道。
//! 生产通道直接请求 CurseForge 官方接口，缺少本地 Key 时仍按实际 HTTP 响应处理。

const PRODUCTION: &str = "production";
const DEVELOPMENT: &str = "development";

fn resolve_channel(primary: Option<&str>, legacy: Option<&str>) -> &'static str {
    match primary.filter(|value| !value.trim().is_empty()).or(legacy) {
        Some(value) if value.eq_ignore_ascii_case(PRODUCTION) => PRODUCTION,
        _ => DEVELOPMENT,
    }
}

pub fn current() -> &'static str {
    resolve_channel(
        option_env!("NICKEL_CHANNEL"),
        option_env!("NOWHERE_CHANNEL"),
    )
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
    fn primary_channel_overrides_legacy_without_runtime_environment_changes() {
        assert_eq!(
            resolve_channel(Some(DEVELOPMENT), Some(PRODUCTION)),
            DEVELOPMENT
        );
        assert_eq!(
            resolve_channel(Some("PRODUCTION"), Some(DEVELOPMENT)),
            PRODUCTION
        );
        assert_eq!(
            resolve_channel(Some("unknown"), Some(PRODUCTION)),
            DEVELOPMENT
        );
        assert_eq!(resolve_channel(None, Some("PRODUCTION")), PRODUCTION);
        assert_eq!(resolve_channel(Some(" "), Some(PRODUCTION)), PRODUCTION);
        assert_eq!(resolve_channel(None, None), DEVELOPMENT);
    }
}
