---
name: plan
description: "Emit a plan item and persist it into workflow state."
generated: true
kind: recipe
category: planning
---

# Plan

Emit a plan item and persist it into workflow state.

Use `plan` as the primary workflow anchor. Confirm the required capabilities are available before execution, and persist workflow state after each significant step.

> [!CAUTION]
> This workflow can mutate shared state. Confirm approvals before making irreversible changes.

## Workflow Sequence
1. `#plan`

## Required Capabilities
- `plan`
- `approve_team_plan`
- `claim_team_task`
- `close_agent`
- `skill`
- `spawn_agent`
- `submit_team_plan`
- `team_status`

## Expected Artifacts
- `plan_text`
- `workflow_resume_token`

## Completion Checks
- Persist the workflow run state after each meaningful step.
- Verify approval state before mutating shared state.
- Ensure the recorded plan is concrete enough for execution or review.
