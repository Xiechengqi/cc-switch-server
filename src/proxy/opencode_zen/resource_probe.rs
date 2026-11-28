// Explicit offline probe of the production lane and Bun stdio transport.
// The Rust host also contains the loopback fixture, so host RSS is not a pure
// production-server baseline. Summed child RSS includes shared mappings.
#[derive(Default)]
struct ZenR6ResourceSample {
    samples: u64,
    host_peak_kib: u64,
    child_peak_kib: u64,
    max_children: usize,
    seen: std::collections::BTreeSet<u32>,
}

fn zen_r6_children(pid: u32) -> Vec<u32> {
    let mut out = std::collections::BTreeSet::new();
    if let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) {
        for task in tasks.flatten() {
            if let Ok(raw) = std::fs::read_to_string(task.path().join("children")) {
                out.extend(
                    raw.split_whitespace()
                        .filter_map(|id| id.parse::<u32>().ok()),
                );
            }
        }
    }
    out.into_iter().collect()
}

fn zen_r6_rss_kib(pid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|s| {
            s.lines().find_map(|line| {
                line.strip_prefix("VmRSS:")
                    .and_then(|s| s.split_whitespace().next()?.parse().ok())
            })
        })
        .unwrap_or(0)
}

fn zen_r6_cgroup_snapshot() -> serde_json::Value {
    let relative = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|line| line.strip_prefix("0::").map(str::to_owned))
        });
    let mut fields = serde_json::Map::new();
    if let Some(relative) = relative {
        let root = std::path::Path::new("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
        for name in [
            "memory.current",
            "memory.max",
            "memory.events",
            "memory.pressure",
        ] {
            fields.insert(
                name.into(),
                std::fs::read_to_string(root.join(name))
                    .map(|s| serde_json::Value::String(s.trim().to_owned()))
                    .unwrap_or(serde_json::Value::Null),
            );
        }
    }
    serde_json::Value::Object(fields)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "offline R6 resource experiment; requires pinned Bun and explicit invocation"]
