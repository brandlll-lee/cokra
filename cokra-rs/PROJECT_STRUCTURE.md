# Cokra Rust 项目结构详解

## 📋 项目概述
**Cokra** 是一个 AI Agent Team CLI 环境，使用 Rust 编写的工作空间项目。

- **Rust 版本**: 1.93.0+
- **Edition**: 2024
- **License**: MIT OR Apache-2.0
- **Repository**: https://github.com/cokra/cokra

---

## 🏗️ 工作空间结构

### 核心模块 (Core Modules)

#### 1. **cli** - 命令行界面
- 主入口点
- 文件: `cli/src/main.rs`
- 功能: CLI 命令解析和执行

#### 2. **core** - 核心业务逻辑
- 主要模块: 
  - `cokra.rs` - 核心 Cokra 功能
  - `config.rs` - 配置管理
  - `exec.rs` - 执行引擎
  - `exec_policy.rs` - 执行策略
  - `mcp.rs` - MCP 协议支持
  - `sandbox_manager.rs` - 沙箱管理
  - `shell.rs` - Shell 命令处理
  - `thread_manager.rs` - 线程管理
  - `truncate.rs` - 文本截断
- 测试: `core/tests/sse_integration_test.rs`

#### 3. **protocol** - 协议定义
- 通信协议定义
- 数据结构序列化/反序列化

#### 4. **tui** - 终端用户界面
- 基于 Ratatui 框架
- 交互式终端界面

---

### 服务器模块 (Server Modules)

#### 5. **app-server** - 应用服务器
- 主应用服务器实现
- 文件: `app-server/src/lib.rs`

#### 6. **app-server-protocol** - 应用服务器协议
- 应用服务器通信协议
- 版本: `v2.rs`

#### 7. **exec-server** - 执行服务器
- 执行命令的服务器
- 文件: `exec-server/src/lib.rs`

#### 8. **mcp-server** - MCP 服务器
- Model Context Protocol 服务器实现
- 文件: `mcp-server/src/lib.rs`

---

### 执行和沙箱模块 (Execution & Sandbox)

#### 9. **exec** - 执行模块
- 通用执行接口
- 文件: `exec/src/lib.rs`

#### 10. **unified-exec** - 统一执行
- 统一的执行框架
- 文件: `unified-exec/src/lib.rs`

#### 11. **linux-sandbox** - Linux 沙箱
- Linux 平台沙箱实现
- 文件: `linux-sandbox/src/lib.rs`

#### 12. **windows-sandbox-rs** - Windows 沙箱
- Windows 平台沙箱实现
- 文件: `windows-sandbox-rs/src/lib.rs`

---

### 客户端模块 (Client Modules)

#### 13. **rmcp-client** - RMCP 客户端
- Remote MCP 客户端
- 文件: `rmcp-client/src/lib.rs`

#### 14. **codex-client** - Codex 客户端
- Codex API 客户端
- 文件: `codex-client/src/lib.rs`

#### 15. **cloud-tasks-client** - 云任务客户端
- Google Cloud Tasks 客户端
- 文件: `cloud-tasks-client/src/lib.rs`

---

### 功能模块 (Feature Modules)

#### 16. **config** - 配置管理
- 分层配置系统
- 文件:
  - `config/src/layered.rs` - 分层配置
  - `config/src/layer_stack.rs` - 配置栈
  - `config/src/loader.rs` - 配置加载器
  - `config/src/profile.rs` - 配置文件
  - `config/src/types.rs` - 类型定义

#### 17. **state** - 状态管理
- 应用状态管理
- 文件: `state/src/lib.rs`

#### 18. **secrets** - 密钥管理
- 密钥存储和管理
- 文件: `secrets/src/lib.rs`

#### 19. **keyring-store** - 密钥环存储
- 系统密钥环集成
- 文件: `keyring-store/src/lib.rs`

#### 20. **file-search** - 文件搜索
- 文件搜索功能
- 文件: `file-search/src/lib.rs`

#### 21. **apply-patch** - 补丁应用
- 代码补丁应用
- 文件: `apply-patch/src/lib.rs`

#### 22. **shell-command** - Shell 命令
- Shell 命令执行
- 文件: `shell-command/src/lib.rs`

#### 23. **network-proxy** - 网络代理
- 网络代理功能
- 文件: `network-proxy/src/lib.rs`

#### 24. **stdio-to-uds** - 标准输入输出到 UDS
- 标准 I/O 到 Unix Domain Socket 转换
- 文件: `stdio-to-uds/src/lib.rs`

---

### 云服务模块 (Cloud Services)

#### 25. **cloud-tasks** - 云任务
- Google Cloud Tasks 集成
- 文件: `cloud-tasks/src/lib.rs`

#### 26. **cloud-requirements** - 云需求
- 云环境需求定义
- 文件: `cloud-requirements/src/lib.rs`

#### 27. **codex-api** - Codex API
- Codex API 定义
- 文件: `codex-api/src/lib.rs`

---

### 模型提供者模块 (Model Provider)

