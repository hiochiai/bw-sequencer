#!/bin/sh
set -e

# Identify the interface that reaches the server-net (10.0.2.0/24).
# Docker assigns interface names non-deterministically, so we look it up.
IFACE=$(ip route show 10.0.2.0/24 2>/dev/null | awk '{print $3; exit}')
if [ -z "$IFACE" ]; then
    echo "[throttler] ERROR: cannot find interface for 10.0.2.0/24" >&2
    ip route
    exit 1
fi

echo "[throttler] Shaping interface: ${IFACE}"
echo "[throttler] Starting bw-sequencer with step_trace.csv"
exec bw-sequencer \
    --interface "${IFACE}" \
    --csv /traces/step_trace.csv \
    --ceil-kbps 1000000
