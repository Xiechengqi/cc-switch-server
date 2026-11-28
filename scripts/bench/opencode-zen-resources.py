#!/usr/bin/env python3
"""Measure the repository's opt-in Zen lane/Bun transport probe on loopback.

Every run uses a new private cgroup. The parent controller policy is never
changed. A pressure-induced 504 is recorded as a negative capacity observation,
with its original test exit code; it is not a successful request or test pass.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile
import threading
import time
import uuid

PHASES = ["runtime_startup", "serial_first", "serial_repeat", "parallel_4",
          "parallel_configured_max_32", "queued_36_over_32", "slow_consumer_4",
          "cancel_before_headers_4", "cancel_unread_body_4", "cancel_read_body_4",
          "transport_maximum_body_128m", "request_budget_pressure", "finished"]
MARKER = "R6_ZEN_RESOURCE "


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def counters(raw):
    return {name: int(value) for name, value in (line.split() for line in raw.splitlines())}


def classify(mode, code, raw, events, last_phase, memory_events, active_before):
    if active_before:
        return "unexpected_child_cleanup_failure"
    if code == 0 and "test result: ok. 1 passed; 0 failed" in raw and len(events) == 12:
        return "normal_probe_passed" if mode == "normal" else "pressure_probe_passed"
    if (mode == "pressure" and code == 101 and "status: 504" in raw
            and last_phase == "transport_maximum_body_128m"
            and memory_events.get("high", 0) > 0
            and memory_events.get("oom", 0) == 0
            and memory_events.get("oom_kill", 0) == 0):
        return "observed_controlled_504_under_memory_high"
    return "unexpected_probe_failure"


def children(pid):
    result = set()
    try:
        for task in (Path("/proc") / str(pid) / "task").iterdir():
            try:
                result.update(int(value) for value in (task / "children").read_text().split())
            except OSError:
                pass
    except OSError:
        pass
    return result


def rss_kib(pid):
    try:
        for line in (Path("/proc") / str(pid) / "status").read_text().splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
    except OSError:
        pass
    return 0


def wait_empty(group):
    deadline = time.monotonic() + 3
    while (group / "cgroup.procs").read_text().split() and time.monotonic() < deadline:
        time.sleep(.02)
    return (group / "cgroup.procs").read_text().split()


def measure(args, mode, high, env):
    group = args.cgroup_parent / ("cc-switch-zen-probe-" + uuid.uuid4().hex)
    group.mkdir()
    stop = threading.Event()
    trace, events, thread_errors = [], [], []
    log = args.output_dir / (mode + ".log")
    sample_path = args.output_dir / (mode + "-samples.json")
    sampler = None
    result = None
    try:
        (group / "memory.max").write_text(str(args.memory_max_mib * 1024 ** 2))
        (group / "memory.high").write_text(high)
        (group / "pids.max").write_text("1024")
        if (group / "memory.swap.max").exists():
            (group / "memory.swap.max").write_text("0")

        def enter_group():
            (group / "cgroup.procs").write_text(str(os.getpid()))

        with log.open("w") as output:
            process = subprocess.Popen(
                [str(args.binary), "opencode_zen_r6_bun_resource_probe", "--ignored",
                 "--nocapture", "--test-threads=1"], cwd=args.root, env=env,
                stdout=output, stderr=subprocess.STDOUT, preexec_fn=enter_group)
            started = time.monotonic()

            def sample():
                phase, pending = PHASES[0], ""
                try:
                    with log.open() as source:
                        while not stop.is_set():
                            pending += source.read()
                            lines = pending.split("\n")
                            pending = lines.pop()
                            for line in lines:
                                if MARKER in line:
                                    event = json.loads(line.split(MARKER, 1)[1])
                                    events.append(event)
                                    phase = PHASES[PHASES.index(event["phase"]) + 1]
                            pids = children(process.pid)
                            for pid in list(pids):
                                pids.update(children(pid))
                            rows = [{"pid": pid, "rssKiB": rss_kib(pid)} for pid in sorted(pids)]
                            trace.append({"elapsedMs": (time.monotonic() - started) * 1000,
                                          "phase": phase, "hostRssKiB": rss_kib(process.pid),
                                          "children": rows,
                                          "summedChildRssKiB": sum(row["rssKiB"] for row in rows),
                                          "cgroupMemoryCurrentBytes": int((group / "memory.current").read_text())})
                            stop.wait(.01)
                except Exception as error:
                    thread_errors.append(type(error).__name__)

            sampler = threading.Thread(target=sample)
            sampler.start()
            try:
                code = process.wait(timeout=180)
            except subprocess.TimeoutExpired:
                (group / "cgroup.kill").write_text("1")
                process.wait()
                code = -999
            stop.set()
            sampler.join()
        raw = log.read_text()
        # Read complete events again so a final event between two samples is retained.
        events = [json.loads(line.split(MARKER, 1)[1]) for line in raw.splitlines() if MARKER in line]
        active_before = (group / "cgroup.procs").read_text().split()
        forced = bool(active_before)
        if forced:
            (group / "cgroup.kill").write_text("1")
        remaining = wait_empty(group)
        memory_events = counters((group / "memory.events").read_text())
        phases = []
        for phase in PHASES[:-1]:
            rows = [row for row in trace if row["phase"] == phase]
            if rows:
                phases.append({"phase": phase, "samples": len(rows),
                               "hostPeakRssKiB": max(row["hostRssKiB"] for row in rows),
                               "summedChildPeakRssKiB": max(row["summedChildRssKiB"] for row in rows),
                               "cgroupPeakObservedBytes": max(row["cgroupMemoryCurrentBytes"] for row in rows),
                               "maxObservedChildren": max(len(row["children"]) for row in rows),
                               "windowMs": rows[-1]["elapsedMs"] - rows[0]["elapsedMs"]})
        outcome = classify(mode, code, raw, events, phases[-1]["phase"] if phases else None,
                           memory_events, active_before)
        if thread_errors or remaining:
            outcome = "unexpected_probe_failure"
        result = {"mode": mode, "outcome": outcome, "probeExitCode": code,
                  "probePassed": code == 0, "memoryHighBytes": high,
                  "memoryMaxBytes": args.memory_max_mib * 1024 ** 2,
                  "remainingBeforeForcedCleanup": active_before,
                  "forcedCleanup": forced, "remainingAfterCleanup": remaining,
                  "memoryEventsAfter": memory_events, "sampleErrors": thread_errors,
                  "summaries": phases, "events": events,
                  "rawLog": str(log), "rawTrace": str(sample_path)}
        sample_path.write_text(json.dumps(trace, separators=(",", ":")) + "\n")
    finally:
        stop.set()
        if sampler:
            sampler.join()
        if (group / "cgroup.procs").read_text().split():
            (group / "cgroup.kill").write_text("1")
        if wait_empty(group):
            raise RuntimeError("private resource cgroup is still populated")
        group.rmdir()
    result["privateCgroupRemoved"] = not group.exists()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path, help="compiled Server library test binary")
    parser.add_argument("--bun", required=True, type=Path, help="pinned Bun 1.4.4 Linux x64 baseline artifact")
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--cgroup-parent", type=Path)
    parser.add_argument("--memory-max-mib", type=int, default=4096)
    parser.add_argument("--pressure-high-mib", type=int, default=384)
    args = parser.parse_args()
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        parser.error("this version of the resource probe is scoped to Linux x86_64")
    args.root = Path(__file__).resolve().parents[2]
    args.binary, args.bun = args.binary.resolve(), args.bun.resolve()
    manifest = json.loads((args.root / "assets/contract/opencode-zen-runtime.json").read_text())
    row = next(row for row in manifest["artifacts"] if row["asset"] == "bun-linux-x64-baseline")
    if sha256(args.bun) != row["binarySha256"]:
        parser.error("Bun does not match the repository runtime pin")
    listing = subprocess.check_output([str(args.binary), "opencode_zen_r6_bun_resource_probe", "--list"], text=True)
    if "opencode_zen_r6_bun_resource_probe: test" not in listing:
        parser.error("test binary does not contain the current resource probe")
    if not 64 <= args.pressure_high_mib < args.memory_max_mib <= 16384:
        parser.error("require 64 <= pressure-high-mib < memory-max-mib <= 16384")
    if args.cgroup_parent is None:
        relative = next(line[3:] for line in Path("/proc/self/cgroup").read_text().splitlines() if line.startswith("0::"))
        args.cgroup_parent = (Path("/sys/fs/cgroup") / relative.lstrip("/")).parent
    if "memory" not in (args.cgroup_parent / "cgroup.subtree_control").read_text().split():
        parser.error("memory controller must already be delegated at the selected parent")
    args.output_dir = (args.output_dir or Path(tempfile.mkdtemp(prefix="cc-switch-zen-resources-"))).resolve()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    if any((args.output_dir / name).exists() for name in ("results.json", "normal.log", "pressure.log", "normal-samples.json", "pressure-samples.json")):
        parser.error("output directory already contains experiment files")
    env = os.environ.copy()
    for name in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY",
                 "http_proxy", "https_proxy", "all_proxy", "no_proxy"):
        env.pop(name, None)
    env.update({"RUST_MIN_STACK": "67108864", "CC_SWITCH_ZEN_TEST_BUN": str(args.bun),
                "CC_SWITCH_ZEN_MAX_CONCURRENCY": "32"})
    result = {"schemaVersion": 1, "kind": "offline_zen_resource_experiment",
              "testBinarySha256": sha256(args.binary), "runtimeVersion": manifest["version"],
              "runtimeBinarySha256": row["binarySha256"], "sampleIntervalMs": 10,
              "scope": "production lane and Bun stdio transport; host includes loopback fixture; summed RSS counts shared pages; phase boundaries are sampled; localhost has no TLS",
              "runs": []}
    for mode, high in [("normal", "max"), ("pressure", str(args.pressure_high_mib * 1024 ** 2))]:
        run = measure(args, mode, high, env)
        result["runs"].append(run)
        (args.output_dir / "results.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({key: run[key] for key in ("mode", "outcome", "probeExitCode", "probePassed", "remainingAfterCleanup", "memoryEventsAfter")}), flush=True)
    if any(run["outcome"].startswith("unexpected_") for run in result["runs"]):
        raise SystemExit(1)
    print(str(args.output_dir / "results.json"))


if __name__ == "__main__":
    main()
