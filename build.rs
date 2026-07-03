use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    configure_reruns();
    generate_embedded_web_assets();

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
    println!("cargo:rerun-if-changed=web-dist");
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

fn generate_embedded_web_assets() {
    let root = Path::new("web-dist");
    let assets = collect_web_assets(root).unwrap_or_else(|error| {
        panic!("failed to collect embedded web assets from web-dist: {error}");
    });
    for (_, path) in &assets {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let mut output = String::from("pub static EMBEDDED_WEB_ASSETS: &[EmbeddedWebAsset] = &[\n");
    for (relative, path) in assets {
        let absolute = env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path);
        output.push_str("    EmbeddedWebAsset {\n");
        output.push_str(&format!(
            "        path: {},\n",
            rust_string_literal(&relative)
        ));
        output.push_str(&format!(
            "        content_type: {},\n",
            rust_string_literal(content_type_for_path(&relative))
        ));
        output.push_str(&format!(
            "        bytes: include_bytes!({}),\n",
            rust_string_literal(&absolute.display().to_string())
        ));
        output.push_str("    },\n");
    }
    output.push_str("];\n");

    let out_dir = env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo");
    fs::write(Path::new(&out_dir).join("embedded_web_assets.rs"), output)
        .expect("write embedded web assets module");
}

fn collect_web_assets(root: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut assets = Vec::new();
    if !root.is_dir() {
        return Ok(assets);
    }
    collect_web_assets_inner(root, root, &mut assets)?;
    assets.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(assets)
}

fn collect_web_assets_inner(
    root: &Path,
    dir: &Path,
    assets: &mut Vec<(String, PathBuf)>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            collect_web_assets_inner(root, &path, assets)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let relative = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        assets.push((relative, path));
    }
    Ok(())
}

fn content_type_for_path(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|value| value.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("txt") => "text/plain; charset=utf-8",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn rust_string_literal(value: &str) -> String {
    format!("{value:?}")
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
