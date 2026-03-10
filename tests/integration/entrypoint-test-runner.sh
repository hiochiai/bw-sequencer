#!/bin/sh
set -e

IPERF_SERVER="10.0.2.10"
WARMUP_SECS="${WARMUP_SECS:-12}"
IPERF_DURATION="${IPERF_DURATION:-70}"
RESULTS="/tmp/iperf_results.json"

# Route to the server-net via the throttler gateway
ip route add 10.0.2.0/24 via 10.0.1.1 2>/dev/null || true

echo "[test-runner] Waiting ${WARMUP_SECS}s for throttler warmup..."
sleep "${WARMUP_SECS}"

echo "[test-runner] Checking connectivity to iperf server..."
ping -c 3 -W 2 "${IPERF_SERVER}" || {
    echo "[test-runner] ERROR: cannot reach ${IPERF_SERVER}" >&2
    exit 1
}

echo "[test-runner] Starting iperf3 (${IPERF_DURATION}s, 1-second intervals)..."
iperf3 \
    --client          "${IPERF_SERVER}" \
    --time            "${IPERF_DURATION}" \
    --interval        1 \
    --json \
    --get-server-output \
    > "${RESULTS}"

echo "[test-runner] Verifying bandwidth results..."
python3 /verify.py "${RESULTS}"
