import requests
import json
import time

BASE_URL = "http://127.0.0.1:8100"

def test_endpoint(name, method, path, json_data=None, params=None, is_stream=False):
    print(f"--- Testing {name} ({path}) ---")
    url = f"{BASE_URL}{path}"
    try:
        if method == "GET":
            r = requests.get(url, params=params)
        else:
            r = requests.post(url, json=json_data or {}, params=params, stream=is_stream)
        
        print(f"Status: {r.status_code}")
        if is_stream:
            for line in r.iter_lines():
                if line:
                    try:
                        line_str = line.decode('utf-8')
                        print(f"Stream: {line_str[:100]}...")
                        if "[DONE]" in line_str or "message_stop" in line_str or "stream-end" in line_str:
                            break
                    except UnicodeDecodeError:
                        print(f"Stream (Binary): {len(line)} bytes")

        else:
            print(f"Response: {json.dumps(r.json(), indent=2)[:200]}...")
        return r.status_code == 200
    except Exception as e:
        print(f"FAILED: {e}")
        return False

def run_all_tests():
    results = []
    
    # 1. Models
    results.append(test_endpoint("OpenAI Models", "GET", "/v1/models"))
    # results.append(test_endpoint("Anthropic Model", "GET", "/v1/models/claude-3-opus"))
    
    # 2. Embeddings
    results.append(test_endpoint("OpenAI Embeddings", "POST", "/v1/embeddings"))
    results.append(test_endpoint("Gemini Embeddings", "POST", "/v1beta/models/gemini-pro:embedContent"))
    
    # 3. Token Counts
    results.append(test_endpoint("Gemini Tokens", "POST", "/v1beta/models/gemini-pro:countTokens"))
    
    # 4. Corporate Providers (Bedrock/Vertex)
    results.append(test_endpoint("Bedrock Converse", "POST", "/model/anthropic.claude-v3/converse"))
    results.append(test_endpoint("Vertex Generate", "POST", "/v1/projects/p/locations/l/publishers/google/models/gemini-pro:generateContent"))
    results.append(test_endpoint("Groq Chat", "POST", "/v1/chat/completions", params={"format": "groq"}))
    results.append(test_endpoint("Cohere Chat", "POST", "/v1/chat/completions", params={"format": "cohere"}))

    # 5. Streaming
    results.append(test_endpoint("OpenAI Stream", "POST", "/v1/chat/completions/stream", is_stream=True))
    results.append(test_endpoint("Anthropic Stream", "POST", "/v1/messages/stream", params={"format": "anthropic"}, is_stream=True))
    results.append(test_endpoint("Gemini Stream", "POST", "/v1/projects/p/locations/l/publishers/google/models/gemini-pro:streamGenerateContent", is_stream=True))
    results.append(test_endpoint("Bedrock Stream", "POST", "/model/anthropic.claude-v3/converse-stream", is_stream=True))

    success = all(results)
    if success:
        print("\n✅ ALL TESTS PASSED")
    else:
        print("\n❌ SOME TESTS FAILED")
    return success

if __name__ == "__main__":
    run_all_tests()
