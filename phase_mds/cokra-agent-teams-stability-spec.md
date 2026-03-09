# Cokra Agent Teams 稳定性与渲染优化 — 技术规格说明书

> **版本**: v1.0  
> **日期**: 2026-03-09  
> **作者**: Cascade (AI Architect)  
> **目标实施者**: GPT-5.4 或同等能力的实施模型  
> **状态**: 待实施

---

## 1. 执行摘要

Cokra 的 Agent Teams 功能存在三个层面的问题：

1. **功能层 — Agent 成员未真正工作**：模型调用 `spawn_agent` 时，子代理实际已被创建并运行（后端逻辑正确），但模型在子代理完成前就"假装"已获得结果，自行编造了辩论内容。这是因为**模型对 `wait` 工具返回的结果处理不当**——当 `wait` 超时返回空结果或子代理仍在运行时，模型没有重新等待，而是自己编造输出。根本原因是系统提示词中缺少对 Agent Teams 工作流程的明确指导。

2. **渲染层 — `<tool_call>`/`<tool_response>` 原始 XML 泄露到 TUI**：如截图 2 所示，模型输出中包含的 `<tool_call>` 和 `<tool_response>` XML 标签被当作普通文本渲染到了聊天面板中。这说明模型在 agent-teams 场景下将工具调用"内联"到了文本输出中（而非使用正式的 tool_use 机制），TUI 未对此进行过滤/隐藏。

3. **视觉层 — 前缀符号与表格渲染**：Cokra 使用 `•`（U+2022 小圆点）作为 AI 消息前缀，而 Claude Code 使用 `●`（U+25CF 实心圆）。此外 Cokra 的 markdown 渲染器完全忽略了表格（Table）标签，导致模型生成的表格无法渲染。

本规格说明书提出针对以上三个问题的具体修复方案，确保 Agent Teams 功能稳定可用、输出美观。

---

## 2. 根因分析

### 2.1 Bug 1：Agent 成员未真正工作（模型自欺）

**现象**：用户截图显示输入框底部显示 `team: 1 members, 0 unread, 0 bg approvals, 0 pending plans`，说明后端确实只注册了 1 个成员（而非预期的 2 个），但更关键的是模型在第一次尝试中完全没有调用任何 tool——它"假装"了整个辩论过程。

**根因 1 — 系统提示词缺失 Agent Teams 使用指导**：

当前 `build_spawned_agent_system_prompt()` 仅为**子代理**设置了简短的角色提示（见 `@f:\CodeHub\leehub\cokra\cokra-rs\core\src\agent\team_runtime.rs:711-741`）。但**主代理**（orchestrator/leader）的系统提示中**完全没有关于 Agent Teams 工具如何正确使用的指导**。

对比 Claude Code：Claude Code 的系统提示中包含详细的 agent-teams 使用指导，告知模型：
- 必须通过 `spawn_agent` 工具实际创建子代理
- 必须使用 `wait` 工具等待子代理完成
- 子代理的输出只能通过 `wait` 的返回值获取
- 不要自己编造子代理的输出

**根因 2 — `wait` 超时行为不够健壮**：

查看 `@f:\CodeHub\leehub\cokra\cokra-rs\core\src\tools\handlers\wait.rs:22-24`：
```rust
const DEFAULT_WAIT_TIMEOUT_MS: i64 = 30_000; // 30秒
```

30 秒对于需要 LLM 推理的子代理来说太短了。当 `wait` 超时时返回的是子代理的当前状态（仍为 `Running`），模型看到 `Running` 后没有重新调用 `wait`，而是自己编造了结果。

对比 Codex（`@f:\CodeHub\leehub\codex\codex-rs\core\src\tools\handlers\multi_agents.rs:49-51`）：
```rust
pub(crate) const DEFAULT_WAIT_TIMEOUT_MS: i64 = 30_000;
pub(crate) const MAX_WAIT_TIMEOUT_MS: i64 = 3600 * 1000; // 1小时
```
Codex 的超时上限为 1 小时，与 cokra 相同，但 Codex 的系统提示会指导模型在超时后重新等待。

**根因 3 — `prompt` 参数别名缺失**：

查看 `SpawnAgentArgs`（`@f:\CodeHub\leehub\cokra\cokra-rs\core\src\tools\handlers\spawn_agent.rs:14-25`）：
```rust
struct SpawnAgentArgs {
  #[serde(alias = "initial_task")]
  task: Option<String>,
  #[serde(alias = "input")]
  message: Option<String>,
  #[serde(alias = "name")]
  nickname: Option<String>,
  ...
}
```

`task` 字段有 `initial_task` 别名，`message` 有 `input` 别名，但缺少 `prompt` 别名。模型（尤其是 Claude）经常使用 `"prompt"` 作为参数名，导致解析时 `task` 和 `message` 都为空，触发错误 `"spawn_agent requires a non-empty task or message"`。当此错误发生在**模型以内联 XML 方式调用工具时**，错误被吞没，模型继续自行编造。

### 2.2 Bug 2：`<tool_call>`/`<tool_response>` XML 原始泄露

**现象**：截图 2 显示 `<tool_call>{"name": "spawn_agent", ...}</tool_call>` 和 `<tool_response>` 被直接渲染到聊天面板。

