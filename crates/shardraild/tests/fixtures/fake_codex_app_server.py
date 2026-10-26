#!/usr/bin/env python3
"""Scripted Codex app-server for shardraild tests. Speaks newline JSON-RPC only."""

import fcntl
import json
import os
import select
import stat
import sys
import time

MODE = os.environ.get("SHARDRAIL_CODEX_FAKE_MODE", "happy")
TRACE = os.environ.get("SHARDRAIL_CODEX_TRACE")
ARGV_OUT = os.environ.get("SHARDRAIL_CODEX_ARGV_OUT")
PID_OUT = os.environ.get("SHARDRAIL_CODEX_PID_OUT")
FD_OUT = os.environ.get("SHARDRAIL_CODEX_FD_OUT")
HERE = os.path.dirname(os.path.abspath(__file__))
AUTOWRITE = os.path.join(HERE, "codex-0.160.0-on-request-autowrite.jsonl")
SECRET = "Qm9tV2l6a0p4Y2xRbFh2N1N4bE5vV2V5WjRrU2J1R3A"
# Present on the config/read wire. The daemon must not copy it into a journal,
# log, or trace. The fixture redacts env and headers before it writes a trace.
DECOY_SECRET = "mcp-decoy-secret-SHOULD-NOT-LEAK-9c1e4b"


def scrub(node):
    if isinstance(node, dict):
        for key in ("env", "headers"):
            if isinstance(node.get(key), dict):
                node[key] = {name: "[redacted]" for name in node[key]}
        for value in node.values():
            scrub(value)
    elif isinstance(node, list):
        for item in node:
            scrub(item)


def log(message):
    if not TRACE:
        return
    cloned = json.loads(json.dumps(message))
    scrub(cloned)
    with open(TRACE, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(cloned) + "\n")


def fd_target(fd):
    # macOS readlink("/dev/fd/N") returns EINVAL. F_GETPATH is the path lookup.
    try:
        raw = fcntl.fcntl(fd, 50, b"\0" * 1024)
    except OSError:
        return ""
    if isinstance(raw, bytes):
        return raw.split(b"\0", 1)[0].decode("utf-8", "replace")
    return ""


def report_fds():
    if not FD_OUT:
        return
    numbers = set(range(0, 64))
    try:
        numbers.update(int(name) for name in os.listdir("/dev/fd") if name.isdigit())
    except OSError:
        pass
    rows = []
    for fd in sorted(numbers):
        try:
            info = os.fstat(fd)
        except OSError:
            continue
        rows.append(
            {
                "fd": fd,
                "target": fd_target(fd),
                "dir": stat.S_ISDIR(info.st_mode),
                "dev": info.st_dev,
                "ino": info.st_ino,
            }
        )
    with open(FD_OUT, "a", encoding="utf-8") as handle:
        handle.write(json.dumps({"pid": os.getpid(), "fds": rows}) + "\n")


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()
    log({"direction": "out", "message": message})


def read_message():
    line = sys.stdin.readline()
    if line == "":
        return None
    message = json.loads(line)
    log({"direction": "in", "message": message})
    return message


def reply(message_id, result):
    send({"jsonrpc": "2.0", "id": message_id, "result": result})


def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})


def request(message_id, method, params):
    send({"jsonrpc": "2.0", "id": message_id, "method": method, "params": params})


def thread_result(params):
    reviewer = "auto_review" if MODE == "bad-reviewer" else "user"
    cwd = params.get("cwd") or "/"
    if MODE == "bad-sandbox":
        sandbox = {"type": "dangerFullAccess"}
    elif MODE == "readonly-network":
        sandbox = {"type": "readOnly", "networkAccess": True}
    else:
        sandbox = {"type": "workspaceWrite"}
    result = {
        "approvalPolicy": "on-request" if MODE == "weak-policy" else (params.get("approvalPolicy") or "untrusted"),
        "approvalsReviewer": reviewer,
        "cwd": cwd,
        "model": "fake",
        "modelProvider": "fake",
        "sandbox": sandbox,
        "thread": {
            "cliVersion": "0.160.0",
            "createdAt": 0,
            "cwd": cwd,
            "ephemeral": params.get("ephemeral") is True and MODE != "bad-ephemeral",
            "id": "thread-1",
            "modelProvider": "fake",
            "path": "/tmp/rollout-thread-1.jsonl" if MODE == "thread-path" else None,
            "preview": "",
            "projectId": None,
            "sessionId": "session-1",
            "source": "appServer",
            "status": {"type": "idle"},
            "turns": [],
            "updatedAt": 0,
        },
    }
    if MODE == "omit-reviewer":
        result.pop("approvalsReviewer", None)
    return result


