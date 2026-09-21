# AgentMoving

在 macOS、Windows、Linux 之间迁移 **同一种 Agent** 的本地聊天会话。Rust 原生程序，提供终端 TUI 和可脚本化 CLI，不依赖 Electron、Node.js、Python 或 Rust 运行环境。

当前版本：`0.1.0`。支持 Codex、Claude Code、Pi Coding Agent；不做跨 Agent 转换。项目代码、登录凭证、Agent 配置和外部文件不会随聊天自动迁移。

## 开发与运行

开发需要 Rust；普通用户只需要对应系统和 CPU 架构的可执行文件。本项目在 Homebrew Rust 1.98.0 上开发，不需要再安装 rustup。

```sh
cargo build --release --locked
./target/release/agentmoving
```

Windows 对应 `target\release\agentmoving.exe`。首次构建需要下载 crates，之后可以 `--offline` 构建。程序运行不联网，不调用模型。

## TUI

直接运行 `agentmoving`，或 `agentmoving tui`。

- 导出：选择 Agent/数据目录 → 搜索、预览和多选会话 → 指定 ZIP 输出位置。
- 导入：选择 ZIP → 选择会话 → 指定各 Agent 目标数据目录 → 映射项目目录 → 审阅计划 → 输入 `IMPORT`。
- `↑↓` / `j k` 移动，空格选择，`A` 选择/取消当前筛选结果，`/` 搜索，回车继续，Esc 返回。
- 文本输入支持 Backspace 和 Ctrl+U 清空；长结果页支持方向键与 PageUp/PageDown。
- Pi 的父会话会在导出时自动加入。选择性导入时需要同时选中父会话。
- 扫描、打包、校验和导入在后台线程执行，并显示进度。写入期间请等待操作完成；强制退出后可用事务记录恢复。

自定义数据目录：

```sh
agentmoving tui --agent codex --root /path/to/codex-home
```

## CLI

### 检测和列举

```sh
agentmoving scan
agentmoving scan --agent codex --json
agentmoving scan --agent claude-code --root /backup/.claude
agentmoving scan --agent pi --root /backup/.pi/agent/sessions
```

**路径约定：Codex 和 Claude Code 的 `--root` 是配置根目录；Pi 的 `--root` 是会话目录（通常以 `sessions` 结尾）。** `--root` 可重复；指定它时必须同时指定 `--agent`，并替代该次扫描的默认根目录。

默认检测当前用户目录、环境变量指定目录和常见命令位置。程序未安装但数据仍在时也能导出。错误、无权限、符号链接和损坏记录会显示在扫描警告中，不冒充全部扫描成功。

| Agent | 默认数据根目录 | 环境变量 |
|---|---|---|
| Codex | `~/.codex` | `CODEX_HOME` |
| Claude Code | `~/.claude` | `CLAUDE_CONFIG_DIR` |
| Pi | `~/.pi/agent/sessions` | `PI_CODING_AGENT_SESSION_DIR`；其次 `PI_CODING_AGENT_DIR` 下的设置和 sessions |

Pi 同时读取配置中的绝对 `sessionDir` 或 `~/...`。相对配置路径无法脱离原工作目录可靠解析，请使用 `--root`。WSL 与 Windows 原生数据分开管理：可在 WSL 内运行 Linux 版本，或通过 `--root` 显式选择可访问的数据目录；本版不自动枚举 WSL 发行版。

### 导出

```sh
agentmoving export --agent codex --all -o codex.agentmove.zip
agentmoving export --all -o all-agents.agentmove.zip
agentmoving export --agent claude-code --session SESSION_ID -o selected.agentmove.zip
agentmoving export --agent pi --all --project my-project -o project.agentmove.zip
```

`--session` 可重复，接受原始 ID 或 `codex:ID` / `claude-code:ID` / `pi:ID`。`--project` 是项目路径子串筛选。`--all` 指有效、已发现的数据，不包含已删除数据、其他账户、远程机器或任意未知目录。

