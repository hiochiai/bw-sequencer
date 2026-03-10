#!/usr/bin/env python3
"""
verify.py – Validate iperf3 JSON results against expected bandwidth phases.

Strategy: instead of checking absolute interval numbers (fragile due to
start-time uncertainty), we use a sliding-window detector to find the
longest contiguous stretch of intervals that fall within each expected range.
We then assert that:
  1. A sustained low-bandwidth phase  (~5 Mbps)  of at least MIN_PHASE_SECS exists.
  2. A sustained high-bandwidth phase (~100 Mbps) of at least MIN_PHASE_SECS exists.
  3. The low phase comes before the high phase.

Tolerances are intentionally wide (±70 %) to accommodate Docker bridge overhead.

Usage:
    python3 verify.py <iperf3_results.json>
Exit 0 on pass, 1 on failure.
"""

import json
import re
import sys

# ── Configuration ─────────────────────────────────────────────────────────────

LOW_MIN_MBPS   = 1.0    # lower bound for "5 Mbps" phase
LOW_MAX_MBPS   = 15.0   # upper bound

HIGH_MIN_MBPS  = 40.0   # lower bound for "100 Mbps" phase (Docker limits vary)

MIN_PHASE_SECS = 10     # minimum contiguous seconds required per phase


# ── Helpers ───────────────────────────────────────────────────────────────────

def parse_server_text(text: str) -> list[float]:
    """Parse per-interval throughput (Mbps) from iperf3 server text output."""
    # Matches lines like: [  5]   1.00-2.00   sec   625 KBytes  5.12 Mbits/sec
    pattern = re.compile(
        r'\[\s*\d+\]\s+([\d.]+)\s*-\s*([\d.]+)\s+sec\s+[\d.]+\s+\w+Bytes\s+([\d.]+)\s+(K|M|G)?bits/sec'
    )
    multipliers = {"K": 1e-3, "M": 1.0, "G": 1e3}
    results = []
    for line in text.splitlines():
        m = pattern.search(line)
        if m:
            t_start, t_end = float(m.group(1)), float(m.group(2))
            if t_end - t_start < 2.0:  # skip summary line spanning full test duration
                val = float(m.group(3))
                prefix = m.group(4)
                results.append(val * multipliers.get(prefix, 1.0) if prefix else val / 1e6)
    return results


def longest_run(values: list[float], lo: float, hi: float) -> tuple[int, int]:
    """
    Return (start_idx, length) of the longest contiguous sub-sequence where
    lo <= v <= hi.  Returns (-1, 0) if none found.
    """
    best_start, best_len = -1, 0
    run_start, run_len   = 0,  0

    for i, v in enumerate(values):
        if lo <= v <= hi:
            if run_len == 0:
                run_start = i
            run_len += 1
            if run_len > best_len:
                best_start, best_len = run_start, run_len
        else:
            run_len = 0

    return best_start, best_len


# ── Main ──────────────────────────────────────────────────────────────────────

def main() -> int:
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <iperf3_results.json>", file=sys.stderr)
        return 1

    with open(sys.argv[1]) as fh:
        data = json.load(fh)

    if "error" in data:
        print(f"FAIL: iperf3 error: {data['error']}", file=sys.stderr)
        return 1

    # Prefer server-side received-rate measurements (actual network throughput)
    # over client-side sender stats (which reflect socket-buffer write rate and
    # can stay artificially high while the kernel slowly drains a full send buffer).
    server_out = data.get("server_output_json", {})
    server_text = data.get("server_output_text", "")
    if server_out and "intervals" in server_out:
        # Newer iperf3 versions return server output as JSON
        intervals = server_out["intervals"]
        mbps = [iv["sum"]["bits_per_second"] / 1e6 for iv in intervals]
    elif server_text:
        # iperf3 3.12 returns server output as human-readable text
        mbps = parse_server_text(server_text)
    else:
        intervals = data.get("intervals", [])
        mbps = [iv["sum"]["bits_per_second"] / 1e6 for iv in intervals]
    total = len(mbps)
    print(f"[verify] {total} intervals received")

    if total < MIN_PHASE_SECS * 2:
        print(f"FAIL: too few intervals ({total}); expected at least {MIN_PHASE_SECS * 2}")
        return 1

    # Detect low-bandwidth phase
    low_start, low_len = longest_run(mbps, LOW_MIN_MBPS, LOW_MAX_MBPS)
    print(
        f"[verify] Low phase:  longest run = {low_len}s "
        f"(intervals {low_start}–{low_start + low_len - 1})"
        if low_len > 0 else "[verify] Low phase:  not found"
    )

    # Detect high-bandwidth phase
    high_start, high_len = longest_run(mbps, HIGH_MIN_MBPS, float("inf"))
    print(
        f"[verify] High phase: longest run = {high_len}s "
        f"(intervals {high_start}–{high_start + high_len - 1})"
        if high_len > 0 else "[verify] High phase: not found"
    )

    failures: list[str] = []

    if low_len < MIN_PHASE_SECS:
        failures.append(
            f"Low-bandwidth phase too short: {low_len}s < {MIN_PHASE_SECS}s "
            f"(expected ~5 Mbps for at least {MIN_PHASE_SECS}s)"
        )

    if high_len < MIN_PHASE_SECS:
        failures.append(
            f"High-bandwidth phase too short: {high_len}s < {MIN_PHASE_SECS}s "
            f"(expected ≥{HIGH_MIN_MBPS} Mbps for at least {MIN_PHASE_SECS}s)"
        )

    if low_len >= MIN_PHASE_SECS and high_len >= MIN_PHASE_SECS:
        if low_start >= high_start:
            failures.append(
                f"Phase order wrong: low phase starts at interval {low_start} "
                f"but high phase starts at interval {high_start} "
                f"(low must precede high)"
            )

    if failures:
        print("\nFAIL:")
        for msg in failures:
            print(f"  {msg}")
        return 1

    print("\nPASS – both bandwidth phases verified in correct order.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
