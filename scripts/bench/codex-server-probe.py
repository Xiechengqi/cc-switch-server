#!/usr/bin/env python3
"""Measure production Rust HTTP/WS handlers using synthetic loopback traffic.

This explicitly runs one ignored test in a separate cargo test process. It
never uses real credentials, changes production endpoints, or runs UI or
deployment tests. The test process contains Server, fake upstream and clients;
its RSS must not be described as an isolated production Server RSS.
"""
import argparse
import hashlib
import importlib.util
import json
import os
import platform
from pathlib import Path
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=101)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--background-load", default="unspecified shared host load")
    args = parser.parse_args()
    if not 3 <= args.samples <= 1000:
        parser.error("samples must be between 3 and 1000")
    if platform.system() != "Linux":
        parser.error("the opt-in RSS sampler requires Linux /proc")
    root = Path(__file__).resolve().parents[2]
    specification = importlib.util.spec_from_file_location(
        "codec_probe", root / "scripts/bench/codex-request-compression.py")
    codec_probe = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(codec_probe)
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)

    def rust_sources():
        paths = sorted(root.glob("src/**/*.rs")) + [root / name for name in ("build.rs", "Cargo.toml", "Cargo.lock")]
        return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths}

    frozen = rust_sources()
    probe_paths = [Path(__file__).resolve(), root / "src/test_support/codex_server_probe.rs",
                   root / "scripts/bench/codex-request-compression.py"]
    probe_sources = {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
                     for path in probe_paths}
    with tempfile.TemporaryDirectory(prefix="cc-switch-codex-server-probe-inputs-") as directory:
        fixtures = {}
        for case in codec_probe.CASES:
            body = codec_probe.payload(case)
            (Path(directory) / f"{case}.json").write_bytes(body)
            fixtures[case] = {"sha256": hashlib.sha256(body).hexdigest(), "bytes": len(body)}
        env = os.environ.copy()
        env.pop("CC_SWITCH_OUTBOUND_PROXY", None)
        env["CC_SWITCH_PROCESS_MEMORY_BUDGET_MB"] = "0"
        env.update(CC_SWITCH_CODEX_PROBE_INPUT_DIR=directory,
                   CC_SWITCH_CODEX_PROBE_OUTPUT=str(output),
                   CC_SWITCH_CODEX_PROBE_SAMPLES=str(args.samples))
        env.setdefault("RUST_MIN_STACK", "67108864")
        subprocess.run(["cargo", "test", "--lib",
                        "test_support::codex_server_probe::codex_server_loopback_measurement",
                        "--", "--exact", "--ignored", "--nocapture"],
                       cwd=root, env=env, check=True)
    report = json.loads(output.read_text())
    if rust_sources() != frozen:
        raise RuntimeError("Rust sources changed during the probe; exclude this run")
    if probe_sources != {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
                         for path in probe_paths}:
        raise RuntimeError("Probe sources changed during the measurement; exclude this run")
    report["frozenRustSourcesUnchanged"] = True
    report["rustSourceFileCount"] = len(frozen)
    report["rustSourceTreeSha256"] = hashlib.sha256(
        json.dumps(frozen, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    report["fixtureInputs"] = fixtures
    for row in report["results"]:
        row["preparedInputBytes"] = row.pop("clientBodyBytes")
        row["preparedInputSha256"] = row.pop("inputSha256")
    report["applicationByteScope"] = (
        "prepared input includes the stream selection but precedes the WS response.create envelope; "
        "upstreamApplicationRequestBytes counts the actual normalized application payloads received "
        "by the fake upstream; both exclude transport framing, headers and TLS")
    report["evidenceState"] = "offline_loopback_test_assembly_measurement"
    report["targetState"] = "working_tree"
    report["targetHeadCommit"] = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    report["environment"] = {"system": platform.system(), "architecture": platform.machine(),
                             "cpuCount": os.cpu_count(), "backgroundLoad": args.background_load,
                             "rustMinStackBytes": int(env["RUST_MIN_STACK"]),
                             "cargoBuildJobs": env.get("CARGO_BUILD_JOBS"),
                             "processMemoryBudgetMb": env.get("CC_SWITCH_PROCESS_MEMORY_BUDGET_MB"),
                             "explicitOutboundProxyDisabled": "CC_SWITCH_OUTBOUND_PROXY" not in env,
                             "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip()}
    report["inputDirectoryRemoved"] = not Path(directory).exists()
    report["sourceSha256"] = probe_sources
    report["probeSourcesUnchanged"] = True
    output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"output": str(output), "rows": len(report["results"]),
                      "maxSamplesPerRow": args.samples, "listenersClosed": report["listenersClosed"],
                      "inputDirectoryRemoved": report["inputDirectoryRemoved"]}))


if __name__ == "__main__":
    main()
