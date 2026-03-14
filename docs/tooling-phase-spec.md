# Tooling Phase Spec

## Scope

This document freezes the current tool-kernel baseline before deeper changes land in the command execution, network policy, and LSP tool stacks.

The baseline is defined by:

- builtin tool schemas from `cokra-rs/core/src/tools/spec/primitive_specs.rs`
- runtime routing and approval behavior from `cokra-rs/core/src/tools/router.rs`
- runtime catalog and execution normalization from `cokra-rs/core/src/tool_runtime/executor.rs`
- runtime tool summary rendering from `cokra-rs/core/src/turn/executor.rs`

## Track 1: Command Execution

### Current State

- `shell` exposes a shell-string surface for the model.
- `unified_exec` exposes a pre-tokenized argv surface.
- router-level approval uses `eval_exec_approval`, but shell-string approval still depends on coarse command parsing.
- sandbox retry behavior already exists through `ToolOrchestrator`.
- tool output is normalized into the structured exec envelope.

### Benchmarks

- `codex` uses a unified exec kernel, canonical command approval keys, and approval caching keyed by command plus environment-relevant execution context.
- `opencode` parses shell commands up front and derives path intent before permission prompts.

### Gaps

- shell-string approval keys are not frozen against canonical command intent.
- command validation still relies on raw dangerous-pattern matching.
- path intent, mutation class, and network hint are not part of the command baseline.
- interactive process session APIs are still missing.

### Target Definition

- approval keys freeze on canonical command plus execution context instead of raw JSON arguments
- command intent extraction returns:
  - canonical command
  - command prefix
  - path intents
  - mutation class
  - network hint
  - external path set
- shell-string approval reuses the same safety policy as argv execution

### Non-Goals

- PTY session management
- stdin streaming
- TTY resize/kill/list session APIs
- replacing the unified exec layer

## Track 2: Network Policy And Approval

### Current State

- `web_fetch` and `web_search` exist and require approval in tool metadata.
- orchestrator has a lightweight network-approval lifecycle for immediate and deferred modes.
- managed-network mode exists as a turn flag, but host-level session caching is not frozen.

### Benchmarks

- `codex` tracks active network calls, host-level approval state, blocked-request outcomes, and session-scoped allow/deny caches.
- `opencode` combines network tools with explicit permission flow and provider-aware routing.

### Gaps

- no frozen host/protocol/port key shape
- no pending host approval dedupe
- no session-scoped approved/denied host cache
- no turn-context allow/deny domain lists surfaced to tool handlers
- runtime summary does not describe network policy state

### Target Definition

- host approval key freezes on protocol, host, and port
- pending host approvals dedupe concurrent prompts
- allow-once, allow-for-session, and deny decisions are tracked in session memory
- tool handlers receive `allowed_domains` and `denied_domains` through runtime context
- runtime summary includes managed-network state plus domain policy lists when present

### Non-Goals

- provider-specific proxy transport
- OS-level network sandbox implementation changes
- attack tooling or offensive-network capability

## Track 3: LSP Semantics

### Current State

- `diagnostics` is the only built-in LSP-facing tool surface.
- diagnostics are invoked ad hoc per call.

### Benchmarks

- `opencode` exposes persistent LSP-backed semantic operations such as definition, references, hover, symbols, implementations, and call hierarchy.
- `codex` uses stronger runtime summaries and tool control-plane messaging to teach the model which semantic tools are active.

### Gaps

- no persistent LSP manager
- no semantic navigation tool family
- no frozen runtime summary slot for semantic capability reporting

### Target Definition

- LSP service layer persists client state across calls
- semantic navigation tools become first-class runtime definitions
- runtime summary exposes semantic capability availability

### Non-Goals

- full LSP implementation in this phase
- editor integration work
- language-server auto-install UX polish

## Exit Criteria For Baseline Freeze

- selected built-in tool definitions are snapshot-tested
- runtime summary output is snapshot-tested
- exec approval key shape is snapshot-tested
- the phase document exists and matches the code baseline
