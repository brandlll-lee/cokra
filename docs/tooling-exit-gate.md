# Tooling Exit Gate

This checklist defines the minimum bar before Cokra can claim the command execution, network, and LSP tool stack is in a mature, dependable state.

## Command execution

- `shell` approval keys are stable and snapshot-covered.
- Safe commands do not prompt unexpectedly.
- Dangerous commands produce explicit approval requirements.
- Command intent extraction covers command prefix, path intents, external paths, and network hints.

## Network capability

- `web_search` exposes provider-native web search when the active model runtime supports it.
- Local `web_search` fallback returns structured citation-ready results with backend metadata.
- `web_fetch`, `web_open_page`, and `web_find_in_page` return structured page metadata.
- `code_search` supports both `scope=local` and `scope=web`.
- Host approval decisions and policy denials are visible through `tool_audit_log`.

## Runtime control plane

- `inspect_tool` includes capability facets for network backends and semantic LSP support.
- `active_tool_status` reports model runtime, native web-search availability, and live LSP client counts.
- Runtime prompt summary tells the model when to prefer `lsp`, `code_search`, `grep_files`, and `web_search`.

## LSP

- Shared LSP clients are reused across repeated requests in the same workspace.
- LSP status and restart tools remain available in the active runtime.
- LSP client lifecycle and auto-install activity are visible through `tool_audit_log`.

## Verification

- Schema snapshots cover `shell`, `unified_exec`, `web_search`, `web_fetch`, and `diagnostics`.
- Runtime summary snapshots cover model-runtime and network-policy rendering.
- Unit tests cover native web-search tool exposure, local/web code search routing, and updated web-fetch/page helpers.
