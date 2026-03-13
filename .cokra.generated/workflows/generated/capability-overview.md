---
name: capability-overview
description: "Inventory the currently registered capabilities, adapters, and workflow surfaces before deciding what to execute."
generated: true
kind: recipe
category: inventory
---

# Capability Overview

Inventory the currently registered capabilities, adapters, and workflow surfaces before deciding what to execute.

Start from the capability catalog instead of repo documentation. Read resources first, then decide whether any callable tool is actually needed.

## Workflow Sequence
1. `#plan`

## Required Capabilities
- `inspect_tool`
- `mcp__tavily__tavily_search`
- `search_tool`
- `list_mcp_resource_templates`
- `list_mcp_resources`
- `mcp__tavily__tavily_crawl`
- `mcp__tavily__tavily_extract`
- `mcp__tavily__tavily_map`

## Expected Artifacts
- `capability_inventory`
- `adapter_summary`

## Completion Checks
- List callable tools separately from resources and resource templates.
- Call out which capabilities mutate state or require approval.
