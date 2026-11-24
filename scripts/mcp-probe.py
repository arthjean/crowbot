#!/usr/bin/env python3
"""A throwaway probe of how each provider agent treats an MCP server (US-001
of tasks/prd-crow-bot-harness.md). It is not the daemon's bridge.

    mcp-probe.py run <provider> <program> <work folder> <log file> [args...]
    mcp-probe.py server <log file>

`run` starts the agent as the daemon's profile of that provider starts it
(the arguments, the environment, the session `_meta`, and the session mode
that asks first), hands it this script as a stdio MCP server, sends the
prompts of the probe one turn at a time, allows every permission request
once, and writes every message to the log, one JSON object per line.
`server` is the MCP server: one read tool and one write tool, each call
logged.

Codex and Grok run in the homes the daemon owns, under `CROWBOT_DATA`
(default `~/.local/share/crowbot`); the probe writes their `config.toml` as
the daemon does before each start. Claude runs on the user's own sign-in,
as the daemon's Claude profile does.
"""

import json
import os
import sys
import threading
import time

SERVER_NAME = "crowbot_probe"
READ_ANSWER = "PROBE-READ-7731"


def log_to(path):
    lock = threading.Lock()

    def log(kind, payload):
        with lock, open(path, "a", encoding="utf-8") as file:
            file.write(json.dumps({"at": time.time(), "kind": kind, "payload": payload}) + "\n")

    return log


def server(log_path):
    log = log_to(log_path)
    tools = [
        {
            "name": "probe_read",
            "description": "Returns the probe's secret word. Read only.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False},
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "probe_write",
            "description": "Stores a note in the probe's notebook.",
            "inputSchema": {
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
                "additionalProperties": False,
            },
            "annotations": {"readOnlyHint": False, "destructiveHint": False},
        },
    ]
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        log("mcp_in", message)
        method, ident = message.get("method"), message.get("id")
        if ident is None:
            continue
        if method == "initialize":
            result = {
                "protocolVersion": message.get("params", {}).get("protocolVersion", "2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": SERVER_NAME, "version": "0.0.1"},
            }
        elif method == "tools/list":
            result = {"tools": tools}
        elif method == "tools/call":
            name = message["params"].get("name")
            if name == "probe_read":
                text = READ_ANSWER
            elif name == "probe_write":
                text = "stored: " + str(message["params"].get("arguments", {}).get("text"))
            else:
                text = "unknown tool"
            result = {"content": [{"type": "text", "text": text}], "isError": False}
        elif method == "ping":
            result = {}
        else:
            reply = {"jsonrpc": "2.0", "id": ident, "error": {"code": -32601, "message": "unknown method"}}
            log("mcp_out", reply)
            print(json.dumps(reply), flush=True)
            continue
        reply = {"jsonrpc": "2.0", "id": ident, "result": result}
        log("mcp_out", reply)
        print(json.dumps(reply), flush=True)


GROK_CONFIG = """[permission]
ask = ["Edit", "Write", "Bash", "WebFetch", "MCPTool"]

[ui]
remember_tool_approvals = false
"""
CODEX_CONFIG = "# Written by Crow Bot before each start. Changes here do not last.\n"


def profile(provider, data):
    """The environment, the session `_meta`, the mode, and the sign-in of the
    daemon's profile (crates/crowbotd/src/provider/*.rs)."""
    env, meta, mode, auth, args = {}, None, None, None, []
    if provider == "claude":
        meta = {"claudeCode": {"options": {"settingSources": [], "allowDangerouslySkipPermissions": False}}}
        mode = "default"
    elif provider == "codex":
        home = os.path.join(data, "providers", "codex", "home")
        with open(os.path.join(home, "config.toml"), "w", encoding="utf-8") as file:
            file.write(CODEX_CONFIG)
        env = {"CODEX_HOME": home, "INITIAL_AGENT_MODE": "read-only"}
        mode = "read-only"
    elif provider == "grok":
        home = os.path.join(data, "providers", "grok", "home")
        with open(os.path.join(home, "config.toml"), "w", encoding="utf-8") as file:
            file.write(GROK_CONFIG)
        env = {"GROK_HOME": home, "GROK_REMEMBER_TOOL_APPROVALS": "0"}
        for tool in ["CLAUDE", "CURSOR", "CODEX"]:
            for part in ["AGENTS", "HOOKS", "MCPS", "RULES", "SKILLS", "SESSIONS"]:
                env[f"GROK_{tool}_{part}_ENABLED"] = "0"
        args = ["--permission-mode", "default", "agent", "stdio"]
        auth = "grok.com"
    return env, meta, mode, auth, args


