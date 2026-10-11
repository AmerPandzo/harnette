#!/usr/bin/env python3
"""Scripted ACP agent standing in for `copilot --acp` in tests.
A first prompt containing SCENARIO:quota answers like an exhausted Copilot plan; anything else does work."""
import json, os, sys

scenario = "work"
cwd = None
next_id = 1000


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def read():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    return json.loads(line)


def update(u):
    send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1", "update": u}})


def say(text):
    update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}})


def ask(call_id, kind, title, raw=None, locations=None):
    """Request permission for a tool call; return the chosen option id (or 'cancelled')."""
    global next_id
    next_id += 1
    tc = {"toolCallId": call_id, "title": title, "kind": kind, "status": "pending"}
    if raw is not None:
        tc["rawInput"] = raw
    if locations is not None:
        tc["locations"] = locations
    update({"sessionUpdate": "tool_call", **tc})
    options = [
        {"optionId": "always", "name": "Always", "kind": "allow_always"},
        {"optionId": "once", "name": "Allow", "kind": "allow_once"},
        {"optionId": "no", "name": "Reject", "kind": "reject_once"},
    ]
    send({"jsonrpc": "2.0", "id": next_id, "method": "session/request_permission",
          "params": {"sessionId": "s1", "toolCall": tc, "options": options}})
    while True:
        m = read()
        if m.get("id") == next_id and "method" not in m:
            outcome = m["result"]["outcome"]
            choice = outcome.get("optionId", outcome.get("outcome"))
            status = "completed" if choice == "once" else "failed"
            update({"sessionUpdate": "tool_call_update", "toolCallId": call_id, "status": status,
                    "content": [{"type": "content", "content": {"type": "text", "text": f"{title}: {choice}"}}]})
            return choice


prompts = 0
while True:
    m = read()
    method, mid = m.get("method"), m.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": 1, "agentCapabilities": {}}})
    elif method == "session/new":
        cwd = m["params"]["cwd"]
        send({"jsonrpc": "2.0", "id": mid, "result": {"sessionId": "s1", "models": {"currentModelId": "fake-model"}}})
    elif method == "session/prompt":
        prompts += 1
        text = m["params"]["prompt"][0]["text"]
        if prompts == 1 and "SCENARIO:quota" in text:
            scenario = "quota"
        if scenario == "quota":
            say("Error: You have exceeded your monthly quota (Request ID: TEST)")
        elif prompts == 1:
            say("Working on it.")
            a = ask("t1", "edit", "Edit src/a.js", locations=[{"path": os.path.join(cwd, "src/a.js")}])
            b = ask("t2", "execute", "Push", raw={"command": "git push origin main"})
            c = ask("t3", "execute", "Deploy", raw={"command": "make deploy"})
            say(f"outcomes {a},{b},{c}")
        else:
            say("second turn saw: " + ("NOTE" if "Note from the human supervisor: hurry" in text else "no note"))
        send({"jsonrpc": "2.0", "id": mid, "result": {"stopReason": "end_turn"}})
    elif method == "session/cancel":
        pass
    elif mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "unknown"}})
