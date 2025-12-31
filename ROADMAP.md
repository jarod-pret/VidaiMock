# VidaiMock Roadmap

## Current Release (v0.1.0)

✅ **Shipped Features:**
- Batteries-included provider support (OpenAI, Anthropic, Gemini, Bedrock, Azure, Cohere, Mistral, Groq)
- SSE streaming with provider-specific lifecycles
- Tera templating engine for dynamic responses
- Chaos engineering (latency, drops, malformed responses, disconnects)
- Header overrides for per-request behavior
- High-performance Rust engine (50k+ RPS)
- Zero-config startup with embedded defaults
- Shadowing system for customization

## Coming Next

### v0.2.0 — Enhanced Simulation
- [ ] Rate limit simulation (token bucket for 429 responses)
- [ ] Cost estimation metadata in responses
- [ ] Per-provider latency configuration
- [ ] Improved token counting for usage stats

### v0.3.0 — Ecosystem Expansion
- [ ] Vector DB mocks (Pinecone, Qdrant API parity)
- [ ] Image generation mocks (DALL-E, Midjourney placeholders)
- [ ] Embedding dimension configuration

## Future Ideas

- Distributed coordination (multi-instance sync)
- Web UI for scenario management
- gRPC support
- Webhook analytics

---

## Contributing

We welcome community contributions:
- **Provider configs**: Add support for new LLM providers
- **Templates**: Create useful response scenarios
- **Bug fixes**: PRs always appreciated

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

---

*Last updated: December 2025*