#### 28. **model-provider** - 模型提供者
- AI 模型提供者接口
- 文件: `model-provider/src/lib.rs`

---

### 工具库模块 (Utility Libraries)

#### 29. **utils/** - 工具库集合

| 工具库 | 功能 |
|--------|------|
| `absolute-path` | 绝对路径处理 |
| `async-priority` | 异步优先级队列 |
| `cancel` | 取消令牌 |
| `cargo-bin` | Cargo 二进制工具 |
| `cli` | CLI 工具库 |
| `env` | 环境变量处理 |
| `fs-err` | 文件系统错误处理 |
| `git` | Git 操作 |
| `path` | 路径处理 |
| `proj-list` | 项目列表 |
| `runfiles` | 运行文件管理 |
| `temp-dir` | 临时目录管理 |
| `testing` | 测试工具 |

---

## 📦 主要依赖

### 异步运行时
- `tokio` (1.49) - 异步运行时
- `async-trait` (0.1.89) - 异步 trait 支持

### 序列化
- `serde` (1.0) - 序列化框架
- `serde_json` (1.0) - JSON 支持
- `toml` (0.9.5) - TOML 支持
- `toml_edit` (0.24.0) - TOML 编辑

### 网络
- `reqwest` (0.12) - HTTP 客户端
- `tokio-tungstenite` (0.28) - WebSocket

### 数据库
- `sqlx` (0.8) - SQL 工具包 (SQLite + Tokio)

### MCP
- `rmcp` (0.15) - Remote MCP 客户端

### 错误处理
- `anyhow` (1.0) - 灵活的错误处理
- `thiserror` (2.0) - 错误类型派生

### 日志
- `tracing` (0.1.44) - 结构化日志
- `tracing-subscriber` (0.3) - 日志订阅者

### UI
- `ratatui` (0.29) - 终端 UI 框架
- `crossterm` (0.28) - 终端控制
- `supports-color` (3) - 颜色支持检测
- `textwrap` (0.16) - 文本换行
- `unicode-width` (0.2) - Unicode 宽度

### CLI
- `clap` (4.5) - 命令行参数解析

### 工具库
- `uuid` (1.0) - UUID 生成
- `chrono` (0.4) - 日期时间
- `regex` (1.12) - 正则表达式
- `ignore` (0.4) - 文件忽略
- `itertools` (0.14) - 迭代器工具
- `derive_more` (2) - 派生宏扩展
- `tokio-stream` (0.1) - 异步流
- `tokio-util` (0.7) - Tokio 工具
- `libc` (0.2) - C 库绑定

### 测试
- `insta` (1.46) - 快照测试
- `wiremock` (0.6) - HTTP 模拟
- `pretty_assertions` (1) - 漂亮的断言

---

## 🔧 构建配置

### Cargo 配置
- 位置: `.cargo/config.toml`
- 工作空间解析器: 3

### Clippy 配置
- 位置: `clippy.toml`
- 严格检查: 禁止 `expect_used`, `unwrap_used`, `redundant_clone`, `needless_collect`

### Rustfmt 配置
- 位置: `rustfmt.toml`

### Bazel 构建
- 位置: `BUILD.bazel`

---

## 📊 项目统计

- **工作空间成员**: 48 个
- **核心模块**: 4 个 (cli, core, protocol, tui)
- **服务器模块**: 4 个
- **执行/沙箱模块**: 4 个
- **客户端模块**: 3 个
- **功能模块**: 9 个
- **云服务模块**: 3 个
- **模型提供者**: 1 个
- **工具库**: 13 个

---

## 🎯 项目特点

1. **多平台支持**: Linux 和 Windows 沙箱
2. **MCP 集成**: Model Context Protocol 支持
3. **云服务集成**: Google Cloud Tasks 支持
4. **灵活的配置系统**: 分层配置管理
5. **安全的密钥管理**: 系统密钥环集成
6. **现代 CLI**: 基于 Clap 和 Ratatui
7. **异步优先**: 基于 Tokio 的异步架构
8. **严格的代码质量**: Clippy 检查和测试覆盖

---

## 📝 文件组织

```
cokra-rs/
├── Cargo.toml                 # 工作空间配置
├── Cargo.lock                 # 依赖锁定
├── BUILD.bazel                # Bazel 构建配置
├── clippy.toml                # Clippy 检查配置
├── rustfmt.toml               # Rustfmt 配置
├── .cargo/                    # Cargo 配置目录
├── cli/                       # CLI 模块
├── core/                      # 核心模块
├── protocol/                  # 协议模块
├── tui/                       # TUI 模块
├── app-server/                # 应用服务器
├── exec-server/               # 执行服务器
├── mcp-server/                # MCP 服务器
├── linux-sandbox/             # Linux 沙箱
├── windows-sandbox-rs/        # Windows 沙箱
├── config/                    # 配置管理
├── state/                     # 状态管理
├── secrets/                   # 密钥管理
├── utils/                     # 工具库集合
└── target/                    # 编译输出目录
```

