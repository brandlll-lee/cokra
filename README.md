# Cokra

> **Cokra** - AI Agent Team CLI Environment

Cokra is an AI-powered agent team system for autonomous coding and collaboration, inspired by [OpenAI Codex](https://github.com/openai/codex).

## Project Status

**Early Development** - This project is in its initial setup phase.

## What is Cokra?

Cokra is an AI agent team CLI environment that enables:

- **Multi-Agent Coordination** - Spawn and manage specialized AI agents
- **Tool Integration** - 20+ built-in tools (shell, file operations, MCP, etc.)
- **MCP Protocol Support** - Model Context Protocol server/client
- **Terminal UI** - Interactive terminal-based interface (ratatui)
- **Cross-Platform** - Linux, macOS, Windows support

## Architecture

Cokra architecture follows patterns from Codex:

```
cokra/
├── cokra-rs/          # Rust core (907 files, 65+ crates)
│   ├── cli/          # CLI entry point
│   ├── core/         # Core orchestrator
│   ├── protocol/     # Protocol definitions
│   ├── tui/          # Terminal UI
│   ├── app-server/   # JSON-RPC server
│   └── ...
├── cokra-cli/        # Node.js wrapper
├── sdk/typescript/   # TypeScript SDK
└── shell-tool-mcp/   # MCP server
```

## Quick Start (Coming Soon)

```bash
# Install via npm (when published)
npm install -g @cokra/cli

# Or build from source
git clone https://github.com/cokra/cokra
cd cokra
just build
just run
```

## Documentation

- [Configuration Guide](docs/configuration.md) (TODO)
- [Authentication](docs/authentication.md) (TODO)
- [Skills System](docs/skills.md) (TODO)
- [Architecture](docs/architecture.md) (TODO)

## Development

### Prerequisites

- Rust 1.93.0+ (Edition 2024)
- Bazel 9.0.0
- Node.js 24+
- PNPM 10.28.0+
- Just command runner

### Build

```bash
# Format code
just fmt

# Run linting
just lint

# Run tests
just test

# Build release binaries
just build-for-release
```

## License

Apache License 2.0 - see [LICENSE](LICENSE) for details.

## Acknowledgments

Cokra architecture is inspired by [OpenAI Codex](https://github.com/openai/codex). The original Codex project is copyrighted by OpenAI and licensed under the Apache License 2.0.

This project also includes code derived from:
- [Ratatui](https://github.com/ratatui/ratatui) - MIT License
- [Meriyah](https://github.com/meriyah/meriyah) - ISC License

See [NOTICE](NOTICE) for full attribution.

## Contributing

Contributions are welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

---

**Note**: Cokra is not affiliated with or endorsed by OpenAI. It is an independent project inspired by Codex architecture.