def turn_result(turn_id):
    result = {"turn": {"id": turn_id, "items": [], "status": "inProgress"}}
    if MODE == "bad-turn-reviewer":
        result["approvalsReviewer"] = "auto_review"
    return result


def command_approval(turn_id, cwd):
    request(
        11,
        "item/commandExecution/requestApproval",
        {
            "itemId": "item-cmd",
            "threadId": "thread-1",
            "turnId": turn_id,
            "startedAtMs": 0,
            "command": "python3 -m unittest",
            "cwd": cwd,
            "reason": "run tests",
        },
    )


def session_child():
    return any(str(arg).startswith("skills.config=") for arg in sys.argv)


def requested_cwd(message):
    params = message.get("params") or {}
    cwds = params.get("cwds") or []
    if cwds and isinstance(cwds[0], str) and cwds[0].startswith("/"):
        return cwds[0]
    return "/tmp"


def skill_row(name, enabled):
    return {
        "name": name,
        "description": "fixture skill",
        "enabled": enabled,
        "path": "/tmp/skill/fixture/SKILL.md",
        "scope": "user",
    }


def skills_result(cwd, skills):
    return {"data": [{"cwd": cwd, "errors": [], "skills": skills}]}


def hooks_result(cwd, hooks):
    return {"data": [{"cwd": cwd, "errors": [], "hooks": hooks, "warnings": []}]}


def empty_plugins():
    return {"marketplaces": [], "marketplaceLoadErrors": []}


def empty_apps():
    return {"data": [], "nextCursor": None}


def config_servers(child):
    # Wire shape measured on codex-cli 0.160.0: result.config.mcp_servers is an
    # object keyed by server name. env and headers carry the decoy on purpose.
    servers = {
        "node_repl": {"enabled": True, "command": "node", "env": {"TOKEN": DECOY_SECRET}},
        "odd.server": {"enabled": True, "command": "odd", "env": {"TOKEN": DECOY_SECRET}},
        "soma": {
            "enabled": True,
            "command": "soma",
            "env": {"TOKEN": DECOY_SECRET},
            "headers": {"Authorization": DECOY_SECRET},
        },
    }
    if child:
        for server in servers.values():
            server["enabled"] = False
        if MODE == "mcp-enabled":
            servers["soma"]["enabled"] = True
    return {"config": {"mcp_servers": servers, "model": "fake"}, "origins": {}}


def command_hook():
    return {
        "currentHash": "fixture",
        "displayOrder": 0,
        "enabled": True,
        "eventName": "sessionStart",
        "isManaged": False,
        "key": "sessionStart:0",
        "source": "plugin",
        "sourcePath": "/tmp/hooks/sessionStart",
        "timeoutSec": 10,
        "trustStatus": "trusted",
        "handlerType": "command",
        "command": "node /tmp/hooks/sessionStart.js",
        "async": False,
    }


def enabled_plugin():
    return {
        "marketplaces": [
            {
                "name": "local",
                "plugins": [
                    {
                        "authPolicy": "ON_INSTALL",
                        "enabled": True,
                        "id": "plugin-1",
                        "installPolicy": "AVAILABLE",
                        "installed": True,
                        "name": "ponytail",
                        "source": {"type": "local", "path": "/tmp/plugins/ponytail"},
                    }
                ],
            }
        ],
        "marketplaceLoadErrors": [],
    }


def recorded_results(filename):
    results = {}
    pending = None
    path = os.path.join(HERE, filename)
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            if not line.strip():
                continue
            item = json.loads(line)
            frame = item["frame"]
            if item.get("dir") == "client":
                pending = frame.get("method")
            elif item.get("dir") == "server" and pending and "result" in frame:
                results[pending] = frame["result"]
                pending = None
    return results


