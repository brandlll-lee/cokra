# Cokra Configuration Guide

## Configuration Locations

Cokra uses a layered configuration system. Configurations are loaded in the following order (later configs override earlier ones):

1. **Built-in defaults** - Hardcoded default values
2. **Global config** - `~/.cokra/config.toml`
3. **Project config** - `.cokra/config.toml` (in your project directory)
4. **Remote config** - Cloud requirements (if enabled)
5. **CLI overrides** - `-c key=value` flags

## Quick Start

Create your initial configuration:

```bash
# Create global config directory
mkdir -p ~/.cokra

# Create a basic config file
cat > ~/.cokra/config.toml << 'EOF'
# Cokra Configuration

[approval]
policy = "ask"

[sandbox]
mode = "permissive"
EOF
```

## Configuration Sections

### Approval Policy

Control when Cokra asks for permission before executing actions.

```toml
[approval]
# Overall approval policy: "ask", "auto", or "never"
policy = "ask"

# Shell command approval
shell = "on_failure"  # | "unless_trusted" | "always" | "never"

# Patch application approval
patch = "on_request"  # | "auto" | "never"

# File write approval
write = "ask"  # | "auto" | "never"
```

### Sandbox Mode

Control the security sandbox for command execution.

```toml
[sandbox]
# Sandbox mode: "strict", "permissive", or "danger_full_access"
mode = "permissive"

# Linux-specific options
[sandbox.linux]
# Enable network access (requires relaxed mode)
network = false
```

**Modes:**

- `strict` - Maximum security, filesystem isolation
- `permissive` - Balanced security with project access
- `danger_full_access` - No restrictions (use with caution!)

### Personality

Configure the AI agent's behavior style.

```toml
[personality]
# Personality: "default", "friendly", "pragmatic"
name = "pragmatic"
```

### Features

Enable or disable experimental features.

```toml
[features]
# Enable features
mcp = true
memories = false
web_search = false
js_repl = false

# Cloud features
cloud_tasks = false
```

### Model Configuration

Configure which AI model to use.

```toml
[models]
# Model provider: "openai", "ollama", "lmstudio"
provider = "openai"

# Model name
model = "gpt-5.2-codex"

# Base URL (for custom endpoints)
base_url = "https://api.openai.com/v1"

# Ollama-specific configuration
[models.ollama]
base_url = "http://localhost:11434"

# LM Studio specific configuration
[models.lmstudio]
base_url = "http://localhost:1234/v1"
```

### MCP Servers

Configure Model Context Protocol servers.

```toml
[mcp.servers.github]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]

[mcp.servers.filesystem]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/allowed"]

[mcp.servers.brave-search]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-brave-search"]
```

### Skills

Configure skills (reusable agent templates).

```toml
[skills]
# Enable skills system
enabled = true

# Local skill paths
paths = ["~/.cokra/skills", "./skills"]

# Remote skills (Git repositories)
[skills.remote]
github = ["org/repo", "another-org/another-repo"]
git = ["https://example.com/skill.git"]
```

### Notifications

Configure notification hooks.

```toml
[notifications]
# Enable notifications
enabled = false

# Notification methods
[[notifications.notify]]
method = "stdout"

[[notifications.notify]]
method = "command"
command = "notify-send"
args = ["Cokra", "{{message}}"]
```

## Example Configurations

### Development Mode

```toml
# ~/.cokra/config.toml
[approval]
policy = "auto"

[sandbox]
mode = "permissive"

[personality]
name = "friendly"

[features]
mcp = true
```

### Production Mode

```toml
# ~/.cokra/config.toml
[approval]
policy = "ask"
shell = "always"

[sandbox]
mode = "strict"

[personality]
name = "pragmatic"
```

### Local Models

```toml
# ~/.cokra/config.toml
[models]
provider = "ollama"
model = "codellama"

[models.ollama]
base_url = "http://localhost:11434"
```

## Project-Specific Configuration

Create `.cokra/config.toml` in your project directory for project-specific settings:

```toml
# .cokra/config.toml
[approval]
policy = "auto"  # Auto-approve for this project

[sandbox]
mode = "permissive"  # Allow project access

[personality]
name = "pragmatic"  # Use pragmatic personality for this project
```

## CLI Overrides

Override any configuration value via CLI:

```bash
# Override approval policy
cokra -c approval.policy=auto

# Override model
cokra -c models.provider=ollama -c models.model=codellama

# Multiple overrides
cokra -c approval.policy=auto -c sandbox.mode=permissive
```

## Configuration Schema

The JSON schema for configuration is at `cokra-rs/core/config.schema.json`. You can regenerate it with:

```bash
just write-config-schema
```

## Validation

Validate your configuration:

```bash
# Check if config is valid
cokra config validate
```

## Environment Variables

Some settings can be overridden via environment variables:

- `COKRA_CONFIG` - Path to config file
- `COKRA_API_KEY` - API key for models
- `OPENAI_API_KEY` - OpenAI API key
- `RUST_LOG` - Log level (e.g., `debug`, `info`, `warn`)

## Troubleshooting

### Config Not Loading

```bash
# Show current config
cokra config show

# Validate config
cokra config validate
```

### Permission Errors

Make sure your config file has correct permissions:

```bash
chmod 600 ~/.cokra/config.toml
```

## See Also

- [Authentication Guide](authentication.md)
- [Sandbox Guide](sandbox.md)
- [Skills System](skills.md)
