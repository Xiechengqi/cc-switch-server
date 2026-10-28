#!/usr/bin/env python3
"""Offline codec measurements; never starts a server or sends a request.

Uses the host libzstd, not an external proxy checkout or the Rust server's
encoder. RSS belongs to each fresh Python worker, not to cc-switch-server.
"""
import argparse
import base64
import ctypes
import ctypes.util
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import resource
import subprocess
import sys
import time


CASES = ("short", "long_history", "tool_history", "base64_entropy")


def payload(case):
    body = {"model": "gpt-5.4-mini", "stream": True, "input": "say OK"}
    if case == "long_history":
        body["input"] = [
            {"role": "user" if index % 2 == 0 else "assistant",
             "content": ("Local fixture: 仅用于压缩测量，不会发送。\n" * 180)
             + hashlib.sha256(str(index).encode()).hexdigest()}
            for index in range(96)
        ]
    elif case == "tool_history":
        body["input"] = []
        for index in range(128):
            call_id = f"fixture-{index}"
            body["input"].extend([
                {"type": "function_call", "call_id": call_id, "name": "read_fixture",
                 "arguments": json.dumps({"path": f"fixture/{index}.txt"})},
                {"type": "function_call_output", "call_id": call_id,
                 "output": json.dumps({"rows": [
                     {"index": row, "text": "const fixture = 'offline';",
                      "digest": hashlib.sha256(f"{index}:{row}".encode()).hexdigest()}
                     for row in range(80)]})},
            ])
    elif case == "base64_entropy":
        # An opaque high-entropy string, not an image or vendor-accepted input.
        rng = random.Random(42)
        body["input"] = base64.b64encode(rng.randbytes(768 * 1024)).decode()
    return json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode()


def percentile(values, fraction):
    values = sorted(values)
    return round(values[min(len(values) - 1, int((len(values) - 1) * fraction))], 6)


def worker(case, samples):
    library = ctypes.util.find_library("zstd")
    if not library:
        raise RuntimeError("optional probe requires the host libzstd")
    codec = ctypes.CDLL(library)
    codec.ZSTD_compressBound.argtypes = [ctypes.c_size_t]
    codec.ZSTD_compressBound.restype = ctypes.c_size_t
    codec.ZSTD_compress.argtypes = [ctypes.c_void_p, ctypes.c_size_t,
                                   ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
    codec.ZSTD_compress.restype = ctypes.c_size_t
    codec.ZSTD_decompress.argtypes = [ctypes.c_void_p, ctypes.c_size_t,
                                     ctypes.c_void_p, ctypes.c_size_t]
    codec.ZSTD_decompress.restype = ctypes.c_size_t
    codec.ZSTD_isError.argtypes = [ctypes.c_size_t]
    codec.ZSTD_isError.restype = ctypes.c_uint
    codec.ZSTD_versionString.restype = ctypes.c_char_p
    raw = payload(case)
    rss_before = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    source = ctypes.create_string_buffer(raw)
    destination = ctypes.create_string_buffer(codec.ZSTD_compressBound(len(raw)))
    cpu, wall = [], []
    size = None
    for _ in range(samples):
        cpu_start = time.process_time_ns()
        wall_start = time.perf_counter_ns()
        result = codec.ZSTD_compress(destination, len(destination), source, len(raw), 3)
        wall.append((time.perf_counter_ns() - wall_start) / 1e6)
        cpu.append((time.process_time_ns() - cpu_start) / 1e6)
        if codec.ZSTD_isError(result):
            raise RuntimeError("zstd encoding failed")
        if size is not None and size != result:
            raise RuntimeError("non-deterministic compressed size")
        size = result
    restored = ctypes.create_string_buffer(len(raw))
    decoded = codec.ZSTD_decompress(restored, len(restored), destination, size)
    if codec.ZSTD_isError(decoded) or decoded != len(raw) or restored.raw != raw:
        raise RuntimeError("codec round trip changed fixture bytes")
    rss_after = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    rss_scale = 1 if platform.system() == "Darwin" else 1024
    return {
        "case": case, "inputSha256": hashlib.sha256(raw).hexdigest(),
        "plainBodyBytes": len(raw), "zstdBodyBytes": size,
        "bodyBytesSavedPercent": round((1 - size / len(raw)) * 100, 4),
        "samples": samples, "level": 3, "encoder": "one_shot_ZSTD_compress",
        "libzstd": codec.ZSTD_versionString().decode(),
        "encodingCpuMs": {f"p{int(p * 100)}": percentile(cpu, p)
                          for p in (0.5, 0.95, 0.99)},
        "encodingWallMs": {f"p{int(p * 100)}": percentile(wall, p)
                           for p in (0.5, 0.95, 0.99)},
        "workerPeakRssBytes": rss_after * rss_scale,
        "workerPeakRssIncreaseBytes": max(0, rss_after - rss_before) * rss_scale,
        "roundTripExact": True,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=31)
    parser.add_argument("--background-load", default="unspecified")
    parser.add_argument("--worker", choices=CASES, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not 3 <= args.samples <= 1000:
        parser.error("samples must be between 3 and 1000")
    if args.worker:
        print(json.dumps(worker(args.worker, args.samples)))
        return
    results = [json.loads(subprocess.check_output([
        sys.executable, str(Path(__file__).resolve()), "--worker", case,
        "--samples", str(args.samples)], text=True)) for case in CASES]
    print(json.dumps({
        "format": "cc-switch-codex-compression-codec-probe", "schemaVersion": 1,
        "capturedAt": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "evidenceState": "offline_synthetic_codec_measurement",
        "scriptSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "environment": {"python": platform.python_version(), "system": platform.system(),
                        "architecture": platform.machine(), "cpuCount": os.cpu_count(),
                        "backgroundLoad": args.background_load},
        "networkRequests": 0, "serverStarted": False,
        "limits": ["body_bytes_only_excludes_transport_headers_and_TLS",
                   "host_libzstd_is_not_the_Rust_server_build",
                   "worker_RSS_is_not_server_RSS_or_request_budget",
                   "does_not_measure_TTFT_cancellation_or_upstream_acceptance",
                   "not_a_production_performance_threshold"],
        "results": results,
        "decision": "keep_current_outbound_encoding_pending_transport_and_endpoint_evidence",
    }, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
