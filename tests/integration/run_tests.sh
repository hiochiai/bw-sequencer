#!/usr/bin/env bash
# run_tests.sh – Run the bw-sequencer integration test suite.
#
# Works with Docker Compose v2 (docker compose) or v1 (docker-compose).
#
# Usage:
#   cd tests/integration && ./run_tests.sh
#   ./tests/integration/run_tests.sh   # from repo root

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${SCRIPT_DIR}"

# Detect docker compose command (v2 plugin preferred, v1 binary fallback)
if docker compose version &>/dev/null 2>&1; then
    COMPOSE="docker compose"
elif command -v docker-compose &>/dev/null; then
    COMPOSE="docker-compose"
else
    echo "ERROR: neither 'docker compose' (v2) nor 'docker-compose' (v1) found." >&2
    exit 1
fi
echo "Using: ${COMPOSE}"

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    ${COMPOSE} down --volumes --remove-orphans 2>/dev/null || true
}
trap cleanup EXIT

echo "=== bw-sequencer integration tests ==="

# Explicitly load only docker-compose.yml so that docker-compose.override.yml
# (which requires an external named volume for local trace injection) is not
# picked up automatically by Docker Compose.
COMPOSE="${COMPOSE} -f docker-compose.yml"

${COMPOSE} build --parallel

${COMPOSE} up --detach throttler iperf-server

echo "Waiting for throttler to become healthy (TC hierarchy ready)..."
${COMPOSE} up \
    --abort-on-container-exit \
    --exit-code-from test-runner \
    test-runner

EXIT_CODE=$?
if [ "${EXIT_CODE}" -eq 0 ]; then
    echo ""
    echo "PASS: Integration tests passed."
else
    echo ""
    echo "FAIL: Integration tests failed (exit ${EXIT_CODE})."
    echo "--- throttler logs ---"
    ${COMPOSE} logs throttler
    echo "--- test-runner logs ---"
    ${COMPOSE} logs test-runner
fi
exit "${EXIT_CODE}"
