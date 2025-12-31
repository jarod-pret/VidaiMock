import requests
import json
import time

BASE_URL = "http://127.0.0.1:8100"

def test_chaos_malformed():
    print("--- Testing Malformed JSON (100% chance) ---")
    try:
        r = requests.post(f"{BASE_URL}/v1/chat/completions", json={"model": "gpt-4"})
        print(f"Status: {r.status_code}")
        try:
            r.json()
            print("Response is valid JSON (unexpected if malformed should be 100%)")
        except json.JSONDecodeError:
            print("Successfully received malformed JSON!")
            return True
    except Exception as e:
        print(f"Error: {e}")
    return False

def test_chaos_trickle():
    print("--- Testing Slow Trickle (300ms between chunks) ---")
    start_time = time.time()
    try:
        # Note: We need disconnect_pct=0 for this to see multiple chunks
        r = requests.post(f"{BASE_URL}/v1/chat/completions/stream", json={"stream": True}, stream=True)
        print(f"Status: {r.status_code}")
        chunk_count = 0
        for line in r.iter_lines():
            if line:
                chunk_count += 1
                curr_elapsed = time.time() - start_time
                print(f"Received chunk {chunk_count} at {curr_elapsed:.2f}s")
        
        duration = time.time() - start_time
        print(f"Total duration: {duration:.2f}s for {chunk_count} chunks")
        if duration > 0.1: # Even 100ms is enough to prove it works
             return True
    except Exception as e:
        print(f"Error: {e}")
    return False

def test_chaos_disconnect():
    print("--- Testing Partial SSE (100% disconnect chance) ---")
    try:
        r = requests.post(f"{BASE_URL}/v1/chat/completions/stream", json={"stream": True}, stream=True)
        print(f"Status: {r.status_code}")
        received_done = False
        chunk_count = 0
        for line in r.iter_lines():
            if line:
                chunk_count += 1
                line_str = line.decode('utf-8')
                if "[DONE]" in line_str or "message_stop" in line_str:
                    received_done = True
        
        if not received_done:
             print(f"Successfully disconnected after {chunk_count} chunks (before completion)")
             return True
        else:
             print("Received completion event, disconnect failed")
    except Exception as e:
        print(f"Error: {e}")
    return False

if __name__ == "__main__":
    success = True
    if not test_chaos_malformed(): success = False
    if not test_chaos_trickle(): success = False
    if not test_chaos_disconnect(): success = False
    
    if success:
        print("\n✅ CHAOS TESTS PASSED")
    else:
        print("\n❌ SOME CHAOS TESTS FAILED")
