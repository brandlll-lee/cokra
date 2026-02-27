# Cokra Architecture

## Overview

Cokra is a 1:1 architectural replica of [OpenAI Codex](https://github.com/openai/codex), an AI-powered agent team system for autonomous coding and collaboration.

## High-Level Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         User                                │
└────────────────────┬───────────���────────────────────────────┘
                     │
        ┌────────────┴────────────┐
        │                         │
┌───────▼────────┐        ┌───────▼────────┐
│  cokra-cli     │        │  Desktop App/  │
│  (Node.js)     │        │      IDE       │
│  Entry Point   │        │  Extension     │
└───────┬───────┘        └───────┬────────┘
        │                         │
        │    Platform Binary      │ WebSocket
        │    Detection            │ JSON-RPC
        │                         │
        └────────────┬────────────┘
                     │
        ┌────────────▼────────────┐
        │     cokra-rs/cli        │
        │   (Main Entry Point)    │
        └────────────┬────────────┘
                     │
        ┌──────────────────────────▼─────────────────────────────┐
        │              cokra-core                                │
        │  ┌──────────────────────────────────────────┐          │
        │  │  Cokra (Main Orchestrator)               │          │
        │  │  - Session management                    │          │
        │  │  - Turn execution                        │          │
        │  │  - Agent coordination                    │          │
        │  │  - Tool routing                          │          │
        │  └──────────────────────────────────────────┘          │
        │                                                             │
        │  Key Subsystems:                                           │
        │  - Agent System (spawn, coordinate, multi-agents)         │
        │  - Tool Registry (20+ tools)                              │
        │  - MCP Manager (server connections)                       │
        │  - Models Manager (provider routing)                      │
        │  - Config Manager (layered config)                        │
        │  - State Manager (SQLite persistence)                     │
        └────────────┬───────────────────────────────────────────────┘
                     │
        ┌────────────▼────────────────┐
        │         Protocols            │
        │  - cokra-protocol           │
        │  - app-server-protocol      │
        └────────────┬────────────────┘
                     │
        ┌────────────▼──────────────────────────────────────┐
        │                   External Services                │
        │  - OpenAI API (HTTP/SSE)                          │
        │  - MCP Servers (stdio/HTTP)                       │
        │  - Ollama/LMStudio (local models)                 │
        └───────────────────────────────────────────────────┘
```

## Core Components

### 1. Cokra Core (`cokra-rs/core/`)

The heart of the system, containing:

- **Cokra** (`cokra.rs`): Main orchestrator (8,463 lines in Codex)
- **Config System**: Layered configuration management
- **Agent System**: Multi-agent spawning and coordination
- **Tool System**: 20+ built-in tools
- **MCP Integration**: Model Context Protocol support
- **Session Management**: Turn execution and state

### 2. Protocol Layer (`cokra-rs/protocol/`)

Defines all communication:

- Events (TurnStarted, ItemStarted, etc.)
- Operations (UserTurn, Steer, Interrupt)
- Response Items (AgentMessage, CommandExecution, etc.)
- Content Items and Turn Context

### 3. CLI (`cokra-rs/cli/`)

Command-line interface built with `clap`:

- Interactive mode
- Single-task execution
- Configuration management
- MCP server management

### 4. Terminal UI (`cokra-rs/tui/`)

Interactive terminal interface using `ratatui`:

- Chat interface
- Approval prompts
- File search
- Command palette
- Streaming output

### 5. App Server (`cokra-rs/app-server/`)

JSON-RPC server for IDE/desktop integrations:

- Thread management
- Turn execution
- Configuration API
- Model listing
- Skills management

## Technology Stack

| Component | Technology |
|-----------|------------|
| **Core** | Rust 2024 Edition |
| **CLI Wrapper** | TypeScript/Node.js |
| **Build System** | Bazel 9.0.0 |
| **Package Manager** | PNPM workspace |
| **Terminal UI** | ratatui |
| **Database** | SQLite (sqlx) |
| **Async Runtime** | tokio |
| **Serialization** | serde |
| **MCP** | rmcp (Rust MCP) |

## Multi-Agent System

Cokra supports spawning specialized sub-agents:

```
Main Agent (Orchestrator)
    ├── spawn_agent(research)
    ├── spawn_agent(implement)
    └── spawn_agent(test)
```

### Agent Tools

- `spawn_agent` - Create a sub-agent
- `send_input` - Send messages to agents
- `wait` - Wait for agent completion
- `close_agent` - Terminate agent
- `resume_agent` - Resume paused agent

## Tool System

### Built-in Tools (20+)

| Tool | Purpose |
|------|---------|
| `shell` | Execute shell commands |
| `apply_patch` | Apply unified diffs |
| `read_file` | Read file contents |
| `write_file` | Write files |
| `list_dir` | List directory contents |
| `grep_files` | Search with ripgrep |
| `search_tool` | BM25 semantic search |
| `mcp` | Call MCP tools |
| `spawn_agent` | Create sub-agents |
| `plan` | Show execution plan |
| `request_user_input` | Ask user for input |
| `view_image` | View image files |

## MCP Integration

Cokra implements the Model Context Protocol:

- **MCP Server**: Exposes Cokra tools to MCP clients
- **MCP Client**: Connects to external MCP servers
- **Transport**: stdio, HTTP, WebSocket
- **Features**: Tool discovery, resource reading, approval flow

## Configuration System

### Layered Configuration

1. Built-in defaults
2. Global config (`~/.cokra/config.toml`)
3. Project config (`.cokra/config.toml`)
4. Remote config (cloud requirements)
5. CLI overrides (`-c key=value`)

### Key Sections

```toml
[approval]
policy = "ask"  # | "auto" | "never"

[sandbox]
mode = "strict"  # | "permissive" | "danger_full_access"

[personality]
name = "pragmatic"  # | "friendly"

[mcp.servers.myserver]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
```

## State Management

Cokra uses SQLite for persistent state:

- Thread metadata
- Token usage
- Session history
- Archive state
- Rollout entries

## Security

### Sandboxing

- **Linux**: Landlock (filesystem + network)
- **macOS**: Seatbelt (profile-based)
- **Windows**: Job Objects (process isolation)

### Command Safety

- Dangerous command detection
- Approval policies
- Trusted command caching

## Project Structure

```
cokra/
├── cokra-rs/              # Rust workspace (65+ crates)
│   ├── cli/               # CLI entry
│   ├── core/              # Core orchestrator
│   ├── protocol/          # Protocol definitions
│   ├── tui/               # Terminal UI
│   ├── app-server/        # JSON-RPC server
│   ├── state/             # SQLite state
│   ├── mcp-server/        # MCP server
│   └── utils/             # Utility crates
├── cokra-cli/             # Node.js wrapper
├── sdk/typescript/        # TypeScript SDK
├── shell-tool-mcp/        # MCP server
├── docs/                  # Documentation
├── scripts/               # Build scripts
├── patches/               # Dependency patches
└── third_party/           # Vendored dependencies
```

## Development Workflow

```bash
# Format code
just fmt

# Run linting
just lint

# Run tests
just test

# Build release binaries
just build-for-release

# Generate config schema
just write-config-schema

# Generate protocol schema
just write-app-server-schema
```

## License

Apache License 2.0 - See [LICENSE](../LICENSE) for details.

## Acknowledgments

Cokra is a 1:1 architectural replica of [OpenAI Codex](https://github.com/openai/codex). The original project is copyrighted by OpenAI and licensed under the Apache License 2.0.
