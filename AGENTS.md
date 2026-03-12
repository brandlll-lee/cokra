# Rust/cokra-rs

In the cokra-rs folder where the rust code lives:

- Crate names are prefixed with `cokra-`. For example, the `core` folder's crate is named `cokra-core`
- When using format! and you can inline variables into {}, always do that.
- Install any commands the repo relies on (for example `just`, `rg`, or `cargo-insta`) if they aren't already available before running instructions here.
- Never add or modify any code related to `COKRA_SANDBOX_NETWORK_DISABLED_ENV_VAR` or `COKRA_SANDBOX_ENV_VAR`.
  - You operate in a sandbox where `COKRA_SANDBOX_NETWORK_DISABLED=1` will be set whenever you use the `shell` tool. Any existing code that uses `COKRA_SANDBOX_NETWORK_DISABLED_ENV_VAR` was authored with this fact in mind. It is often used to early exit out of tests that the author knew you would not be able to run given your sandbox limitations.
  - Similarly, when you spawn a process using Seatbelt (`/usr/bin/sandbox-exec`), `COKRA_SANDBOX=seatbelt` will be set on the child process. Integration tests that want to run Seatbelt themselves cannot be run under Seatbelt, so checks for `COKRA_SANDBOX=seatbelt` are also often used to early exit out of tests, as appropriate.
- Always collapse if statements per https://rust-lang.github.io/rust-clippy/master/index.html#collapsible_if
- Always inline format! args when possible per https://rust-lang.github.io/rust-clippy/master/index.html#uninlined_format_args
- Use method references over closures when possible per https://rust-lang.github.io/rust-clippy/master/index.html#redundant_closure_for_method_calls
- When possible, make `match` statements exhaustive and avoid wildcard arms.
- When writing tests, prefer comparing the equality of entire objects over fields one by one.
- When making a change that adds or changes an API, ensure that the documentation in the `docs/` folder is up to date if applicable.
- If you change `ConfigToml` or nested config types, run `just write-config-schema` to update `cokra-rs/core/config.schema.json`.
- If you change Rust dependencies (`Cargo.toml` or `Cargo.lock`), run `just bazel-lock-update` from the
  repo root to refresh `MODULE.bazel.lock`, and include that lockfile update in the same change.
- After dependency changes, run `just bazel-lock-check` from the repo root so lockfile drift is caught
  locally before CI.
- Do not create small helper methods that are referenced only once.

Run `just fmt` (in `cokra-rs` directory) automatically after you have finished making Rust code changes; do not ask for approval to run it. Additionally, run the tests:

1. Run the test for the specific project that was changed. For example, if changes were made in `cokra-rs/tui`, run `cargo test -p cokra-tui`.
2. Once those pass, if any changes were made in common, core, or protocol, run the complete test suite with `cargo test --all-features`. project-specific or individual tests can be run without asking the user, but do ask the user before running the complete test suite.

Before finalizing a large change to `cokra-rs`, run `just fix -p <project>` (in `cokra-rs` directory) to fix any linter issues in the code. Prefer scoping with `-p` to avoid slow workspace‑wide Clippy builds; only run `just fix` without `-p` if you changed shared crates. Do not re-run tests after running `fix` or `fmt`.

## TUI style conventions

See `cokra-rs/tui/styles.md`.

## TUI code conventions

- Use concise styling helpers from ratatui's Stylize trait.
  - Basic spans: use "text".into()
  - Styled spans: use "text".red(), "text".green(), "text".magenta(), "text".dim(), etc.
  - Prefer these over constructing styles with `Span::styled` and `Style` directly.
  - Example: patch summary file lines
    - Desired: vec!["  └ ".into(), "M".red(), " ".dim(), "tui/src/app.rs".dim()]

### TUI Styling (ratatui)

