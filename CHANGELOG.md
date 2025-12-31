# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
### Added
- Vertex AI provider with support for Google Cloud endpoint patterns.
- Robust Google Gemini AI Studio vs Vertex AI matching logic.
- Comprehensive documentation restructure (10+ new guides).
- Enhanced `extract_content_from_str` for better Gemini/Vertex streaming support.
- **Provider Priority**: New `priority` field in YAML configs for deterministic matching when patterns overlap.
- **Stable Context Variables**: `{{ uuid }}` and `{{ timestamp }}` (Number) are now stable across the entire request.

### Fixed
- Route conflict between Gemini POST and Anthropic GET paths.
- Tera template syntax for `random_int` (requires named arguments).
- Regression in template context variables (`uuid`, `timestamp`).

## [0.1.0] - 2025-12-15

### Added
- Initial release of VidaiMock
- Multi-provider support: OpenAI, Anthropic, Gemini, OpenRouter formats
- High-performance async server using Axum and Tokio
- `mimalloc` allocator for improved performance
- Latency simulation modes: `benchmark` (zero-latency) and `realistic` (configurable delay + jitter)
- Custom preset support via JSON files in `presets/` directory
- Custom response file override via `--response-file` flag
- Configurable endpoints via CLI or TOML config file
- Prometheus metrics endpoint (`/metrics`)
- Health check endpoint (`/health`)
- Status endpoint (`/status`)
- Echo handler for debugging
- Path traversal protection (security hardened)
- Fuzz testing with proptest
- Configurable bind address via `--host` flag
- Graceful shutdown on SIGTERM/SIGINT
- Structured JSON logging via tracing

### Security
- Path traversal protection tested and verified
- No `unsafe` code blocks
- Configurable network binding (localhost vs all interfaces)

### Documentation
- README with quick start guide
- USER_GUIDE with detailed configuration
- TUNING guide for performance optimization
- SECURITY.md for vulnerability reporting
- CONTRIBUTING.md for contributors
