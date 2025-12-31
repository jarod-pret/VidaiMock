
import requests
import time
import json

print("Testing Anthropic Streaming Latency...")
url = "http://localhost:8100/v1/messages"
payload = {
    "model": "claude-3-opus-20240229",
    "messages": [{"role": "user", "content": "Hello"}],
    "stream": True,
    "max_tokens": 100
}

start_time = time.time()
resp = requests.post(url, json=payload, stream=True)
ttft_done = False
ttft = 0

byte_count = 0
for line in resp.iter_lines():
    if not line: continue
    if not ttft_done:
        ttft = (time.time() - start_time) * 1000
        ttft_done = True
    byte_count += len(line)

total_time = (time.time() - start_time) * 1000

print(f"TTFT: {ttft:.2f} ms")
print(f"Total Time: {total_time:.2f} ms")
print(f"Total Bytes: {byte_count}")

if total_time > 500:
    print("WARNING: High Latency (>500ms)")
else:
    print("SUCCESS: Latency acceptable")
