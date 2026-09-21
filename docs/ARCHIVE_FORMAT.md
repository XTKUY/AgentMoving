# 迁移包格式 v1

文件建议命名 `*.agentmove.zip`。标准 ZIP/Deflate，可用普通解压工具检查，导入必须通过 AgentMoving 校验。

```text
manifest.json
agents/codex/0/sessions/2026/09/21/rollout-....jsonl
agents/claude-code/1/projects/<encoded-project>/<id>.jsonl
agents/claude-code/1/projects/<encoded-project>/<id>/subagents/agent-....jsonl
agents/pi/2/--project--/<timestamp>_<id>.jsonl
```

数字是 manifest.sessions 的零基索引。每个 payload 的 entry 必须严格匹配 `agents/{agent}/{index}/{relative_path}`，防止清单将任意包内容路由到其他目录。

manifest 顶层字段：

| 字段 | 含义 |
|---|---|
| format | 固定为 `agentmoving` |
| version | 整数 `1`；不支持的版本拒绝导入 |
| created_at | RFC 3339 导出时间 |
| source_os | 构建平台 OS 名称 |
| tool_version | 导出工具版本 |
| sessions | 会话元数据与 payload 列表 |
| warnings | 扫描遗漏和迁移边界提示 |

每个 sessions 项含 `session` 与 `files`。session 的字段由 `src/model.rs::Session` 定义，包括 Agent、ID、原工作目录、来源根目录、来源会话文件、原生格式版本和父会话路径。原路径仅是迁移元数据，绝不能直接作为解包写入目标。

每个 files 项：

| 字段 | 含义 |
|---|---|
| entry | ZIP 内唯一相对路径 |
| relative_path | 相对来源 Agent 根目录的路径 |
| bytes | 未压缩字节数 |
| sha256 | 原始字节的 64 字符十六进制 SHA-256 |
| transcript | 是否是适配器识别的原生 JSONL 会话记录 |

SHA-256 基于**导出时的原始文件**。路径映射后内容可能改变，导入事务记录保存的是**目标文件摘要**。不能拿映射后的文件直接与包内摘要比较。

清单中同一种 Agent 的 ID 必须唯一。不同根目录存在相同 ID 时，用户需要分开导出并自行判断哪个版本应迁移。v1 不拼接分叉会话，不自动重写 ID。

以后改变文件路由规则或不兼容字段语义时增加包格式版本；Agent 原生格式的变化不意味着必须改变包格式，但需要更新对应适配器和兼容性声明。