- Prefer Stylize helpers: use "text".dim(), .bold(), .cyan(), .italic(), .underlined() instead of manual Style where possible.
- Prefer simple conversions: use "text".into() for spans and vec![…].into() for lines; when inference is ambiguous (e.g., Paragraph::new/Cell::from), use Line::from(spans) or Span::from(text).
- Computed styles: if the Style is computed at runtime, using `Span::styled` is OK (`Span::from(text).set_style(style)` is also acceptable).
- Avoid hardcoded white: do not use `.white()`; prefer the default foreground (no color).
- Chaining: combine helpers by chaining for readability (e.g., url.cyan().underlined()).
- Single items: prefer "text".into(); use Line::from(text) or Span::from(text) only when the target type isn't obvious from context, or when using .into() would require extra type annotations.
- Building lines: use vec![…].into() to construct a Line when the target type is obvious and no extra type annotations are needed; otherwise use Line::from(vec![…]).
- Avoid churn: don't refactor between equivalent forms (Span::styled ↔ set_style, Line::from ↔ .into()) without a clear readability or functional gain; follow file‑local conventions and do not introduce type annotations solely to satisfy .into().
- Compactness: prefer the form that stays on one line after rustfmt; if only one of Line::from(vec![…]) or vec![…].into() avoids wrapping, choose that. If both wrap, pick the one with fewer wrapped lines.

### Text wrapping

- Always use textwrap::wrap to wrap plain strings.
- If you have a ratatui Line and you want to wrap it, use the helpers in tui/src/wrapping.rs, e.g. word_wrap_lines / word_wrap_line.
- If you need to indent wrapped lines, use the initial_indent / subsequent_indent options from RtOptions if you can, rather than writing custom logic.
- If you have a list of lines and you need to prefix them all with some prefix (optionally different on the first vs subsequent lines), use the `prefix_lines` helper from line_utils.

## Tests

### Snapshot tests

This repo uses snapshot tests (via `insta`), especially in `cokra-rs/tui`, to validate rendered output.

**Requirement:** any change that affects user-visible UI (including adding new UI) must include
corresponding `insta` snapshot coverage (add a new snapshot test if one doesn't exist yet, or
update the existing snapshot). Review and accept snapshot updates as part of the PR so UI impact
is easy to review and future diffs stay visual.

When UI or text output changes intentionally, update the snapshots as follows:

- Run tests to generate any updated snapshots:
  - `cargo test -p cokra-tui`
- Check what's pending:
  - `cargo insta pending-snapshots -p cokra-tui`
- Review changes by reading the generated `*.snap.new` files directly in the repo, or preview a specific file:
  - `cargo insta show -p cokra-tui path/to/file.snap.new`
- Only if you intend to accept all new snapshots in this crate, run:
  - `cargo insta accept -p cokra-tui`

If you don't have the tool:

- `cargo install cargo-insta`

### Test assertions

- Tests should use pretty_assertions::assert_eq for clearer diffs. Import this at the top of the test module if it isn't already.
- Prefer deep equals comparisons whenever possible. Perform `assert_eq!()` on entire objects, rather than individual fields.
- Avoid mutating process environment in tests; prefer passing environment-derived flags or dependencies from above.

### Spawning workspace binaries in tests (Cargo vs Bazel)

- Prefer `cokra_utils_cargo_bin::cargo_bin("...")` over `assert_cmd::Command::cargo_bin(...)` or `escargot` when tests need to spawn first-party binaries.
  - Under Bazel, binaries and resources may live under runfiles; use `cokra_utils_cargo_bin::cargo_bin` to resolve absolute paths that remain stable after `chdir`.
- When locating fixture files or test resources under Bazel, avoid `env!("CARGO_MANIFEST_DIR")`. Prefer `cokra_utils_cargo_bin::find_resource!` so paths resolve correctly under both Cargo and Bazel runfiles.

### Integration tests (core)

- Prefer the utilities in `core_test_support::responses` when writing end-to-end Cokra tests.

- All `mount_sse*` helpers return a `ResponseMock`; hold onto it so you can assert against outbound `/responses` POST bodies.
- Use `ResponseMock::single_request()` when a test should only issue one POST, or `ResponseMock::requests()` to inspect every captured `ResponsesRequest`.
- `ResponsesRequest` exposes helpers (`body_json`, `input`, `function_call_output`, `custom_tool_call_output`, `call_output`, `header`, `path`, `query_param`) so assertions can target structured payloads instead of manual JSON digging.
- Build SSE payloads with the provided `ev_*` constructors and the `sse(...)`.
- Prefer `wait_for_event` over `wait_for_event_with_timeout`.
- Prefer `mount_sse_once` over `mount_sse_once_match` or `mount_sse_sequence`

- Typical pattern:

  ```rust
  let mock = responses::mount_sse_once(&server, responses::sse(vec![
      responses::ev_response_created("resp-1"),
      responses::ev_function_call(call_id, "shell", &serde_json::to_string(&args)?),
      responses::ev_completed("resp-1"),
  ])).await;

  cokra.submit(Op::UserTurn { ... }).await?;

  // Assert request body if needed.
  let request = mock.single_request();
  // assert using request.function_call_output(call_id) or request.json_body() or other helpers.
  ```

