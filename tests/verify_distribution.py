import requests
import json
import subprocess
import time
import os
import shutil

BASE_URL = "http://127.0.0.1:8100"

def start_server(args=None):
    cmd = ["target/debug/vidaimock", "--port", "8191"]
    if args:
        cmd.extend(args)
    return subprocess.Popen(cmd, env={"VIDAIMOCK_LOG_LEVEL": "info"})

def test_embedded_defaults():
    print("--- Testing Embedded Defaults (Zero-Config) ---")
    # Start server with a non-existent config dir
    p = start_server(["--config-dir", "non_existent_dir_random_123"])
    time.sleep(2)
    try:
        r = requests.get(f"{BASE_URL}/v1/models")
        print(f"Status: {r.status_code}")
        models = r.json()
        print(f"Models: {len(models['data'])} loaded")
        assert r.status_code == 200
        assert len(models["data"]) > 0
        print("✅ Embedded defaults test passed")
    finally:
        p.terminate()
        p.wait()

def test_shadowing():
    print("--- Testing Shadowing (Local Overrides) ---")
    config_dir = "test_shadowing_dir"
    os.makedirs(os.path.join(config_dir, "providers"), exist_ok=True)
    
    custom_provider = {
        "name": "shadow-provider",
        "matcher": "^/shadow$",
        "response_template": "openai/chat.json.j2"
    }
    
    with open(os.path.join(config_dir, "providers/openai.yaml"), "w") as f:
        import yaml
        yaml.dump(custom_provider, f)
        
    p = start_server(["--config-dir", config_dir])
    time.sleep(2)
    try:
        # Check if "openai" provider is now "shadow-provider"
        # Since we use the same filename "openai.yaml", it shadows the embedded one.
        # But we also gave it a new matcher "^/shadow$"
        r = requests.get(f"{BASE_URL}/v1/models")
        models = r.json()["data"]
        names = [m["id"] for m in models]
        print(f"Loaded providers: {names}")
        assert "shadow-provider" in names
        assert "openai" not in names # Should be shadowed
        
        print("✅ Shadowing test passed")
    finally:
        p.terminate()
        p.wait()
        shutil.rmtree(config_dir)

def test_cli_overrides():
    print("--- Testing CLI path:format Overrides ---")
    p = start_server(["--endpoints", "/v1/custom:echo"])
    time.sleep(2)
    try:
        # Test the custom endpoint with POST to get echo
        payload = {"hello": "world"}
        r = requests.post(f"{BASE_URL}/v1/custom", json=payload)
        print(f"Status: {r.status_code}")
        assert r.status_code == 200
        # Echo format returns request body
        print(f"Response: {r.text}")
        assert "hello" in r.text
        print("✅ CLI Overrides test passed")
    finally:
        p.terminate()
        p.wait()

if __name__ == "__main__":
    try:
        # Build binary first
        print("Building binary...")
        subprocess.run(["cargo", "build"], check=True)
        
        test_embedded_defaults()
        print("\n")
        test_shadowing()
        print("\n")
        test_cli_overrides()
        
        print("\n🎉 ALL DISTRIBUTION VERIFICATION TESTS PASSED")
    except Exception as e:
        print(f"❌ TEST FAILED: {e}")
        exit(1)
