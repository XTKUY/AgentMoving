# 开发验证记录

日期：2026-09-21。环境：macOS ARM64，Homebrew rustc/cargo 1.98.0。

## 已执行

| 检查 | 结果 |
|---|---|
| `cargo fmt --all --check` | 通过 |
| `cargo clippy --all-targets --locked --offline -- -D warnings` | 通过 |
| `cargo test --locked --offline` | 13 项集成测试通过 |
| `cargo build --release --locked --offline` | 通过，生成 macOS ARM64 二进制，约 2 MiB |
| release 二进制的 TUI PTY smoke | 完整导出、导入、项目映射、确认、退出通过 |
| release 二进制的 Codex 原生 smoke | Codex 0.155.1 在已有隔离状态库下，先 thread/list 可发现，再 thread/read 可读取，cwd 映射正确 |
| 本机只读检测 | 找到 Codex 与 Claude Code 程序及会话，0 个扫描警告；没有导出或写回真实聊天 |
| `otool -L` | 仅系统 CoreFoundation、libiconv、libSystem；不依赖 Homebrew Rust 动态库 |

## 集成测试覆盖

1. 三种 Agent 的导出/导入、项目映射、重复导入与回滚。
2. 回滚保留后来被修改的会话文件。
3. 冲突拒绝覆盖，缺少项目阻塞导入。
4. Pi 父会话自动加入导出并在目标端重新关联。
5. 可移植路径校验、目录边界和最长前缀映射。
6. 不完整 JSONL 和未知 Pi 版本显式报错。
7. payload 篡改和 manifest 身份不匹配拒绝导入。
8. ZIP `../` 越界条目拒绝导入。
9. Unix symlink 目标拒绝写入。
10. 第二个文件写入失败时撤回第一个新文件，保留目标原有文件。
11. 不改变 cwd 的映射保持原始字节，扫描警告进入导出包。
12. Pi 自定义平铺目录与已知 sidecar 恢复到目标项目分组。
13. 计划生成后目标出现新文件时停止，不覆盖竞争写入内容。

原生 smoke 全程未发送 `turn/start`，未使用真实会话或账户调用模型。它验证存储读取与发现，不等于真实业务对话的续聊验收。

## 尚未执行

- Windows/Linux 实机或远端 CI 运行。
- Claude Code/Pi 原生客户端的续聊。
- 所有历史/未来 Agent 版本、Codex 桌面侧栏状态、SQLite 专属会话历史迁移。
- 大规模数十 GiB 负载与断电级故障注入。

相关约束和手动验收步骤见 [COMPATIBILITY.md](COMPATIBILITY.md)。
