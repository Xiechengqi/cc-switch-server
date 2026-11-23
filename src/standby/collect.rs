use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::Configuration;
use crate::host_health_protocol::{Reading, Snapshot};

fn read_number(path: &Path) -> Option<f64> {
    let value = fs::read_to_string(path).ok()?;
    let parsed: f64 = value.trim().parse().ok()?;
    (parsed.is_finite() && parsed >= 0.0 && parsed < (1u64 << 60) as f64).then_some(parsed)
}

fn memory_info(field: &str) -> Option<f64> {
    let content = fs::read_to_string("/proc/meminfo").ok()?;
    content.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name == field)
            .then(|| {
                value
                    .split_whitespace()
                    .next()?
                    .parse::<f64>()
                    .ok()
                    .map(|n| n * 1024.0)
            })
            .flatten()
    })
}

fn cpus(value: &str) -> Option<f64> {
    if value.trim().is_empty() {
        return None;
    }
    let mut total = 0u64;
    for range in value.trim().split(',') {
        let (start, end) = range.split_once('-').unwrap_or((range, range));
        let start: u64 = start.parse().ok()?;
        let end: u64 = end.parse().ok()?;
        total = total.checked_add(end.checked_sub(start)? + 1)?;
    }
    Some(total as f64)
}

fn min_known(current: &mut Option<f64>, other: Option<f64>) {
    if let Some(value) = other {
        *current = Some(current.map(|previous| previous.min(value)).unwrap_or(value));
    }
}

fn mount_for(path: &Path) -> Option<(String, PathBuf)> {
    let mounts = fs::read_to_string("/proc/self/mountinfo").ok()?;
    mounts
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            let mount = PathBuf::from(fields.get(4)?.replace("\\040", " ").replace("\\134", "\\"));
            let separator = fields.iter().position(|field| *field == "-")?;
            // Device, mount root and filesystem remain stable across the
            // systemd service's private mount namespace. Numeric mount IDs do not.
            let identity = format!(
                "{}:{}:{}",
                fields.get(2)?,
                fields.get(3)?,
                fields.get(separator + 1)?
            );
            path.starts_with(&mount).then_some((identity, mount))
        })
        .max_by_key(|(_, mount)| mount.components().count())
}

fn add_systemd_tenant_paths(paths: &mut Vec<PathBuf>, boundary: &Path) {
    let slice = boundary.join("system.slice");
    if !slice.is_dir() {
        return;
    }
    // Capture the future tenant unit even while the Host is still idle.
    for path in [slice.clone(), slice.join("cc-switch-server.service")] {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
}

pub(super) fn configure(config: &mut Configuration) -> anyhow::Result<()> {
    config.supervision = if Path::new("/run/systemd/system").is_dir() {
        "systemd"
    } else if Path::new("/run/openrc/softlevel").is_file()
        && (Path::new("/sbin/rc-service").exists() || Path::new("/usr/sbin/rc-service").exists())
    {
        "openrc"
    } else if config.supervision == "supervisor" {
        "supervisor"
    } else {
        "nohup"
    }
    .into();
    config.resource_paths.clear();
    let mounts = fs::read_to_string("/proc/self/mountinfo")?;
    let cgroups = fs::read_to_string("/proc/1/cgroup")?;
    // Use PID 1's container boundary and its visible ancestors, never the reporter unit.
    for line in cgroups.lines() {
        let fields: Vec<_> = line.splitn(3, ':').collect();
        if fields.len() != 3 || fields[2].contains("..") {
            continue;
        }
        for mount in mounts.lines() {
            let Some((left, right)) = mount.split_once(" - ") else {
                continue;
            };
            let parts: Vec<_> = left.split_whitespace().collect();
            let fs_fields: Vec<_> = right.split_whitespace().collect();
            if parts.len() < 5 || fs_fields.len() < 3 {
                continue;
            }
            let relevant = fs_fields[0] == "cgroup2" && fields[1].is_empty()
                || fs_fields[0] == "cgroup"
                    && fields[1]
                        .split(',')
                        .any(|name| fs_fields[2].split(',').any(|c| c == name));
            if !relevant {
                continue;
            }
            let root = PathBuf::from(parts[4]);
            let relative = fields[2]
                .strip_prefix(parts[3])
                .unwrap_or(fields[2])
                .trim_start_matches('/');
            let mut candidate = root.join(relative);
            if config.supervision == "systemd"
                && candidate
                    .file_name()
                    .is_some_and(|name| name == "init.scope")
            {
                candidate.pop();
            }
            if !candidate.exists() {
                continue;
            }
            let boundary = candidate.clone();
            loop {
                if !config.resource_paths.contains(&candidate) {
                    config.resource_paths.push(candidate.clone())
                }
                if candidate == root || !candidate.pop() || !candidate.starts_with(&root) {
                    break;
                }
            }
            // Resolve under each controller mount, including cgroup v1's
            // separate CPU/memory hierarchies and namespaced container roots.
            if config.supervision == "systemd" {
                add_systemd_tenant_paths(&mut config.resource_paths, &boundary);
            }
        }
    }
    if config.data_path.as_os_str().is_empty() {
        config.data_path = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/root"))
            .join(".cc-switch-server");
    }
    if let Ok(canonical) = fs::canonicalize(&config.data_path) {
        config.data_path = canonical;
    }
    config.data_mount = mount_for(&config.data_path).map(|(id, _)| id);
    config.data_probe_path = None;
    if let Some((_, mount)) = mount_for(&config.data_path) {
        // Do not grant traversal into tenant directories. Only use an already searchable alias.
        for candidate in [Path::new("/"), Path::new("/var"), mount.as_path()] {
            let accessible = fs::metadata(candidate)
                .ok()
                .is_some_and(|m| m.mode() & 0o005 == 0o005);
            if accessible && mount_for(candidate).map(|(id, _)| id) == config.data_mount {
                config.data_probe_path = Some(candidate.to_path_buf());
                break;
            }
        }
    }
    Ok(())
}

/// `Err` carries the protocol unknown reason: an unverified data mount is a scope
/// problem, a failed or stalled read is a temporary collection failure.
fn disk(config: &Configuration) -> Result<(f64, f64), &'static str> {
    let probe = config.data_probe_path.as_ref().ok_or("scope_unverified")?;
    if mount_for(&config.data_path).map(|(id, _)| id) != config.data_mount
        || mount_for(probe).map(|(id, _)| id) != config.data_mount
    {
        return Err("scope_unverified");
    }
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(probe.as_os_str().as_bytes()).map_err(|_| "unavailable")?;
    let mut result: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut result) } != 0 {
        return Err("unavailable");
    }
    Ok((
        result.f_blocks as f64 * result.f_frsize as f64,
        result.f_bavail as f64 * result.f_frsize as f64,
    ))
}

