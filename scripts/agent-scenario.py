"""Drives Plunger's MCP server the way an agent does, against scripts/tracker-api.py, and checks what comes back.

Run:  python scripts/agent-scenario.py path/to/plunger[.exe]

It starts the fake tracker on a free port and the real `plunger mcp` over stdio with a throwaway data
folder, so it touches nothing of yours. It talks to the MCP server itself (not the command line), because
that is the surface an agent uses: JSON-RPC arguments, tool results, structured content. It exists because
real use found problems our unit tests did not: text with line breaks and quotes in a JSON body, optional
fields, error pages that are not JSON, nulls, saved requests, and the shapes an agent gets wrong.
"""
import hashlib
import json
import os
import socket
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
PLUNGER = os.path.abspath(sys.argv[1]) if len(sys.argv) > 1 else "plunger"


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


PORT = free_port()
BASE = f"http://127.0.0.1:{PORT}"
DATA = tempfile.mkdtemp(prefix="plunger-scenario-")
failures = []


class Mcp:
    """Just enough of an MCP client: initialize, then tools/call over stdio."""

    def __init__(self):
        env = dict(os.environ, PLUNGER_DATA_DIR=DATA)
        self.proc = subprocess.Popen([PLUNGER, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.DEVNULL, text=True, encoding="utf-8", env=env, bufsize=1)
        self.next_id = 0
        self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                    "clientInfo": {"name": "agent-scenario", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message):
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def request(self, method, params):
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError("the MCP server ended")
            reply = json.loads(line)
            if reply.get("id") == self.next_id:
                return reply

    def call(self, tool, **arguments):
        """(is_error, structured result or None, text)"""
        reply = self.request("tools/call", {"name": tool, "arguments": arguments})
        if "error" in reply:
            return True, None, json.dumps(reply["error"])
        result = reply["result"]
        text = "".join(c.get("text", "") for c in result.get("content", []))
        return bool(result.get("isError")), result.get("structuredContent"), text

    def tools(self):
        return [t["name"] for t in self.request("tools/list", {})["result"]["tools"]]


def check(name, condition, detail=""):
    print(("ok   " if condition else "FAIL ") + name + ("" if condition else f"   {str(detail)[:300]}"))
    if not condition:
        failures.append(name)


server = subprocess.Popen([sys.executable, os.path.join(HERE, "tracker-api.py"), str(PORT)])
mcp = None
try:
    for _ in range(50):
        try:
            socket.create_connection(("127.0.0.1", PORT), timeout=0.2).close()
            break
        except OSError:
            time.sleep(0.1)
    mcp = Mcp()
    check("the tools are listed", {"send_request", "save_request", "run_workflow", "save_workflow"} <= set(mcp.tools()), mcp.tools())

    mcp.call("set_variable", name="base", value=BASE)
    mcp.call("set_variable", name="tracker_key", value="tok_tracker")
    auth = {"Authorization": "Bearer {{tracker_key}}"}

    # 1. text a person writes: line breaks, quotes and a backslash must arrive exactly as written
    text = 'First line\nSecond "quoted" line\nC:\\temp\\new and a tab\there'
    err, res, _ = mcp.call("send_request", url="{{base}}/issue", method="POST", headers=auth,
                           json={"fields": {"summary": "Bug {{n}}", "description": "{{description}}"}},
                           variables={"n": "1", "description": text}, select=["$.key", "header:Location"])
    key = (res or {}).get("selected", {}).get("$.key")
    check("create with awkward text is accepted", not err and res and res["status"] == 201 and key, (err, res))
    check("select leaves the header noise out", res and not res.get("headers"), res and res.get("headers"))
    err, res, _ = mcp.call("send_request", url=f"{BASE}/issue/{key}", headers=auth, select=["$.fields.description"])
    check("the text came back exactly", res and res["selected"]["$.fields.description"] == text, res)

    # 2. optional fields: an unquoted placeholder is not a JSON value, so an agent sends `body` with a JSON
    #    content type; left out when there is no value, sent (as a real array) when there is
    optional = '{"fields": {"summary": "S", "assignee": {"name": "{{who?}}"}, "labels": {{labels?}}, "epic": "{{epic?}}"}}'
    json_headers = dict(auth, **{"Content-Type": "application/json"})
    err, bare, _ = mcp.call("send_request", url="{{base}}/issue", method="POST", headers=json_headers, body=optional)
    check("optional fields are left out (an empty assignee would be a 400)", not err and bare and bare["status"] == 201, bare)
    err, full, _ = mcp.call("send_request", url="{{base}}/issue", method="POST", headers=json_headers, body=optional,
                            variables={"who": "ann", "labels": '["a", "b"]', "epic": "E-1"})
    check("optional fields are sent when given", not err and full and full["status"] == 201, full)
    err, got, _ = mcp.call("send_request", url=f"{BASE}/issue/{full['json']['key']}", headers=auth,
                           select=["$.fields.labels", "$.fields.assignee.displayName"])
    check("the array and the assignee arrived", got and got["selected"] == {"$.fields.labels": ["a", "b"], "$.fields.assignee.displayName": "ann"}, got)

    # 3. a null parent is null, not a problem
    err, un, _ = mcp.call("send_request", url=f"{BASE}/issue/{bare['json']['key']}", headers=auth, select=["$.fields.assignee.displayName"])
    check("a path below a null is null with no problem", un and un["selected"] == {"$.fields.assignee.displayName": None} and not un.get("problems"), un)

    # 4. an error page that is not JSON: the whole response comes back, once
    err, boom, _ = mcp.call("send_request", url=f"{BASE}/boom", headers=auth, select=["$.key"])
    check("a failed select returns the body and says why", boom and "Internal Server Error" in (boom.get("body") or "") and boom.get("problems"), boom)

    # 5. a saved request keeps its timeout and select
    mcp.call("save_request", name="create issue", url="{{base}}/issue", method="POST", headers=auth,
             json={"fields": {"summary": "{{summary}}"}}, timeout_secs=90, select=["$.key"])
    err, shown, _ = mcp.call("get_saved_request", name="create issue")
    check("saved request shows its options", shown and shown.get("options", {}).get("timeout_secs") == 90 and shown["options"].get("select") == ["$.key"], shown)
    err, again, _ = mcp.call("send_request", saved_request="create issue", variables={"summary": "From a saved request"})
    check("sending it by name uses the saved select", again and again.get("selected", {}).get("$.key", "").startswith("PRJ-"), again)
    err, dropped, _ = mcp.call("save_request", name="with extras", url=f"{BASE}/search?jql=a", variables={"x": "1"}, extract=[{"name": "n", "from": "status"}])
    check("what a save cannot keep is named", dropped and any("variables" in s for s in dropped.get("not_saved", [])), dropped)

    # 6. a workflow: find a transition by name, apply it, accept a 404, skip a step, clean up
    steps = [
        {"label": "transitions", "url": "{{base}}/issue/{{key}}/transitions", "headers": auth,
         "extract": [{"name": "transition_id", "from": "json:$.transitions[?(@.name=='Done')].id", "scope": "run"}]},
        {"label": "move", "method": "POST", "url": "{{base}}/issue/{{key}}/transitions", "headers": auth,
         "json": {"transition": {"id": "{{transition_id}}"}}, "expect_status": 204},
        {"label": "look for a missing one", "url": "{{base}}/issue/PRJ-999", "headers": auth,
         "expect_status": [200, 404], "extract": [{"name": "keep_me", "from": "status"}]},
        {"label": "create it only if it was found", "method": "POST", "url": "{{base}}/issue", "headers": auth,
         "json": {"fields": {"summary": "never"}}, "skip_if": "{{previous_status}} == 404"},
    ]
    err, saved, _ = mcp.call("save_workflow", name="close issue", steps=steps)
    check("the workflow is saved", not err, saved)
    err, result, _ = mcp.call("run_workflow", name="close issue", variables={"key": key}, cleanup=True)
    check("workflow ran: transition by name, 404 accepted, create skipped",
          result and result.get("ok") and result["steps"][3].get("skipped") and result["steps"][1]["status"] == 204, result)
    err, status, _ = mcp.call("send_request", url=f"{BASE}/issue/{key}", headers=auth, select=["$.fields.status.name"])
    check("the issue is Done", status and status["selected"]["$.fields.status.name"] == "Done", status)
    err, listed, _ = mcp.call("list_variables")
    names = [v["name"] for v in (listed or {}).get("variables", [])]
    check("cleanup left no workflow variables behind", "keep_me" not in names and "transition_id" not in names, names)

    # 6b. groups: filed on save, found by group (and what is under it), moved without re-sending
    mcp.call("save_request", name="jira search", url=f"{BASE}/search?jql=a", headers=auth, group="Jira / Issues")
    mcp.call("save_request", name="gitlab ping", url="https://gitlab.example.com/ping", group="GitLab")
    err, listed, _ = mcp.call("list_saved_requests", group="Jira")
    found = [r["name"] for r in (listed or {}).get("requests", [])]
    check("the list can be narrowed to a group and what is under it", found == ["jira search"], found)
    err, moved, _ = mcp.call("set_group", kind="request", name="gitlab ping", group="Ops")
    check("set_group moves a saved request", not err and moved and moved["group"] == "Ops", (err, moved))
    err, shown, _ = mcp.call("get_saved_request", name="jira search")
    check("get_saved_request says the group", shown and shown.get("group") == "Jira/Issues", shown)
    err, res, text = mcp.call("save_request", name="bad group", url=f"{BASE}/x", group="a//b")
    check("a bad group name is refused and nothing is saved", err and "group" in text, text)
    mcp.call("save_workflow", name="grouped flow", steps=[{"url": f"{BASE}/search?jql=a", "headers": auth}], group="Jira")
    err, wl, _ = mcp.call("list_workflows", group="Jira")
    check("workflows are filed and filtered by group", wl and [w["name"] for w in wl["workflows"]] == ["grouped flow"], wl)

    # 7. what a careless agent sends is either understood or refused by name, never silently dropped
    err, res, text = mcp.call("send_request", url=f"{BASE}/search?jql=a", headers=[{"name": "Authorization", "value": "Bearer tok_tracker"}])
    check("headers as a list of name/value objects are understood", not err and res and res["status"] == 200, text)
    err, res, text = mcp.call("send_request", url=f"{BASE}/search?jql=a", headers=["Authorization: Bearer tok_tracker"])
    check("headers as 'Name: value' strings are understood", not err and res and res["status"] == 200, text)
    err, res, text = mcp.call("send_request", url="{{base}}/issue", method="POST", headers=auth, json='{"fields": {"summary": "from a string"}}')
    check("json given as a string is sent as that JSON", not err and res and res["status"] == 201, (err, text))
    err, res, text = mcp.call("send_request", url=f"{BASE}/search?jql=a", headers=auth, retries=3)
    check("an unknown field is refused by name", err and "retries" in text, text)
    err, res, text = mcp.call("send_request", url=f"{BASE}/search?jql=a", headers={"X-A": "1\r\nX-B: 2"})
    check("a header with a line break is refused", err and "line break" in text, text)
    err, res, text = mcp.call("send_request", url="{{base}}/search", headers=dict(auth, **{"X-Trace": "{{trace}}"}), variables={"trace": "ok\nX-Evil: 1"})
    check("a value with a line break cannot become another header", err and "line break" in text, text)
    err, res, text = mcp.call("send_request", url="{{nope}}/x", headers=auth)
    check("an undefined variable says what an agent can do about it", err and "variables" in text and "Tools >" not in text, text)
    err, res, text = mcp.call("send_request", url=f"{BASE}/issue", method="POST", headers=auth, form={"file": "@C:/secret.txt"})
    check("a form value that is a file path is refused, not sent as text", err and "upload" in text, text)
    err, res, text = mcp.call("send_request", url=f"{BASE}/search?jql=a", headers=auth, timeout_secs=0, method="FETCH")
    check("an adjusted timeout and an odd method are noted", res and len(res.get("problems", [])) >= 2, res and res.get("problems"))

    # 7b. what comes back: ttfb, where a redirect went, a binary checked without its bytes, which headers
    err, moved, _ = mcp.call("send_request", url=f"{BASE}/old", headers=auth)
    check("a redirect says where it went", moved and moved.get("final_url", "").endswith("/search?jql=moved")
          and [r["status"] for r in moved.get("redirects", [])] == [301] and moved.get("ttfb_ms") is not None, moved)
    err, plain, _ = mcp.call("send_request", url=f"{BASE}/search?jql=a", headers=auth)
    check("no redirect fields when there was none", plain and "redirects" not in plain and "final_url" not in plain, plain)
    err, relevant, _ = mcp.call("send_request", url=f"{BASE}/old", headers=auth, include_headers="relevant", follow_redirects=False)
    names = [h["name"].lower() for h in (relevant or {}).get("headers", [])]
    check("include_headers: relevant keeps location and drops the rest", "location" in names and "server" not in names and "date" not in names, names)
    err, blob, _ = mcp.call("send_request", url=f"{BASE}/blob", headers=auth)
    expected = hashlib.sha256(b"\x89PNG\r\n\x1a\n" + bytes(64)).hexdigest()
    check("a binary body is checked, not returned", blob and blob.get("binary") and blob.get("sha256") == expected
          and blob.get("detected_type") == "png" and "json" not in blob and "body" not in blob, blob)

    # 8. the audit log: every change is in it, in order, and tampering with it is found
    err, _, text = mcp.call("set_variable", name="scenario_token", value="S3CR3T-VALUE-123")
    # Linux has no credential store yet, so a secret cannot be kept there (and is refused, not stored in the open)
    no_store = bool(err and "credential store" in text)
    mcp.call("send_request", url=f"{BASE}/search?jql=a", headers=dict(auth, **{"X-Secret": "{{scenario_token}}"}))
    if mcp:
        mcp.proc.terminate()
        mcp.proc.wait()
        mcp = None

    def plunger(*args):
        r = subprocess.run([PLUNGER, *args], capture_output=True, text=True, encoding="utf-8",
                           env=dict(os.environ, PLUNGER_DATA_DIR=DATA), timeout=60)
        try:
            return r.returncode, json.loads(r.stdout)
        except ValueError:
            return r.returncode, {"_raw": r.stdout}

    out_file = os.path.join(DATA, "body.out")
    code, written = plunger("send", "--url", f"{BASE}/search?jql=whole", "-H", "Authorization: Bearer tok_tracker", "--out", out_file)
    check("--out writes the whole body to a file", code == 0 and written.get("body_file") == out_file
          and "whole" in open(out_file, encoding="utf-8").read() and "json" not in written, written)
    code, verified = plunger("audit", "verify")
    check("the audit log verifies", code == 0 and verified.get("ok") and verified.get("entries", 0) > 20, verified)
    code, shown = plunger("audit", "show", "--source", "mcp", "--action", "send", "--limit", "500")
    sends = shown.get("entries", [])
    check("every send an agent made is in the log, with who sent it", len(sends) >= 10 and all(e["source"] == "mcp" for e in sends), len(sends))
    log_text = open(os.path.join(DATA, "audit.log"), encoding="utf-8").read()
    check("no secret value is in the log", "S3CR3T-VALUE-123" not in log_text and (no_store or "scenario_token" in log_text), text)

    # the database can be rebuilt from the log
    for name in os.listdir(DATA):
        if name.startswith("history.sqlite3"):
            os.remove(os.path.join(DATA, name))
    code, saved_again = plunger("saved")
    names = [r.get("name") for r in (saved_again.get("requests") if isinstance(saved_again, dict) else saved_again) or []]
    check("the saved requests come back from the log after the database is deleted", "create issue" in names, names)

    # and an edited line is found, with its number
    lines = log_text.splitlines()
    index = next(i for i, l in enumerate(lines) if '"action":"request_added"' in l and '"status":201' in l)
    lines[index] = lines[index].replace('"status":201', '"status":500')
    with open(os.path.join(DATA, "audit.log"), "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    code, broken = plunger("audit", "verify")
    check("an edited line is found and named", code == 4 and broken.get("ok") is False and broken.get("broken_at") == index + 1, broken)
finally:
    if mcp:
        mcp.proc.terminate()
    server.terminate()

print()
if failures:
    print(f"{len(failures)} check(s) failed: {failures}")
    sys.exit(1)
print("all checks passed")
