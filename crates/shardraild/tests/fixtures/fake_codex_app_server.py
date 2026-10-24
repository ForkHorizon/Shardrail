#!/usr/bin/env python3
"""Scripted Codex app-server for shardraild tests. Speaks newline JSON-RPC only."""

import json
import os
import select
import sys
import time

MODE = os.environ.get("SHARDRAIL_CODEX_FAKE_MODE", "happy")
TRACE = os.environ.get("SHARDRAIL_CODEX_TRACE")
ARGV_OUT = os.environ.get("SHARDRAIL_CODEX_ARGV_OUT")
PID_OUT = os.environ.get("SHARDRAIL_CODEX_PID_OUT")
SECRET = "Qm9tV2l6a0p4Y2xRbFh2N1N4bE5vV2V5WjRrU2J1R3A"


def log(message):
    if not TRACE:
        return
    with open(TRACE, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(message) + "\n")


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
    sandbox = {"type": "dangerFullAccess"} if MODE == "bad-sandbox" else {"type": "workspaceWrite"}
    result = {
        "approvalPolicy": params.get("approvalPolicy") or "on-request",
        "approvalsReviewer": reviewer,
        "cwd": params.get("cwd") or "/",
        "model": "fake",
        "modelProvider": "fake",
        "sandbox": sandbox,
        "thread": {"id": "thread-1"},
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


def complete(turn_id, status):
    notify(
        "turn/completed",
        {"threadId": "thread-1", "turn": {"id": turn_id, "items": [], "status": status}},
    )


def main():
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
            reply(message["id"], {"userAgent": "fake-codex/0.160.0"})
            continue
        if method == "initialized":
            continue
        if method == "thread/start":
            reply(message["id"], thread_result(message.get("params") or {}))
            if MODE == "idle":
                time.sleep(30)
            continue
        if method == "turn/start":
            turns += 1
            turn_id = f"turn-{turns}"
            params = message.get("params") or {}
            reply(message["id"], turn_result(turn_id))
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
