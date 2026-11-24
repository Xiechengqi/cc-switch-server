//! Effective hardware for the process serving the online lease.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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

fn resource_paths(mounts: &str, cgroups: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
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
            if !candidate.exists() {
                continue;
            }
            loop {
                if !paths.contains(&candidate) {
                    paths.push(candidate.clone())
                }
                if candidate == root || !candidate.pop() || !candidate.starts_with(&root) {
                    break;
                }
            }
        }
    }
    paths
}

fn capacity(paths: &[PathBuf]) -> (Option<f64>, Option<f64>) {
    if paths.is_empty() {
        return (None, None);
    }
    let mut cpu = fs::read_to_string("/sys/devices/system/cpu/online")
        .ok()
        .and_then(|s| cpus(&s));
    let mut memory = memory_info("MemTotal");
    for path in paths {
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
        for file in ["memory.max", "memory.limit_in_bytes"] {
            min_known(&mut memory, read_number(&path.join(file)));
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
    (cpu.filter(|n| *n > 0.0), memory.filter(|n| *n > 0.0))
}

fn disk_bytes(path: &Path) -> Option<f64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut result = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // statvfs initializes the result only on success.
    if unsafe { libc::statvfs(path.as_ptr(), result.as_mut_ptr()) } != 0 {
        return None;
    }
    let result = unsafe { result.assume_init() };
    Some(result.f_blocks as f64 * result.f_frsize as f64).filter(|n| *n > 0.0)
}

static DISK_READING: AtomicBool = AtomicBool::new(false);

pub(crate) async fn workspace_hardware(data_dir: PathBuf) -> anyhow::Result<Value> {
    let (cpu, memory) = tokio::task::spawn_blocking(|| {
        let mounts = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        let cgroups = fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
        capacity(&resource_paths(&mounts, &cgroups))
    })
    .await?;
    // A stalled data volume must not queue further blocking filesystem calls.
    let disk = if DISK_READING.swap(true, Ordering::AcqRel) {
        None
    } else {
        let probe = tokio::task::spawn_blocking(move || {
            let value = disk_bytes(&data_dir);
            DISK_READING.store(false, Ordering::Release);
            value
        });
        tokio::time::timeout(Duration::from_millis(250), probe)
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
    };
    Ok(
        json!({"arch": std::env::consts::ARCH, "cpu": cpu, "memoryBytes": memory, "diskBytes": disk}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_capacity_observes_the_process_cgroup_and_ancestors() {
        let root = std::env::temp_dir().join(format!("client-hardware-{}", rand::random::<u64>()));
        let leaf = root.join("client");
        fs::create_dir_all(&leaf).unwrap();
        fs::write(root.join("cpu.max"), "50000 100000").unwrap();
        fs::write(leaf.join("memory.max"), "536870912").unwrap();
        let mounts = format!("1 0 0:1 / {} rw - cgroup2 cgroup rw", root.display());
        let paths = resource_paths(&mounts, "0::/client");
        assert_eq!(paths, [leaf, root.clone()]);
        assert_eq!(capacity(&paths), (Some(0.5), Some(536870912.0)));
        assert_eq!(capacity(&[]), (None, None));
        assert!(disk_bytes(&root).is_some());
        fs::remove_dir_all(root).unwrap();
    }
}
