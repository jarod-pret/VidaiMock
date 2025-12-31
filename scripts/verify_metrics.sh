#!/bin/bash
set -e

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo "Building release binary..."
cd "$(dirname "$0")/.."
cargo build --release --quiet

SERVER_BIN="./target/release/vidaimock"
PORT=9444

function cleanup {
  if [ ! -z "$PID" ]; then
    kill $PID 2>/dev/null || true
  fi
}
trap cleanup EXIT

echo -e "\n${GREEN}=== Testing Metrics Endpoint ===${NC}"
MOCK_SERVER_LOG_LEVEL=error $SERVER_BIN --port $PORT &
PID=$!
sleep 2

# Make some requests
curl -s -X POST http://localhost:$PORT/v1/chat/completions > /dev/null
curl -s -X POST http://localhost:$PORT/v1/chat/completions > /dev/null
curl -s -X POST http://localhost:$PORT/v1/chat/completions > /dev/null

# Check metrics
METRICS=$(curl -s http://localhost:$PORT/metrics)
echo "$METRICS"

if [[ "$METRICS" == *"http_requests_total"* ]]; then
  echo -e "\n${GREEN}PASS: Found http_requests_total${NC}"
else
  echo -e "\n${RED}FAIL: Missing http_requests_total${NC}"
  exit 1
fi

if [[ "$METRICS" == *"http_request_duration_seconds"* ]]; then
  echo -e "${GREEN}PASS: Found http_request_duration_seconds${NC}"
else
  echo -e "${RED}FAIL: Missing http_request_duration_seconds${NC}"
  exit 1
fi
