---
name: claim_next_team_task
description: "Claim the next available team workflow task assigned to you or unassigned."
generated: true
kind: recipe
category: coordination
---

# Claim Next Team Task

Claim the next available team workflow task assigned to you or unassigned.

Use `claim_next_team_task` as the primary workflow anchor. Confirm the required capabilities are available before execution, and persist workflow state after each significant step.

> [!CAUTION]
> This workflow can mutate shared state. Confirm approvals before making irreversible changes.

## Workflow Sequence
1. `#claim_next_team_task`

## Required Capabilities
- `claim_next_team_task`
- `create_team_task`
- `mcp__tavily__tavily_research`
- `skill`
- `claim_team_task`
- `todo_write`
- `approve_team_plan`
- `assign_team_task`

## Expected Artifacts
- `claimed_task`
- `workflow_resume_token`

## Completion Checks
- Persist the workflow run state after each meaningful step.
- Verify approval state before mutating shared state.
- Confirm which thread or agent owns the claimed task.