static DISK_READING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

async fn bounded_disk(config: &Configuration) -> Result<(f64, f64), &'static str> {
    use std::sync::atomic::Ordering;
    if DISK_READING.swap(true, Ordering::AcqRel) {
        return Err("unavailable");
    }
    let config = config.clone();
    let task = tokio::task::spawn_blocking(move || {
        let result = disk(&config);
        DISK_READING.store(false, Ordering::Release);
        result
    });
    // A stalled network mount leaves at most one blocking job. Later hours
    // still report the other measurements and do not reuse an old disk result.
    tokio::time::timeout(Duration::from_millis(250), task)
        .await
        .map_err(|_| "unavailable")?
        .map_err(|_| "unavailable")?
}

fn cpu_counter(paths: &[PathBuf]) -> Option<f64> {
    // Prefer the most specific confirmed tenant boundary.
    let mut paths: Vec<_> = paths.iter().collect();
    paths.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for path in paths {
        if let Ok(stat) = fs::read_to_string(path.join("cpu.stat")) {
            if let Some(value) = stat.lines().find_map(|line| {
                line.strip_prefix("usage_usec ")
                    .and_then(|v| v.parse::<f64>().ok())
            }) {
                return Some(value / 1_000_000.0);
            }
        }
        if let Some(value) = read_number(&path.join("cpuacct.usage")) {
            return Some(value / 1_000_000_000.0);
        }
    }
    None
}

