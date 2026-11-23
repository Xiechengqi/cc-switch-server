//! Lightweight Host reporter. It never initializes tenant configuration or services.
mod collect;

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand};
use ed25519_dalek::{Signer, SigningKey};
use fs2::FileExt;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

use crate::host_health_protocol::{self as wire, Report};
use crate::infra::storage::write_bytes_atomic;

#[derive(Debug, Clone, Args)]
pub struct StandbyArgs {
    #[arg(
        long,
        global = true,
        default_value = "/etc/cc-switch-host-standby/config.json"
    )]
    pub config: PathBuf,
    #[command(subcommand)]
    pub command: Option<StandbyCommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum StandbyCommand {
    /// Read managed configuration from stdin and initialize local keys without sending.
    Init,
    /// Print local hardware and public key. Never sends a heartbeat.
    Probe {
        #[arg(long)]
        challenge: Option<String>,
    },
    /// Run the hourly reporter.
    Run,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Configuration {
    pub host_id: String,
    pub router_url: String,
    pub machine_epoch: u64,
    pub key_generation: u64,
    pub first_due_at: String,
    pub router_time: String,
    pub state_dir: PathBuf,
    pub supervision: String,
    pub operation_sequence: u64,
    #[serde(default)]
    pub resource_paths: Vec<PathBuf>,
    #[serde(default)]
    pub data_path: PathBuf,
    #[serde(default)]
    pub data_probe_path: Option<PathBuf>,
    #[serde(default)]
    pub data_mount: Option<String>,
    #[serde(default)]
    pub anchor_boot_id: String,
    #[serde(default)]
    pub anchor_boot_millis: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendState {
    attempted_hours: BTreeSet<i64>,
    closed_before: i64,
    revoked_generation: Option<u64>,
    #[serde(default)]
    clock_anchor: Option<ClockAnchor>,
}

impl SendState {
    fn claim(&mut self, hour: i64) -> bool {
        hour >= self.closed_before && self.attempted_hours.insert(hour)
    }

    fn compact(&mut self, trusted_hour: i64) {
        let boundary = trusted_hour.saturating_sub(24 * 7);
        self.closed_before = self.closed_before.max(boundary);
        self.attempted_hours
            .retain(|hour| *hour >= self.closed_before);
    }
}

fn timestamp(value: &str) -> anyhow::Result<i64> {
    Ok(DateTime::parse_from_rfc3339(value)?.timestamp_millis())
}

fn boot_id() -> String {
    fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn boot_millis() -> u64 {
    // CLOCK_BOOTTIME includes time spent suspended.
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut time) } != 0 {
        return 0;
    }
    time.tv_sec as u64 * 1000 + time.tv_nsec as u64 / 1_000_000
}

fn synchronized_wall_clock() -> bool {
    let mut status: libc::timex = unsafe { std::mem::zeroed() };
    unsafe { libc::adjtimex(&mut status) >= 0 && status.status & libc::STA_UNSYNC == 0 }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClockAnchor {
    boot_id: String,
    utc_millis: i64,
    boot_millis: u64,
}

struct Clock {
    utc_millis: i64,
    boot_millis: u64,
    from_router: bool,
}

#[derive(Debug, PartialEq)]
enum ClockChange {
    Stable,
    Paused,
    Rebased,
}

impl Clock {
    fn load(config: &Configuration) -> anyhow::Result<Option<Self>> {
        let boot = boot_millis();
        if !config.anchor_boot_id.is_empty()
            && config.anchor_boot_id == boot_id()
            && boot >= config.anchor_boot_millis
        {
            return Ok(Some(Self {
                utc_millis: timestamp(&config.router_time)?,
                boot_millis: config.anchor_boot_millis,
                from_router: true,
            }));
        }
        if synchronized_wall_clock() {
            return Ok(Some(Self {
                utc_millis: Utc::now().timestamp_millis(),
                boot_millis: boot,
                from_router: false,
            }));
        }
        Ok(None)
    }

    fn now(&self) -> i64 {
        self.utc_millis + boot_millis().saturating_sub(self.boot_millis) as i64
    }
    fn reconcile(&mut self, wall_millis: i64, boot_millis: u64, synchronized: bool) -> ClockChange {
        let estimated = self.utc_millis + boot_millis.saturating_sub(self.boot_millis) as i64;
        if estimated.abs_diff(wall_millis) <= 120_000 {
            return ClockChange::Stable;
        }
        if !self.from_router && synchronized {
            self.utc_millis = wall_millis;
            self.boot_millis = boot_millis;
            ClockChange::Rebased
        } else {
            ClockChange::Paused
        }
    }
}

fn load_key(config: &Configuration) -> anyhow::Result<SigningKey> {
    let bytes = fs::read(
        config
            .state_dir
            .join(format!("key-{}", config.key_generation)),
    )?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid standby key length"))?;
    Ok(SigningKey::from_bytes(&seed))
}

fn save_state(path: &Path, state: &SendState) -> anyhow::Result<()> {
    write_bytes_atomic(path, &serde_json::to_vec(state)?)
}

/// The run loop never exits on a state write failure: a claimed slot that cannot be
/// persisted is skipped (the in-memory claim still prevents a second send), and a
/// `nohup` deployment has no supervisor to restart a process that exited.
fn persist(path: &Path, state: &SendState) -> bool {
    match save_state(path, state) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, "standby could not persist send state; skipping this slot");
            false
        }
    }
}

