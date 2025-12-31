# VidaiMock

[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)

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
| OpenAI | `/v1/chat/completions` | ✅ |
| Anthropic | `/v1/messages` | ✅ |
| Gemini | `/v1beta/models/*` | ✅ |
| Azure OpenAI | `/openai/deployments/*` | ✅ |
| Bedrock | `/model/*/invoke` | ✅ |
| Cohere, Mistral, Groq | OpenAI-compatible | ✅ |

Plus: Tool calling, RAG citations, embeddings, and more.

## ✨ Key Features

- **🚀 Zero Config**: Single binary, instant startup, all providers included
- **🌊 Streaming Physics**: Realistic TTFT and token-by-token delivery
- **⚡ High Performance**: 50,000+ RPS in benchmark mode
- **🎛️ Chaos Testing**: Inject failures, latency, malformed responses
- **📝 Customizable**: YAML configs + Tera templates for any API

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

# Linux x64
curl -LO https://github.com/vidaiUK/VidaiMock/releases/latest/download/vidaimock-linux-x64.tar.gz
tar -xzf vidaimock-linux-x64.tar.gz && cd vidaimock

./vidaimock
```

### 🔐 Security Notice (macOS/Windows)
Since VidaiMock is an open-source project without a paid developer certificate, your OS may show a security warning:

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
  -d '{"model": "gpt-4", "messages": [{"role": "user", "content": "Hi"}]}'

# Anthropic messages
curl http://localhost:8100/v1/messages \
  -d '{"model": "claude-3", "messages": [{"role": "user", "content": "Hi"}]}'

# With latency simulation
./vidaimock --latency 500 --mode realistic

# Force errors (test retry logic)
curl -H "X-Vidai-Chaos-Drop: 100" http://localhost:8100/v1/chat/completions ...
```

## 📚 Documentation

The documentation for VidaiMock has moved to a separate documentation site (coming soon). 

Please check the [GitHub Wiki](https://github.com/vidaiUK/VidaiMock/wiki) or our website for the latest guides.

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

## 📄 License

Apache 2.0 — See [LICENSE](LICENSE).

---

Built with ❤️ by [Vidai](https://vidai.uk) from Scotland
