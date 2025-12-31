#!/bin/bash
set -e

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo "Building release binary..."
# Navigate to root to build
cd "$(dirname "$0")/.."
cargo build --release --quiet
# Stay in root or go to scripts? 
# The script runs server which expects presets/ in CWD usually.
# Let's run from root.

SERVER_BIN="./target/release/vidaimock"
PORT=9333

function cleanup {
  if [ ! -z "$PID" ]; then
    kill $PID 2>/dev/null || true
  fi
  rm -f custom_resp.json temp_config.toml
}
trap cleanup EXIT

echo -e "\n${GREEN}=== Test 1: Custom Port & Echo Endpoint ===${NC}"
$SERVER_BIN --port $PORT --endpoints /echo --format echo &
PID=$!
sleep 2

RESPONSE=$(curl -s -X POST http://localhost:$PORT/echo -d '{"hello": "world"}')
echo "Response: $RESPONSE"
if [[ "$RESPONSE" == *'"hello": "world"'* ]]; then
  echo -e "${GREEN}PASS${NC}"
else
  echo -e "${RED}FAIL${NC}"
  exit 1
fi

kill $PID
wait $PID 2>/dev/null || true

echo -e "\n${GREEN}=== Test 2: Custom Response File ===${NC}"
echo '{"custom_cli": "success"}' > custom_resp.json
$SERVER_BIN --port $PORT --response-file custom_resp.json &
PID=$!
sleep 2

RESPONSE=$(curl -s -X POST http://localhost:$PORT/v1/chat/completions -d '{}')
echo "Response: $RESPONSE"
if [[ "$RESPONSE" == *'"custom_cli": "success"'* ]]; then
  echo -e "${GREEN}PASS${NC}"
else
  echo -e "${RED}FAIL${NC}"
  exit 1
fi

kill $PID
wait $PID 2>/dev/null || true

echo -e "\n${GREEN}=== Test 3: Multiple Custom Endpoints ===${NC}"
$SERVER_BIN --port $PORT --endpoints /api/openai,/api/anthropic --format openai &
PID=$!
sleep 2

echo "Testing /api/openai..."
RESP1=$(curl -s -X POST http://localhost:$PORT/api/openai -d '{}')
if [[ "$RESP1" == *"chat.completion"* ]]; then
  echo -e "/api/openai: ${GREEN}PASS${NC}"
else
  echo -e "/api/openai: ${RED}FAIL${NC}"
  exit 1
fi

echo "Testing /api/anthropic..."
RESP2=$(curl -s -X POST http://localhost:$PORT/api/anthropic -d '{}')
# Note: In the current CLI implementation, if --format is passed, it applies to ALL endpoints in --endpoints.
# So we expect openai format for both.
if [[ "$RESP2" == *"chat.completion"* ]]; then
  echo -e "/api/anthropic: ${GREEN}PASS${NC}"
else
  echo -e "/api/anthropic: ${RED}FAIL${NC}"
  exit 1
fi

echo -e "\n${GREEN}=== Test 4: Configurable Content-Type ===${NC}"
cat > temp_config.toml <<TOML
port = 9223
[[endpoints]]
path = "/v1/xml"
format = "openai"
content_type = "application/xml"
TOML

$SERVER_BIN --config temp_config.toml &
PID=$!
sleep 2

CTYPE=$(curl -s -v -X POST http://localhost:9223/v1/xml 2>&1 | grep "< content-type:" | tr -d '\r')
echo "Content-Type Header: $CTYPE"

if [[ "$CTYPE" == *"application/xml"* ]]; then
  echo -e "${GREEN}PASS${NC}"
else
  echo -e "${RED}FAIL${NC}"
  exit 1
fi

kill $PID
wait $PID 2>/dev/null || true

echo -e "\n${GREEN}=== Test 5: CLI Content-Type Flag ===${NC}"
$SERVER_BIN --port 9224 --endpoints /cli/xml --format openai --content-type application/xml &
PID=$!
sleep 2

CTYPE=$(curl -s -v -X POST http://localhost:9224/cli/xml 2>&1 | grep "< content-type:" | tr -d '\r')
echo "Content-Type Header: $CTYPE"

if [[ "$CTYPE" == *"application/xml"* ]]; then
  echo -e "${GREEN}PASS${NC}"
else
  echo -e "${RED}FAIL${NC}"
  kill $PID
  exit 1
fi

kill $PID
wait $PID 2>/dev/null || true