**根因**：某些模型（特别是 Claude 系列通过 Anthropic API 使用时）在 agent-teams 场景下，不通过正式的 `tool_use` content block 调用工具，而是在 `text` content block 中以 XML 格式"内联"工具调用。这些 XML 标签在 streaming delta 中被当作普通文本推送到 TUI。

cokra 的 `on_agent_message_delta()` 直接将所有文本 delta 推入 `StreamController`，没有对 `<tool_call>` / `<tool_response>` XML 进行任何过滤。

对比 Claude Code：Claude Code 不会泄露这些 XML 标签——它在 streaming 层面就将内联工具调用解析为正式的工具调用事件。

### 2.3 Bug 3：前缀符号 `•` vs `●` 与表格渲染

**前缀问题**：

`AgentMessageCell::display_lines()` 中（`@f:\CodeHub\leehub\cokra\cokra-rs\tui\src\history_cell.rs:652-663`）：
```rust
.initial_indent(if self.is_first_line {
  "• ".dim().into()   // U+2022 BULLET (小圆点)
} else {
  "  ".into()
})
```

Claude Code 使用 `●`（U+25CF BLACK CIRCLE），视觉上更醒目。

同样，`notice_events.rs` 中大量使用 `"• "` 前缀（如 `@f:\CodeHub\leehub\cokra\cokra-rs\tui\src\chatwidget\notice_events.rs:74-84`），以及 `multi_agents.rs` 的 `title_text()` 和 `title_with_agent()` 函数（`@f:\CodeHub\leehub\cokra\cokra-rs\tui\src\multi_agents.rs:420-429`）。

**表格渲染问题**：

`markdown_render.rs` 中（`@f:\CodeHub\leehub\cokra\cokra-rs\tui\src\markdown_render.rs:195-207`）：
```rust
Tag::Table(_)
| Tag::TableHead
| Tag::TableRow
| Tag::TableCell
| Tag::Image { .. } => {}  // 完全忽略！
```

表格标签被直接忽略（空实现），导致模型生成的 markdown 表格完全不渲染。

---

## 3. 解决方案设计

### 3.1 架构概览

```
修复分为三个独立且可并行的工作流：

[工作流 A] Agent Teams 功能修复
  ├── A1. 增强主代理系统提示词（Agent Teams 使用指导）
  ├── A2. spawn_agent 参数别名扩展
  ├── A3. wait 超时默认值提高
  └── A4. 增强子代理完成事件传递

[工作流 B] XML 泄露过滤
  ├── B1. streaming delta 层 XML 标签过滤
  └── B2. 静态消息层 XML 过滤

[工作流 C] 视觉对齐
  ├── C1. 前缀符号 • → ●
  ├── C2. Markdown 表格渲染
  └── C3. notice_events 前缀统一
```

---

## 4. 详细变更清单

### 4.1 工作流 A — Agent Teams 功能修复

#### A1. 增强主代理系统提示词

**文件**: `cokra-rs/core/src/agent/team_runtime.rs`  
**函数**: 新增 `build_leader_agent_teams_prompt_suffix()`  
**位置**: 在 `build_spawned_agent_system_prompt()` 同一文件中

**变更理由**：主代理（orchestrator）缺少关于如何正确使用 agent-teams 工具的指导。

**新增函数**：
```rust
/// 为主代理（leader/orchestrator）构建 agent-teams 系统提示后缀。
/// 当 agent-teams 功能启用时，追加到主代理的系统提示末尾。
pub(crate) fn build_leader_agent_teams_prompt_suffix() -> &'static str {
  r#"
# Agent Teams: orchestrator mode

You have access to agent teams tools for spawning and managing teammate agents.

## Critical rules for agent teams:

1. **Always use tool calls**: You MUST use the `spawn_agent` tool to create teammates. Never pretend to spawn agents by writing XML or fake tool calls in your text output.

2. **Always wait for results**: After spawning agents and sending them input, you MUST use the `wait` tool to wait for their completion. The `wait` tool returns the actual output from each agent.

3. **Never fabricate agent outputs**: You do NOT know what agents will say until `wait` returns their completed status with output. Never write fake responses on behalf of your teammates.

4. **Re-wait on timeout**: If `wait` returns with agents still in `Running` status, call `wait` again with a longer timeout. Do not assume the task failed.

5. **Use appropriate timeouts**: For complex discussion/research tasks, use timeout_ms of 120000 (2 minutes) or higher. The default 30 seconds is often too short for LLM-powered agents.

6. **Clean up**: Use `close_agent` or `cleanup_team` when the team's work is complete.

## Tool usage pattern:
1. `spawn_agent` with `task` parameter → returns agent_id
2. `wait` with agent_ids → returns status + output when agents complete
3. `send_input` to provide follow-up messages to specific agents
4. `wait` again for responses
5. `close_agent` or `cleanup_team` when done
"#
}
```

**集成点**：需要在主代理的系统提示构建流程中调用此函数。查找主代理系统提示的构建位置。

**文件**: `cokra-rs/core/src/turn/executor.rs` 或 `cokra-rs/core/src/cokra.rs`  
**变更**：在构建系统提示时，如果 agent-teams 功能已启用（即 `config.agents.enabled` 为 true 或相关检测），追加 `build_leader_agent_teams_prompt_suffix()` 的内容到系统提示末尾。