def extension_result(method, message):
    cwd = requested_cwd(message)
    child = session_child()
    if MODE == "list-error" and child and method == "skills/list":
        return "error"
    if MODE == "recorded":
        filename = (
            "codex-0.160.0-extensions-disabled.jsonl"
            if child
            else "codex-0.160.0-extensions-default.trimmed.jsonl"
        )
        results = recorded_results(filename)
        if method in results:
            return results[method]
    if MODE == "extensions-on" and not child and method == "skills/list":
        return skills_result(cwd, [skill_row("cloudflare", True)])
    if MODE == "extensions-on" and child and method == "plugin/installed":
        return enabled_plugin()
    if MODE == "skill-enabled" and method == "skills/list":
        return skills_result(cwd, [skill_row("cloudflare", True)])
    if MODE == "hook-present" and child and method == "hooks/list":
        return hooks_result(cwd, [command_hook()])
    if method == "skills/list":
        return skills_result(cwd, [])
    if method == "hooks/list":
        return hooks_result(cwd, [])
    if method == "plugin/installed":
        return empty_plugins()
    if method == "app/list":
        return empty_apps()
    return {}


def complete(turn_id, status):
    notify(
        "turn/completed",
        {"threadId": "thread-1", "turn": {"id": turn_id, "items": [], "status": status}},
    )


