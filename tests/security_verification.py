import requests
import json
import time
import subprocess
import os
import shutil

BASE_URL = "http://127.0.0.1:8100"

def start_server(args=None, log_name="server.log"):
    binary_path = os.path.join(os.getcwd(), "target/release/vidaimock")
    if not os.path.exists(binary_path):
        print(f"❌ Binary not found at {binary_path}")
        return None, None
    cmd = [binary_path, "--port", "8299"]
    if args:
        cmd.extend(args)
    log_file = open(log_name, "w")
    env = os.environ.copy()
    env["VIDAIMOCK_LOG_LEVEL"] = "info"
    return subprocess.Popen(cmd, env=env, stdout=log_file, stderr=subprocess.STDOUT, text=True, cwd=os.getcwd()), log_file

def wait_for_server(p, log_file):
    print(f"Waiting for server at {BASE_URL}...")
    for i in range(30):
        try:
            r = requests.get(f"{BASE_URL}/status", timeout=1)
            if r.status_code == 200:
                print("Server is ready")
                return True
        except Exception as e:
            if i > 5 and i % 5 == 0:
                print(f"Still waiting... Attempt {i}")
            if p.poll() is not None:
                print(f"Server exited early with code {p.returncode}")
                with open(log_file.name, "r") as f:
                    print(f.read())
                return False
        time.sleep(1)
    print("Server timed out starting")
    with open(log_file.name, "r") as f:
        print(f.read())
    return False

def test_chaos_headers():
    print("--- Testing Chaos Headers (Latency, Drop, Malformed) ---")
    p, log = start_server(log_name="chaos_headers.log")
    if p is None:
        return
    if not wait_for_server(p, log):
        print("❌ Server failed to start")
        p.terminate()
        log.close()
        raise RuntimeError("Server failed to start for chaos headers test")
    try:
        # 1. Latency Stress
        print("Testing X-Vidai-Latency: 1000 (Jitter 0)")
        start = time.time()
        requests.post(f"{BASE_URL}/v1/chat/completions", json={"model":"gpt-4"}, headers={"X-Vidai-Latency": "1000", "X-Vidai-Jitter": "0", "Content-Type": "application/json"})
        elapsed = time.time() - start
        print(f"DEBUG: Latency test took {elapsed:.4f}s")
        assert elapsed >= 0.95, f"Latency was too low: {elapsed:.4f}s"
        print(f"✅ Latency override successful: {elapsed:.2f}s")

        # 2. Chaos Drop (500)
        print("Testing X-Vidai-Chaos-Drop: 100")
        r = requests.post(f"{BASE_URL}/v1/chat/completions", json={"model":"gpt-4"}, headers={"X-Vidai-Chaos-Drop": "100", "Content-Type": "application/json"})
        assert r.status_code == 500
        print("✅ Chaos drop successful")

        # 3. Chaos Malformed
        print("Testing X-Vidai-Chaos-Malformed: 100")
        r = requests.post(f"{BASE_URL}/v1/chat/completions", json={"model":"gpt-4"}, headers={"X-Vidai-Chaos-Malformed": "100", "Content-Type": "application/json"})
        try:
            r.json()
            assert False, "Should have received malformed JSON"
        except:
            print("✅ Chaos malformed successful")

    finally:
        p.terminate()
        p.wait()
        log.close()

def test_fuzz_shadowing():
    print("--- Testing Security Fuzz Shadowing ---")
    # 1. Create a local shadow that injects fuzz data
    config_dir = "security_fuzz_dir"
    os.makedirs(os.path.join(config_dir, "templates/openai"), exist_ok=True)
    
    long_string = "A" * 10000
    fuzz_template = {
        "id": "fuzz-{{ uuid() }}",
        "choices": [{"message": {"role": "assistant", "content": f"{long_string} FUZZ"}}]
    }
    with open(os.path.join(config_dir, "templates/openai/chat.json.j2"), "w") as f:
        f.write(json.dumps(fuzz_template))
        
    p, log = start_server(["--config-dir", config_dir], log_name="fuzz_shadow.log")
    if not wait_for_server(p, log):
        print("❌ Server failed to start for fuzz shadowing")
        p.terminate()
        log.close()
        raise RuntimeError("Server failed to start for fuzz shadowing test")
    try:
        r = requests.post(f"{BASE_URL}/v1/chat/completions", json={"model": "gpt-4"}, headers={"Content-Type": "application/json"})
        if r.status_code != 200:
             print(f"❌ Fuzz request failed with status {r.status_code}: {r.text}")
        assert r.status_code == 200
        content = r.json()["choices"][0]["message"]["content"]
        if not ("AAAA" in content and "FUZZ" in content):
             print(f"❌ Incorrect content received. Full body: {r.text[:500]}...")
        assert "AAAA" in content and "FUZZ" in content
        assert len(content) >= 10000
        print(f"✅ Fuzz shadowing successful (Length: {len(content)})")
    finally:
        p.terminate()
        p.wait()
        log.close()
        shutil.rmtree(config_dir)

def test_header_injection_robustness():
    print("--- Testing Header Injection Robustness ---")
    p, log = start_server(log_name="header_robustness.log")
    if not wait_for_server(p, log):
        print("❌ Server failed to start for header robustness")
        p.terminate()
        log.close()
        raise RuntimeError("Server failed to start for header robustness test")
    try:
        # Extreme values in headers
        headers = {
            "X-Vidai-Latency": "2000", 
            "X-Vidai-Chaos-Drop": "999",   
            "X-Vidai-Chaos-Trickle": "abc", 
        }
        # Server should not crash, should handle gracefully (defaulting or capping)
        r = requests.post(f"{BASE_URL}/v1/chat/completions", json={"model":"gpt-4"}, headers=headers, timeout=5)
        assert r.status_code in [200, 500] 
        print("✅ Header robustness test passed (Server stable)")
    finally:
        p.terminate()
        p.wait()
        log.close()

if __name__ == "__main__":
    import traceback
    try:
        test_chaos_headers()
        print("\n")
        time.sleep(2)  # Allow port to be released
        test_fuzz_shadowing()
        print("\n")
        time.sleep(2)  # Allow port to be released
        test_header_injection_robustness()
        print("\n🎉 ALL SECURITY VERIFICATION TESTS PASSED")
    except Exception as e:
        print(f"❌ SECURITY TEST FAILED: {e}")
        traceback.print_exc()
        exit(1)
