"""End-to-end test for the Rust team-mode MCP server.

Drives the 9-tool surface over stdio JSON-RPC and validates the full
lead -> worker -> claude-code -> reply chain.
"""

from __future__ import annotations

import json
import os
import queue
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any, Optional

ROOT = Path(r"E:\aigc内容整理\agent-teams-rs-team-mode")
MCP_BIN = ROOT / "target" / "debug" / "team_mode_mcp.exe"
LOG_PATH = ROOT / "mcp_server.log"
TEAMS_DIR = Path(os.path.expanduser("~")) / ".claude" / "teams" / "mcp-test"


# ---------------------------------------------------------------------------
# Minimal JSON-RPC 2.0 client over a child process's stdio
# ---------------------------------------------------------------------------


class RpcClient:
    def __init__(self, proc: subprocess.Popen):
        self.proc = proc
        self._id = 0
        self._pending: dict[int, queue.Queue] = {}
        self._lock = threading.Lock()
        self._notifications: list[dict] = []
        self._alive = True
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()

    def _read_loop(self) -> None:
        assert self.proc.stdout is not None
        for raw in self.proc.stdout:
            line = raw.decode("utf-8", errors="replace").strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                print(f"[non-json stdout] {line}")
                continue
            if "id" in msg and msg.get("id") is not None and ("result" in msg or "error" in msg):
                rid = msg["id"]
                with self._lock:
                    q = self._pending.pop(rid, None)
                if q is not None:
                    q.put(msg)
            else:
                # Notification / server->client request: just record.
                method = msg.get("method", "?")
                self._notifications.append(msg)
                # Be quiet about spammy progress notifications.
                if not method.startswith("notifications/"):
                    print(f"[server->client] {method}: {json.dumps(msg, ensure_ascii=False)[:200]}")
        self._alive = False

    def call(self, method: str, params: Optional[dict] = None, timeout: float = 30.0) -> dict:
        with self._lock:
            self._id += 1
            rid = self._id
            q: queue.Queue = queue.Queue(maxsize=1)
            self._pending[rid] = q
        req = {"jsonrpc": "2.0", "id": rid, "method": method}
        if params is not None:
            req["params"] = params
        self._send_raw(req)
        try:
            resp = q.get(timeout=timeout)
        except queue.Empty:
            raise TimeoutError(f"No response within {timeout}s for {method} (id={rid})")
        return resp

    def notify(self, method: str, params: Optional[dict] = None) -> None:
        req = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            req["params"] = params
        self._send_raw(req)

    def _send_raw(self, obj: dict) -> None:
        assert self.proc.stdin is not None
        data = (json.dumps(obj, ensure_ascii=False) + "\n").encode("utf-8")
        self.proc.stdin.write(data)
        self.proc.stdin.flush()


# ---------------------------------------------------------------------------
# Pretty helpers
# ---------------------------------------------------------------------------


def short(obj: Any, limit: int = 400) -> str:
    s = json.dumps(obj, ensure_ascii=False)
    if len(s) > limit:
        return s[:limit] + f"... <+{len(s) - limit} chars>"
    return s


def tool_call(client: RpcClient, name: str, args: dict, timeout: float = 30.0) -> tuple[bool, dict, Optional[dict]]:
    """Invoke tools/call. Returns (ok, raw_response, parsed_inner_json_or_none)."""
    print(f"\n>>> tools/call {name} args={short(args, 300)}")
    resp = client.call("tools/call", {"name": name, "arguments": args}, timeout=timeout)
    if "error" in resp:
        print(f"<<< [X] RPC ERROR: {short(resp['error'])}")
        return False, resp, None
    result = resp.get("result", {})
    is_error = result.get("isError", False)
    content = result.get("content", [])
    inner = None
    if content and isinstance(content, list) and content[0].get("type") == "text":
        text = content[0].get("text", "")
        try:
            inner = json.loads(text)
        except json.JSONDecodeError:
            inner = {"_raw_text": text}
    tag = "[X] isError" if is_error else "[OK]"
    print(f"<<< {tag} result={short(inner if inner is not None else result, 500)}")
    return (not is_error), resp, inner


def dump_log_tail(n: int = 50) -> None:
    if not LOG_PATH.exists():
        print(f"(no log file at {LOG_PATH})")
        return
    try:
        data = LOG_PATH.read_text(encoding="utf-8", errors="replace").splitlines()
    except Exception as exc:  # pragma: no cover
        print(f"(could not read log: {exc})")
        return
    print(f"\n===== {LOG_PATH.name} last {n} of {len(data)} lines =====")
    for line in data[-n:]:
        print(line)
    print("===== end log =====\n")


