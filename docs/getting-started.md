# Getting Started with Cokra

Welcome to Cokra! This guide will help you get started with the AI Agent Team CLI Environment.

## What is Cokra?

Cokra is a 1:1 architectural replica of [OpenAI Codex](https://github.com/openai/codex). It's an AI-powered agent team system that can:

- **Write and modify code** - Create, edit, and refactor code across multiple files
- **Execute commands** - Run shell commands in a secure sandbox
- **Coordinate multiple agents** - Spawn specialized agents for different tasks
- **Integrate with tools** - Use 20+ built-in tools and MCP servers
- **Interactive terminal UI** - Beautiful terminal-based interface

## Prerequisites

Before installing Cokra, make sure you have:

- **Operating System**: Linux, macOS, or Windows
- **Node.js**: 24.0 or later
- **API Key**: OpenAI API key or compatible service

## Installation

### Option 1: Install via npm (Recommended)

```bash
npm install -g @cokra/cli
```

### Option 2: Build from Source

```bash
# Clone the repository
git clone https://github.com/cokra/cokra
cd cokra

# Install dependencies
pnpm install

# Build
just build

# Install
just install
```

## Initial Setup

### 1. Configure Your API Key

Set your OpenAI API key:

```bash
export OPENAI_API_KEY="sk-..."
```

Or add it to your config file:

```bash
mkdir -p ~/.cokra
cat > ~/.cokra/config.toml << 'EOF'
# Cokra Configuration
EOF
```

### 2. Create a Basic Configuration

```bash
cat > ~/.cokra/config.toml << 'EOF'
[approval]
policy = "ask"

[sandbox]
mode = "permissive"

[personality]
name = "friendly"
EOF
```

## Your First Session

Start Cokra in interactive mode:

```bash
cokra
```

You'll see the terminal UI interface. Try these commands:

### Example 1: Create a New File

```
Create a Python script that calculates Fibonacci numbers
```

Cokra will:
1. Understand your request
2. Create the file
3. Ask for approval
4. Execute the change

### Example 2: Refactor Code

```
Refactor the function in main.py to be more efficient
```

### Example 3: Run Tests

```
Run all tests and fix any failures
```

## Basic Commands

### Interactive Mode

```bash
cokra                    # Start interactive mode
cokra -d /path/to/project  # Start in specific directory
```

### Single Task Mode

```bash
cokra run "your task here"    # Run a single task
cokra run -t "your task"      # Run with timeout
```

### Configuration Commands

```bash
cokra config show          # Show current configuration
cokra config edit          # Edit configuration
cokra config validate      # Validate configuration
```

### MCP Commands

```bash
cokra mcp list            # List MCP servers
cokra mcp test <server>   # Test MCP server connection
```

## Understanding the Interface

### Terminal UI

The terminal UI has several sections:

```
┌─────────────────────────────────────────────┐
│              Chat Area                      │
│  - Agent messages                           │
│  - Tool executions                          │
│  - File changes                             │
├─────────────────────────────────────────────┤
│              Input Area                     │
│  - Type your requests here                  │
│  - Multi-line support (Ctrl+Enter)          │
└─────────────────────────────────────────────┘
```

### Approval Prompts

When Cokra needs approval, you'll see:

```
┌─ Approve? ─────────────────────────────────┐
│ Shell: echo "Hello, World!"                │
│                                              │
│ [y] Yes  [n] No  [a] Always  [s] Skip      │
└──────────────────────────────────────────────┘
```

## Next Steps

### Learn About

- **[Configuration](configuration.md)** - Customize Cokra behavior
- **[Authentication](authentication.md)** - Set up authentication
- **[Sandbox](sandbox.md)** - Understand security sandboxing
- **[Skills](skills.md)** - Create reusable agent templates
- **[Slash Commands](slash_commands.md)** - Use built-in commands

### Advanced Features

- **Multi-Agent Coordination** - Spawn specialized agents
- **MCP Integration** - Connect to MCP servers
- **Custom Tools** - Create your own tools
- **App Server** - Use JSON-RPC API for IDE integration

## Troubleshooting

### API Key Issues

```bash
# Check if API key is set
echo $OPENAI_API_KEY

# Set API key temporarily
export OPENAI_API_KEY="sk-..."
```

### Permission Errors

```bash
# Check config permissions
ls -la ~/.cokra/config.toml

# Fix permissions
chmod 600 ~/.cokra/config.toml
```

### Build Issues

```bash
# Clean and rebuild
cd cokra
just clean
just build
```

## Getting Help

- **Documentation**: [https://github.com/cokra/cokra/tree/main/docs](https://github.com/cokra/cokra/tree/main/docs)
- **Issues**: [https://github.com/cokra/cokra/issues](https://github.com/cokra/cokra/issues)
- **Discussions**: [https://github.com/cokra/cokra/discussions](https://github.com/cokra/cokra/discussions)

## License

Apache License 2.0 - See [LICENSE](../LICENSE) for details.

---

**Welcome to Cokra! Happy coding! 🚀**
