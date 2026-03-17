# Cokra

> AI Agent Team CLI Environment (Rust TUI)

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.93%2B-93450a.svg)](cokra-rs/Cargo.toml)

Cokra is a terminal-based coding agent that can orchestrate multiple AI teammates, run tools with approvals and sandboxing, and connect to multiple model providers (API key or OAuth). It is inspired by the architecture of OpenAI's Codex CLI, plus lessons from modern multi-agent orchestration systems.

If you find Cokra useful, please star the repo. Stars directly help open source distribution and long-term maintenance.

## 60-Second Quickstart

Prerequisites:

- Rust `1.93.0+` (Edition 2024)
- `just` (recommended, optional)
- Bazel `9.x` (optional, only for Bazel builds)
- Node.js `24+` + PNPM (optional, only for formatting markdown/json in this repo)

Run from source:

```bash
git clone https://github.com/brandlll-lee/cokra.git
cd cokra
just cokra
```

Or run directly with Cargo:

```bash
cd cokra/cokra-rs
cargo run --bin cokra --
```

Set a provider (example: OpenAI API key):

```bash
export OPENAI_API_KEY="sk-..."
```

Then in the UI try:

```text
Create a small CLI in Rust and add tests. If needed, spawn two teammates: one to implement, one to write tests.
```

Tip: use `/model` to connect providers via OAuth (GitHub Copilot, Google, etc.) and browse models.

## What You Get

- **Agent Teams**: spawn persistent teammates, message them, wait for scheduled work to settle, and coordinate work through a shared mailbox and task board.
- **Safe-by-default tool execution**: approvals + sandbox policies before shell or patches run.
- **Terminal UI (TUI)**: a focused coding workflow designed for long sessions.
- **Multi-provider model routing**: API key and OAuth connections across providers.
- **MCP support**: connect to external MCP servers and expose Cokra as an MCP server.

## Features (Current)

### Multi-Agent Collaboration

Cokra supports a lightweight agent-teams workflow:

- `spawn_agent`, `send_input`, `wait`, `close_agent`
- mailbox and task board tools (team messages, tasks, plans, ownership-aware handoff/review)

In the UI:

- `/agent` to browse teammates and switch threads
- `/collab` to view the team dashboard

### Tools (20+)

Core tools include:

- `shell` (command execution with approvals/sandbox)
- `apply_patch`, `read_file`, `write_file`, `list_dir`, `grep_files`
- MCP client tooling (`mcp`) and agent-teams tools

### Providers & Authentication

Cokra supports connecting providers via API key or OAuth (provider availability may vary by platform):

- OpenAI (API key)
- OpenRouter (API key)
- Anthropic (API key, OAuth)
- Google (API key)
- ChatGPT Plus/Pro Codex subscription (OAuth)
- GitHub Copilot (OAuth)
- Google Cloud Code Assist / Gemini CLI (OAuth)
- Antigravity (OAuth)

## Configuration

Cokra uses a layered config system:

1. built-in defaults
2. global: `~/.cokra/config.toml`
3. project: `.cokra/config.toml`
4. CLI overrides

See [docs/configuration.md](docs/configuration.md).

## Documentation

- [Getting Started](docs/getting-started.md)
- [Configuration](docs/configuration.md)
- [Architecture](docs/architecture.md)

## Project Status

**Early development.** Expect breaking changes. The best way to track progress is via issues/PRs and the changelog.

We care about:

- correctness and safety (approvals, sandboxing)
- fast iteration and strong UX
- production-grade internals (clear protocols, tests, deterministic behavior)

## Roadmap (High Level)

- Prebuilt binaries and installers
- More provider/runtime parity (especially OAuth runtimes and model discovery)
- Better docs, examples, and “starter tasks” for new users
- Stronger team workflows (task lifecycles, mailbox UX, persistence ergonomics)

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).

If you want to help Cokra grow (and become attractive to the broader open source ecosystem and open core builders), high-impact contributions include:

- docs and real-world tutorials
- reliability fixes (provider auth, model discovery, streaming correctness)
- UX improvements (TUI ergonomics, team dashboards, approvals)

## License & Attribution

Licensed under the Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

Cokra is not affiliated with or endorsed by OpenAI. It is an independent project inspired by Codex-style architecture.
