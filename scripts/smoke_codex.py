#!/usr/bin/env python3
"""Isolated native Codex read smoke test; never sends a turn or reads real chat data."""
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time

binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/agentmoving").resolve()
codex = shutil.which("codex")
if not codex:
    raise SystemExit("codex not installed")

def probe(root, target, project, session_id=None):
    env = {**os.environ, "CODEX_HOME": str(target)}
    messages = queue.Queue()
    with (root / "server.log").open("w") as log:
        process = subprocess.Popen([codex, "app-server", "--listen", "stdio://"], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True)
        def reader():
            for line in process.stdout:
                try:
                    messages.put(json.loads(line))
                except json.JSONDecodeError:
                    pass
        threading.Thread(target=reader, daemon=True).start()
        def send(value):
            process.stdin.write(json.dumps(value) + "\n")
            process.stdin.flush()
        def request(identifier, method, params):
            send({"id": identifier, "method": method, "params": params})
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                try:
                    response = messages.get(timeout=1)
                except queue.Empty:
                    continue
                if response.get("id") == identifier:
                    return response
            raise TimeoutError(method)
        try:
            init = request(1, "initialize", {"clientInfo": {"name": "agentmoving_smoke", "version": "0.1.0"}})
            if "error" in init:
                raise RuntimeError(init)
            send({"method": "initialized"})
            if session_id is None:
                request(2, "thread/list", {"limit": 100})
                return
            listing = request(3, "thread/list", {"limit": 100, "sourceKinds": ["cli"]})
            listed = any(item["id"] == session_id for item in listing.get("result", {}).get("data", []))
            response = request(2, "thread/read", {"threadId": session_id, "includeTurns": True})
            if "error" in response:
                raise RuntimeError(response)
            thread = response["result"]["thread"]
            assert thread["id"] == session_id, thread
            assert thread["cwd"] == str(project), thread
            assert "migration smoke test" in json.dumps(thread), thread
            print(json.dumps({"native_read": True, "mapped_cwd": True, "native_list_before_read": listed, "existing_target_database": True, "model_turn_sent": False}))
            if not listed:
                raise RuntimeError("Native read passed, but imported session is not discoverable in thread/list")
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()

with tempfile.TemporaryDirectory(prefix="agentmoving-native-") as temporary:
    root = Path(temporary).resolve()
    source, target, project = root / "source", root / "target", root / "project"
    project.mkdir()
    session_id = "019a0123-4567-7890-abcd-123456789abc"
    transcript = source / "sessions/2026/09/21" / f"rollout-2026-09-21T00-00-00-{session_id}.jsonl"
    transcript.parent.mkdir(parents=True)
    records = [
        {"timestamp": "2026-09-21T00:00:00Z", "type": "session_meta", "payload": {
            "id": session_id, "timestamp": "2026-09-21T00:00:00Z", "cwd": "/old/project",
            "originator": "codex_cli_rs", "cli_version": "0.155.1", "source": "cli",
            "model_provider": "openai", "base_instructions": {"text": "Test fixture"}}},
        {"timestamp": "2026-09-21T00:00:01Z", "type": "response_item", "payload": {
            "type": "message", "role": "user", "content": [{"type": "input_text", "text": "migration smoke test"}]}},
        {"timestamp": "2026-09-21T00:00:01Z", "type": "event_msg", "payload": {
            "type": "user_message", "message": "migration smoke test", "images": [], "local_images": []}},
    ]
    transcript.write_text("".join(json.dumps(v) + "\n" for v in records))
    bundle = root / "test.zip"
    subprocess.run([str(binary), "export", "--agent", "codex", "--root", str(source), "--all", "-o", str(bundle)], check=True)
    target.mkdir()
    probe(root, target, project)
    assert list(target.glob("state_*.sqlite")), "Native baseline database was not created"
    subprocess.run([str(binary), "import", str(bundle), "--target", f"codex={target}", "--map", f"/old/project={project}"], check=True)
    probe(root, target, project, session_id)
