# VidaiMock

[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)

[Home Page](https://Vidai.uk) | [Documentation](https://vidai.uk/docs/mock/intro/)

**Batteries-included mock server for LLM APIs** — works instantly with OpenAI, Anthropic, Gemini, Bedrock, and more. Zero config required.

## ⚡ 30-Second Demo

```bash
# Download and bundle (macOS Apple Silicon)
curl -LO https://github.com/vidaiUK/VidaiMock/releases/latest/download/vidaimock-macos-arm64.tar.gz
tar -xzf vidaimock-macos-arm64.tar.gz && cd vidaimock

# Run and enjoy!
./vidaimock

# (In another terminal) Test it!
curl -N http://localhost:8100/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "gpt-4", "stream": true, "messages": [{"role": "user", "content": "Hello!"}]}'
```

Watch tokens appear one by one — that's realistic LLM simulation.

## 🔋 Batteries Included

No configuration needed. These providers work immediately:

| Provider | Endpoint | Streaming |
|----------|----------|-----------|
| **OpenAI Chat** | `/v1/chat/completions` | ✅ |
| **OpenAI Responses** | `/v1/responses` | ✅ (typed SSE events) |
| **OpenAI Embeddings** | `/v1/embeddings` | — |
| **OpenAI Images** | `/v1/images/generations` | — |
| **OpenAI Moderations** | `/v1/moderations` | — |
| **Anthropic** | `/v1/messages` | ✅ |
| **Gemini Generate** | `/v1beta/models/*:generateContent` | ✅ |
| **Gemini Embeddings** | `/v1beta/models/*:embedContent` | — |
| **Gemini Token Count** | `/v1beta/models/*:countTokens` | — |
| **Gemini Models** | `GET /v1beta/models` | — |
| **Gemini OpenAI Shim** | `/v1beta/openai/*` | ✅ |
| **Azure OpenAI** | `/openai/deployments/*` | ✅ |
| **Bedrock** | `/model/*/invoke` | ✅ |
| **Cohere, Mistral, Groq** | OpenAI-compatible | ✅ |
| **Error Simulator** | `/error/{code}` | — |

Plus: Tool calling (OpenAI + Gemini `functionCall`), reasoning model tokens, Gemini 2.5 `thoughtsTokenCount`, RAG citations, and more.

## ✨ Key Features

- **🚀 Zero Config / Zero Fixtures**: Single **~7MB binary**, instant startup, no Docker/DB, and zero setup required.
- **🌊 Physics-Accurate Streaming**: Realistic TTFT and token-by-token delivery with **provider-native streaming payloads** (OpenAI SSE, Responses API typed events, Anthropic EventStream, Gemini, etc.)
- **⚡ High Performance**: 50,000+ RPS in benchmark mode
- **🎛️ Chaos & Error Testing**: Inject failures, latency, malformed responses, and **custom HTTP status codes** (400, 401, 404, 429, 500, etc.) for error path testing
- **🧠 Smart Response Branching**: Templates auto-detect tool calls (OpenAI `tools` + Gemini `functionDeclarations`), reasoning models (o-series), structured output, and respond with the correct shape
- **📝 Customizable**: YAML configs + Tera templates for any API

## 🛡️ Built for Vidai.Server

VidaiMock is the official development environment for [Vidai.Server](https://vidai.uk)—the **High-Density Enterprise AI Gateway**. 

The same logic that powers VidaiMock's simulation of network jitter, latency, and failure modes is used in production to provide sovereign control planes for enterprise LLM infrastructure.

### 🌊 More than a Mock
Unlike tools that just record and replay static data or intercept browser requests, **VidaiMock is a standalone Simulation Engine**. It emulates the exact wire-format and per-token timing of LLM streaming payloads, making it the perfect tool for testing streaming UI/UX and SDK resilience.

*   **Truly Dynamic**: Every response is a Tera template. You can reflect request data, generate random IDs, or use complex logic to make your mock feel alive.
*   **Physics-Accurate**: Emulates real-world network protocols (SSE, EventStream) and silver-level latency.
*   **Error Path Testing**: Custom HTTP status codes (static or dynamic) let you test upstream error handling — 400s, 401s, 404s, 429s, 500s — with provider-accurate error envelopes.
*   **Smart Branching**: Chat templates auto-detect `tools`, `response_format`, and reasoning models (o1/o3/o4-series) from the request and return the correctly shaped response — no per-scenario config needed.

## 📂 Project Structure

- `bin/`: The VidaiMock executable
- `config/`: Default provider YAMLs and J2 templates
- `examples/`: 20+ advanced templates (RAG, Tool calling, Fuzzing, etc.)
- `scripts/`: Diagnostic and verification helpers

## 📦 Installation

**Download Bundled Release** (Recommended):
Releases come bundled with the binary, default providers, templates, and usage examples.

```bash
# macOS Apple Silicon
curl -LO https://github.com/vidaiUK/VidaiMock/releases/latest/download/vidaimock-macos-arm64.tar.gz
tar -xzf vidaimock-macos-arm64.tar.gz && cd vidaimock

# macOS Intel
curl -LO https://github.com/vidaiUK/VidaiMock/releases/latest/download/vidaimock-macos-x64.tar.gz
tar -xzf vidaimock-macos-x64.tar.gz && cd vidaimock

# Linux ARM64
curl -LO https://github.com/vidaiUK/VidaiMock/releases/latest/download/vidaimock-linux-arm64.tar.gz
tar -xzf vidaimock-linux-arm64.tar.gz && cd vidaimock

# Linux x64
curl -LO https://github.com/vidaiUK/VidaiMock/releases/latest/download/vidaimock-linux-x64.tar.gz
tar -xzf vidaimock-linux-x64.tar.gz && cd vidaimock

# Windows x64 (PowerShell)
Invoke-WebRequest -Uri https://github.com/vidaiUK/VidaiMock/releases/latest/download/vidaimock-windows-x64.zip -OutFile vidaimock-windows-x64.zip
Expand-Archive vidaimock-windows-x64.zip -DestinationPath .
cd vidaimock

./vidaimock
```

### 🔐 Security Notice (macOS/Windows)
Since VidaiMock is an open-source project, your OS may show a security warning:

*   **macOS**: Run `xattr -d com.apple.quarantine vidaimock` in your terminal to allow the binary to run.
*   **Windows**: Click "More info" in the SmartScreen popup and select "Run anyway".

**Build from source**:
```bash
git clone https://github.com/vidaiUK/VidaiMock.git
cd vidaimock && cargo build --release
./target/release/vidaimock
```

## 🎮 Quick Examples

```bash
# OpenAI chat completion
curl http://localhost:8100/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "gpt-4", "messages": [{"role": "user", "content": "Hi"}]}'

# Tool calling — auto-detects tools and returns tool_calls response
curl http://localhost:8100/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "gpt-4o", "messages": [{"role": "user", "content": "Weather?"}], "tools": [{"type": "function", "function": {"name": "get_weather", "parameters": {}}}]}'

# Reasoning models — returns reasoning_tokens in usage
curl http://localhost:8100/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "o4-mini", "messages": [{"role": "user", "content": "2+2"}]}'

# OpenAI Responses API (non-streaming)
curl http://localhost:8100/v1/responses \
  -H "Content-Type: application/json" \
  -d '{"model": "gpt-4o-mini", "input": "Say hello", "max_output_tokens": 50}'

# OpenAI Responses API (streaming with typed SSE events)
curl -N http://localhost:8100/v1/responses \
  -H "Content-Type: application/json" \
  -d '{"model": "gpt-4o-mini", "input": "Say hello", "stream": true}'

# Streaming with usage reporting
curl -N http://localhost:8100/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "gpt-4o", "stream": true, "stream_options": {"include_usage": true}, "messages": [{"role": "user", "content": "Hi"}]}'

# Embeddings, images, moderations
curl http://localhost:8100/v1/embeddings -H "Content-Type: application/json" \
  -d '{"model": "text-embedding-3-small", "input": "Hello"}'
curl http://localhost:8100/v1/images/generations -H "Content-Type: application/json" \
  -d '{"model": "dall-e-2", "prompt": "a red circle", "n": 1}'
curl http://localhost:8100/v1/moderations -H "Content-Type: application/json" \
  -d '{"model": "omni-moderation-latest", "input": "Hello"}'

# Gemini generateContent
curl http://localhost:8100/v1beta/models/gemini-2.5-flash:generateContent \
  -H "Content-Type: application/json" \
  -d '{"contents": [{"role": "user", "parts": [{"text": "Hello"}]}]}'

# Gemini tool calling (returns functionCall)
curl http://localhost:8100/v1beta/models/gemini-2.5-flash:generateContent \
  -H "Content-Type: application/json" \
  -d '{"contents": [{"role": "user", "parts": [{"text": "Weather?"}]}], "tools": [{"functionDeclarations": [{"name": "get_weather", "parameters": {"type": "OBJECT", "properties": {"city": {"type": "STRING"}}}}]}]}'

# Gemini embedContent, countTokens, model listing
curl http://localhost:8100/v1beta/models/gemini-embedding-001:embedContent \
  -H "Content-Type: application/json" -d '{"content": {"parts": [{"text": "Hello"}]}}'
curl http://localhost:8100/v1beta/models/gemini-2.5-flash:countTokens \
  -H "Content-Type: application/json" -d '{"contents": [{"role": "user", "parts": [{"text": "Hello"}]}]}'
curl http://localhost:8100/v1beta/models

# Error simulation — any HTTP status code, provider-agnostic
curl http://localhost:8100/error/400 -H "Content-Type: application/json" -d '{}'
curl http://localhost:8100/error/429 -H "Content-Type: application/json" -d '{}'

# Anthropic messages
curl http://localhost:8100/v1/messages \
  -H "Content-Type: application/json" \
  -d '{"model": "claude-3", "messages": [{"role": "user", "content": "Hi"}]}'

# With latency simulation
./vidaimock --latency 500 --mode realistic

# Force chaos errors (test retry logic)
curl -H "X-Vidai-Chaos-Drop: 100" http://localhost:8100/v1/chat/completions \
  -H "Content-Type: application/json" -d '{"model": "gpt-4", "messages": [{"role": "user", "content": "Hi"}]}'
```

## 📚 Documentation

The documentation for VidaiMock is available at our [Documentation Site](https://vidai.uk/docs/mock/intro/). 

For more information about Vidai, visit our [Home Page](https://Vidai.uk).

## 🛠️ CLI Reference

```
Usage: vidaimock [OPTIONS]

Options:
  --host <HOST>        Bind address [default: 0.0.0.0]
  -p, --port <PORT>    Listen port [default: 8100]
  --latency <MS>       Base response delay
  --mode <MODE>        benchmark | realistic
  --config-dir <DIR>   Custom provider configs
  -h, --help           Print help
```

## 🎯 Provider Config Reference

Provider YAML files in `config/providers/` define how endpoints match and respond:

```yaml
name: "my-provider"
matcher: "^/v1/my/endpoint$"         # Regex path match
response_template: "my/template.j2"  # Tera template path
status_code: "200"                   # HTTP status (static or Tera expression)
priority: 10                         # Higher matches first
stream:
  enabled: true
  frame_format: raw                  # "raw" = template controls SSE framing
  lifecycle:
    on_start:
      template_path: "my/stream_start.j2"
    on_chunk:
      template_path: "my/stream_delta.j2"
    on_stop:
      template_path: "my/stream_stop.j2"
```

**`status_code`** accepts static values (`"400"`) or Tera expressions (`"{{ path_segments | last }}"`) for dynamic HTTP status codes. The bundled `/error/{code}` endpoint uses this to simulate any error.

**`frame_format: raw`** gives the template full control over SSE framing — essential for providers like OpenAI's Responses API that use typed `event:` lines.

## 📄 License

Apache 2.0 — See [LICENSE](LICENSE).

---

### 🌐 Looking for Centralized Test Infrastructure?

VidaiMock runs locally, but we offer a managed control plane for enterprise teams. 

**[Get Started with Vidai Managed](https://vidai.uk)**

---

## 💜 Acknowledgments

VidaiMock is built on the shoulders of giants in the Rust ecosystem:
- [Axum](https://github.com/tokio-rs/axum) & [Tokio](https://github.com/tokio-rs/tokio) for the high-performance async foundation.
- [Tera](https://github.com/Keats/tera) for the flexible templating engine.
- [rust-embed](https://github.com/pyrossh/rust-embed) for the zero-config binary magic.
- [Mimalloc](https://github.com/microsoft/mimalloc) for the lightning-fast memory allocation.

---

## 👥 Contributors

A special thanks to everyone who helps make VidaiMock better!

| Contributor | Highlights |
| :--- | :--- |
| [<img src="https://github.com/NiltonVolpato.png?size=64" width="64" alt="NiltonVolpato"/><br/>**NiltonVolpato**](https://github.com/NiltonVolpato) | 🛠️ Improvements to listener address reporting and OS-assigned port support. |
| [<img src="https://github.com/bbRLdev.png?size=64" width="64" alt="bbRLdev"/><br/>**bbRLdev**](https://github.com/bbRLdev) | 🌊 Improvements to OpenAI streaming logic and termination events. |
| [<img src="https://github.com/nagug.png?size=64" width="64" alt="nagug"/><br/>**nagug**](https://github.com/nagug) | 🚀 Core architecture, high-density engine design, and project maintainer. |

---

Built with ❤️ by [Vidai](https://vidai.uk) from Scotland 🏴󠁧󠁢󠁳󠁣󠁴󠁿
