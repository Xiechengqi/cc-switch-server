use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    configure_reruns();

    let commit = env_or_git("CC_SWITCH_BUILD_COMMIT", &["rev-parse", "HEAD"], "unknown");
    let commit_short = env_or_git(
        "CC_SWITCH_BUILD_COMMIT_SHORT",
        &["rev-parse", "--short=12", "HEAD"],
        "unknown",
    );
    let commit_message = env_or_git(
        "CC_SWITCH_BUILD_COMMIT_MESSAGE",
        &["log", "-1", "--format=%s"],
        "unknown",
    );
    let commit_time = env_or_git(
        "CC_SWITCH_BUILD_COMMIT_TIME",
        &["log", "-1", "--format=%cI"],
        "unknown",
    );
    let build_time = env::var("CC_SWITCH_BUILD_TIME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(current_build_time);
    let target = env::var("CC_SWITCH_BUILD_TARGET")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| env::var("TARGET").ok())
        .unwrap_or_else(|| "unknown".to_string());
    let profile = env::var("CC_SWITCH_BUILD_PROFILE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| env::var("PROFILE").ok())
        .unwrap_or_else(|| "unknown".to_string());
    let rustc_version = env_or_command(
        "CC_SWITCH_BUILD_RUSTC_VERSION",
        "rustc",
        &["--version"],
        "unknown",
    );
    let dirty = env::var("CC_SWITCH_BUILD_DIRTY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| git_dirty().to_string());

    set_build_env("CC_SWITCH_BUILD_COMMIT", &commit);
    set_build_env("CC_SWITCH_BUILD_COMMIT_SHORT", &commit_short);
    set_build_env("CC_SWITCH_BUILD_COMMIT_MESSAGE", &commit_message);
    set_build_env("CC_SWITCH_BUILD_COMMIT_TIME", &commit_time);
    set_build_env("CC_SWITCH_BUILD_TIME", &build_time);
    set_build_env("CC_SWITCH_BUILD_TARGET", &target);
    set_build_env("CC_SWITCH_BUILD_PROFILE", &profile);
    set_build_env("CC_SWITCH_BUILD_RUSTC_VERSION", &rustc_version);
    set_build_env("CC_SWITCH_BUILD_DIRTY", &dirty);
}

fn configure_reruns() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-env-changed=CC_SWITCH_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=CC_SWITCH_BUILD_COMMIT_SHORT");
    println!("cargo:rerun-if-env-changed=CC_SWITCH_BUILD_COMMIT_MESSAGE");
    println!("cargo:rerun-if-env-changed=CC_SWITCH_BUILD_COMMIT_TIME");
    println!("cargo:rerun-if-env-changed=CC_SWITCH_BUILD_TIME");
    println!("cargo:rerun-if-env-changed=CC_SWITCH_BUILD_TARGET");
    println!("cargo:rerun-if-env-changed=CC_SWITCH_BUILD_PROFILE");
    println!("cargo:rerun-if-env-changed=CC_SWITCH_BUILD_RUSTC_VERSION");
    println!("cargo:rerun-if-env-changed=CC_SWITCH_BUILD_DIRTY");

    if let Some(head_ref) = git_head_ref() {
        println!("cargo:rerun-if-changed={head_ref}");
    }
}

fn git_head_ref() -> Option<String> {
    let head = fs::read_to_string(".git/HEAD").ok()?;
    let relative = head.trim().strip_prefix("ref: ")?;
    let path = Path::new(".git").join(relative);
    Some(path.display().to_string())
}

fn env_or_git(name: &str, args: &[&str], fallback: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| command_output("git", args))
        .unwrap_or_else(|| fallback.to_string())
}

fn env_or_command(name: &str, command: &str, args: &[&str], fallback: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| command_output(command, args))
        .unwrap_or_else(|| fallback.to_string())
}

fn command_output(command: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(command).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn git_dirty() -> bool {
    command_output(
        "git",
        &["status", "--porcelain", "--untracked-files=normal"],
    )
    .is_some()
}

fn current_build_time() -> String {
    command_output("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]).unwrap_or_else(|| {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        format!("unix:{seconds}")
    })
}

fn set_build_env(name: &str, value: &str) {
    println!("cargo:rustc-env={name}={}", sanitize(value));
}

fn sanitize(value: &str) -> String {
    value.replace(['\r', '\n'], " ").trim().to_string()
}
