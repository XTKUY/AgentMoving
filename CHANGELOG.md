# Changelog

## 0.1.0 — 2026-09-21（本地开发版，未发布）

### 新增

- Rust 原生 CLI 与 Ratatui/Crossterm TUI，面向 macOS、Windows、Linux。
- Codex、Claude Code、Pi Coding Agent 程序/会话目录检测，环境变量和显式数据根目录支持。
- 会话摘要、项目筛选、搜索、多选和全选；Codex 归档会话；Claude 子会话/快照依赖；Pi 父会话闭包。
- 标准 ZIP 迁移包、版本化 manifest、流式文件复制、SHA-256 和导出快照检查。
- 全包校验、dry-run、目标项目路径映射、原生未知字段保留、Pi 父会话重定位。
- 会话 ID / 文件冲突检查、重复导入跳过、不覆盖发布、自身迁移锁、事务记录和保守回滚。
- 非法路径、ZIP 越界、摘要篡改、大小/数量超限和 symlink 检查；Windows reparse point 检查。
- macOS 本地集成测试、隔离 Codex 原生读/列举 smoke 脚本，以及跨平台 CI/构建工作流。
- README、技术架构、包格式和兼容性文档。

### 已知边界

- 不转换不同 Agent 之间的会话，不迁移项目代码、登录凭证、全局配置或任意外部附件。
- 不直接复制/覆盖 Codex 内部 SQLite 库，不承诺桌面侧栏和 SQLite 专属历史迁移。
- Claude/Pi 原生继续聊天、Windows/Linux 实机结果尚未验证。
- 普通 ZIP 不加密；包摘要不提供来源认证。
- 第三方客户端需停止写入后迁移；跨文件发布支持恢复但不是单一原子事务。
