import requests
import json

BASE_URL = "http://127.0.0.1:8100"

def test_models():
    print("Testing /v1/models...")
    r = requests.get(f"{BASE_URL}/v1/models")
    print(f"Status: {r.status_code}")
    print(r.json())

    print("\nTesting Anthropic models...")
    r = requests.get(f"{BASE_URL}/v1/models/anthropic")
    print(r.json())

def test_embeddings():
    print("\nTesting /v1/embeddings...")
    r = requests.post(f"{BASE_URL}/v1/embeddings", json={})
    print(r.json())

def test_streaming_openai():
    print("\nTesting OpenAI Streaming...")
    r = requests.post(f"{BASE_URL}/v1/chat/completions/stream", stream=True)
    for line in r.iter_lines():
        if line:
            print(f"Line: {line.decode('utf-8')}")

def test_streaming_anthropic():
    print("\nTesting Anthropic Streaming...")
    r = requests.post(f"{BASE_URL}/v1/messages/stream?format=anthropic", stream=True)
    for line in r.iter_lines():
        if line:
            print(f"Line: {line.decode('utf-8')}")

def test_bedrock_converse():
    print("\nTesting Bedrock Converse...")
    r = requests.post(f"{BASE_URL}/model/anthropic.claude-3-sonnet/converse", json={})
    print(r.json())

if __name__ == "__main__":
    try:
        test_models()
        test_embeddings()
        test_bedrock_converse()
        test_streaming_openai()
        test_streaming_anthropic()
    except Exception as e:
        print(f"Error: {e}")
