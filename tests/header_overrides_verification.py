import requests
import time
import json

BASE_URL = "http://127.0.0.1:8100"

def test_latency_override():
    print("--- Testing X-Vidai-Latency Overrides ---")
    
    # 1. No latency
    start = time.time()
    requests.post(f"{BASE_URL}/v1/chat/completions", json={})
    print(f"Normal request: {time.time() - start:.2f}s")
    
    # 2. 500ms override (regardless of global mode)
    start = time.time()
    requests.post(f"{BASE_URL}/v1/chat/completions", json={}, headers={"X-Vidai-Latency": "500"})
    elapsed = time.time() - start
    print(f"Override (500ms): {elapsed:.2f}s")
    if elapsed >= 0.4:
        print("✅ Latency override detected")

def test_chaos_drop_override():
    print("--- Testing X-Vidai-Chaos-Drop Overrides ---")
    
    # Force 100% drop via header
    success_count = 0
    fail_count = 0
    for _ in range(5):
        r = requests.post(f"{BASE_URL}/v1/chat/completions", json={}, headers={"X-Vidai-Chaos-Drop": "100"})
        if r.status_code == 500:
            fail_count += 1
        else:
            success_count += 1
            
    if fail_count == 5:
        print("✅ Chaos drop (100%) detected via header")
    else:
        print(f"❌ Failed to drop consistently (Fails: {fail_count}, Successes: {success_count})")

def test_chaos_malformed_override():
    print("--- Testing X-Vidai-Chaos-Malformed Overrides ---")
    
    # Force 100% malformed via header
    r = requests.post(f"{BASE_URL}/v1/chat/completions", json={}, headers={"X-Vidai-Chaos-Malformed": "100"})
    try:
        r.json()
        print("❌ Received valid JSON, expected malformed")
    except:
        print("✅ Chaos malformed (100%) detected via header")

def test_streaming_trickle_override():
    print("--- Testing X-Vidai-Chaos-Trickle Overrides ---")
    
    # Start stream with 300ms trickle
    r = requests.post(f"{BASE_URL}/v1/chat/completions/stream", json={"stream": True}, headers={"X-Vidai-Chaos-Trickle": "300"}, stream=True)
    start = time.time()
    count = 0
    for line in r.iter_lines():
        if line: count += 1
        if count >= 3: break
    
    elapsed = time.time() - start
    print(f"Streaming trickle (3 chunks): {elapsed:.2f}s")
    if elapsed >= 0.6: # Relaxed check
        print("✅ Streaming trickle override detected")

def test_streaming_disconnect_override():
    print("--- Testing X-Vidai-Chaos-Disconnect Overrides ---")
    
    # Force 100% disconnect early
    r = requests.post(f"{BASE_URL}/v1/chat/completions/stream", json={"stream": True}, headers={"X-Vidai-Chaos-Disconnect": "100"}, stream=True)
    chunks = []
    for line in r.iter_lines():
        if line: chunks.append(line)
        
    print(f"Chunks received: {len(chunks)}")
    if len(chunks) < 5: # Should disconnect almost immediately
        print("✅ Streaming disconnect override detected")

if __name__ == "__main__":
    import subprocess
    import os
    
    # Start server in benchmark mode (no latency/chaos by default)
    config_content = """
port = 3005
[latency]
mode = "benchmark"
base_ms = 0
jitter_pct = 0.0

[chaos]
enabled = false
drop_pct = 0.0
malformed_pct = 0.0
trickle_ms = 0
disconnect_pct = 0.0
"""
    with open("header-test.toml", "w") as f:
        f.write(config_content)
        
    server = subprocess.Popen(["./target/debug/vidaimock", "--config", "header-test.toml"])
    time.sleep(2)
    
    try:
        test_latency_override()
        test_chaos_drop_override()
        test_chaos_malformed_override()
        test_streaming_trickle_override()
        test_streaming_disconnect_override()
        print("\n✅ ALL HEADER OVERRIDE TESTS FINISHED")
    finally:
        server.terminate()
        os.remove("header-test.toml")