**实现建议**：
1. 在 `TurnConfig` 中检查是否启用了 agent-teams（可通过检查 `config.agents.max_threads > 0` 或 `config.agents.enabled`）。
2. 在构建 `system_prompt` 字符串时，如果启用，追加后缀。
3. 具体位置需要实施者查找 `system_prompt` 的最终拼接点——可能在 `cokra.rs` 的 `start()` 或 `build_turn_config()` 中。

#### A2. `spawn_agent` 参数别名扩展

**文件**: `cokra-rs/core/src/tools/handlers/spawn_agent.rs`  
**结构体**: `SpawnAgentArgs`  
**行号**: 14-25

**当前代码**:
```rust
struct SpawnAgentArgs {
  #[serde(alias = "initial_task")]
  task: Option<String>,
  #[serde(alias = "input")]
  message: Option<String>,
  #[serde(alias = "name")]
  nickname: Option<String>,
  role: Option<String>,
  #[serde(alias = "type")]
  agent_type: Option<String>,
}
```

**修改为**:
```rust
struct SpawnAgentArgs {
  #[serde(alias = "initial_task")]
  task: Option<String>,
  #[serde(alias = "input", alias = "prompt")]
  message: Option<String>,
  #[serde(alias = "name")]
  nickname: Option<String>,
  role: Option<String>,
  #[serde(alias = "type")]
  agent_type: Option<String>,
}
```

**变更说明**：为 `message` 字段添加 `alias = "prompt"` 别名。Claude 系列模型经常使用 `prompt` 作为参数名。

**参考**: Codex 的 `SpawnAgentArgs`（`@f:\CodeHub\leehub\codex\codex-rs\core\src\tools\handlers\multi_agents.rs:110-117`）同时支持 `message` 和 `items` 字段。

#### A3. `wait` 默认超时提高

**文件**: `cokra-rs/core/src/tools/handlers/wait.rs`  
**行号**: 22-24

**当前代码**:
```rust
const MIN_WAIT_TIMEOUT_MS: i64 = 10_000;
const DEFAULT_WAIT_TIMEOUT_MS: i64 = 30_000;
const MAX_WAIT_TIMEOUT_MS: i64 = 3_600_000;
```

**修改为**:
```rust
const MIN_WAIT_TIMEOUT_MS: i64 = 10_000;
const DEFAULT_WAIT_TIMEOUT_MS: i64 = 120_000;   // 2分钟，LLM子代理需要更长时间
const MAX_WAIT_TIMEOUT_MS: i64 = 3_600_000;
```

**变更说明**：从 30 秒提高到 120 秒。LLM 子代理处理复杂任务（如辩论、研究）通常需要 60-120 秒。30 秒的超时导致 `wait` 频繁返回 `Running` 状态，模型未能正确处理此情况。

#### A4. 增强子代理完成通知

**文件**: `cokra-rs/core/src/agent/team_runtime.rs`  
**函数**: `launch_spawned_agent()`  
**行号**: 665-698（tokio::spawn 块）

**当前问题**：子代理完成后，状态通过 `watch::channel` 更新为 `Completed`，但主代理的 `wait` 工具依赖于在 deadline 内轮询此 channel。如果模型没有调用 `wait` 或 `wait` 已超时返回，完成通知可能丢失。

**建议变更**：在子代理完成后，主动向 `root_tx_event` 发送一个 `BackgroundEvent`，通知用户/主代理子代理已完成。这样即使模型没有主动 `wait`，TUI 也会显示完成通知。

在 `tokio::spawn` 块的 `ChildCommand::UserTurn` 分支中，在 `status_tx.send(Completed/Errored)` 之后，添加：

```rust
// 通知主代理的 TUI 子代理已完成
// （此处需要 clone root_tx_event 进 spawn 块）
if let Some(ref tx) = root_tx_event_clone {
    let nickname_display = thread_info_nickname.clone().unwrap_or_else(|| "agent".to_string());
    let bg_msg = match &final_message {
        Some(_) => format!("@{nickname_display} 已完成任务"),
        None => format!("@{nickname_display} 已完成（无输出）"),
    };
    let _ = tx.send(EventMsg::BackgroundEvent(
        cokra_protocol::BackgroundEventPayload { message: bg_msg }
    )).await;
}
```

**实施细节**：
1. 在 `launch_spawned_agent()` 中，clone `self.root_tx_event` 和 `thread_info` 的 nickname 到 spawn 块中。
2. 在 `ChildCommand::UserTurn` 的 `Ok(result)` 和 `Err(err)` 分支中，发送 `BackgroundEvent`。

---

### 4.2 工作流 B — XML 泄露过滤

#### B1. Streaming Delta 层 XML 标签过滤

**文件**: `cokra-rs/tui/src/chatwidget/stream_events.rs`  
**函数**: `on_agent_message_delta()`  
**行号**: 7-24

**当前代码**:
```rust
pub(super) fn on_agent_message_delta(&mut self, item_id: &str, delta: &str) {
    self.transcript.streamed_agent_item_ids.insert(item_id.to_string());
    let is_new = self.transcript.stream_controller.is_none();
    let controller = self.transcript.stream_controller
        .get_or_insert_with(|| crate::streaming::controller::StreamController::new(None));
    let committed = controller.push(delta);
    // ...
}
```

