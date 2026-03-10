#!/usr/bin/env python3
"""
generate_trace.py – Sample trace generators for bw-sequencer.

Usage:
    python3 generate_trace.py sine     > sine_wave.csv
    python3 generate_trace.py tunnel   > tunnel_fading.csv
    python3 generate_trace.py highway  > highway_5g.csv

Each generator writes a two-column CSV (time_sec, bandwidth_kbps) to stdout.
Feed the output directly to bw-sequencer:

    sudo bw-sequencer -i eth0 -c sine_wave.csv
"""

import sys
import math
import csv


def write_csv(rows: list[tuple[float, int]]) -> None:
    writer = csv.writer(sys.stdout)
    writer.writerow(["time_sec", "bandwidth_kbps"])
    for t, bw in rows:
        writer.writerow([f"{t:.4f}", bw])


# ── Generators ─────────────────────────────────────────────────────────────────

def sine_wave(
    duration_s: float = 60.0,
    interval_s: float = 0.5,
    base_kbps: int = 5_000,
    amplitude_kbps: int = 4_000,
    period_s: float = 10.0,
) -> None:
    """Smooth sinusoidal bandwidth oscillation."""
    rows: list[tuple[float, int]] = []
    t = 0.0
    while t <= duration_s:
        bw = int(base_kbps + amplitude_kbps * math.sin(2 * math.pi * t / period_s))
        bw = max(8, bw)  # never zero
        rows.append((t, bw))
        t += interval_s
    write_csv(rows)


def tunnel_fading(
    pre_s: float = 5.0,
    drop_s: float = 0.05,   # aggressive drop over 50 ms
    tunnel_s: float = 10.0,
    rise_s: float = 0.2,
    post_s: float = 5.0,
    open_kbps: int = 10_000,
    tunnel_kbps: int = 200,
) -> None:
    """
    Simulates a vehicle entering and exiting a tunnel:
      - Full bandwidth before the tunnel
      - Rapid drop (dense data points every 50 ms)
      - Low bandwidth inside
      - Rapid recovery
      - Full bandwidth after
    """
    rows: list[tuple[float, int]] = []
    t = 0.0

    # Before tunnel – sparse (1 s intervals)
    while t < pre_s:
        rows.append((t, open_kbps))
        t += 1.0

    # Entering tunnel – dense (50 ms intervals), linear drop
    steps = max(1, int(drop_s / 0.05))
    for i in range(steps + 1):
        bw = int(open_kbps + (tunnel_kbps - open_kbps) * i / steps)
        rows.append((t, bw))
        t += drop_s / steps

    # Inside tunnel – sparse (1 s intervals)
    end_tunnel = t + tunnel_s
    while t < end_tunnel:
        rows.append((t, tunnel_kbps))
        t += 1.0

    # Exiting tunnel – 200 ms recovery
    steps = max(1, int(rise_s / 0.05))
    for i in range(steps + 1):
        bw = int(tunnel_kbps + (open_kbps - tunnel_kbps) * i / steps)
        rows.append((t, bw))
        t += rise_s / steps

    # After tunnel – sparse
    end = t + post_s
    while t <= end:
        rows.append((t, open_kbps))
        t += 1.0

    write_csv(rows)


def highway_5g(
    duration_s: float = 120.0,
    seed: int = 42,
) -> None:
    """
    Realistic 5G highway trace: mix of high throughput, handover dips,
    and occasional dead zones.  Uses a simple pseudo-random walk.
    """
    import random
    rng = random.Random(seed)

    rows: list[tuple[float, int]] = []
    t = 0.0
    bw = 50_000  # start at 50 Mbps

    while t <= duration_s:
        rows.append((t, bw))

        # Random walk with mean-reversion toward 50 Mbps
        delta = rng.gauss(0, 5_000)
        bw = int(bw + delta + 0.1 * (50_000 - bw))
        bw = max(500, min(100_000, bw))  # clamp 500 kbps – 100 Mbps

        # Occasional handover: sharp 80% dip lasting 200–500 ms
        if rng.random() < 0.02:
            dip_bw = max(500, int(bw * 0.2))
            dip_dur = rng.uniform(0.2, 0.5)
            rows.append((t + 0.01, dip_bw))
            rows.append((t + dip_dur, bw))
            t += dip_dur

        t += 1.0

    write_csv(rows)


# ── Entry point ────────────────────────────────────────────────────────────────

GENERATORS = {
    "sine": sine_wave,
    "tunnel": tunnel_fading,
    "highway": highway_5g,
}

if __name__ == "__main__":
    name = sys.argv[1] if len(sys.argv) > 1 else "sine"
    if name not in GENERATORS:
        print(f"Unknown generator '{name}'. Choose from: {', '.join(GENERATORS)}", file=sys.stderr)
        sys.exit(1)
    GENERATORS[name]()
