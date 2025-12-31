#!/bin/bash
set -e

# Run from root
cd "$(dirname "$0")/.."

echo "Building release binary..."
cargo build --release --quiet

SERVER_BIN="./target/release/vidaimock"

# Create a non-JSON preset
echo "Just plain text, not JSON" > presets/plaintext.json

# Start server in background
"$SERVER_BIN" --port 8100 --endpoints /v1/text --format plaintext &
SERVER_PID=$!
sleep 2

# Test it
echo "Testing non-JSON response..."
curl -v -X POST http://localhost:8100/v1/text

# Cleanup
kill $SERVER_PID
rm presets/plaintext.json