**修改方案**：在 `controller.push(delta)` 之前，对 delta 进行 XML 标签过滤。

**新增辅助函数**（建议放在 `stream_events.rs` 或新建 `cokra-rs/tui/src/xml_filter.rs`）：

```rust
/// 从 streaming 文本中过滤内联工具调用的 XML 标签。
///
/// 某些模型（特别是 Claude 系列）会在文本输出中以 XML 格式内联工具调用，
/// 例如 `<tool_call>{"name":"spawn_agent",...}</tool_call>` 和 `<tool_response>...</tool_response>`。
/// 这些不应显示在聊天面板中。
pub(crate) struct XmlToolFilter {
    buffer: String,
    /// 当我们处于一个正在匹配的标签内时为 true
    in_tag: bool,
    /// 当前正在匹配的标签名（如 "tool_call", "tool_response"）
    tag_name: Option<String>,
}

const FILTERED_XML_TAGS: &[&str] = &["tool_call", "tool_response", "function_call", "function_response"];

impl XmlToolFilter {
    pub(crate) fn new() -> Self {
        Self {
            buffer: String::new(),
            in_tag: false,
            tag_name: None,
        }
    }

    /// 处理输入 delta，返回应该显示的文本。
    /// 
    /// 策略：
    /// - 检测 `<tool_call>` 开标签 → 进入过滤模式，不输出
    /// - 在过滤模式中 → 缓冲内容，不输出
    /// - 检测到对应闭标签 `</tool_call>` → 退出过滤模式，丢弃缓冲
    /// - 如果缓冲超过合理长度且未找到闭标签 → flush 缓冲（容错）
    pub(crate) fn filter(&mut self, delta: &str) -> String {
        let mut output = String::new();
        self.buffer.push_str(delta);

        loop {
            if self.in_tag {
                // 寻找闭标签
                let close_tag = format!("</{}>", self.tag_name.as_deref().unwrap_or(""));
                if let Some(end_pos) = self.buffer.find(&close_tag) {
                    // 丢弃整个标签内容
                    self.buffer = self.buffer[end_pos + close_tag.len()..].to_string();
                    self.in_tag = false;
                    self.tag_name = None;
                    continue;
                }
                // 闭标签尚未到达，继续缓冲
                // 安全阀：如果缓冲超过 32KB，flush 它（防止内存泄漏）
                if self.buffer.len() > 32 * 1024 {
                    output.push_str(&self.buffer);
                    self.buffer.clear();
                    self.in_tag = false;
                    self.tag_name = None;
                }
                break;
            }

            // 寻找开标签
            let mut found = false;
            for tag_name in FILTERED_XML_TAGS {
                let open_tag = format!("<{}", tag_name);
                if let Some(start_pos) = self.buffer.find(&open_tag) {
                    // 检查标签是否完整（有 > 结尾）
                    let rest = &self.buffer[start_pos + open_tag.len()..];
                    let tag_end = rest.find('>');
                    if let Some(tag_end_pos) = tag_end {
                        // 输出标签之前的文本
                        output.push_str(&self.buffer[..start_pos]);
                        // 进入过滤模式
                        self.in_tag = true;
                        self.tag_name = Some(tag_name.to_string());
                        self.buffer = self.buffer[start_pos + open_tag.len() + tag_end_pos + 1..].to_string();
                        found = true;
                        break;
                    } else {
                        // 标签开头存在但尚未完整，保留缓冲等待更多数据
                        if start_pos > 0 {
                            output.push_str(&self.buffer[..start_pos]);
                            self.buffer = self.buffer[start_pos..].to_string();
                        }
                        found = true;
                        break;
                    }
                }
            }

            if !found {
                // 没有匹配的标签，检查是否有可能的部分匹配
                // 保留最后 20 个字符以处理跨 delta 的标签
                let safe_len = self.buffer.len().saturating_sub(20);
                if safe_len > 0 {
                    output.push_str(&self.buffer[..safe_len]);
                    self.buffer = self.buffer[safe_len..].to_string();
                }
                break;
            }
        }

        output
    }

    /// Flush 所有剩余缓冲内容（在消息结束时调用）。
    pub(crate) fn flush(&mut self) -> String {
        let remaining = std::mem::take(&mut self.buffer);
        self.in_tag = false;
        self.tag_name = None;
        remaining
    }
}
```

**集成到 `stream_events.rs`**：

```rust
pub(super) fn on_agent_message_delta(&mut self, item_id: &str, delta: &str) {
    self.transcript.streamed_agent_item_ids.insert(item_id.to_string());

    // 过滤内联 XML 工具调用标签
    let xml_filter = self.transcript.xml_tool_filter
        .get_or_insert_with(XmlToolFilter::new);
    let filtered_delta = xml_filter.filter(delta);
    if filtered_delta.is_empty() {
        return; // 整个 delta 被过滤掉了
    }

    let is_new = self.transcript.stream_controller.is_none();
    let controller = self.transcript.stream_controller
        .get_or_insert_with(|| crate::streaming::controller::StreamController::new(None));
    let committed = controller.push(&filtered_delta);
    // ... 其余逻辑不变
}
```

**需要在 `TranscriptState`（或等效结构体）中新增字段**：
```rust
pub(crate) xml_tool_filter: Option<XmlToolFilter>,
```