def main():
    report_fds()
    if ARGV_OUT:
        with open(ARGV_OUT, "w", encoding="utf-8") as handle:
            json.dump(sys.argv, handle)
    if PID_OUT:
        with open(PID_OUT, "w", encoding="utf-8") as handle:
            handle.write(str(os.getpid()))
    turns = 0
    while True:
        message = read_message()
        if message is None:
            return
        method = message.get("method")
        if method == "initialize" and "id" in message:
            reply(
                message["id"],
                {
                    "codexHome": "/tmp/fake-codex-home",
                    "platformFamily": "unix",
                    "platformOs": "macos",
                    "userAgent": "fake-codex/0.160.0",
                },
            )
            continue
        if method == "initialized":
            # codex-cli 0.160.0 sends these before any thread exists (observed live).
            notify(
                "configWarning",
                {
                    "details": None,
                    "summary": "Codex is ignoring 1 unrecognized configuration setting.\n  user (/Users/example/.codex/config.toml): `features.rmcp_client` is ignored.",
                },
            )
            notify(
                "remoteControl/status/changed",
                {"environmentId": None, "installationId": "00000000-0000-4000-8000-000000000000", "serverName": "fake", "status": "disabled"},
            )
            continue
        if method == "config/read":
            if MODE == "config-read-error" and session_child():
                send(
                    {
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "error": {"code": -32000, "message": "config/read failed"},
                    }
                )
            else:
                reply(message["id"], config_servers(session_child()))
            continue
        if method in ("skills/list", "hooks/list", "plugin/installed", "app/list"):
            result = extension_result(method, message)
            if result == "error":
                send(
                    {
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "error": {"code": -32000, "message": "skills/list failed"},
                    }
                )
            else:
                reply(message["id"], result)
            continue
        if method == "thread/start":
            result = thread_result(message.get("params") or {})
            reply(message["id"], result)
            notified_thread = dict(result["thread"])
            if MODE == "notification-path":
                notified_thread["path"] = "/tmp/rollout-notification.jsonl"
            if MODE == "notification-ephemeral":
                notified_thread["ephemeral"] = False
            notify("thread/started", {"thread": notified_thread})
            if MODE in ("mcp-item-started", "mcp-item-updated", "mcp-item-completed"):
                item_method = {
                    "mcp-item-started": "item/started",
                    "mcp-item-updated": "item/updated",
                    "mcp-item-completed": "item/completed",
                }[MODE]
                notify(item_method, {"item": {"id": "mcp-item", "type": "mcpToolCall"}})
                return
            if MODE == "dynamic-tool-item":
                notify("item/started", {"item": {"id": "dynamic-tool", "type": "dynamicToolCall"}})
                return
            if MODE == "mcp-item-method":
                notify("item/mcpToolCall/progress", {"itemId": "mcp-item"})
                return
            if MODE == "mcp-startup":
                notify(
                    "mcpServer/startupStatus/updated",
                    {"name": "soma", "status": "starting", "threadId": "thread-1"},
                )
            if MODE == "mcp-request":
                request(
                    "mcp-1",
                    "mcpServer/elicitation/request",
                    {"message": "no", "serverName": "soma"},
                )
            if MODE == "idle":
                time.sleep(30)
            continue
        if method == "turn/start":
            turns += 1
            turn_id = f"turn-{turns}"
            params = message.get("params") or {}
            if MODE == "slow-turn":
                time.sleep(2)
            reply(message["id"], turn_result(turn_id))
            if MODE == "mcp-during-turn":
                notify(
                    "mcpServer/startupStatus/updated",
                    {"name": "soma", "status": "ready", "threadId": "thread-1"},
                )
                nxt = read_message()
                if nxt and nxt.get("method") == "turn/interrupt":
                    reply(nxt["id"], {})
                return
            if MODE == "slow-turn":
                complete(turn_id, "completed")
                continue
            if MODE == "crash":
                log({"direction": "exit", "reason": "crash"})
                os._exit(1)
            if MODE == "unknown":
                request(
                    "user-1",
                    "item/tool/requestUserInput",
                    {"threadId": "thread-1", "turnId": turn_id, "questions": []},
                )
                read_message()
                complete(turn_id, "completed")
                continue
            if MODE == "hold":
                command_approval(turn_id, params.get("cwd"))
                read_message()
                complete(turn_id, "completed")
                continue
            if MODE == "disconnect":
                command_approval(turn_id, params.get("cwd"))
                # sys.stdout.close() leaves fd 1 open, so the daemon never sees EOF.
                sys.stdout.flush()
                os.close(1)
                deadline = time.time() + 3
                while time.time() < deadline:
                    remaining = deadline - time.time()
                    readable, _, _ = select.select([sys.stdin], [], [], remaining)
                    if not readable:
                        break
                    line = sys.stdin.readline()
                    if line == "":
                        break
                    try:
                        log({"direction": "in", "message": json.loads(line)})
                    except json.JSONDecodeError:
                        log({"direction": "in", "raw": line})
                return
            if MODE == "bad-turn-reviewer":
                complete(turn_id, "failed")
                continue
            if MODE == "autowrite":
                # Replays the notification order Codex 0.160.0 produced in the S1-lite
                # live run under on-request + workspace-write: a patch applied with no
                # item/fileChange/requestApproval in between.
                with open(AUTOWRITE, encoding="utf-8") as handle:
                    for line in handle:
                        if line.strip():
                            frame = json.loads(line.replace("TURN_ID", turn_id))
                            send(frame)
                while True:
                    nxt = read_message()
                    if nxt is None:
                        return
                    if nxt.get("method") == "turn/interrupt":
                        reply(nxt["id"], {})
                        complete(turn_id, "interrupted")
                        break
                continue
            if turns == 1:
                notify(
                    "item/agentMessage/delta",
                    {
                        "delta": f"token {SECRET} ok",
                        "itemId": "item-msg",
                        "threadId": "thread-1",
                        "turnId": turn_id,
                    },
                )
                notify(
                    "turn/started",
                    {
                        "threadId": "thread-1",
                        "turn": {"id": turn_id, "items": [], "status": "inProgress"},
                    },
                )
                command_approval(turn_id, params.get("cwd"))
                decision = read_message()
                notify("serverRequest/resolved", {"requestId": 11, "threadId": "thread-1"})
                chosen = (decision or {}).get("result", {}).get("decision")
                if chosen == "decline":
                    notify(
                        "item/started",
                        {
                            "item": {
                                "type": "fileChange",
                                "id": "item-file",
                                "changes": [
                                    {"path": "calc.py", "diff": "+def sub(a, b):\n+    return a - b\n", "kind": "update"}
                                ],
                            },
                            "startedAtMs": 0,
                            "threadId": "thread-1",
                            "turnId": turn_id,
                        },
                    )
                    request(
                        12,
                        "item/fileChange/requestApproval",
                        {
                            "itemId": "item-file",
                            "threadId": "thread-1",
                            "turnId": turn_id,
                            "startedAtMs": 0,
                            "reason": "edit calc.py",
                        },
                    )
                    read_message()
                    notify("serverRequest/resolved", {"requestId": 12, "threadId": "thread-1"})
                complete(turn_id, "completed")
                continue
            notify(
                "item/agentMessage/delta",
                {
                    "delta": "Step one explains calc.py.",
                    "itemId": "item-msg-2",
                    "threadId": "thread-1",
                    "turnId": turn_id,
                },
            )
            while True:
                nxt = read_message()
                if nxt is None:
                    return
                if nxt.get("method") == "turn/interrupt":
                    reply(nxt["id"], {})
                    complete(turn_id, "interrupted")
                    break
            continue
        if method == "turn/interrupt":
            reply(message["id"], {})
            continue


if __name__ == "__main__":
    try:
        main()
    except Exception as error:  # noqa: BLE001 - test fixture must record why it stopped
        log({"direction": "exit", "error": str(error)})
        raise