PROMPTS = [
    (
        "tools",
        f"Use the MCP server {SERVER_NAME}: call its tool probe_read, then call its tool "
        "probe_write with the text 'caw'. Then tell me exactly what each tool returned.",
    ),
    (
        "edit",
        "Create a file named edited.txt in the current folder that holds the single word caw. "
        "Use your file editing tool, not a shell command.",
    ),
    (
        "cd",
        "Run the shell command `cd sub` on its own, then, as a second and separate shell command, "
        "run `ls`. Tell me what ls printed.",
    ),
    ("compact", "/compact"),
    ("after", "Say the word done."),
]


class Client:
    def __init__(self, process, log):
        self.process = process
        self.log = log
        self.next_id = 0
        self.waiting = {}
        self.lock = threading.Lock()
        threading.Thread(target=self.read, daemon=True).start()

    def send(self, message):
        self.log("acp_out", message)
        self.process.stdin.write((json.dumps(message) + "\n").encode())
        self.process.stdin.flush()

    def request(self, method, params, patience=600):
        with self.lock:
            self.next_id += 1
            ident = self.next_id
            event = threading.Event()
            self.waiting[ident] = [event, None]
        self.send({"jsonrpc": "2.0", "id": ident, "method": method, "params": params})
        if not event.wait(patience):
            raise TimeoutError(method)
        return self.waiting.pop(ident)[1]

    def read(self):
        for line in self.process.stdout:
            try:
                message = json.loads(line)
            except ValueError:
                self.log("acp_noise", line.decode(errors="replace"))
                continue
            self.log("acp_in", message)
            if "method" in message and "id" in message:
                self.answer(message)
            elif "id" in message and message["id"] in self.waiting:
                slot = self.waiting[message["id"]]
                slot[1] = message
                slot[0].set()

    def answer(self, message):
        if message["method"] == "session/request_permission":
            options = message["params"].get("options", [])
            allow = next((o for o in options if o.get("kind") == "allow_once"), None)
            outcome = (
                {"outcome": "selected", "optionId": allow["optionId"]}
                if allow
                else {"outcome": "cancelled"}
            )
            result = {"outcome": outcome}
        else:
            self.send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "not offered"}})
            return
        self.send({"jsonrpc": "2.0", "id": message["id"], "result": result})


def run(provider, program, work, log_path, extra):
    import subprocess

    log = log_to(log_path)
    data = os.environ.get("CROWBOT_DATA", os.path.expanduser("~/.local/share/crowbot"))
    env, meta, mode, auth, args = profile(provider, data)
    os.makedirs(os.path.join(work, "sub"), exist_ok=True)
    with open(os.path.join(work, "sub", "inside.txt"), "w", encoding="utf-8") as file:
        file.write("inside\n")
    process = subprocess.Popen(
        [program, *extra, *args],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=open(log_path + ".stderr", "wb"),
        env={**os.environ, **env},
        cwd=work,
    )
    client = Client(process, log)
    initialized = client.request(
        "initialize",
        {"protocolVersion": 1, "clientCapabilities": {}, "clientInfo": {"name": "crowbot-probe", "version": "0"}},
    )
    print("initialize:", json.dumps(initialized.get("result", initialized))[:600])
    if auth:
        print("authenticate:", client.request("authenticate", {"methodId": auth}))
    server = {
        "name": SERVER_NAME,
        "command": sys.executable,
        "args": [os.path.abspath(__file__), "server", log_path + ".mcp"],
        "env": [],
    }
    params = {"cwd": work, "mcpServers": [server]}
    if meta:
        params["_meta"] = meta
    created = client.request("session/new", params)
    if "error" in created:
        print("session/new failed:", created["error"])
        return
    session = created["result"]["sessionId"]
    modes = created["result"].get("modes")
    print("modes:", json.dumps(modes))
    if mode and modes and modes.get("currentModeId") != mode:
        print("set_mode:", client.request("session/set_mode", {"sessionId": session, "modeId": mode}))
    only = os.environ.get("PROBE_ONLY")
    for name, text in PROMPTS:
        if only and name not in only.split(","):
            continue
        log("turn", name)
        started = time.time()
        answer = client.request(
            "session/prompt", {"sessionId": session, "prompt": [{"type": "text", "text": text}]}
        )
        print(f"turn {name}: {time.time() - started:.0f}s", json.dumps(answer)[:400])
    process.stdin.close()
    try:
        process.wait(10)
    except subprocess.TimeoutExpired:
        process.kill()


if __name__ == "__main__":
    if sys.argv[1] == "server":
        server(sys.argv[2])
    else:
        run(sys.argv[2], sys.argv[3], os.path.abspath(sys.argv[4]), os.path.abspath(sys.argv[5]), sys.argv[6:])
