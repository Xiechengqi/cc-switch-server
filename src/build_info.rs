use serde::Serialize;

pub const VERSION_LINE: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("CC_SWITCH_BUILD_COMMIT_SHORT"),
    ")"
);

pub const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\ncommit id: ",
    env!("CC_SWITCH_BUILD_COMMIT"),
    "\ncommit message: ",
    env!("CC_SWITCH_BUILD_COMMIT_MESSAGE"),
    "\ncommit time: ",
    env!("CC_SWITCH_BUILD_COMMIT_TIME"),
    "\nbuild time: ",
    env!("CC_SWITCH_BUILD_TIME"),
    "\ntarget: ",
    env!("CC_SWITCH_BUILD_TARGET"),
    "\nprofile: ",
    env!("CC_SWITCH_BUILD_PROFILE"),
    "\nrustc: ",
    env!("CC_SWITCH_BUILD_RUSTC_VERSION"),
    "\ndirty: ",
    env!("CC_SWITCH_BUILD_DIRTY")
);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub version_line: &'static str,
    pub commit_id: &'static str,
    pub commit_short: &'static str,
    pub commit_message: &'static str,
    pub commit_time: &'static str,
    pub build_time: &'static str,
    pub target: &'static str,
    pub profile: &'static str,
    pub rustc_version: &'static str,
    pub dirty: bool,
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        version_line: VERSION_LINE,
        commit_id: env!("CC_SWITCH_BUILD_COMMIT"),
        commit_short: env!("CC_SWITCH_BUILD_COMMIT_SHORT"),
        commit_message: env!("CC_SWITCH_BUILD_COMMIT_MESSAGE"),
        commit_time: env!("CC_SWITCH_BUILD_COMMIT_TIME"),
        build_time: env!("CC_SWITCH_BUILD_TIME"),
        target: env!("CC_SWITCH_BUILD_TARGET"),
        profile: env!("CC_SWITCH_BUILD_PROFILE"),
        rustc_version: env!("CC_SWITCH_BUILD_RUSTC_VERSION"),
        dirty: env!("CC_SWITCH_BUILD_DIRTY").eq_ignore_ascii_case("true"),
    }
}

impl BuildInfo {
    pub fn format_human(&self) -> String {
        format!(
            "{name} {version}\ncommit id: {commit_id}\ncommit short: {commit_short}\ncommit message: {commit_message}\ncommit time: {commit_time}\nbuild time: {build_time}\ntarget: {target}\nprofile: {profile}\nrustc: {rustc_version}\ndirty: {dirty}",
            name = self.name,
            version = self.version,
            commit_id = self.commit_id,
            commit_short = self.commit_short,
            commit_message = self.commit_message,
            commit_time = self.commit_time,
            build_time = self.build_time,
            target = self.target,
            profile = self.profile,
            rustc_version = self.rustc_version,
            dirty = self.dirty,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_has_package_identity_and_build_fields() {
        let info = build_info();

        assert_eq!(info.name, "cc-switch-server");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.commit_id.is_empty());
        assert!(!info.commit_short.is_empty());
        assert!(!info.build_time.is_empty());
        assert!(!info.target.is_empty());
    }

    #[test]
    fn human_format_contains_operational_metadata() {
        let output = build_info().format_human();

        assert!(output.contains("commit id:"));
        assert!(output.contains("commit message:"));
        assert!(output.contains("build time:"));
        assert!(output.contains("rustc:"));
    }
}
