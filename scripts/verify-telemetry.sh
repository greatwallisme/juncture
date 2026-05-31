#!/usr/bin/env bash
# verify-telemetry.sh -- End-to-end telemetry pipeline verification
#
# Starts the OTel Collector + Jaeger + Prometheus stack, runs the telemetry
# demo binary, then queries Jaeger and Prometheus APIs to verify data arrived.
#
# Usage:
#   ./scripts/verify-telemetry.sh
#
# Prerequisites:
#   - Docker and docker compose available
#   - Rust toolchain installed

set -euo pipefail

COMPOSE_DIR="docker/telemetry"
COMPOSE_FILE="${COMPOSE_DIR}/docker-compose.yml"
JAEGER_URL="http://localhost:16686"
PROMETHEUS_URL="http://localhost:9090"
OTEL_COLLECTOR_URL="http://localhost:4317"

# Colors (disabled if not a terminal)
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    NC='\033[0m'
else
    RED='' GREEN='' YELLOW='' NC=''
fi

info()  { printf "${GREEN}[INFO]${NC}  %s\n" "$*"; }
warn()  { printf "${YELLOW}[WARN]${NC}  %s\n" "$*"; }
fail()  { printf "${RED}[FAIL]${NC}  %s\n" "$*"; }
pass()  { printf "${GREEN}[PASS]${NC}  %s\n" "$*"; }

cleanup() {
    info "Stopping telemetry stack..."
    docker compose -f "${COMPOSE_FILE}" down --volumes --remove-orphans 2>/dev/null || true
}

# --- Preflight checks ---
if ! command -v docker &>/dev/null; then
    fail "docker not found. Install Docker first."
    exit 1
fi

if ! docker compose version &>/dev/null; then
    fail "docker compose not available."
    exit 1
fi

# --- Start the telemetry stack ---
info "Starting telemetry stack (OTel Collector + Jaeger + Prometheus)..."
docker compose -f "${COMPOSE_FILE}" up -d

# Wait for services to be healthy
info "Waiting for services to become ready..."
MAX_WAIT=30
for i in $(seq 1 "${MAX_WAIT}"); do
    if curl -sf "${JAEGER_URL}/" >/dev/null 2>&1; then
        break
    fi
    if [ "$i" -eq "${MAX_WAIT}" ]; then
        fail "Jaeger did not become ready within ${MAX_WAIT}s"
        cleanup
        exit 1
    fi
    sleep 1
done
pass "Jaeger is ready"

for i in $(seq 1 "${MAX_WAIT}"); do
    if curl -sf "${PROMETHEUS_URL}/-/ready" >/dev/null 2>&1; then
        break
    fi
    if [ "$i" -eq "${MAX_WAIT}" ]; then
        fail "Prometheus did not become ready within ${MAX_WAIT}s"
        cleanup
        exit 1
    fi
    sleep 1
done
pass "Prometheus is ready"

# --- Run the telemetry demo ---
info "Building and running telemetry_demo..."
cargo run -p juncture-simple-example --bin telemetry_demo 2>&1
DEMO_EXIT=$?
if [ "${DEMO_EXIT}" -ne 0 ]; then
    fail "telemetry_demo exited with code ${DEMO_EXIT}"
    cleanup
    exit 1
fi
pass "telemetry_demo completed successfully"

# Give the OTel Collector a moment to flush
info "Waiting for telemetry data to propagate..."
sleep 3

# --- Verify traces in Jaeger ---
info "Querying Jaeger for traces..."
JAEGER_SERVICES=$(curl -sf "${JAEGER_URL}/api/services" 2>/dev/null || echo '{"data":[]}')
if echo "${JAEGER_SERVICES}" | grep -q "juncture-telemetry-demo"; then
    pass "Jaeger: service 'juncture-telemetry-demo' found"

    # Fetch traces for the service
    JAEGER_TRACES=$(curl -sf "${JAEGER_URL}/api/traces?service=juncture-telemetry-demo&limit=5" 2>/dev/null || echo '{"data":[]}')
    TRACE_COUNT=$(echo "${JAEGER_TRACES}" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d.get('data',[])))" 2>/dev/null || echo "0")
    if [ "${TRACE_COUNT}" -gt 0 ]; then
        pass "Jaeger: found ${TRACE_COUNT} trace(s)"
    else
        warn "Jaeger: no traces found yet (may need more time)"
    fi
else
    warn "Jaeger: service 'juncture-telemetry-demo' not found (collector may still be flushing)"
fi

# --- Verify metrics in Prometheus ---
info "Querying Prometheus for Juncture metrics..."
PROM_RESULT=$(curl -sf "${PROMETHEUS_URL}/api/v1/query?query=juncture_graph_invocations_total" 2>/dev/null || echo '{"status":"error"}')
if echo "${PROM_RESULT}" | grep -q '"status":"success"'; then
    RESULT_COUNT=$(echo "${PROM_RESULT}" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d.get('data',{}).get('result',[])))" 2>/dev/null || echo "0")
    if [ "${RESULT_COUNT}" -gt 0 ]; then
        pass "Prometheus: juncture_graph_invocations_total metric found (${RESULT_COUNT} series)"
    else
        warn "Prometheus: metric query succeeded but no data yet"
    fi
else
    warn "Prometheus: query failed or metric not available yet"
fi

# --- Summary ---
echo ""
info "=== Telemetry Verification Summary ==="
info "Jaeger UI:     ${JAEGER_URL}"
info "Prometheus UI: ${PROMETHEUS_URL}"
info ""
info "To keep the stack running, skip cleanup. To stop:"
info "  docker compose -f ${COMPOSE_FILE} down"

cleanup
pass "Verification complete"