并在 turn 结束（`on_turn_complete`）时 flush + 重置。

#### B2. 静态消息层 XML 过滤

**文件**: `cokra-rs/tui/src/chatwidget/stream_events.rs`  
**函数**: `on_agent_message()`  
**行号**: 26-43

对于非 streaming 的完整消息（`AgentMessage` 事件），同样需要过滤 XML 标签。

在 `on_agent_message()` 中，对每个 `AgentMessageContent::Text { text }` 的 `text` 进行过滤：

```rust
AgentMessageContent::Text { text } => {
    let filtered = strip_inline_xml_tool_tags(text);
    if !filtered.trim().is_empty() {
        lines.push(Line::from(filtered));
    }
}
```

**新增辅助函数**（比 streaming 版本简单，因为是完整文本）：
```rust
/// 从完整文本中移除内联工具调用 XML 标签及其内容。
fn strip_inline_xml_tool_tags(text: &str) -> String {
    let mut result = text.to_string();
    for tag_name in &["tool_call", "tool_response", "function_call", "function_response"] {
        loop {
            let open = format!("<{}", tag_name);
            let close = format!("</{}>", tag_name);
            let Some(start) = result.find(&open) else { break };
            let Some(open_end) = result[start..].find('>') else { break };
            let search_from = start + open_end + 1;
            let Some(end) = result[search_from..].find(&close) else { break };
            let remove_end = search_from + end + close.len();
            result = format!("{}{}", &result[..start], &result[remove_end..]);
        }
    }
    result
}
```

---

### 4.3 工作流 C — 视觉对齐

#### C1. 前缀符号统一：`•` → `●`

**变更范围**：全局替换所有 TUI 渲染中的 `"• "` 为 `"● "`。

**受影响的文件和位置**：

| 文件 | 位置 | 当前值 | 新值 |
|------|------|--------|------|
| `tui/src/history_cell.rs` | `AgentMessageCell::display_lines()` L657 | `"• ".dim()` | `"● ".dim()` |
| `tui/src/multi_agents.rs` | `title_text()` L421 | `"• ".dim()` | `"● ".dim()` |
| `tui/src/multi_agents.rs` | `title_with_agent()` L427 | `"• ".dim()` | `"● ".dim()` |
| `tui/src/chatwidget/notice_events.rs` | 多处（L34-171） | 所有 `"• "` 前缀 | `"● "` 前缀 |
| `tui/src/chatwidget/mod.rs` | `ThreadNameUpdated` L218 | `"• Thread renamed: "` | `"● Thread renamed: "` |

**实施策略**：在以上文件中使用全局查找替换 `"• "` → `"● "`。注意仅替换**渲染前缀**中的 bullet，不要替换测试断言或注释中的。

**技术细节**：
- `•` = U+2022 BULLET
- `●` = U+25CF BLACK CIRCLE
- 两者在终端中都占 1 个字宽，不影响布局计算

#### C2. Markdown 表格渲染

**文件**: `cokra-rs/tui/src/markdown_render.rs`  
**位置**: `start_tag()` L195-207 和 `end_tag()` L224-236

**当前状态**：Table 相关标签完全被忽略（空匹配臂）。

**修改方案**：实现基于 Unicode box-drawing 字符的表格渲染，与 Claude Code 的终端表格风格对齐。

**新增字段到 `Writer` 结构体**：
```rust
// 表格渲染状态
table_state: Option<TableRenderState>,
```

```rust
struct TableRenderState {
    /// 当前表格的所有行数据：Vec<Vec<String>>
    /// 第一行是表头
    rows: Vec<Vec<String>>,
    /// 当前正在收集的行
    current_row: Vec<String>,
    /// 当前单元格的文本缓冲
    current_cell: String,
    /// 是否在表头区域
    in_head: bool,
    /// 对齐方式
    alignments: Vec<pulldown_cmark::Alignment>,
}
```

**表格渲染逻辑**：

