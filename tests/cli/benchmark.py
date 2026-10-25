#!/usr/bin/env python3
"""Compare release CLI binaries against one disposable populated cache (R-2/R-10).

This runs offline reads, checks byte-identical outputs, and alternates binary order.
Use an isolated cache copy with its snapshot roots available; no refresh is run.
"""

import argparse
import hashlib
import json
import re
import statistics
import subprocess
import time
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("binaries", nargs="+")
p.add_argument("--config", type=Path, required=True)
p.add_argument("--cache-dir", type=Path, required=True)
p.add_argument("--library", required=True)
p.add_argument("--query", required=True)
p.add_argument("--code-query", required=True)
p.add_argument("--rounds", type=int, default=9)
p.add_argument("--output", type=Path, required=True)
a = p.parse_args()
if a.rounds < 1:
    p.error("--rounds must be positive")
a.binaries = [str(Path(b).resolve()) for b in a.binaries]
a.config = a.config.resolve()
a.cache_dir = a.cache_dir.resolve()
if not a.config.is_file() or not (a.cache_dir / "index.sqlite3").is_file():
    p.error("--config and --cache-dir must identify an existing populated cache")
commands = {
    "path": ["path", a.library],
    "versions": ["versions", "--json"],
    "sources": ["sources", "--json"],
    "search_library": ["search", a.query, "--library", a.library, "--json"],
    "search_all": ["search", a.query, "--json"],
    "search_diagnostics": [
        "search",
        a.query,
        "--library",
        a.library,
        "--diagnostics",
        "--json",
    ],
    "search_all_diagnostics": ["search", a.query, "--diagnostics", "--json"],
    "search_missing": [
        "search",
        "rtfm_benchmark_absent_8642",
        "--diagnostics",
        "--json",
    ],
    "code_literal": ["search-code", a.code_query, "--library", a.library, "--json"],
    "code_missing": [
        "search-code",
        "rtfm_benchmark_absent_8642",
        "--library",
        a.library,
        "--json",
    ],
    "code_regex": [
        "search-code",
        re.escape(a.code_query),
        "--library",
        a.library,
        "--regex",
        "--json",
    ],
    "status": ["status", "--json"],
}
scope = ["--config", str(a.config), "--cache-dir", str(a.cache_dir)]


def run(binary, command):
    return subprocess.check_output([binary, *scope, *command], timeout=60)


outputs = {}
results = {binary: {} for binary in a.binaries}
for name, command in commands.items():
    for binary in a.binaries:
        out = run(binary, command)
        if name in outputs:
            assert out == outputs[name], (name, "outputs changed")
        outputs[name] = out
    timings = {binary: [] for binary in a.binaries}
    for iteration in range(a.rounds):
        binaries = a.binaries if iteration % 2 == 0 else list(reversed(a.binaries))
        for binary in binaries:
            start = time.perf_counter_ns()
            out = run(binary, command)
            timings[binary].append((time.perf_counter_ns() - start) / 1e6)
            assert out == outputs[name], (name, "unstable output")
    for binary, values in timings.items():
        results[binary][name] = {
            "median_ms": statistics.median(values),
            "samples_ms": values,
            "output_sha256": hashlib.sha256(outputs[name]).hexdigest(),
        }
    print(
        name,
        {b: round(statistics.median(v), 3) for b, v in timings.items()},
        flush=True,
    )
a.output.write_text(json.dumps(results, indent=2) + "\n")
