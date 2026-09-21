# 兼容性与验收边界

## 支持的数据

| Agent | 扫描/导出 | 导入策略 | 不包含 |
|---|---|---|---|
| Codex | sessions、archived_sessions 中带 session_meta 的 rollout JSONL | 按原目录恢复、映射 cwd，由原生客户端读取/发现 | SQLite 专属历史、云端会话、桌面项目/侧栏/置顶状态、goals、memories、认证和配置 |
| Claude Code | projects 下顶层会话；同 ID 目录中的子会话/工具结果；file-history/ID | 按项目映射会话目录和顶层 cwd，保留 UUID 关系 | 全局索引覆盖、全局 history、认证、配置、外部附件、Cowork/Desktop 独立格式 |
| Pi | JSONL session version 1–3，树/分支记录，parentSession；已知 .acp.json sidecar | 目标项目目录编码，映射 header cwd 和父会话文件路径 | 其他扩展的私有数据库、任意外部输出文件、认证和配置 |

**“全部”是全部已发现的受支持本地会话。** 不保证恢复已被 Agent 自动清理的会话，也不能自动发现所有任意自定义目录。未知 Pi 格式版本拒绝处理；Codex/Claude 保留未知字段，但不存在官方通用迁移稳定性保证。

Claude/Pi 的附件如果内嵌在原生 JSONL 中会自然保留。对话中仅引用的任意磁盘图片、项目文件、外部工具输出不自动打包，以免将无关目录或凭证带入包。

Pi parentSession 未找到时导出仍可作为备份，但包内记录警告；导入必须包含该父会话，否则阻塞。副本导入不会迁移扩展运行环境。

## 验证矩阵

| 验证项 | 当前证据 |
|---|---|
| macOS / Homebrew Rust 1.98.0 | 本地编译与自动测试 |
| Windows / Linux | 提供 CI 与原生路径分支；尚未在本次开发环境实机执行 |
| Codex CLI 0.155.1 | 隔离合成 rollout 导入后，原生 thread/read 可读、cwd 正确、thread/list 可发现；已覆盖目标存在 SQLite 状态库的场景，先列举后读取；未发模型轮次 |
| Claude Code | 合成 JSONL、子会话、file-history 往返与路径映射测试；原生 resume 未验证 |
| Pi Coding Agent | 合成 JSONL v3、父会话和 sidecar 规则测试；原生 resume 未验证 |
| Codex 桌面 UI | 不保证侧栏布局和项目状态迁移；本地 JSONL 与内部 SQLite 历史不等价 |
| TUI 向导 | macOS PTY 中完整导出、导入、映射、确认和退出通过 |

验收分三层：① 文件及摘要正确；② 原生 Agent 可发现/读取；③ 用户登录、项目代码和模型可用时能继续聊天。本版 CLI/TUI 导入结果承诺第一层，第二层按矩阵记录，第三层由真实目标环境验证。

## 手动验收

1. 源端退出相关会话，保留一份导出包；目标端关闭 Agent。
2. 准备目标项目目录和代码，先 dry-run 并检查映射。
3. 导入后启动相同或已验证兼容版本的 Agent，选择迁入项目。
4. Codex 使用 `codex resume <ID>`；Claude Code 使用 `claude --resume <ID>`；Pi 使用其 `/resume` 会话选择器，或以原生 `--session` 指定文件。
5. 确认用户/助手历史和分支仍在，项目 cwd 正确，再在自己的账户下发起新一轮。
6. 如果列表中未显示，保留包和事务记录，记录客户端版本；不要直接覆盖整个状态数据库。

## 文档依据

查阅日期：2026-09-21。第三方存储格式可能变化。

- [Codex 状态目录](https://learn.chatgpt.com/docs/config-file/config-advanced)
- [Codex App Server：thread/read、thread/list](https://learn.chatgpt.com/docs/app-server)
- [Claude Code 会话与 JSONL 存储](https://code.claude.com/docs/en/sessions)
- [Pi 会话格式](https://pi.dev/docs/latest/session-format)
- [Pi 环境变量](https://pi.dev/docs/latest/environment-variables)