```rust
fn start_table(&mut self, alignments: Vec<pulldown_cmark::Alignment>) {
    if self.needs_newline {
        self.push_blank_line();
    }
    self.table_state = Some(TableRenderState {
        rows: Vec::new(),
        current_row: Vec::new(),
        current_cell: String::new(),
        in_head: false,
        alignments,
    });
}

fn start_table_head(&mut self) {
    if let Some(ref mut state) = self.table_state {
        state.in_head = true;
    }
}

fn end_table_head(&mut self) {
    if let Some(ref mut state) = self.table_state {
        state.in_head = false;
    }
}

fn start_table_row(&mut self) {
    if let Some(ref mut state) = self.table_state {
        state.current_row = Vec::new();
    }
}

fn end_table_row(&mut self) {
    if let Some(ref mut state) = self.table_state {
        let row = std::mem::take(&mut state.current_row);
        state.rows.push(row);
    }
}

fn start_table_cell(&mut self) {
    if let Some(ref mut state) = self.table_state {
        state.current_cell = String::new();
    }
}

fn end_table_cell(&mut self) {
    if let Some(ref mut state) = self.table_state {
        let cell = std::mem::take(&mut state.current_cell);
        state.current_row.push(cell.trim().to_string());
    }
}

fn end_table(&mut self) {
    let Some(state) = self.table_state.take() else { return };
    if state.rows.is_empty() { return; }

    // 计算每列的最大宽度
    let col_count = state.rows.iter().map(|row| row.len()).max().unwrap_or(0);
    let mut col_widths = vec![0usize; col_count];
    for row in &state.rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                // 使用 unicode_width 计算实际显示宽度
                let width = cell.chars().count(); // 简化：按字符数计算
                col_widths[i] = col_widths[i].max(width);
            }
        }
    }

    // 确保最小列宽
    for w in col_widths.iter_mut() {
        *w = (*w).max(3);
    }

    let style = self.styles.table_border; // 需要新增此样式

    // 渲染顶部边框: ┌──────┬──────┐
    let top = render_table_border(&col_widths, '┌', '┬', '┐', '─');
    self.push_line(Line::from(Span::styled(top, style)));

    for (row_idx, row) in state.rows.iter().enumerate() {
        // 渲染数据行: │ cell │ cell │
        let mut spans = Vec::new();
        spans.push(Span::styled("│", style));
        for (col_idx, width) in col_widths.iter().enumerate() {
            let cell_text = row.get(col_idx).map(|s| s.as_str()).unwrap_or("");
            let padded = pad_cell(cell_text, *width);
            spans.push(Span::from(format!(" {} ", padded)));
            spans.push(Span::styled("│", style));
        }
        self.push_line(Line::from(spans));

        // 表头后渲染分隔线: ├──────┼──────┤
        if row_idx == 0 && state.rows.len() > 1 {
            let sep = render_table_border(&col_widths, '├', '┼', '┤', '─');
            self.push_line(Line::from(Span::styled(sep, style)));
        }
    }

    // 渲染底部边框: └──────┴──────┘
    let bottom = render_table_border(&col_widths, '└', '┴', '┘', '─');
    self.push_line(Line::from(Span::styled(bottom, style)));

    self.needs_newline = true;
}
```

**辅助函数**：
```rust
fn render_table_border(
    col_widths: &[usize],
    left: char,
    mid: char,
    right: char,
    fill: char,
) -> String {
    let mut s = String::new();
    s.push(left);
    for (i, width) in col_widths.iter().enumerate() {
        // +2 for padding spaces
        for _ in 0..(*width + 2) {
            s.push(fill);
        }
        if i < col_widths.len() - 1 {
            s.push(mid);
        }
    }
    s.push(right);
    s
}

fn pad_cell(text: &str, width: usize) -> String {
    let text_width = text.chars().count();
    if text_width >= width {
        text.to_string()
    } else {
        format!("{}{}", text, " ".repeat(width - text_width))
    }
}
```

**在 `MarkdownStyles` 中新增**：
```rust
pub(crate) table_border: Style,
```

默认值：`Style::default().dim()`（与 Claude Code 的灰色边框一致）。

**修改 `start_tag()` 和 `end_tag()`**：

将表格标签从空匹配臂中移出，替换为实际调用：

```rust
// start_tag:
Tag::Table(alignments) => self.start_table(alignments.to_vec()),
Tag::TableHead => self.start_table_head(),
Tag::TableRow => self.start_table_row(),
Tag::TableCell => self.start_table_cell(),

// end_tag:
TagEnd::Table => self.end_table(),
TagEnd::TableHead => self.end_table_head(),
TagEnd::TableRow => self.end_table_row(),
TagEnd::TableCell => self.end_table_cell(),
```

**表格内文本收集**：在 `Event::Text` 处理中，如果 `table_state` 存在且 `current_cell` 正在收集，将文本追加到 `current_cell`：

```rust
Event::Text(text) => {
    if let Some(ref mut table_state) = self.table_state {
        table_state.current_cell.push_str(&text);
        return; // 不走普通文本渲染流程
    }
    // ... 原有逻辑
}
```

#### C3. notice_events 前缀统一

此项已包含在 C1 中。所有 `notice_events.rs` 中的 `"• "` 前缀将统一替换为 `"● "`。

---

## 5. 参考模式

### 5.1 Claude Code Agent Teams（闭源参考 — 行为层面）

从用户提供的 Claude Code 输出截图分析：

| 特性 | Claude Code | Cokra 当前 | 目标 |
|------|------------|-----------|------|
| 消息前缀 | `●` (U+25CF) | `•` (U+2022) | 改为 `●` |
| 表格渲染 | Unicode box-drawing | 不渲染（忽略） | 实现 box-drawing 表格 |
| Agent 创建 | 通过正式 tool_use 机制 | 模型有时内联 XML | 过滤 XML + 改进提示词 |
| Agent 输出 | 通过 wait 工具获取真实结果 | 模型自行编造 | 系统提示强制要求 wait |
| 状态显示 | `2 agents launched (ctrl+o to expand)` 树形 | `team: 1 members...` 底部栏 | 保持底部栏但改进通知 |
| 工具调用可见性 | 完全隐藏 XML | 泄露 XML 到聊天 | 过滤掉 |

### 5.2 Codex Multi-Agents（`codex-rs` — 代码层面参考）

**核心架构差异**：