pub async fn execute(args: StandbyArgs) -> anyhow::Result<()> {
    match args.command.unwrap_or(StandbyCommand::Run) {
        StandbyCommand::Init => initialize(&args.config),
        StandbyCommand::Probe { challenge } => {
            let config: Configuration = serde_json::from_slice(&fs::read(args.config)?)?;
            let key = load_key(&config)?;
            let proof = challenge
                .as_ref()
                .map(|value| URL_SAFE_NO_PAD.encode(key.sign(value.as_bytes()).to_bytes()));
            println!(
                "{}",
                serde_json::json!({
                    "publicKey": URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes()),
                    "proof": proof, "supervision": config.supervision,
                    // Without kernel time sync the sender pauses after a reboot until
                    // the clock is confirmed again; Router surfaces this to the provider.
                    "clockSynchronized": synchronized_wall_clock(),
                    "operationSequence": config.operation_sequence,
                    "snapshot": collect::snapshot(&config, true).await,
                })
            );
            Ok(())
        }
        StandbyCommand::Run => run(&args.config).await,
    }
}

/// Longest delay accepted between Router stamping `routerTime` and `init` running.
const MAX_DELIVERY_LAG_MS: i64 = 900_000;

fn initialize(path: &Path) -> anyhow::Result<()> {
    let mut input = Vec::new();
    std::io::stdin().take(8193).read_to_end(&mut input)?;
    if input.len() > 8192 {
        bail!("managed configuration exceeds 8 KiB")
    }
    let mut config: Configuration = serde_json::from_slice(&input)?;
    let router = url::Url::parse(&config.router_url)?;
    if router.scheme() != "https"
        || router.host_str().is_none()
        || !router.username().is_empty()
        || router.password().is_some()
    {
        bail!("standby requires an HTTPS Router URL");
    }
    timestamp(&config.first_due_at)?;
    let router_millis = timestamp(&config.router_time)?;
    // Router stamps `routerTime` before the remote preparation runs, so the value may
    // lag by the delivery time (dependency install, download lock) but never lead.
    let wall_millis = Utc::now().timestamp_millis();
    let lag = wall_millis - router_millis;
    if !(-120_000..=120_000 + MAX_DELIVERY_LAG_MS).contains(&lag) {
        bail!("managed Router time is outside the clock tolerance; check Host time and retry enrollment");
    }
    if synchronized_wall_clock() {
        // A synchronized clock is more accurate than the delayed Router stamp.
        config.router_time = Utc::now().to_rfc3339();
    }
    if config.machine_epoch == 0 || config.key_generation == 0 || config.host_id.is_empty() {
        bail!("invalid managed identity")
    }
    fs::create_dir_all(&config.state_dir)?;
    let restoring_existing_sender = path.exists();
    if restoring_existing_sender {
        let previous: Configuration = serde_json::from_slice(&fs::read(path)?)?;
        if previous.host_id != config.host_id || previous.machine_epoch != config.machine_epoch {
            bail!("standby identity changed; revoke and remove the old installation first");
        }
        if previous.operation_sequence > config.operation_sequence {
            bail!("stale managed operation")
        }
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(config.state_dir.join("sender.lock"))?;
    lock.try_lock_exclusive()
        .context("stop standby before changing managed configuration")?;
    let key_path = config
        .state_dir
        .join(format!("key-{}", config.key_generation));
    if !key_path.exists() {
        let key = SigningKey::generate(&mut OsRng);
        write_bytes_atomic(&key_path, &key.to_bytes())?;
    }
    let state_path = config.state_dir.join("send-state.json");
    if state_path.exists() {
        // The new managed time base replaces an anchor kept by the previous sender.
        let mut state: SendState = serde_json::from_slice(&fs::read(&state_path)?)?;
        if state.clock_anchor.take().is_some() {
            save_state(&state_path, &state)?;
        }
    }
    if !state_path.exists() {
        let mut state = SendState::default();
        // A replacement sender may only start from its delegated hour.
        state.closed_before = timestamp(&config.first_due_at)?.div_euclid(3_600_000);
        if restoring_existing_sender {
            state.closed_before = state
                .closed_before
                .max(timestamp(&config.router_time)?.div_euclid(3_600_000) + 1);
        }
        save_state(&config.state_dir.join("send-state.json"), &state)?;
    }
    config.anchor_boot_id = boot_id();
    config.anchor_boot_millis = boot_millis();
    collect::configure(&mut config)?;
    write_bytes_atomic(path, &serde_json::to_vec(&config)?)?;
    Ok(())
}

/// Router may deliver the first slot shortly before the service starts. A late start
/// may still send inside Router's 5-minute grace, leaving room for network latency.
const FIRST_SEND_WINDOW_MS: i64 = 240_000;
const REGULAR_SEND_WINDOW_MS: i64 = 30_000;
/// The snapshot (≤200 ms CPU sample + ≤250 ms disk read) is prepared just before the
/// due time so a slot near the end of the hour is not lost to collection.
const PREPARE_LEAD_MS: i64 = 1_000;

async fn run(path: &Path) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()?;
    let config: Configuration = serde_json::from_slice(&fs::read(path)?)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(config.state_dir.join("sender.lock"))?;
    lock.try_lock_exclusive()
        .context("standby sender already running")?;
    let key = load_key(&config)?;
    let state_path = config.state_dir.join("send-state.json");
    // Missing/corrupt state must be repaired through managed enrollment.
    let mut state: SendState = serde_json::from_slice(&fs::read(&state_path)?)?;
    if state.revoked_generation == Some(config.key_generation) {
        return Ok(());
    }
    let mut clock = if let Some(anchor) = state
        .clock_anchor
        .as_ref()
        .filter(|a| {
            !a.boot_id.is_empty() && a.boot_id == boot_id() && boot_millis() >= a.boot_millis
        })
    {
        Some(Clock {
            utc_millis: anchor.utc_millis,
            boot_millis: anchor.boot_millis,
            from_router: true,
        })
    } else {
        Clock::load(&config)?
    };
    let first = timestamp(&config.first_due_at)?;
    let first_hour = first.div_euclid(3_600_000);
    let mut last_clock_diagnostic: Option<std::time::Instant> = None;
    let mut clock_paused = false;
    loop {
        if clock.is_none() {
            clock = Clock::load(&config)?;
            if let Some(current) = &clock {
                state.claim(current.now().div_euclid(3_600_000));
                persist(&state_path, &state);
            } else if last_clock_diagnostic
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(60))
            {
                tracing::warn!("standby awaiting synchronized system time");
                last_clock_diagnostic = Some(std::time::Instant::now());
            }
        }
        if let Some(current) = &mut clock {
            match current.reconcile(
                Utc::now().timestamp_millis(),
                boot_millis(),
                synchronized_wall_clock(),
            ) {
                ClockChange::Stable => {
                    if clock_paused {
                        state.claim(current.now().div_euclid(3_600_000));
                        persist(&state_path, &state);
                    }
                    clock_paused = false;
                }
                ClockChange::Rebased => {
                    state.claim(current.now().div_euclid(3_600_000));
                    persist(&state_path, &state);
                    clock_paused = false;
                }
                ClockChange::Paused => {
                    if !clock_paused {
                        tracing::warn!("standby paused for a system clock discrepancy");
                    }
                    clock_paused = true;
                }
            }
        }
        if let Some(current) = clock.as_ref().filter(|_| !clock_paused) {
            let now = current.now();
            let hour = now.div_euclid(3_600_000);
            let (due, window) = if hour == first_hour {
                (first, FIRST_SEND_WINDOW_MS)
            } else {
                (wire::due_at(&config.host_id, hour) * 1000, REGULAR_SEND_WINDOW_MS)
            };
            if hour >= first_hour
                && now >= due - PREPARE_LEAD_MS
                && now <= due + window
                && state.claim(hour)
                && persist(&state_path, &state)
            {
                let snapshot = collect::snapshot(&config, (hour + 1) * 3_600_000 - now > 500).await;
                let sampled = current.now();
                if sampled < due {
                    tokio::time::sleep(Duration::from_millis((due - sampled) as u64)).await;
                }
                // Collection may cross the hour or take too long. The slot stays spent.
                let sending = current.now();
                if sending.div_euclid(3_600_000) == hour && sending <= due + window {
                    let report = Report {
                        report_version: wire::VERSION,
                        host_id: config.host_id.clone(),
                        machine_epoch: config.machine_epoch,
                        key_generation: config.key_generation,
                        utc_hour: hour,
                        sampled_at: DateTime::<Utc>::from_timestamp_millis(sampled)
                            .context("sample time")?
                            .to_rfc3339(),
                        build_version: crate::build_info::VERSION_LINE.to_owned(),
                        snapshot,
                    };
                    let body = serde_json::to_vec(&report)?;
                    if body.len() > wire::MAX_BODY || !report.validate() {
                        // The slot stays spent; keep running for the next hour.
                        tracing::warn!("standby skipped an invalid local health report");
                        continue;
                    }
                    let signature =
                        URL_SAFE_NO_PAD.encode(key.sign(&wire::signing_message(&body)).to_bytes());
                    let result = client
                        .post(format!(
                            "{}/v1/client-market/host-heartbeats",
                            config.router_url.trim_end_matches('/')
                        ))
                        .header("X-CC-Report-Version", wire::VERSION)
                        .header("X-CC-Host-Id", &config.host_id)
                        .header("X-CC-Machine-Epoch", config.machine_epoch)
                        .header("X-CC-Key-Generation", config.key_generation)
                        .header("X-CC-Report-Signature", signature)
                        .header("Content-Type", "application/json")
                        .body(body)
                        .send()
                        .await;
                    match result {
                        Ok(response) => {
                            let status = response.status();
                            // Bound diagnostics even if the upstream sends an HTML error page.
                            let mut response = response;
                            let mut bytes = Vec::new();
                            while let Ok(Some(chunk)) = response.chunk().await {
                                if bytes.len() + chunk.len() > 8192 {
                                    break;
                                }
                                bytes.extend_from_slice(&chunk);
                            }
                            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                                let matches = value["utcHour"] == report.utc_hour
                                    && value["hostId"] == config.host_id
                                    && value["machineEpoch"] == config.machine_epoch
                                    && value["keyGeneration"] == config.key_generation;
                                if matches
                                    && status.as_u16() == 410
                                    && value["code"] == "host_agent_revoked"
                                {
                                    state.revoked_generation = Some(config.key_generation);
                                    persist(&state_path, &state);
                                    return Ok(());
                                }
                                if matches {
                                    if let Some(time) =
                                        value["routerTime"].as_str().and_then(|t| timestamp(t).ok())
                                    {
                                        let corrected_hour = time.div_euclid(3_600_000);
                                        // Only a real clock discrepancy spends the Router's
                                        // hour; a response that crosses the hour through
                                        // ordinary latency must not consume the next slot.
                                        if corrected_hour != hour
                                            && time.abs_diff(current.now()) > 120_000
                                        {
                                            state.claim(corrected_hour);
                                        }
                                        let boot = boot_millis();
                                        state.compact(corrected_hour);
                                        let id = boot_id();
                                        state.clock_anchor = (!id.is_empty()).then(|| ClockAnchor {
                                            boot_id: id,
                                            utc_millis: time,
                                            boot_millis: boot,
                                        });
                                        persist(&state_path, &state);
                                        clock = Some(Clock {
                                            utc_millis: time,
                                            boot_millis: boot,
                                            from_router: true,
                                        });
                                    }
                                }
                            }
                            if !status.is_success() {
                                tracing::warn!(%status, "standby report rejected; waiting for next hour")
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "standby report failed; waiting for next hour")
                        }
                    }
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {},
            _ = tokio::signal::ctrl_c() => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn actual_http_attempts_are_not_retried_or_redirected_and_survive_restart() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // Avoid a real hour transition during this process-level scheduling test.
        let now = Utc::now().timestamp();
        if now.rem_euclid(3600) >= 3595 {
            tokio::time::sleep(Duration::from_secs((3601 - now.rem_euclid(3600)) as u64)).await;
        }
        for (status, matching_revocation) in [
            (200, false),
            (302, false),
            (429, false),
            (500, false),
            (410, false),
            (410, true),
        ] {
            let directory =
                std::env::temp_dir().join(format!("standby-http-{}", rand::random::<u64>()));
            fs::create_dir_all(&directory).unwrap();
            let path = directory.join("config.json");
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let (received, mut count) = tokio::sync::watch::channel(0);
            let host_id = format!("http-{status}-{matching_revocation}");
            let expected_host = host_id.clone();
            let router = tokio::spawn(async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut bytes = Vec::new();
                    let (headers, body) = loop {
                        let mut chunk = [0; 4096];
                        let size = socket.read(&mut chunk).await.unwrap();
                        assert!(size > 0);
                        bytes.extend_from_slice(&chunk[..size]);
                        if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                            let headers = std::str::from_utf8(&bytes[..end]).unwrap();
                            let length: usize = headers
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase()
                                        .strip_prefix("content-length: ")
                                        .map(str::to_owned)
                                })
                                .unwrap()
                                .parse()
                                .unwrap();
                            if bytes.len() >= end + 4 + length {
                                break (
                                    headers.to_owned(),
                                    bytes[end + 4..end + 4 + length].to_vec(),
                                );
                            }
                        }
                    };
                    assert!(headers.starts_with("POST /v1/client-market/host-heartbeats HTTP/1.1"));
                    let report: Report = serde_json::from_slice(&body).unwrap();
                    assert_eq!(report.host_id, expected_host);
                    assert!(report.validate());
                    let signature = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .starts_with("x-cc-report-signature: ")
                                .then(|| line.split_once(": ").unwrap().1)
                        })
                        .unwrap();
                    SigningKey::from_bytes(&[7; 32])
                        .verifying_key()
                        .verify_strict(
                            &wire::signing_message(&body),
                            &ed25519_dalek::Signature::from_slice(
                                &URL_SAFE_NO_PAD.decode(signature).unwrap(),
                            )
                            .unwrap(),
                        )
                        .unwrap();
                    let body = serde_json::json!({"code":"host_agent_revoked","hostId":report.host_id,"machineEpoch":report.machine_epoch,"keyGeneration":if matching_revocation {report.key_generation} else {report.key_generation+1},"utcHour":report.utc_hour,"routerTime":Utc::now().to_rfc3339()}).to_string();
                    let response = format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nLocation: /redirected\r\nConnection: close\r\n\r\n{body}", body.len());
                    socket.write_all(response.as_bytes()).await.unwrap();
                    received.send_modify(|count| *count += 1);
                }
            });
            let config = Configuration {
                host_id,
                router_url: url,
                machine_epoch: 1,
                key_generation: 1,
                first_due_at: Utc::now().to_rfc3339(),
                router_time: Utc::now().to_rfc3339(),
                state_dir: directory.clone(),
                supervision: "nohup".into(),
                operation_sequence: 1,
                resource_paths: vec![],
                data_path: PathBuf::new(),
                data_probe_path: None,
                data_mount: None,
                anchor_boot_id: boot_id(),
                anchor_boot_millis: boot_millis(),
            };
            write_bytes_atomic(&path, &serde_json::to_vec(&config).unwrap()).unwrap();
            write_bytes_atomic(&directory.join("key-1"), &[7; 32]).unwrap();
            save_state(&directory.join("send-state.json"), &SendState::default()).unwrap();
            let first_path = path.clone();
            let first = tokio::spawn(async move { run(&first_path).await });
            tokio::time::timeout(Duration::from_secs(5), count.changed())
                .await
                .unwrap()
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(first.is_finished(), matching_revocation);
            first.abort();
            let _ = first.await;
            let state: SendState =
                serde_json::from_slice(&fs::read(directory.join("send-state.json")).unwrap())
                    .unwrap();
            assert_eq!(state.attempted_hours.len(), 1);
            assert_eq!(state.revoked_generation, matching_revocation.then_some(1));
            let second_path = path.clone();
            let second = tokio::spawn(async move { run(&second_path).await });
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(
                *count.borrow(),
                1,
                "status {status} must consume the persisted hour"
            );
            second.abort();
            let _ = second.await;
            router.abort();
            let _ = router.await;
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn clock_recovery_preserves_router_time_and_rebases_unconfirmed_boot_time() {
        let mut router = Clock {
            utc_millis: 1_000_000,
            boot_millis: 1000,
            from_router: true,
        };
        assert_eq!(
            router.reconcile(99_000_000, 1000, true),
            ClockChange::Paused
        );
        assert_eq!(router.utc_millis, 1_000_000);
        assert_eq!(router.reconcile(1_001_000, 2000, true), ClockChange::Stable);
        let mut reboot = Clock {
            utc_millis: 99_000_000,
            boot_millis: 1000,
            from_router: false,
        };
        assert_eq!(
            reboot.reconcile(1_000_000, 2000, false),
            ClockChange::Paused
        );
        assert_eq!(
            reboot.reconcile(1_000_000, 2000, true),
            ClockChange::Rebased
        );
        assert_eq!(reboot.utc_millis, 1_000_000);
    }

    #[test]
    fn slots_survive_restart_and_clock_correction() {
        let mut state = SendState::default();
        assert!(state.claim(100));
        assert!(state.claim(100_000));
        let mut restored: SendState =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(!restored.claim(100));
        assert!(restored.claim(101));
        restored.compact(102);
        assert!(!restored.claim(100_000));
        assert!(restored.claim(103));
    }
}