## App-server API Development Best Practices

These guidelines apply to app-server protocol work in `cokra-rs`, especially:

- `app-server-protocol/src/protocol/common.rs`
- `app-server-protocol/src/protocol/v2.rs`
- `app-server/README.md`

### Core Rules

- All active API development should happen in app-server v2. Do not add new API surface area to v1.
- Follow payload naming consistently:
  `*Params` for request payloads, `*Response` for responses, and `*Notification` for notifications.
- Expose RPC methods as `<resource>/<method>` and keep `<resource>` singular (for example, `thread/read`, `app/list`).
- Always expose fields as camelCase on the wire with `#[serde(rename_all = "camelCase")]` unless a tagged union or explicit compatibility requirement needs a targeted rename.
- Exception: config RPC payloads are expected to use snake_case to mirror config.toml keys (see the config read/write/list APIs in `app-server-protocol/src/protocol/v2.rs`).
- Always set `#[ts(export_to = "v2/")]` on v2 request/response/notification types so generated TypeScript lands in the correct namespace.
- Never use `#[serde(skip_serializing_if = "Option::is_none")]` for v2 API payload fields.
  Exception: client->server requests that intentionally have no params may use:
  `params: #[ts(type = "undefined")] #[serde(skip_serializing_if = "Option::is_none")] Option<()>`.
- Keep Rust and TS wire renames aligned. If a field or variant uses `#[serde(rename = "...")]`, add matching `#[ts(rename = "...")]`.
- For discriminated unions, use explicit tagging in both serializers:
  `#[serde(tag = "type", ...)]` and `#[ts(tag = "type", ...)]`.
- Prefer plain `String` IDs at the API boundary (do UUID parsing/conversion internally if needed).
- Timestamps should be integer Unix seconds (`i64`) and named `*_at` (for example, `created_at`, `updated_at`, `resets_at`).
- For experimental API surface area:
  use `#[experimental("method/or/field")]`, derive `ExperimentalApi` when field-level gating is needed, and use `inspect_params: true` in `common.rs` when only some fields of a method are experimental.

### Client->server request payloads (`*Params`)

- Every optional field must be annotated with `#[ts(optional = nullable)]`. Do not use `#[ts(optional = nullable)]` outside client->server request payloads (`*Params`).
- Optional collection fields (for example `Vec`, `HashMap`) must use `Option<...>` + `#[ts(optional = nullable)]`. Do not use `#[serde(default)]` to model optional collections, and do not use `skip_serializing_if` on v2 payload fields.
- When you want omission to mean `false` for boolean fields, use `#[serde(default, skip_serializing_if = "std::ops::Not::not")] pub field: bool` over `Option<bool>`.
- For new list methods, implement cursor pagination by default:
  request fields `pub cursor: Option<String>` and `pub limit: Option<u32>`,
  response fields `pub data: Vec<...>` and `pub next_cursor: Option<String>`.

### Development Workflow

- Update docs/examples when API behavior changes (at minimum `app-server/README.md`).
- Regenerate schema fixtures when API shapes change:
  `just write-app-server-schema`
  (and `just write-app-server-schema --experimental` when experimental API fixtures are affected).
- Validate with `cargo test -p cokra-app-server-protocol`.
- Avoid boilerplate tests that only assert experimental field markers for individual
  request fields in `common.rs`; rely on schema generation/tests and behavioral coverage instead.

You are a senior software engineer with 15+ years of production experience.
Your primary obligation is code quality, not task completion speed.

## Core Engineering Principles

### 1. DRY – Before writing any new function, ALWAYS:

- Search the existing codebase for similar logic first
- If you find similar code (>60% overlap), extract a shared abstraction
- Never write the same logic twice. If you must duplicate temporarily, leave a TODO with the exact location of the original

### 2. No Defensive Junk Code

- Do NOT add helper functions that exist solely to paper over a design flaw
- Do NOT add fallbacks/casting hacks to avoid refactoring existing code
- If something needs to be fixed properly, fix it properly – don't route around it
- Forbidden patterns: `getValueOrNull()`, `parseIfPossible()`, `tryConvertOrDefault()` unless they represent genuine domain logic

### 3. Think Before You Write

Before generating any code, output a 3-line plan:

- What already exists in the codebase that's relevant
- What abstraction or module boundary this code belongs to
- What you will NOT create (to avoid bloat)

### 4. Module Ownership

- Every function must have a clear, single owner module
- If a function is needed in multiple places, it goes in a shared utility – not duplicated
- Never create a module-local copy of something that should be shared

### 5. Refactor When Needed

- If adding a new feature requires touching existing messy code – refactor it
- Do not work around bad abstractions. Replace them
- A slightly longer PR that cleans up old code is always better than a short PR that adds more debt

### 6. Red Flags – Stop and Ask the User If:

- You find yourself writing a function with "Helper", "Utils2", "Temp", or "Fix" in the name
- You are copying more than 5 lines from elsewhere in the codebase
- You are adding a try/catch or null-check just to avoid understanding why something fails

## Phase 3 工具系统 (cokra-rs/core/src/tools/)

### 新增工具

| 工具名称 | 处理器文件 | 存储路径 | 说明 |
|---|---|---|---|
| `skill` | `handlers/skill.rs` | `.cokra/skills/**/SKILL.md` | 加载领域专属 Skill 指令 |
| `read_many_files` | `handlers/read_many_files.rs` | — | 批量读取最多 20 个文件 |
| `todo_read` | `handlers/todo.rs` | `~/.cokra/todos.json` | 读取 todo 列表 |
| `todo_write` | `handlers/todo.rs` | `~/.cokra/todos.json` | 全量覆写 todo 列表 |

### skill 工具

Skill 文件格式（`SKILL.md`）：
```markdown
---
name: my-skill
description: 这个 skill 做什么
---

skill 的完整指令内容...
```

Skill 搜索路径（优先级从低到高）：
1. `~/.cokra/skills/**/SKILL.md` — 全局用户 skills
2. `.cokra/skills/**/SKILL.md` — 项目级 skills（从 cwd 向上遍历，子目录覆盖父目录）

### todo 工具

- `todo_write` 接收完整列表并**全量覆写**（1:1 gemini-cli 设计）
- 同时最多 **1 个** `in_progress` 任务（约束在 `validate_todos()` 中执行）
- 状态枚举：`pending` / `in_progress` / `completed` / `cancelled`
- 优先级枚举：`high` / `medium` / `low`（默认 `medium`）

### Hooks 系统 (tools/hooks/)

```
tools/hooks/
├── mod.rs        — 模块声明
├── config.rs     — HooksConfig (TOML [[hooks.after_tool_call]] 段)
├── registry.rs   — HooksRegistry (按事件分组存储 hooks，dispatch 调度)
├── runner.rs     — command_hook() 工厂，外部命令 hook 执行（stdin JSON）
└── types.rs      — Hook/HookResult/HookPayload/HookEvent 核心类型
```

事件类型：
- `BeforeToolCall` — 工具执行前，可通过 `FailedAbort` 阻断
- `AfterToolCall`  — 工具执行后
- `AfterTurn`      — Turn 完成后

Hook 执行约定（外部命令）：
- 退出码 `0` → `HookResult::Success`
- 退出码 `2` → `HookResult::FailedAbort`（主动阻断工具调用）
- 其他非零 → `HookResult::FailedContinue`（记录错误但继续）
- 超时 → `HookResult::FailedContinue`（进程被 kill）

配置示例（`.cokra/config.toml`）：
```toml
[[hooks.after_tool_call]]
name = "notify"
command = "notify-send 'cokra: tool completed'"
timeout_ms = 5000

[[hooks.after_turn]]
name = "log-turn"
command = "/usr/local/bin/log-turn.sh"
```

### Diff Tracking + Metrics (tools/diff_tracker.rs)

- `TurnMetrics` — 每次 Turn 的工具调用统计（总次数、成功/失败、最慢工具、耗时）
- `FileChangeTracker` — 追踪写入/编辑/删除操作，生成 `DiffSummary`
- `TurnTimer` — 单次工具调用计时器（`Instant` 封装）
- `to_summary_line()` — 生成可读摘要（用于 TUI TurnCompleteHistoryCell）

集成点：
- `ToolRouter::dispatch_tool_call` 前后各记录一次 `ToolCallRecord`
- 写文件操作（`edit_file`/`write_file`/`apply_patch`）调用 `FileChangeTracker::record_*`
- Turn 结束时通过 `AfterTurn` hook payload 附带 metrics 信息