| 方面 | Codex | Cokra | 建议 |
|------|-------|-------|------|
| AgentControl | 集中式 `AgentControl` 结构体，持有 `ThreadManagerState` 弱引用 | 分散的 `TeamRuntime` + 全局 `OnceLock` 注册表 | 保持 cokra 当前设计（已可用） |
| spawn_agent 工具 | 统一的 `MultiAgentHandler` 处理所有 collab 工具 | 每个工具一个 Handler 文件 | 保持 cokra 当前设计 |
| TUI 渲染 | `multi_agents.rs` 统一 collab 事件渲染 | 类似的 `multi_agents.rs` | 已基本对齐 |
| 前缀符号 | `"• "` (Codex 也用小圆点) | `"• "` | 改为 `"● "` 以对齐 Claude Code |

**从 Codex 借鉴的模式**：

1. **`XmlToolFilter` 思路**：Codex 不需要此过滤器因为它始终通过正式的 tool_use 机制调用工具。但 cokra 需要因为它对接多种模型 provider，某些 provider 的模型会内联 XML。

2. **系统提示指导**：Codex 通过 `build_agent_spawn_config()` 为每个子代理构建完整的配置，包括限制子代理的工具集。Cokra 已有类似机制（`build_spawned_agent_system_prompt()`），但缺少主代理的指导。

### 5.3 Gastown（多 Agent 编排 — 架构参考）

Gastown 采用完全不同的架构：通过 tmux 会话管理独立的 Claude Code 实例，使用 beads（本地数据库）进行状态跟踪和 mail 系统进行代理间通信。

**与 cokra 的关系**：Gastown 是**外部编排器**，cokra 是**内部编排器**（所有子代理在同一进程内）。Gastown 的架构对本次修复不直接适用，但其以下设计思想值得记录：

1. **重试与容错**：Gastown 的 `createAgentBeadWithRetry()` 使用指数退避重试（10 次，500ms 基础退避）。Cokra 的 `wait` 工具可以借鉴此模式在内部实现超时重试。

2. **状态持久化**：Gastown 通过 Dolt 数据库持久化代理状态，cokra 通过 `StateDb` + JSON 持久化 `TeamState`。两者都支持跨会话恢复。

---

## 6. 实施注意事项

### 6.1 实施顺序建议

```
Phase 1 (最高优先级 — 功能修复):
  A2 → A3 → A1 → A4
  
Phase 2 (高优先级 — 渲染清理):
  B1 → B2
  
Phase 3 (中优先级 — 视觉优化):
  C1 → C2
```

**理由**：
- A2/A3 是最小成本最大收益的修复（1 行改动）
- A1 是最关键的功能修复（需要新增系统提示内容）
- B1/B2 解决视觉噪音问题
- C1/C2 是视觉增强

### 6.2 边界情况与陷阱

1. **XmlToolFilter 跨 delta 匹配**：XML 标签可能跨多个 streaming delta 到达（如 `<tool_` 在一个 delta，`call>` 在下一个 delta）。`XmlToolFilter` 必须维护缓冲区来处理此情况。已在设计中通过保留尾部 20 字符来处理。

2. **表格中的 CJK 字符宽度**：中文/日文字符在终端中通常占 2 个字宽，但 `chars().count()` 只按字符数计算。正确实现应使用 `unicode-width` crate 的 `UnicodeWidthStr::width()`。如果项目中已有此依赖则直接使用，否则暂用 `chars().count()` 作为简化实现。

3. **前缀替换的测试影响**：`multi_agents.rs` 中的测试断言（如 L785-793）检查渲染输出的文本内容。替换 `"• "` 为 `"● "` 后，这些测试可能不受影响（因为测试断言通常不检查前缀符号），但需要验证。

4. **系统提示长度**：`build_leader_agent_teams_prompt_suffix()` 大约增加 ~800 tokens 到系统提示。考虑到系统提示通常已有数千 tokens，这是可接受的。

5. **`XmlToolFilter` 不应过滤用户消息**：过滤器只应用于 `on_agent_message_delta()` 和 `on_agent_message()`（AI 输出），不应影响 `UserHistoryCell`（用户输入）。

6. **表格宽度超出终端宽度**：当表格内容过宽时，应进行截断。可以在 `pad_cell()` 中添加最大宽度限制（如 `min(width, terminal_width / col_count - 4)`）。

### 6.3 不要做的事情

1. **不要重构 `TeamRuntime` 架构**：当前的全局 `OnceLock` 注册表设计虽然不够优雅，但功能正确。本次修复不涉及架构重构。

2. **不要修改子代理的工具集**：子代理应继续继承主代理的完整工具集。限制工具集可能导致某些合法用例失败。

3. **不要在 core 层面拦截 XML**：XML 过滤应只在 TUI 渲染层进行。Core 层面应保持对模型输出的原始忠实。

---

## 7. 验收标准

### 7.1 功能验收（Agent Teams 稳定性）

| ID | 测试场景 | 预期结果 | 验证方法 |
|----|---------|---------|---------|
| F1 | 用户请求创建 2 个 agent 进行辩论 | `spawn_agent` 被调用 2 次，底部栏显示 `team: 2 members` | 手动测试 |
| F2 | 模型使用 `prompt` 参数名调用 `spawn_agent` | 参数正确解析为 `message` 字段 | 单元测试 |
| F3 | `wait` 默认等待 120 秒 | 子代理有足够时间完成任务 | 代码审查 |
| F4 | 子代理完成后 TUI 显示通知 | 聊天面板出现 `@nickname 已完成任务` | 手动测试 |
| F5 | 主代理通过 `wait` 获取子代理真实输出 | 不再自行编造辩论内容 | 手动测试 |