pub(super) async fn snapshot(config: &Configuration, sample_cpu: bool) -> Snapshot {
    let confirmed = !config.resource_paths.is_empty();
    let mut cpu = fs::read_to_string("/sys/devices/system/cpu/online")
        .ok()
        .and_then(|s| cpus(&s));
    let mut memory = memory_info("MemTotal");
    let mut available = memory_info("MemAvailable");
    for path in &config.resource_paths {
        for file in ["cpuset.cpus.effective", "cpuset.cpus"] {
            min_known(
                &mut cpu,
                fs::read_to_string(path.join(file))
                    .ok()
                    .and_then(|v| cpus(&v)),
            );
        }
        if let Ok(value) = fs::read_to_string(path.join("cpu.max")) {
            let fields: Vec<_> = value.split_whitespace().collect();
            if fields.len() == 2 {
                if let (Ok(quota), Ok(period)) =
                    (fields[0].parse::<f64>(), fields[1].parse::<f64>())
                {
                    if period > 0.0 {
                        min_known(&mut cpu, Some(quota / period))
                    }
                }
            }
        }
        if let (Some(quota), Some(period)) = (
            read_number(&path.join("cpu.cfs_quota_us")),
            read_number(&path.join("cpu.cfs_period_us")),
        ) {
            if period > 0.0 {
                min_known(&mut cpu, Some(quota / period))
            }
        }
        for (limit_file, used_file) in [
            ("memory.max", "memory.current"),
            ("memory.limit_in_bytes", "memory.usage_in_bytes"),
        ] {
            if let Some(limit) = read_number(&path.join(limit_file)) {
                min_known(&mut memory, Some(limit));
                min_known(
                    &mut available,
                    read_number(&path.join(used_file)).map(|used| (limit - used).max(0.0)),
                );
            }
        }
    }
    // lxcfs can expose a smaller visible CPU count.
    if let Ok(info) = fs::read_to_string("/proc/cpuinfo") {
        let count = info
            .lines()
            .filter(|line| line.starts_with("processor") && line.contains(':'))
            .count();
        if count > 0 {
            min_known(&mut cpu, Some(count as f64))
        }
    }
    min_known(&mut available, memory);
    let reading = |value: Option<f64>| {
        if confirmed {
            value
                .map(Reading::known)
                .unwrap_or_else(|| Reading::unknown("unavailable"))
        } else {
            Reading::unknown("scope_unverified")
        }
    };
    // A confirmed boundary whose sample was skipped or had no counter is a temporary gap.
    let mut usage = Reading::unknown(if confirmed {
        "unavailable"
    } else {
        "scope_unverified"
    });
    if confirmed && sample_cpu {
        if let (Some(before), Some(capacity)) = (
            cpu_counter(&config.resource_paths),
            cpu.filter(|v| *v > 0.0),
        ) {
            let start = Instant::now();
            tokio::time::sleep(Duration::from_millis(200)).await;
            usage = match cpu_counter(&config.resource_paths) {
                Some(after) if after >= before => Reading::known(
                    ((after - before) / start.elapsed().as_secs_f64() / capacity * 100.0)
                        .clamp(0.0, 100.0),
                ),
                _ => Reading::unknown("counter_reset"),
            };
        }
    }
    let disk = bounded_disk(config).await;
    Snapshot {
        os: "Linux".to_string(),
        arch: std::env::consts::ARCH.to_string(),
        effective_vcpu: reading(cpu),
        memory_total_bytes: reading(memory),
        memory_available_bytes: reading(available),
        data_volume_total_bytes: disk
            .map(|d| Reading::known(d.0))
            .unwrap_or_else(Reading::unknown),
        data_volume_available_bytes: disk
            .map(|d| Reading::known(d.1))
            .unwrap_or_else(Reading::unknown),
        cpu_usage_pct: usage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn nested_container_limits_and_unconfirmed_disk() {
        let path = std::env::temp_dir().join(format!("standby-cgroup-{}", rand::random::<u64>()));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("cpu.max"), "50000 100000").unwrap();
        fs::write(path.join("memory.max"), "536870912").unwrap();
        fs::write(path.join("memory.current"), "268435456").unwrap();
        let config:Configuration=serde_json::from_value(serde_json::json!({
            "hostId":"fixture","routerUrl":"https://router.example","machineEpoch":1,"keyGeneration":1,
            "firstDueAt":"2026-01-01T10:59:59Z","routerTime":"2026-01-01T10:59:55Z",
            "stateDir":path,"supervision":"systemd","operationSequence":1,"resourcePaths":[path],
            "dataPath":"/root/private-independent-volume"
        })).unwrap();
        let report = snapshot(&config, true).await;
        assert_eq!(report.effective_vcpu.value, Some(0.5));
        assert_eq!(report.memory_total_bytes.value, Some(536870912.0));
        assert!(report.memory_available_bytes.value.unwrap() <= 268435456.0);
        assert_eq!(report.data_volume_total_bytes.value, None);
        assert!(report.validate());
        let mut unknown = config;
        unknown.resource_paths.clear();
        let unknown = snapshot(&unknown, true).await;
        assert_eq!(
            unknown.effective_vcpu.unknown_reason.as_deref(),
            Some("scope_unverified")
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn cpuset_and_quota_basics() {
        assert_eq!(cpus("0-3,6,8-9"), Some(7.0));
        assert_eq!(cpus(""), None);
        assert_eq!(cpus("3-1"), None);
        let mut value = Some(64.0);
        min_known(&mut value, Some(0.5));
        min_known(&mut value, Some(2.0));
        assert_eq!(value, Some(0.5));
    }

    #[test]
    fn cpu_counter_uses_deepest_visible_boundary_regardless_of_path_order() {
        let root = std::env::temp_dir().join(format!("standby-cpu-{}", rand::random::<u64>()));
        let tenant = root.join("tenant");
        fs::create_dir_all(&tenant).unwrap();
        fs::write(root.join("cpu.stat"), "usage_usec 9000000000\n").unwrap();
        fs::write(tenant.join("cpu.stat"), "usage_usec 123000\n").unwrap();
        assert_eq!(cpu_counter(&[tenant.clone(), root.clone()]), Some(0.123));
        assert_eq!(cpu_counter(&[root.clone(), tenant]), Some(0.123));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn systemd_tenant_paths_include_future_units_under_each_controller_root() {
        let root = std::env::temp_dir().join(format!("standby-systemd-{}", rand::random::<u64>()));
        let mut paths = Vec::new();
        for controller in ["v1-cpu/container", "v1-memory/container", "v2"] {
            let boundary = root.join(controller);
            fs::create_dir_all(boundary.join("system.slice")).unwrap();
            add_systemd_tenant_paths(&mut paths, &boundary);
            assert!(paths.contains(&boundary.join("system.slice/cc-switch-server.service")));
        }
        assert_eq!(paths.len(), 6);
        fs::remove_dir_all(root).unwrap();
    }
}
