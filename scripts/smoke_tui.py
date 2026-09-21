#!/usr/bin/env python3
"""macOS/Linux PTY smoke: complete TUI export + import using synthetic Pi data."""
import errno
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unicodedata

binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/agentmoving").resolve()
with tempfile.TemporaryDirectory(prefix="agentmoving-tui-") as temporary:
    root = Path(temporary).resolve()
    source, target, project = root / "source", root / "target", root / "project"
    project.mkdir()
    transcript = source / "--old-project--/fixture.jsonl"
    transcript.parent.mkdir(parents=True)
    transcript.write_text(json.dumps({"type": "session", "version": 3, "id": "tui-fixture", "cwd": "/old/project", "timestamp": "2026-09-21T00:00:00Z"}) + "\n" + json.dumps({"type": "message", "id": "m1", "parentId": None, "message": {"role": "user", "content": "TUI smoke fixture"}}) + "\n")
    bundle = root / "tui.zip"
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 140, 0, 0))
    def controlling_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    process = subprocess.Popen([str(binary), "tui", "--agent", "pi", "--root", str(source)], stdin=slave, stdout=slave, stderr=slave, env={**os.environ, "TERM": "xterm-256color"}, preexec_fn=controlling_terminal)
    os.close(slave)
    screen = [[" "] * 140 for _ in range(36)]
    cursor = [0, 0]
    pending = ""
    decoder = codecs.getincrementaldecoder("utf-8")("replace")
    def feed(data):
        global pending
        pending += decoder.decode(data)
        while pending:
            if pending.startswith("\x1b["):
                match = re.match(r"\x1b\[([0-?]*)([ -/]*)([@-~])", pending)
                if not match:
                    break
                params, _, command = match.groups()
                if command in "Hf":
                    values = [int(x or 1) for x in params.split(";")]
                    cursor[:] = [values[0] - 1, (values[1] if len(values) > 1 else 1) - 1]
                elif command == "J" and params in ("2", "3"):
                    for row in screen:
                        row[:] = [" "] * 140
                pending = pending[match.end():]
                continue
            if pending == "\x1b":
                break
            c, pending = pending[0], pending[1:]
            if c == "\r":
                cursor[1] = 0
            elif c == "\n":
                cursor[0] += 1
            elif c >= " ":
                y, x = cursor
                width = 2 if unicodedata.east_asian_width(c) in ("W", "F") else 1
                if 0 <= y < 36 and 0 <= x < 140:
                    screen[y][x] = c
                    if width == 2 and x + 1 < 140:
                        screen[y][x + 1] = ""
                cursor[1] += width
    def wait_for(text):
        data = b""
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.1)[0]:
                try:
                    data += os.read(master, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:
                        break
                    raise
                feed(data)
                data = b""
                if text in "\n".join("".join(row) for row in screen):
                    return
        raise RuntimeError(f"TUI did not display {text!r}: " + "\n".join("".join(row) for row in screen))
    def send(value):
        os.write(master, value.encode())
    def wait_exit():
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and process.poll() is None:
            if select.select([master], [], [], 0.1)[0]:
                try:
                    os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
        return process.poll()
    try:
        wait_for("AgentMoving 0.1")
        send("\r")
        wait_for("选择 Agent / 数据目录")
        send(" \r")
        wait_for("选择会话")
        send("a\r")
        wait_for("导出文件路径")
        send("\x15" + str(bundle) + "\r")
        wait_for("导出完成")
        assert bundle.is_file()
        send("\r")
        wait_for("AgentMoving 0.1")
        send("j\r")
        wait_for("迁移包路径")
        send(str(bundle) + "\r")
        wait_for("选择导入会话")
        send("a\r")
        wait_for("目标数据目录")
        send("\x15" + str(target) + "\r")
        wait_for("项目目录映射")
        send("\x15" + str(project) + "\r")
        wait_for("导入计划（尚未写入）")
        send("\r")
        wait_for("输入 IMPORT")
        send("IMPORT\r")
        wait_for("导入完成")
        assert len(list(target.rglob("fixture.jsonl"))) == 1
        assert len(list(target.glob(".agentmoving/transactions/import-*.json"))) == 1
        send("\r")
        wait_for("AgentMoving 0.1")
        send("\x1b")
        assert wait_exit() == 0
        print("TUI export/import/mapping/confirmation/exit passed in isolated PTY")
    finally:
        if process.poll() is None:
            process.terminate()
            if wait_exit() is None:
                process.kill()
                process.wait(timeout=5)
        os.close(master)
