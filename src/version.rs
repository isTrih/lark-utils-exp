use std::sync::OnceLock;

/// Cargo 中声明的基础语义化版本号，是项目版本的唯一来源。
pub const BASE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Docker 构建时注入的版本追踪信息。本地直接编译时使用可识别的默认值。
pub const GIT_COMMIT: &str = match option_env!("LARK_EXP_GIT_COMMIT") {
    Some(value) => value,
    None => "unknown",
};
pub const BUILD_TIME: &str = match option_env!("LARK_EXP_BUILD_TIME") {
    Some(value) => value,
    None => "unknown",
};
const BUILD_METADATA: Option<&str> = option_env!("LARK_EXP_BUILD_METADATA");

static VERSION: OnceLock<String> = OnceLock::new();

/// 返回当前二进制的完整 SemVer。
///
/// Docker 发布版会包含构建时间和 Git 提交，例如：
/// `0.2.0+build.20260806T010203Z.git.fa2eb7ca7c9e`。
pub fn version() -> &'static str {
    VERSION
        .get_or_init(|| compose_version(BASE_VERSION, BUILD_METADATA))
        .as_str()
}

fn compose_version(base_version: &str, build_metadata: Option<&str>) -> String {
    match build_metadata
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(metadata) => format!("{base_version}+{metadata}"),
        None => base_version.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_version_appends_semver_build_metadata() {
        assert_eq!(
            compose_version("1.2.3", Some("build.20260806T010203Z.git.fa2eb7ca7c9e")),
            "1.2.3+build.20260806T010203Z.git.fa2eb7ca7c9e"
        );
    }

    #[test]
    fn local_version_uses_cargo_version_without_empty_metadata() {
        assert_eq!(compose_version("1.2.3", None), "1.2.3");
        assert_eq!(compose_version("1.2.3", Some("  ")), "1.2.3");
    }
}