导出保留原始会话文件，关联文件见 [兼容性说明](docs/COMPATIBILITY.md)。包内保存扫描警告、来源路径和会话元数据。已有输出文件不会被覆盖。导出完成前会重新校验整个包；正在变化的源文件会导致失败，请停止相关会话后重试。

### 校验与查看包

```sh
agentmoving inspect backup.agentmove.zip
agentmoving inspect backup.agentmove.zip --json
```

这会验证全部文件的 SHA-256 和会话元数据；只在临时目录解包，不写入 Agent 数据目录。

### 跨系统导入

先在目标机器准备项目代码，关闭目标 Agent，然后预览计划。下面是 Windows PowerShell 示例：

```powershell
.\agentmoving.exe import .\backup.agentmove.zip --map '/Users/alice/code/demo=D:\Projects\demo' --dry-run
.\agentmoving.exe import .\backup.agentmove.zip --map '/Users/alice/code/demo=D:\Projects\demo'
```

多个项目可以重复 `--map`；最长路径前缀优先，并按目录边界匹配。只改明确识别的 cwd 和 Pi 父会话引用，正文与历史命令中的路径保持原样。

指定自定义目标根目录：

```sh
agentmoving import backup.agentmove.zip \
  --target codex=/data/codex-home \
  --target claude-code=/data/claude-home \
  --target pi=/data/pi/sessions \
  --map /old/project=/new/project \
  --dry-run
```

其他选项：

- `--session AGENT:ID`：选择包内的会话，可重复。
- `--skip-conflicts`：跳过整个冲突会话，不覆盖其中任何文件。Pi 的冲突仍需先处理，以免破坏父子引用。
- `--allow-missing-projects`：允许仅恢复聊天存档，项目目录暂时不存在；不表示项目可继续执行。
- `--json`：输出结构化计划；进度和导入结果写 stderr。

计划状态：`new` 可导入；`identical` 已存在、跳过；`conflict` 内容或 ID 冲突；`blocked` 缺少项目或 Pi 父会话。默认只要存在阻塞/冲突，就不会发布会话文件。`--dry-run` 成功表示已生成计划，**不表示计划没有阻塞**，自动化调用方应检查状态。

### 回滚

导入会返回目标目录下的 `.agentmoving/transactions/import-*.json`：

```sh
agentmoving rollback /target/root/.agentmoving/transactions/import-ABC123.json
```

回滚只删除该事务创建且内容未改变的文件，保留原有文件和后来被修改的文件。空目录不会删除。发生普通错误时自动尝试回滚；断电/强制终止后，确认迁移进程已经停止，才可移除对应根目录下残留的 `.agentmoving.lock`，再用原位置的事务记录回滚。

请在目标 Agent 关闭时导入/回滚。AgentMoving 的锁用于排除自身的并发迁移，不能锁住第三方 Agent。

## 验证与限制

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
python3 scripts/smoke_codex.py target/debug/agentmoving
python3 scripts/smoke_tui.py target/debug/agentmoving
```

两个脚本仅供开发验证：Codex 脚本构造合成会话、使用隔离的 `CODEX_HOME`，调用本机 Codex 的 `thread/read` / `thread/list`，不访问真实聊天、不发模型请求；TUI 脚本在 macOS/Linux 的隔离伪终端中完成导出和导入向导。Python 不是产品运行依赖。

本机已验证 macOS 编译、迁移测试以及 Codex 0.155.1 原生读取/列举。Windows/Linux 的 CI 配置已提供，但尚未在本机实际运行这些系统。Claude Code/Pi 当前通过格式与文件往返测试，真实客户端继续聊天仍需在对应版本验证。详见 [兼容性与验证矩阵](docs/COMPATIBILITY.md)。

聊天包可能包含用户原本写在对话里的密钥、代码或个人信息；本版不加密、不脱敏。它排除登录凭证文件，不会自动清除正文内的信息。仅导入自己信任的迁移包。

## 文档

- [技术架构](docs/TECHNICAL.md)
- [迁移包格式](docs/ARCHIVE_FORMAT.md)
- [兼容性与验证矩阵](docs/COMPATIBILITY.md)
- [本次验证记录](docs/VALIDATION.md)
- [CHANGELOG](CHANGELOG.md)
