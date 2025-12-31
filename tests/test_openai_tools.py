
import requests
import json
import sys

print("Testing OpenAI Tool Streaming...")
url = "http://localhost:8100/v1/tools_mock"

payload = {
    "model": "gpt-4",
    "messages": [{"role": "user", "content": "call function"}],
    "stream": True
}

resp = requests.post(url, json=payload, stream=True)
if resp.status_code != 200:
    print(f"FAILED: Status {resp.status_code}")
    exit(1)

found_tool_calls = False
for line in resp.iter_lines():
    if not line: continue
    line = line.decode('utf-8')
    if line.startswith("data: "):
        data = line[6:]
        if data == "[DONE]": break
        try:
            chunk = json.loads(data)
            # Check for tool_calls in delta
            delta = chunk['choices'][0]['delta']
            if 'tool_calls' in delta:
                print(f"Found tool_calls: {delta['tool_calls']}")
                found_tool_calls = True
        except Exception as e:
            print(f"Error parsing chunk: {e}")

if found_tool_calls:
    print("SUCCESS: Received tool_calls in stream")
else:
    print("FAILED: No tool_calls found in stream")
    exit(1)
