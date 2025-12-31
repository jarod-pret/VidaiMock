# Contributing to VidaiMock

We welcome contributions! Thank you for your interest in improving VidaiMock.

## Development Setup

1. **Install Rust** (latest stable): https://rustup.rs/
2. **Clone the repository**:
   ```bash
   git clone https://github.com/vidaiUK/VidaiMock.git
   cd vidaimock
   ```
3. **Run tests**:
   ```bash
   cargo test
   ```
4. **Run verification scripts**:
   ```bash
   ./scripts/verify_cli.sh
   ./scripts/verify_metrics.sh
   ```

## Pull Request Process

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/my-feature`
3. Make your changes
4. Ensure all tests pass: `cargo test`
5. Run formatting: `cargo fmt`
6. Run lints: `cargo clippy`
7. Submit a PR with a clear description

## Code Style

- Run `cargo fmt` before committing
- Run `cargo clippy` and address warnings
- Add tests for new features
- Update documentation for user-facing changes

## Adding New Provider Presets

To add a new LLM provider format:

1. Create a JSON file in `presets/` (e.g., `presets/my_provider.json`)
2. The filename (without extension) becomes the format name
3. Update documentation if needed

## Reporting Issues

- Use GitHub Issues for bug reports and feature requests
- For security vulnerabilities, see [SECURITY.md](SECURITY.md)

## License

By contributing, you agree that your contributions will be licensed under the Apache 2.0 License.
