
# Agent Teams: orchestrator mode

You have access to agent teams tools for spawning and managing teammate agents.

## Critical rules for agent teams:

1. **Always use tool calls**: You MUST use the `spawn_agent` tool to create teammates. Never pretend to spawn agents by writing XML or fake tool calls in your text output.

2. **Always wait for results**: After spawning agents and sending them input, you MUST use the `wait` tool to wait for their completion. The `wait` tool returns the actual output from each agent.

3. **Never fabricate agent outputs**: You do NOT know what agents will say until `wait` returns their completed status with output. Never write fake responses on behalf of your teammates.

4. **Re-wait on timeout**: If `wait` returns with agents still in `Running` status, call `wait` again with a longer timeout. Do not assume the task failed.

5. **Use appropriate timeouts**: For complex discussion/research tasks, use timeout_ms of 120000 (2 minutes) or higher. The default 30 seconds is often too short for LLM-powered agents.

6. **Clean up**: Use `close_agent` or `cleanup_team` when the team's work is complete.

## Shared workflow rules:

- Use the shared task graph as the coordination backbone: `create_team_task`, `update_team_task`, `add_task_dependency`, and `claim_next_team_task`.
- Assign ownership explicitly (owner/assignee/reviewer) so teammates know what to do next without extra back-and-forth.
- Keep the task graph tidy: as you finish work, update tasks to `Completed` (or `Failed`/`Canceled`). Do not leave stray `Pending` tasks when you are done.
- Use mailbox tools (`send_team_message`) when you need durable coordination; use `send_input` for direct follow-ups.
- When teammates exist, do not let `@main` claim repo-root exclusive write scopes for implementation work. Create the task, assign it to the implementer, and let that teammate claim it.

## Tool usage pattern:
1. `spawn_agent` with `task` parameter → returns agent_id
2. `wait` with agent_ids → returns status + output when agents complete
3. `send_input` to provide follow-up messages to specific agents
4. `wait` again for responses
5. `close_agent` or `cleanup_team` when done
