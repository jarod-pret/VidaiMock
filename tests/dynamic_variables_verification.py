import requests
import json
import time
import re

BASE_URL = "http://127.0.0.1:8100"

def test_advanced_vars():
    print("--- Testing Advanced Random & Time Offset Variables ---")
    r = requests.post(f"{BASE_URL}/v1/chat/completions", json={"model": "gpt-4"})
    content = r.json()["choices"][0]["message"]["content"]
    print(f"Content: {content}")
    
    if "GARBAGE:" in content and re.search(r"[!@#$%^&*(){}\[\]]", content):
        print("✅ rand_garbage detected")
    if "WORDS:" in content and len(content.split("WORDS:")[1].split(" ")[1:4]) >= 3:
        print("✅ rand_words detected")
    if "OFFSET:" in content and re.search(r"\d{10}", content):
        print("✅ now_offset detected")

def test_deep_json():
    print("--- Testing Deep JSON Reflection ---")
    payload = {
        "metadata": {
            "session": {
                "id": "sess_999"
            }
        }
    }
    r = requests.post(f"{BASE_URL}/v1/chat/completions", json=payload)
    content = r.json()["choices"][0]["message"]["content"]
    print(f"Content: {content}")
    if "SESS: sess_999" in content:
        print("✅ echo_json deep path detected")

if __name__ == "__main__":
    # Expanded Test Preset
    test_preset = {
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": (
                        "TS: {{timestamp}} ISO: {{iso_timestamp}} UUID: {{uuid}} REQ: {{request_id}} "
                        "USER: {{echo_json:user}} ORG: {{echo_header:X-Org}} MSG: {{echo_last_user_msg}} "
                        "CHOICE: {{choice('A','B')}} GARBAGE: {{rand_garbage(5)}} WORDS: {{rand_words(3)}} "
                        "OFFSET: {{now_offset('1h')}} SESS: {{echo_json:metadata.session.id}}"
                    )
                },
                "finish_reason": "stop"
            }
        ]
    }
    
    with open("presets/dynamic_test.json", "w") as f:
        json.dump(test_preset, f)
        
    import subprocess
    config_content = """
port = 3002
[[endpoints]]
path = "/v1/chat/completions"
format = "dynamic_test"
"""
    with open("dynamic-test.toml", "w") as f:
        f.write(config_content)
        
    server = subprocess.Popen(["./target/debug/vidaimock", "--config", "dynamic-test.toml"])
    time.sleep(2)
    
    try:
        test_advanced_vars()
        test_deep_json()
        print("\n✅ ADVANCED DYNAMIC VARIABLE TESTS FINISHED")
    finally:
        server.terminate()