async fn opencode_zen_r6_bun_resource_probe() {
    use crate::proxy::request_memory::{RequestMemoryBudget, RequestMemoryComponent};
    use serde_json::json;
    let executable = bun();
    let manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../assets/contract/opencode-zen-runtime.json"
    ))
    .unwrap();
    let row = manifest["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["asset"] == "bun-linux-x64-baseline")
        .unwrap();
    use sha2::Digest;
    assert_eq!(
        hex::encode(sha2::Sha256::digest(std::fs::read(&executable).unwrap())),
        row["binarySha256"].as_str().unwrap()
    );
    assert_eq!(
        std::env::var("CC_SWITCH_ZEN_MAX_CONCURRENCY").unwrap(),
        "32"
    );
    let before = zen_r6_cgroup_snapshot();
    let mut startup_ms = Vec::new();
    for _ in 0..6 {
        let started = std::time::Instant::now();
        let status = tokio::process::Command::new(&executable)
            .env_clear()
            .env("BUN_BE_BUN", "1")
            .args(["--eval", "process.exit(0)"])
            .status()
            .await
            .unwrap();
        assert!(status.success());
        startup_ms.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    eprintln!(
        "R6_ZEN_RESOURCE {}",
        json!({"phase":"runtime_startup","version":manifest["version"],"milliseconds":startup_ms,"coldMeaning":"first observed launch; OS page cache was not flushed","cgroupBefore":before})
    );
    for (phase, concurrency, request_bytes, response_bytes, slow, cancel) in [
        ("serial_first", 1_usize, 1024_usize, 8192_usize, false, 0u8),
        ("serial_repeat", 1, 1024, 8192, false, 0),
        ("parallel_4", 4, 1024, 8192, false, 0),
        ("parallel_configured_max_32", 32, 1024, 8192, false, 0),
        ("queued_36_over_32", 36, 1024, 8192, false, 0),
        ("slow_consumer_4", 4, 1024, 4 * 1024 * 1024, true, 0),
        (
            "cancel_before_headers_4",
            4,
            1024,
            4 * 1024 * 1024,
            false,
            1,
        ),
        ("cancel_unread_body_4", 4, 1024, 4 * 1024 * 1024, false, 2),
        ("cancel_read_body_4", 4, 1024, 4 * 1024 * 1024, false, 3),
        (
            "transport_maximum_body_128m",
            1,
            128 * 1024 * 1024,
            8192,
            false,
            0,
        ),
    ] {
        assert!(zen_r6_children(std::process::id()).is_empty());
        let sample = Arc::new(std::sync::Mutex::new(ZenR6ResourceSample::default()));
        let running = Arc::new(AtomicBool::new(true));
        let sampling = sample.clone();
        let active = running.clone();
        let sampler = std::thread::spawn(move || {
            while active.load(Ordering::Acquire) {
                let mut children = zen_r6_children(std::process::id());
                let direct = children.clone();
                for pid in direct {
                    children.extend(zen_r6_children(pid));
                }
                children.sort_unstable();
                children.dedup();
                let mut s = sampling.lock().unwrap();
                s.samples += 1;
                s.host_peak_kib = s.host_peak_kib.max(zen_r6_rss_kib(std::process::id()));
                s.child_peak_kib = s
                    .child_peak_kib
                    .max(children.iter().map(|pid| zen_r6_rss_kib(*pid)).sum());
                s.max_children = s.max_children.max(children.len());
                s.seen.extend(children);
                drop(s);
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let app = axum::Router::new()
            .fallback(axum::routing::post(move |body: Bytes| async move {
                assert_eq!(body.len(), request_bytes);
                if cancel == 1 {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(axum::body::Body::from_stream(async_stream::stream! {
                        // A 100ms lifetime makes the active-lane limit observable.
                        for _ in 0..response_bytes.div_ceil(8192) {
                            yield Ok::<Bytes,std::io::Error>(Bytes::from(vec![b'x';8192]));
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }))
                    .unwrap()
            }))
            .layer(axum::extract::DefaultBodyLimit::max(130 * 1024 * 1024));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let url = format!("{origin}/inference/openai/v1/chat/completions");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut workers = Vec::new();
        for _ in 0..concurrency {
            let executable = executable.clone();
            let origin = origin.clone();
            let url = url.clone();
            workers.push(tokio::spawn(async move {
                let queued = std::time::Instant::now();
                let deadline = tokio::time::Instant::now()+Duration::from_secs(30);
                let _permit = crate::proxy::opencode_zen::acquire_lane(&origin,"resource-free",Default::default(),deadline).await.unwrap();
                let queue_ms = queued.elapsed().as_secs_f64()*1000.0;
                let payload = vec![b'a';request_bytes];
                let sent = std::time::Instant::now();
                let work = post_inner(&executable,&url,&payload,"ses_resource_probe",true,deadline,deadline,true);
                if cancel == 1 {
                    assert!(tokio::time::timeout(Duration::from_millis(80),work).await.is_err());
                    return json!({"queueMs":queue_ms,"cancelPhase":"before_headers","elapsedMs":sent.elapsed().as_secs_f64()*1000.0});
                }
                let response = work.await.unwrap();
                assert_eq!(response.status(),StatusCode::OK);
                let headers_ms = sent.elapsed().as_secs_f64()*1000.0;
                if cancel == 2 { drop(response); return json!({"queueMs":queue_ms,"transportHeadersMs":headers_ms,"cancelPhase":"unread_body"}); }
                let body_started = std::time::Instant::now();
                let mut body = response.bytes_stream(); let mut received = 0;
                while let Some(chunk) = body.next().await {
                    received += chunk.unwrap().len();
                    if cancel == 3 { break; }
                    if slow { tokio::time::sleep(Duration::from_millis(20)).await; }
                }
                drop(body);
                if cancel == 0 { assert_eq!(received,response_bytes); }
                json!({"queueMs":queue_ms,"transportHeadersMs":headers_ms,"bodyMs":body_started.elapsed().as_secs_f64()*1000.0,"receivedBytes":received,"cancelPhase":if cancel==3 {"read_body"} else {"none"}})
            }));
        }
        let mut timings = Vec::new();
        for worker in workers {
            timings.push(worker.await.unwrap());
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            while !zen_r6_children(std::process::id()).is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("Bun children must be killed and reaped after every phase");
        running.store(false, Ordering::Release);
        sampler.join().unwrap();
        server.abort();
        let s = sample.lock().unwrap();
        assert!(
            s.max_children <= 32,
            "active lane exceeded configured maximum"
        );
        eprintln!(
            "R6_ZEN_RESOURCE {}",
            json!({"phase":phase,"concurrency":concurrency,"requestBytes":request_bytes,"responseBytes":response_bytes,"hostPeakRssKiB":s.host_peak_kib,"summedChildPeakRssKiB":s.child_peak_kib,"maxObservedChildren":s.max_children,"observedChildPids":s.seen,"sampleCount":s.samples,"sampleIntervalMs":5,"remainingChildren":0,"timings":timings})
        );
    }
    let budget = RequestMemoryBudget::new(1024);
    assert!(budget
        .reserve(
            RequestMemoryComponent::NormalizedBody,
            256 * 1024 * 1024 + 128 * 1024
        )
        .is_err());
    assert!(zen_r6_children(std::process::id()).is_empty());
    eprintln!(
        "R6_ZEN_RESOURCE {}",
        json!({"phase":"request_budget_pressure","reservationAccepted":false,"remainingChildren":0,"scope":"logical request budget; no cgroup limit was changed and no OS OOM was induced","cgroupAfter":zen_r6_cgroup_snapshot()})
    );
}
