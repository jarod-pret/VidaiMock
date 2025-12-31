
import requests
import json
import base64
import struct
import binascii
import zlib

# Helper to decode AWS Event Stream
def decode_event_stream(data):
    offset = 0
    events = []
    
    while offset < len(data):
        # 1. Total Length
        if offset + 4 > len(data): break
        total_len = struct.unpack('>I', data[offset:offset+4])[0]
        
        # 2. Headers Length
        headers_len = struct.unpack('>I', data[offset+4:offset+8])[0]
        
        # 3. Prelude CRC
        prelude_crc = struct.unpack('>I', data[offset+8:offset+12])[0]
        
        # Verify Prelude CRC
        # crc = binascii.crc32(data[offset:offset+8]) & 0xffffffff
        # if crc != prelude_crc:
        #    print(f"Prelude CRC mismatch: {crc} != {prelude_crc}")
        
        # 4. Headers
        headers_end = offset + 12 + headers_len
        headers_bytes = data[offset+12:headers_end]
        
        # Parse Headers (simplified)
        headers = {}
        h_offset = 0
        while h_offset < len(headers_bytes):
            key_len = headers_bytes[h_offset]
            h_offset += 1
            key = headers_bytes[h_offset:h_offset+key_len].decode('utf-8')
            h_offset += key_len
            val_type = headers_bytes[h_offset]
            h_offset += 1
            
            if val_type == 7: # String
                val_len = struct.unpack('>H', headers_bytes[h_offset:h_offset+2])[0]
                h_offset += 2
                val = headers_bytes[h_offset:h_offset+val_len].decode('utf-8')
                h_offset += val_len
                headers[key] = val
        
        # 5. Payload
        payload_end = offset + total_len - 4
        payload_bytes = data[headers_end:payload_end]
        
        # 6. Message CRC
        msg_crc = struct.unpack('>I', data[payload_end:offset+total_len])[0]
        
        # Parse Payload if JSON
        try:
            payload_json = json.loads(payload_bytes)
            # Unwrap "bytes" if present -> Base64 -> JSON
            if "bytes" in payload_json:
                inner = base64.b64decode(payload_json["bytes"]).decode('utf-8')
                payload_json["_decoded_bytes"] = json.loads(inner)
        except:
            pass
            
        events.append({
            "headers": headers,
            "payload": payload_json
        })
        
        offset += total_len
        
    return events

print("Testing Bedrock Streaming...")
url = "http://localhost:8100/model/anthropic.claude-v2/invoke-with-response-stream"
# Need to use post with stream=true or just hit the endpoint which defaults to stream for this path?
# config says: matcher regex
# It doesn't enforce stream param, but handler checks for headers or "stream": true in body?
# The handler checks URI for /invoke-with-response-stream.

resp = requests.post(url, json={"prompt": "Hello"}, stream=True)

if resp.status_code != 200:
    print(f"Error: {resp.status_code} - {resp.text}")
    exit(1)

content_type = resp.headers.get("Content-Type")
print(f"Content-Type: {content_type}")

if content_type != "application/vnd.amazon.eventstream":
    print("FAIL: Wrong Content-Type")
    # exit(1) # Continue to see what we got

raw_data = resp.raw.read()
print(f"Received {len(raw_data)} bytes")

events = decode_event_stream(raw_data)
for i, ev in enumerate(events):
    print(f"Event {i}: {json.dumps(ev, indent=2, default=str)}")

if len(events) > 0:
    print("SUCCESS: Parsed events")
else:
    print("FAIL: No events parsed")