### 7.2 渲染验收（XML 过滤）

| ID | 测试场景 | 预期结果 | 验证方法 |
|----|---------|---------|---------|
| R1 | 模型输出包含 `<tool_call>...</tool_call>` | XML 标签不显示在聊天面板 | 单元测试 + 手动测试 |
| R2 | 模型输出包含 `<tool_response>...</tool_response>` | XML 标签不显示在聊天面板 | 单元测试 |
| R3 | XML 标签跨多个 streaming delta | 标签仍被正确过滤 | 单元测试 |
| R4 | 正常文本中包含 `<` 和 `>` 字符 | 不被错误过滤 | 单元测试 |

### 7.3 视觉验收

| ID | 测试场景 | 预期结果 | 验证方法 |
|----|---------|---------|---------|
| V1 | AI 消息前缀 | 显示 `●` 而非 `•` | 视觉检查 |
| V2 | Markdown 表格渲染 | 表格以 Unicode box-drawing 边框渲染 | 手动测试 |
| V3 | 表格中包含 CJK 字符 | 列对齐正确 | 手动测试 |
| V4 | notice 事件前缀 | 所有通知使用 `●` 前缀 | 代码审查 |

### 7.4 回归验收

| ID | 测试场景 | 预期结果 | 验证方法 |
|----|---------|---------|---------|
| G1 | 现有 `cargo test -p cokra-tui` 全部通过 | 无回归 | CI |
| G2 | 现有 `cargo test -p cokra-core` 全部通过 | 无回归 | CI |
| G3 | 非 agent-teams 场景下的正常聊天 | 不受影响 | 手动测试 |
| G4 | 普通文本中的 `<` 和 `>` | 不被过滤 | 单元测试 |

---

## 8. 超出范围

以下项目明确**不在**本次修复范围内：

1. ❌ **TeamRuntime 架构重构**（全局 OnceLock → DI 注入）
2. ❌ **子代理工具集限制**（如禁止子代理再 spawn 子代理）
3. ❌ **Agent Teams 的 TUI 交互增强**（如 `/agent` 命令、agent picker 面板）
4. ❌ **Agent Teams 的任务分配 UI**（任务板 TUI 渲染）
5. ❌ **Gastown 风格的外部编排集成**（tmux 会话管理）
6. ❌ **模型 provider 层面的 tool_use 机制修复**（强制模型使用正式 tool_use 而非内联 XML）
7. ❌ **Markdown 图片渲染**（`Tag::Image` 仍保持忽略）
8. ❌ **Agent Teams 的持久化/恢复增强**
9. ❌ **前缀符号的颜色定制**（保持 `.dim()` 样式）
10. ❌ **表格的高级功能**（列排序、可交互表格）

---

## 附录 A：文件变更清单汇总

| 文件 | 变更类型 | 工作流 | 描述 |
|------|---------|--------|------|
| `core/src/tools/handlers/spawn_agent.rs` | 修改 | A2 | `message` 字段增加 `alias = "prompt"` |
| `core/src/tools/handlers/wait.rs` | 修改 | A3 | `DEFAULT_WAIT_TIMEOUT_MS` 30s → 120s |
| `core/src/agent/team_runtime.rs` | 修改 | A1, A4 | 新增 leader 提示后缀 + 子代理完成 BackgroundEvent |
| `core/src/turn/executor.rs` 或 `core/src/cokra.rs` | 修改 | A1 | 集成 leader 提示后缀到系统提示构建流程 |
| `tui/src/xml_filter.rs` | **新增** | B1 | `XmlToolFilter` 结构体 |
| `tui/src/chatwidget/stream_events.rs` | 修改 | B1 | 集成 XmlToolFilter 到 streaming delta |
| `tui/src/chatwidget/mod.rs` | 修改 | B2, C1 | 静态消息 XML 过滤 + 前缀替换 |
| `tui/src/history_cell.rs` | 修改 | C1 | `"• "` → `"● "` |
| `tui/src/multi_agents.rs` | 修改 | C1 | `"• "` → `"● "` |
| `tui/src/chatwidget/notice_events.rs` | 修改 | C1 | `"• "` → `"● "` |
| `tui/src/markdown_render.rs` | 修改 | C2 | 实现表格渲染逻辑 |
| `tui/src/lib.rs` 或 `tui/src/mod.rs` | 修改 | B1 | 注册 `xml_filter` 模块 |

---

## 附录 B：WSL 构建验证命令

```bash
# 在 WSL Ubuntu 中运行
cd /mnt/f/CodeHub/leehub/cokra/cokra-rs

# Core 测试
$HOME/.cargo/bin/cargo test -p cokra-core

# TUI 测试
$HOME/.cargo/bin/cargo test -p cokra-tui

# 编译检查（不运行测试）
$HOME/.cargo/bin/cargo check -p cokra-core -p cokra-tui

# 手动集成测试
$HOME/.cargo/bin/cargo run --bin cokra --
```

---

*EOF — 本规格说明书是自包含的，实施者无需额外澄清即可完整实施。*
