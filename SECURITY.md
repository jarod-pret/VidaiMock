# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

Please report security vulnerabilities to: **security@vidai.uk**

**Do NOT create public GitHub issues for security vulnerabilities.**

### Response Timeline
- **Initial response**: 48 hours
- **Assessment**: 7 days
- **Fix timeline**: Depends on severity

## Security Features

VidaiMock includes several security features:

- **Path Traversal Protection**: Preset file loading validates paths to prevent directory traversal attacks (tested)
- **No Unsafe Code**: The codebase contains no `unsafe` Rust blocks
- **Configurable Bind Address**: Use `--host 127.0.0.1` to bind only to localhost
- **Dependency Auditing**: We recommend running `cargo audit` regularly

## Recommended Deployment

For production use:

```bash
# Bind to localhost only (recommended for local development)
vidaimock --host 127.0.0.1 --port 8100

# For container deployments, use 0.0.0.0 (default)
vidaimock --port 8100
```

When exposing to networks:
- Place behind a reverse proxy (nginx, traefik) with rate limiting
- Use firewall rules to restrict access
- Consider TLS termination at the proxy level