# ---------------------------------------------------------------------------
# Main flow
# ---------------------------------------------------------------------------


def main() -> int:
    results: dict[str, str] = {}
    worker_reply_text: Optional[str] = None
    worker_reply_found = False

    if LOG_PATH.exists():
        LOG_PATH.unlink()

    env = os.environ.copy()
    env["CLAUDE_CODE_GIT_BASH_PATH"] = r"D:\Git\bin\bash.exe"
    env["RUST_LOG"] = "info"

    log_file = open(LOG_PATH, "wb")
    print(f"[boot] starting {MCP_BIN}")
    proc = subprocess.Popen(
        [str(MCP_BIN)],
        cwd=str(ROOT),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=log_file,
        env=env,
        bufsize=0,
    )
    client = RpcClient(proc)

    try:
        # ------------------------------------------------------------------
        # initialize + tools/list
        # ------------------------------------------------------------------
        print("\n>>> initialize")
        init_resp = client.call(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "mcp-e2e", "version": "0.1"},
            },
            timeout=15,
        )
        if "error" in init_resp:
            print(f"<<< [X] initialize error: {short(init_resp['error'])}")
            results["initialize"] = "FAIL"
            return 1
        print(f"<<< [OK] initialize: {short(init_resp.get('result', {}), 300)}")
        results["initialize"] = "PASS"
        try:
            client.notify("notifications/initialized")
        except Exception:
            pass

        print("\n>>> tools/list")
        tl = client.call("tools/list", {}, timeout=15)
        tools = [t["name"] for t in tl.get("result", {}).get("tools", [])]
        print(f"<<< tools = {tools}")
        expected = {
            "team_create",
            "team_list",
            "team_delete",
            "member_add",
            "member_remove",
            "member_list",
            "send_message",
            "spawn_member",
            "shutdown_member",
        }
        if set(tools) == expected and len(tools) == 9:
            results["tools/list"] = "PASS (9 tools)"
        else:
            missing = expected - set(tools)
            extra = set(tools) - expected
            results["tools/list"] = f"FAIL (count={len(tools)}, missing={missing}, extra={extra})"

        # ------------------------------------------------------------------
        # team_create
        # ------------------------------------------------------------------
        ok, _, _ = tool_call(
            client,
            "team_create",
            {"id": "mcp-test", "name": "MCP Test", "lead_member_id": "lead"},
        )
        results["team_create"] = "PASS" if ok else "FAIL"

        # member_add: lead
        ok, _, _ = tool_call(
            client,
            "member_add",
            {
                "id": "lead",
                "team_id": "mcp-test",
                "name": "Lead",
                "handle": "lead",
                "kind": "lead",
                "role_label": "Team Lead",
            },
        )
        results["member_add(lead)"] = "PASS" if ok else "FAIL"

        # member_add: worker
        ok, _, _ = tool_call(
            client,
            "member_add",
            {
                "id": "worker",
                "team_id": "mcp-test",
                "name": "Worker",
                "handle": "worker",
                "kind": "member",
                "role_label": "Tester",
            },
        )
        results["member_add(worker)"] = "PASS" if ok else "FAIL"

        # spawn_member
        ok_spawn, spawn_resp, spawn_inner = tool_call(
            client,
            "spawn_member",
            {
                "member_id": "worker",
                "adapter": "claude-code",
                "system_prompt": "You are a brief, friendly test worker. Reply with one short sentence.",
            },
            timeout=30,
        )
        results["spawn_member"] = "PASS" if ok_spawn else "FAIL"
        if not ok_spawn:
            print("\n[!] spawn_member failed — dumping server log tail:")
            dump_log_tail(50)
        else:
            print("\n[*] waiting 5s for claude subprocess to become ready...")
            time.sleep(5)

        # send_message
        ok_send, _, _ = tool_call(
            client,
            "send_message",
            {
                "team_id": "mcp-test",
                "from": "lead",
                "text": "@worker please reply with just the word pong",
            },
        )
        results["send_message"] = "PASS" if ok_send else "FAIL"

        # Poll resources/read for the main room
        print("\n[*] polling room messages for worker reply (up to 60s)...")
        deadline = time.time() + 60
        attempt = 0
        last_msgs: list[dict] = []
        while time.time() < deadline:
            attempt += 1
            try:
                r = client.call(
                    "resources/read",
                    {"uri": "team://mcp-test/rooms/main/messages"},
                    timeout=10,
                )
            except TimeoutError as exc:
                print(f"  [{attempt}] resources/read timeout: {exc}")
                time.sleep(3)
                continue
            if "error" in r:
                print(f"  [{attempt}] resources/read error: {short(r['error'])}")
                time.sleep(3)
                continue
            contents = r.get("result", {}).get("contents", [])
            payload_text = ""
            if contents:
                payload_text = contents[0].get("text", "")
            try:
                payload = json.loads(payload_text) if payload_text else {}
            except json.JSONDecodeError:
                payload = {"_raw": payload_text}
            msgs = payload.get("messages", []) if isinstance(payload, dict) else []
            last_msgs = msgs
            reply_msgs = [
                m
                for m in msgs
                if (m.get("sender") == "worker" or m.get("sender_member_id") == "worker")
                and (m.get("kind") == "reply" or m.get("type") == "reply")
            ]
            print(f"  [{attempt}] messages={len(msgs)} reply_from_worker={len(reply_msgs)}")
            if reply_msgs:
                worker_reply_found = True
                m = reply_msgs[-1]
                worker_reply_text = m.get("text") or m.get("body") or json.dumps(m, ensure_ascii=False)
                print(f"  [{attempt}] [OK] worker reply: {worker_reply_text}")
                break
            time.sleep(3)

        if worker_reply_found:
            results["worker_reply"] = "PASS"
        else:
            results["worker_reply"] = "FAIL (no reply within 60s)"
            print("\n[!] No worker reply detected — dumping diagnostics.")
            print(f"Last observed messages ({len(last_msgs)}):")
            for m in last_msgs[-10:]:
                print("  -", short(m, 240))
            print("\n[!] MCP server log (tail 200):")
            dump_log_tail(200)
            if TEAMS_DIR.exists():
                msg_dir = TEAMS_DIR / "messages"
                print(f"\n[!] Listing {msg_dir}:")
                if msg_dir.exists():
                    for p in sorted(msg_dir.rglob("*")):
                        if p.is_file():
                            try:
                                sz = p.stat().st_size
                            except OSError:
                                sz = -1
                            print(f"  {p}  ({sz} bytes)")
                            if 0 < sz < 20000:
                                try:
                                    print("    ---")
                                    print(p.read_text(encoding="utf-8", errors="replace"))
                                    print("    ---")
                                except Exception as exc:
                                    print(f"    (unreadable: {exc})")
                else:
                    print(f"  (no {msg_dir})")
            else:
                print(f"\n[!] Team dir {TEAMS_DIR} not found.")

        # ------------------------------------------------------------------
        # Cleanup: shutdown + delete (always try)
        # ------------------------------------------------------------------
        ok, _, _ = tool_call(client, "shutdown_member", {"member_id": "worker"}, timeout=15)
        results["shutdown_member"] = "PASS" if ok else "FAIL"

        ok, _, _ = tool_call(client, "team_delete", {"team_id": "mcp-test"}, timeout=15)
        results["team_delete"] = "PASS" if ok else "FAIL"

    except Exception as exc:
        print(f"\n[FATAL] {type(exc).__name__}: {exc}")
        import traceback

        traceback.print_exc()
        results.setdefault("exception", str(exc))
    finally:
        print("\n[boot] killing MCP server process...")
        try:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
        except Exception as exc:
            print(f"  (terminate error: {exc})")
        try:
            log_file.close()
        except Exception:
            pass
        dump_log_tail(50)

    # --------------------------------------------------------------------
    # Summary
    # --------------------------------------------------------------------
    print("\n" + "=" * 70)
    print("SUMMARY")
    print("=" * 70)
    for k, v in results.items():
        marker = "[PASS]" if v.startswith("PASS") else "[FAIL]"
        print(f"  {marker}  {k:20s}  {v}")
    print("-" * 70)
    print(f"worker_received_message: {'yes' if results.get('send_message', '').startswith('PASS') else 'no'}")
    print(f"worker_replied:          {'yes' if worker_reply_found else 'no'}")
    print(f"worker_reply_text:       {worker_reply_text!r}")
    overall_ok = (
        worker_reply_found
        and all(v.startswith("PASS") for k, v in results.items() if k != "worker_reply")
    )
    print(f"overall:                 {'PASS' if overall_ok else 'FAIL'}")
    return 0 if overall_ok else 2


if __name__ == "__main__":
    sys.exit(main())
